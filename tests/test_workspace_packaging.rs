//! Workspace packaging contract tests.

use toml::Value;

const CARGO_TOML: &str = include_str!("../Cargo.toml");
const CORE_CARGO_TOML: &str = include_str!("../crates/corky-core/Cargo.toml");
const GOOGLE_CARGO_TOML: &str = include_str!("../crates/corky-google/Cargo.toml");
const MAIL_CARGO_TOML: &str = include_str!("../crates/corky-mail/Cargo.toml");
const PYPROJECT_TOML: &str = include_str!("../pyproject.toml");
const SOCIAL_CARGO_TOML: &str = include_str!("../crates/corky-social/Cargo.toml");
const SPEC: &str = include_str!("../SPEC.md");
const TRANSCRIBE_CARGO_TOML: &str = include_str!("../crates/corky-transcribe/Cargo.toml");

fn parse_toml(content: &str) -> Value {
    content.parse::<Value>().expect("valid TOML")
}

fn array_strings<'a>(root: &'a Value, path: &[&str]) -> Vec<&'a str> {
    let mut value = root;
    for key in path {
        value = value
            .get(*key)
            .unwrap_or_else(|| panic!("missing TOML key `{}`", path.join(".")));
    }
    value
        .as_array()
        .unwrap_or_else(|| panic!("{} should be an array", path.join(".")))
        .iter()
        .map(|item| {
            item.as_str()
                .unwrap_or_else(|| panic!("{} should contain strings", path.join(".")))
        })
        .collect()
}

#[test]
fn workspace_declares_domain_crates() {
    let cargo = parse_toml(CARGO_TOML);
    let members = array_strings(&cargo, &["workspace", "members"]);

    for member in [
        ".",
        "crates/corky-core",
        "crates/corky-transcribe",
        "crates/corky-google",
        "crates/corky-mail",
        "crates/corky-social",
    ] {
        assert!(
            members.contains(&member),
            "workspace should include {member}"
        );
    }
}

#[test]
fn root_features_forward_transcribe_build_modes() {
    let cargo = parse_toml(CARGO_TOML);
    let transcribe = array_strings(&cargo, &["features", "transcribe"]);
    let cuda = array_strings(&cargo, &["features", "transcribe-cuda"]);
    let metal = array_strings(&cargo, &["features", "transcribe-metal"]);
    let diarize = array_strings(&cargo, &["features", "diarize"]);

    assert!(transcribe.contains(&"corky-transcribe/transcribe"));
    assert!(cuda.contains(&"corky-transcribe/transcribe-cuda"));
    assert!(metal.contains(&"corky-transcribe/transcribe-metal"));
    assert!(diarize.contains(&"corky-transcribe/diarize"));
}

#[test]
fn python_packaging_targets_workspace_root() {
    let pyproject = parse_toml(PYPROJECT_TOML);
    let maturin = pyproject
        .get("tool")
        .and_then(|tool| tool.get("maturin"))
        .expect("tool.maturin exists");

    assert_eq!(maturin.get("bindings").and_then(Value::as_str), Some("bin"));
    assert_eq!(
        maturin.get("manifest-path").and_then(Value::as_str),
        Some("Cargo.toml")
    );

    let include = array_strings(&pyproject, &["tool", "maturin", "include"]);
    assert!(
        include.iter().any(|pattern| pattern.starts_with("crates/")),
        "maturin source packaging should include workspace crates"
    );
}

#[test]
fn spec_documents_workspace_release_order() {
    assert!(SPEC.contains("The root `corky` package remains the published CLI package"));
    assert!(SPEC.contains(
        "corky-core -> corky-transcribe / corky-google / corky-social -> corky-mail -> corky"
    ));
}

#[test]
fn implementation_crates_are_publishable_for_root_release() {
    for (name, manifest) in [
        ("corky-core", CORE_CARGO_TOML),
        ("corky-transcribe", TRANSCRIBE_CARGO_TOML),
        ("corky-google", GOOGLE_CARGO_TOML),
        ("corky-social", SOCIAL_CARGO_TOML),
        ("corky-mail", MAIL_CARGO_TOML),
    ] {
        let cargo = parse_toml(manifest);
        assert_ne!(
            cargo
                .get("package")
                .and_then(|package| package.get("publish"))
                .and_then(Value::as_bool),
            Some(false),
            "{name} must be publishable before the root corky crate can be published"
        );
    }
}
