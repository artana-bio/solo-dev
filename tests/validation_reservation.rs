//! #59 acceptance: durable exact-key validation reservations.

mod support;

use std::{
    fs,
    path::Path,
    process::{Command, Output},
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use support::{Workspace, git};

fn allocated() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Reserve one expensive proof",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace
}

fn reserve(workspace: &Workspace, actor: &str) -> serde_json::Value {
    workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--actor",
        actor,
    ])
}

fn reserve_process(control: &std::path::Path, actor: &str, fail_at: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_change-harness"));
    command.args([
        "gate",
        "reserve",
        "--output",
        "json",
        "--control",
        control.to_str().unwrap(),
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--actor",
        actor,
    ]);
    if let Some(step) = fail_at {
        command.env("CHANGE_HARNESS_FAIL_AT", step);
    }
    command.output().unwrap()
}

/// Only the bounded, administrative project-lock refusal is a tolerable
/// outcome for the losing side of the reservation race. A policy or
/// validation failure must stay visible to the test immediately.
fn is_project_lock_refusal(stdout: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(stdout)
        .ok()
        .and_then(|envelope| envelope["error"]["code"].as_str().map(str::to_owned))
        .as_deref()
        == Some("CH-POLICY-LOCK-HELD")
}

fn describe(output: &Output) -> String {
    format!(
        "{}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Re-runs one contender's reserve after its administrative lock refusal,
/// retrying only that refusal within a bound. Any other outcome — success,
/// a different error, or exhaustion — returns immediately.
fn reserve_after_lock_refusal(control: &Path, actor: &str) -> Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = reserve_process(control, actor, None);
        if output.status.success()
            || !is_project_lock_refusal(&output.stdout)
            || Instant::now() >= deadline
        {
            return output;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn identical_concurrent_requests_produce_one_durable_winner_and_one_waiter() {
    let workspace = allocated();
    let barrier = Arc::new(Barrier::new(3));
    let run = |actor: &'static str| {
        let barrier = Arc::clone(&barrier);
        let control = workspace.control.clone();
        thread::spawn(move || {
            barrier.wait();
            reserve_process(&control, actor, None)
        })
    };
    let first = run("first");
    let second = run("second");
    barrier.wait();
    let first = first.join().unwrap();
    let second = second.join().unwrap();
    // The project lock is fail-fast: under load a contender can exhaust the
    // binary's bounded internal retry and exit with the administrative
    // CH-POLICY-LOCK-HELD refusal instead of a decision. That contender is
    // the waiter. Every other failure is a real one.
    for (actor, output) in [("first", &first), ("second", &second)] {
        assert!(
            output.status.success() || is_project_lock_refusal(&output.stdout),
            "reserve ({actor}) failed beyond the administrative lock refusal: {}",
            describe(output)
        );
    }
    assert!(
        first.status.success() || second.status.success(),
        "the lock admits one contender, so at most one refusal\nfirst: {}\nsecond: {}",
        describe(&first),
        describe(&second)
    );
    let observe = |actor: &'static str, output: Output| -> serde_json::Value {
        let output = if output.status.success() {
            output
        } else {
            // Classified above as the lock refusal: the race is settled, so
            // the refused contender must now observe the durable winner.
            let observed = reserve_after_lock_refusal(&workspace.control, actor);
            assert!(
                observed.status.success(),
                "the refused contender ({actor}) must observe the winner: {}",
                describe(&observed)
            );
            observed
        };
        serde_json::from_slice(&output.stdout).expect("the JSON envelope")
    };
    let first = observe("first", first);
    let second = observe("second", second);
    let (winner, waiter) = if first["data"]["disposition"]["kind"] == "reserved" {
        (first, second)
    } else {
        (second, first)
    };

    assert_eq!(winner["data"]["disposition"]["kind"], "reserved");
    assert_eq!(
        waiter["data"]["disposition"]["kind"],
        "wait_for_reserved_run"
    );
    assert_eq!(
        winner["data"]["reservation"]["reservation_id"],
        waiter["data"]["reservation"]["reservation_id"]
    );
    assert_eq!(
        winner["data"]["reservation"]["key_digest"],
        waiter["data"]["reservation"]["key_digest"]
    );
    assert_eq!(
        waiter["data"]["reservation"]["holder_actor_id"],
        winner["data"]["reservation"]["holder_actor_id"],
        "the waiter must identify the actor that won the durable reservation"
    );
    assert_eq!(
        winner["data"]["schema"],
        "harness.validation-reservation-decision/v1"
    );
    assert_eq!(winner["data"]["reservation"]["key"]["card_id"], "F-001");
    assert_eq!(
        winner["data"]["reservation"]["key"]["execution_mode"],
        "named-gate"
    );
    assert!(winner["data"]["reservation"]["expires_at"].is_string());
    assert_eq!(
        winner["data"]["reservation"]["recovery_policy"],
        "explicit_recovery_required"
    );
    let reservations = fs::read_dir(workspace.control.join("validation-reservations"))
        .unwrap()
        .filter_map(Result::ok)
        .count();
    assert_eq!(reservations, 1, "only the winning record is durable");
}

#[test]
fn interruption_before_record_write_creates_no_phantom_reservation() {
    let workspace = allocated();
    let interrupted = reserve_process(
        &workspace.control,
        "first",
        Some("reservation-record-write"),
    );
    assert!(!interrupted.status.success());
    assert!(
        !workspace.control.join("validation-reservations").exists(),
        "the record write boundary must not leave a phantom reservation"
    );
    let status = Workspace::run(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(
        status["data"]["unresolved_operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation["command"] == "gate.reserve"),
        "the interruption remains explicitly recoverable rather than silently permitting a retry"
    );
}

#[test]
fn a_changed_gate_definition_gets_a_distinct_reservation_key() {
    let workspace = allocated();
    let first = reserve(&workspace, "first");
    workspace.register_gate_revision("gate.unit", 2, &["true"]);
    let second = reserve(&workspace, "second");

    assert_eq!(second["data"]["disposition"]["kind"], "reserved");
    assert_ne!(
        first["data"]["reservation"]["key_digest"], second["data"]["reservation"]["key_digest"],
        "a changed required-check definition must never deduplicate"
    );
}

#[test]
fn a_moved_candidate_gets_a_distinct_reservation_key() {
    let workspace = allocated();
    let first = reserve(&workspace, "first");
    let worktree = workspace.work_json(&["status", "--card-id", "F-001"])["data"]["held_lease"]
        ["worktree_path"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::write(
        std::path::Path::new(&worktree).join("candidate.txt"),
        "changed\n",
    )
    .unwrap();
    git(std::path::Path::new(&worktree), &["add", "candidate.txt"]);
    git(
        std::path::Path::new(&worktree),
        &["commit", "-qm", "move candidate"],
    );

    let second = reserve(&workspace, "second");
    assert_eq!(second["data"]["disposition"]["kind"], "reserved");
    assert_ne!(
        first["data"]["reservation"]["key_digest"], second["data"]["reservation"]["key_digest"],
        "a different candidate must not consume the earlier reservation"
    );
}

#[test]
fn a_corrupt_reservation_key_is_refused_without_creating_another_record() {
    let workspace = allocated();
    let winner = reserve(&workspace, "first");
    let reservation_id = winner["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    let path = workspace
        .control
        .join(format!("validation-reservations/{reservation_id}.json"));
    let mut record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    record["key_digest"] = serde_json::json!(
        "sha256:0000000000000000000000000000000000000000000000000000000000000000"
    );
    fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

    let output = workspace.gate_raw(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--actor",
        "second",
    ]);
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "CH-INTERNAL-CONTROL-CORRUPT");
    assert_eq!(
        fs::read_dir(workspace.control.join("validation-reservations"))
            .unwrap()
            .filter_map(Result::ok)
            .count(),
        1
    );
}

#[test]
fn a_changed_project_policy_is_refused_before_it_can_reuse_a_reservation() {
    let workspace = allocated();
    reserve(&workspace, "first");
    let project_path = workspace.control.join("project/project.json");
    let mut project: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["validation_policy"]["proof_map_required_for"] = serde_json::json!(["low"]);
    fs::write(&project_path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();

    let output = workspace.gate_raw(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--actor",
        "second",
    ]);
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "CH-POLICY-INVALID-CYCLE");
    assert_eq!(
        fs::read_dir(workspace.control.join("validation-reservations"))
            .unwrap()
            .filter_map(Result::ok)
            .count(),
        1,
        "a policy mismatch must refuse before it can create or consume another reservation"
    );
}

#[test]
fn only_the_project_lock_refusal_is_a_tolerable_concurrent_outcome() {
    assert!(is_project_lock_refusal(
        br#"{"error":{"code":"CH-POLICY-LOCK-HELD"}}"#
    ));
    assert!(!is_project_lock_refusal(
        br#"{"error":{"code":"CH-POLICY-INVALID-TRANSITION"}}"#
    ));
    assert!(!is_project_lock_refusal(b"not a command envelope"));
}
