//! `WP-310` acceptance: the gate runner and its receipts.

mod support;

use std::fs;

use change_harness::domain::digest::Digest;
use serde_json::Value;
use support::Workspace;

/// A gate definition body.
fn definition(gate_id: &str, argv: &str, timeout: u64) -> String {
    format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: {timeout}\nenvironment:\n  allow: [PATH]\n  set: {{}}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
    )
}

/// A workspace with an allocated card and a candidate commit.
fn allocated() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

fn snapshot_json(workspace: &Workspace) -> Value {
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
    serde_json::from_slice(&output.stdout).unwrap()
}

fn junit_gate_definition(gate_id: &str, script: &str, reports: &str, max_attempts: u32) -> String {
    let argv = serde_json::to_string(&["sh", "-c", script]).unwrap();
    format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set: {{}}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: {max_attempts}\nartifacts: []\njunit_reports: {reports}\n"
    )
}

fn junit_workspace(gate_id: &str, script: &str, reports: &str, max_attempts: u32) -> Workspace {
    let workspace = Workspace::initialized();
    let path = workspace.gate_definition(
        gate_id,
        &junit_gate_definition(gate_id, script, reports, max_attempts),
    );
    workspace.gate(&["register", "--definition", &path]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "JUnit metrics",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &[gate_id]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace
}

const VALID_JUNIT: &str = "<testsuite tests=\"4\" failures=\"1\" errors=\"1\" skipped=\"1\"><testcase/><testcase><failure>do not expose this</failure></testcase><testcase><error>do not expose this</error></testcase><testcase><skipped/></testcase></testsuite>";

/// Extracts every backtick-delimited span from `text`, in source order.
/// Mirrors the extraction rule `tests/recovery_text.rs` uses to check
/// command references named in `src/error.rs`'s recovery text against the
/// real CLI, applied here to a refusal's `reason` text instead.
fn backtick_spans(text: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('`') else {
            break;
        };
        spans.push(&after_open[..close]);
        rest = &after_open[close + 1..];
    }
    spans
}

/// Executes one known gate through the only production-capable path: an exact
/// holder reservation followed by the governed disposable-source run.
fn reserved_run_raw(workspace: &Workspace, card_id: &str, gate_id: &str) -> std::process::Output {
    let reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        card_id,
        "--gate-id",
        gate_id,
        "--actor",
        "operator",
    ]);
    let reservation_id = reservation["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    workspace.gate_raw(&[
        "run",
        "--card-id",
        card_id,
        "--gate-id",
        gate_id,
        "--reservation-id",
        reservation_id,
        "--actor",
        "operator",
    ])
}

#[test]
fn a_passing_gate_produces_a_receipt_bound_to_the_exact_commit() {
    let workspace = allocated();
    let envelope = workspace.gate_json(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    assert_eq!(envelope["data"]["passed"], true);
    assert_eq!(envelope["data"]["termination"], "completed");
    assert_eq!(envelope["data"]["exit_code"], 0);
    assert_eq!(envelope["data"]["attempt"], 1);
    assert_eq!(
        envelope["data"]["evaluated_sha"].as_str().unwrap(),
        workspace.authority_head(),
        "the receipt names the exact commit the gate ran against"
    );
    assert!(
        envelope["data"]["gate_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );

    let receipt_id = envelope["data"]["receipt_id"].as_str().unwrap();
    let receipt: Value = serde_json::from_str(&support::capture(
        &workspace.control,
        &["show", &format!("HEAD:receipts/{receipt_id}.json")],
    ))
    .unwrap();
    let provenance = &receipt["provenance"];
    assert_eq!(provenance["schema"], "harness.receipt-provenance/v1");
    assert_eq!(provenance["subject"]["kind"], "card");
    assert_eq!(
        provenance["subject"]["candidate_sha"],
        receipt["evaluated_sha"]
    );
    assert_eq!(provenance["subject"]["card_digest"], receipt["card_digest"]);
    assert!(
        provenance["argv_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(
        provenance["dimensions"]["environment"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(
        !serde_json::to_string(provenance)
            .unwrap()
            .contains("gate.unit"),
        "provenance records only digests of executable context, never the argv or raw output"
    );
}

#[test]
fn a_declared_junit_report_is_recorded_as_redacted_typed_counts() {
    let script = format!("printf '%s' '{VALID_JUNIT}' > report.xml");
    let workspace = junit_workspace("gate.junit", &script, "[report.xml]", 1);
    let receipt = workspace.gate_json(&["run", "--card-id", "F-001", "--gate-id", "gate.junit"]);

    assert_eq!(receipt["data"]["test_results"]["status"], "reported");
    assert_eq!(receipt["data"]["test_results"]["total"], 4);
    assert_eq!(receipt["data"]["test_results"]["passed"], 1);
    assert_eq!(receipt["data"]["test_results"]["failed"], 1);
    assert_eq!(receipt["data"]["test_results"]["errors"], 1);
    assert_eq!(receipt["data"]["test_results"]["skipped"], 1);
    assert!(
        !serde_json::to_string(&receipt)
            .unwrap()
            .contains("do not expose this")
    );

    let snapshot = snapshot_json(&workspace);
    let rendered = serde_json::to_string(&snapshot).unwrap();
    assert_eq!(snapshot["data"]["test_metrics"]["status"], "reported");
    assert_eq!(snapshot["data"]["test_metrics"]["total"], 4);
    assert!(!rendered.contains("do not expose this"));
    assert!(!rendered.contains(workspace.root.to_str().unwrap()));
}

#[test]
fn an_undeclared_report_is_not_reported_and_is_not_inferred_from_output() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "JUnit metrics",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);

    let snapshot = snapshot_json(&workspace);
    assert_eq!(snapshot["data"]["test_metrics"]["status"], "not_reported");
    assert_eq!(snapshot["data"]["test_metrics"]["total"], 0);
}

#[test]
fn malformed_and_inconsistent_declared_reports_fail_closed() {
    for (name, xml) in [
        ("gate.malformed", "<testsuite><testcase>"),
        (
            "gate.inconsistent",
            "<testsuite tests=\"2\" failures=\"0\" errors=\"0\" skipped=\"0\"><testcase/></testsuite>",
        ),
    ] {
        let script = format!("printf '%s' '{xml}' > report.xml");
        let workspace = junit_workspace(name, &script, "[report.xml]", 1);
        let output = reserved_run_raw(&workspace, "F-001", name);
        assert!(
            !output.status.success(),
            "{name} must refuse invalid evidence"
        );
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("JUnit report validation failed"), "{text}");
    }
}

#[test]
fn path_escape_and_duplicate_report_declarations_are_refused() {
    let workspace = Workspace::initialized();
    let escaped = workspace.gate_definition(
        "gate.escape",
        &junit_gate_definition("gate.escape", "true", "[../report.xml]", 1),
    );
    let escaped_output = workspace.gate_raw(&["register", "--definition", &escaped]);
    assert!(!escaped_output.status.success());

    let duplicate = workspace.gate_definition(
        "gate.duplicate",
        &junit_gate_definition("gate.duplicate", "true", "[report.xml, report.xml]", 1),
    );
    let duplicate_output = workspace.gate_raw(&["register", "--definition", &duplicate]);
    assert!(!duplicate_output.status.success());
}

#[cfg(unix)]
#[test]
fn symlinked_declared_report_is_refused() {
    let workspace = Workspace::initialized();
    let outside = workspace.root.join("outside.xml");
    fs::write(&outside, VALID_JUNIT).unwrap();
    let script = format!("ln -s '{}' report.xml", outside.display());
    let path = workspace.gate_definition(
        "gate.symlink",
        &junit_gate_definition("gate.symlink", &script, "[report.xml]", 1),
    );
    workspace.gate(&["register", "--definition", &path]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "JUnit symlink",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.symlink"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let output = reserved_run_raw(&workspace, "F-001", "gate.symlink");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("traverses a symlink"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn retry_attempts_are_aggregated_without_reusing_the_first_report() {
    let script = format!("printf '%s' '{VALID_JUNIT}' > report.xml");
    let workspace = junit_workspace("gate.retry-junit", &script, "[report.xml]", 2);
    let first =
        workspace.gate_json(&["run", "--card-id", "F-001", "--gate-id", "gate.retry-junit"]);
    assert_eq!(first["data"]["attempt"], 1);

    let receipt_path = fs::read_dir(workspace.control.join("receipts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .unwrap();
    let mut retry: Value =
        serde_json::from_str(&fs::read_to_string(&receipt_path).unwrap()).unwrap();
    retry["receipt_id"] = "R-000002".into();
    retry["attempt"] = 2.into();
    fs::write(
        workspace.control.join("receipts/R-000002.json"),
        serde_json::to_vec_pretty(&retry).unwrap(),
    )
    .unwrap();
    support::git(&workspace.control, &["add", "--all"]);
    support::git(
        &workspace.control,
        &["commit", "-q", "-m", "test: retry receipt"],
    );

    let snapshot = snapshot_json(&workspace);
    assert_eq!(snapshot["data"]["test_metrics"]["status"], "reported");
    assert_eq!(snapshot["data"]["test_metrics"]["total"], 8);
    assert_eq!(snapshot["data"]["gate_metrics"]["attempts"], 2);
}

#[test]
fn status_projects_the_exact_reuse_decision_from_a_frozen_request() {
    let workspace = allocated();
    workspace.gate_json(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    let preflight = workspace.gate_json(&["preflight", "--card-id", "F-001"]);
    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let receipt = status["data"]["receipts"][0].clone();
    let request = serde_json::json!({
        "plan": {
            "schema": preflight["data"]["schema"],
            "card_revision": preflight["data"]["card_revision"],
            "card_digest": preflight["data"]["card_digest"],
            "base_sha": preflight["data"]["base_sha"],
            "risk": preflight["data"]["risk"],
            "policy_digest": preflight["data"]["policy_digest"],
            "proof_map_digest": preflight["data"]["proof_map_digest"],
            "stages": preflight["data"]["stages"],
            "next_permitted_stage": preflight["data"]["next_permitted_stage"],
        },
        "stage": "narrow",
        "check": preflight["data"]["stages"][0]["checks"][0],
        "expected": receipt["provenance"],
    });
    let path = workspace.root.join("compatibility-request.json");
    fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();

    let projection = workspace.gate_json(&[
        "status",
        "--card-id",
        "F-001",
        "--compatibility-request",
        path.to_str().unwrap(),
    ]);
    assert_eq!(
        projection["data"]["receipt_compatibility"]["disposition"]["kind"], "rerun_required",
        "a receipt without the complete reusable-provenance extension must remain a truthful rerun requirement: {projection}"
    );
    assert_eq!(
        projection["data"]["receipt_compatibility"]["disposition"]["reasons"][0],
        "incomplete"
    );
}

#[test]
fn a_failing_gate_refuses_but_still_records_its_receipt() {
    let workspace = Workspace::initialized();
    workspace.register_gate("gate.fails", &["false"]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.fails"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let output = reserved_run_raw(&workspace, "F-001", "gate.fails");
    assert_eq!(output.status.code(), Some(7), "gate category is exit 7");
    assert_eq!(error_code(&output), "CH-GATE-FAILED");

    // Invariant 7.4.1: the failing attempt is still evidence.
    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let receipts = status["data"]["receipts"].as_array().unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["passed"], false);
    assert_eq!(receipts[0]["gate_id"], "gate.fails");
}

#[test]
fn a_timeout_is_recorded_distinctly_from_a_failure() {
    let workspace = Workspace::initialized();
    let path =
        workspace.gate_definition("slow", &definition("gate.slow", "[\"sleep\", \"30\"]", 1));
    workspace.gate(&["register", "--definition", &path]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.slow"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let output = reserved_run_raw(&workspace, "F-001", "gate.slow");
    assert_eq!(output.status.code(), Some(7));

    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let receipts = status["data"]["receipts"].as_array().unwrap();
    assert_eq!(
        receipts[0]["termination"], "timeout",
        "a timeout must be distinguishable from a clean non-zero exit"
    );
    assert_eq!(receipts[0]["passed"], false);
}

#[test]
fn a_settled_failed_reservation_cannot_silently_rerun_the_same_gate() {
    let workspace = Workspace::initialized();
    workspace.register_gate("gate.fails", &["false"]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.fails"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let first = reserved_run_raw(&workspace, "F-001", "gate.fails");
    assert_eq!(first.status.code(), Some(7));

    let reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.fails",
        "--actor",
        "operator",
    ]);
    assert_eq!(reservation["data"]["disposition"]["kind"], "settled");

    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let receipts = status["data"]["receipts"].as_array().unwrap();
    assert_eq!(receipts.len(), 1, "a settled reservation must not be rerun");
    let attempts: Vec<u64> = receipts
        .iter()
        .map(|receipt| receipt["attempt"].as_u64().unwrap())
        .collect();
    assert_eq!(attempts, vec![1]);
}

#[test]
fn a_pass_beyond_the_declared_attempts_is_not_acceptable_evidence() {
    let workspace = Workspace::initialized();
    workspace.register_gate("gate.fails", &["false"]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.fails"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let _ = reserved_run_raw(&workspace, "F-001", "gate.fails");

    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let gate = &status["data"]["feature_gates"][0];
    assert_eq!(gate["gate_id"], "gate.fails");
    assert_eq!(
        gate["satisfied"], false,
        "a gate with no passing attempt is not satisfied"
    );
}

#[test]
fn a_receipt_goes_stale_when_the_candidate_moves() {
    let workspace = allocated();
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    let before = workspace.gate_json(&["status", "--card-id", "F-001"]);
    assert_eq!(before["data"]["feature_gates"][0]["satisfied"], true);

    // Move the candidate. The receipt named the previous commit.
    let path = workspace.worktrees.join("F-001");
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add a.rs"]);

    let after = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let gate = &after["data"]["feature_gates"][0];
    assert_eq!(
        gate["satisfied"], false,
        "invariant 7.4.3: a receipt for an earlier SHA cannot be reused"
    );
    assert!(
        gate["stale_reason"]
            .as_str()
            .unwrap()
            .contains("candidate is now"),
        "{gate}"
    );
}

#[test]
fn a_receipt_goes_stale_when_the_gate_definition_changes() {
    let workspace = allocated();
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    assert_eq!(
        workspace.gate_json(&["status", "--card-id", "F-001"])["data"]["feature_gates"][0]["satisfied"],
        true
    );

    // Revise the gate. The receipt named the previous definition.
    let path = workspace.gate_definition(
        "unit-r2",
        &definition("gate.unit", "[\"true\", \"--strict\"]", 60)
            .replace("revision: 1", "revision: 2"),
    );
    workspace.gate(&["register", "--definition", &path]);

    let gate = &workspace.gate_json(&["status", "--card-id", "F-001"])["data"]["feature_gates"][0];
    assert_eq!(gate["satisfied"], false);
    assert!(
        gate["stale_reason"]
            .as_str()
            .unwrap()
            .contains("registry now holds"),
        "{gate}"
    );
}

#[test]
fn logs_are_written_outside_git_history_and_their_digests_recorded() {
    let workspace = Workspace::initialized();
    let path = workspace.gate_definition(
        "noisy",
        &definition(
            "gate.noisy",
            "[\"sh\", \"-c\", \"echo receipt-stdout; echo receipt-stderr >&2\"]",
            60,
        ),
    );
    workspace.gate(&["register", "--definition", &path]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.noisy"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let envelope = workspace.gate_json(&["run", "--card-id", "F-001", "--gate-id", "gate.noisy"]);
    let log_location = envelope["data"]["log_location"].as_str().unwrap();
    let stdout = fs::read(std::path::Path::new(log_location).join("stdout.log")).unwrap();
    let stderr = fs::read(std::path::Path::new(log_location).join("stderr.log")).unwrap();
    assert_eq!(stdout, b"receipt-stdout\n");
    assert_eq!(stderr, b"receipt-stderr\n");
    assert!(
        !stdout.is_empty() && !stderr.is_empty() && stdout != stderr,
        "the fixture must write distinct, non-empty evidence to both logs"
    );

    let expected_stdout_digest = Digest::of_bytes(&stdout);
    let expected_stderr_digest = Digest::of_bytes(&stderr);
    assert!(
        envelope["data"]["stdout_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"),
        "receipt digests must retain their stable algorithm prefix"
    );

    let receipt_id = envelope["data"]["receipt_id"].as_str().unwrap();
    let receipt_path = format!("receipts/{receipt_id}.json");
    let committed: Value = serde_json::from_str(&support::capture(
        &workspace.control,
        &["show", &format!("HEAD:{receipt_path}")],
    ))
    .unwrap();
    assert_eq!(
        committed, envelope["data"],
        "the command must report the exact receipt committed to control history"
    );
    assert_eq!(
        committed["stdout_digest"].as_str().unwrap(),
        expected_stdout_digest.as_str(),
        "the versioned receipt stdout digest must identify stdout.log"
    );
    assert_eq!(
        committed["stderr_digest"].as_str().unwrap(),
        expected_stderr_digest.as_str(),
        "the versioned receipt stderr digest must identify stderr.log"
    );
    assert_eq!(
        committed["log_location"].as_str().unwrap(),
        log_location,
        "the versioned receipt must point at the evidence whose digests it carries"
    );

    // Section 14.3: logs live outside Git; only their location and digest are
    // recorded in the versioned receipt.
    let tracked = workspace.control_tracked_files();
    assert!(
        !tracked.iter().any(|path| path.starts_with("logs/")),
        "logs must not enter control history: {tracked:?}"
    );
    assert!(tracked.contains(&receipt_path));
}

#[test]
fn gate_output_never_contaminates_the_json_envelope() {
    let workspace = Workspace::initialized();
    let path = workspace.gate_definition(
        "chatty",
        &definition(
            "gate.chatty",
            "[\"sh\", \"-c\", \"echo '{\\\"fake\\\": true}'\"]",
            60,
        ),
    );
    workspace.gate(&["register", "--definition", &path]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.chatty"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let output = workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.chatty"]);
    // stdout must parse as exactly one envelope; gate output goes to log files.
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("one clean envelope");
    assert_eq!(envelope["schema"], "harness.command-result/v1");
    assert!(envelope["data"]["fake"].is_null());
}

#[test]
fn an_unregistered_gate_cannot_be_run() {
    let workspace = allocated();
    let output = workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.absent"]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(error_code(&output), "CH-CONFIG-UNKNOWN-GATE");
}

#[test]
fn running_a_gate_without_an_allocation_is_a_precondition_failure() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);

    let output = workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    assert_eq!(output.status.code(), Some(4));
}

/// #105 acceptance: the refusal names the command that produces the missing
/// reservation. `gate`, unlike `gate_raw`, silently reserves on behalf of any
/// `run` fixture that omits `--reservation-id` (see tests/support/mod.rs), so
/// a test written against `gate` could never reach this refusal — only
/// `gate_raw` passes the arguments through untouched.
///
/// `message.contains("gate reserve")` alone would pass for any string
/// containing those two words, including one that names a flag `gate
/// reserve` does not have — it proves the command is *present*, not that it
/// *works*. The extraction-and-run below, the same mechanism
/// `tests/recovery_text.rs` uses for command references in `src/error.rs`,
/// proves the printed invocation is real: `clap` validates the subcommand
/// path and every flag before `--help` short-circuits, so a corrupted
/// subcommand or a corrupted flag both fail argument parsing, without
/// needing a control repository or any fixture state.
#[test]
fn gate_run_without_a_reservation_names_the_command_that_makes_one() {
    let workspace = allocated();
    let output = workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);
    assert_eq!(output.status.code(), Some(5), "policy category is exit 5");
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("gate reserve"),
        "the refusal must name the command that produces a reservation: {message}"
    );

    // Extract the exact printed invocation rather than hardcoding the
    // expected string — asserting the message equals a literal typed here
    // would only prove two copies of the same text match.
    let spans = backtick_spans(message);
    assert_eq!(
        spans.len(),
        1,
        "expected exactly one backticked command reference in the refusal: {message}"
    );
    let invocation = spans[0];
    let args: Vec<&str> = invocation.split_whitespace().collect();
    let help = std::process::Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(&args)
        .arg("--help")
        .output()
        .expect("the CLI binary should start");
    assert!(
        help.status.success(),
        "the refusal names `{invocation}`, which is not a real command shape (exit {:?}):\n\
         stdout: {}\nstderr: {}",
        help.status.code(),
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr),
    );
}

/// The no-false-positive counterpart: a properly reserved run must still
/// succeed. Without this, a change that refused every `gate run` (for
/// example, deleting the `reservation_id.is_none()` guard) would also pass
/// the test above.
#[test]
fn gate_run_with_a_reservation_is_unaffected() {
    let workspace = allocated();
    let output = reserved_run_raw(&workspace, "F-001", "gate.unit");
    assert!(
        output.status.success(),
        "a properly reserved run must still succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// #115: pins the #105 acceptance above against being deleted, quietly
/// repointed at the helping `gate` fixture, or left naming `gate_raw` while
/// no longer driving an unreserved `run` — see `tests/support/mod.rs`'s
/// "Plumbing versus a governed step" module doc for the rule this guards.
///
/// Reads this file's own source rather than re-running the scenario. Two
/// separate properties are pinned:
///
/// - *Which fixture method is named* — `gate_raw` (unhelped) versus `gate`
///   (silently reserves first). A behavioral check has nothing to observe
///   here: point the test at `gate` instead of `gate_raw` and it stops
///   reaching the refusal at all, it does not reach a *different* one, so
///   nothing downstream would notice.
/// - *What that call actually drives* — `run` as the first argument, with
///   no `--reservation-id` among the rest. Checking only the fixture name
///   is not enough: a body that keeps calling `gate_raw` but drives an
///   unrelated command (`gate_raw(&["list"])`), or a `run` that already
///   carries `--reservation-id`, would still satisfy "calls `gate_raw`, not
///   `gate`" while no longer exercising the un-helped path #2 asks for —
///   `gate_raw` only means "not silently reserved," not "reserved this
///   card, this gate, unreserved, right now."
///
/// What this does not pin: everything after the call. The exit code, error
/// code, and message assertions the test above makes could still be
/// weakened or deleted without this guard noticing — it checks the shape of
/// the invocation, not what the test does with the result.
#[test]
fn gate_run_without_a_reservation_test_still_drives_the_unhelped_fixture() {
    let source = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/gate_runner.rs"))
        .expect("gate_runner.rs should be readable at the repository root");

    // Built with `format!` rather than written out as one literal: this test
    // reads its own file, and a literal `"fn gate_run_..._makes_one()"` would
    // be a second, accidental occurrence of the very string it searches for
    // — right here — so deleting or renaming the real declaration below
    // would still leave a "match" for `source.find` to report, just the
    // wrong one, and this guard would silently stop guarding anything.
    let target_fn = "gate_run_without_a_reservation_names_the_command_that_makes_one";
    let marker = format!("fn {target_fn}()");
    let start = source.find(&marker).unwrap_or_else(|| {
        panic!(
            "{target_fn} must still exist: it is the only test that drives `gate run` \
             without the fixture's auto-reserve help (see tests/support/mod.rs's \
             \"Plumbing versus a governed step\" module doc)"
        )
    });

    let body_start = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .expect("a fn declaration is followed by a body");
    // Every top-level item in this rustfmt'd file closes at column zero, so
    // the first `}` alone on its own line after `body_start` is this
    // function's own closing brace — not a nested block (those are always
    // indented) and not the next item (a blank line, then a doc comment or
    // attribute, comes after it). That keeps the extracted text to exactly
    // this function, never leaking into the next one's doc comment.
    let body_end = source[body_start..]
        .find("\n}\n")
        .map(|offset| body_start + offset + "\n}".len())
        .expect("a fn body closes with a brace alone on its own line");
    let body = &source[body_start..body_end];

    assert!(
        body.contains(".gate_raw(&["),
        "{target_fn} must call `gate_raw`, the fixture that does not auto-reserve, or it \
         can never reach the refusal it exists to prove:\n{body}"
    );
    assert!(
        !body.contains(".gate(&["),
        "{target_fn} must not call the helping `gate` fixture — it silently reserves on \
         `run`'s behalf, so the refusal this test exists to prove would never fire:\n{body}"
    );

    // The two assertions above only confirm which fixture *method* is
    // named. A body that still calls `gate_raw` but drives an unrelated
    // command, or a `run` that already carries `--reservation-id`, would
    // pass both while no longer exercising the un-helped path #2 asks for.
    // Isolate the exact argument list `gate_raw` is called with — not just
    // its presence — the same way `Workspace::gate` itself decides whether
    // a `run` needs help: `args.first() == Some(&"run")` and no
    // `--reservation-id` among the rest (tests/support/mod.rs).
    let call_marker = ".gate_raw(&[";
    let call_start = body
        .find(call_marker)
        .map(|offset| offset + call_marker.len())
        .expect("already confirmed present by the assertion above");
    let call_end = body[call_start..]
        .find(']')
        .map(|offset| call_start + offset)
        .expect("gate_raw's argument list closes with `]`, and this call has no nested `[`");
    let call_args = &body[call_start..call_end];

    let first_arg = call_args.split(',').next().unwrap_or_default().trim();
    assert_eq!(
        first_arg, "\"run\"",
        "{target_fn} must call `gate_raw` with `run` as its first argument, not {first_arg} \
         — calling `gate_raw` alone only proves the call was not silently reserved, not that \
         it drove the invocation an operator gets refused for:\n{body}"
    );
    assert!(
        !call_args.contains("\"--reservation-id\""),
        "{target_fn} must not pass --reservation-id to `gate_raw` — a `run` that already \
         carries one is the reserved, helped path, not the omission an operator hits and \
         this test exists to prove:\n{body}"
    );
}

#[test]
fn a_dry_run_produces_no_receipt() {
    let workspace = allocated();
    let reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--actor",
        "operator",
    ]);
    let reservation_id = reservation["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    let before = workspace.control_head();

    let envelope = workspace.gate_json(&[
        "run",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--reservation-id",
        reservation_id,
        "--dry-run",
    ]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(workspace.control_head(), before);

    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    assert_eq!(status["data"]["receipts"].as_array().unwrap().len(), 0);
}

#[test]
fn the_run_is_recorded_as_an_event_with_its_verdict() {
    let workspace = allocated();
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    let ran = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "gate.ran")
        .expect("the run must be recorded");
    assert_eq!(ran["metadata"]["gate_id"], "gate.unit");
    assert_eq!(ran["metadata"]["passed"], true);
    assert_eq!(ran["metadata"]["termination"], "completed");
    assert_eq!(ran["metadata"]["attempt"], 1);
    assert_eq!(ran["head_sha"], workspace.authority_head());
}

#[test]
fn a_gate_that_passed_on_uncommitted_content_cannot_satisfy_a_handoff() {
    // Tier 1, defect 1. `gate run` resolved `evaluated_sha` from HEAD without
    // checking that the worktree matched HEAD, so a gate could pass on content
    // that was never committed and the receipt would bind the pass to a commit
    // whose own tree fails it. Write a file, do not commit it, let the gate
    // pass, delete it: a signed receipt asserts something untrue.
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.register_gate("gate.scratch", &["sh", "-c", "test -f scratch.txt"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.scratch"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let path = workspace.worktrees.join("F-001");
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add a.rs"]);

    fs::write(path.join("src/b.rs"), "fn other() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add b.rs"]);
    let head = support::capture(&path, &["rev-parse", "HEAD"]);

    fs::write(path.join("scratch.txt"), "never committed\n").unwrap();
    let reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.scratch",
        "--actor",
        "holder",
    ]);
    let reservation_id = reservation["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    let governed = workspace.gate_raw(&[
        "run",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.scratch",
        "--reservation-id",
        reservation_id,
        "--actor",
        "holder",
    ]);
    assert!(
        !governed.status.success(),
        "a governed exact-source run must not observe worktree-only scratch content"
    );
    assert_eq!(error_code(&governed), "CH-GATE-FAILED");
    let receipts = workspace.gate_json(&["status", "--card-id", "F-001"])["data"]["receipts"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["passed"], false);
    assert_eq!(receipts[0]["evaluated_sha"], head);
    assert_eq!(
        receipts[0]["worktree_clean"], false,
        "the receipt preserves the dirty candidate fact even though the governed source was exact"
    );

    // Remove the dirty fixture. The candidate has never contained scratch.txt,
    // and the failed governed receipt must remain unusable at handoff.
    fs::remove_file(path.join("scratch.txt")).unwrap();

    let body = format!(
        "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [kept it minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert the commit\n"
    );
    let declaration = workspace.root.join("declaration.yaml");
    fs::write(&declaration, body).unwrap();

    let output = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
    ]);
    assert!(
        !output.status.success(),
        "a receipt earned on uncommitted content must not satisfy a handoff: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-GATE-EVIDENCE-STALE");
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("no passing clean receipt"),
        "the refused exact-source run must not become usable gate evidence: {message}"
    );
}

#[test]
fn a_gate_that_leaves_a_process_holding_its_pipes_still_times_out() {
    // Tier 1, defect 5. The deadline bounded only the direct child. The reader
    // threads were joined afterwards, on the reasoning that "the pipes are
    // closed by then" — true of the child, false of anything it spawned that
    // inherited them. A gate that exits 0 immediately while leaving a
    // background process holding stdout blocked the runner for as long as that
    // process lived, and the overrun was then recorded as a clean pass, because
    // `timed_out` was only ever set by the wait loop.
    let workspace = Workspace::initialized();
    let path = workspace.gate_definition(
        "escapee",
        &definition("gate.escapee", "[\"sh\", \"-c\", \"sleep 30 & exit 0\"]", 1),
    );
    workspace.gate(&["register", "--definition", &path]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/**"], &["gate.escapee"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let started = std::time::Instant::now();
    let output = reserved_run_raw(&workspace, "F-001", "gate.escapee");
    let elapsed = started.elapsed();

    // The bound is the point. Without it this returns in about thirty seconds,
    // whatever the gate declared.
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "the timeout must bound the whole attempt, not just the direct child: took {elapsed:?}"
    );
    assert_eq!(output.status.code(), Some(7));

    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let receipts = status["data"]["receipts"].as_array().unwrap();
    assert_eq!(
        receipts[0]["termination"], "timeout",
        "an attempt that ran past its deadline is a timeout however the direct child exited"
    );
    assert_eq!(
        receipts[0]["passed"], false,
        "the direct child exited 0; that must not make an overrun a pass"
    );
}
