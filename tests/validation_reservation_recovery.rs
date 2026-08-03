//! #65 frozen proof: expired reservation recovery creates one linked successor.

mod support;

use std::{
    fs,
    process::{Command, Output},
    sync::{Arc, Barrier},
    thread,
};

use support::Workspace;

fn marker_gate(workspace: &Workspace, gate_id: &str, marker: &std::path::Path, verdict: &str) {
    let command = format!("printf invoked > \"$MARKER_PATH\"; {verdict}");
    let argv = serde_json::to_string(&["sh", "-c", &command]).unwrap();
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let body = format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER_PATH: {marker}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n",
    );
    let definition = workspace.gate_definition(gate_id, &body);
    workspace.gate(&["register", "--definition", &definition]);
}

fn expire(workspace: &Workspace, reservation_id: &str) {
    let path = workspace
        .control
        .join(format!("validation-reservations/{reservation_id}.json"));
    let mut reservation: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    reservation["expires_at"] = serde_json::json!("1970-01-01T00:00:00Z");
    fs::write(&path, serde_json::to_vec_pretty(&reservation).unwrap()).unwrap();
    assert!(
        Command::new("git")
            .args(["-C", workspace.control.to_str().unwrap(), "add", "-A"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-C",
                workspace.control.to_str().unwrap(),
                "commit",
                "-m",
                "expire reservation fixture",
            ])
            .status()
            .unwrap()
            .success()
    );
}

fn reserve_process(control: &std::path::Path, actor: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args([
            "gate",
            "reserve",
            "--output",
            "json",
            "--control",
            control.to_str().unwrap(),
            "--card-id",
            "F-001",
            "--gate-id",
            "gate.named",
            "--actor",
            actor,
        ])
        .output()
        .unwrap()
}

fn command_with_setup_failure(workspace: &Workspace, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .env("CHANGE_HARNESS_FAIL_AT", "validation-execution-setup")
        .args(["gate", args[0], "--output", "json", "--control"])
        .arg(&workspace.control)
        .args(&args[1..])
        .output()
        .unwrap()
}

#[test]
fn holder_abandons_only_expired_reservations_and_one_linked_generation_recovers() {
    let workspace = Workspace::initialized();
    let named_marker = workspace.root.join("named-ran");
    let mutation_marker = workspace.root.join("mutation-ran");
    marker_gate(&workspace, "gate.named", &named_marker, "true");
    marker_gate(&workspace, "gate.mutation", &mutation_marker, "false");
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Recover one expired reservation",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/named/**"], &["gate.named"]);
    workspace.activate_card_with_gates("F-002", &["src/mutation/**"], &["gate.mutation"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace.work(&["start", "--card-id", "F-002"]);

    let campaign = workspace.root.join("campaign.json");
    fs::write(
        &campaign,
        r#"{"schema":"harness.declared-mutation-campaign/v1","mutations":[{"id":"M-001","path":"README.md","expected_utf8":"hello\n","replacement_utf8":"mutant\n"}]}"#,
    )
    .unwrap();
    let named = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.named",
        "--actor",
        "holder",
    ]);
    let mutation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-002",
        "--gate-id",
        "gate.mutation",
        "--execution-mode",
        "declared-mutations",
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    let named_id = named["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mutation_id = mutation["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let premature = workspace.gate_raw(&[
        "abandon",
        "--reservation-id",
        &named_id,
        "--actor",
        "holder",
    ]);
    assert!(
        !premature.status.success(),
        "a live unexpired permit is not abandonable"
    );
    expire(&workspace, &named_id);
    expire(&workspace, &mutation_id);

    let wrong_holder =
        workspace.gate_raw(&["abandon", "--reservation-id", &named_id, "--actor", "other"]);
    assert!(
        !wrong_holder.status.success(),
        "only the original holder may abandon"
    );

    for reservation_id in [&named_id, &mutation_id] {
        let abandoned = workspace.gate_json(&[
            "abandon",
            "--reservation-id",
            reservation_id,
            "--actor",
            "holder",
        ]);
        assert_eq!(
            abandoned["data"]["settlement"]["outcome"]["kind"],
            "abandoned"
        );
    }

    let old_run = command_with_setup_failure(
        &workspace,
        &[
            "run",
            "--card-id",
            "F-001",
            "--gate-id",
            "gate.named",
            "--reservation-id",
            &named_id,
            "--actor",
            "holder",
        ],
    );
    let old_mutate = command_with_setup_failure(
        &workspace,
        &[
            "mutate",
            "--reservation-id",
            &mutation_id,
            "--campaign",
            campaign.to_str().unwrap(),
            "--actor",
            "holder",
        ],
    );
    assert!(!old_run.status.success());
    assert!(!old_mutate.status.success());
    assert!(!named_marker.exists());
    assert!(!mutation_marker.exists());

    let barrier = Arc::new(Barrier::new(3));
    let run = |actor: &'static str| {
        let barrier = Arc::clone(&barrier);
        let control = workspace.control.clone();
        thread::spawn(move || {
            barrier.wait();
            reserve_process(&control, actor)
        })
    };
    let first = run("recover-a");
    let second = run("recover-b");
    barrier.wait();
    let first: serde_json::Value = serde_json::from_slice(&first.join().unwrap().stdout).unwrap();
    let second: serde_json::Value = serde_json::from_slice(&second.join().unwrap().stdout).unwrap();
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
        waiter["data"]["reservation"]["reservation_id"],
    );
    assert_eq!(winner["data"]["reservation"]["generation"], 2);
    assert_eq!(
        winner["data"]["reservation"]["predecessor_reservation_id"],
        named_id,
    );
    assert_eq!(
        fs::read_dir(workspace.control.join("validation-reservations"))
            .unwrap()
            .count(),
        3,
        "concurrent recovery must create exactly one next generation",
    );
    let predecessor: serde_json::Value = serde_json::from_slice(
        &fs::read(
            workspace
                .control
                .join(format!("validation-reservations/{named_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(predecessor["generation"], 1);
    assert!(predecessor["predecessor_reservation_id"].is_null());
}
