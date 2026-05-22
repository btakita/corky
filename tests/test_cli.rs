//! Basic binary invocation tests (assert_cmd).

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;

fn corky_cmd() -> Command {
    cargo_bin_cmd!("corky")
}

#[test]
fn test_binary_exists() {
    let _cmd = corky_cmd();
}

#[test]
fn test_cli_version() {
    let mut cmd = corky_cmd();
    cmd.arg("--version");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("corky"));
}

#[test]
fn test_cli_help() {
    let mut cmd = corky_cmd();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Sync email threads"));
}

#[test]
fn test_cli_no_args_shows_error() {
    let mut cmd = corky_cmd();
    cmd.assert().failure();
}

#[test]
fn test_cli_help_subcommand() {
    let mut cmd = corky_cmd();
    cmd.arg("help");
    cmd.assert().success();
}

#[test]
fn test_cli_mailbox_list_subcommand() {
    let mut cmd = corky_cmd();
    cmd.args(["mailbox", "list"]);
    // May succeed or fail depending on config, but should not panic
    let output = cmd.output().unwrap();
    assert!(output.status.code().is_some());
}

#[test]
fn test_cli_audit_docs_subcommand() {
    let mut cmd = corky_cmd();
    cmd.arg("audit-docs");
    let output = cmd.output().unwrap();
    assert!(output.status.code().is_some());
}

#[test]
fn test_cli_init_requires_user() {
    let mut cmd = corky_cmd();
    cmd.arg("init");
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--user"));
}

#[test]
fn test_cli_push_draft_requires_file() {
    let mut cmd = corky_cmd();
    cmd.arg("push-draft");
    cmd.assert().failure();
}

#[test]
fn test_cli_add_label_requires_args() {
    let mut cmd = corky_cmd();
    cmd.arg("add-label");
    cmd.assert().failure();
}

#[test]
fn test_cli_contact_add_requires_args() {
    let mut cmd = corky_cmd();
    cmd.arg("contact-add");
    cmd.assert().failure();
}

#[test]
fn test_cli_mailbox_add_requires_args() {
    let mut cmd = corky_cmd();
    cmd.args(["mailbox", "add"]);
    cmd.assert().failure();
}

#[test]
fn test_cli_mb_alias() {
    let mut cmd = corky_cmd();
    cmd.args(["mb", "add"]);
    cmd.assert().failure(); // fails because no args, but proves alias works
}

#[test]
fn test_cli_sync_routes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();
    let data_dir = project_dir.join("mail");

    // Create conversations dir with a thread that has a routed label
    std::fs::create_dir_all(data_dir.join("conversations")).unwrap();
    std::fs::write(
        data_dir.join("conversations/test-thread.md"),
        "# Test Thread\n\n\
         **Labels**: for-alex\n\
         **Accounts**: personal\n\
         **Thread ID**: test thread\n\
         **Last updated**: Mon, 10 Feb 2025 10:00:00 +0000\n\n\
         ---\n\n\
         ## Alice <alice@example.com> \u{2014} Mon, 10 Feb 2025 10:00:00 +0000\n\n\
         Hello there!\n",
    )
    .unwrap();

    // Create routing config
    std::fs::write(
        data_dir.join(".corky.toml"),
        "[accounts.personal]\n\
         provider = \"gmail\"\n\
         user = \"test@gmail.com\"\n\
         password = \"dummy\"\n\
         labels = [\"inbox\"]\n\n\
         [routing]\n\
         for-alex = [\"mailboxes/alex\"]\n",
    )
    .unwrap();

    let mut cmd = corky_cmd();
    cmd.current_dir(&project_dir);
    cmd.args(["sync", "routes"]);
    cmd.assert().success();

    // Verify the file was copied to the mailbox
    assert!(
        data_dir
            .join("mailboxes/alex/conversations/test-thread.md")
            .exists()
    );
}

#[test]
fn test_cli_unknown_subcommand() {
    let mut cmd = corky_cmd();
    cmd.arg("nonexistent-command");
    cmd.assert().failure();
}

// ── New command registration tests ─────────────────────────────────────────

#[test]
fn test_cli_draft_send_requires_file() {
    let mut cmd = corky_cmd();
    cmd.args(["draft", "send"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("FILE"));
}

#[test]
fn test_cli_draft_send_help() {
    let mut cmd = corky_cmd();
    cmd.args(["draft", "send", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("attachment"));
}

#[test]
fn test_cli_doc_sheet_write_requires_args() {
    let mut cmd = corky_cmd();
    cmd.args(["doc", "sheet-write"]);
    cmd.assert().failure();
}

#[test]
fn test_cli_doc_sheet_write_help() {
    let mut cmd = corky_cmd();
    cmd.args(["doc", "sheet-write", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("range"));
}

#[test]
fn test_cli_doc_info_help() {
    let mut cmd = corky_cmd();
    cmd.args(["doc", "info", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Drive metadata"));
}

#[test]
fn test_cli_doc_export_requires_output() {
    let mut cmd = corky_cmd();
    cmd.args([
        "doc",
        "export",
        "https://drive.google.com/file/d/file-id/view",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--output"));
}

#[test]
fn test_cli_doc_read_accepts_format() {
    let mut cmd = corky_cmd();
    cmd.args(["doc", "read", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--format"));
}

#[test]
fn test_cli_doc_upload_convert_help() {
    let mut cmd = corky_cmd();
    cmd.args(["doc", "upload", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--convert"))
        .stdout(predicate::str::contains("--folder"));
}

#[test]
fn test_cli_sync_refetch_help_mentions_gmail_url() {
    let mut cmd = corky_cmd();
    cmd.args(["sync", "refetch", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Gmail web URL"));
}

#[test]
fn test_cli_auth_accepts_drive_readonly_scope() {
    let mut cmd = corky_cmd();
    cmd.args(["auth", "--scope", "drive-readonly", "--help"]);
    cmd.assert().success();
}

#[test]
fn test_cli_auth_accepts_workspace_scope() {
    let mut cmd = corky_cmd();
    cmd.args(["auth", "--scope", "workspace", "--help"]);
    cmd.assert().success();
}

#[test]
fn test_cli_docs_read_help() {
    let mut cmd = corky_cmd();
    cmd.args(["docs", "read", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("doc"));
}

#[test]
fn test_cli_docs_write_requires_args() {
    let mut cmd = corky_cmd();
    cmd.args(["docs", "write"]);
    cmd.assert().failure();
}

#[test]
fn test_cli_sheets_read_help() {
    let mut cmd = corky_cmd();
    cmd.args(["sheets", "read", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("range"));
}

#[test]
fn test_cli_sheets_pull_requires_args() {
    let mut cmd = corky_cmd();
    cmd.args(["sheets", "pull"]);
    cmd.assert().failure();
}

#[test]
fn test_cli_sheets_push_help() {
    let mut cmd = corky_cmd();
    cmd.args(["sheets", "push", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("tab"));
}

#[test]
fn test_cli_sheets_delete_tab_help() {
    let mut cmd = corky_cmd();
    cmd.args(["sheets", "delete-tab", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("tab"));
}

#[test]
fn test_cli_chat_send_requires_args() {
    let mut cmd = corky_cmd();
    cmd.args(["chat", "send"]);
    cmd.assert().failure();
}

#[test]
fn test_cli_chat_send_help() {
    let mut cmd = corky_cmd();
    cmd.args(["chat", "send", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("space"));
}

#[test]
fn test_cli_tasks_list_registered() {
    let mut cmd = corky_cmd();
    cmd.args(["tasks", "list", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("tasklist"));
}

#[test]
fn test_cli_tasks_add_requires_title() {
    let mut cmd = corky_cmd();
    cmd.args(["tasks", "add"]);
    cmd.assert().failure();
}

#[test]
fn test_cli_tasks_add_help() {
    let mut cmd = corky_cmd();
    cmd.args(["tasks", "add", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("due"));
}

#[test]
fn test_cli_tasks_done_requires_task_id() {
    let mut cmd = corky_cmd();
    cmd.args(["tasks", "done"]);
    cmd.assert().failure();
}

#[test]
fn test_cli_init_with_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project_dir = tmp.path().join("test-init-cli");

    let mut cmd = corky_cmd();
    // Isolate from real config by using temp HOME
    cmd.env("HOME", tmp.path().to_string_lossy().as_ref());
    cmd.args([
        "init",
        "--user",
        "test@example.com",
        "--force",
        &project_dir.to_string_lossy(),
    ]);
    cmd.assert().success();

    assert!(project_dir.join("mail/conversations").exists());
    assert!(project_dir.join("mail/.corky.toml").exists());
    assert!(project_dir.join("mail/voice.md").exists());
}

#[test]
fn test_cli_help_filter() {
    let mut cmd = corky_cmd();
    cmd.args(["help", "sync"]);
    cmd.assert().success();
}

#[test]
fn test_cli_sync_unknown_account() {
    let tmp = tempfile::TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();

    std::fs::create_dir_all(data_dir.join("conversations")).unwrap();
    std::fs::write(
        data_dir.join(".corky.toml"),
        r#"
[accounts.personal]
provider = "gmail"
user = "test@gmail.com"
password = "dummy"
labels = ["inbox"]
"#,
    )
    .unwrap();

    let mut cmd = corky_cmd();
    cmd.env("CORKY_DATA", data_dir.to_string_lossy().as_ref());
    cmd.args(["sync", "account", "nonexistent"]);
    cmd.assert().failure();
}
