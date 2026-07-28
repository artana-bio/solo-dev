//! `WP-240` acceptance: candidate verification against real repositories.

mod support;

use std::{fs, os::unix::fs::PermissionsExt as _};

use serde_json::Value;
use support::Workspace;

/// A workspace with an allocated card, ready to accept candidate commits.
fn allocated(include: &[&str]) -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", include);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace
}

/// The allocated worktree for F-001.
fn worktree(workspace: &Workspace) -> std::path::PathBuf {
    workspace.worktrees.join("F-001")
}

/// Commits a change inside the allocated worktree.
fn commit(workspace: &Workspace, message: &str) {
    let path = worktree(workspace);
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", message]);
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn an_in_scope_candidate_verifies() {
    let workspace = allocated(&["src/**"]);
    let path = worktree(&workspace);
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs").as_path(), "fn main() {}\n").unwrap();
    commit(&workspace, "feat: add a.rs");

    let envelope = workspace.work_json(&["verify", "--card-id", "F-001"]);
    assert_eq!(envelope["data"]["passed"], true);
    assert_eq!(
        envelope["data"]["changed_paths"].as_array().unwrap().len(),
        1
    );
    assert_eq!(envelope["data"]["card_revision"], 1);
}

#[test]
fn an_out_of_scope_modification_refuses_handoff() {
    let workspace = allocated(&["src/**"]);
    let path = worktree(&workspace);
    fs::write(path.join("README.md"), "edited outside scope\n").unwrap();
    commit(&workspace, "docs: edit readme");

    let output = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-CANDIDATE-OUT-OF-SCOPE");
}

#[test]
fn an_excluded_path_refuses_even_though_the_include_would_admit_it() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_excluding("F-001", &["src/**"], &["src/generated/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let path = worktree(&workspace);
    fs::create_dir_all(path.join("src/generated")).unwrap();
    fs::write(path.join("src/generated/api.rs"), "generated\n").unwrap();
    commit(&workspace, "feat: add generated api");

    let output = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("excluded"),
        "the refusal must say the path is excluded, not merely out of scope"
    );
}

#[test]
fn a_rename_out_of_scope_is_caught_on_the_source_side() {
    let workspace = allocated(&["src/**"]);
    let path = worktree(&workspace);
    fs::create_dir_all(path.join("src")).unwrap();

    // README.md exists at the baseline and is outside scope. Moving it into
    // src/ would look in-scope if only the destination were checked.
    support::git(&path, &["mv", "README.md", "src/readme.md"]);
    commit(&workspace, "refactor: move readme");

    let output = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert_eq!(
        output.status.code(),
        Some(5),
        "a rename must be checked on both sides"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("README.md"), "{stdout}");
}

#[test]
fn a_deletion_outside_scope_refuses() {
    let workspace = allocated(&["src/**"]);
    let path = worktree(&workspace);
    support::git(&path, &["rm", "-q", "README.md"]);
    commit(&workspace, "chore: remove readme");

    let output = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
}

#[test]
fn a_symlink_addition_refuses() {
    let workspace = allocated(&["src/**"]);
    let path = worktree(&workspace);
    fs::create_dir_all(path.join("src")).unwrap();
    std::os::unix::fs::symlink("../README.md", path.join("src/link")).unwrap();
    commit(&workspace, "feat: add link");

    let output = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&output.stdout).contains("symbolic link"));
}

#[test]
fn an_executable_bit_change_is_advisory_and_still_verifies() {
    // The mode change must be on a file that exists at the baseline. A file
    // added and chmod'd within the candidate appears in a base-to-candidate
    // diff as a single addition at mode 100755, not as a mode change.
    let workspace = allocated(&["README.md"]);
    let path = worktree(&workspace);

    let target = path.join("README.md");
    let mut permissions = fs::metadata(&target).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&target, permissions).unwrap();
    commit(&workspace, "chore: make readme executable");

    let envelope = workspace.work_json(&["verify", "--card-id", "F-001"]);
    assert_eq!(
        envelope["data"]["passed"], true,
        "{:?}",
        envelope["data"]["findings"]
    );
    let kinds: Vec<&str> = envelope["data"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"executable-bit-change"), "{kinds:?}");
}

#[test]
fn a_dirty_worktree_refuses_verification() {
    let workspace = allocated(&["src/**"]);
    let path = worktree(&workspace);
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    commit(&workspace, "feat: add a.rs");

    // Uncommitted work means the candidate commit is not what is on disk.
    fs::write(path.join("src/scratch.rs"), "not committed\n").unwrap();

    let output = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&output.stdout).contains("uncommitted"));
}

#[test]
fn a_cached_card_in_the_worktree_cannot_alter_verification() {
    let workspace = allocated(&["src/**"]);
    let path = worktree(&workspace);
    fs::write(path.join("README.md"), "edited outside scope\n").unwrap();

    // Plant a permissive card inside the worktree, exactly as a candidate actor
    // could. Verification reads control state and Git objects only.
    fs::create_dir_all(path.join(".agent")).unwrap();
    fs::write(
        path.join(".agent/card.json"),
        r#"{"write_scope":{"include":["**"],"exclude":[]}}"#,
    )
    .unwrap();
    commit(&workspace, "docs: edit readme");

    let output = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert_eq!(
        output.status.code(),
        Some(5),
        "a card cached in the worktree must not widen the real scope"
    );
}

#[test]
fn verification_is_identical_from_a_second_clean_clone() {
    let workspace = allocated(&["src/**"]);
    let path = worktree(&workspace);
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    commit(&workspace, "feat: add a.rs");

    let first = workspace.work_json(&["verify", "--card-id", "F-001"]);
    let second = workspace.work_json(&["verify", "--card-id", "F-001"]);
    assert_eq!(
        first["data"], second["data"],
        "verification must be a pure function of committed objects"
    );
}

#[test]
fn the_verdict_names_the_exact_card_digest_and_candidate() {
    let workspace = allocated(&["src/**"]);
    let path = worktree(&workspace);
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/a.rs"), "fn main() {}\n").unwrap();
    commit(&workspace, "feat: add a.rs");

    let envelope = workspace.work_json(&["verify", "--card-id", "F-001"]);
    let status = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(envelope["data"]["card_digest"], status["data"]["digest"]);
    assert_eq!(
        envelope["data"]["candidate_sha"].as_str().unwrap(),
        support::capture(&path, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        envelope["data"]["base_sha"].as_str().unwrap(),
        workspace.authority_head()
    );
}

#[test]
fn a_candidate_with_no_commits_verifies_trivially() {
    let workspace = allocated(&["src/**"]);
    let envelope = workspace.work_json(&["verify", "--card-id", "F-001"]);
    assert_eq!(envelope["data"]["passed"], true);
    assert_eq!(
        envelope["data"]["changed_paths"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn verification_without_a_lease_is_a_precondition_failure() {
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

    let output = workspace.work_raw(&["verify", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(4));
}
