//! #69 proof: subprocess execution must not hold the global control lock.

mod support;

use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};
use support::Workspace;

fn slow_gate(w: &Workspace, id: &str, marker: &Path) {
    let argv =
        serde_json::to_string(&["sh", "-c", "printf started > \"$MARKER\"; sleep 0.4"]).unwrap();
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let body = format!(
        "schema: harness.gate/v1\ngate_id: {id}\nrevision: 1\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER: {marker}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
    );
    let definition = w.gate_definition(id, &body);
    w.gate(&["register", "--definition", &definition]);
}
fn reserve(w: &Workspace, card: &str, gate: &str) -> String {
    w.gate_json(&[
        "reserve",
        "--card-id",
        card,
        "--gate-id",
        gate,
        "--actor",
        "holder",
    ])["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn run(control: &Path, card: &str, gate: &str, id: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args([
            "gate",
            "run",
            "--output",
            "json",
            "--control",
            control.to_str().unwrap(),
            "--card-id",
            card,
            "--gate-id",
            gate,
            "--reservation-id",
            id,
            "--actor",
            "holder",
        ])
        .output()
        .unwrap()
}
fn wait(p: &Path) {
    let d = Instant::now() + Duration::from_secs(3);
    while !p.exists() && Instant::now() < d {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(p.exists());
}

#[test]
#[allow(clippy::many_single_char_names)]
fn exact_permits_run_outside_the_global_lock_then_settle_once() {
    let w = Workspace::initialized();
    let a = w.root.join("a");
    let b = w.root.join("b");
    slow_gate(&w, "gate.a", &a);
    slow_gate(&w, "gate.b", &b);
    w.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Concurrent governed run",
    ]);
    w.cycle(&["activate", "--cycle-id", "C-001"]);
    for (card, scope, gate) in [
        ("F-001", "src/a/**", "gate.a"),
        ("F-002", "src/b/**", "gate.b"),
    ] {
        w.activate_card_with_gates(card, &[scope], &[gate]);
        w.work(&["start", "--card-id", card]);
    }
    let ra = reserve(&w, "F-001", "gate.a");
    let rb = reserve(&w, "F-002", "gate.b");
    let c = w.control.clone();
    let ta = thread::spawn({
        let c = c.clone();
        let r = ra.clone();
        move || run(&c, "F-001", "gate.a", &r)
    });
    let tb = thread::spawn({
        let r = rb.clone();
        move || run(&c, "F-002", "gate.b", &r)
    });
    wait(&a);
    wait(&b);
    assert!(ta.join().unwrap().status.success());
    assert!(tb.join().unwrap().status.success());
    assert!(
        !run(&w.control, "F-001", "gate.a", &ra).status.success(),
        "settled permit cannot run twice"
    );
    assert!(
        w.gate_json(&["status", "--card-id", "F-001"])["data"]["receipts"]
            .as_array()
            .unwrap()
            .len()
            == 1
    );
    assert!(
        fs::read_dir(w.control.join("validation-reservation-settlements"))
            .unwrap()
            .count()
            == 2
    );
}

#[test]
fn interruption_after_durable_acquire_leaves_one_explicit_recovery_permit() {
    let w = Workspace::initialized();
    let marker = w.root.join("marker");
    slow_gate(&w, "gate.a", &marker);
    w.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Governed recovery",
    ]);
    w.cycle(&["activate", "--cycle-id", "C-001"]);
    w.activate_card_with_gates("F-001", &["src/a/**"], &["gate.a"]);
    w.work(&["start", "--card-id", "F-001"]);
    let reservation = reserve(&w, "F-001", "gate.a");

    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .env("CHANGE_HARNESS_FAIL_AT", "governed-execution-after-acquire")
        .args([
            "gate",
            "run",
            "--output",
            "json",
            "--control",
            w.control.to_str().unwrap(),
            "--card-id",
            "F-001",
            "--gate-id",
            "gate.a",
            "--reservation-id",
            &reservation,
            "--actor",
            "holder",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        w.control
            .join(format!("validation-execution-permits/{reservation}.json"))
            .exists(),
        "a post-acquire interruption must leave a durable, non-reusable permit"
    );
    assert!(
        !w.control
            .join(format!(
                "validation-reservation-settlements/{reservation}.json"
            ))
            .exists()
    );
    assert!(
        !run(&w.control, "F-001", "gate.a", &reservation)
            .status
            .success(),
        "a second runner must refuse while recovery owns the permit"
    );
}

#[test]
fn candidate_change_during_execution_refuses_settlement_and_preserves_permit() {
    let w = Workspace::initialized();
    let marker = w.root.join("marker");
    slow_gate(&w, "gate.a", &marker);
    w.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Settlement must revalidate",
    ]);
    w.cycle(&["activate", "--cycle-id", "C-001"]);
    w.activate_card_with_gates("F-001", &["src/a/**"], &["gate.a"]);
    w.work(&["start", "--card-id", "F-001"]);
    let reservation = reserve(&w, "F-001", "gate.a");
    let control = w.control.clone();
    let running = thread::spawn({
        let reservation = reservation.clone();
        move || run(&control, "F-001", "gate.a", &reservation)
    });
    wait(&marker);
    let worktree =
        w.work_json(&["status", "--card-id", "F-001"])["data"]["held_lease"]["worktree_path"]
            .as_str()
            .unwrap()
            .to_owned();
    fs::write(
        Path::new(&worktree).join("candidate-change.txt"),
        "changed\n",
    )
    .unwrap();
    support::git(Path::new(&worktree), &["add", "candidate-change.txt"]);
    support::git(
        Path::new(&worktree),
        &["commit", "-qm", "change during governed run"],
    );

    let output = running.join().unwrap();
    assert!(!output.status.success());
    assert!(
        w.control
            .join(format!("validation-execution-permits/{reservation}.json"))
            .exists(),
        "stale settlement must retain the permit for explicit recovery"
    );
    assert!(
        !w.control
            .join(format!(
                "validation-reservation-settlements/{reservation}.json"
            ))
            .exists(),
        "stale settlement must not write a terminal fact"
    );
    assert_eq!(
        w.gate_json(&["status", "--card-id", "F-001"])["data"]["receipts"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "stale settlement must not attach a receipt to changed state"
    );
}
