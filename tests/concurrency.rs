//! `WP-510` acceptance: concurrency and lease hardening.
//!
//! Two claims are load-bearing here. A lock is only trusted when its holder is
//! provably alive, because a PID is a recycled number and trusting one blocks
//! the harness on a process that has nothing to do with it. And a lease can be
//! taken over without losing work, because an abandoned lease is a
//! coordination problem, not a reason to destroy code.

mod support;

use std::fs;

use support::Workspace;

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

/// Writes a lock file naming a chosen holder.
fn plant_lock(workspace: &Workspace, pid: u32, process_start: Option<&str>) {
    let holder = serde_json::json!({
        "pid": pid,
        "acquired_at": "2026-07-28T00:00:00Z",
        "operation": "something.interrupted",
        "process_start": process_start,
    });
    fs::write(
        workspace.control.join("harness.lock"),
        serde_json::to_string_pretty(&holder).unwrap(),
    )
    .unwrap();
}

/// The lock section of `project status`.
fn lock_state(workspace: &Workspace) -> serde_json::Value {
    Workspace::run_json(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ])["data"]["lock"]
        .clone()
}

#[test]
fn a_lock_left_by_a_dead_process_is_reported_stale() {
    let workspace = Workspace::initialized();
    plant_lock(&workspace, 4_294_967_294, Some("whenever"));

    let lock = lock_state(&workspace);
    assert_eq!(lock["state"], "stale");
    assert!(
        lock["reason"]
            .as_str()
            .unwrap()
            .contains("no longer running"),
        "unexpected: {lock}"
    );
}

#[test]
fn a_stale_lock_names_itself_rather_than_blocking_silently() {
    let workspace = Workspace::initialized();
    plant_lock(&workspace, 4_294_967_294, Some("whenever"));

    let output = workspace.cycle_raw(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    assert!(!output.status.success());
    assert_eq!(error_code(&output), "CH-POLICY-STALE-LOCK");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["recovery"]
            .as_str()
            .unwrap()
            .contains("recover --resume"),
        "the operator must be told what to do: {envelope}"
    );
}

#[test]
fn recovery_clears_a_provably_stale_lock() {
    let workspace = Workspace::initialized();
    plant_lock(&workspace, 4_294_967_294, Some("whenever"));

    Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--resume".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(lock_state(&workspace)["state"], "free");

    // And work proceeds again.
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
}

#[test]
fn a_reused_pid_does_not_silently_steal_the_lock() {
    let workspace = Workspace::initialized();
    // A live PID — this test process — but a start instant that does not
    // match. That is exactly what PID reuse looks like from the outside, and
    // trusting it would block the harness on an unrelated program forever.
    plant_lock(&workspace, std::process::id(), Some("a different instant"));

    let lock = lock_state(&workspace);
    assert_eq!(lock["state"], "stale");
    assert!(
        lock["reason"].as_str().unwrap().contains("PID was reused"),
        "unexpected: {lock}"
    );
}

#[test]
fn an_unprovable_lock_requires_a_human_decision() {
    let workspace = Workspace::initialized();
    // A live PID with no recorded start time: it might be the real holder, or
    // it might be a reuse. Nothing can tell, so nothing may clear it.
    plant_lock(&workspace, std::process::id(), None);

    let lock = lock_state(&workspace);
    assert_eq!(lock["state"], "ambiguous");

    let output = workspace.cycle_raw(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    assert_eq!(error_code(&output), "CH-POLICY-LOCK-AMBIGUOUS");

    // Recovery must not resolve it by optimism.
    Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--resume".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(
        lock_state(&workspace)["state"],
        "ambiguous",
        "an unprovable lock must survive recovery untouched"
    );
}

#[test]
fn a_genuinely_held_lock_still_refuses_a_second_mutation() {
    // The regression this package must not cause: real contention still wins.
    let workspace = Workspace::initialized();
    plant_lock(&workspace, std::process::id(), None);
    let output = workspace.cycle_raw(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    assert!(
        !output.status.success(),
        "contention must still block, whatever the diagnosis"
    );
}

/// A cycle with one card allocated to `operator`.
fn allocated() -> Workspace {
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
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace
}

#[test]
fn reclaiming_a_lease_preserves_every_candidate_commit() {
    let workspace = allocated();
    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src/F-001")).unwrap();
    fs::write(worktree.join("src/F-001/a.rs"), "// work\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: work in progress"]);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);

    let envelope = workspace.work_json(&[
        "reclaim",
        "--card-id",
        "F-001",
        "--actor-id",
        "second-operator",
        "--reason",
        "the original actor's session ended",
    ]);
    assert_eq!(envelope["data"]["to_actor"], "second-operator");
    assert_eq!(
        envelope["data"]["preserved_head"], head,
        "the reclaim must name the commit it preserved"
    );

    // The work itself is untouched: same branch, same head, same files.
    assert_eq!(support::capture(&worktree, &["rev-parse", "HEAD"]), head);
    assert!(worktree.join("src/F-001/a.rs").exists());
    assert!(workspace.candidate_branch_exists("card/F-001"));
}

#[test]
fn a_reclaimed_lease_lets_the_new_actor_continue() {
    let workspace = allocated();
    workspace.work(&[
        "reclaim",
        "--card-id",
        "F-001",
        "--actor-id",
        "second-operator",
        "--reason",
        "handover",
    ]);

    // The locator was rewritten, so resume agrees with control state.
    workspace.work(&["resume", "--card-id", "F-001"]);
    let status = workspace.work_json(&["status", "--card-id", "F-001"]);
    assert_eq!(status["data"]["held_lease"]["actor_id"], "second-operator");
}

#[test]
fn reclaiming_your_own_lease_is_refused() {
    let workspace = allocated();
    let output = workspace.work_raw(&[
        "reclaim",
        "--card-id",
        "F-001",
        "--actor-id",
        "operator",
        "--reason",
        "no reason",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-LEASE-HELD");
}

#[test]
fn a_reclaim_is_recorded_with_the_head_it_preserved() {
    let workspace = allocated();
    // The card must have a commit, or "the head it preserved" is vacuously
    // null and the assertion below proves nothing.
    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src/F-001")).unwrap();
    fs::write(worktree.join("src/F-001/a.rs"), "// work\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: work in progress"]);
    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);

    workspace.work(&[
        "reclaim",
        "--card-id",
        "F-001",
        "--actor-id",
        "second-operator",
        "--reason",
        "the original actor's session ended",
    ]);

    let event = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "work.reclaimed")
        .expect("the takeover must be recorded");
    assert_eq!(event["actor_id"], "second-operator");
    assert_eq!(event["metadata"]["from_actor"], "operator");
    assert_eq!(
        event["metadata"]["reason"],
        "the original actor's session ended"
    );
    // The point of the name. Without this the test passes whether or not the
    // head is recorded, which is how it shipped the first time.
    assert_eq!(
        event["metadata"]["preserved_head"], head,
        "the event must name the commit the reclaim preserved"
    );
}

#[test]
fn a_reclaim_dry_run_changes_nothing() {
    let workspace = allocated();
    let before = workspace.control_head();
    let envelope = workspace.work_json(&[
        "reclaim",
        "--card-id",
        "F-001",
        "--actor-id",
        "second-operator",
        "--reason",
        "considering it",
        "--dry-run",
    ]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(workspace.control_head(), before);
    assert_eq!(
        workspace.work_json(&["status", "--card-id", "F-001"])["data"]["held_lease"]["actor_id"],
        "operator"
    );
}

#[test]
fn a_lock_survives_being_diagnosed_under_a_different_locale() {
    // `ps -o lstart=` renders in the caller's locale, and this value is
    // compared as a string across separate invocations that may run under
    // different environments. Without pinning it, the same live process reads
    // as a different one, the lock is declared stale, and recovery clears a
    // lock whose holder is still writing.
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);

    // Take a real lock by planting this live process with the start time the
    // harness itself would record.
    let recorded = std::process::Command::new("ps")
        .env("LC_ALL", "C")
        .args(["-o", "lstart=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps should run");
    let recorded = String::from_utf8_lossy(&recorded.stdout).trim().to_owned();
    plant_lock(&workspace, std::process::id(), Some(&recorded));

    // Diagnose from a process whose locale differs from the default.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .env("LC_ALL", "de_DE.UTF-8")
        .args([
            "project",
            "status",
            "--control",
            &workspace.control.display().to_string(),
            "--output",
            "json",
        ])
        .output()
        .expect("the CLI should start");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["lock"]["state"], "held",
        "a live lock must stay held whatever locale reads it: {}",
        envelope["data"]["lock"]
    );
}
