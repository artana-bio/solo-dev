//! #80 frozen proof: a held lease with no recorded sign of life for at
//! least the silence threshold is visible from `project status` and
//! `project recover`, a lease that is actively checkpointing never is, and
//! `project recover`'s writeup names `work reclaim` as an option rather
//! than an instruction.
//!
//! A new file rather than an extension of `tests/control_state.rs` (which
//! already covers `project status`/`project recover` for other facts) or
//! `tests/interrupted_reservation_recovery.rs` (the sibling proof for
//! stranded validation reservations, #110's parallel mechanism): this file
//! is about a lease's own `granted_at`/`progress` timestamps, a fact
//! neither of those builds or asserts on, and keeping it separate keeps
//! each file's fixture story about one kind of recovery-visible fact.
//!
//! Post-review repair: `is_held()` never excludes a finished card's lease
//! on its own, so every lease ever granted would read as silent forever
//! past the threshold. `archive close` now releases a lease it cleans up,
//! but that only covers cards that reach an integration and land; a card
//! abandoned on its own terms never gets there, and its lease stays held
//! for good (#199). `a_finished_cards_stale_lease_is_never_reported_silent`,
//! below, is the case the four tests above could not reach: all four use a
//! card that stays `Active` throughout, so none of them exercise the
//! card-state exclusion `work.rs`'s `card_work_is_over` adds.

mod support;

use std::fs;

use support::Workspace;

/// A card with a fresh lease, the shape every test below tampers with.
fn allocated() -> (Workspace, String) {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Silent lease proof",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    let started = workspace.work_json(&["start", "--card-id", "F-001"]);
    let lease_id = started["data"]["lease_id"].as_str().unwrap().to_owned();
    (workspace, lease_id)
}

/// Directly rewrites a lease's `granted_at`, simulating one granted long
/// enough ago to be silent, the same "external edit" fixture technique
/// `Workspace::tamper_card_state` uses: left uncommitted on purpose, since
/// `ControlRepository::read` (`src/control/repository.rs`) reads the
/// working tree directly rather than a Git object, so an uncommitted edit
/// is exactly as visible to a reporting command as a committed one, and
/// `project status`/`project recover` never write, so there is nothing for
/// a dirty tree to interfere with here.
fn backdate_lease_grant(workspace: &Workspace, lease_id: &str) {
    let path = workspace.control.join(format!("leases/{lease_id}.json"));
    let raw = fs::read_to_string(&path).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    value["granted_at"] = serde_json::json!("1970-01-01T00:00:00Z");
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
}

fn project_status_json(workspace: &Workspace) -> serde_json::Value {
    Workspace::run_json(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ])
}

fn project_status_text(workspace: &Workspace) -> String {
    let output = Workspace::run(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        workspace.control.display().to_string(),
    ]);
    assert!(
        output.status.success(),
        "project status (text mode) must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn project_recover_json(workspace: &Workspace) -> serde_json::Value {
    Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ])
}

fn project_recover_text(workspace: &Workspace) -> String {
    let output = Workspace::run(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
    ]);
    assert!(
        output.status.success(),
        "project recover (text mode) must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// #80 required test 1 (§10 mutation 1's proof): a lease granted long
/// enough ago, with no checkpoints at all, is named by `project status`.
///
/// Fails if the threshold is made infinite (nothing is ever silent) or if
/// `project status` stops reading `silent_leases` at all.
#[test]
fn a_lease_silent_since_it_was_granted_is_named_by_project_status() {
    let (workspace, lease_id) = allocated();
    backdate_lease_grant(&workspace, &lease_id);

    let status = project_status_json(&workspace);
    let silent = status["data"]["silent_leases"]
        .as_array()
        .expect("silent_leases must be an array");
    assert_eq!(silent.len(), 1, "exactly one lease is silent: {status}");
    assert_eq!(silent[0]["lease_id"], lease_id);
    assert_eq!(silent[0]["card_id"], "F-001");
    assert_eq!(silent[0]["actor_id"], "operator");
    assert!(
        silent[0]["elapsed_seconds"].as_i64().unwrap() > 1_700_000_000,
        "elapsed_seconds must reflect a grant near the Unix epoch: {status}"
    );

    // The text-mode surface names it too, the same summary-line-plus-entry
    // shape stranded reservations already use.
    let text = project_status_text(&workspace);
    assert!(
        text.contains("silent leases: 1"),
        "text mode must summarize the count: {text}"
    );
    assert!(
        text.contains(&lease_id) && text.contains("F-001") && text.contains("operator"),
        "text mode must name the lease, card, and actor: {text}"
    );
}

/// #80 required test 2 (§10 mutation 2's proof, the false-positive
/// direction): a lease that was just granted, with no tampering at all, is
/// never reported silent.
///
/// Fails if the threshold is made zero (everything is silent), which is
/// exactly the mutation that makes an operator learn to ignore this
/// feature.
#[test]
fn a_freshly_granted_lease_is_never_reported_silent() {
    let (workspace, _lease_id) = allocated();

    let status = project_status_json(&workspace);
    let silent = status["data"]["silent_leases"]
        .as_array()
        .expect("silent_leases must be an array");
    assert!(
        silent.is_empty(),
        "a lease granted moments ago must never be reported silent: {status}"
    );

    let recover = project_recover_json(&workspace);
    assert_eq!(recover["data"]["recovery_required"], false);
    assert_eq!(
        recover["data"]["silent_leases"].as_array().unwrap().len(),
        0
    );
}

/// #80 required test 3 (§10 mutation 3's proof): a lease granted long ago
/// but checkpointed a moment ago is not silent — a checkpoint is a sign of
/// life the report must actually consult, not just `granted_at`.
///
/// Fails if `silent_leases` (or `LeaseRecord::last_activity_at`) stops
/// reading `progress` and looks only at `granted_at`.
#[test]
fn a_recent_checkpoint_keeps_an_anciently_granted_lease_off_the_silent_list() {
    let (workspace, lease_id) = allocated();
    backdate_lease_grant(&workspace, &lease_id);

    // Elapsed time since `granted_at` alone is now decades; a checkpoint
    // recorded just now must override that for `last_activity_at`.
    workspace.work(&["checkpoint", "--card-id", "F-001", "--note", "still going"]);

    let status = project_status_json(&workspace);
    let silent = status["data"]["silent_leases"]
        .as_array()
        .expect("silent_leases must be an array");
    assert!(
        silent.is_empty(),
        "a lease checkpointed moments ago must not be reported silent even though it was \
         granted decades ago: {status}"
    );
}

/// #80 §6.3's proof: `project recover`'s writeup names `work reclaim` as an
/// available option, never as a step to run. Also pins the §6.2 decision
/// that a silent lease alone (no unresolved operation, no stranded
/// reservation) does not flip `recovery_required`, while still appearing
/// in both the text and the JSON.
///
/// Fails if the wording is changed to a "confirm ..., then run `work
/// reclaim ...` to release/reclaim it" instruction — the same shape
/// `project recover` uses for a stranded reservation's `gate settle`, which
/// is safe there because settling is non-destructive and destructive here
/// because reclaiming hands the card to someone else. It would also fail if
/// `work reclaim` were dropped from the text entirely (the option must
/// still be named) or if `recovery_required` started counting a silent
/// lease as torn state.
#[test]
fn project_recover_names_reclaim_as_an_option_not_an_instruction() {
    let (workspace, lease_id) = allocated();
    backdate_lease_grant(&workspace, &lease_id);

    let recover = project_recover_json(&workspace);
    assert_eq!(
        recover["data"]["recovery_required"], false,
        "a silent lease alone is not torn journal/reservation state"
    );
    let silent = recover["data"]["silent_leases"]
        .as_array()
        .expect("silent_leases must be an array");
    assert_eq!(silent.len(), 1);
    assert_eq!(silent[0]["lease_id"], lease_id);

    let text = project_recover_text(&workspace);
    assert!(
        text.contains("work reclaim"),
        "must name the option: {text}"
    );
    assert!(
        text.contains("does not mean") || text.contains("cannot tell"),
        "must hedge rather than assert the actor is gone: {text}"
    );
    assert!(
        !text.contains("run `work reclaim") && !text.contains("to reclaim it"),
        "must not read as an instruction to reclaim: {text}"
    );
    assert!(
        text.contains(&lease_id) && text.contains("F-001"),
        "must name this exact lease and card, not speak generically: {text}"
    );

    // Recover must not have written anything while merely reporting.
    let before = workspace.control_head();
    let _ = project_recover_json(&workspace);
    assert_eq!(
        workspace.control_head(),
        before,
        "project recover must not write to control while reporting a silent lease"
    );
}

/// Post-review repair's required proof: a lease for a card whose own work
/// is over (here, abandoned) is never reported silent, no matter how old
/// `granted_at` is. `card abandon` moves the card there directly from
/// `active` without touching the lease at all, and an abandoned card never
/// joins an integration, so the one path that does release a lease —
/// `archive close`, via `release_lease` — never reaches it. Its lease
/// stays `held` permanently, which is exactly why `is_held()` alone cannot
/// tell this case apart from a genuinely stuck one; only card state can.
///
/// This is the case §10 mutation 2 could not reach: that test (and every
/// other test in this file) uses a card left `active` throughout, so none
/// of them exercise `work.rs`'s `card_work_is_over` exclusion at all.
///
/// Fails if `silent_leases` stops consulting card state and goes back to
/// filtering `is_held()` alone.
#[test]
fn a_finished_cards_stale_lease_is_never_reported_silent() {
    let (workspace, lease_id) = allocated();
    backdate_lease_grant(&workspace, &lease_id);
    workspace.card(&[
        "abandon",
        "--card-id",
        "F-001",
        "--reason",
        "no longer needed",
    ]);

    let status = project_status_json(&workspace);
    let silent = status["data"]["silent_leases"]
        .as_array()
        .expect("silent_leases must be an array");
    assert!(
        silent.is_empty(),
        "a lease for an abandoned card must never be reported silent, however old \
         granted_at is: {status}"
    );

    let recover = project_recover_json(&workspace);
    assert_eq!(recover["data"]["recovery_required"], false);
    assert_eq!(
        recover["data"]["silent_leases"].as_array().unwrap().len(),
        0,
        "project recover must not name it either: {recover}"
    );
}
