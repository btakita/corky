//! Profile validation edge cases (P1–P9).

mod common;

use corky::social::platform::Platform;
use corky::social::profiles::ProfilesFile;
use tempfile::TempDir;

// P1: Duplicate handle within same platform
#[test]
fn p1_duplicate_handle_same_platform() {
    let profiles = ProfilesFile::parse(
        r#"
[alice]
[alice.linkedin]
handle = "alice-smith"

[bob]
[bob.linkedin]
handle = "alice-smith"
"#,
    )
    .unwrap();

    let result = profiles.validate();
    assert!(!result.is_ok());
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.contains("handle") && e.contains("alice-smith")),
        "Expected duplicate handle error, got: {:?}",
        result.errors
    );
}

// P2: Duplicate URN within same platform
#[test]
fn p2_duplicate_urn_same_platform() {
    let profiles = ProfilesFile::parse(
        r#"
[alice]
[alice.linkedin]
handle = "alice"
urn = "urn:li:person:123"

[bob]
[bob.linkedin]
handle = "bob"
urn = "urn:li:person:123"
"#,
    )
    .unwrap();

    let result = profiles.validate();
    assert!(!result.is_ok());
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.contains("URN") && e.contains("urn:li:person:123")),
        "Expected duplicate URN error, got: {:?}",
        result.errors
    );
}

// P3: Same URN across different profiles (cross-platform)
#[test]
fn p3_cross_profile_urn_conflict() {
    let profiles = ProfilesFile::parse(
        r#"
[alice]
[alice.linkedin]
handle = "alice"
urn = "shared-urn-123"

[bob]
[bob.twitter]
handle = "bob"
urn = "shared-urn-123"
"#,
    )
    .unwrap();

    let result = profiles.validate();
    assert!(!result.is_ok());
    assert!(
        result.errors.iter().any(|e| e.contains("shared-urn-123")),
        "Expected cross-profile URN conflict, got: {:?}",
        result.errors
    );
}

// P4: Profile with no platform entries
#[test]
fn p4_profile_no_platforms() {
    let profiles = ProfilesFile::parse(
        r#"
[emptyprofile]
"#,
    )
    .unwrap();

    let result = profiles.validate();
    assert!(result.is_ok()); // warnings only, no errors
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("emptyprofile") && w.contains("no platform")),
        "Expected no-platform warning, got: {:?}",
        result.warnings
    );
}

// P5: Cross-platform coherence (info message)
#[test]
fn p5_cross_platform_coherence() {
    let profiles = ProfilesFile::parse(
        r#"
[alice]
[alice.linkedin]
handle = "alice-li"
[alice.twitter]
handle = "alice-tw"
"#,
    )
    .unwrap();

    let result = profiles.validate();
    assert!(result.is_ok());
    assert!(
        result
            .info
            .iter()
            .any(|i| i.contains("alice") && i.contains("verify same person")),
        "Expected coherence info, got: {:?}",
        result.info
    );
}

// P6: Empty profiles.toml
#[test]
fn p6_empty_profiles() {
    let profiles = ProfilesFile::parse("").unwrap();
    assert!(profiles.profiles.is_empty());
    let result = profiles.validate();
    assert!(result.is_ok());
    assert!(result.warnings.is_empty());
    assert!(result.info.is_empty());
}

// P7: Malformed TOML
#[test]
fn p7_malformed_toml() {
    let result = ProfilesFile::parse("this is not [valid toml");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(!err.is_empty(), "Error should have parse details");
}

// P8: Missing profiles.toml
#[test]
fn p8_missing_profiles_toml() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nonexistent.toml");
    let result = ProfilesFile::load_from(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not found"),
        "Expected 'not found' message, got: {}",
        err
    );
}

// P9: Collaborator merge introduces duplicate URN (same as P2 but framed as post-merge)
#[test]
fn p9_merge_conflict_duplicate_urn() {
    // After merging two collaborators' profiles.toml, duplicate URN surfaces
    let profiles = ProfilesFile::parse(
        r#"
[alice]
[alice.linkedin]
handle = "alice"
urn = "urn:li:person:same"

[bob]
[bob.linkedin]
handle = "bob"
urn = "urn:li:person:same"
"#,
    )
    .unwrap();

    let result = profiles.validate();
    assert!(!result.is_ok());
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.contains("urn:li:person:same")),
        "Post-merge validation should surface URN conflict"
    );
}

// Additional: resolve_urn works
#[test]
fn resolve_urn_success() {
    let profiles = ProfilesFile::parse(
        r#"
[btakita]
[btakita.linkedin]
handle = "brian-takita"
urn = "urn:li:person:abc123"
"#,
    )
    .unwrap();

    let urn = profiles.resolve_urn("btakita", Platform::LinkedIn).unwrap();
    assert_eq!(urn, "urn:li:person:abc123");
}

// resolve_urn: profile not found
#[test]
fn resolve_urn_profile_not_found() {
    let profiles = ProfilesFile::parse(
        r#"
[btakita]
[btakita.linkedin]
handle = "brian-takita"
urn = "urn:li:person:abc123"
"#,
    )
    .unwrap();

    let result = profiles.resolve_urn("nobody", Platform::LinkedIn);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

// resolve_urn: no platform entry
#[test]
fn resolve_urn_no_platform_entry() {
    let profiles = ProfilesFile::parse(
        r#"
[btakita]
[btakita.linkedin]
handle = "brian-takita"
urn = "urn:li:person:abc123"
"#,
    )
    .unwrap();

    let result = profiles.resolve_urn("btakita", Platform::Twitter);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("no twitter entry"));
}

// resolve_handle
#[test]
fn resolve_handle_success() {
    let profiles = ProfilesFile::parse(
        r#"
[btakita]
[btakita.linkedin]
handle = "brian-takita"
"#,
    )
    .unwrap();

    let name = profiles.resolve_handle("brian-takita", Platform::LinkedIn);
    assert_eq!(name, Some("btakita".to_string()));
}
