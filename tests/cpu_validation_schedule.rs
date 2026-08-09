//! #68 frozen proof: authoritative CPU-heavy next-action projection.

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::Duration,
};

use support::Workspace;

fn profile(workspace: &Workspace) -> String {
    let path = workspace.root.join("cpu.json");
    fs::write(&path, r#"{"schema":"harness.cpu-heavy-validation-profile/v1","risk":"high","expected_duration_seconds":60,"resource_cost":{"cpu_cores":1,"memory_mib":1024}}"#).unwrap();
    path.display().to_string()
}

fn slow_gate(workspace: &Workspace, gate_id: &str, marker: &Path, release: &Path) {
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let release = serde_json::to_string(&release.display().to_string()).unwrap();
    let definition = workspace.gate_definition(gate_id, &format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: [\"sh\", \"-c\", \"printf started > \\\"$MARKER\\\"; while [ ! -e \\\"$RELEASE\\\" ]; do sleep 0.01; done\"]\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER: {marker}\n    RELEASE: {release}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\nmigration: legacy_v1\n"
    ));
    workspace.gate(&["register", "--definition", &definition]);
}

fn schedule_raw(
    control: &Path,
    reservation_id: &str,
    revision: Option<&str>,
) -> std::process::Output {
    let mut args = vec![
        "gate",
        "schedule",
        "--output",
        "json",
        "--control",
        control.to_str().unwrap(),
        "--reservation-id",
        reservation_id,
    ];
    if let Some(revision) = revision {
        args.extend(["--state-revision", revision]);
    }
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(args)
        .output()
        .unwrap()
}

fn run_child(control: &Path, card: &str, gate: &str, reservation: &str) -> Child {
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
            reservation,
            "--actor",
            "holder",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
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

fn assert_cpu_lanes(control: &Path, expected: &[(u8, &str)]) {
    let mut paths = fs::read_dir(control.join("cpu-heavy-lanes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    assert_eq!(paths.len(), expected.len());
    for (path, (lane_id, reservation_id)) in paths.iter().zip(expected) {
        let lane: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
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
fn schedule_is_fresh_read_only_and_reports_wait_with_independent_work_at_two_cpu_lanes() {
    let workspace = Workspace::initialized();
    let markers = [
        workspace.root.join("one"),
        workspace.root.join("two"),
        workspace.root.join("three"),
    ];
    let releases = [
        workspace.root.join("one.release"),
        workspace.root.join("two.release"),
        workspace.root.join("three.release"),
    ];
    let independent_marker = workspace.root.join("independent");
    let independent_release = workspace.root.join("independent.release");
    for (gate, marker, release) in [
        ("gate.one", &markers[0], &releases[0]),
        ("gate.two", &markers[1], &releases[1]),
        ("gate.three", &markers[2], &releases[2]),
    ] {
        slow_gate(&workspace, gate, marker, release);
    }
    slow_gate(
        &workspace,
        "gate.independent",
        &independent_marker,
        &independent_release,
    );
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "scheduled validation",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    for (card, scope, gate) in [
        ("F-001", "src/one/**", "gate.one"),
        ("F-002", "src/two/**", "gate.two"),
        ("F-003", "src/three/**", "gate.three"),
        ("F-004", "src/four/**", "gate.independent"),
    ] {
        workspace.activate_card_with_gates(card, &[scope], &[gate]);
    }
    for card in ["F-001", "F-002", "F-003", "F-004"] {
        workspace.work(&["start", "--card-id", card]);
    }
    let profile = profile(&workspace);
    let reserve = |card: &str, gate: &str| {
        workspace.gate_json(&[
            "reserve",
            "--card-id",
            card,
            "--gate-id",
            gate,
            "--execution-mode",
            "cpu-heavy",
            "--cpu-profile",
            &profile,
            "--actor",
            "holder",
        ])["data"]["reservation"]["reservation_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = reserve("F-001", "gate.one");
    let second = reserve("F-002", "gate.two");
    let third = reserve("F-003", "gate.three");
    let mut one = HeldGateRun::new(
        releases[0].clone(),
        run_child(&workspace.control, "F-001", "gate.one", &first),
    );
    one.await_started(&markers[0]);
    let mut two = HeldGateRun::new(
        releases[1].clone(),
        run_child(&workspace.control, "F-002", "gate.two", &second),
    );
    two.await_started(&markers[1]);
    assert_cpu_lanes(&workspace.control, &[(1, &first), (2, &second)]);
    let initial: serde_json::Value =
        serde_json::from_slice(&schedule_raw(&workspace.control, &third, None).stdout).unwrap();
    assert_eq!(
        initial["data"]["schema"],
        "harness.cpu-validation-schedule/v1"
    );
    assert_eq!(
        initial["data"]["recommendation"]["kind"],
        "start_independent_work"
    );
    assert_eq!(initial["data"]["blocker"]["kind"], "cpu_lanes_occupied");
    assert!(initial["data"]["release_condition"].is_object());
    assert!(initial["data"]["state_revision"].is_string());
    assert_eq!(initial["data"]["reservation"]["reservation_id"], third);
    assert!(initial["data"]["reservation"]["key"].is_object());
    assert_eq!(
        initial["data"]["receipt_reuse"]["disposition"]["kind"], "rerun_required",
        "without exact compatible provenance, scheduling must not infer reuse"
    );
    assert!(initial["data"]["receipt_reuse"]["request"].is_object());
    let initial_revision = initial["data"]["state_revision"]
        .as_str()
        .unwrap()
        .to_owned();
    workspace.work(&[
        "block",
        "--card-id",
        "F-004",
        "--reason",
        "independent work is no longer eligible",
    ]);
    let blocked: serde_json::Value =
        serde_json::from_slice(&schedule_raw(&workspace.control, &third, None).stdout).unwrap();
    assert_eq!(blocked["data"]["recommendation"]["kind"], "wait");
    assert_ne!(blocked["data"]["state_revision"], initial_revision);
    let stale_after_eligibility_change =
        schedule_raw(&workspace.control, &third, Some(&initial_revision));
    assert!(
        !stale_after_eligibility_change.status.success(),
        "an eligibility change must invalidate the prior next-action revision"
    );
    let stale = schedule_raw(&workspace.control, &third, Some("stale"));
    assert!(
        !stale.status.success(),
        "opaque stale state revision must refuse"
    );
    assert!(one.release_and_reap().status.success());
    assert!(two.release_and_reap().status.success());
    assert_cpu_lanes(&workspace.control, &[]);
    let after_release: serde_json::Value =
        serde_json::from_slice(&schedule_raw(&workspace.control, &third, None).stdout).unwrap();
    assert_eq!(after_release["data"]["recommendation"]["kind"], "start");
    assert_ne!(
        initial["data"]["state_revision"], after_release["data"]["state_revision"],
        "a lane release creates fresh authoritative schedule state"
    );
}

#[test]
fn occupied_lanes_without_independent_work_recommend_wait_without_mutation() {
    let workspace = Workspace::initialized();
    let first_marker = workspace.root.join("wait-first");
    let second_marker = workspace.root.join("wait-second");
    let first_release = workspace.root.join("wait-first.release");
    let second_release = workspace.root.join("wait-second.release");
    let third_release = workspace.root.join("wait-third.release");
    slow_gate(&workspace, "gate.wait.one", &first_marker, &first_release);
    slow_gate(&workspace, "gate.wait.two", &second_marker, &second_release);
    slow_gate(
        &workspace,
        "gate.wait.three",
        &workspace.root.join("wait-third"),
        &third_release,
    );
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "wait only"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    for (card, scope, gate) in [
        ("F-001", "src/one/**", "gate.wait.one"),
        ("F-002", "src/two/**", "gate.wait.two"),
        ("F-003", "src/three/**", "gate.wait.three"),
    ] {
        workspace.activate_card_with_gates(card, &[scope], &[gate]);
    }
    for card in ["F-001", "F-002", "F-003"] {
        workspace.work(&["start", "--card-id", card]);
    }
    let profile = profile(&workspace);
    let reserve = |card: &str, gate: &str| {
        workspace.gate_json(&[
            "reserve",
            "--card-id",
            card,
            "--gate-id",
            gate,
            "--execution-mode",
            "cpu-heavy",
            "--cpu-profile",
            &profile,
            "--actor",
            "holder",
        ])["data"]["reservation"]["reservation_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let first = reserve("F-001", "gate.wait.one");
    let second = reserve("F-002", "gate.wait.two");
    let third = reserve("F-003", "gate.wait.three");
    let mut first_run = HeldGateRun::new(
        first_release,
        run_child(&workspace.control, "F-001", "gate.wait.one", &first),
    );
    first_run.await_started(&first_marker);
    let mut second_run = HeldGateRun::new(
        second_release,
        run_child(&workspace.control, "F-002", "gate.wait.two", &second),
    );
    second_run.await_started(&second_marker);
    assert_cpu_lanes(&workspace.control, &[(1, &first), (2, &second)]);
    let before = workspace.control_head();
    let schedule: serde_json::Value =
        serde_json::from_slice(&schedule_raw(&workspace.control, &third, None).stdout).unwrap();
    assert_eq!(schedule["data"]["recommendation"]["kind"], "wait");
    assert_eq!(schedule["data"]["blocker"]["kind"], "cpu_lanes_occupied");
    assert!(schedule["data"]["release_condition"].is_object());
    assert_eq!(workspace.control_head(), before, "schedule is read-only");
    assert!(first_run.release_and_reap().status.success());
    assert!(second_run.release_and_reap().status.success());
    assert_cpu_lanes(&workspace.control, &[]);
}

#[test]
fn budget_crossing_emits_one_event_without_changing_capacity_or_policy() {
    let workspace = Workspace::initialized();
    let marker = workspace.root.join("budget-marker");
    slow_gate(
        &workspace,
        "gate.budget",
        &marker,
        &workspace.root.join("budget.release"),
    );
    slow_gate(
        &workspace,
        "gate.budget.second",
        &workspace.root.join("budget-second"),
        &workspace.root.join("budget-second.release"),
    );
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "budget event",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/budget/**"], &["gate.budget"]);
    workspace.activate_card_with_gates("F-002", &["src/budget-second/**"], &["gate.budget.second"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace.work(&["start", "--card-id", "F-002"]);
    let profile = profile(&workspace);
    let reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.budget",
        "--execution-mode",
        "cpu-heavy",
        "--cpu-profile",
        &profile,
        "--actor",
        "holder",
    ])["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second_reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-002",
        "--gate-id",
        "gate.budget.second",
        "--execution-mode",
        "cpu-heavy",
        "--cpu-profile",
        &profile,
        "--actor",
        "holder",
    ])["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let before_policy = workspace.control_head();
    // The implementation supplies a small deterministic fixture command; this
    // proof requires its event to be advisory only and never authorize work.
    let run_budget = |reservation_id: &str| {
        Command::new(env!("CARGO_BIN_EXE_change-harness"))
            .args([
                "gate",
                "schedule",
                "--output",
                "json",
                "--control",
                workspace.control.to_str().unwrap(),
                "--reservation-id",
                reservation_id,
                "--record-budget-crossing",
                "validation-suite",
                "--observed-seconds",
                "61",
                "--budget-seconds",
                "60",
            ])
            .output()
            .unwrap()
    };
    let output = run_budget(&reservation);
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["data"]["budget_event"]["schema"],
        "harness.validation-budget-event/v1"
    );
    assert_eq!(value["data"]["budget_event"]["action"], "observe_only");
    assert!(value["data"]["budget_event"]["policy_digest"].is_string());
    assert!(value["data"]["budget_event"]["capacity_changed"].is_null());
    let repeat = run_budget(&reservation);
    assert!(repeat.status.success());
    assert_eq!(
        workspace
            .events()
            .into_iter()
            .filter(|event| event["event_type"] == "validation.budget_crossed")
            .count(),
        1
    );
    let different_reservation = run_budget(&second_reservation);
    assert!(different_reservation.status.success());
    assert_eq!(
        workspace
            .events()
            .into_iter()
            .filter(|event| event["event_type"] == "validation.budget_crossed")
            .count(),
        2,
        "the same numeric observation for a distinct reservation must not be deduplicated"
    );
    assert!(
        workspace.control_head() != before_policy,
        "the one event is durable"
    );
    assert!(!workspace.control.join("cpu-heavy-lanes").exists());
    assert!(
        workspace
            .control
            .join(format!("validation-reservations/{reservation}.json"))
            .exists()
    );
}
