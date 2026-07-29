//! `WP-130` acceptance: every parser exercised against real Git fixtures.
//!
//! These use temporary repositories rather than recorded strings, because the
//! failures worth catching are the ones where Git's real output differs from
//! what a hand-written fixture assumed.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use change_harness::git::{
    command::{GitScope, run, run_ok},
    diff::{ChangeKind, diff_commits},
    inspect::{
        InProgressOperation, RepositoryClass, classify, in_progress_operations, is_ancestor,
        merge_base, object_type, resolve_commit, worktree_state, worktrees,
    },
};
use tempfile::TempDir;

/// Runs Git in a fixture, asserting success.
fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned()
}

/// Creates a repository with one commit and a deterministic identity.
fn repo_with_commit() -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().to_path_buf();
    git(&path, &["init", "-q", "-b", "main"]);
    git(&path, &["config", "user.email", "fixture@local.invalid"]);
    git(&path, &["config", "user.name", "Fixture"]);
    fs::write(path.join("README.md"), "hello\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "initial"]);
    (temp, path)
}

/// Creates an empty repository with a deterministic identity, no commits.
fn init_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "fixture@local.invalid"]);
    git(path, &["config", "user.name", "Fixture"]);
}

fn scope(path: &Path) -> GitScope {
    GitScope::work_tree(path)
}

#[test]
fn a_main_worktree_is_classified_with_its_top_level_and_git_dir() {
    let (_temp, path) = repo_with_commit();
    let class = classify(&path).unwrap();

    let RepositoryClass::Repository {
        top_level,
        git_dir,
        common_dir,
        bare,
        linked_worktree,
        detached_head,
    } = class
    else {
        panic!("expected a repository, got {class:?}");
    };
    assert!(top_level.is_some());
    assert_eq!(git_dir, common_dir, "the main worktree owns the common dir");
    assert!(!bare && !linked_worktree && !detached_head);
}

#[test]
fn a_linked_worktree_is_distinguished_from_the_main_one() {
    let (_temp, path) = repo_with_commit();
    let linked = path.join("linked");
    git(
        &path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    );

    let class = classify(&linked).unwrap();
    let RepositoryClass::Repository {
        linked_worktree,
        git_dir,
        common_dir,
        ..
    } = class
    else {
        panic!("expected a repository");
    };
    assert!(linked_worktree, "a linked worktree must be identified");
    assert_ne!(git_dir, common_dir);
}

#[test]
fn a_detached_head_is_identified() {
    let (_temp, path) = repo_with_commit();
    let head = git(&path, &["rev-parse", "HEAD"]);
    git(&path, &["checkout", "-q", "--detach", &head]);

    let class = classify(&path).unwrap();
    let RepositoryClass::Repository { detached_head, .. } = class else {
        panic!("expected a repository");
    };
    assert!(detached_head);
}

#[test]
fn a_bare_repository_is_identified_and_has_no_top_level() {
    let temp = tempfile::tempdir().unwrap();
    let bare_path = temp.path().join("authority.git");
    Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&bare_path)
        .output()
        .expect("git init --bare");

    let class = classify(&bare_path).unwrap();
    let RepositoryClass::Repository {
        bare, top_level, ..
    } = class
    else {
        panic!("expected a repository");
    };
    assert!(bare);
    assert!(top_level.is_none(), "a bare repository has no working tree");
}

#[test]
fn a_genuine_non_repository_is_reported_as_not_repository() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        classify(temp.path()).unwrap(),
        RepositoryClass::NotRepository
    );
}

#[test]
fn a_git_refusal_is_reported_as_git_error_and_never_as_absence() {
    // R-013 regression. A malformed gitfile makes Git refuse rather than
    // report absence; the foundation probe conflated the two by discarding
    // stderr, so a refusal read as "no repository here".
    let temp = tempfile::tempdir().unwrap();
    let broken = temp.path().join("broken");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join(".git"), "this is not a gitfile\n").unwrap();

    let class = classify(&broken).unwrap();
    let RepositoryClass::GitError { diagnostic } = class else {
        panic!("a Git refusal must not be reported as {class:?}");
    };
    assert!(
        diagnostic.contains("invalid gitfile format"),
        "the diagnostic must be preserved, got: {diagnostic}"
    );
    assert!(!diagnostic.contains('\n'), "diagnostics are single-line");
}

#[test]
fn paths_containing_spaces_and_unicode_are_inspected_correctly() {
    let temp = tempfile::tempdir().unwrap();
    let awkward = temp.path().join("a dir with spaces and ünïcodé — dash");
    fs::create_dir(&awkward).unwrap();
    git(&awkward, &["init", "-q", "-b", "main"]);
    git(&awkward, &["config", "user.email", "fixture@local.invalid"]);
    git(&awkward, &["config", "user.name", "Fixture"]);
    fs::write(awkward.join("file.txt"), "x\n").unwrap();
    git(&awkward, &["add", "-A"]);
    git(&awkward, &["commit", "-q", "-m", "initial"]);

    assert!(classify(&awkward).unwrap().is_repository());
    assert!(worktree_state(&scope(&awkward)).unwrap().clean);
}

#[test]
fn worktree_inventory_lists_the_main_and_linked_worktrees() {
    let (_temp, path) = repo_with_commit();
    let linked = path.join("linked");
    git(
        &path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    );

    let entries = worktrees(&scope(&path)).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(
        entries
            .iter()
            .any(|entry| entry.branch.as_deref() == Some("refs/heads/main"))
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.branch.as_deref() == Some("refs/heads/side"))
    );
    assert!(entries.iter().all(|entry| entry.head.is_some()));
}

#[test]
fn worktree_lock_state_is_reported() {
    let (_temp, path) = repo_with_commit();
    let linked = path.join("linked");
    git(
        &path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    );
    git(&path, &["worktree", "lock", linked.to_str().unwrap()]);

    let entries = worktrees(&scope(&path)).unwrap();
    let locked = entries
        .iter()
        .find(|entry| entry.branch.as_deref() == Some("refs/heads/side"))
        .expect("the linked worktree should be listed");
    assert!(locked.locked);
}

#[test]
fn clean_and_dirty_state_counts_untracked_files() {
    let (_temp, path) = repo_with_commit();
    assert!(worktree_state(&scope(&path)).unwrap().clean);

    fs::write(path.join("untracked.txt"), "new\n").unwrap();
    let state = worktree_state(&scope(&path)).unwrap();
    assert!(!state.clean, "an untracked file makes a worktree dirty");
    assert!(
        state
            .dirty_paths
            .iter()
            .any(|p| p.contains("untracked.txt"))
    );
}

#[test]
fn an_in_progress_merge_is_detected() {
    let (_temp, path) = repo_with_commit();
    fs::write(path.join("conflict.txt"), "base\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "base"]);

    git(&path, &["checkout", "-q", "-b", "left"]);
    fs::write(path.join("conflict.txt"), "left\n").unwrap();
    git(&path, &["commit", "-qam", "left"]);

    git(&path, &["checkout", "-q", "main"]);
    fs::write(path.join("conflict.txt"), "right\n").unwrap();
    git(&path, &["commit", "-qam", "right"]);

    // Expected to conflict, so failure here is the fixture working.
    let _ = Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["merge", "left"])
        .output()
        .unwrap();

    let operations = in_progress_operations(&scope(&path)).unwrap();
    assert!(
        operations.contains(&InProgressOperation::Merge),
        "expected a merge in progress, got {operations:?}"
    );
}

#[test]
fn a_settled_repository_reports_no_in_progress_operations() {
    let (_temp, path) = repo_with_commit();
    assert!(in_progress_operations(&scope(&path)).unwrap().is_empty());
}

#[test]
fn ancestry_and_merge_base_resolve_against_real_history() {
    let (_temp, path) = repo_with_commit();
    let first = git(&path, &["rev-parse", "HEAD"]);
    fs::write(path.join("second.txt"), "second\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "second"]);
    let second = git(&path, &["rev-parse", "HEAD"]);

    let scope = scope(&path);
    assert!(is_ancestor(&scope, &first, &second).unwrap());
    assert!(!is_ancestor(&scope, &second, &first).unwrap());
    assert_eq!(merge_base(&scope, &first, &second).unwrap(), first);
    assert_eq!(resolve_commit(&scope, "HEAD").unwrap(), second);
    assert_eq!(object_type(&scope, &second).unwrap(), "commit");
}

#[test]
fn resolving_a_missing_revision_fails_rather_than_guessing() {
    let (_temp, path) = repo_with_commit();
    assert!(resolve_commit(&scope(&path), "no-such-ref").is_err());
}

#[test]
fn diff_detects_renames_across_a_real_commit_pair() {
    let (_temp, path) = repo_with_commit();
    fs::write(
        path.join("original.txt"),
        "content that is long enough to match\n",
    )
    .unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "add original"]);
    let base = git(&path, &["rev-parse", "HEAD"]);

    git(&path, &["mv", "original.txt", "renamed.txt"]);
    git(&path, &["commit", "-qm", "rename"]);
    let head = git(&path, &["rev-parse", "HEAD"]);

    let summary = diff_commits(&scope(&path), &base, &head).unwrap();
    let change = summary
        .paths
        .iter()
        .find(|change| change.kind == ChangeKind::Renamed)
        .expect("a rename should be detected");
    assert_eq!(change.previous_path.as_deref(), Some("original.txt"));
    assert_eq!(change.path, "renamed.txt");
    // Both sides must be visible, or a rename could smuggle a change across an
    // ownership boundary.
    assert!(summary.touched_paths().contains("original.txt"));
    assert!(summary.touched_paths().contains("renamed.txt"));
}

#[test]
fn diff_detects_symlinks_executable_bits_and_gitlinks() {
    let (_temp, path) = repo_with_commit();
    let base = git(&path, &["rev-parse", "HEAD"]);

    std::os::unix::fs::symlink("README.md", path.join("link")).unwrap();
    fs::write(path.join("script.sh"), "#!/bin/sh\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "add link and script"]);
    let with_link = git(&path, &["rev-parse", "HEAD"]);

    let summary = diff_commits(&scope(&path), &base, &with_link).unwrap();
    assert_eq!(summary.symlink_changes().len(), 1, "{:?}", summary.paths);
    assert_eq!(summary.symlink_changes()[0].path, "link");

    // Flip the executable bit on an existing file.
    git(&path, &["update-index", "--chmod=+x", "script.sh"]);
    git(&path, &["commit", "-qm", "chmod"]);
    let with_exec = git(&path, &["rev-parse", "HEAD"]);
    let exec_summary = diff_commits(&scope(&path), &with_link, &with_exec).unwrap();
    assert!(
        exec_summary
            .paths
            .iter()
            .any(change_harness::git::diff::ChangedPath::changes_executable_bit),
        "{:?}",
        exec_summary.paths
    );

    // A gitlink entry, added directly so the fixture needs no real submodule.
    let commit_oid = git(&path, &["rev-parse", "HEAD"]);
    git(
        &path,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{commit_oid},vendor/dep"),
        ],
    );
    git(&path, &["commit", "-qm", "add gitlink"]);
    let with_gitlink = git(&path, &["rev-parse", "HEAD"]);
    let gitlink_summary = diff_commits(&scope(&path), &with_exec, &with_gitlink).unwrap();
    assert_eq!(gitlink_summary.submodule_changes().len(), 1);
    assert_eq!(gitlink_summary.submodule_changes()[0].path, "vendor/dep");
}

#[test]
fn diff_survives_paths_with_newlines_and_unicode() {
    let (_temp, path) = repo_with_commit();
    let base = git(&path, &["rev-parse", "HEAD"]);

    let awkward = path.join("na me\nwith\nnewlines.txt");
    fs::write(&awkward, "x\n").unwrap();
    fs::write(path.join("ünïcodé — dash.md"), "y\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "awkward paths"]);
    let head = git(&path, &["rev-parse", "HEAD"]);

    let summary = diff_commits(&scope(&path), &base, &head).unwrap();
    let touched = summary.touched_paths();
    assert!(
        touched.contains("na me\nwith\nnewlines.txt"),
        "NUL-delimited parsing must survive newlines in paths: {touched:?}"
    );
    assert!(touched.contains("ünïcodé — dash.md"), "{touched:?}");
}

#[test]
fn diff_detects_deletions() {
    let (_temp, path) = repo_with_commit();
    let base = git(&path, &["rev-parse", "HEAD"]);
    git(&path, &["rm", "-q", "README.md"]);
    git(&path, &["commit", "-qm", "remove readme"]);
    let head = git(&path, &["rev-parse", "HEAD"]);

    let summary = diff_commits(&scope(&path), &base, &head).unwrap();
    assert_eq!(summary.paths.len(), 1);
    assert_eq!(summary.paths[0].kind, ChangeKind::Deleted);
    assert_eq!(summary.paths[0].path, "README.md");
}

#[test]
fn inspection_performs_no_repository_mutation() {
    let (_temp, path) = repo_with_commit();
    let scope = scope(&path);
    let before_head = git(&path, &["rev-parse", "HEAD"]);
    let before_refs = git(
        &path,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );
    let before_status = git(&path, &["status", "--porcelain"]);
    let before_reflog = git(&path, &["reflog", "--format=%H %gs"]);

    // Exercise every read-only entry point.
    let _ = classify(&path).unwrap();
    let _ = worktrees(&scope).unwrap();
    let _ = worktree_state(&scope).unwrap();
    let _ = in_progress_operations(&scope).unwrap();
    let _ = resolve_commit(&scope, "HEAD").unwrap();
    let _ = object_type(&scope, &before_head).unwrap();
    let _ = is_ancestor(&scope, &before_head, &before_head).unwrap();
    let _ = diff_commits(&scope, &before_head, &before_head).unwrap();

    assert_eq!(git(&path, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(
        git(
            &path,
            &["for-each-ref", "--format=%(refname) %(objectname)"]
        ),
        before_refs
    );
    assert_eq!(git(&path, &["status", "--porcelain"]), before_status);
    assert_eq!(git(&path, &["reflog", "--format=%H %gs"]), before_reflog);
}

#[test]
fn a_failing_query_retains_its_diagnostic_instead_of_discarding_it() {
    let (_temp, path) = repo_with_commit();
    let output = run(&scope(&path), ["rev-parse", "--verify", "no-such-ref"]).unwrap();
    assert!(!output.success());
    assert!(!output.diagnostic().is_empty());
}

#[test]
fn successful_queries_expose_trimmed_stdout() {
    let (_temp, path) = repo_with_commit();
    let output = run_ok(&scope(&path), ["rev-parse", "HEAD"]).unwrap();
    assert_eq!(output.trimmed_stdout().len(), 40);
    assert!(!output.trimmed_stdout().ends_with('\n'));
}

#[test]
fn a_staged_rename_reports_both_real_paths() {
    // Tier 4. `git status --porcelain=v1 -z` emits TWO fields for a rename —
    // `R  <dest>\0<source>\0` — and the second is a bare path with no `XY `
    // prefix. The parser stripped three bytes from every field, so
    // `src/alpha.rs` was reported as `/alpha.rs`: a dirty path that exists
    // nowhere, in a report an actor is meant to act on.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    fs::create_dir_all(path.join("src")).unwrap();
    fs::write(path.join("src/alpha.rs"), "fn alpha() {}\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "add alpha"]);
    git(&path, &["mv", "src/alpha.rs", "src/beta.rs"]);

    let state = worktree_state(&scope(&path)).expect("a status");
    assert!(!state.clean);
    assert!(
        state.dirty_paths.contains(&"src/alpha.rs".to_owned()),
        "the rename source must be reported as itself: {:?}",
        state.dirty_paths
    );
    assert!(
        state.dirty_paths.contains(&"src/beta.rs".to_owned()),
        "and so must the destination: {:?}",
        state.dirty_paths
    );

    // The guard against a fix that merely stops inventing paths: every path
    // reported must be one that actually exists, on disk or in HEAD.
    for reported in &state.dirty_paths {
        let on_disk = path.join(reported).exists();
        let in_head = std::process::Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["cat-file", "-t", &format!("HEAD:{reported}")])
            .output()
            .expect("git should run")
            .status
            .success();
        assert!(on_disk || in_head, "{reported} exists nowhere");
    }
}

#[test]
fn a_rename_source_named_like_a_status_prefix_is_not_truncated() {
    // The test that kills a heuristic fix. "Strip three bytes only when the
    // field looks like it has an `XY ` prefix" passes every other test here and
    // silently truncates this one, because the file is literally named
    // `R  looks-like-status.txt`.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    fs::write(path.join("R  looks-like-status.txt"), "content\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "add oddly named file"]);
    git(&path, &["mv", "R  looks-like-status.txt", "renamed.txt"]);

    let state = worktree_state(&scope(&path)).expect("a status");
    assert!(
        state
            .dirty_paths
            .contains(&"R  looks-like-status.txt".to_owned()),
        "the source must be reported verbatim: {:?}",
        state.dirty_paths
    );
}

#[test]
fn the_report_does_not_depend_on_status_renames_config() {
    // `status.renames` is an operator setting. Whether a cleanliness report
    // names both sides of a rename must not turn on it.
    for setting in ["true", "false", "copies"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        init_repo(&path);
        git(&path, &["config", "status.renames", setting]);
        fs::write(path.join("alpha.rs"), "fn alpha() {}\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "-m", "add alpha"]);
        git(&path, &["mv", "alpha.rs", "beta.rs"]);

        let mut reported = worktree_state(&scope(&path)).expect("a status").dirty_paths;
        reported.sort();
        assert_eq!(
            reported,
            vec!["alpha.rs".to_owned(), "beta.rs".to_owned()],
            "status.renames={setting} changed the answer"
        );
    }
}

#[test]
fn a_rename_does_not_swallow_the_next_dirty_path() {
    // The guard against a lookahead fix. Consuming a second field on `R`/`C`
    // works until the record after a rename is an ordinary one, which it then
    // eats.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    fs::write(path.join("alpha.rs"), "fn alpha() {}\n").unwrap();
    fs::write(path.join("README.md"), "hello\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "base"]);
    git(&path, &["mv", "alpha.rs", "zeta.rs"]);
    fs::write(path.join("README.md"), "changed\n").unwrap();
    fs::write(path.join("untracked.txt"), "new\n").unwrap();

    let mut reported = worktree_state(&scope(&path)).expect("a status").dirty_paths;
    reported.sort();
    assert_eq!(
        reported,
        vec![
            "README.md".to_owned(),
            "alpha.rs".to_owned(),
            "untracked.txt".to_owned(),
            "zeta.rs".to_owned(),
        ]
    );
}

#[test]
fn an_untracked_file_inside_an_untracked_directory_is_named() {
    // `--untracked-files=all` was entirely unguarded: deleting it left the full
    // suite green, because every existing fixture writes its untracked file at
    // the repository root, where Git's default `-unormal` names it anyway. One
    // directory down the default collapses to `nested/` and the file is never
    // named — and an untracked file that no report names is exactly what
    // handoff refuses to let past.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    fs::write(path.join("seed.txt"), "seed\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "seed"]);
    fs::create_dir_all(path.join("nested")).unwrap();
    fs::write(path.join("nested/hidden.txt"), "hidden\n").unwrap();

    let state = worktree_state(&scope(&path)).expect("a status");
    assert_eq!(
        state.dirty_paths,
        vec!["nested/hidden.txt".to_owned()],
        "the file must be named, not collapsed to its directory"
    );
}

#[test]
fn a_conflicted_worktree_is_still_reportable() {
    // The shape most likely to blindside a strict status parser here: `handoff`
    // and `work verify` both run against a worktree that may be mid-merge, and
    // `UU`/`AA`/`DU` are what Git emits there. A parser that cannot answer
    // blocks the lifecycle.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    fs::write(path.join("shared.txt"), "base\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "base"]);
    git(&path, &["checkout", "-q", "-b", "other"]);
    fs::write(path.join("shared.txt"), "other\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "other"]);
    git(&path, &["checkout", "-q", "main"]);
    fs::write(path.join("shared.txt"), "main\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "main"]);
    let merge = std::process::Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["merge", "other"])
        .output()
        .expect("git should run");
    assert!(
        !merge.status.success(),
        "the fixture must actually conflict"
    );

    let state = worktree_state(&scope(&path)).expect("a conflicted worktree must still answer");
    assert!(!state.clean);
    assert!(state.dirty_paths.contains(&"shared.txt".to_owned()));
}
