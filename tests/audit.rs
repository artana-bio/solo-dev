//! `WP-530` acceptance: cycle audit and evidence cross-checking.
//!
//! The report's whole value is the discrepancies. A summary of records that all
//! agree tells a reader nothing they could not get by listing files; what they
//! cannot get any other way is whether the evidence still describes the objects
//! it names. So every test here is about what happens when it does not.

mod support;

use std::fs;

use support::Workspace;

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

fn audit_raw(workspace: &Workspace, cycle: &str) -> std::process::Output {
    Workspace::run(&[
        "audit".into(),
        "cycle".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        cycle.into(),
        "--output".into(),
        "json".into(),
    ])
}

fn audit_json(workspace: &Workspace, cycle: &str) -> serde_json::Value {
    let output = audit_raw(workspace, cycle);
    assert!(
        output.status.success(),
        "audit failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).expect("the JSON envelope")
}

/// A cycle carried all the way to a promoted, archived integration.
fn completed() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let id = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }
    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    workspace.integration(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
    ]);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "owner",
    ]);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    workspace
}

#[test]
fn a_clean_cycle_reports_no_discrepancies() {
    let workspace = completed();
    let envelope = audit_json(&workspace, "C-001");
    assert_eq!(
        envelope["data"]["discrepancies"].as_array().unwrap().len(),
        0,
        "unexpected: {}",
        envelope["data"]["discrepancies"]
    );
    assert!(envelope["data"]["events"].as_u64().unwrap() > 0);
}

#[test]
fn the_timeline_reconstructs_the_cycle_in_order() {
    let workspace = completed();
    let envelope = audit_json(&workspace, "C-001");
    let timeline = envelope["data"]["timeline"].as_array().unwrap();

    let types: Vec<&str> = timeline
        .iter()
        .map(|entry| entry["type"].as_str().unwrap())
        .collect();
    // Order matters, not just membership: a set would not show that review
    // followed handoff rather than preceding it.
    let handoff = types.iter().position(|t| *t == "handoff.created");
    let review = types.iter().position(|t| *t == "review.recorded");
    let promoted = types.iter().position(|t| *t == "integration.promoted");
    assert!(handoff < review, "review must follow handoff: {types:?}");
    assert!(review < promoted, "promotion must follow review: {types:?}");
}

#[test]
fn the_report_names_the_exact_protected_branch_transition() {
    let workspace = completed();
    let envelope = audit_json(&workspace, "C-001");
    let transitions = envelope["data"]["protected_branch_transitions"]
        .as_array()
        .unwrap();

    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0]["to"], workspace.authority_head());
    assert!(
        transitions[0]["from"].as_str().unwrap() != workspace.authority_head(),
        "both ends of the transition must be named: {}",
        transitions[0]
    );
    assert!(
        transitions[0]["acceptance_id"]
            .as_str()
            .unwrap()
            .starts_with("ACC-"),
        "the authorizing decision must be named: {}",
        transitions[0]
    );
}

#[test]
fn a_card_revision_that_no_longer_matches_its_review_is_reported() {
    let workspace = completed();
    // Tamper with the stored revision so its digest no longer matches what the
    // review was bound to. This is what an edited immutable record looks like.
    let path = workspace.control.join("cards/F-001/r1.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    value["title"] = serde_json::json!("something else entirely");
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();

    let output = audit_raw(&workspace, "C-001");
    assert!(!output.status.success(), "a tampered record must not pass");
    assert_eq!(error_code(&output), "CH-POLICY-AUDIT-DISCREPANCY");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("review RV-"),
        "the affected record must be named: {envelope}"
    );
}

#[test]
fn evidence_a_record_refers_to_but_which_is_absent_is_reported() {
    let workspace = completed();
    // Gate logs live outside control history and have a retention window, so
    // this is the ordinary way evidence goes missing rather than a contrived
    // corruption.
    fs::remove_dir_all(workspace.control.join("logs")).unwrap();

    let output = audit_raw(&workspace, "C-001");
    assert!(!output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("receipt R-"),
        "the receipt whose logs are gone must be named: {envelope}"
    );
}

#[test]
fn a_missing_card_revision_is_reported_rather_than_skipped() {
    let workspace = completed();
    fs::remove_file(workspace.control.join("cards/F-001/r1.json")).unwrap();

    let output = audit_raw(&workspace, "C-001");
    assert!(
        !output.status.success(),
        "a record whose subject is gone must not read as clean"
    );
    assert_eq!(error_code(&output), "CH-POLICY-AUDIT-DISCREPANCY");
}

#[test]
fn gate_log_contents_never_appear_in_the_report() {
    let workspace = Workspace::initialized();
    // A gate that prints something a reader must never find in a report.
    workspace.register_gate(
        "gate.leaky",
        &["sh", "-c", "echo 'AKIAIOSFODNN7EXAMPLE super-secret'"],
    );
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gate_sets(
        "F-001",
        &["src/F-001/**"],
        &["gate.leaky"],
        &["gate.all"],
    );
    workspace.work(&["start", "--card-id", "F-001"]);
    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src/F-001")).unwrap();
    fs::write(worktree.join("src/F-001/a.rs"), "// work\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: work"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.leaky"]);

    // The secret really is on disk, or this test proves nothing.
    let logs = support::capture_stdout_of_logs(&workspace.control);
    assert!(
        logs.contains("AKIAIOSFODNN7EXAMPLE"),
        "the fixture must actually have written the secret"
    );

    let output = audit_raw(&workspace, "C-001");
    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(
        !rendered.contains("AKIAIOSFODNN7EXAMPLE"),
        "gate output must never reach the report"
    );
    assert!(
        !rendered.contains("super-secret"),
        "gate output must never reach the report"
    );
}

#[test]
fn auditing_an_unknown_cycle_is_a_precondition_failure() {
    let workspace = Workspace::initialized();
    let output = audit_raw(&workspace, "C-404");
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-NOT-FOUND");
}

#[test]
fn the_report_is_reproducible_from_control_state_alone() {
    let workspace = completed();
    let first = audit_json(&workspace, "C-001");
    let second = audit_json(&workspace, "C-001");
    assert_eq!(
        first["data"], second["data"],
        "two runs over unchanged state must agree exactly"
    );
}
