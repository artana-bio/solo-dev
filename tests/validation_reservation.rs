//! #59 acceptance: durable exact-key validation reservations.

mod support;

use std::{
    fs,
    process::{Command, Output},
    sync::{Arc, Barrier},
    thread,
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
    assert!(first.status.success());
    assert!(second.status.success());
    let first: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let second: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
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
