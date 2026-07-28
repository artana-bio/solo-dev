//! Bounded worktree and branch mutation.
//!
//! This is the first module that changes a candidate repository, so invariant
//! 7.2.3 governs it: no `reset --hard`, no force checkout, no force worktree
//! removal, no unconditional force push. Every operation here either succeeds
//! on its own terms or refuses, and refusing is always the safe outcome.
//!
//! Removal in particular is non-forcing. A worktree holding uncommitted work
//! must block its own removal, because the alternative is deleting something
//! nobody has a copy of.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::error::{ErrorCode, HarnessError};

use super::{
    command::{GitScope, run, run_ok},
    inspect,
};

/// The directory a worktree link lives in, relative to a worktree root.
pub const AGENT_DIR: &str = ".agent";

/// Exclude rule that keeps the harness link out of a candidate's diffs.
pub const AGENT_EXCLUDE_RULE: &str = ".agent/";

/// True when a local branch already exists.
///
/// # Errors
///
/// Returns an error when Git cannot be executed.
pub fn branch_exists(scope: &GitScope, branch: &str) -> Result<bool, HarnessError> {
    Ok(run(
        scope,
        [
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )?
    .success())
}

/// Creates a branch at an exact commit, refusing to move an existing one.
///
/// `git branch <name> <sha>` fails when the branch exists, which is the
/// behavior wanted: silently repointing a branch someone is working on would
/// discard their commits from that ref.
///
/// # Errors
///
/// Returns an error when the branch exists or the base is not a commit.
pub fn create_branch(scope: &GitScope, branch: &str, base_sha: &str) -> Result<(), HarnessError> {
    // Resolving first gives a precise diagnostic rather than Git's generic one.
    inspect::resolve_commit(scope, base_sha).map_err(|_| HarnessError::Control {
        reason: format!("base `{base_sha}` does not name a commit in this repository"),
        code: ErrorCode::PreconditionBaseMissing,
    })?;
    if branch_exists(scope, branch)? {
        return Err(HarnessError::Control {
            reason: format!("branch `{branch}` already exists"),
            code: ErrorCode::PreconditionBranchExists,
        });
    }
    run_ok(scope, ["branch", branch, base_sha])?;
    Ok(())
}

/// Deletes a branch without forcing.
///
/// Uses `-d`, not `-D`. Git refuses when the branch holds commits that are not
/// merged elsewhere, and that refusal is the point: unreachable commits are how
/// work is lost.
///
/// # Errors
///
/// Returns an error when Git refuses the deletion.
pub fn delete_branch(scope: &GitScope, branch: &str) -> Result<(), HarnessError> {
    let output = run(scope, ["branch", "-d", branch])?;
    if output.success() {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!(
            "refusing to delete branch `{branch}`: {}",
            output.diagnostic()
        ),
        code: ErrorCode::PreconditionUnmergedWork,
    })
}

/// Creates a linked worktree checked out on an existing branch.
///
/// # Errors
///
/// Returns an error when the path exists or Git refuses.
pub fn add_worktree(scope: &GitScope, path: &Path, branch: &str) -> Result<(), HarnessError> {
    if path.exists() {
        return Err(HarnessError::Control {
            reason: format!("worktree path already exists: {}", path.display()),
            code: ErrorCode::PreconditionWorktreeExists,
        });
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| HarnessError::ControlIo {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    run_ok(
        scope,
        [
            "worktree".as_ref(),
            "add".as_ref(),
            "--quiet".as_ref(),
            path.as_os_str(),
            branch.as_ref(),
        ],
    )?;
    Ok(())
}

/// Locks a worktree so `git worktree prune` cannot reclaim it.
///
/// # Errors
///
/// Returns an error when Git refuses.
pub fn lock_worktree(scope: &GitScope, path: &Path, reason: &str) -> Result<(), HarnessError> {
    run_ok(
        scope,
        [
            "worktree".as_ref(),
            "lock".as_ref(),
            "--reason".as_ref(),
            reason.as_ref(),
            path.as_os_str(),
        ],
    )?;
    Ok(())
}

/// Unlocks a worktree.
///
/// # Errors
///
/// Returns an error when Git refuses.
pub fn unlock_worktree(scope: &GitScope, path: &Path) -> Result<(), HarnessError> {
    run_ok(
        scope,
        ["worktree".as_ref(), "unlock".as_ref(), path.as_os_str()],
    )?;
    Ok(())
}

/// Removes a worktree without forcing.
///
/// # Errors
///
/// Returns an error when the worktree is dirty, locked, or Git otherwise
/// refuses.
pub fn remove_worktree(scope: &GitScope, path: &Path) -> Result<(), HarnessError> {
    let output = run(
        scope,
        ["worktree".as_ref(), "remove".as_ref(), path.as_os_str()],
    )?;
    if output.success() {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!(
            "refusing to remove worktree {}: {}",
            path.display(),
            output.diagnostic()
        ),
        code: ErrorCode::PreconditionWorktreeDirty,
    })
}

/// Adds an exclude rule to the repository's common exclude file if absent.
///
/// Idempotent by construction: the file is read first and left untouched when
/// an equivalent rule is present. Section 9.3 forbids editing the candidate's
/// committed `.gitignore` for this, because that would put harness bookkeeping
/// into the project's own history.
///
/// # Errors
///
/// Returns an error when the common directory cannot be resolved or written.
pub fn install_agent_exclude(scope: &GitScope) -> Result<bool, HarnessError> {
    let common = run_ok(
        scope,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let exclude_path = PathBuf::from(common.trimmed_stdout())
        .join("info")
        .join("exclude");

    let existing = fs::read_to_string(&exclude_path).unwrap_or_default();
    if existing
        .lines()
        .any(|line| line.trim() == AGENT_EXCLUDE_RULE || line.trim() == ".agent")
    {
        return Ok(false);
    }

    if let Some(parent) = exclude_path.parent() {
        fs::create_dir_all(parent).map_err(|source| HarnessError::ControlIo {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(AGENT_EXCLUDE_RULE);
    updated.push('\n');
    fs::write(&exclude_path, updated).map_err(|source| HarnessError::ControlIo {
        path: exclude_path,
        source,
    })?;
    Ok(true)
}

/// True when a path is registered as a worktree of this repository.
///
/// # Errors
///
/// Returns an error when Git cannot be executed.
pub fn is_registered(scope: &GitScope, path: &Path) -> Result<bool, HarnessError> {
    let entries = inspect::worktrees(scope)?;
    let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Ok(entries.iter().any(|entry| {
        entry
            .path
            .canonicalize()
            .unwrap_or_else(|_| entry.path.clone())
            == target
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "f@local.invalid"],
            vec!["config", "user.name", "F"],
        ] {
            Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(&args)
                .output()
                .unwrap();
        }
        fs::write(path.join("README.md"), "hello\n").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "initial"]] {
            Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(&args)
                .output()
                .unwrap();
        }
        (temp, path)
    }

    fn head(path: &Path) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn a_branch_is_created_at_the_exact_base() {
        let (_temp, path) = fixture();
        let scope = GitScope::work_tree(&path);
        let base = head(&path);

        create_branch(&scope, "card/F-001", &base).unwrap();
        assert!(branch_exists(&scope, "card/F-001").unwrap());
        assert_eq!(
            inspect::resolve_commit(&scope, "card/F-001").unwrap(),
            base,
            "the branch must start at exactly the requested commit"
        );
    }

    #[test]
    fn creating_an_existing_branch_is_refused_rather_than_repointing_it() {
        let (_temp, path) = fixture();
        let scope = GitScope::work_tree(&path);
        let base = head(&path);
        create_branch(&scope, "card/F-001", &base).unwrap();

        let error = create_branch(&scope, "card/F-001", &base).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PreconditionBranchExists);
    }

    #[test]
    fn a_base_that_is_not_a_commit_is_refused() {
        let (_temp, path) = fixture();
        let scope = GitScope::work_tree(&path);
        let error = create_branch(&scope, "card/F-001", &"a".repeat(40)).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PreconditionBaseMissing);
    }

    #[test]
    fn a_worktree_is_added_locked_and_removable_when_clean() {
        let (temp, path) = fixture();
        let scope = GitScope::work_tree(&path);
        let base = head(&path);
        let worktree = temp.path().join("worktrees/F-001");

        create_branch(&scope, "card/F-001", &base).unwrap();
        add_worktree(&scope, &worktree, "card/F-001").unwrap();
        assert!(worktree.join("README.md").exists());
        assert!(is_registered(&scope, &worktree).unwrap());

        lock_worktree(&scope, &worktree, "allocated to F-001").unwrap();
        let entry = inspect::worktrees(&scope)
            .unwrap()
            .into_iter()
            .find(|entry| entry.branch.as_deref() == Some("refs/heads/card/F-001"))
            .unwrap();
        assert!(entry.locked, "the worktree must be protected from pruning");

        unlock_worktree(&scope, &worktree).unwrap();
        remove_worktree(&scope, &worktree).unwrap();
        assert!(!worktree.exists());
    }

    #[test]
    fn adding_a_worktree_at_an_existing_path_is_refused() {
        let (temp, path) = fixture();
        let scope = GitScope::work_tree(&path);
        let base = head(&path);
        let occupied = temp.path().join("occupied");
        fs::create_dir_all(&occupied).unwrap();

        create_branch(&scope, "card/F-001", &base).unwrap();
        let error = add_worktree(&scope, &occupied, "card/F-001").expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PreconditionWorktreeExists);
    }

    #[test]
    fn a_dirty_worktree_refuses_removal() {
        let (temp, path) = fixture();
        let scope = GitScope::work_tree(&path);
        let base = head(&path);
        let worktree = temp.path().join("worktrees/F-001");

        create_branch(&scope, "card/F-001", &base).unwrap();
        add_worktree(&scope, &worktree, "card/F-001").unwrap();
        fs::write(worktree.join("uncommitted.txt"), "work in progress\n").unwrap();

        let error = remove_worktree(&scope, &worktree).expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PreconditionWorktreeDirty);
        assert!(
            worktree.join("uncommitted.txt").exists(),
            "the uncommitted file must survive the refusal"
        );
    }

    #[test]
    fn a_branch_with_unmerged_commits_refuses_deletion() {
        let (temp, path) = fixture();
        let scope = GitScope::work_tree(&path);
        let base = head(&path);
        let worktree = temp.path().join("worktrees/F-001");

        create_branch(&scope, "card/F-001", &base).unwrap();
        add_worktree(&scope, &worktree, "card/F-001").unwrap();
        fs::write(worktree.join("new.txt"), "work\n").unwrap();
        for args in [
            vec!["add", "-A"],
            vec!["commit", "-q", "-m", "unmerged work"],
        ] {
            Command::new("git")
                .arg("-C")
                .arg(&worktree)
                .args(&args)
                .output()
                .unwrap();
        }
        remove_worktree(&scope, &worktree).unwrap();

        let error = delete_branch(&scope, "card/F-001").expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::PreconditionUnmergedWork);
        assert!(
            branch_exists(&scope, "card/F-001").unwrap(),
            "the branch and its commits must survive"
        );
    }

    #[test]
    fn a_merged_branch_deletes_cleanly() {
        let (_temp, path) = fixture();
        let scope = GitScope::work_tree(&path);
        let base = head(&path);
        create_branch(&scope, "card/F-001", &base).unwrap();
        delete_branch(&scope, "card/F-001").expect("a branch at the base is fully merged");
        assert!(!branch_exists(&scope, "card/F-001").unwrap());
    }

    #[test]
    fn installing_the_exclude_rule_is_idempotent() {
        let (_temp, path) = fixture();
        let scope = GitScope::work_tree(&path);

        assert!(install_agent_exclude(&scope).unwrap(), "first call writes");
        assert!(
            !install_agent_exclude(&scope).unwrap(),
            "second call must be a no-op"
        );

        let exclude = fs::read_to_string(path.join(".git/info/exclude")).unwrap();
        assert_eq!(
            exclude.matches(AGENT_EXCLUDE_RULE).count(),
            1,
            "the rule must appear exactly once: {exclude}"
        );
    }

    #[test]
    fn the_exclude_rule_never_touches_the_committed_gitignore() {
        let (_temp, path) = fixture();
        let scope = GitScope::work_tree(&path);
        install_agent_exclude(&scope).unwrap();
        assert!(
            !path.join(".gitignore").exists(),
            "Section 9.3 forbids putting harness bookkeeping in project history"
        );
    }

    #[test]
    fn an_existing_equivalent_rule_is_respected() {
        let (_temp, path) = fixture();
        let scope = GitScope::work_tree(&path);
        fs::create_dir_all(path.join(".git/info")).unwrap();
        fs::write(path.join(".git/info/exclude"), "# comment\n.agent\n").unwrap();

        assert!(
            !install_agent_exclude(&scope).unwrap(),
            "`.agent` without a slash is equivalent and must be recognized"
        );
    }
}
