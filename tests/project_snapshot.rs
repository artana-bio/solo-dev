//! WP-550 chunk 1: the project snapshot is typed, redacted, read-only, and
//! bound to one captured control commit.

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;

use change_harness::{
    control::repository::ControlRepository,
    domain::{clock::FixedClock, project_snapshot::ProjectSnapshot},
    error::ErrorCode,
};
use support::Workspace;

fn snapshot_json(workspace: &Workspace) -> serde_json::Value {
    let output = Workspace::run(&[
        "project".into(),
        "snapshot".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        output.status.success(),
        "snapshot failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("snapshot JSON envelope")
}

fn control_status(control: &Path) -> String {
    let output = Command::new("git")
        .args(["-C", control.to_str().unwrap(), "status", "--porcelain"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

fn receipt_path(workspace: &Workspace) -> PathBuf {
    fs::read_dir(workspace.control.join("receipts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .expect("a gate receipt")
}

fn commit_control(workspace: &Workspace) {
    let add = Command::new("git")
        .args(["-C", workspace.control.to_str().unwrap(), "add", "--all"])
        .status()
        .unwrap();
    assert!(add.success());
    let commit = Command::new("git")
        .args([
            "-C",
            workspace.control.to_str().unwrap(),
            "commit",
            "-q",
            "-m",
            "test: corrupt receipt fixture",
        ])
        .status()
        .unwrap();
    assert!(commit.success());
}

fn mutate_receipt(workspace: &Workspace, mutate: impl FnOnce(&mut Value)) {
    let path = receipt_path(workspace);
    let mut receipt: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    mutate(&mut receipt);
    fs::write(&path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    commit_control(workspace);
}

fn snapshot_refusal(workspace: &Workspace, reason: &str) {
    let output: Output = Workspace::run(&[
        "project".into(),
        "snapshot".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(!output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("error envelope");
    assert_eq!(envelope["error"]["code"], "CH-INTERNAL-CONTROL-CORRUPT");
    assert_eq!(envelope["error"]["details"]["reason"], reason);
}

fn approved_workspace() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "Snapshot"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace
}

#[test]
fn snapshot_json_and_text_use_the_redacted_typed_projection() {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "Snapshot"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);

    let json = snapshot_json(&workspace);
    assert_eq!(json["data"]["schema"], "harness.project-snapshot/v1");
    assert_eq!(
        json["data"]["control_head"],
        workspace.control_head(),
        "all durable data must name the captured control commit"
    );
    assert_eq!(json["data"]["cycle_state_counts"]["active"], 1);
    assert_eq!(json["data"]["card_state_counts"]["ready"], 1);
    assert_eq!(json["data"]["active_cards"].as_array().unwrap().len(), 0);
    assert!(
        !serde_json::to_string(&json)
            .unwrap()
            .contains(workspace.root.to_str().unwrap()),
        "machine-facing snapshot must not expose filesystem paths"
    );

    let text_output = Workspace::run(&[
        "project".into(),
        "snapshot".into(),
        "--control".into(),
        workspace.control.display().to_string(),
    ]);
    assert!(text_output.status.success());
    let text = String::from_utf8(text_output.stdout).unwrap();
    assert!(text.contains("Project example snapshot"));
    assert!(text.contains("control head:"));
    assert!(text.contains("cards: ready=1"));
}

#[test]
fn snapshot_reads_durable_records_from_head_and_reports_ephemeral_dirty_state() {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "Snapshot"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    let before_head = workspace.control_head();
    let state = workspace.control.join("cards/F-001/state.json");
    let original = fs::read_to_string(&state).unwrap();
    fs::write(&state, original.replace("ready", "active")).unwrap();

    let snapshot = snapshot_json(&workspace);
    assert_eq!(snapshot["data"]["control_head"], before_head);
    assert_eq!(
        snapshot["data"]["card_state_counts"]["ready"], 1,
        "uncommitted authoritative edits must not be mixed into the captured view"
    );
    assert_eq!(
        snapshot["data"]["consistency"]["control_worktree_clean"],
        false
    );
    assert_eq!(workspace.control_head(), before_head);
    assert!(!control_status(&workspace.control).is_empty());
}

#[test]
fn snapshot_reports_structured_gate_metrics_and_active_card_actor() {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "Snapshot"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");

    let snapshot = snapshot_json(&workspace)["data"].clone();
    assert!(snapshot["gate_metrics"]["attempts"].as_u64().unwrap() >= 1);
    assert!(snapshot["gate_metrics"]["duration_ms"].is_number());
    let active = snapshot["active_cards"].as_array().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0]["card_id"], "F-001");
    assert_eq!(active[0]["phase"], "approved");
    assert_eq!(active[0]["actor_id"], "operator");
    assert!(active[0]["last_activity_at"].is_string());
}

#[test]
fn stale_captured_head_is_rejected_instead_of_returning_a_mixed_snapshot() {
    let workspace = Workspace::initialized();
    let captured = workspace.control_head();
    workspace.register_gate("gate.new", &["true"]);

    let control = ControlRepository::open(&workspace.control).unwrap();
    let clock = FixedClock::at_unix_seconds(1_785_196_800).unwrap();
    let error = ProjectSnapshot::collect_at_head(&control, &captured, &clock).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConflictControlHeadMoved);
}

#[test]
fn duplicate_receipt_ids_are_rejected_before_metrics_projection() {
    let workspace = approved_workspace();
    let original = receipt_path(&workspace);
    fs::copy(&original, workspace.control.join("receipts/duplicate.json")).unwrap();
    commit_control(&workspace);

    snapshot_refusal(
        &workspace,
        "project snapshot receipt integrity: duplicate_receipt_id",
    );
}

#[test]
fn receipt_file_name_must_match_its_logical_id() {
    let workspace = approved_workspace();
    let original = receipt_path(&workspace);
    fs::rename(
        &original,
        workspace.control.join("receipts/not-the-id.json"),
    )
    .unwrap();
    commit_control(&workspace);

    snapshot_refusal(
        &workspace,
        "project snapshot receipt integrity: receipt_file_name_mismatch",
    );
}

#[test]
fn receipt_cannot_name_a_card_and_an_integration() {
    let workspace = approved_workspace();
    mutate_receipt(&workspace, |receipt| {
        receipt["integration_id"] = "INT-001".into();
    });

    snapshot_refusal(
        &workspace,
        "project snapshot receipt integrity: receipt_subject_invalid",
    );
}

#[test]
fn card_receipt_requires_a_digest() {
    let workspace = approved_workspace();
    mutate_receipt(&workspace, |receipt| receipt["card_digest"] = Value::Null);

    snapshot_refusal(
        &workspace,
        "project snapshot receipt integrity: receipt_subject_invalid",
    );
}

#[test]
fn receipt_card_reference_must_belong_to_its_cycle() {
    let workspace = approved_workspace();
    mutate_receipt(&workspace, |receipt| receipt["card_id"] = "F-002".into());

    snapshot_refusal(
        &workspace,
        "project snapshot receipt integrity: receipt_card_cycle_mismatch",
    );
}

#[test]
fn receipt_cycle_reference_must_exist() {
    let workspace = approved_workspace();
    mutate_receipt(&workspace, |receipt| receipt["cycle_id"] = "C-999".into());

    snapshot_refusal(
        &workspace,
        "project snapshot receipt integrity: receipt_cycle_reference_missing",
    );
}

#[test]
fn integration_receipt_reference_must_exist() {
    let workspace = approved_workspace();
    mutate_receipt(&workspace, |receipt| {
        receipt["card_id"] = Value::Null;
        receipt["card_digest"] = Value::Null;
        receipt["integration_id"] = "INT-999".into();
    });

    snapshot_refusal(
        &workspace,
        "project snapshot receipt integrity: receipt_integration_reference_invalid",
    );
}
