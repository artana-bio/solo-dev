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

fn error_message(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["message"].as_str().unwrap().to_owned()
}

/// Withdraws the handoff, the way an implementer pulling a delivery back does.
fn revoke_the_handoff(workspace: &Workspace) {
    workspace.handoff(&[
        "revoke",
        "--card-id",
        "F-001",
        "--reason",
        "withdrawn before the verdict was filed",
    ]);
}

/// Revises the card underneath the handoff, changing what it was measured
/// against.
fn revise_the_card(workspace: &Workspace) {
    let body = format!(
        "card_id: F-001\ncycle_id: C-001\ntitle: Implement F-001\ngoal: Deliver F-001 against changed requirements\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/**\"]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works differently now]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head(),
    );
    let path = workspace.root.join("F-001-r2.yaml");
    fs::write(&path, body).unwrap();
    workspace.card(&[
        "revise",
        "--card-id",
        "F-001",
        "--draft",
        &path.display().to_string(),
        "--reason",
        "the requirements changed underneath the handoff",
    ]);
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

// The relaxation above drops exactly one question. These are the two it must
// not drop. Written because the first version of it wrapped the whole check in
// `if decision == approved`, which stopped asking all three.

#[test]
fn blocked_against_a_revoked_handoff_is_refused() {
    // Revocation is the implementer withdrawing the delivery — not the branch
    // moving under a delivery that still stands. A verdict filed against it
    // would put a review in the record naming a handoff nobody was offering.
    let (workspace, _) = under_review();
    revoke_the_handoff(&workspace);

    let path = verdict_file(&workspace, "blocked");
    let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);

    assert_eq!(output.status.code(), Some(5));
    assert_eq!(
        error_code(&output),
        "CH-POLICY-STALE-HANDOFF",
        "the staleness check must be what refuses this, not the transition \
         table reaching it by accident"
    );
    let message = error_message(&output);
    assert!(
        message.contains("revoked"),
        "the refusal must say the handoff was withdrawn: {message}"
    );
}

#[test]
fn no_verdict_is_recorded_against_a_revoked_handoff() {
    // The refusal has to leave the record empty. Asserting the exit code alone
    // would pass even if the review were written and the error raised after.
    let (workspace, _) = under_review();
    revoke_the_handoff(&workspace);

    let path = verdict_file(&workspace, "blocked");
    workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);

    let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let recorded = inspected["data"]["reviews"]
        .as_array()
        .expect("the review history");
    assert!(
        recorded.is_empty(),
        "a refused verdict must not reach the record: {recorded:?}"
    );
}

#[test]
fn a_non_approval_against_a_revised_card_is_refused_for_the_true_reason() {
    // `card revise` returns the card to `ready`, from which the transition
    // table refuses `blocked` anyway — so this case was never a hole in the
    // record. What it was is a refusal that named the wrong thing: the
    // operator was told a card cannot move from `ready` to `blocked`, when
    // what actually happened is that the card was revised out from under the
    // handoff the reviewer was holding. This asserts the accurate one.
    let (workspace, _) = under_review();
    revise_the_card(&workspace);

    let path = verdict_file(&workspace, "blocked");
    let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);

    assert_eq!(output.status.code(), Some(5));
    assert_eq!(
        error_code(&output),
        "CH-POLICY-STALE-HANDOFF",
        "not CH-POLICY-INVALID-TRANSITION, which is the state machine \
         answering a question nobody asked"
    );
    let message = error_message(&output);
    assert!(
        message.contains("card digest"),
        "the refusal must name the binding that broke: {message}"
    );
}

#[test]
fn an_approval_against_a_revoked_handoff_is_still_refused() {
    // Unchanged by this card, and asserted so that narrowing the scope for
    // non-approvals cannot quietly narrow it for approvals too.
    let (workspace, _) = under_review();
    revoke_the_handoff(&workspace);

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
fn a_revoked_handoff_refuses_a_verdict_even_after_the_branch_moves() {
    // The two conditions together. The moved branch is the one a non-approval
    // is allowed to outlive; the revocation is not, and the presence of the
    // first must not excuse the second.
    let (workspace, _) = under_review();
    revoke_the_handoff(&workspace);
    move_the_branch(&workspace);

    let path = verdict_file(&workspace, "blocked");
    let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);

    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-STALE-HANDOFF");
    let message = error_message(&output);
    assert!(
        message.contains("revoked"),
        "revocation is reported ahead of the branch having moved: {message}"
    );
}
