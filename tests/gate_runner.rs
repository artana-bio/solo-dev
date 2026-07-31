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
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let output = workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.fails"]);
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
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let output = workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.slow"]);
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
fn every_attempt_is_recorded_and_numbered() {
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
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    for _ in 0..3 {
        let _ = workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.fails"]);
    }

    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let receipts = status["data"]["receipts"].as_array().unwrap();
    assert_eq!(receipts.len(), 3, "no attempt may be overwritten");
    let attempts: Vec<u64> = receipts
        .iter()
        .map(|receipt| receipt["attempt"].as_u64().unwrap())
        .collect();
    assert_eq!(attempts, vec![1, 2, 3]);
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

    let _ = workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.fails"]);

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
    workspace.activate_card("F-001", &["src/**"]);
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
    workspace.activate_card("F-001", &["src/**"]);
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

#[test]
fn a_dry_run_produces_no_receipt() {
    let workspace = allocated();
    let before = workspace.control_head();

    let envelope = workspace.gate_json(&[
        "run",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
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

    // The fixture has to discriminate or the rest of this proves nothing: the
    // gate must fail without the file and pass with it. This runs against an
    // earlier commit on purpose — spending an attempt against the candidate
    // itself would make the handoff refuse under the undeclared-retry rule of
    // Section 14.2, and the test would pass while proving nothing about
    // cleanliness. It did exactly that on the first draft.
    let without = workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.scratch"]);
    assert!(
        !without.status.success(),
        "the gate must depend on the uncommitted file, or this test is vacuous"
    );

    fs::write(path.join("src/b.rs"), "fn other() {}\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "feat: add b.rs"]);
    let head = support::capture(&path, &["rev-parse", "HEAD"]);

    fs::write(path.join("scratch.txt"), "never committed\n").unwrap();
    let with = workspace.gate_json(&["run", "--card-id", "F-001", "--gate-id", "gate.scratch"]);
    assert_eq!(with["data"]["passed"], true);
    assert_eq!(
        with["data"]["attempt"], 1,
        "this must be the first attempt against the candidate, so that a \
         refusal below is about the worktree and not about retries"
    );
    assert_eq!(
        with["data"]["evaluated_sha"], head,
        "the receipt names HEAD even though HEAD is not what ran"
    );
    assert_eq!(
        with["data"]["worktree_clean"], false,
        "the receipt must record that what ran was not the named commit"
    );

    // Remove it. The worktree is clean at HEAD again, and HEAD's tree has never
    // contained scratch.txt, so the gate fails against the commit the receipt
    // names.
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
        message.contains("uncommitted"),
        "the refusal must name the worktree, not a retry: {message}"
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
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let started = std::time::Instant::now();
    let output = workspace.gate_raw(&["run", "--card-id", "F-001", "--gate-id", "gate.escapee"]);
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
