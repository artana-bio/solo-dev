//! #69 proof: subprocess execution must not hold the global control lock.

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::Duration,
};
use support::Workspace;

fn slow_gate(w: &Workspace, id: &str, marker: &Path, release: &Path) {
    let argv = serde_json::to_string(&[
        "sh",
        "-c",
        "printf started > \"$MARKER\"; while [ ! -e \"$RELEASE\" ]; do sleep 0.01; done",
    ])
    .unwrap();
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let release = serde_json::to_string(&release.display().to_string()).unwrap();
    let body = format!(
        "schema: harness.gate/v1\ngate_id: {id}\nrevision: 1\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER: {marker}\n    RELEASE: {release}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
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
fn run(control: &Path, card: &str, gate: &str, id: &str) -> Output {
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
fn run_child(control: &Path, card: &str, gate: &str, id: &str) -> Child {
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn describe(output: &Output) -> String {
    format!(
        "{}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn await_started(marker: &Path, child: &mut Child) {
    while !marker.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("gate exited before writing {}: {status}", marker.display());
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        child.try_wait().unwrap().is_none(),
        "gate exited after writing {}",
        marker.display()
    );
}

struct HeldGateRun {
    release: PathBuf,
    child: Option<Child>,
}

impl HeldGateRun {
    fn new(release: PathBuf, child: Child) -> Self {
        Self {
            release,
            child: Some(child),
        }
    }

    fn await_started(&mut self, marker: &Path) {
        await_started(marker, self.child.as_mut().unwrap());
    }

    fn release_and_reap(&mut self) -> Output {
        fs::write(&self.release, "release\n").unwrap();
        self.child.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for HeldGateRun {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = fs::write(&self.release, "release\n");
            let _ = child.wait();
        }
    }
}

#[test]
fn exact_permits_run_outside_the_global_lock_then_settle_once() {
    let w = Workspace::initialized();
    let a_marker = w.root.join("a");
    let b_marker = w.root.join("b");
    let a_release = w.root.join("a.release");
    let b_release = w.root.join("b.release");
    slow_gate(&w, "gate.a", &a_marker, &a_release);
    slow_gate(&w, "gate.b", &b_marker, &b_release);
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
    }
    for card in ["F-001", "F-002"] {
        w.work(&["start", "--card-id", card]);
    }
    let ra = reserve(&w, "F-001", "gate.a");
    let rb = reserve(&w, "F-002", "gate.b");
    let mut a_run = HeldGateRun::new(a_release, run_child(&w.control, "F-001", "gate.a", &ra));
    a_run.await_started(&a_marker);
    // The proof: while gate.a's subprocess is deterministically held open,
    // gate.b can acquire its permit and start its own subprocess. That is
    // only possible if execution does not hold the global control lock.
    let mut b_run = HeldGateRun::new(b_release, run_child(&w.control, "F-002", "gate.b", &rb));
    b_run.await_started(&b_marker);
    let settled_a = a_run.release_and_reap();
    assert!(
        settled_a.status.success(),
        "gate.a must settle once released: {}",
        describe(&settled_a)
    );
    let settled_b = b_run.release_and_reap();
    assert!(
        settled_b.status.success(),
        "gate.b must settle once released: {}",
        describe(&settled_b)
    );
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
    slow_gate(&w, "gate.a", &marker, &w.root.join("marker.release"));
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
    let release = w.root.join("marker.release");
    slow_gate(&w, "gate.a", &marker, &release);
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
    let mut running = HeldGateRun::new(
        release,
        run_child(&w.control, "F-001", "gate.a", &reservation),
    );
    running.await_started(&marker);
    // The gate stays deterministically held open while the candidate moves,
    // so settlement always observes a changed candidate.
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

    let output = running.release_and_reap();
    assert!(
        !output.status.success(),
        "settlement must refuse a candidate that changed during execution: {}",
        describe(&output)
    );
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
