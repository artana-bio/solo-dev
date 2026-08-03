//! #62 frozen proof: one governed run uses one disposable source and cache.

mod support;

use std::{fs, path::PathBuf};

use support::Workspace;

fn marker_gate(workspace: &Workspace, gate_id: &str, marker: &PathBuf) {
    let command = "printf '%s\\n%s\\n%s\\n' \"$PWD\" \"$CHANGE_HARNESS_VALIDATION_CACHE\" \"$(git rev-parse HEAD)\" > \"$MARKER_PATH\"";
    let argv = serde_json::to_string(&["sh", "-c", command]).unwrap();
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let body = format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER_PATH: {marker}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n",
    );
    let definition = workspace.gate_definition(gate_id, &body);
    workspace.gate(&["register", "--definition", &definition]);
}

fn allocated() -> (Workspace, PathBuf, PathBuf) {
    let workspace = Workspace::initialized();
    let first_marker = workspace.root.join("first-marker.txt");
    let second_marker = workspace.root.join("second-marker.txt");
    marker_gate(&workspace, "gate.first", &first_marker);
    marker_gate(&workspace, "gate.second", &second_marker);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Prove isolated validation execution",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/one/**"], &["gate.first"]);
    workspace.activate_card_with_gates("F-002", &["src/two/**"], &["gate.second"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace.work(&["start", "--card-id", "F-002"]);
    (workspace, first_marker, second_marker)
}

fn reserve_and_run(workspace: &Workspace, card_id: &str, gate_id: &str) -> serde_json::Value {
    let reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        card_id,
        "--gate-id",
        gate_id,
        "--actor",
        "holder",
    ]);
    let reservation_id = reservation["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    let output = workspace.gate_raw(&[
        "run",
        "--card-id",
        card_id,
        "--gate-id",
        gate_id,
        "--reservation-id",
        reservation_id,
        "--actor",
        "holder",
    ]);
    assert!(
        output.status.success(),
        "reserved gate must run: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    reservation
}

fn marker(path: &PathBuf) -> (PathBuf, PathBuf, String) {
    let lines = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 3, "marker must record cwd, cache, and SHA");
    (
        PathBuf::from(&lines[0]),
        PathBuf::from(&lines[1]),
        lines[2].clone(),
    )
}

#[test]
fn reserved_gates_run_in_distinct_disposable_exact_source_and_cache_directories() {
    let (workspace, first_marker, second_marker) = allocated();
    let first = reserve_and_run(&workspace, "F-001", "gate.first");
    let second = reserve_and_run(&workspace, "F-002", "gate.second");
    let (first_source, first_cache, first_sha) = marker(&first_marker);
    let (second_source, second_cache, second_sha) = marker(&second_marker);
    let first_candidate = workspace.work_json(&["status", "--card-id", "F-001"])["data"]
        ["held_lease"]["worktree_path"]
        .as_str()
        .unwrap()
        .to_owned();
    let second_candidate = workspace.work_json(&["status", "--card-id", "F-002"])["data"]
        ["held_lease"]["worktree_path"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first_source, PathBuf::from(first_candidate));
    assert_ne!(second_source, PathBuf::from(second_candidate));
    assert!(
        !first_source.exists(),
        "source copy must be cleaned after attempt"
    );
    assert!(!first_cache.exists(), "cache must be cleaned after attempt");
    assert_ne!(
        first_cache, second_cache,
        "reservations must not share a cache"
    );
    assert_eq!(
        first_sha,
        first["data"]["reservation"]["key"]["candidate_sha"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        second_sha,
        second["data"]["reservation"]["key"]["candidate_sha"]
            .as_str()
            .unwrap()
    );
}
