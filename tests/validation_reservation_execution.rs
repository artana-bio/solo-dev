//! #61 frozen proof: gate execution requires one exact live reservation.

mod support;

use std::{fs, path::PathBuf};

use support::{Workspace, git};

fn allocated() -> Workspace {
    let workspace = Workspace::initialized();
    let marker = serde_json::to_string(
        &workspace
            .root
            .join("reservation-run-marker")
            .display()
            .to_string(),
    )
    .unwrap();
    let definition = workspace.gate_definition(
        "gate-marker",
        &format!(
            "schema: harness.gate/v1\ngate_id: gate.marker\nrevision: 1\nargv: [sh, -c, \"printf invoked >> \\\"$MARKER_PATH\\\"\"]\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER_PATH: {marker}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
        ),
    );
    workspace.gate(&["register", "--definition", &definition]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Run one reserved proof",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.marker"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace
}

fn marker(workspace: &Workspace) -> PathBuf {
    workspace.root.join("reservation-run-marker")
}

fn reserve(workspace: &Workspace, actor: &str) -> (String, String) {
    let value = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.marker",
        "--actor",
        actor,
    ]);
    (
        value["data"]["reservation"]["reservation_id"]
            .as_str()
            .unwrap()
            .to_owned(),
        value["data"]["reservation"]["key_digest"]
            .as_str()
            .unwrap()
            .to_owned(),
    )
}

fn run(workspace: &Workspace, reservation_id: Option<&str>, actor: &str) -> std::process::Output {
    let mut args = vec!["run", "--card-id", "F-001", "--gate-id", "gate.marker"];
    if let Some(reservation_id) = reservation_id {
        args.extend(["--reservation-id", reservation_id]);
    }
    args.extend(["--actor", actor]);
    workspace.gate_raw(&args)
}

fn run_dry(
    workspace: &Workspace,
    reservation_id: Option<&str>,
    actor: &str,
) -> std::process::Output {
    let mut args = vec![
        "run",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.marker",
        "--dry-run",
    ];
    if let Some(reservation_id) = reservation_id {
        args.extend(["--reservation-id", reservation_id]);
    }
    args.extend(["--actor", actor]);
    workspace.gate_raw(&args)
}

fn assert_not_started(workspace: &Workspace, output: &std::process::Output) {
    assert!(
        !output.status.success(),
        "a reservation refusal must occur before the gate subprocess starts"
    );
    assert!(
        !marker(workspace).exists(),
        "a refused run must not create the gate's marker"
    );
}

#[test]
fn gate_execution_requires_one_exact_live_holder_reservation_before_subprocess_side_effects() {
    let missing = allocated();
    assert_not_started(&missing, &run(&missing, None, "holder"));
    assert_not_started(&missing, &run_dry(&missing, None, "holder"));

    let wrong_actor = allocated();
    let (reservation_id, _) = reserve(&wrong_actor, "holder");
    assert_not_started(
        &wrong_actor,
        &run(&wrong_actor, Some(&reservation_id), "other"),
    );

    let moved = allocated();
    let (reservation_id, _) = reserve(&moved, "holder");
    let worktree: PathBuf =
        moved.work_json(&["status", "--card-id", "F-001"])["data"]["held_lease"]["worktree_path"]
            .as_str()
            .unwrap()
            .into();
    fs::write(worktree.join("moved.txt"), "later\n").unwrap();
    git(&worktree, &["add", "moved.txt"]);
    git(&worktree, &["commit", "-qm", "move candidate"]);
    assert_not_started(&moved, &run(&moved, Some(&reservation_id), "holder"));

    let settled = allocated();
    let (reservation_id, _) = reserve(&settled, "holder");
    settled.gate(&[
        "settle",
        "--reservation-id",
        &reservation_id,
        "--outcome",
        "abandoned",
        "--actor",
        "holder",
    ]);
    assert_not_started(&settled, &run(&settled, Some(&reservation_id), "holder"));

    let allowed = allocated();
    let (reservation_id, key_digest) = reserve(&allowed, "holder");
    let output = run(&allowed, Some(&reservation_id), "holder");
    assert!(
        output.status.success(),
        "matching live holder reservation must permit one run: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(fs::read_to_string(marker(&allowed)).unwrap(), "invoked");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["provenance"]["validation_reservation"]["reservation_id"],
        reservation_id
    );
    assert_eq!(
        envelope["data"]["provenance"]["validation_reservation"]["key_digest"],
        key_digest
    );
    let second = run(&allowed, Some(&reservation_id), "holder");
    assert!(
        !second.status.success(),
        "a consumed reservation must refuse the second attempt"
    );
    assert_eq!(
        fs::read_to_string(marker(&allowed)).unwrap(),
        "invoked",
        "a consumed reservation must never launch a second subprocess"
    );
    assert_eq!(
        allowed.gate_json(&["status", "--card-id", "F-001"])["data"]["receipts"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "a consumed reservation must not create a second receipt"
    );
}
