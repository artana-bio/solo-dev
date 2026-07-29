//! `WP-400` acceptance: the bare authority repository.
//!
//! The authority owns the protected ref. These tests hold it to two promises:
//! `project init` establishes it without disturbing anything already there, and
//! nothing short of promotion moves the branch it protects.

mod support;

use std::{fs, process::Command};

use support::{Workspace, capture, git};

/// Runs `project init` against a workspace, returning the raw output.
fn init(workspace: &Workspace) -> std::process::Output {
    Workspace::run(&[
        "project".into(),
        "init".into(),
        "--project-id".into(),
        "example".into(),
        "--repository".into(),
        workspace.repository.display().to_string(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--authority".into(),
        workspace.authority.display().to_string(),
        "--worktree-root".into(),
        workspace.worktrees.display().to_string(),
    ])
}

/// Reads a config value from the authority repository.
fn authority_config(workspace: &Workspace, key: &str) -> String {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(&workspace.authority)
        .args(["config", "--get", key])
        .output()
        .expect("git should run");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Every ref the authority holds.
fn authority_refs(workspace: &Workspace) -> Vec<String> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(&workspace.authority)
        .args(["for-each-ref", "--format=%(refname)"])
        .output()
        .expect("git should run");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

#[test]
fn init_creates_a_bare_authority() {
    let workspace = Workspace::new();
    let output = init(&workspace);
    assert!(output.status.success());

    assert!(workspace.authority.exists(), "authority should exist");
    assert_eq!(authority_config(&workspace, "core.bare"), "true");
    assert!(
        !workspace.authority.join("index").exists(),
        "a bare repository has no index"
    );
}

#[test]
fn init_seeds_the_protected_branch_from_the_candidate() {
    let workspace = Workspace::new();
    let expected = workspace.candidate_head();
    assert!(init(&workspace).status.success());

    assert_eq!(
        workspace.authority_head(),
        expected,
        "the authority should start where the candidate's protected branch is"
    );
}

#[test]
fn init_registers_the_authority_remote_in_the_candidate() {
    let workspace = Workspace::new();
    assert!(init(&workspace).status.success());

    let url = capture(
        &workspace.repository,
        &["remote", "get-url", "harness-authority"],
    );
    assert_eq!(url, workspace.authority.display().to_string());
}

#[test]
fn init_leaves_no_staging_refs_behind() {
    let workspace = Workspace::new();
    assert!(init(&workspace).status.success());

    let refs = authority_refs(&workspace);
    assert_eq!(
        refs,
        vec!["refs/heads/main".to_owned()],
        "seeding uses a staging ref, which must not survive it"
    );
}

#[test]
fn init_adopts_an_existing_compatible_bare_repository() {
    let workspace = Workspace::new();
    fs::create_dir_all(&workspace.authority).unwrap();
    Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&workspace.authority)
        .output()
        .expect("git should run");
    // A marker config proves the repository was adopted rather than recreated.
    Command::new("git")
        .arg("--git-dir")
        .arg(&workspace.authority)
        .args(["config", "harness.fixture", "pre-existing"])
        .output()
        .expect("git should run");

    assert!(init(&workspace).status.success());
    assert_eq!(
        authority_config(&workspace, "harness.fixture"),
        "pre-existing",
        "an existing bare authority should be adopted, not replaced"
    );
    assert_eq!(workspace.authority_head(), workspace.candidate_head());
}

#[test]
fn init_refuses_a_path_holding_unrelated_content() {
    let workspace = Workspace::new();
    fs::create_dir_all(&workspace.authority).unwrap();
    fs::write(workspace.authority.join("notes.txt"), "someone's files\n").unwrap();

    // `project init` has no `--output` flag, so the assertion is on the exit
    // category and the human message rather than an error envelope.
    let output = init(&workspace);
    assert_eq!(
        output.status.code(),
        Some(3),
        "an unusable authority path is a configuration error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("is not a Git repository"),
        "unexpected error: {stderr}"
    );
    assert!(
        workspace.authority.join("notes.txt").exists(),
        "a refused initialization must not delete what it found"
    );
}

#[test]
fn init_refuses_an_authority_with_a_working_tree() {
    let workspace = Workspace::new();
    fs::create_dir_all(&workspace.authority).unwrap();
    git(&workspace.authority, &["init", "-q", "-b", "main"]);

    let output = init(&workspace);
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("has a working tree"),
        "unexpected error: {stderr}"
    );
}

#[test]
fn init_does_not_repoint_an_existing_remote() {
    let workspace = Workspace::new();
    let decoy = workspace.root.join("somewhere-else.git");
    Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&decoy)
        .output()
        .expect("git should run");
    git(
        &workspace.repository,
        &[
            "remote",
            "add",
            "harness-authority",
            &decoy.display().to_string(),
        ],
    );

    assert!(init(&workspace).status.success());
    let url = capture(
        &workspace.repository,
        &["remote", "get-url", "harness-authority"],
    );
    assert_eq!(
        url,
        decoy.display().to_string(),
        "initialization must not repoint a remote someone else configured"
    );
}

#[test]
fn rerunning_init_with_identical_state_changes_nothing() {
    let workspace = Workspace::new();
    assert!(init(&workspace).status.success());
    let head = workspace.authority_head();
    let refs = authority_refs(&workspace);

    let output = init(&workspace);
    assert!(
        output.status.success(),
        "a rerun with identical configuration is a no-op, not an error"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("already initialized"),
        "the rerun should say it did nothing"
    );
    assert_eq!(workspace.authority_head(), head);
    assert_eq!(authority_refs(&workspace), refs);
}

/// Runs `project status` and returns its envelope.
fn status(workspace: &Workspace) -> serde_json::Value {
    let output = Workspace::run(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        output.status.success(),
        "project status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("an envelope")
}

#[test]
fn status_reports_a_healthy_authority() {
    let workspace = Workspace::initialized();
    let authority = status(&workspace)["data"]["authority"].clone();

    assert_eq!(authority["bare"], true);
    assert_eq!(authority["protected_branch"], "main");
    assert_eq!(authority["protected_sha"], workspace.authority_head());
    assert_eq!(authority["remote"], "harness-authority");
    assert_eq!(authority["remote_matches"], true);
    assert_eq!(authority["diagnostic"], serde_json::Value::Null);
}

#[test]
fn status_reports_an_authority_that_has_gone_missing() {
    let workspace = Workspace::initialized();
    fs::remove_dir_all(&workspace.authority).unwrap();

    // Diagnosing a broken authority is precisely when the report is needed, so
    // the command must still succeed.
    let authority = status(&workspace)["data"]["authority"].clone();
    assert_eq!(authority["bare"], false);
    assert_eq!(authority["protected_sha"], serde_json::Value::Null);
    assert!(
        authority["diagnostic"].is_string(),
        "a missing authority must be described: {authority}"
    );
}

#[test]
fn status_reports_a_remote_that_points_somewhere_else() {
    let workspace = Workspace::initialized();
    let decoy = workspace.root.join("elsewhere.git");
    Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&decoy)
        .output()
        .expect("git should run");
    git(
        &workspace.repository,
        &[
            "remote",
            "set-url",
            "harness-authority",
            &decoy.display().to_string(),
        ],
    );

    let authority = status(&workspace)["data"]["authority"].clone();
    assert_eq!(
        authority["remote_matches"], false,
        "a repointed remote must be visible before it misdirects a promotion"
    );
    assert_eq!(authority["diagnostic"], serde_json::Value::Null);
}

#[test]
fn card_work_does_not_move_the_protected_branch() {
    let workspace = Workspace::initialized();
    let baseline = workspace.authority_head();

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

    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/lib.txt"), "work\n").unwrap();
    git(&worktree, &["add", "-A"]);
    git(&worktree, &["commit", "-q", "-m", "do the work"]);

    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    let delivered = capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {delivered}\nbehavior_delivered: adds lib.txt\n\
             implementation_decisions: [kept it minimal]\nassumptions: []\n\
             known_limitations: []\nresidual_risks: []\nrollback_notes: revert the commit\n"
        ),
    )
    .unwrap();
    workspace.handoff(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
    ]);

    assert_eq!(
        workspace.authority_head(),
        baseline,
        "only promotion may move the protected branch"
    );
}
