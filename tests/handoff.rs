//! `WP-250` acceptance: exact-SHA handoff.
//!
//! The centrepiece is the `delivered_sha` binding from `SPIKE-001` finding F-1.

mod support;

use std::fs;

use serde_json::Value;
use support::Workspace;

/// A card allocated, worked, and gated, ready to hand off.
fn ready_to_hand_off() -> (Workspace, String) {
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
    (workspace, head)
}

/// Writes a declaration file and returns its path.
fn declaration(workspace: &Workspace, delivered_sha: &str) -> String {
    let body = format!(
        "delivered_sha: {delivered_sha}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [kept it minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert the commit\n"
    );
    let path = workspace.root.join("declaration.yaml");
    fs::write(&path, body).unwrap();
    path.display().to_string()
}

/// Extends the otherwise valid candidate with a file the card does not own.
fn out_of_scope_candidate(workspace: &Workspace) -> (String, String) {
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("outside.rs"), "fn outside_scope() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: add outside file"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = declaration(workspace, &head);
    (head, declaration)
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn a_handoff_binds_the_exact_candidate_and_its_evidence() {
    let (workspace, head) = ready_to_hand_off();
    let path = declaration(&workspace, &head);

    let envelope =
        workspace.handoff_json(&["create", "--card-id", "F-001", "--declaration", &path]);
    let handoff = &envelope["data"]["handoff"];

    assert_eq!(handoff["candidate_sha"], head);
    assert_eq!(handoff["status"], "active");
    assert_eq!(handoff["worktree_clean"], true);
    assert_eq!(handoff["commits"].as_array().unwrap().len(), 1);
    assert_eq!(handoff["changed_paths"].as_array().unwrap().len(), 1);
    assert_eq!(handoff["receipts"].as_array().unwrap().len(), 1);
    assert_eq!(
        handoff["canonical_algorithm"], "harness.canonical-json/v1",
        "a record must name the algorithm its digest was computed under"
    );
    assert!(
        envelope["data"]["handoff_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
}

#[test]
fn a_branch_rewritten_after_delivery_is_refused() {
    // SPIKE-001 finding F-1, the situation the spike actually surfaced: the
    // reviewed commit was not the commit the implementer produced.
    let (workspace, delivered) = ready_to_hand_off();
    let path = declaration(&workspace, &delivered);

    // Someone amends the branch after the actor finished.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/a.rs"), "fn main() { /* changed */ }\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "--amend", "--no-edit"]);
    let rewritten = support::capture(&worktree, &["rev-parse", "HEAD"]);
    assert_ne!(delivered, rewritten);

    let output = workspace.handoff_raw(&["create", "--card-id", "F-001", "--declaration", &path]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-DELIVERED-SHA-MISMATCH");

    let message = String::from_utf8_lossy(&output.stdout);
    assert!(message.contains(&delivered), "must name the delivered SHA");
    assert!(message.contains(&rewritten), "must name the actual SHA");
}

#[test]
fn a_declaration_missing_narrative_fails() {
    let (workspace, head) = ready_to_hand_off();
    for missing in [
        ("behavior_delivered: adds a.rs", "behavior_delivered: ''"),
        ("rollback_notes: revert the commit", "rollback_notes: ''"),
        (
            "implementation_decisions: [kept it minimal]",
            "implementation_decisions: []",
        ),
    ] {
        let body = format!(
            "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [kept it minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert the commit\n"
        )
        .replace(missing.0, missing.1);
        let path = workspace.root.join("bad.yaml");
        fs::write(&path, body).unwrap();

        let output = workspace.handoff_raw(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &path.display().to_string(),
        ]);
        assert_eq!(
            output.status.code(),
            Some(5),
            "missing `{}` must be refused",
            missing.0
        );
        assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-HANDOFF");
    }
}

#[test]
fn a_dirty_worktree_refuses_handoff() {
    let (workspace, head) = ready_to_hand_off();
    let path = declaration(&workspace, &head);
    fs::write(
        workspace.worktrees.join("F-001/src/scratch.rs"),
        "uncommitted\n",
    )
    .unwrap();

    let output = workspace.handoff_raw(&["create", "--card-id", "F-001", "--declaration", &path]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-WORKTREE-DIRTY");
}

#[test]
fn an_out_of_scope_candidate_refuses_handoff_with_the_path() {
    let (workspace, _) = ready_to_hand_off();
    let (_head, path) = out_of_scope_candidate(&workspace);

    let output = workspace.handoff_raw(&["create", "--card-id", "F-001", "--declaration", &path]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-CANDIDATE-OUT-OF-SCOPE");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("outside.rs"),
        "the refusal must name the path outside the card's write scope"
    );
}

#[test]
fn a_dry_run_refuses_an_out_of_scope_candidate_with_the_path() {
    let (workspace, _) = ready_to_hand_off();
    let (_head, path) = out_of_scope_candidate(&workspace);

    let output = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &path,
        "--dry-run",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-CANDIDATE-OUT-OF-SCOPE");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("outside.rs"),
        "the preview refusal must name the path outside the card's write scope"
    );
}

#[test]
fn missing_gate_evidence_refuses_handoff() {
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

    let worktree = workspace.worktrees.join("F-001");
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let path = declaration(&workspace, &head);

    // No gate has been run for this candidate. Preview and real creation must
    // refuse identically; a successful preview here would be false readiness.
    for args in [
        vec![
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &path,
            "--dry-run",
        ],
        vec!["create", "--card-id", "F-001", "--declaration", &path],
    ] {
        let output = workspace.handoff_raw(&args);
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(error_code(&output), "CH-GATE-EVIDENCE-STALE");
    }
}

#[test]
fn stale_gate_evidence_refuses_handoff() {
    let (workspace, _) = ready_to_hand_off();
    let worktree = workspace.worktrees.join("F-001");

    // Move the candidate after gating. The receipt named the previous commit.
    fs::write(worktree.join("src/b.rs"), "fn b() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: add b.rs"]);
    let moved = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let path = declaration(&workspace, &moved);

    let output = workspace.handoff_raw(&["create", "--card-id", "F-001", "--declaration", &path]);
    assert_eq!(output.status.code(), Some(7));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("candidate is now"),
        "the refusal must say which binding broke"
    );
}

#[test]
fn a_head_change_invalidates_an_existing_handoff() {
    let (workspace, head) = ready_to_hand_off();
    let path = declaration(&workspace, &head);
    workspace.handoff(&["create", "--card-id", "F-001", "--declaration", &path]);

    let before = workspace.handoff_json(&["inspect", "--card-id", "F-001"]);
    assert_eq!(before["data"]["is_current"], true);

    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/b.rs"), "fn b() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: add b.rs"]);

    let output = workspace.handoff_raw(&["inspect", "--card-id", "F-001"]);
    let after: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(after["data"]["is_current"], false);
    assert!(
        after["data"]["stale_reason"]
            .as_str()
            .unwrap()
            .contains("branch is now")
    );
    assert!(
        after["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("reviewing different code"))
    );
}

#[test]
fn a_card_revision_invalidates_an_existing_handoff() {
    let (workspace, head) = ready_to_hand_off();
    let path = declaration(&workspace, &head);
    workspace.handoff(&["create", "--card-id", "F-001", "--declaration", &path]);

    // Revising the card changes its digest, which the handoff was bound to.
    workspace.revise_card("F-001", &["src/**"], "risk reassessed");

    let output = workspace.handoff_raw(&["inspect", "--card-id", "F-001"]);
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["data"]["is_current"], false);
    assert!(
        envelope["data"]["stale_reason"]
            .as_str()
            .unwrap()
            .contains("card is now")
    );
}

#[test]
fn a_handoff_can_be_reproduced_from_control_and_git() {
    let (workspace, head) = ready_to_hand_off();
    let path = declaration(&workspace, &head);
    let created = workspace.handoff_json(&["create", "--card-id", "F-001", "--declaration", &path]);

    // Inspect re-reads from control and recomputes the digest; it must agree.
    let inspected = workspace.handoff_json(&["inspect", "--card-id", "F-001"]);
    assert_eq!(
        created["data"]["handoff_digest"],
        inspected["data"]["handoff_digest"]
    );
    assert_eq!(created["data"]["handoff"], inspected["data"]["handoff"]);
}

#[test]
fn revoking_returns_the_card_to_active_work() {
    let (workspace, head) = ready_to_hand_off();
    let path = declaration(&workspace, &head);
    workspace.handoff(&["create", "--card-id", "F-001", "--declaration", &path]);
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "handed_off"
    );

    let envelope = workspace.handoff_json(&[
        "revoke",
        "--card-id",
        "F-001",
        "--reason",
        "found a problem",
    ]);
    assert_eq!(envelope["data"]["state"], "active");
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "active"
    );

    let revoked = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "handoff.revoked")
        .expect("the revocation must be recorded");
    assert_eq!(revoked["metadata"]["reason"], "found a problem");
}

#[test]
fn a_revoked_handoff_never_applies_again() {
    let (workspace, head) = ready_to_hand_off();
    let path = declaration(&workspace, &head);
    workspace.handoff(&["create", "--card-id", "F-001", "--declaration", &path]);
    workspace.handoff(&["revoke", "--card-id", "F-001", "--reason", "withdrawn"]);

    let output = workspace.handoff_raw(&["inspect", "--card-id", "F-001"]);
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["data"]["is_current"], false);
    assert_eq!(envelope["data"]["stale_reason"], "the handoff was revoked");

    let again = workspace.handoff_raw(&["revoke", "--card-id", "F-001", "--reason", "again"]);
    assert_eq!(again.status.code(), Some(5));
}

#[test]
fn the_handoff_is_versioned_and_recorded_as_an_event() {
    let (workspace, head) = ready_to_hand_off();
    let path = declaration(&workspace, &head);
    let envelope =
        workspace.handoff_json(&["create", "--card-id", "F-001", "--declaration", &path]);
    let id = envelope["data"]["handoff"]["handoff_id"].as_str().unwrap();

    let tracked = workspace.control_tracked_files();
    assert!(
        tracked.contains(&format!("handoffs/{id}.json")),
        "{tracked:?}"
    );

    let created = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "handoff.created")
        .expect("the handoff must be recorded");
    assert_eq!(created["metadata"]["handoff_id"], id);
    assert_eq!(created["metadata"]["delivered_sha"], head);
    assert_eq!(created["head_sha"], head);
    assert_eq!(created["next_state"], "handed_off");
}

#[test]
fn a_dry_run_creates_nothing() {
    let (workspace, head) = ready_to_hand_off();
    let path = declaration(&workspace, &head);
    let before = workspace.control_head();

    let envelope = workspace.handoff_json(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &path,
        "--dry-run",
    ]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(workspace.control_head(), before);
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "active"
    );
}
