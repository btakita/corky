//! Integration tests for `corky doctor` (src/doctor.rs).
//!
//! Each test sets HOME and CORKY_DATA to a temp dir to isolate from the real
//! config. Tests run serially via a shared mutex to avoid env var races.

mod common;

use std::sync::Mutex;
use tempfile::TempDir;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// Run doctor::check with HOME pointed at a temp dir so config/token resolution
/// is fully isolated.
fn run_doctor_isolated(tmp: &TempDir, provider: Option<&str>) -> corky::doctor::DoctorReport {
    let _lock = ENV_MUTEX.lock().unwrap();
    let old_home = std::env::var("HOME").ok();
    let old_data = std::env::var("CORKY_DATA").ok();
    // SAFETY: Tests run serially under ENV_MUTEX so no concurrent env access.
    unsafe {
        std::env::set_var("HOME", tmp.path().to_string_lossy().as_ref());
        // Point CORKY_DATA at the tmp dir so resolve::data_dir() finds it.
        std::env::set_var("CORKY_DATA", tmp.path().join("mail").to_string_lossy().as_ref());
    }
    let report = corky::doctor::check(provider);
    // Restore env.
    match old_home {
        Some(h) => unsafe { std::env::set_var("HOME", h) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match old_data {
        Some(d) => unsafe { std::env::set_var("CORKY_DATA", d) },
        None => unsafe { std::env::remove_var("CORKY_DATA") },
    }
    report
}

// ---------------------------------------------------------------------------
// Helper: write a .corky.toml at the location resolve::corky_toml() expects
// when CORKY_DATA is set to tmp/mail.
// ---------------------------------------------------------------------------
fn write_config(tmp: &TempDir, toml_content: &str) {
    let mail_dir = tmp.path().join("mail");
    std::fs::create_dir_all(mail_dir.join("conversations")).unwrap();
    std::fs::create_dir_all(mail_dir.join("drafts")).unwrap();
    std::fs::write(mail_dir.join(".corky.toml"), toml_content).unwrap();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_doctor_missing_config_does_not_panic() {
    let tmp = TempDir::new().unwrap();
    // No config file at all — should report failure without panicking.
    let report = run_doctor_isolated(&tmp, None);
    assert!(!report.ok);
    let output = report.lines.join("\n");
    assert!(output.contains('\u{2717}'), "expected a failure checkmark");
}

#[test]
fn test_doctor_empty_config() {
    let tmp = TempDir::new().unwrap();
    // Minimal valid TOML (empty config, no accounts).
    write_config(&tmp, "");
    let report = run_doctor_isolated(&tmp, None);
    let output = report.lines.join("\n");
    assert!(output.contains('\u{2713}'), "should have at least one success check");
    assert!(output.contains("Config loaded"), "should report config loaded");
    assert!(output.contains("Mail directory"), "should report mail directory");
}

#[test]
fn test_doctor_gmail_provider_filter() {
    let tmp = TempDir::new().unwrap();
    write_config(
        &tmp,
        r#"
[accounts.personal]
provider = "gmail"
user = "me@gmail.com"
password = "test"
imap_host = "imap.gmail.com"
imap_port = 993
labels = []
default = true

[accounts.work]
provider = "imap"
user = "me@work.com"
password = "test"
imap_host = "imap.work.com"
imap_port = 993
labels = []
default = false
"#,
    );

    let report = run_doctor_isolated(&tmp, Some("gmail"));
    let output = report.lines.join("\n");
    // Should show the gmail account.
    assert!(output.contains("personal"), "should show gmail account");
    // Should NOT show the imap account.
    assert!(!output.contains("Account: work"), "should not show imap account when filtering to gmail");
}

#[test]
fn test_doctor_gmail_api_account() {
    let tmp = TempDir::new().unwrap();
    write_config(
        &tmp,
        r#"
[accounts.main]
provider = "gmail-api"
user = "user@gmail.com"
labels = []
default = true
"#,
    );

    let report = run_doctor_isolated(&tmp, Some("gmail-api"));
    let output = report.lines.join("\n");
    assert!(output.contains("Account: main (gmail-api)"));
    assert!(output.contains("OAuth credentials resolved"));
    assert!(output.contains("User: user@gmail.com"));
    // No token in store, so should show missing.
    assert!(output.contains("No sync token found"));
}

#[test]
fn test_doctor_linkedin_section() {
    let tmp = TempDir::new().unwrap();
    write_config(
        &tmp,
        r#"
[linkedin]
client_id = "test-client-id"
client_secret = "test-secret"

[profiles.btakita.linkedin]
handle = "briantakita"
urn = "urn:li:person:abc123"
"#,
    );

    let report = run_doctor_isolated(&tmp, Some("linkedin"));
    let output = report.lines.join("\n");
    assert!(output.contains("LinkedIn:"), "should have LinkedIn section");
    assert!(
        output.contains("OAuth credentials configured (config)"),
        "should detect config creds"
    );
    assert!(
        output.contains("Profile 'btakita': handle=briantakita, urn=urn:li:person:abc123"),
        "should show profile info"
    );
    // No token in store — should show missing.
    assert!(
        output.contains("No LinkedIn token found"),
        "should report missing token"
    );
}

#[test]
fn test_doctor_linkedin_no_urn() {
    let tmp = TempDir::new().unwrap();
    write_config(
        &tmp,
        r#"
[linkedin]
client_id = "test-id"

[profiles.someone.linkedin]
handle = "someone"
"#,
    );

    let report = run_doctor_isolated(&tmp, Some("linkedin"));
    let output = report.lines.join("\n");
    assert!(!report.ok, "should fail when no URN");
    assert!(
        output.contains("no URN"),
        "should report missing URN: {}",
        output
    );
    assert!(
        output.contains("corky linkedin auth"),
        "should suggest fix command"
    );
}

#[test]
fn test_doctor_linkedin_no_profiles() {
    let tmp = TempDir::new().unwrap();
    write_config(
        &tmp,
        r#"
[linkedin]
client_id = "test-id"
"#,
    );

    let report = run_doctor_isolated(&tmp, Some("linkedin"));
    let output = report.lines.join("\n");
    assert!(!report.ok);
    assert!(
        output.contains("No LinkedIn profiles configured"),
        "should report no profiles: {}",
        output,
    );
}

#[test]
fn test_doctor_youtube_section() {
    let tmp = TempDir::new().unwrap();
    write_config(
        &tmp,
        r#"
[youtube]
client_id = "yt-client-id"
client_secret = "yt-secret"

[profiles.mychannel.youtube]
handle = "@mychannel"
urn = "UCabc123"
"#,
    );

    let report = run_doctor_isolated(&tmp, Some("youtube"));
    let output = report.lines.join("\n");
    assert!(output.contains("YouTube:"), "should have YouTube section");
    assert!(
        output.contains("OAuth credentials configured (config)"),
        "should detect config creds"
    );
    assert!(
        output.contains("Profile 'mychannel': handle=@mychannel, channel=UCabc123"),
        "should show profile info: {}",
        output,
    );
    // No token — should report missing.
    assert!(
        output.contains("No YouTube token found"),
        "should report missing token"
    );
}

#[test]
fn test_doctor_youtube_no_config() {
    let tmp = TempDir::new().unwrap();
    // No [youtube] section at all — should be silent.
    write_config(&tmp, "");

    let report = run_doctor_isolated(&tmp, Some("youtube"));
    let output = report.lines.join("\n");
    assert!(
        !output.contains("YouTube:"),
        "should not show YouTube section when not configured"
    );
}

#[test]
fn test_doctor_provider_filter_skips_unrelated() {
    let tmp = TempDir::new().unwrap();
    write_config(
        &tmp,
        r#"
[accounts.personal]
provider = "gmail"
user = "me@gmail.com"
password = "test"
imap_host = "imap.gmail.com"
imap_port = 993
labels = []
default = true

[linkedin]
client_id = "li-id"

[profiles.me.linkedin]
handle = "myhandle"
urn = "urn:li:person:xyz"

[youtube]
client_id = "yt-id"

[profiles.me.youtube]
handle = "@me"
urn = "UCxyz"
"#,
    );

    // Filter to linkedin — should not show gmail accounts or youtube.
    let report = run_doctor_isolated(&tmp, Some("linkedin"));
    let output = report.lines.join("\n");
    assert!(output.contains("LinkedIn:"));
    assert!(!output.contains("Account: personal"), "should not show email accounts");
    assert!(!output.contains("YouTube:"), "should not show YouTube");

    // Filter to youtube — should not show gmail or linkedin.
    let report = run_doctor_isolated(&tmp, Some("youtube"));
    let output = report.lines.join("\n");
    assert!(output.contains("YouTube:"));
    assert!(!output.contains("Account: personal"), "should not show email accounts");
    assert!(!output.contains("LinkedIn:"), "should not show LinkedIn");
}

#[test]
fn test_doctor_checkmarks_format() {
    let tmp = TempDir::new().unwrap();
    write_config(
        &tmp,
        r#"
[accounts.default]
provider = "gmail"
user = "test@gmail.com"
password = "pw"
imap_host = "imap.gmail.com"
imap_port = 993
labels = []
default = true
"#,
    );

    let report = run_doctor_isolated(&tmp, None);
    let output = report.lines.join("\n");
    // All success lines should use ✓ (U+2713).
    for line in &report.lines {
        if line.trim().is_empty() || line.starts_with("Account:") || line.starts_with("Gmail") {
            continue;
        }
        assert!(
            line.contains('\u{2713}') || line.contains('\u{2717}'),
            "Line should contain a checkmark: {:?}",
            line
        );
    }
    // At least one ✓ present.
    assert!(output.contains('\u{2713}'));
}
