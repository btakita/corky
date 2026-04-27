//! Model download and cache management.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Known whisper.cpp ggml model variants.
const MODELS: &[(&str, &str)] = &[
    ("tiny", "ggml-tiny.bin"),
    ("tiny.en", "ggml-tiny.en.bin"),
    ("base", "ggml-base.bin"),
    ("base.en", "ggml-base.en.bin"),
    ("small", "ggml-small.bin"),
    ("small.en", "ggml-small.en.bin"),
    ("medium", "ggml-medium.bin"),
    ("medium.en", "ggml-medium.en.bin"),
    ("large-v1", "ggml-large-v1.bin"),
    ("large-v2", "ggml-large-v2.bin"),
    ("large-v3", "ggml-large-v3.bin"),
    ("large-v3-turbo", "ggml-large-v3-turbo.bin"),
];

const HF_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// Base URL for pyannote-rs ONNX models.
#[cfg(feature = "diarize")]
const ONNX_MODELS_BASE: &str =
    "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0";

/// Resolve a model name to a local file path.
/// Downloads from HuggingFace if not cached.
pub fn resolve_model(name: &str, cache_dir: Option<&str>) -> Result<PathBuf> {
    let filename = model_filename(name)?;
    let cache = cache_directory(cache_dir)?;
    let model_path = cache.join(filename);

    if model_path.exists() {
        let size = std::fs::metadata(&model_path)?.len();
        eprintln!(
            "Model: {} ({:.1} GB)",
            model_path.display(),
            size as f64 / 1e9
        );
        return Ok(model_path);
    }

    download_model(name, filename, &cache, &model_path)
}

fn model_filename(name: &str) -> Result<&'static str> {
    // Allow passing a direct file path
    if Path::new(name).exists() {
        return Ok(""); // sentinel — caller uses the raw path
    }

    for (model_name, filename) in MODELS {
        if *model_name == name {
            return Ok(filename);
        }
    }

    let known: Vec<&str> = MODELS.iter().map(|(n, _)| *n).collect();
    bail!(
        "Unknown model '{}'. Known models: {}",
        name,
        known.join(", ")
    );
}

fn cache_directory(custom: Option<&str>) -> Result<PathBuf> {
    let dir = if let Some(path) = custom {
        let expanded = shellexpand(path);
        PathBuf::from(expanded)
    } else if let Some(cache) = dirs_cache_dir() {
        cache.join("corky").join("models")
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".cache")
            .join("corky")
            .join("models")
    };

    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn dirs_cache_dir() -> Option<PathBuf> {
    // Use XDG_CACHE_HOME or ~/.cache
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg));
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".cache"))
}

fn shellexpand(path: &str) -> String {
    if path.starts_with("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return format!("{}{}", home, &path[1..]);
    }
    path.to_string()
}

fn download_model(name: &str, filename: &str, cache: &Path, model_path: &Path) -> Result<PathBuf> {
    let url = format!("{}/{}", HF_BASE_URL, filename);
    download_url(&url, name, filename, cache, model_path)
}

/// Download a file from a URL to the cache directory.
fn download_url(
    url: &str,
    label: &str,
    filename: &str,
    cache: &Path,
    model_path: &Path,
) -> Result<PathBuf> {
    eprintln!("Downloading '{}'...", label);
    eprintln!("  URL: {}", url);
    eprintln!("  Saving to: {}", model_path.display());

    let response = ureq::get(url).call()?;
    let total = response
        .header("Content-Length")
        .and_then(|h| h.parse::<u64>().ok());

    if let Some(size) = total {
        if size > 1_000_000_000 {
            eprintln!("  Size: {:.1} GB", size as f64 / 1e9);
        } else {
            eprintln!("  Size: {:.1} MB", size as f64 / 1e6);
        }
    }

    // Download to a temp file first, then rename (atomic)
    let tmp_path = cache.join(format!("{}.download", filename));
    {
        let mut reader = response.into_reader();
        let mut file = std::fs::File::create(&tmp_path)?;
        let mut downloaded: u64 = 0;
        let mut buf = vec![0u8; 8 * 1024 * 1024]; // 8MB buffer
        let mut last_report = std::time::Instant::now();

        loop {
            let n = std::io::Read::read(&mut reader, &mut buf)?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut file, &buf[..n])?;
            downloaded += n as u64;

            if last_report.elapsed().as_secs() >= 2 {
                if let Some(t) = total {
                    eprintln!("  {:.0}%", downloaded as f64 / t as f64 * 100.0);
                }
                last_report = std::time::Instant::now();
            }
        }
    }

    std::fs::rename(&tmp_path, model_path)?;
    eprintln!("  Download complete.");
    Ok(model_path.to_path_buf())
}

/// Resolve an ONNX model file, downloading from pyannote-rs releases if needed.
#[cfg(feature = "diarize")]
pub fn resolve_onnx_model(filename: &str, cache_dir: Option<&str>) -> Result<PathBuf> {
    let cache = cache_directory(cache_dir)?;
    let model_path = cache.join(filename);

    if model_path.exists() {
        let size = std::fs::metadata(&model_path)?.len();
        eprintln!(
            "ONNX model: {} ({:.1} MB)",
            model_path.display(),
            size as f64 / 1e6
        );
        return Ok(model_path);
    }

    let url = format!("{}/{}", ONNX_MODELS_BASE, filename);
    download_url(&url, filename, filename, &cache, &model_path)
}

/// Resolve model path — if the name is a direct file path, use it; otherwise resolve from cache.
pub fn resolve_model_path(name: &str, cache_dir: Option<&str>) -> Result<PathBuf> {
    let path = Path::new(name);
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    resolve_model(name, cache_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_filenames() {
        assert_eq!(
            model_filename("large-v3-turbo").unwrap(),
            "ggml-large-v3-turbo.bin"
        );
        assert_eq!(model_filename("tiny").unwrap(), "ggml-tiny.bin");
        assert_eq!(model_filename("base.en").unwrap(), "ggml-base.en.bin");
    }

    #[test]
    fn unknown_model_errors() {
        assert!(model_filename("nonexistent").is_err());
    }

    #[test]
    fn shellexpand_tilde() {
        // SAFETY: Test-only; no concurrent env access.
        unsafe { std::env::set_var("HOME", "/home/test") };
        assert_eq!(shellexpand("~/models"), "/home/test/models");
        assert_eq!(shellexpand("/abs/path"), "/abs/path");
    }
}
