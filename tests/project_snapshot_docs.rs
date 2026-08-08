//! Documentation-contract checks for the WP-550 project snapshot surface.
//!
//! `tests/readme_guide.rs` already proves that every fenced README command has
//! a real CLI shape. This file pins the snapshot examples and the claims that
//! are easy to accidentally drift away from the typed schema or the watch
//! contract. Runtime behavior remains covered by `tests/project_snapshot.rs`.

use std::process::Command;

use change_harness::domain::project_snapshot::PROJECT_SNAPSHOT_SCHEMA;

fn readme() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))
        .expect("README.md should be readable at the repository root")
}

#[test]
fn readme_pins_all_project_snapshot_examples_to_the_public_surface() {
    let readme = readme();

    for example in [
        "change-harness project snapshot --control \"$CHANGE_HARNESS_CONTROL\"",
        "change-harness project snapshot --control \"$CHANGE_HARNESS_CONTROL\" --output json",
        "change-harness project snapshot --control \"$CHANGE_HARNESS_CONTROL\" --watch --interval-ms 1000",
    ] {
        assert!(
            readme.contains(example),
            "README.md must retain the executable snapshot example: {example}"
        );
    }

    assert!(
        readme.contains(PROJECT_SNAPSHOT_SCHEMA),
        "README.md must name the schema exported by the typed projection"
    );
}

#[test]
fn snapshot_documentation_claims_match_the_cli_help_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(["project", "snapshot", "--help"])
        .output()
        .expect("the CLI binary should start");
    assert!(
        output.status.success(),
        "project snapshot --help should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let help = String::from_utf8_lossy(&output.stdout);
    for option in ["--watch", "--interval-ms"] {
        assert!(
            help.contains(option),
            "README.md's snapshot/watch contract must remain backed by CLI help containing {option}"
        );
    }
}

#[test]
fn snapshot_documentation_keeps_the_evidence_and_redaction_boundaries_explicit() {
    let readme = readme();
    for claim in [
        "control_head",
        "wall-clock facts",
        "raw logs, free-form progress",
        "status: not_reported",
        "status: invalid",
        "auditable receipt",
        "not proof of a person's identity",
    ] {
        assert!(
            readme.contains(claim),
            "README.md must retain the bounded snapshot claim: {claim}"
        );
    }
}
