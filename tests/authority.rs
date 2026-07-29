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

#[test]
fn init_refuses_to_adopt_an_occupied_control_directory() {
    // Tier 2, defect 6. Section 9.1 requires initialization to refuse a
    // directory whose contents nobody checked. The protection was written for
    // the authority path and never for control: `init` adopted whatever was
    // there and overwrote its `.gitignore` with the control one, then reported
    // success. An operator who pointed `--control` at the wrong directory —
    // a notes folder, a checkout, their home directory — lost that file and was
    // told the project was initialized.
    let workspace = Workspace::new();
    let occupied = workspace.root.join("occupied");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("notes.txt"), "months of work\n").unwrap();
    fs::write(occupied.join(".gitignore"), "*.secret\n").unwrap();

    let output = Workspace::run(&[
        "project".into(),
        "init".into(),
        "--output".into(),
        "json".into(),
        "--project-id".into(),
        "example".into(),
        "--repository".into(),
        workspace.repository.display().to_string(),
        "--control".into(),
        occupied.display().to_string(),
        "--authority".into(),
        workspace.authority.display().to_string(),
        "--worktree-root".into(),
        workspace.worktrees.display().to_string(),
    ]);

    assert!(
        !output.status.success(),
        "init must refuse a directory it did not create: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        fs::read_to_string(occupied.join(".gitignore")).unwrap(),
        "*.secret\n",
        "the operator's ignore file must survive"
    );
    assert_eq!(
        fs::read_to_string(occupied.join("notes.txt")).unwrap(),
        "months of work\n"
    );
    assert!(
        !occupied.join(".git").exists(),
        "nothing should have been initialized in it"
    );
}

#[test]
fn init_still_accepts_an_empty_directory_that_already_exists() {
    // The guard on the fix above. Creating the directory first and then
    // initializing into it is ordinary — `mkdir -p` in a setup script, a mounted
    // volume — and refusing it would make the check a nuisance rather than a
    // protection.
    let workspace = Workspace::new();
    fs::create_dir_all(&workspace.control).unwrap();

    let output = Workspace::run(&[
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
    ]);
    assert!(
        output.status.success(),
        "an empty existing directory is fine: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_ambient_git_author_does_not_reach_control_history() {
    // Tier 3, defect 16, second half. Section 9.2 keeps workflow actor identity
    // in the authoritative event rather than in Git author configuration, so
    // control history stays byte-identical regardless of who ran the command —
    // and `initialize_git` sets that identity in the repository config.
    // Environment identity outranks repository config, so an exported
    // `GIT_AUTHOR_NAME` quietly undid it.
    let workspace = Workspace::new();
    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .env("GIT_AUTHOR_NAME", "Somebody Else")
        .env("GIT_AUTHOR_EMAIL", "else@example.invalid")
        .env("GIT_COMMITTER_NAME", "Somebody Else")
        .env("GIT_COMMITTER_EMAIL", "else@example.invalid")
        .args([
            "project".to_owned(),
            "init".to_owned(),
            "--project-id".to_owned(),
            "example".to_owned(),
            "--repository".to_owned(),
            workspace.repository.display().to_string(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
            "--authority".to_owned(),
            workspace.authority.display().to_string(),
            "--worktree-root".to_owned(),
            workspace.worktrees.display().to_string(),
        ])
        .output()
        .expect("the CLI should start");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let identity = capture(
        &workspace.control,
        &["log", "-1", "--format=%an <%ae>|%cn <%ce>"],
    );
    assert_eq!(
        identity.trim(),
        "Change Harness <change-harness@local.invalid>|Change Harness <change-harness@local.invalid>",
        "control history must not carry whoever's shell ran the command"
    );
}

#[test]
fn a_control_path_reaching_the_candidate_through_a_symlink_is_refused() {
    // Tier 3, defect 18. A path that does not exist yet is recorded
    // uncanonicalized, because there is nothing to canonicalize. The candidate
    // repository does exist and so is canonicalized — and the nesting and alias
    // checks then compare the two in different forms. A control path reaching
    // the candidate through a symlink slipped past both, and the control
    // repository, which Section 9.2 places deliberately outside any candidate
    // worktree so a candidate actor cannot rewrite the policy judging it, was
    // created inside one.
    let workspace = Workspace::new();
    let link = workspace.root.join("link");
    std::os::unix::fs::symlink(&workspace.repository, &link).expect("a symlink");

    let output = Workspace::run(&[
        "project".into(),
        "init".into(),
        "--output".into(),
        "json".into(),
        "--project-id".into(),
        "example".into(),
        "--repository".into(),
        workspace.repository.display().to_string(),
        "--control".into(),
        link.join("control").display().to_string(),
        "--authority".into(),
        workspace.authority.display().to_string(),
        "--worktree-root".into(),
        workspace.worktrees.display().to_string(),
    ]);

    assert!(
        !output.status.success(),
        "the control repository must not be created inside the candidate: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !workspace.repository.join("control").exists(),
        "and nothing may have been created there"
    );
}
