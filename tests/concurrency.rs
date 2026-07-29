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
/// A process id that is in range and reliably absent.
///
/// Not an enormous one. `ps` answers "process id too large" on stderr for those
/// — a refusal to answer, not a report of death — and the fixtures here used
/// `4_294_967_294` precisely because the old code treated that refusal as proof
/// the holder was gone. That was defect 13, so these fixtures were asserting
/// the bug.
const ABSENT_PID: u32 = 99_999;

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
    plant_lock(&workspace, ABSENT_PID, Some("whenever"));

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
    plant_lock(&workspace, ABSENT_PID, Some("whenever"));

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
    plant_lock(&workspace, ABSENT_PID, Some("whenever"));

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

/// How many rounds the contention tests run.
///
/// A race that shows up one time in twenty would pass a single round more than
/// half the time, which is indistinguishable from correct. Enough rounds to
/// make that unlikely, few enough to stay inside an ordinary test run.
const ROUNDS: usize = 40;

/// How many contenders each round starts.
const CONTENDERS: usize = 8;

#[test]
fn many_threads_contending_for_one_lock_produce_exactly_one_winner() {
    use std::sync::{Arc, Barrier};

    use change_harness::{control::lock::ProjectLock, domain::clock::SystemClock};

    for round in 0..ROUNDS {
        let temp = tempfile::tempdir().expect("temp dir");
        let control = temp.path().to_path_buf();
        // A barrier is the point: without it the threads start staggered and
        // the first one finishes before the last one begins, which is the
        // sequential case wearing a thread pool.
        let gate = Arc::new(Barrier::new(CONTENDERS));

        // The acquired locks are *kept* until every contender has finished.
        // Returning `is_ok()` would drop each one at the end of its closure,
        // releasing it in time for the next thread to acquire cleanly — which
        // measures nothing and passes trivially. The first version of this
        // test did exactly that.
        let held: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..CONTENDERS)
                .map(|index| {
                    let control = control.clone();
                    let gate = Arc::clone(&gate);
                    scope.spawn(move || {
                        gate.wait();
                        ProjectLock::acquire(&control, &format!("worker-{index}"), &SystemClock)
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect()
        });
        let winners = held.iter().filter(|outcome| outcome.is_ok()).count();

        assert_eq!(
            winners, 1,
            "round {round}: exactly one of {CONTENDERS} contenders may hold the lock"
        );

        // Every loser must be told the lock is *held*, not that its state is
        // unknowable. An ambiguous verdict during ordinary contention would
        // send an operator looking for a process that is running normally.
        for outcome in &held {
            if let Err(error) = outcome {
                assert_eq!(
                    error.code(),
                    change_harness::error::ErrorCode::PolicyLockHeld,
                    "round {round}: a loser saw `{error}`"
                );
            }
        }
    }
}

#[test]
fn many_processes_mutating_one_project_produce_one_commit_each_round() {
    // Threads share an address space; separate processes do not, and the lock
    // has to work across both. This is the case an operator actually hits when
    // two terminals run a command at once.
    let workspace = Workspace::initialized();
    let mut created = 0usize;

    for round in 0..ROUNDS {
        let before = workspace.control_head();
        let cycle = format!("C-{round:03}");

        let children: Vec<_> = (0..CONTENDERS)
            .map(|_| {
                std::process::Command::new(env!("CARGO_BIN_EXE_change-harness"))
                    .args([
                        "cycle",
                        "create",
                        "--control",
                        &workspace.control.display().to_string(),
                        "--cycle-id",
                        &cycle,
                        "--objective",
                        "contention",
                        "--output",
                        "json",
                    ])
                    .spawn()
                    .expect("the CLI should start")
            })
            .collect();

        let successes = children
            .into_iter()
            .filter_map(|mut child| child.wait().ok())
            .filter(std::process::ExitStatus::success)
            .count();

        // At most one may succeed. Zero is legitimate: a contender that finds
        // the lock held fails, and if the winner is slow every other process
        // can lose before it commits.
        assert!(
            successes <= 1,
            "round {round}: {successes} processes each believed they created {cycle}"
        );
        if successes == 1 {
            created += 1;
            assert_ne!(
                workspace.control_head(),
                before,
                "round {round}: a success must have committed something"
            );
        } else {
            assert_eq!(
                workspace.control_head(),
                before,
                "round {round}: no winner must mean no commit"
            );
        }
    }

    assert!(
        created > 0,
        "the fixture must actually have let someone through, or this proves nothing"
    );
}

#[test]
fn a_losing_contender_never_leaves_the_lock_behind() {
    // The failure that would matter most: a process that loses the race must
    // not release a lock it never held, or the winner is left unprotected.
    let workspace = Workspace::initialized();

    for round in 0..ROUNDS {
        let cycle = format!("C-{round:03}");
        let children: Vec<_> = (0..CONTENDERS)
            .map(|_| {
                std::process::Command::new(env!("CARGO_BIN_EXE_change-harness"))
                    .args([
                        "cycle",
                        "create",
                        "--control",
                        &workspace.control.display().to_string(),
                        "--cycle-id",
                        &cycle,
                        "--objective",
                        "contention",
                    ])
                    .spawn()
                    .expect("the CLI should start")
            })
            .collect();
        for mut child in children {
            let _ = child.wait();
        }

        assert_eq!(
            lock_state(&workspace)["state"],
            "free",
            "round {round}: every contender exited, so the lock must be released"
        );
    }
}
