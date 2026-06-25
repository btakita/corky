use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub fn load_json_or_default<T>(path: &Path) -> Result<T>
where
    T: DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&data).with_context(|| format!("parsing {}", path.display()))
}

pub fn save_json_with_lock<T, F>(path: &Path, permissions: Option<u32>, merge: F) -> Result<()>
where
    T: Serialize + DeserializeOwned + Default,
    F: FnOnce(T) -> Result<T>,
{
    let _lock = FileLock::acquire(&lock_path(path))?;
    let current = load_json_or_default(path)?;
    let next = merge(current)?;
    write_json_atomic(path, &next, permissions)
}

fn write_json_atomic<T>(path: &Path, value: &T, permissions: Option<u32>) -> Result<()>
where
    T: Serialize,
{
    let serialized = serde_json::to_vec(value)?;
    atomic_write(path, &serialized, permissions)
}

/// Atomically write `contents` to `path`: write to a temp file in the same
/// directory, `fsync` it, then rename into place (#ckyatomicwrite).
///
/// The `sync_all` before the rename is what makes this power-loss durable —
/// without it the rename can be visible while the file's data blocks are not yet
/// flushed, exposing a truncated/empty file after a crash. Use this instead of a
/// plain `fs::write` for any file whose partial contents would corrupt state
/// (conversation markdown, JSON stores). Optionally sets unix permissions on the
/// temp file before the rename.
pub fn atomic_write(path: &Path, contents: &[u8], permissions: Option<u32>) -> Result<()> {
    #[cfg(not(unix))]
    let _ = permissions;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", path.display()))?;
    let mut tmp = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temp file in {}", parent.display()))?;
    tmp.write_all(contents)
        .with_context(|| format!("writing temp file for {}", path.display()))?;
    tmp.flush()
        .with_context(|| format!("flushing temp file for {}", path.display()))?;
    tmp.as_file()
        .sync_all()
        .with_context(|| format!("syncing temp file for {}", path.display()))?;

    #[cfg(unix)]
    if let Some(mode) = permissions {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("setting permissions on {}", tmp.path().display()))?;
    }

    tmp.persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("persisting {}", path.display()))?;
    Ok(())
}

fn lock_path(path: &Path) -> PathBuf {
    let mut lock_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "store".into());
    lock_name.push(".lock");
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(lock_name)
}

struct FileLock {
    file: File,
}

impl FileLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("opening lock file {}", path.display()))?;
        lock_exclusive(&file).with_context(|| format!("locking {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unlock(&self.file);
    }
}

#[cfg(unix)]
fn lock_exclusive(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn unlock(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn lock_exclusive(_file: &File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn unlock(_file: &File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    struct TestState {
        value: u32,
    }

    #[test]
    fn atomic_write_creates_and_overwrites_full_contents() {
        // #ckyatomicwrite: write lands fully; a second write fully replaces it
        // (no truncation/leftover bytes), and nested parent dirs are created.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/conv.md");
        atomic_write(&path, b"# Thread\n\nfirst message\n", None).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# Thread\n\nfirst message\n"
        );
        // Overwrite with shorter content — must not leave trailing bytes.
        atomic_write(&path, b"short", None).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "short");
    }

    #[test]
    fn save_json_with_lock_merges_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, r#"{"value":1}"#).unwrap();

        save_json_with_lock::<TestState, _>(&path, None, |mut current| {
            current.value += 2;
            Ok(current)
        })
        .unwrap();

        let state: TestState = load_json_or_default(&path).unwrap();
        assert_eq!(state, TestState { value: 3 });
    }
}
