//! WP-550 chunk 1: the project snapshot is typed, redacted, read-only, and
//! bound to one captured control commit.

mod support;

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;

use change_harness::{
    cli::output::OutputFormat,
    commands::{
        project::SnapshotArgs,
        project_snapshot::{self, WatchTermination},
    },
    control::repository::ControlRepository,
    domain::{
        clock::{FixedClock, SystemClock},
        digest::Digest,
        project_snapshot::ProjectSnapshot,
    },
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

fn git_head(repository: &Path) -> String {
    let output = Command::new("git")
        .args(["-C", repository.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn receipt_path(workspace: &Workspace) -> PathBuf {
    fs::read_dir(workspace.control.join("receipts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .expect("a gate receipt")
}

fn first_json_path(directory: &Path) -> PathBuf {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .expect("a JSON control record")
}

fn copy_control_record(workspace: &Workspace, source: &str, destination: &str) {
    fs::copy(
        workspace.control.join(source),
        workspace.control.join(destination),
    )
    .unwrap();
    commit_control(workspace);
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
            "test: update snapshot fixture",
        ])
        .status()
        .unwrap();
    assert!(commit.success());
}

fn backdate_held_leases(workspace: &Workspace, lease_ids: &[&str]) {
    for lease_id in lease_ids {
        let path = workspace.control.join(format!("leases/{lease_id}.json"));
        let mut lease: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).expect("lease JSON");
        lease["granted_at"] = "1970-01-01T00:00:00Z".into();
        fs::write(&path, serde_json::to_vec_pretty(&lease).unwrap()).unwrap();
    }
    commit_control(workspace);
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

fn legacy_revoked_handoff_workspace() -> Workspace {
    let workspace = approved_workspace();
    let handoff_path = first_json_path(&workspace.control.join("handoffs"));
    let review_path = first_json_path(&workspace.control.join("reviews"));
    let mut handoff: Value =
        serde_json::from_str(&fs::read_to_string(&handoff_path).unwrap()).expect("handoff JSON");
    let mut review: Value =
        serde_json::from_str(&fs::read_to_string(&review_path).unwrap()).expect("review JSON");

    // Reproduce a handoff written before dependency_bindings existed. The
    // review's historical digest was computed over that exact object, not
    // over the typed record after serde supplied an empty default.
    handoff
        .as_object_mut()
        .unwrap()
        .remove("dependency_bindings");
    review["handoff_digest"] =
        serde_json::to_value(Digest::of_canonical(&handoff).unwrap()).unwrap();
    fs::write(&handoff_path, serde_json::to_vec_pretty(&handoff).unwrap()).unwrap();
    fs::write(&review_path, serde_json::to_vec_pretty(&review).unwrap()).unwrap();
    commit_control(&workspace);

    handoff["status"] = "revoked".into();
    fs::write(&handoff_path, serde_json::to_vec_pretty(&handoff).unwrap()).unwrap();
    commit_control(&workspace);
    workspace
}

#[test]
fn watch_json_is_refused_with_a_stable_usage_error() {
    let workspace = Workspace::initialized();
    let before_head = workspace.control_head();
    let output = Workspace::run(&[
        "project".into(),
        "snapshot".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--watch".into(),
        "--output".into(),
        "json".into(),
    ]);

    assert_eq!(output.status.code(), Some(2));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("error envelope");
    assert_eq!(envelope["error"]["code"], "CH-USAGE-CONFLICTING-OPTIONS");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot be combined")
    );
    assert!(!envelope["error"]["recovery"].as_str().unwrap().is_empty());
    assert_eq!(workspace.control_head(), before_head);
}

#[test]
fn watch_interval_is_bounded_before_the_command_reads_state() {
    let workspace = Workspace::initialized();
    let before_head = workspace.control_head();
    let output = Workspace::run(&[
        "project".into(),
        "snapshot".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--watch".into(),
        "--interval-ms".into(),
        "99".into(),
        "--output".into(),
        "json".into(),
    ]);

    assert_eq!(output.status.code(), Some(2));
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("error envelope");
    assert_eq!(envelope["error"]["code"], "CH-USAGE-INVALID-ARGUMENTS");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("between 100 and 3600000")
    );
    assert_eq!(workspace.control_head(), before_head);
}

#[test]
fn watch_non_tty_emits_one_plain_frame_without_mutation() {
    let workspace = Workspace::initialized();
    let before_control = workspace.control_head();
    let before_repository = git_head(&workspace.repository);
    let before_authority = git_head(&workspace.authority);
    let output = Workspace::run(&[
        "project".into(),
        "snapshot".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--watch".into(),
        "--interval-ms".into(),
        "100".into(),
    ]);

    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.matches("Project example snapshot").count(), 1);
    assert!(!text.contains('\x1b'));
    assert_eq!(workspace.control_head(), before_control);
    assert_eq!(git_head(&workspace.repository), before_repository);
    assert_eq!(git_head(&workspace.authority), before_authority);
    assert!(control_status(&workspace.control).is_empty());
}

struct BrokenPipeWriter;

impl Write for BrokenPipeWriter {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "consumer closed"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn watch_stops_cleanly_when_the_output_pipe_closes() {
    let workspace = Workspace::initialized();
    let args = SnapshotArgs {
        control: workspace.control.clone(),
        watch: true,
        interval_ms: Some(100),
    };
    let result = project_snapshot::run_watch(
        &args,
        OutputFormat::Text,
        &SystemClock,
        &mut BrokenPipeWriter,
        true,
    );

    assert_eq!(result.unwrap(), WatchTermination::OutputClosed);
}

struct ControlChangingWriter<'a> {
    workspace: &'a Workspace,
    bytes: Vec<u8>,
    changed: bool,
}

impl Write for ControlChangingWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        let header_count = self
            .bytes
            .windows(b"Project example snapshot".len())
            .filter(|window| *window == b"Project example snapshot")
            .count();
        if !self.changed && header_count >= 1 {
            self.workspace
                .register_gate_revision("gate.unit", 2, &["true"]);
            self.changed = true;
        }
        if header_count >= 2 {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "stop test"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn watch_recollects_the_control_head_between_frames() {
    let workspace = Workspace::initialized();
    let before_head = workspace.control_head();
    let args = SnapshotArgs {
        control: workspace.control.clone(),
        watch: true,
        interval_ms: Some(100),
    };
    let mut writer = ControlChangingWriter {
        workspace: &workspace,
        bytes: Vec::new(),
        changed: false,
    };

    let result =
        project_snapshot::run_watch(&args, OutputFormat::Text, &SystemClock, &mut writer, true);

    assert_eq!(result.unwrap(), WatchTermination::OutputClosed);
    assert_ne!(workspace.control_head(), before_head);
    let text = String::from_utf8(writer.bytes).unwrap();
    assert_eq!(text.matches("Project example snapshot").count(), 2);
    assert!(text.contains("control head:"));
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
fn terminal_cycle_revision_drift_is_suppressed_for_closed_and_abandoned_cycles() {
    let workspace = Workspace::initialized();

    // No command closes a cycle yet, so use the established fixture seam for
    // the terminal-success state. Abandonment is exercised through the real
    // lifecycle command below.
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Closed legacy",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.tamper_cycle_status("C-001", "closed");
    commit_control(&workspace);

    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-002",
        "--objective",
        "Abandoned legacy",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-002"]);
    workspace.cycle(&["abandon", "--cycle-id", "C-002", "--reason", "obsolete"]);

    // Both cycles retain their original revision while the current project
    // document advances. Snapshot must keep the records and counts but omit
    // only the no-longer-actionable revision warning.
    workspace.configure_convergence_policy(3, 3);
    let snapshot = snapshot_json(&workspace)["data"].clone();
    let diagnostics = snapshot["consistency"]["diagnostics"]
        .as_array()
        .expect("diagnostics array");

    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic == "cycle_project_revision_mismatch"),
        "terminal legacy cycles must not create revision-drift noise: {snapshot}"
    );
    assert_eq!(snapshot["cycle_state_counts"]["abandoned"], 1);
    assert_eq!(
        fs::read_dir(workspace.control.join("cycles"))
            .unwrap()
            .filter_map(Result::ok)
            .count(),
        2,
        "snapshot suppression must not remove historical cycle records"
    );
}

#[test]
fn non_terminal_cycle_revision_drift_remains_visible() {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "Live work"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.configure_convergence_policy(3, 3);

    let snapshot = snapshot_json(&workspace)["data"].clone();
    assert!(
        snapshot["consistency"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic == "cycle_project_revision_mismatch"),
        "an active cycle with revision drift must remain actionable: {snapshot}"
    );
    assert_eq!(snapshot["cycle_state_counts"]["active"], 1);
}

#[test]
fn terminal_card_leases_are_suppressed_for_closed_and_abandoned_cards() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Legacy leases",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/abandoned/**"]);
    workspace.activate_card("F-002", &["src/closed/**"]);
    let abandoned = workspace.work_json(&["start", "--card-id", "F-001"]);
    let closed = workspace.work_json(&["start", "--card-id", "F-002"]);
    let abandoned_lease = abandoned["data"]["lease_id"].as_str().unwrap();
    let closed_lease = closed["data"]["lease_id"].as_str().unwrap();

    workspace.card(&["abandon", "--card-id", "F-001", "--reason", "obsolete"]);
    // Archive close normally releases a successful card's lease, so the
    // established state-tamper seam constructs the legacy closed+held shape.
    workspace.tamper_card_state("F-002", "closed");
    backdate_held_leases(&workspace, &[abandoned_lease, closed_lease]);

    let before_head = workspace.control_head();
    let snapshot = snapshot_json(&workspace)["data"].clone();
    assert!(
        snapshot["silent_leases"].as_array().unwrap().is_empty(),
        "terminal cards must not appear as silent active work: {snapshot}"
    );
    assert_eq!(snapshot["card_state_counts"]["abandoned"], 1);
    assert_eq!(snapshot["card_state_counts"]["closed"], 1);
    for lease_id in [abandoned_lease, closed_lease] {
        let lease: Value = serde_json::from_str(
            &fs::read_to_string(workspace.control.join(format!("leases/{lease_id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(lease["status"], "held");
    }
    assert_eq!(workspace.control_head(), before_head);
}

#[test]
fn non_terminal_card_silent_lease_remains_visible() {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "Live lease"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/live/**"]);
    let started = workspace.work_json(&["start", "--card-id", "F-001"]);
    let lease_id = started["data"]["lease_id"].as_str().unwrap();
    backdate_held_leases(&workspace, &[lease_id]);

    let snapshot = snapshot_json(&workspace)["data"].clone();
    let silent = snapshot["silent_leases"].as_array().unwrap();
    assert_eq!(
        silent.len(),
        1,
        "live silent work must remain visible: {snapshot}"
    );
    assert_eq!(silent[0]["lease_id"], lease_id);
    assert_eq!(silent[0]["card_id"], "F-001");
    assert_eq!(snapshot["card_state_counts"]["active"], 1);
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
fn legacy_handoff_digest_uses_the_captured_blob_and_still_rejects_tampering() {
    let workspace = legacy_revoked_handoff_workspace();

    // The legacy field omission is compatible: the snapshot must validate the
    // review against the canonical object actually stored in control Git.
    let snapshot = snapshot_json(&workspace);
    assert_eq!(snapshot["data"]["project_id"], "example");

    // A real handoff mutation must still break the binding. This prevents the
    // compatibility path from degrading into "trust the review" behavior.
    let handoff_path = first_json_path(&workspace.control.join("handoffs"));
    let mut handoff: Value = serde_json::from_str(&fs::read_to_string(&handoff_path).unwrap())
        .expect("legacy handoff JSON");
    handoff["branch"] = "tampered-branch".into();
    fs::write(&handoff_path, serde_json::to_vec_pretty(&handoff).unwrap()).unwrap();
    commit_control(&workspace);

    snapshot_refusal(
        &workspace,
        "project snapshot durable-record integrity: review_handoff_binding_invalid",
    );
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

#[test]
fn duplicate_cycle_records_are_rejected_before_projection() {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "Snapshot"]);

    copy_control_record(&workspace, "cycles/C-001.json", "cycles/C-001-copy.json");

    snapshot_refusal(
        &workspace,
        "project snapshot durable-record integrity: duplicate_cycle_identity",
    );
}

#[test]
fn duplicate_card_revisions_are_rejected_before_projection() {
    let workspace = approved_workspace();

    copy_control_record(&workspace, "cards/F-001/r1.json", "cards/F-001/r2.json");

    snapshot_refusal(
        &workspace,
        "project snapshot durable-record integrity: duplicate_card_revision_identity",
    );
}

#[test]
fn duplicate_review_records_are_rejected_before_projection() {
    let workspace = approved_workspace();
    let review = first_json_path(&workspace.control.join("reviews"));
    let review_name = review.file_name().unwrap().to_str().unwrap();

    copy_control_record(
        &workspace,
        &format!("reviews/{review_name}"),
        "reviews/duplicate.json",
    );

    snapshot_refusal(
        &workspace,
        "project snapshot durable-record integrity: duplicate_review_identity",
    );
}

#[test]
fn duplicate_integration_records_are_rejected_before_projection() {
    let workspace = approved_workspace();
    let prepared = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    let integration_id = prepared["data"]["integration_id"].as_str().unwrap();

    copy_control_record(
        &workspace,
        &format!("integrations/{integration_id}.json"),
        "integrations/duplicate.json",
    );

    snapshot_refusal(
        &workspace,
        "project snapshot durable-record integrity: duplicate_integration_identity",
    );
}

#[test]
fn non_receipt_record_file_name_must_match_its_identity() {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "Snapshot"]);
    fs::rename(
        workspace.control.join("cycles/C-001.json"),
        workspace.control.join("cycles/not-the-id.json"),
    )
    .unwrap();
    commit_control(&workspace);

    snapshot_refusal(
        &workspace,
        "project snapshot durable-record integrity: cycle_file_name_mismatch",
    );
}

#[test]
fn card_revision_file_name_must_match_its_composite_identity() {
    let workspace = approved_workspace();
    let path = workspace.control.join("cards/F-001/r1-copy.json");
    fs::copy(workspace.control.join("cards/F-001/r1.json"), &path).unwrap();
    let mut card: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    card["revision"] = 2.into();
    fs::write(&path, serde_json::to_vec_pretty(&card).unwrap()).unwrap();
    commit_control(&workspace);

    snapshot_refusal(
        &workspace,
        "project snapshot durable-record integrity: card_revision_file_name_mismatch",
    );
}
