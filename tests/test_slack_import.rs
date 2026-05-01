//! CLI integration tests for `corky slack import`.

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::io::Write;
use tempfile::TempDir;

fn corky_cmd() -> Command {
    cargo_bin_cmd!("corky")
}

/// Build a minimal Slack export ZIP in memory.
fn build_slack_zip(tmp: &std::path::Path) -> std::path::PathBuf {
    let zip_path = tmp.join("slack-export.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();

    // users.json
    zip.start_file("users.json", options).unwrap();
    zip.write_all(
        br#"[
        {"id": "U001", "name": "alice", "profile": {"real_name": "Alice Smith"}},
        {"id": "U002", "name": "bob", "profile": {"real_name": "Bob Jones"}}
    ]"#,
    )
    .unwrap();

    // channels.json
    zip.start_file("channels.json", options).unwrap();
    zip.write_all(
        br#"[
        {"id": "C001", "name": "general"}
    ]"#,
    )
    .unwrap();

    // general/2024-06-15.json
    zip.start_file("general/2024-06-15.json", options).unwrap();
    zip.write_all(
        br#"[
        {
            "type": "message",
            "user": "U001",
            "text": "Hello from Slack CLI test",
            "ts": "1718438400.000100"
        },
        {
            "type": "message",
            "user": "U002",
            "text": "Reply in Slack CLI test",
            "ts": "1718438460.000200",
            "thread_ts": "1718438400.000100"
        }
    ]"#,
    )
    .unwrap();

    zip.finish().unwrap();
    zip_path
}

#[test]
fn test_slack_import_missing_path() {
    let mut cmd = corky_cmd();
    cmd.args(["slack", "import"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("PATH"));
}

#[test]
fn test_slack_import_nonexistent_file() {
    let mut cmd = corky_cmd();
    cmd.args(["slack", "import", "/tmp/nonexistent-slack-export.zip"]);
    cmd.assert().failure();
}

#[test]
fn test_slack_import_full_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();
    let data_dir = project_dir.join("mail");
    let conv_dir = data_dir.join("conversations");
    std::fs::create_dir_all(&conv_dir).unwrap();
    std::fs::write(data_dir.join(".corky.toml"), "").unwrap();

    let zip_path = build_slack_zip(tmp.path());

    let mut cmd = corky_cmd();
    cmd.current_dir(&project_dir);
    cmd.env("CORKY_DATA", data_dir.to_string_lossy().as_ref());
    cmd.args(["slack", "import", &zip_path.to_string_lossy()]);
    cmd.assert().success();

    // Should have created conversation file(s)
    let files: Vec<_> = std::fs::read_dir(&conv_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    assert!(
        !files.is_empty(),
        "Should have created at least one conversation file"
    );

    // Check content of the first file
    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(content.contains("**Accounts**: slack"));
    assert!(content.contains("Hello from Slack CLI test"));
}

#[test]
fn test_slack_import_custom_account() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();
    let data_dir = project_dir.join("mail");
    let conv_dir = data_dir.join("conversations");
    std::fs::create_dir_all(&conv_dir).unwrap();
    std::fs::write(data_dir.join(".corky.toml"), "").unwrap();

    let zip_path = build_slack_zip(tmp.path());

    let mut cmd = corky_cmd();
    cmd.current_dir(&project_dir);
    cmd.env("CORKY_DATA", data_dir.to_string_lossy().as_ref());
    cmd.args([
        "slack",
        "import",
        &zip_path.to_string_lossy(),
        "--account",
        "slack-work",
    ]);
    cmd.assert().success();

    let files: Vec<_> = std::fs::read_dir(&conv_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    assert!(!files.is_empty());

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(content.contains("**Accounts**: slack-work"));
}

#[test]
fn test_slack_import_custom_label() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();
    let data_dir = project_dir.join("mail");
    let conv_dir = data_dir.join("conversations");
    std::fs::create_dir_all(&conv_dir).unwrap();
    std::fs::write(data_dir.join(".corky.toml"), "").unwrap();

    let zip_path = build_slack_zip(tmp.path());

    let mut cmd = corky_cmd();
    cmd.current_dir(&project_dir);
    cmd.env("CORKY_DATA", data_dir.to_string_lossy().as_ref());
    cmd.args([
        "slack",
        "import",
        &zip_path.to_string_lossy(),
        "--label",
        "work-slack",
    ]);
    cmd.assert().success();

    let files: Vec<_> = std::fs::read_dir(&conv_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    assert!(!files.is_empty());

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(content.contains("**Labels**: work-slack"));
}
