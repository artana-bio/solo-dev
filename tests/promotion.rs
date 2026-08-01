//! `WP-450` acceptance: acceptance decisions and promotion.
//!
//! Promotion is the only irreversible step in the whole harness. Every test
//! here is about the same question from a different angle: does everything that
//! can be checked get checked *before* the protected branch moves, and does
//! nothing rewind it afterwards?

mod support;

use std::fs;

use support::Workspace;

/// A cycle carried all the way to a reviewed integration.
fn reviewed(count: usize) -> (Workspace, String) {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    for index in 1..=count {
        let card = format!("F-{index:03}");
        workspace.activate_card(&card, &[&format!("src/{card}/**")]);
        workspace.approve_card(&card, &format!("src/{card}/a.rs"));
    }

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
    (workspace, id)
}

/// The same, accepted by the named owner.
fn accepted(count: usize) -> (Workspace, String) {
    let (workspace, id) = reviewed(count);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "owner",
    ]);
    (workspace, id)
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

fn landing_of(workspace: &Workspace, id: &str) -> String {
    workspace.integration_json(&["inspect", "--integration-id", id])["data"]["landing_sha"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn an_acceptance_binds_the_exact_landing_commit_and_its_evidence() {
    let (workspace, id) = reviewed(2);
    let landing = landing_of(&workspace, &id);

    let envelope = workspace.acceptance_json(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "owner",
        "--residual-risk",
        "the migration is untested at scale",
    ]);
    assert_eq!(envelope["data"]["landing_sha"], landing);
    assert_eq!(envelope["data"]["decision"], "accepted");
    assert_eq!(envelope["data"]["authorizes_promotion"], true);
    assert_eq!(envelope["data"]["acceptance_owner"], "owner");
    assert!(
        !envelope["data"]["receipt_ids"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the decision must name the evidence it rests on"
    );
    assert!(
        envelope["data"]["rollback_reference"]
            .as_str()
            .unwrap()
            .contains(&landing),
        "a rollback reference that does not name the commit is useless"
    );

    assert_eq!(
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["status"],
        "accepted"
    );
}

#[test]
fn the_implementer_cannot_accept_their_own_integration() {
    // Acceptance is the only thing that authorizes moving the protected
    // branch. Both reviews already refuse a self-verdict; the step they lead
    // to did not, which left the authorization itself self-grantable.
    // `approve_card` hands off as the default actor, `operator`.
    let (workspace, id) = reviewed(1);

    let output = workspace.acceptance_raw(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "operator",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-SAME-ACTOR");
    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(
        rendered.contains("F-001"),
        "the refusal names the card they wrote: {rendered}"
    );
}

#[test]
fn a_spelling_variant_does_not_get_around_the_separation_check() {
    // The comparison was exact equality, so a trailing space or a capital
    // letter was a different person as far as the harness was concerned.
    let (workspace, id) = reviewed(1);

    let output = workspace.acceptance_raw(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        " Operator ",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-SAME-ACTOR");
}

#[test]
fn the_implementer_cannot_promote_their_own_integration() {
    let (workspace, id) = accepted(1);

    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "operator"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-SAME-ACTOR");
}

#[test]
fn a_promote_dry_run_refuses_the_author_too() {
    // The dry run runs the real preconditions, so a separation refusal must
    // surface there rather than waiting for the real promotion. Covered
    // separately because `preview_promote` reaches `check_promotion` by its
    // own path.
    let (workspace, id) = accepted(1);

    let output = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &id,
        "--actor-id",
        "operator",
        "--dry-run",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-SAME-ACTOR");
}

#[test]
fn the_integration_reviewer_cannot_be_the_verifier_under_a_spelling_variant() {
    // The card-level self-review check and this one are separate comparisons
    // in separate files; normalizing one said nothing about the other.
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

    let output = workspace.integration_raw(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        " Verifier ",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-SAME-ACTOR");
}

#[test]
fn the_acceptance_owner_may_promote_their_own_decision() {
    // Deliberately permitted, and the reason the promoter rule is written
    // against implementers rather than "everyone else". Section 15.1's model
    // is one human and many agent sessions: the human accepts and the human
    // promotes. A rule requiring a fourth distinct party there would make the
    // documented way of working impossible, which is how a control ends up
    // being worked around instead of kept.
    let (workspace, id) = accepted(1);

    let envelope =
        workspace.integration_json(&["promote", "--integration-id", &id, "--actor-id", "owner"]);
    assert_eq!(envelope["status"], "success");
}

#[test]
fn an_unreviewed_integration_cannot_be_accepted() {
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

    let output = workspace.acceptance_raw(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "owner",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
}

#[test]
fn a_rejection_is_recorded_and_blocks_promotion() {
    let (workspace, id) = reviewed(1);
    let envelope = workspace.acceptance_json(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "owner",
        "--reject",
    ]);
    assert_eq!(envelope["data"]["decision"], "rejected");
    assert_eq!(envelope["data"]["authorizes_promotion"], false);

    // A rejection advances nothing: the work reaching review is a fact worth
    // keeping, and there is no Section 11.3 transition for a refusal.
    assert_eq!(
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["status"],
        "reviewed"
    );

    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(output.status.code(), Some(5));
}

#[test]
fn an_exact_accepted_landing_promotes() {
    let (workspace, id) = accepted(2);
    let landing = landing_of(&workspace, &id);
    let before = workspace.authority_head();

    let envelope =
        workspace.integration_json(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(envelope["data"]["landing_sha"], landing);
    assert_eq!(envelope["data"]["previous_main_sha"], before);
    assert_eq!(envelope["data"]["status"], "promoted");

    assert_eq!(
        workspace.authority_head(),
        landing,
        "the protected branch must now be the accepted commit"
    );
    assert_eq!(
        workspace.candidate_head(),
        landing,
        "the local protected worktree must be fast-forwarded to match"
    );
    for card in ["F-001", "F-002"] {
        assert_eq!(
            workspace.card_json(&["status", "--card-id", card])["data"]["state"],
            "landed"
        );
    }
}

#[test]
fn a_moved_authority_branch_fails_before_the_update() {
    let (workspace, id) = accepted(1);
    // Someone lands directly on the protected branch after acceptance.
    let moved = workspace.advance_authority();

    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(output.status.code(), Some(6));
    assert_eq!(error_code(&output), "CH-CONFLICT-CONTROL-HEAD-MOVED");
    assert_eq!(
        workspace.authority_head(),
        moved,
        "a refused promotion must leave the branch exactly as it found it"
    );
}

#[test]
fn a_dirty_local_main_fails_before_the_authority_update() {
    let (workspace, id) = accepted(1);
    let authority_before = workspace.authority_head();
    // Uncommitted work in the protected worktree, which promotion would
    // otherwise fast-forward over.
    fs::write(workspace.repository.join("scratch.txt"), "in progress\n").unwrap();
    support::git(&workspace.repository, &["add", "-A"]);

    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-WORKTREE-DIRTY");
    assert_eq!(
        workspace.authority_head(),
        authority_before,
        "the authority must not move when a later step would have failed"
    );
    assert!(
        workspace.repository.join("scratch.txt").exists(),
        "the uncommitted work must be untouched"
    );
}

#[test]
fn a_stale_local_main_fails_before_the_authority_update() {
    let (workspace, id) = accepted(1);
    let authority_before = workspace.authority_head();
    // The local protected branch moves away from the commit the plan expects.
    workspace.commit_candidate("local-only.txt", "not pushed\n");

    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-LOCAL-MAIN-STALE");
    assert_eq!(workspace.authority_head(), authority_before);
}

#[test]
fn promotion_without_an_acceptance_is_refused() {
    let (workspace, id) = reviewed(1);
    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
}

#[test]
fn an_integration_cannot_be_accepted_twice() {
    let (workspace, id) = accepted(1);
    let output = workspace.acceptance_raw(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "someone-else",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
}

#[test]
fn an_integration_cannot_be_promoted_twice() {
    let (workspace, id) = accepted(1);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    let landing = workspace.authority_head();

    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(
        workspace.authority_head(),
        landing,
        "a refused second promotion must not disturb the branch"
    );
}

#[test]
fn a_promote_dry_run_checks_everything_and_moves_nothing() {
    let (workspace, id) = accepted(2);
    let authority_before = workspace.authority_head();
    let control_before = workspace.control_head();

    let envelope = workspace.integration_json(&[
        "promote",
        "--integration-id",
        &id,
        "--actor-id",
        "promoter",
        "--dry-run",
    ]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(envelope["data"]["landing_sha"], landing_of(&workspace, &id));

    assert_eq!(workspace.authority_head(), authority_before);
    assert_eq!(workspace.control_head(), control_before);
}

#[test]
fn a_dry_run_surfaces_the_same_refusal_a_real_promotion_would() {
    let (workspace, id) = accepted(1);
    fs::write(workspace.repository.join("scratch.txt"), "in progress\n").unwrap();
    support::git(&workspace.repository, &["add", "-A"]);

    let output = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &id,
        "--actor-id",
        "promoter",
        "--dry-run",
    ]);
    assert_eq!(
        error_code(&output),
        "CH-PRECONDITION-WORKTREE-DIRTY",
        "a dry run that cannot predict the real refusal is worthless"
    );
}

#[test]
fn promotion_records_evidence_naming_the_acceptance_it_rested_on() {
    let (workspace, id) = accepted(1);
    let landing = landing_of(&workspace, &id);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);

    let event = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "integration.promoted")
        .expect("promotion must be recorded");
    assert_eq!(event["head_sha"], landing);
    assert_eq!(event["metadata"]["landing_sha"], landing);
    assert!(
        event["metadata"]["acceptance_id"]
            .as_str()
            .unwrap()
            .starts_with("ACC-"),
        "the authorizing decision must be named: {event}"
    );
}

#[test]
fn a_read_command_is_not_labelled_as_the_write_it_reuses() {
    // Tier 4. `acceptance inspect` reused `report_acceptance` and so emitted
    // `command: "acceptance.record"` on success. A consumer routing on that
    // field would treat a read as a write — and the field exists for routing.
    let (workspace, id) = accepted(1);

    let recorded = workspace.acceptance_json(&["inspect", "--integration-id", &id]);
    assert_eq!(recorded["command"], "acceptance.inspect");
    assert_eq!(recorded["status"], "success");

    // And the write still says what it is, so this is not a blanket rename.
    let (other, second) = reviewed(1);
    let written = other.acceptance_json(&[
        "record",
        "--integration-id",
        &second,
        "--acceptance-owner",
        "owner",
    ]);
    assert_eq!(written["command"], "acceptance.record");
}

#[test]
fn acceptance_inspect_reports_the_decision_without_promoting() {
    let (workspace, id) = accepted(1);
    let authority_before = workspace.authority_head();

    let envelope = workspace.acceptance_json(&["inspect", "--integration-id", &id]);
    assert_eq!(envelope["data"]["decision"], "accepted");
    assert_eq!(
        envelope["status"], "success",
        "inspection is a read, not a decision"
    );
    assert!(
        envelope["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("promote")),
        "inspection must not read as though it promoted anything"
    );
    assert_eq!(workspace.authority_head(), authority_before);
}

#[test]
fn a_local_sync_failure_after_promotion_requires_recovery_and_does_not_rewind() {
    let (workspace, id) = accepted(1);
    let landing = landing_of(&workspace, &id);
    let authority_before = workspace.authority_head();

    // Under normal conditions the fast-forward cannot fail — the landing
    // commit descends from the commit the precondition check confirmed. The
    // realistic trigger is another Git process holding the index lock, which
    // is exactly the interruption Section 13.6 anticipates.
    fs::write(workspace.repository.join(".git/index.lock"), "").unwrap();

    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(
        output.status.code(),
        Some(9),
        "a promoted authority with an unsynchronized local is recovery-required"
    );
    let message = {
        let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        envelope["error"]["message"].as_str().unwrap().to_owned()
    };
    assert!(
        message.contains("authority_promoted_local_sync_pending"),
        "the recorded state must be named: {message}"
    );

    // Section 13.6: the authority must NOT be rolled back automatically.
    assert_eq!(
        workspace.authority_head(),
        landing,
        "promotion happened; rewinding a published branch would be worse than the gap"
    );
    assert_ne!(workspace.authority_head(), authority_before);

    // And the operation must be journaled as needing recovery, not as a clean
    // failure — the control tree is clean here, so only the error's own
    // category can tell the difference.
    let status = Workspace::run_json(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(
        status["data"]["unresolved_operations"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "`project recover` must be able to see this: {}",
        status["data"]
    );
}

#[test]
fn scenario_35_both_mvp_mutation_boundaries_are_recoverable() {
    // Section 16.2 scenario 35, scoped by the plan to the two boundaries MVP
    // packages journal: worktree allocation and promotion.
    let (workspace, id) = accepted(1);

    // Promotion boundary: the authority moved, the local did not.
    fs::write(workspace.repository.join(".git/index.lock"), "").unwrap();
    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(output.status.code(), Some(9));

    // `project recover` must name the operation and say what it was doing,
    // rather than reporting a generic unfinished state.
    let recover = Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    let unresolved = recover["data"]["unresolved_operations"]
        .as_array()
        .expect("an operation list");
    assert_eq!(unresolved.len(), 1, "unexpected: {}", recover["data"]);
    assert_eq!(unresolved[0]["command"], "integration.promote");
    assert!(
        unresolved[0]["operation_id"]
            .as_str()
            .unwrap()
            .starts_with("OP-"),
        "the operation must be identifiable: {}",
        unresolved[0]
    );
}

#[test]
fn scenario_35_an_allocation_boundary_is_recoverable() {
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

    // An allocation that cannot complete: the branch it would create is taken.
    support::git(
        &workspace.repository,
        &["branch", "card/F-001", "refs/heads/main"],
    );
    let output = workspace.work_raw(&["start", "--card-id", "F-001"]);
    assert!(!output.status.success());

    // A refusal that wrote nothing must not demand recovery — a recover
    // command that cries wolf is one nobody reads.
    let status = Workspace::run_json(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(
        status["data"]["unresolved_operations"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "a clean refusal is not an interrupted operation: {}",
        status["data"]
    );

    // And the next mutation is not blocked by it.
    support::git(&workspace.repository, &["branch", "-D", "card/F-001"]);
    workspace.work(&["start", "--card-id", "F-001"]);
}

#[test]
fn a_recovery_required_promotion_can_be_resumed_to_completion() {
    let (workspace, id) = accepted(2);
    let landing = landing_of(&workspace, &id);

    // Interrupt the promotion after the authority moves.
    fs::write(workspace.repository.join(".git/index.lock"), "").unwrap();
    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(output.status.code(), Some(9));
    assert_eq!(workspace.authority_head(), landing);
    assert_eq!(
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["status"],
        "accepted",
        "the control bookkeeping is the part left undone"
    );

    // Clear what blocked the fast-forward, then resume.
    fs::remove_file(workspace.repository.join(".git/index.lock")).unwrap();
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
    assert_eq!(resumed["data"]["landing_sha"], landing);

    // The end state is indistinguishable from an uninterrupted promotion.
    assert_eq!(workspace.authority_head(), landing);
    assert_eq!(
        workspace.candidate_head(),
        landing,
        "the local protected worktree must be caught up"
    );
    for card in ["F-001", "F-002"] {
        assert_eq!(
            workspace.card_json(&["status", "--card-id", card])["data"]["state"],
            "landed"
        );
    }

    // And nothing is left demanding attention.
    let status = Workspace::run_json(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(
        status["data"]["unresolved_operations"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "a resumed operation must stop blocking further work: {}",
        status["data"]
    );

    // The promotion is recorded once, by the actor who finished it.
    let promotions: Vec<_> = workspace
        .events()
        .into_iter()
        .filter(|event| event["event_type"] == "integration.promoted")
        .collect();
    assert_eq!(promotions.len(), 1, "unexpected: {promotions:?}");
    assert_eq!(promotions[0]["actor_id"], "recoverer");
}

#[test]
fn resuming_when_the_authority_never_moved_completes_nothing() {
    let (workspace, id) = accepted(1);
    let authority_before = workspace.authority_head();

    let resumed = Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--resume".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(
        resumed["data"]["resumed"], false,
        "resume must not invent a promotion that never happened"
    );

    assert_eq!(workspace.authority_head(), authority_before);
    assert_eq!(
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["status"],
        "accepted"
    );
}

#[test]
fn a_plan_changed_after_acceptance_is_refused_at_promotion() {
    // Tier 1, defect 3. Acceptance recorded `integration_record_digest` and
    // nothing ever compared it to anything. Worse, it was computed over the
    // whole record including `status`, which necessarily moves `reviewed →
    // accepted` immediately afterwards — so the check could not have passed had
    // anyone written it. A member appended after acceptance promoted: never
    // verified, never accepted, not in the tree the owner signed off.
    let (workspace, id) = accepted(2);

    let path = workspace.control.join(format!("integrations/{id}.json"));
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    // The fixture must be a real tamper, not a no-op rewrite: the members list
    // has to actually grow, or a refusal below proves nothing.
    let members = record["members"].as_array_mut().unwrap();
    let before = members.len();
    let mut extra = members[0].clone();
    extra["card_id"] = serde_json::json!("F-999");
    members.push(extra);
    assert_eq!(members.len(), before + 1, "the tamper must change the plan");

    fs::write(&path, serde_json::to_string_pretty(&record).unwrap()).unwrap();
    support::git(&workspace.control, &["add", "-A"]);
    support::git(&workspace.control, &["commit", "-q", "-m", "tamper"]);

    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert!(
        !output.status.success(),
        "a plan that changed after acceptance must not promote"
    );
    assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("changed after it was accepted"),
        "the refusal must say what is wrong: {message}"
    );
}

#[test]
fn the_acceptance_digest_ignores_the_status_it_necessarily_changes() {
    // The guard on the fix above. Acceptance sets `status` to `accepted` as
    // part of recording itself, so a digest covering `status` can never match
    // at promotion and every promotion would be refused. This asserts the
    // ordinary path still works — which is the whole reason the recorded digest
    // went unchecked for as long as it did.
    let (workspace, id) = accepted(1);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);

    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(workspace.control.join(format!("integrations/{id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(record["status"], "promoted");
}

#[test]
fn promoting_from_a_different_checked_out_branch_is_refused() {
    // Tier 3, defect 14. The local sync is `git merge --ff-only <landing>` in
    // the candidate repository, which fast-forwards whatever branch HEAD is on.
    // The preconditions check that `refs/heads/<protected>` is at the expected
    // commit and that the worktree is clean, but never that HEAD is that
    // branch. An operator sitting on a feature branch — the ordinary state —
    // had the landing commit fast-forwarded onto it, and the command reported
    // "local `main` fast-forwarded".
    let (workspace, id) = accepted(1);
    let landing = landing_of(&workspace, &id);
    let authority_before = workspace.authority_head();

    support::git(
        &workspace.repository,
        &["checkout", "-q", "-b", "sidetrack"],
    );

    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert!(
        !output.status.success(),
        "promotion must refuse rather than move the wrong branch"
    );
    assert_eq!(
        workspace.authority_head(),
        authority_before,
        "and it must refuse before the authority moves, not after"
    );
    assert_eq!(
        support::capture(&workspace.repository, &["rev-parse", "HEAD"]),
        support::capture(&workspace.repository, &["rev-parse", "refs/heads/main"]),
        "sidetrack must not have been fast-forwarded to the landing commit"
    );
    assert_ne!(
        support::capture(&workspace.repository, &["rev-parse", "HEAD"]),
        landing
    );

    // The guard: checking the protected branch back out makes it work. The
    // refusal must be about which branch is out, not a new blanket blocker.
    support::git(&workspace.repository, &["checkout", "-q", "main"]);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(workspace.authority_head(), landing);
}

#[test]
fn a_landed_card_cannot_be_revised() {
    // Tier 3, defect 19. `card revise` writes `ready` and never checks the
    // transition — the only guard was `is_terminal()`, and `landed` is not
    // terminal because it still has to reach `closed`. So revising a landed
    // card set it to `ready`, from which the only exits are `leased` and
    // `abandoned`: it could never be closed, and its integration referenced a
    // card whose state had gone backwards past the acceptance that authorized
    // it.
    let (workspace, id) = accepted(1);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "landed",
        "the fixture must reach the state under test"
    );

    let baseline = workspace.authority_head();
    let draft = workspace.root.join("F-001-v2.yaml");
    fs::write(
        &draft,
        format!(
            "card_id: F-001\ncycle_id: C-001\ntitle: Implement F-001\ngoal: Substantially different\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {baseline}\nwrite_scope:\n  include: [src/F-001/**]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n"
        ),
    )
    .unwrap();

    let output = workspace.card_raw(&[
        "revise",
        "--card-id",
        "F-001",
        "--draft",
        &draft.display().to_string(),
        "--reason",
        "second thoughts",
    ]);
    assert!(
        !output.status.success(),
        "a landed card must not be revisable: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "landed",
        "and the card must be left where it was, still able to close"
    );

    workspace.archive(&["create", "--integration-id", &id]);
    workspace.archive(&["close", "--integration-id", &id]);
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "closed"
    );
}

#[test]
fn a_landed_card_cannot_be_abandoned() {
    let (workspace, id) = accepted(1);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "landed",
        "the fixture must reach the state Section 11.2 protects"
    );

    let output = workspace.card_raw(&["abandon", "--card-id", "F-001", "--reason", "too late"]);
    assert!(
        !output.status.success(),
        "landed cards must not be abandoned"
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "landed",
        "the refusal must leave the card able to close"
    );
}

#[test]
fn a_completed_integration_does_not_advance_the_cycle_status() {
    let (workspace, id) = accepted(1);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    workspace.archive(&["create", "--integration-id", &id]);
    workspace.archive(&["close", "--integration-id", &id]);

    let status = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(status["data"]["status"], "active");
    assert_eq!(status["data"]["status_matches_history"], true);
}

#[test]
fn abandoning_an_integration_does_not_abandon_its_cycle() {
    // `integration.abandoned`'s `next_state` is `IntegrationStatus::Abandoned`,
    // which serializes to the same string as `CycleStatus::Abandoned`. Nothing
    // in this file exercised `cycle status` after an integration abandon before
    // this test — the collision was real but had zero coverage, incidental or
    // otherwise, until an independent review of F-019 found it by enumerating
    // every `.cycle(...)` call site rather than trusting the fix's own tests.
    let (workspace, id) = reviewed(1);
    workspace.integration(&[
        "abandon",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
        "--reason",
        "the combination cannot be made to work in this cycle",
    ]);

    let status = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(
        status["data"]["status"], "active",
        "abandoning one integration must not fold the cycle to abandoned"
    );
    assert_eq!(status["data"]["status_matches_history"], true);
}
