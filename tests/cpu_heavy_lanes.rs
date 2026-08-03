//! #67 frozen proof: two durable CPU-heavy lanes bound to governed execution.

mod support;

use std::{
    fs,
    path::Path,
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use support::Workspace;

fn gate(workspace: &Workspace, gate_id: &str, marker: &Path, sleep_seconds: f64) {
    let command = format!(
        "printf started > \"$MARKER_PATH\"; sleep {sleep_seconds}; printf finished >> \"$MARKER_PATH\""
    );
    let argv = serde_json::to_string(&["sh", "-c", &command]).unwrap();
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let body = format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER_PATH: {marker}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n",
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

/// Only the bounded, administrative project-lock refusal is retryable here.
/// A policy or execution failure must stay visible to the test immediately.
fn is_project_lock_refusal(stdout: &[u8]) -> bool {
    serde_json::from_slice::<Value>(stdout)
        .ok()
        .and_then(|envelope| envelope["error"]["code"].as_str().map(str::to_owned))
        .as_deref()
        == Some("CH-POLICY-LOCK-HELD")
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        path.exists(),
        "gate never reached subprocess marker {}",
        path.display()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn two_cpu_heavy_lanes_are_durable_and_release_before_a_third_execution_starts() {
    let workspace = Workspace::initialized();
    let first_marker = workspace.root.join("first.marker");
    let second_marker = workspace.root.join("second.marker");
    let third_marker = workspace.root.join("third.marker");
    let ordinary_marker = workspace.root.join("ordinary.marker");
    gate(&workspace, "gate.cpu.one", &first_marker, 0.4);
    // Stagger terminal settlement so this test isolates lane capacity rather
    // than introducing an unrelated race for the project transaction lock.
    gate(&workspace, "gate.cpu.two", &second_marker, 0.7);
    gate(&workspace, "gate.cpu.three", &third_marker, 0.0);
    gate(&workspace, "gate.ordinary", &ordinary_marker, 0.0);
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
    let control = workspace.control.clone();
    let first_thread = thread::spawn({
        let control = control.clone();
        let id = first.clone();
        move || run_process(&control, "F-001", "gate.cpu.one", &id)
    });
    let second_thread = thread::spawn({
        let id = second.clone();
        move || run_process(&control, "F-002", "gate.cpu.two", &id)
    });
    wait_for(&first_marker);
    wait_for(&second_marker);
    assert_eq!(
        fs::read_dir(workspace.control.join("cpu-heavy-lanes"))
            .unwrap()
            .count(),
        2
    );
    let blocked = run_process(&workspace.control, "F-003", "gate.cpu.three", &third);
    assert!(!blocked.status.success(), "third CPU run must wait/refuse");
    assert!(
        !third_marker.exists(),
        "third subprocess must not start without a lane"
    );
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
    // CPU runs hold the project lock only while acquiring/settling. Retry that
    // short administrative contention, but do not wait for a CPU lane: the
    // ordinary subprocess must start while both lanes remain occupied.
    let ordinary_deadline = Instant::now() + Duration::from_secs(2);
    let ordinary = loop {
        let output = run_process(
            &workspace.control,
            "F-004",
            "gate.ordinary",
            &ordinary_reservation,
        );
        if output.status.success()
            || !is_project_lock_refusal(&output.stdout)
            || Instant::now() >= ordinary_deadline
        {
            break output;
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(
        ordinary.status.success(),
        "ordinary named gate is not lane constrained: stdout={} stderr={}",
        String::from_utf8_lossy(&ordinary.stdout),
        String::from_utf8_lossy(&ordinary.stderr),
    );
    assert!(ordinary_marker.exists());
    assert!(first_thread.join().unwrap().status.success());
    assert!(second_thread.join().unwrap().status.success());
    assert!(
        fs::read_dir(workspace.control.join("cpu-heavy-lanes"))
            .unwrap()
            .next()
            .is_none(),
        "terminal CPU outcomes release durable lanes"
    );
    assert!(
        run_process(&workspace.control, "F-003", "gate.cpu.three", &third)
            .status
            .success()
    );
    assert!(third_marker.exists());
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
