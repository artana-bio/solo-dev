//! Mutation receipt cleanup and recovery boundaries.

mod support;

use std::process::Output;

use change_harness::domain::digest::Digest;
use serde_json::Value;
use support::Workspace;

fn mutation_args(workspace: &Workspace, receipt_id: &str, command: &[&str]) -> Vec<String> {
    let candidate_sha = workspace.authority_head();
    let mut args = vec![
        "mutation".to_owned(),
        "create".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--receipt-id".to_owned(),
        receipt_id.to_owned(),
        "--card-revision".to_owned(),
        "F-001-r1".to_owned(),
        "--candidate-sha".to_owned(),
        candidate_sha,
        "--reviewer-actor-id".to_owned(),
        "reviewer".to_owned(),
        "--reviewer-principal-id".to_owned(),
        "reviewer-principal".to_owned(),
        "--reviewer-session-id".to_owned(),
        "review-session".to_owned(),
        "--mutation-digest".to_owned(),
        Digest::of_bytes(b"declared-mutation").to_string(),
        "--patch-digest".to_owned(),
        Digest::of_bytes(b"declared-patch").to_string(),
        "--gate-oracle".to_owned(),
        "gate.mutation".to_owned(),
        "--expected-failure".to_owned(),
        "oracle must fail after mutation".to_owned(),
        "--observed-result".to_owned(),
        "probe".to_owned(),
        "--failed-at-oracle".to_owned(),
        "--restoration-proof".to_owned(),
        "restore disposable worktree".to_owned(),
    ];
    for token in command {
        args.push(format!("--command={token}"));
    }
    args
}

fn run_mutation(workspace: &Workspace, receipt_id: &str, command: &[&str]) -> Output {
    Workspace::run(&mutation_args(workspace, receipt_id, command))
}

fn assert_failed_partial_cleanup(workspace: &Workspace, output: &Output) {
    assert!(
        !output.status.success(),
        "mutation unexpectedly succeeded: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Workspace::run_json(&[
        "project".to_owned(),
        "status".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    let unresolved = status["data"]["unresolved_operations"]
        .as_array()
        .expect("unresolved operation list");
    assert_eq!(unresolved.len(), 1, "{status}");
    assert_eq!(unresolved[0]["command"], "mutation.create");
    let recovery = Workspace::run_json(&[
        "project".to_owned(),
        "recover".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert_eq!(
        recovery["data"]["recovery_required"],
        true,
        "{status}; command output: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let worktrees = support::capture(&workspace.repository, &["worktree", "list", "--porcelain"]);
    assert_eq!(
        worktrees.matches("worktree ").count(),
        1,
        "failed mutation must deregister its disposable worktree: {worktrees}"
    );
}

fn workspace_with_oracle(argv: &[&str]) -> Workspace {
    let workspace = Workspace::initialized();
    workspace.register_gate("gate.mutation", argv);
    workspace
}

#[test]
fn a_successful_mutation_receipt_is_committed_to_clean_control_history() {
    let workspace = workspace_with_oracle(&["sh", "-c", "test ! -f MUTATION_MARKER"]);
    let output = run_mutation(
        &workspace,
        "MR-COMMITTED",
        &["sh", "-c", "touch MUTATION_MARKER"],
    );
    assert!(
        output.status.success(),
        "mutation receipt creation failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let receipt = "mutation-receipts/MR-COMMITTED.json";
    assert!(workspace.control.join(receipt).is_file());
    assert!(
        workspace
            .control_tracked_files()
            .iter()
            .any(|path| path == receipt),
        "the authoritative receipt must be present in control Git history"
    );
    assert_eq!(
        support::capture(
            &workspace.control,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        ),
        "",
        "a successful mutation command must leave control state identical to control history"
    );
}

#[test]
fn a_mutation_receipt_identifier_cannot_escape_its_control_directory() {
    let workspace = Workspace::initialized();
    let project = workspace.control.join("project/project.json");
    let project_before = std::fs::read(&project).unwrap();
    let head_before = workspace.control_head();

    let output = run_mutation(&workspace, "../project/project", &["true"]);

    assert!(!output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-USAGE-INVALID-ID");
    assert_eq!(std::fs::read(project).unwrap(), project_before);
    assert_eq!(workspace.control_head(), head_before);
    assert_eq!(
        support::capture(
            &workspace.control,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        ),
        "",
        "identifier refusal must not mutate control state"
    );
}

#[test]
fn failed_mutation_process_cleans_up_and_retains_recovery_evidence() {
    let workspace = workspace_with_oracle(&["sh", "-c", "test ! -f MUTATION_MARKER"]);
    let output = run_mutation(
        &workspace,
        "MR-FAILED-COMMAND",
        &["sh", "-c", "touch MUTATION_MARKER; exit 7"],
    );
    assert_failed_partial_cleanup(&workspace, &output);
}

#[test]
fn passing_oracle_after_a_real_mutation_cleans_up_and_retains_recovery_evidence() {
    let workspace = workspace_with_oracle(&["true"]);
    let output = run_mutation(
        &workspace,
        "MR-ORACLE-PASSED",
        &["sh", "-c", "printf changed > README.md"],
    );
    assert_failed_partial_cleanup(&workspace, &output);
}

#[test]
fn no_tracked_patch_cleans_up_and_retains_recovery_evidence() {
    let workspace = workspace_with_oracle(&["sh", "-c", "test ! -f .git/mutation-marker"]);
    let output = run_mutation(&workspace, "MR-NO-PATCH", &["true"]);
    assert_failed_partial_cleanup(&workspace, &output);
}

#[allow(dead_code)]
fn _assert_json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("JSON error envelope")
}
