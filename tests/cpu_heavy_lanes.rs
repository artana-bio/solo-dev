//! #67 frozen proof: two durable CPU-heavy lanes bound to governed execution.

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::Duration,
};

use serde_json::Value;
use support::Workspace;

fn gate(workspace: &Workspace, gate_id: &str, marker: &Path, release: &Path) {
    let command =
        "printf started > \"$MARKER_PATH\"; while [ ! -e \"$RELEASE_PATH\" ]; do sleep 0.01; done";
    let argv = serde_json::to_string(&["sh", "-c", command]).unwrap();
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let release = serde_json::to_string(&release.display().to_string()).unwrap();
    let body = format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER_PATH: {marker}\n    RELEASE_PATH: {release}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\nmigration: legacy_v1\n",
    );
    let definition = workspace.gate_definition(gate_id, &body);
    workspace.gate(&["register", "--definition", &definition]);
}

fn profile(workspace: &Workspace) -> std::path::PathBuf {
    let path = workspace.root.join("cpu.json");
    fs::write(&path, r#"{"schema":"harness.cpu-heavy-validation-profile/v1","risk":"high","expected_duration_seconds":60,"resource_cost":{"cpu_cores":1,"memory_mib":1024}}"#).unwrap();
    path
}

fn run_process(control: &Path, card_id: &str, gate_id: &str, reservation_id: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args([
            "gate",
            "run",
            "--output",
            "json",
            "--control",
            control.to_str().unwrap(),
            "--card-id",
            card_id,
            "--gate-id",
            gate_id,
            "--reservation-id",
            reservation_id,
            "--actor",
            "holder",
        ])
        .output()
        .unwrap()
}

fn run_child(control: &Path, card_id: &str, gate_id: &str, reservation_id: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args([
            "gate",
            "run",
            "--output",
            "json",
            "--control",
            control.to_str().unwrap(),
            "--card-id",
            card_id,
            "--gate-id",
            gate_id,
            "--reservation-id",
            reservation_id,
            "--actor",
            "holder",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Only the bounded, administrative project-lock refusal is retryable here.
/// A policy or execution failure must stay visible to the test immediately.
fn is_project_lock_refusal(stdout: &[u8]) -> bool {
    serde_json::from_slice::<Value>(stdout)
        .ok()
        .and_then(|envelope| envelope["error"]["code"].as_str().map(str::to_owned))
        .as_deref()
        == Some("CH-POLICY-LOCK-HELD")
}

fn await_started(path: &Path, child: &mut Child) {
    while !path.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("gate exited before writing {}: {status}", path.display());
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        child.try_wait().unwrap().is_none(),
        "gate exited after writing {}",
        path.display()
    );
}

fn assert_cpu_lanes(control: &Path, expected: &[(u8, &str)]) {
    let mut paths = fs::read_dir(control.join("cpu-heavy-lanes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    assert_eq!(paths.len(), expected.len());
    for (path, (lane_id, reservation_id)) in paths.iter().zip(expected) {
        let lane: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(lane["lane_id"], *lane_id);
        assert_eq!(lane["reservation_id"], *reservation_id);
    }
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
#[allow(clippy::too_many_lines)]
fn two_cpu_heavy_lanes_are_durable_and_release_before_a_third_execution_starts() {
    let workspace = Workspace::initialized();
    let first_marker = workspace.root.join("first.marker");
    let second_marker = workspace.root.join("second.marker");
    let third_marker = workspace.root.join("third.marker");
    let ordinary_marker = workspace.root.join("ordinary.marker");
    let first_release = workspace.root.join("first.release");
    let second_release = workspace.root.join("second.release");
    let third_release = workspace.root.join("third.release");
    let ordinary_release = workspace.root.join("ordinary.release");
    gate(&workspace, "gate.cpu.one", &first_marker, &first_release);
    gate(&workspace, "gate.cpu.two", &second_marker, &second_release);
    gate(&workspace, "gate.cpu.three", &third_marker, &third_release);
    gate(
        &workspace,
        "gate.ordinary",
        &ordinary_marker,
        &ordinary_release,
    );
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Bound CPU lanes",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    for (card, scope, gate_id) in [
        ("F-001", "src/one/**", "gate.cpu.one"),
        ("F-002", "src/two/**", "gate.cpu.two"),
        ("F-003", "src/three/**", "gate.cpu.three"),
        ("F-004", "src/four/**", "gate.ordinary"),
    ] {
        workspace.activate_card_with_gates(card, &[scope], &[gate_id]);
    }
    for card in ["F-001", "F-002", "F-003", "F-004"] {
        workspace.work(&["start", "--card-id", card]);
    }
    let profile = profile(&workspace);
    let reserve = |card: &str, gate_id: &str| {
        workspace.gate_json(&[
            "reserve",
            "--card-id",
            card,
            "--gate-id",
            gate_id,
            "--execution-mode",
            "cpu-heavy",
            "--cpu-profile",
            profile.to_str().unwrap(),
            "--actor",
            "holder",
        ])["data"]["reservation"]["reservation_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = reserve("F-001", "gate.cpu.one");
    let second = reserve("F-002", "gate.cpu.two");
    let third = reserve("F-003", "gate.cpu.three");
    let mut first_run = HeldGateRun::new(
        first_release,
        run_child(&workspace.control, "F-001", "gate.cpu.one", &first),
    );
    first_run.await_started(&first_marker);
    let mut second_run = HeldGateRun::new(
        second_release,
        run_child(&workspace.control, "F-002", "gate.cpu.two", &second),
    );
    second_run.await_started(&second_marker);
    assert_cpu_lanes(&workspace.control, &[(1, &first), (2, &second)]);
    // If a mutated scheduler admits the third run, it exits immediately and
    // makes the capacity failure visible instead of leaving this proof hung.
    fs::write(&third_release, "release\n").unwrap();
    let blocked = run_process(&workspace.control, "F-003", "gate.cpu.three", &third);
    assert!(
        !third_marker.exists(),
        "third subprocess must not start without a lane"
    );
    assert!(!blocked.status.success(), "third CPU run must wait/refuse");
    let blocked_json: Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(
        blocked_json["error"]["code"],
        "CH-POLICY-INVALID-TRANSITION"
    );
    assert!(
        blocked_json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("two CPU-heavy validation lanes are occupied"),
        "third refusal must name the two occupied lanes"
    );
    fs::remove_file(&third_release).unwrap();
    // An ordinary named gate still needs the universal execution capability,
    // but it must not consume one of the CPU-heavy lanes.
    let ordinary_reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-004",
        "--gate-id",
        "gate.ordinary",
        "--actor",
        "holder",
    ])["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut ordinary_run = HeldGateRun::new(
        ordinary_release,
        run_child(
            &workspace.control,
            "F-004",
            "gate.ordinary",
            &ordinary_reservation,
        ),
    );
    ordinary_run.await_started(&ordinary_marker);
    assert_cpu_lanes(&workspace.control, &[(1, &first), (2, &second)]);
    assert!(
        ordinary_run.release_and_reap().status.success(),
        "ordinary named gate is not lane constrained"
    );
    assert!(first_run.release_and_reap().status.success());
    assert_cpu_lanes(&workspace.control, &[(2, &second)]);
    let mut third_run = HeldGateRun::new(
        third_release,
        run_child(&workspace.control, "F-003", "gate.cpu.three", &third),
    );
    third_run.await_started(&third_marker);
    assert!(
        second_run.release_and_reap().status.success(),
        "second lane remains independently held until explicitly released"
    );
    assert!(third_run.release_and_reap().status.success());
    assert!(
        fs::read_dir(workspace.control.join("cpu-heavy-lanes"))
            .unwrap()
            .next()
            .is_none(),
        "terminal CPU outcomes release durable lanes"
    );
}

#[test]
fn only_the_project_lock_refusal_is_retryable() {
    assert!(is_project_lock_refusal(
        br#"{"error":{"code":"CH-POLICY-LOCK-HELD"}}"#
    ));
    assert!(!is_project_lock_refusal(
        br#"{"error":{"code":"CH-POLICY-INVALID-TRANSITION"}}"#
    ));
    assert!(!is_project_lock_refusal(b"not a command envelope"));
}
