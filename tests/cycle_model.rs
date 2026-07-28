//! `WP-200` acceptance: the cycle model against a real project.

mod support;

use serde_json::Value;
use support::Workspace;

#[test]
fn create_records_a_draft_cycle_without_freezing_a_baseline() {
    let workspace = Workspace::initialized();
    let envelope = workspace.cycle_json(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);

    assert_eq!(envelope["data"]["status"], "draft");
    assert_eq!(envelope["data"]["cycle_id"], "C-001");
    assert!(
        envelope["operation_id"].is_string(),
        "a mutating command is journaled"
    );
}

#[test]
fn activation_freezes_one_exact_authority_baseline() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    let envelope = workspace.cycle_json(&["activate", "--cycle-id", "C-001"]);

    let baseline = envelope["data"]["baseline_sha"].as_str().unwrap();
    assert_eq!(baseline.len(), 40);
    assert_eq!(
        baseline,
        workspace.authority_head(),
        "the baseline must come from the authority, not the candidate"
    );
    assert_eq!(envelope["data"]["status"], "active");
}

#[test]
fn the_baseline_comes_from_authority_not_the_candidate_branch() {
    let workspace = Workspace::initialized();
    // Advance the candidate so the two heads differ. A cycle must still freeze
    // what the authority has accepted, not what a local actor last did.
    workspace.commit_candidate("local.txt", "local work\n");
    assert_ne!(workspace.candidate_head(), workspace.authority_head());

    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    let envelope = workspace.cycle_json(&["activate", "--cycle-id", "C-001"]);
    assert_eq!(
        envelope["data"]["baseline_sha"].as_str().unwrap(),
        workspace.authority_head()
    );
}

#[test]
fn an_active_cycle_cannot_silently_change_its_baseline() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    let frozen = workspace.cycle_json(&["status", "--cycle-id", "C-001"])["data"]["baseline_sha"]
        .as_str()
        .unwrap()
        .to_owned();

    // Move the authority forward, then try to re-activate.
    workspace.advance_authority();
    let output = workspace.cycle_raw(&["activate", "--cycle-id", "C-001"]);
    assert_eq!(
        output.status.code(),
        Some(5),
        "re-activation is a policy failure"
    );

    let after = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(
        after["data"]["baseline_sha"].as_str().unwrap(),
        frozen,
        "a frozen baseline never moves"
    );
}

#[test]
fn an_authority_move_does_not_disturb_an_active_cycle() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    let frozen = workspace.cycle_json(&["activate", "--cycle-id", "C-001"])["data"]["baseline_sha"]
        .as_str()
        .unwrap()
        .to_owned();

    workspace.advance_authority();
    assert_ne!(workspace.authority_head(), frozen);

    let status = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(status["data"]["baseline_sha"].as_str().unwrap(), frozen);
    assert_eq!(status["data"]["status"], "active");
}

#[test]
fn invalid_transitions_fail_as_policy_violations() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);

    // active -> active is not in Section 11.1.
    let output = workspace.cycle_raw(&["activate", "--cycle-id", "C-001"]);
    assert_eq!(output.status.code(), Some(5));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["code"]
            .as_str()
            .unwrap()
            .starts_with("CH-POLICY-"),
        "{}",
        envelope["error"]["code"]
    );
}

#[test]
fn an_abandoned_cycle_is_terminal_and_accepts_nothing_further() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.cycle(&["abandon", "--cycle-id", "C-001", "--reason", "superseded"]);

    let status = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(status["data"]["status"], "abandoned");

    // Every onward transition is refused.
    for attempt in [
        vec!["activate", "--cycle-id", "C-001"],
        vec!["abandon", "--cycle-id", "C-001", "--reason", "again"],
    ] {
        let output = workspace.cycle_raw(&attempt);
        assert_eq!(
            output.status.code(),
            Some(5),
            "an abandoned cycle must refuse {attempt:?}"
        );
    }
}

#[test]
fn status_is_derived_from_authoritative_events() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);

    let status = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(status["data"]["status"], "active");
    assert_eq!(status["data"]["event_count"], 2, "created then activated");
    assert_eq!(status["data"]["status_matches_history"], true);
}

#[test]
fn a_stored_status_that_disagrees_with_history_is_surfaced_not_trusted() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);

    // Simulate an external edit to the cached field. History must win.
    workspace.tamper_cycle_status("C-001", "closed");

    let output = workspace.cycle_raw(&["status", "--cycle-id", "C-001"]);
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["status"], "active",
        "history is authoritative"
    );
    assert_eq!(envelope["data"]["stored_status"], "closed");
    assert_eq!(envelope["data"]["status_matches_history"], false);
    assert!(
        envelope["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("disagrees")),
        "the drift must be reported"
    );
}

#[test]
fn a_duplicate_cycle_identifier_is_refused() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    let output = workspace.cycle_raw(&["create", "--cycle-id", "C-001", "--objective", "Again"]);
    assert_eq!(output.status.code(), Some(5));
}

#[test]
fn status_for_an_unknown_cycle_is_a_precondition_failure() {
    let workspace = Workspace::initialized();
    let output = workspace.cycle_raw(&["status", "--cycle-id", "C-999"]);
    assert_eq!(output.status.code(), Some(4));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-PRECONDITION-NOT-FOUND");
}

#[test]
fn a_dry_run_changes_nothing() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    let before = workspace.control_head();

    let envelope = workspace.cycle_json(&["activate", "--cycle-id", "C-001", "--dry-run"]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(envelope["data"]["baseline_sha"].as_str().unwrap().len(), 40);

    assert_eq!(workspace.control_head(), before, "control must not advance");
    assert_eq!(
        workspace.cycle_json(&["status", "--cycle-id", "C-001"])["data"]["status"],
        "draft"
    );
}

#[test]
fn every_cycle_mutation_lands_in_control_history() {
    let workspace = Workspace::initialized();
    let after_init = workspace.control_commit_count();

    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);

    assert_eq!(
        workspace.control_commit_count(),
        after_init + 2,
        "create and activate each commit exactly once"
    );
}

#[test]
fn events_are_versioned_but_the_journal_is_not() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);

    let tracked = workspace.control_tracked_files();
    assert!(
        tracked.iter().any(|path| path.starts_with("events/")),
        "events are authoritative: {tracked:?}"
    );
    assert!(
        tracked.iter().any(|path| path.starts_with("cycles/")),
        "cycle records are authoritative: {tracked:?}"
    );
    assert!(
        !tracked.iter().any(|path| path.starts_with("journal/")),
        "the journal is in-flight state, not history: {tracked:?}"
    );
}
