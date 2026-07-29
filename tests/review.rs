//! `WP-320` acceptance: independent review.

mod support;

use std::fs;

use serde_json::Value;
use support::Workspace;

/// A card handed off and ready for review.
fn handed_off() -> (Workspace, String) {
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

    let head = support::capture(&path, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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
    (workspace, head)
}

/// Writes a verdict file and returns its path.
fn verdict(workspace: &Workspace, body: &str) -> String {
    let path = workspace.root.join("verdict.yaml");
    fs::write(&path, body).unwrap();
    path.display().to_string()
}

/// A clean approval by a distinct reviewer.
fn approval(reviewer: &str) -> String {
    format!(
        "reviewer_actor_id: {reviewer}\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\nresidual_risks: []\n"
    )
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn begin_emits_the_reviewer_packet() {
    let (workspace, head) = handed_off();
    let envelope = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    assert_eq!(envelope["data"]["candidate_sha"], head);

    // The packet came from `review begin`, which ran in the fixture.
    let begun = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "review.begun")
        .expect("the review must be recorded as open");
    assert_eq!(begun["next_state"], "review_pending");
    assert_eq!(begun["head_sha"], head);
}

#[test]
fn an_approval_by_a_distinct_actor_is_recorded() {
    let (workspace, head) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));

    let envelope = workspace.review_json(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_eq!(envelope["data"]["state"], "approved");
    assert_eq!(envelope["data"]["review"]["decision"], "approved");
    assert_eq!(envelope["data"]["review"]["candidate_sha"], head);
    assert_eq!(
        envelope["data"]["review"]["canonical_algorithm"],
        "harness.canonical-json/v1"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "approved"
    );
}

#[test]
fn self_review_is_refused() {
    // Invariant 7.3.7. The fixture's handoff actor is `operator`.
    let (workspace, _) = handed_off();
    let path = verdict(&workspace, &approval("operator"));

    let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-SELF-REVIEW");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("procedural"),
        "the message must not overclaim what this check proves"
    );
}

#[test]
fn approving_over_an_open_finding_is_refused() {
    let (workspace, _) = handed_off();
    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n";
    let path = verdict(&workspace, body);

    let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-OPEN-FINDINGS");
}

#[test]
fn approving_over_a_dispositioned_finding_is_permitted() {
    // SPIKE-001 F-4: the reviewer must be able to approve while recording that
    // a real problem cannot be fixed within this card's write scope.
    let (workspace, _) = handed_off();
    let body = "reviewer_actor_id: reviewer-session-a\ndecision: approved\nfindings:\n  - severity: high\n    location: tests/\n    detail: no coverage of behavior 4\n    disposition: out_of_scope\ngate_adequacy:\n  gates_observe_acceptance: false\n  unobserved_behaviors: [raises on invalid input]\n  basis: mutation-tested the suite; it passes with the guard removed\nresidual_risks: [behavior 4 stays ungated]\n";
    let path = verdict(&workspace, body);

    let output = workspace.review(&["record", "--card-id", "F-001", "--verdict", &path]);
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["data"]["state"], "approved");
    assert_eq!(
        envelope["data"]["review"]["gate_adequacy"]["gates_observe_acceptance"],
        false
    );
    assert!(
        envelope["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("cannot observe")),
        "an inadequate gate must be surfaced, not buried"
    );
}

#[test]
fn changes_requested_returns_the_card_to_work() {
    let (workspace, _) = handed_off();
    let body = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n";
    let path = verdict(&workspace, body);

    let envelope = workspace.review_json(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_eq!(envelope["data"]["state"], "changes_requested");
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "changes_requested"
    );
}

#[test]
fn requesting_changes_without_a_finding_is_refused() {
    let (workspace, _) = handed_off();
    let body = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n";
    let path = verdict(&workspace, body);

    let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
}

#[test]
fn a_review_must_state_how_gate_adequacy_was_established() {
    let (workspace, _) = handed_off();
    let path = verdict(
        &workspace,
        &approval("reviewer-session-a").replace(
            "basis: probed each acceptance behavior directly",
            "basis: ''",
        ),
    );

    let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
}

#[test]
fn a_candidate_change_invalidates_an_approval() {
    let (workspace, _) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));
    workspace.review(&["record", "--card-id", "F-001", "--verdict", &path]);

    let before = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    assert_eq!(before["data"]["has_current_approval"], true);

    // Section 15.2: a candidate change invalidates approval.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/b.rs"), "fn b() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: add b.rs"]);

    let after = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    assert_eq!(after["data"]["has_current_approval"], false);
    assert!(
        after["data"]["reviews"][0]["candidate_sha"] != after["data"]["candidate_sha"],
        "the approval names the old candidate"
    );
}

#[test]
fn a_card_revision_invalidates_an_approval() {
    let (workspace, _) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));
    workspace.review(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_eq!(
        workspace.review_json(&["inspect", "--card-id", "F-001"])["data"]["has_current_approval"],
        true
    );

    workspace.revise_card("F-001", &["src/**"], "requirement changed");

    assert_eq!(
        workspace.review_json(&["inspect", "--card-id", "F-001"])["data"]["has_current_approval"],
        false,
        "a card revision invalidates every review bound to the old digest"
    );
}

#[test]
fn reviewing_a_superseded_handoff_is_refused() {
    let (workspace, _) = handed_off();

    // The branch moves after the handoff was created but before review.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/b.rs"), "fn b() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: sneak in b.rs"]);

    let path = verdict(&workspace, &approval("reviewer-session-a"));
    let output = workspace.review_raw(&["record", "--card-id", "F-001", "--verdict", &path]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-STALE-HANDOFF");
}

#[test]
fn findings_remain_visible_after_a_later_approval() {
    let (workspace, _) = handed_off();

    // First round: changes requested with a finding.
    let rejection = "reviewer_actor_id: reviewer-session-a\ndecision: changes_requested\nfindings:\n  - severity: critical\n    location: src/a.rs\n    detail: missing guard\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n";
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, rejection),
    ]);

    // Section 11.2: the card must come back to active before it can be handed
    // off again. Resuming is what performs that transition.
    workspace.work(&["resume", "--card-id", "F-001"]);

    // The actor fixes it and hands off again.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/a.rs"), "fn main() { /* guarded */ }\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "fix: add guard"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration2.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: adds a guard\nimplementation_decisions: [mirrored the sibling]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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

    // Second round: a different reviewer approves.
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict(&workspace, &approval("reviewer-session-b")),
    ]);

    let envelope = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let reviews = envelope["data"]["reviews"].as_array().unwrap();
    assert_eq!(
        reviews.len(),
        2,
        "a re-review supersedes rather than erases"
    );
    assert_eq!(reviews[0]["decision"], "changes_requested");
    assert_eq!(reviews[0]["findings"][0]["detail"], "missing guard");
    assert_eq!(reviews[1]["decision"], "approved");
    assert_eq!(
        reviews[1]["supersedes"], reviews[0]["review_id"],
        "the later review names the one it supersedes"
    );
    assert_eq!(
        reviews[1]["reviewer_actor_id"], "reviewer-session-b",
        "SPIKE-001 H-04: approval came from a different session"
    );
    assert_eq!(envelope["data"]["has_current_approval"], true);
}

#[test]
fn the_review_is_versioned_and_recorded_as_an_event() {
    let (workspace, head) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));
    let envelope = workspace.review_json(&["record", "--card-id", "F-001", "--verdict", &path]);
    let review_id = envelope["data"]["review"]["review_id"].as_str().unwrap();

    let tracked = workspace.control_tracked_files();
    assert!(
        tracked.contains(&format!("reviews/{review_id}.json")),
        "{tracked:?}"
    );

    let recorded = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "review.recorded")
        .expect("the review must be recorded");
    assert_eq!(recorded["metadata"]["decision"], "approved");
    assert_eq!(recorded["actor_id"], "reviewer-session-a");
    assert_eq!(recorded["head_sha"], head);
    assert_eq!(recorded["next_state"], "approved");
}

#[test]
fn a_dry_run_records_nothing() {
    let (workspace, _) = handed_off();
    let path = verdict(&workspace, &approval("reviewer-session-a"));
    let before = workspace.control_head();

    let envelope = workspace.review_json(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--dry-run",
    ]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(workspace.control_head(), before);
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "review_pending"
    );
}
