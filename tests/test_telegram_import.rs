//! CLI integration tests for `corky sync telegram-import`.

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use tempfile::TempDir;

fn corky_cmd() -> Command {
    cargo_bin_cmd!("corky")
}

#[test]
fn test_telegram_import_missing_path() {
    let mut cmd = corky_cmd();
    cmd.args(["sync", "telegram-import"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("PATH"));
}

#[test]
fn test_telegram_import_nonexistent_file() {
    let mut cmd = corky_cmd();
    cmd.args(["sync", "telegram-import", "/tmp/nonexistent-tg-export.json"]);
    cmd.assert().failure();
}

#[test]
fn test_telegram_import_single_chat() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();
    let data_dir = project_dir.join("mail");
    let conv_dir = data_dir.join("conversations");
    std::fs::create_dir_all(&conv_dir).unwrap();
    std::fs::write(data_dir.join(".corky.toml"), "").unwrap();

    let json = r#"{
        "name": "CLI Test Chat",
        "type": "personal_chat",
        "id": 77777,
        "messages": [
            {
                "id": 1,
                "type": "message",
                "date": "2024-06-15T10:00:00",
                "from": "Alice",
                "text": "Hello from CLI test"
            },
            {
                "id": 2,
                "type": "message",
                "date": "2024-06-15T10:01:00",
                "from": "Bob",
                "text": "Reply from CLI test"
            }
        ]
    }"#;

    let json_path = tmp.path().join("result.json");
    std::fs::write(&json_path, json).unwrap();

    let mut cmd = corky_cmd();
    cmd.current_dir(&project_dir);
    cmd.env("CORKY_DATA", data_dir.to_string_lossy().as_ref());
    cmd.args(["sync", "telegram-import", &json_path.to_string_lossy()]);
    cmd.assert().success();

    // Verify output file was created
    let files: Vec<_> = std::fs::read_dir(&conv_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    assert_eq!(files.len(), 1);

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(content.contains("# CLI Test Chat"));
    assert!(content.contains("**Thread ID**: tg:77777"));
    assert!(content.contains("**Accounts**: telegram"));
    assert!(content.contains("Hello from CLI test"));
    assert!(content.contains("Reply from CLI test"));
}

#[test]
fn test_telegram_import_custom_account() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();
    let data_dir = project_dir.join("mail");
    let conv_dir = data_dir.join("conversations");
    std::fs::create_dir_all(&conv_dir).unwrap();
    std::fs::write(data_dir.join(".corky.toml"), "").unwrap();

    let json = r#"{
        "name": "Account Test",
        "type": "personal_chat",
        "id": 88888,
        "messages": [
            {
                "id": 1,
                "type": "message",
                "date": "2024-06-15T10:00:00",
                "from": "Alice",
                "text": "Testing custom account"
            }
        ]
    }"#;

    let json_path = tmp.path().join("result.json");
    std::fs::write(&json_path, json).unwrap();

    let mut cmd = corky_cmd();
    cmd.current_dir(&project_dir);
    cmd.env("CORKY_DATA", data_dir.to_string_lossy().as_ref());
    cmd.args([
        "sync",
        "telegram-import",
        &json_path.to_string_lossy(),
        "--account",
        "tg-personal",
    ]);
    cmd.assert().success();

    let files: Vec<_> = std::fs::read_dir(&conv_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    assert_eq!(files.len(), 1);

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(content.contains("**Accounts**: tg-personal"));
}

#[test]
fn test_telegram_import_directory() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();
    let data_dir = project_dir.join("mail");
    let conv_dir = data_dir.join("conversations");
    std::fs::create_dir_all(&conv_dir).unwrap();
    std::fs::write(data_dir.join(".corky.toml"), "").unwrap();

    let export_dir = tmp.path().join("exports");
    std::fs::create_dir_all(&export_dir).unwrap();

    let json = r#"{
        "name": "Dir Import Chat",
        "type": "personal_chat",
        "id": 55555,
        "messages": [
            {
                "id": 1,
                "type": "message",
                "date": "2024-01-01T12:00:00",
                "from": "Charlie",
                "text": "From directory import"
            }
        ]
    }"#;
    std::fs::write(export_dir.join("result.json"), json).unwrap();

    let mut cmd = corky_cmd();
    cmd.current_dir(&project_dir);
    cmd.env("CORKY_DATA", data_dir.to_string_lossy().as_ref());
    cmd.args(["sync", "telegram-import", &export_dir.to_string_lossy()]);
    cmd.assert().success();

    let files: Vec<_> = std::fs::read_dir(&conv_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    assert_eq!(files.len(), 1);

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(content.contains("From directory import"));
}
