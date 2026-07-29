//! `WP-500` acceptance: injectable interruption at every journaled boundary.
//!
//! Waiting for a natural interruption tests whichever boundary happens to be
//! slow. These tests provoke one at each boundary deliberately, which is the
//! only way to cover all of them.
//!
//! Everything here drives the CLI directly rather than through the shared
//! fixture, because this card's write scope does not include it — the harness
//! would refuse the handoff otherwise, which is the ownership model working.

mod support;

use std::process::{Command, Output};

use support::Workspace;

/// Runs the harness with one journal step selected for deliberate failure.
fn run_failing_at(step: &str, args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .env("CHANGE_HARNESS_FAIL_AT", step)
        .args(args)
        .output()
        .expect("the CLI should start")
}

/// The unresolved operations `project status` reports.
fn unresolved(workspace: &Workspace) -> Vec<serde_json::Value> {
    let envelope = Workspace::run_json(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    envelope["data"]["unresolved_operations"]
        .as_array()
        .expect("an operation list")
        .clone()
}

/// A cycle with one approved card, ready to integrate.
fn approved() -> Workspace {
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
    workspace
}

#[test]
fn an_injected_interruption_never_reports_success() {
    let workspace = Workspace::initialized();
    let args: Vec<String> = vec![
        "cycle".into(),
        "create".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "First slice".into(),
        "--output".into(),
        "json".into(),
    ];

    let output = run_failing_at("control-write", &args);
    assert!(
        !output.status.success(),
        "an interrupted command must not exit zero"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["status"], "error");
    assert_eq!(
        envelope["error"]["code"],
        "CH-RECOVERY-INCOMPLETE-OPERATION"
    );
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("control-write"),
        "the interrupted boundary must be named: {envelope}"
    );
}

#[test]
fn an_injected_interruption_leaves_the_operation_unresolved() {
    let workspace = Workspace::initialized();
    let args: Vec<String> = vec![
        "cycle".into(),
        "create".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "First slice".into(),
    ];
    assert!(!run_failing_at("control-write", &args).status.success());

    let pending = unresolved(&workspace);
    assert_eq!(pending.len(), 1, "unexpected: {pending:?}");
    assert_eq!(pending[0]["command"], "cycle.create");
    assert_eq!(
        pending[0]["steps"],
        serde_json::json!(["control-write"]),
        "the journal must name the boundary that was reached"
    );
}

#[test]
fn an_unresolved_operation_blocks_further_mutation() {
    let workspace = Workspace::initialized();
    let args: Vec<String> = vec![
        "cycle".into(),
        "create".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "First slice".into(),
    ];
    assert!(!run_failing_at("control-write", &args).status.success());

    // The next mutation must refuse rather than build on an unknown state.
    let blocked = workspace.cycle_raw(&[
        "create",
        "--cycle-id",
        "C-002",
        "--objective",
        "Second slice",
    ]);
    assert_eq!(blocked.status.code(), Some(9));
    let envelope: serde_json::Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(
        envelope["error"]["code"],
        "CH-RECOVERY-INCOMPLETE-OPERATION"
    );
}

#[test]
fn an_interruption_before_the_authority_moves_leaves_it_untouched() {
    let workspace = approved();
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
    let authority_before = workspace.authority_head();

    let args: Vec<String> = vec![
        "integration".into(),
        "promote".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--integration-id".into(),
        id.clone(),
        "--actor-id".into(),
        "promoter".into(),
    ];
    assert!(
        !run_failing_at("authority-publish-attempted", &args)
            .status
            .success()
    );

    assert_eq!(
        workspace.authority_head(),
        authority_before,
        "an interruption before the publish must leave the branch alone"
    );
    // And resuming must not invent a promotion that never happened.
    let resumed = Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--resume".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(resumed["data"]["resumed"], false);
    assert_eq!(workspace.authority_head(), authority_before);
}

#[test]
fn an_interruption_after_the_authority_moves_is_resumable() {
    let workspace = approved();
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
    let landing =
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["landing_sha"]
            .as_str()
            .unwrap()
            .to_owned();

    // Interrupt at the boundary immediately after the publish — the hardest
    // case, because the authority has moved and nothing has recorded it.
    let args: Vec<String> = vec![
        "integration".into(),
        "promote".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--integration-id".into(),
        id.clone(),
        "--actor-id".into(),
        "promoter".into(),
    ];
    let output = run_failing_at("authority-published", &args);
    assert_eq!(output.status.code(), Some(9));
    assert_eq!(
        workspace.authority_head(),
        landing,
        "the publish happened and must not be rolled back"
    );
    assert_eq!(
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["status"],
        "accepted",
        "the control bookkeeping is what was left undone"
    );

    let resumed = Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--resume".into(),
        "--actor-id".into(),
        "recoverer".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(resumed["data"]["status"], "promoted");
    assert_eq!(workspace.candidate_head(), landing);
    assert!(unresolved(&workspace).is_empty());
}

#[test]
fn an_interrupted_allocation_leaves_no_ambiguous_work_and_is_retryable() {
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

    let args: Vec<String> = vec![
        "work".into(),
        "start".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--card-id".into(),
        "F-001".into(),
    ];
    assert!(!run_failing_at("worktree-added", &args).status.success());

    // The branch was created; the worktree was not. Recovery must not guess:
    // it reports, and deletes nothing.
    let report = Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(report["data"]["recovery_required"], true);
    assert!(
        workspace.candidate_branch_exists("card/F-001"),
        "recovery must not delete work whose disposition is ambiguous"
    );
}

#[test]
fn a_completed_operation_is_recognized_and_needs_no_recovery() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);

    // Nothing was interrupted, so both forms of recover must say so, and
    // neither may write anything.
    let before = workspace.control_head();
    let report = Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(report["data"]["recovery_required"], false);

    let resumed = Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--resume".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(resumed["data"]["resumed"], false);
    assert_eq!(workspace.control_head(), before);
}

#[test]
fn every_mutating_command_names_at_least_one_boundary() {
    // The acceptance criterion is that injection reaches *every* journaled
    // boundary. A command with no named step has no boundary to reach, so the
    // criterion is checked against the source rather than assumed.
    let sources = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src/commands"))
        .expect("the commands directory");
    let mut missing = Vec::new();
    for entry in sources.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let body = std::fs::read_to_string(&path).expect("a source file");
        if body.contains("with_transaction(") && !body.contains("steps.at(") {
            missing.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    assert!(
        missing.is_empty(),
        "these command modules open transactions but name no boundary: {missing:?}"
    );
}

#[test]
fn resume_does_not_mark_an_operation_it_cannot_resume_as_complete() {
    // Tier 2, defect 8, found independently by three reviewers. `--resume`
    // marked *every* unresolved entry `Completed` and then called a resume path
    // that only knows how to finish promotions. For anything else — an
    // allocation, a merge, an activation — the entry was closed as though its
    // work had been done, the resume reported "nothing to resume", and the only
    // record that a partial mutation had happened was gone. `project status`
    // then reported a healthy project over a half-made allocation.
    //
    // The guard against over-narrowing this is
    // `an_interruption_after_the_authority_moves_is_resumable`, which holds the
    // one case `--resume` exists for: it must still settle the entry and
    // complete the promotion.
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

    let args: Vec<String> = vec![
        "work".into(),
        "start".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--card-id".into(),
        "F-001".into(),
    ];
    assert!(!run_failing_at("worktree-added", &args).status.success());

    // The fixture must actually be unresolved, or the rest proves nothing.
    assert_eq!(
        unresolved(&workspace).len(),
        1,
        "the interruption must have left an unresolved entry"
    );

    let output = Workspace::run(&[
        "project".into(),
        "recover".into(),
        "--resume".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--actor-id".into(),
        "operator".into(),
        "--output".into(),
        "json".into(),
    ]);

    assert!(
        !output.status.success(),
        "resume must not report success for an operation it cannot resume: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        unresolved(&workspace).len(),
        1,
        "the entry must survive: it is the only record that a partial mutation happened"
    );
    assert!(
        workspace.candidate_branch_exists("card/F-001"),
        "and the half-made allocation must still be there to recover"
    );
}

#[test]
fn an_allocation_that_created_a_worktree_is_reported_as_needing_recovery() {
    // Tier 2, defect 11. The partial/clean decision inspects `control.is_clean()`
    // and nothing else. `work start` creates a branch, a worktree, a worktree
    // lock, and a locator before it writes the lease into control — so a failure
    // anywhere in that stretch leaves the control repository genuinely clean,
    // the operation journalled `FailedClean`, and `recover` reporting that
    // nothing is wrong. The card holds a branch and a worktree it has no lease
    // for, and the branch name is taken, so `work start` cannot be retried.
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

    let args: Vec<String> = vec![
        "work".into(),
        "start".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--card-id".into(),
        "F-001".into(),
    ];
    assert!(!run_failing_at("worktree-locked", &args).status.success());

    // The fixture must have left real artifacts outside control, or there is
    // nothing for the decision to get wrong.
    assert!(
        workspace.candidate_branch_exists("card/F-001"),
        "the branch must exist for this to be a partial allocation"
    );
    assert!(
        workspace.worktrees.join("F-001").exists(),
        "and so must the worktree"
    );

    let report = Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(
        report["data"]["recovery_required"], true,
        "a half-made allocation is not a clean failure: {report}"
    );
}

#[test]
fn an_interruption_after_the_local_sync_is_recoverable() {
    // Tier 3, defect 15. `settle_promotion` runs after the authority has
    // already moved, and journaled nothing at all — contrary to Section 7.4,
    // which requires a step before the mutation it names. So an interruption
    // between the local fast-forward and the control commit was attributable to
    // nothing, could not be reproduced deliberately, and left the integration
    // marked `promoted` with its cards short of `landed`. `resume_promotion`
    // required the record to be exactly `Accepted`, so it skipped that shape
    // and reported "no promotion reached the authority" over an authority
    // plainly holding the landing commit.
    let workspace = approved();
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
    let landing =
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["landing_sha"]
            .as_str()
            .unwrap()
            .to_owned();

    // Interrupt after the record says promoted but before the cards do.
    let args: Vec<String> = vec![
        "integration".into(),
        "promote".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--integration-id".into(),
        id.clone(),
        "--actor-id".into(),
        "promoter".into(),
    ];
    let output = run_failing_at("cards-marked-landed", &args);
    assert_eq!(output.status.code(), Some(9), "recovery is required");
    assert_eq!(
        workspace.authority_head(),
        landing,
        "the fixture must be past the publish, or it proves nothing"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "accepted",
        "and the cards must not yet be landed"
    );

    let resumed = Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--resume".into(),
        "--actor-id".into(),
        "recoverer".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(
        resumed["data"]["status"], "promoted",
        "recovery must finish it rather than report that nothing happened"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "landed"
    );
    assert!(unresolved(&workspace).is_empty());
}
