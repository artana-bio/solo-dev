//! A verdict that found problems is recordable against the candidate it read.
//!
//! An approval must bind to the exact current candidate — that is the harness's
//! central claim and nothing here weakens it. A `changes_requested` or
//! `blocked` verdict is a different kind of statement: it is true about the
//! candidate the reviewer actually read, and it stays true when the branch
//! moves afterwards. Refusing to file it protects nothing and destroys the
//! reviewer's work.
//!
//! Found by making the mistake four times on one card. Each time a reviewer
//! returned findings, the findings were fixed, the branch moved, and the
//! verdict became unrecordable — three of that card's review rounds survive
//! only as prose inside a `handoff revoke --reason`, where nothing can query
//! them.

mod support;

use std::fs;

use support::Workspace;

/// A card handed off and under review, plus the candidate that was handed off.
fn under_review() -> (Workspace, String) {
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
    workspace.work(&["start", "--card-id", "F-001"]);

    let path = workspace.worktrees.join("F-001");
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add a.rs"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    let reviewed = support::capture(&path, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {reviewed}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    workspace.handoff(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
    ]);
    workspace.review(&["begin", "--card-id", "F-001"]);
    (workspace, reviewed)
}

/// Moves the candidate on, the way fixing a finding does.
fn move_the_branch(workspace: &Workspace) -> String {
    let path = workspace.worktrees.join("F-001");
    fs::write(path.join("src/a.rs"), "fn main() { /* fixed */ }\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "fix: address the finding"]);
    support::capture(&path, &["rev-parse", "HEAD"])
}

fn verdict_file(workspace: &Workspace, decision: &str) -> String {
    let body = format!(
        "reviewer_actor_id: reviewer-session-a\ndecision: {decision}\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed the guard directly\nresidual_risks: []\n"
    );
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, body).unwrap();
    path.display().to_string()
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn changes_requested_is_recordable_after_the_branch_moves() {
    let (workspace, reviewed) = under_review();
    let moved_on = move_the_branch(&workspace);
    assert_ne!(reviewed, moved_on, "the fixture must move the candidate");

    let path = verdict_file(&workspace, "changes_requested");
    let envelope = workspace.review_json(&["record", "--card-id", "F-001", "--verdict", &path]);

    assert_eq!(envelope["status"], "success");
    assert_eq!(
        envelope["data"]["review"]["candidate_sha"], reviewed,
        "the verdict must name the candidate it was reached against, not the current head"
    );
    assert_eq!(envelope["data"]["review"]["decision"], "changes_requested");
}

#[test]
fn blocked_is_recordable_after_the_branch_moves() {
    let (workspace, reviewed) = under_review();
    move_the_branch(&workspace);

    let path = verdict_file(&workspace, "blocked");
    let envelope = workspace.review_json(&["record", "--card-id", "F-001", "--verdict", &path]);

    assert_eq!(envelope["status"], "success");
    assert_eq!(envelope["data"]["review"]["candidate_sha"], reviewed);
}

#[test]
fn an_approval_after_the_branch_moves_is_still_refused() {
    // The invariant this whole harness exists for. An approval binds to an
    // exact commit; if the branch moved, the approval would attest to code
    // nobody read.
    let (workspace, _) = under_review();
    move_the_branch(&workspace);

    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\nresidual_risks: []\n";
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, body).unwrap();

    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-STALE-HANDOFF");
}

#[test]
fn an_approval_against_the_current_candidate_still_succeeds() {
    // The other half: nothing about the ordinary path changed.
    let (workspace, reviewed) = under_review();

    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\nresidual_risks: []\n";
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, body).unwrap();

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path.display().to_string(),
    ]);
    assert_eq!(envelope["data"]["review"]["decision"], "approved");
    assert_eq!(envelope["data"]["review"]["candidate_sha"], reviewed);
}

#[test]
fn the_recorded_verdict_survives_the_sequence_that_used_to_lose_it() {
    // The exact sequence, end to end: review opens, the reviewer finds
    // problems, the implementer fixes and commits before filing, and the
    // verdict is filed afterwards. Previously the last step was impossible and
    // the only way forward was revoking the handoff, which left the findings
    // in a free-text reason.
    let (workspace, reviewed) = under_review();
    move_the_branch(&workspace);

    let path = verdict_file(&workspace, "changes_requested");
    workspace.review(&["record", "--card-id", "F-001", "--verdict", &path]);

    let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let recorded = inspected["data"]["reviews"]
        .as_array()
        .expect("the review history");
    assert_eq!(recorded.len(), 1, "the verdict is in the record");
    assert_eq!(
        recorded[0]["candidate_sha"], reviewed,
        "and it still names what was actually reviewed"
    );
    assert_eq!(recorded[0]["decision"], "changes_requested");
}
