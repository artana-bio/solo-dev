//! #68 frozen proof: authoritative CPU-heavy next-action projection.

mod support;

use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use support::Workspace;

fn profile(workspace: &Workspace) -> String {
    let path = workspace.root.join("cpu.json");
    fs::write(&path, r#"{"schema":"harness.cpu-heavy-validation-profile/v1","risk":"high","expected_duration_seconds":60,"resource_cost":{"cpu_cores":1,"memory_mib":1024}}"#).unwrap();
    path.display().to_string()
}

fn slow_gate(workspace: &Workspace, gate_id: &str, marker: &Path) {
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let definition = workspace.gate_definition(gate_id, &format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: [\"sh\", \"-c\", \"printf started > \\\"$MARKER\\\"; sleep 0.5\"]\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER: {marker}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
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

fn wait_for(marker: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists());
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
    let independent_marker = workspace.root.join("independent");
    for (gate, marker) in [
        ("gate.one", &markers[0]),
        ("gate.two", &markers[1]),
        ("gate.three", &markers[2]),
    ] {
        slow_gate(&workspace, gate, marker);
    }
    slow_gate(&workspace, "gate.independent", &independent_marker);
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
    let control = workspace.control.clone();
    let run = |card: &'static str, gate: &'static str, id: String| {
        let control = control.clone();
        thread::spawn(move || {
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
                    &id,
                    "--actor",
                    "holder",
                ])
                .output()
                .unwrap()
        })
    };
    let one = run("F-001", "gate.one", first);
    let two = run("F-002", "gate.two", second);
    wait_for(&markers[0]);
    wait_for(&markers[1]);
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
    assert!(one.join().unwrap().status.success());
    assert!(two.join().unwrap().status.success());
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
    slow_gate(&workspace, "gate.wait.one", &first_marker);
    slow_gate(&workspace, "gate.wait.two", &second_marker);
    slow_gate(
        &workspace,
        "gate.wait.three",
        &workspace.root.join("wait-third"),
    );
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "wait only"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    for (card, scope, gate) in [
        ("F-001", "src/one/**", "gate.wait.one"),
        ("F-002", "src/two/**", "gate.wait.two"),
        ("F-003", "src/three/**", "gate.wait.three"),
    ] {
        workspace.activate_card_with_gates(card, &[scope], &[gate]);
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
    let control = workspace.control.clone();
    let execute = |card: &'static str, gate: &'static str, reservation: String| {
        let control = control.clone();
        thread::spawn(move || {
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
                    &reservation,
                    "--actor",
                    "holder",
                ])
                .output()
                .unwrap()
        })
    };
    let first_run = execute("F-001", "gate.wait.one", first);
    let second_run = execute("F-002", "gate.wait.two", second);
    wait_for(&first_marker);
    wait_for(&second_marker);
    let before = workspace.control_head();
    let schedule: serde_json::Value =
        serde_json::from_slice(&schedule_raw(&workspace.control, &third, None).stdout).unwrap();
    assert_eq!(schedule["data"]["recommendation"]["kind"], "wait");
    assert_eq!(schedule["data"]["blocker"]["kind"], "cpu_lanes_occupied");
    assert!(schedule["data"]["release_condition"].is_object());
    assert_eq!(workspace.control_head(), before, "schedule is read-only");
    assert!(first_run.join().unwrap().status.success());
    assert!(second_run.join().unwrap().status.success());
}

#[test]
fn budget_crossing_emits_one_event_without_changing_capacity_or_policy() {
    let workspace = Workspace::initialized();
    let marker = workspace.root.join("budget-marker");
    slow_gate(&workspace, "gate.budget", &marker);
    slow_gate(
        &workspace,
        "gate.budget.second",
        &workspace.root.join("budget-second"),
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
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace.activate_card_with_gates("F-002", &["src/budget-second/**"], &["gate.budget.second"]);
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
