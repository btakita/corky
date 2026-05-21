//! Release workflow contract tests.

use serde_yaml::{Mapping, Value};

const PYPI_WORKFLOW: &str = include_str!("../.github/workflows/pypi.yml");
const SPEC: &str = include_str!("../SPEC.md");

fn mapping<'a>(value: &'a Value, context: &str) -> &'a Mapping {
    value
        .as_mapping()
        .unwrap_or_else(|| panic!("{context} should be a YAML mapping"))
}

fn key(name: &str) -> Value {
    Value::String(name.to_owned())
}

fn get<'a>(mapping: &'a Mapping, name: &str, context: &str) -> &'a Value {
    mapping
        .get(key(name))
        .unwrap_or_else(|| panic!("{context} missing `{name}`"))
}

fn string_at<'a>(mapping: &'a Mapping, name: &str, context: &str) -> &'a str {
    get(mapping, name, context)
        .as_str()
        .unwrap_or_else(|| panic!("{context}.{name} should be a string"))
}

#[test]
fn pypi_workflow_uses_trusted_publishing() {
    let workflow: Value = serde_yaml::from_str(PYPI_WORKFLOW).expect("valid PyPI workflow YAML");
    let root = mapping(&workflow, "workflow");

    let permissions = mapping(get(root, "permissions", "workflow"), "workflow.permissions");
    assert_eq!(
        string_at(permissions, "id-token", "workflow.permissions"),
        "write",
        "PyPI trusted publishing requires GitHub OIDC"
    );

    let jobs = mapping(get(root, "jobs", "workflow"), "workflow.jobs");
    let publish = mapping(get(jobs, "publish", "workflow.jobs"), "jobs.publish");

    let environment = mapping(
        get(publish, "environment", "jobs.publish"),
        "jobs.publish.environment",
    );
    assert_eq!(
        string_at(environment, "name", "jobs.publish.environment"),
        "pypi"
    );

    let steps = get(publish, "steps", "jobs.publish")
        .as_sequence()
        .expect("jobs.publish.steps should be a sequence");
    let publish_step = steps
        .iter()
        .map(|step| mapping(step, "publish step"))
        .find(|step| step.get(key("name")).and_then(Value::as_str) == Some("Publish to PyPI"))
        .expect("publish job should include a named PyPI publish step");

    assert_eq!(
        string_at(publish_step, "uses", "jobs.publish.steps.Publish to PyPI"),
        "pypa/gh-action-pypi-publish@release/v1"
    );

    let publish_with = mapping(
        get(publish_step, "with", "jobs.publish.steps.Publish to PyPI"),
        "jobs.publish.steps.Publish to PyPI.with",
    );
    assert_eq!(
        string_at(
            publish_with,
            "packages-dir",
            "jobs.publish.steps.Publish to PyPI.with",
        ),
        "dist"
    );
    assert_eq!(
        get(
            publish_with,
            "skip-existing",
            "jobs.publish.steps.Publish to PyPI.with",
        )
        .as_bool(),
        Some(true),
        "rerunning PyPI should fill missing files without failing on uploaded ones"
    );
    assert!(
        !publish_with.contains_key(key("password")),
        "trusted publishing should not use a PyPI API token"
    );
}

#[test]
fn release_status_spec_separates_registry_parity_from_workflow_health() {
    assert!(
        SPEC.contains("Release status notes track registry parity separately from workflow health"),
        "SPEC should distinguish uploaded registry artifacts from workflow health"
    );
    assert!(
        SPEC.contains("crates.io complete"),
        "SPEC should define when the crates.io publish item can be marked complete"
    );
    assert!(SPEC.contains("Do not close a PyPI trusted-publishing blocker"));
    assert!(SPEC.contains("solely because PyPI artifacts"));
    assert!(
        SPEC.contains("workflow failure as the remaining PyPI publish blocker"),
        "SPEC should keep a failing trusted-publisher workflow as a blocker"
    );
}
