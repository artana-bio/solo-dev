//! The disposable worktree integration is assembled in.
//!
//! Section 13.2 keeps integration out of any worktree an actor uses. The
//! integration worktree is created for one integration, detached at the
//! authority baseline, and removed afterwards — so a failed merge leaves no
//! branch, no index, and no half-merged files behind for someone to trip over.

use std::path::{Path, PathBuf};

use crate::{
    error::{ErrorCode, HarnessError},
    git::command::{GitScope, run, run_ok},
};

/// Directory under the worktree root that holds integration worktrees.
///
/// Dot-prefixed so it cannot collide with a card identifier, which is what
/// every other directory under the worktree root is named for.
pub const INTEGRATION_WORKTREE_DIR: &str = ".integration";

/// Where an integration's disposable worktree lives.
#[must_use]
pub fn path_for(worktree_root: &Path, integration_id: &str) -> PathBuf {
    worktree_root
        .join(INTEGRATION_WORKTREE_DIR)
        .join(integration_id)
}

/// Creates a detached worktree at the given commit.
///
/// Detached rather than on a branch: an integration that fails must not leave a
/// named ref pointing at a partly merged state, because a named ref is
/// something a later command can find and mistake for a deliberate one.
///
/// # Errors
///
/// Returns a precondition error when the path is already occupied, or an
/// external-tool error when Git refuses.
pub fn create(repository: &Path, path: &Path, commit: &str) -> Result<(), HarnessError> {
    if path.exists() {
        return Err(HarnessError::Control {
            reason: format!(
                "integration worktree path already exists: {}",
                path.display()
            ),
            code: ErrorCode::PreconditionWorktreeExists,
        });
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| HarnessError::ControlIo {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    run_ok(
        &GitScope::work_tree(repository),
        [
            "worktree".as_ref(),
            "add".as_ref(),
            "--quiet".as_ref(),
            "--detach".as_ref(),
            path.as_os_str(),
            commit.as_ref(),
        ],
    )?;
    Ok(())
}

/// Removes an integration worktree and its registration.
///
/// Forcing is permitted here and nowhere else: invariant 7.2 forbids removing
/// a worktree that might hold work, and this one by construction holds only
/// what the harness put there in the last few seconds. Leaving a conflicted
/// merge behind would be worse — the next preparation would refuse the path.
///
/// # Errors
///
/// Returns an external-tool error when Git refuses to remove it.
pub fn remove(repository: &Path, path: &Path) -> Result<(), HarnessError> {
    if !path.exists() {
        return Ok(());
    }
    let scope = GitScope::work_tree(repository);
    run_ok(
        &scope,
        [
            "worktree".as_ref(),
            "remove".as_ref(),
            "--force".as_ref(),
            path.as_os_str(),
        ],
    )?;
    Ok(())
}

/// True when the worktree has no uncommitted or untracked content.
///
/// # Errors
///
/// Returns an external-tool error when Git cannot report status.
pub fn is_clean(path: &Path) -> Result<bool, HarnessError> {
    let output = run_ok(
        &GitScope::work_tree(path),
        ["status", "--porcelain", "--untracked-files=all"],
    )?;
    Ok(output.trimmed_stdout().is_empty())
}

/// Merges a commit into the worktree's head, refusing fast-forward.
///
/// `--no-ff` keeps every candidate's merge visible as its own commit even when
/// it could have been fast-forwarded, so the integration's shape matches the
/// plan that produced it rather than depending on merge order accidents.
///
/// # Errors
///
/// Returns a conflict error carrying Git's diagnostic when the merge does not
/// apply. The worktree is left for the caller to clean up, because the caller
/// knows whether the conflicting state is worth reporting first.
pub fn merge(path: &Path, commit: &str, message: &str) -> Result<String, HarnessError> {
    let scope = GitScope::work_tree(path);
    let output = run(
        &scope,
        ["merge", "--no-ff", "--no-edit", "-m", message, commit],
    )?;
    if !output.success() {
        return Err(HarnessError::Control {
            reason: format!("merging {commit} did not apply: {}", output.diagnostic()),
            code: ErrorCode::ConflictMergeFailed,
        });
    }
    let head = run_ok(&scope, ["rev-parse", "HEAD"])?;
    Ok(head.trimmed_stdout().to_owned())
}

/// Aborts an in-progress merge, leaving the worktree at its previous head.
pub fn abort_merge(path: &Path) {
    // Best effort: this runs on a failure path, and a merge that never started
    // makes `--abort` fail harmlessly.
    let _ = run(&GitScope::work_tree(path), ["merge", "--abort"]);
}

/// The tree an existing commit carries.
///
/// # Errors
///
/// Returns an external-tool error when the commit cannot be resolved.
pub fn tree_of(repository: &Path, commit: &str) -> Result<String, HarnessError> {
    let output = run_ok(
        &GitScope::work_tree(repository),
        ["rev-parse", &format!("{commit}^{{tree}}")],
    )?;
    Ok(output.trimmed_stdout().to_owned())
}

/// Creates a commit object from a tree and parents, moving no ref.
///
/// Used by the preflight to carry an in-memory merge forward: `merge-tree`
/// produces a tree, and merging the next candidate needs a commit. The commits
/// this writes are unreachable and collected by `git gc`; nothing points at
/// them, so writing one changes no state a reader can observe.
///
/// # Errors
///
/// Returns an external-tool error when Git refuses.
pub fn commit_tree(
    repository: &Path,
    tree: &str,
    parents: &[&str],
    message: &str,
) -> Result<String, HarnessError> {
    let mut argv = vec!["commit-tree".to_owned(), tree.to_owned()];
    for parent in parents {
        argv.push("-p".to_owned());
        argv.push((*parent).to_owned());
    }
    argv.push("-m".to_owned());
    argv.push(message.to_owned());

    let output = run_ok(&GitScope::work_tree(repository), argv)?;
    Ok(output.trimmed_stdout().to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A repository with one commit, returned with its base SHA.
    fn repository() -> (tempfile::TempDir, PathBuf, String) {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main", "."],
            vec!["config", "user.email", "f@local.invalid"],
            vec!["config", "user.name", "Fixture"],
        ] {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(&args)
                .output()
                .expect("git should run");
            assert!(output.status.success());
        }
        fs::write(path.join("f.txt"), "base\n").unwrap();
        let base = commit(&path, "base");
        (temp, path, base)
    }

    fn commit(path: &Path, message: &str) -> String {
        for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", message]] {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(&args)
                .output()
                .expect("git should run");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git should run");
        String::from_utf8_lossy(&head.stdout).trim().to_owned()
    }

    #[test]
    fn a_created_worktree_is_detached_and_clean() {
        let (temp, repo, head) = repository();
        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");

        create(&repo, &worktree, &head).expect("a worktree");
        assert!(worktree.exists());
        assert!(is_clean(&worktree).expect("a status"));

        // Detached: no branch name should resolve here.
        let branch = run(
            &GitScope::work_tree(&worktree),
            ["symbolic-ref", "--quiet", "HEAD"],
        )
        .expect("git should run");
        assert!(
            !branch.success(),
            "the integration worktree must not sit on a branch"
        );

        remove(&repo, &worktree).expect("removal");
        assert!(!worktree.exists());
    }

    #[test]
    fn creating_over_an_existing_path_is_refused() {
        let (temp, repo, head) = repository();
        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        fs::create_dir_all(&worktree).unwrap();

        let error = create(&repo, &worktree, &head).expect_err("an occupied path is refused");
        assert!(error.to_string().contains("already exists"), "{error}");
    }

    #[test]
    fn removing_an_absent_worktree_is_not_an_error() {
        let (temp, repo, _base) = repository();
        let worktree = path_for(&temp.path().join("worktrees"), "INT-404");
        remove(&repo, &worktree).expect("removing nothing succeeds");
    }

    #[test]
    fn a_merge_produces_a_new_head_and_leaves_the_worktree_clean() {
        let (temp, repo, base) = repository();

        // A candidate branch with one commit.
        let scope = GitScope::work_tree(&repo);
        run_ok(&scope, ["checkout", "-q", "-b", "card/F-001"]).expect("branch");
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        let candidate = commit(&repo, "feat: a");
        run_ok(&scope, ["checkout", "-q", "main"]).expect("back to main");

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        let head = merge(&worktree, &candidate, "integrate F-001").expect("a merge");

        assert_ne!(head, base, "the merge must advance the head");
        assert!(is_clean(&worktree).expect("a status"));
        assert!(worktree.join("a.txt").exists());

        remove(&repo, &worktree).expect("removal");
    }

    #[test]
    fn a_conflicting_merge_reports_a_conflict_and_can_be_aborted() {
        let (temp, repo, _base) = repository();
        let scope = GitScope::work_tree(&repo);

        run_ok(&scope, ["checkout", "-q", "-b", "card/F-001"]).expect("branch");
        fs::write(repo.join("f.txt"), "ours\n").unwrap();
        let candidate = commit(&repo, "feat: ours");
        run_ok(&scope, ["checkout", "-q", "main"]).expect("back to main");
        fs::write(repo.join("f.txt"), "theirs\n").unwrap();
        let diverged = commit(&repo, "feat: theirs");

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &diverged).expect("a worktree");
        let error = merge(&worktree, &candidate, "integrate F-001")
            .expect_err("a conflicting merge is refused");
        assert!(error.to_string().contains("did not apply"), "{error}");

        abort_merge(&worktree);
        assert!(
            is_clean(&worktree).expect("a status"),
            "aborting must leave nothing half-merged"
        );
        remove(&repo, &worktree).expect("removal");
    }

    #[test]
    fn commit_tree_writes_an_unreachable_commit() {
        let (_temp, repo, head) = repository();
        let tree = tree_of(&repo, &head).expect("a tree");

        let created = commit_tree(&repo, &tree, &[&head], "preflight step").expect("a commit");
        assert_eq!(created.len(), 40);
        assert_ne!(created, head);

        // Nothing points at it: no branch moved.
        let branches = run_ok(
            &GitScope::work_tree(&repo),
            ["branch", "--contains", &created],
        )
        .map(|output| output.trimmed_stdout().to_owned())
        .unwrap_or_default();
        assert!(
            branches.is_empty(),
            "a preflight commit must be unreachable, found: {branches}"
        );
    }
}
