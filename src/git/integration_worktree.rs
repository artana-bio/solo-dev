//! The disposable worktree integration is assembled in.
//!
//! Section 13.2 keeps integration out of any worktree an actor uses. The
//! integration worktree is created for one integration, detached at the
//! authority baseline, and removed afterwards — so a failed merge leaves no
//! branch, no index, and no half-merged files behind for someone to trip over.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
};

use crate::{
    error::{ErrorCode, HarnessError},
    git::command::{
        GitScope, authoring_overrides, run, run_ok, run_with_config, run_with_config_ok,
        strip_ambient_overrides,
    },
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
///
/// # What this deliberately does not do
///
/// This runs the project's hooks, unlike [`merge`]. It materialises a working
/// tree, and `post-checkout` is what git-lfs and its kind install to finish
/// doing that; the smoke gates then judge this tree, so a tree the project's
/// own tooling never finished preparing would make the integration's verdict be
/// about the wrong bytes.
///
/// The cost is stated rather than hidden: `core.hooksPath` is all-or-nothing,
/// so keeping `post-checkout` also keeps `reference-transaction`, and a project
/// whose `reference-transaction` hook refuses cannot get an integration
/// worktree at all. That surfaces as `CH-EXTERNAL-GIT-COMMAND` carrying Git's
/// own "ref updates aborted by hook", which is at least accurate. Nobody has
/// been observed doing this; the alternative costs a real capability.
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
/// # Why this is not one `git merge`
///
/// This commit is authoritative: `landing::create` makes it the landing
/// commit's second parent, so it reaches the protected branch and stays there.
/// A single `git merge` composes the message and writes the commit itself, and
/// both of those are steerable by the project and by the operator's
/// `~/.gitconfig`. Observed on git 2.50.1: a `prepare-commit-msg` hook replaced
/// the harness's message with its own and the merge still reported success;
/// `merge.log=true` appended a shortlog the harness never wrote; a `commit-msg`
/// hook exiting non-zero and `commit.gpgsign=true` with an unusable signer each
/// stopped the integration dead, reported as a merge conflict; and
/// `commit.gpgsign=true` with a *working* signer put the operator's signature
/// on an object the harness authored.
///
/// So the porcelain does the part that is the project's — combining the
/// content, through the project's merge drivers and checkout filters — and
/// stops at `--no-commit`. The plumbing does the part that is the harness's:
/// `write-tree`, `commit-tree`, `update-ref`. `commit-tree` composes no message
/// and consults no hook, which is what makes every one of those routes
/// irrelevant by construction rather than by being individually disabled. The
/// alternative, a list of `-c` flags, was tried: it had two entries and there
/// turned out to be at least four.
///
/// What this deliberately does *not* suppress is [`run_post_merge_hook`] — see
/// there for the line between the two kinds of hook.
///
/// # Errors
///
/// Returns [`ErrorCode::ConflictMergeFailed`] when the two changes disagree,
/// and [`ErrorCode::PreconditionMergeRefused`] when Git would not begin the
/// merge at all. The worktree is left for the caller to clean up, because the
/// caller knows whether the conflicting state is worth reporting first.
pub fn merge(path: &Path, commit: &str, message: &str) -> Result<String, HarnessError> {
    let scope = GitScope::work_tree(path);
    let sink = hook_sink()?;
    let overrides = authoring_overrides(sink.path());

    let before = run_ok(&scope, ["rev-parse", "HEAD"])?
        .trimmed_stdout()
        .to_owned();
    // `--no-autostash` is not tidiness. `merge.autoStash = true` makes Git
    // stash a dirty worktree before merging and restore it afterwards, but
    // "afterwards" means after the *commit*, and there is no commit here:
    // verified on git 2.50.1, the stash is created, never reapplied, and the
    // following `reset` files it away as an ordinary stash entry. A smoke
    // gate's modification would vanish from the worktree and `is_clean` — the
    // check whose whole job is to notice that a merge left something behind —
    // would start passing. Refusing to autostash keeps the modification where
    // it is: harmless when it does not collide, and an accurate refusal when it
    // does.
    let output = run_with_config(
        &scope,
        &overrides,
        ["merge", "--no-ff", "--no-commit", "--no-autostash", commit],
    )?;
    // `MERGE_HEAD` is Git's own record of "a merge is under way", and it is the
    // only signal that separates the two failures. Verified across five states
    // on git 2.50.1: a content conflict leaves it set with unmerged index
    // entries (exit 1); an untracked file that would be overwritten, a
    // `merge.verifySignatures` refusal, and an unrelated-histories refusal all
    // leave it absent (exit 2 or 128) with a clean index. Classifying on the
    // index alone cannot tell the last three from "already up to date", which
    // is why this reads Git's state rather than counting unmerged paths.
    let merge_head = merge_head(&scope)?;
    if !output.success() {
        let (code, kind) = if merge_head.is_some() {
            (ErrorCode::ConflictMergeFailed, "did not apply")
        } else {
            (
                ErrorCode::PreconditionMergeRefused,
                "was refused before any content was combined",
            )
        };
        return Err(HarnessError::Control {
            reason: format!("merging {commit} {kind}: {}", output.diagnostic()),
            code,
        });
    }
    // `--no-ff` forces a merge commit in every case but one: a commit already
    // contained in HEAD produces "Already up to date.", exit 0, and no merge at
    // all. Exit zero read as a completed merge, so a candidate that contributed
    // nothing was recorded as combined and its card could be marked landed on
    // the strength of it — with no merge commit for the audit to find. A
    // candidate already in the tree is a planning error, and the coordinator is
    // the one who can resolve it.
    let Some(merge_head) = merge_head else {
        return Err(HarnessError::Control {
            reason: format!(
                "merging {commit} changed nothing: it is already contained in {before}, so this integration contributes nothing for it"
            ),
            code: ErrorCode::ConflictMergeFailed,
        });
    };

    let tree = run_with_config_ok(&scope, &overrides, ["write-tree"])?
        .trimmed_stdout()
        .to_owned();
    let head = run_with_config_ok(
        &scope,
        &overrides,
        [
            "commit-tree",
            &tree,
            "-p",
            &before,
            "-p",
            &merge_head,
            "-m",
            message,
        ],
    )?
    .trimmed_stdout()
    .to_owned();
    // Before the ref moves, not after: a refusal here must leave the worktree
    // where the caller's `abort_merge` can still put it back.
    verify_authored(&scope, &head, &before, &merge_head, message)?;
    // Compare-and-swap: nothing else should have moved this detached HEAD, and
    // if something did, the merge that produced `tree` was against a different
    // baseline than the one being recorded.
    run_with_config_ok(&scope, &overrides, ["update-ref", "HEAD", &head, &before])?;
    // Clears `MERGE_HEAD` and `MERGE_MSG`. The index already matches the new
    // HEAD, so this moves no content; without it the worktree stays "merging"
    // and the next `is_clean` check reads a state that is not there.
    run_with_config_ok(&scope, &overrides, ["reset", "-q"])?;

    run_post_merge_hook(path);
    Ok(head)
}

/// A private, empty directory to aim `core.hooksPath` at.
///
/// A path under the candidate's `.git` would work too, but the project owns
/// that directory, and "the project cannot alter what the harness authors"
/// should be true of the mechanism rather than only of its intent. This one is
/// created by the harness with a name nobody can predict and removed when the
/// merge returns.
fn hook_sink() -> Result<tempfile::TempDir, HarnessError> {
    tempfile::Builder::new()
        .prefix("change-harness-no-hooks-")
        .tempdir()
        .map_err(|source| HarnessError::ControlIo {
            path: std::env::temp_dir(),
            source,
        })
}

/// The commit being merged in, or `None` when no merge is under way.
fn merge_head(scope: &GitScope) -> Result<Option<String>, HarnessError> {
    let output = run(scope, ["rev-parse", "--quiet", "--verify", "MERGE_HEAD"])?;
    Ok(output
        .success()
        .then(|| output.trimmed_stdout().to_owned())
        .filter(|sha| !sha.is_empty()))
}

/// Reads back the object that was just written and refuses anything else.
///
/// This checks the object, not the flags that produced it. Asserting that the
/// harness passed `--no-commit` would only restate the implementation; reading
/// the commit catches a future Git that changes what `commit-tree` honours, and
/// it is the check that fails if this function's premise stops being true.
///
/// It compares the message the harness passed, not a subject line: the whole
/// message is the harness's, and a Git that appended anything to it — which is
/// exactly what `merge.log` does to a porcelain merge — must be caught rather
/// than tolerated.
fn verify_authored(
    scope: &GitScope,
    head: &str,
    first_parent: &str,
    second_parent: &str,
    message: &str,
) -> Result<(), HarnessError> {
    let raw = run_ok(scope, ["cat-file", "commit", head])?.stdout;
    let (headers, body) = raw.split_once("\n\n").unwrap_or((raw.as_str(), ""));

    let refuse = |reason: String| {
        Err(HarnessError::Control {
            reason,
            code: ErrorCode::InternalControlCorrupt,
        })
    };
    if headers.lines().any(|line| line.starts_with("gpgsig")) {
        return refuse(format!(
            "the integration merge commit {head} carries a signature the harness did not ask for"
        ));
    }
    if headers.lines().any(|line| line.starts_with("encoding ")) {
        return refuse(format!(
            "the integration merge commit {head} carries an encoding header the harness did not ask for"
        ));
    }
    let parents: Vec<&str> = headers
        .lines()
        .filter_map(|line| line.strip_prefix("parent "))
        .collect();
    if parents != [first_parent, second_parent] {
        return refuse(format!(
            "the integration merge commit {head} has parents {parents:?}, not {first_parent} and {second_parent}"
        ));
    }
    if body.trim_end_matches('\n') != message {
        return refuse(format!(
            "the integration merge commit {head} carries a message the harness did not write"
        ));
    }
    Ok(())
}

/// Runs the project's `post-merge` hook against the integration worktree.
///
/// The line this design draws: a hook that *materialises the working tree* the
/// smoke gates then read must keep running, and a hook that authors, vetoes or
/// observes a commit must not. `post-merge` is the first kind — it is what
/// git-lfs and similar tools install to finish bringing a tree up to date — and
/// the smoke gates run in this worktree between merges, so suppressing it would
/// have them judge bytes the project's own tooling never finished preparing.
/// Since the harness now commits with plumbing, Git no longer runs this hook,
/// so the harness runs it, deliberately and visibly.
///
/// Best effort by design. Git ignores this hook's exit status — verified: a
/// `post-merge` exiting 1 still leaves `git merge` at 0 — and a hook that could
/// not be found or executed is the ordinary case. What it cannot do is change
/// the commit: that object already exists and this cannot reach it.
fn run_post_merge_hook(worktree: &Path) {
    let scope = GitScope::work_tree(worktree);
    // `--git-path` resolves `core.hooksPath`, so a project that relocated its
    // hooks still gets them.
    let Ok(hooks) = run_ok(&scope, ["rev-parse", "--git-path", "hooks"]) else {
        return;
    };
    let hook = worktree.join(hooks.trimmed_stdout()).join("post-merge");
    if !hook.is_file() {
        return;
    }
    let Ok(git_dir) = run_ok(&scope, ["rev-parse", "--absolute-git-dir"]) else {
        return;
    };

    let mut command = std::process::Command::new(&hook);
    // Git passes one argument, the squash flag, and runs the hook from the top
    // of the worktree with `GIT_DIR` set. This is not squashing.
    command.arg("0");
    command.current_dir(worktree);
    strip_ambient_overrides(&mut command);
    command.env("GIT_DIR", OsString::from(git_dir.trimmed_stdout()));
    command.env("GIT_TERMINAL_PROMPT", "0");
    // Do not capture pipes: a hook can intentionally leave a child running,
    // and `output()` waits for that child to close inherited stdout/stderr.
    // Git does not wait for those children either.
    let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
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

    /// The same repository plus a divergent candidate branch holding `a.txt`.
    ///
    /// Returned as (temp, repository, base, candidate). `base` is where an
    /// integration worktree should be created; `candidate` is what to merge.
    fn repository_with_candidate() -> (tempfile::TempDir, PathBuf, String, String) {
        let (temp, repo, base) = repository();
        let scope = GitScope::work_tree(&repo);
        run_ok(&scope, ["checkout", "-q", "-b", "card/F-001"]).expect("branch");
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        let candidate = commit(&repo, "feat: a");
        run_ok(&scope, ["checkout", "-q", "main"]).expect("back to main");
        (temp, repo, base, candidate)
    }

    /// Sets a repository-local configuration value, bypassing the harness.
    fn configure(repo: &Path, key: &str, value: &str) {
        run_ok(&GitScope::work_tree(repo), ["config", key, value]).expect("git config");
    }

    /// Installs an executable hook in the repository's hook directory.
    fn hook(repo: &Path, name: &str, body: &str) {
        let dir = repo.join(".git").join("hooks");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        // 0o755. `set_readonly(false)` is not enough: Git skips a hook it
        // cannot execute, and a fixture whose hook never ran proves nothing.
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }

    /// The full message of a commit, as stored in the object.
    fn message_of(path: &Path, commit: &str) -> String {
        let raw = run_ok(&GitScope::work_tree(path), ["cat-file", "commit", commit])
            .expect("cat-file")
            .stdout;
        raw.split_once("\n\n")
            .map(|(_, body)| body.to_owned())
            .unwrap_or_default()
            .trim_end_matches('\n')
            .to_owned()
    }

    /// True when the commit object carries a `gpgsig` header.
    fn is_signed(path: &Path, commit: &str) -> bool {
        let raw = run_ok(&GitScope::work_tree(path), ["cat-file", "commit", commit])
            .expect("cat-file")
            .stdout;
        let headers = raw
            .split_once("\n\n")
            .map_or(raw.as_str(), |(head, _)| head);
        headers.lines().any(|line| line.starts_with("gpgsig"))
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
    fn merging_a_commit_already_contained_is_refused_rather_than_reported_merged() {
        // Tier 3, defect 17. `--no-ff` forces a merge commit except when the
        // commit is already an ancestor, where Git says "Already up to date.",
        // exits 0, and moves nothing. The caller read exit 0 as a completed
        // merge and recorded the candidate as combined, so a card could be
        // marked landed on the strength of a merge that published nothing and
        // left no merge commit for the audit to find.
        let (temp, repo, base) = repository();
        fs::write(repo.join("f.txt"), "second\n").unwrap();
        let head = commit(&repo, "second");
        assert_ne!(base, head, "the fixture needs two commits to be meaningful");

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &head).expect("a worktree");

        // `base` is an ancestor of `head`, so this merge is a no-op.
        let error = merge(&worktree, &base, "merge base")
            .expect_err("a merge that changes nothing must not report success");
        // Pinned against the classification added later: this refusal is on the
        // *success* branch — Git exited zero and simply did nothing — so it must
        // never be routed through the "Git refused to begin" arm, which reads
        // the same absent `MERGE_HEAD`.
        assert_eq!(
            error.code(),
            ErrorCode::ConflictMergeFailed,
            "a candidate already contained is a planning error, not a Git-environment one: {error}"
        );
        assert!(
            error.to_string().contains(&base),
            "the refusal must name the commit that contributed nothing: {error}"
        );

        // The guard: a commit that genuinely is not contained still merges, and
        // still produces the merge commit `--no-ff` is there to force.
        let branch_point = run_ok(&GitScope::work_tree(&repo), ["branch", "-f", "side", &base]);
        assert!(branch_point.is_ok());
        let side_work = temp.path().join("side");
        create(&repo, &side_work, &base).expect("a worktree");
        fs::write(side_work.join("g.txt"), "divergent\n").unwrap();
        let divergent = commit(&side_work, "divergent work");

        let merged = merge(&worktree, &divergent, "merge divergent").expect("a real merge");
        assert_ne!(merged, head, "the integration head must have moved");
        let parents = run_ok(
            &GitScope::work_tree(&worktree),
            ["rev-list", "--parents", "-n", "1", "HEAD"],
        )
        .expect("parents");
        assert_eq!(
            parents.trimmed_stdout().split_whitespace().count(),
            3,
            "a merge commit has two parents: {}",
            parents.trimmed_stdout()
        );
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
        // The string was this test's only assertion, and "did not apply" is
        // text a signing failure and a hook refusal produced too, so it held
        // against the unfixed code and would hold against a fix that
        // reclassified every merge failure as an environment problem. The code
        // is the contract, and the unmerged index entries are the evidence that
        // this really was two changes disagreeing.
        assert_eq!(
            error.code(),
            ErrorCode::ConflictMergeFailed,
            "a content conflict must stay a conflict: {error}"
        );
        assert!(error.to_string().contains("did not apply"), "{error}");
        let unmerged = run_ok(&GitScope::work_tree(&worktree), ["ls-files", "-u"])
            .expect("ls-files")
            .trimmed_stdout()
            .to_owned();
        assert!(
            !unmerged.is_empty(),
            "the fixture must actually have conflicted, not merely failed"
        );

        abort_merge(&worktree);
        assert!(
            is_clean(&worktree).expect("a status"),
            "aborting must leave nothing half-merged"
        );
        remove(&repo, &worktree).expect("removal");
    }

    #[test]
    fn an_integration_merge_carries_the_harness_message_whatever_the_project_configures() {
        // Two routes at once, because a fix that closes one and leaves the
        // other is the shape this repair already failed in. `prepare-commit-msg`
        // replaces the message outright; `merge.log` appends a shortlog. Both
        // alter an object that `landing::create` makes the landing commit's
        // second parent, so both reach the protected branch permanently.
        let (temp, repo, base, candidate) = repository_with_candidate();
        hook(
            &repo,
            "prepare-commit-msg",
            r#"echo "chore: message chosen by the hook" > "$1""#,
        );
        configure(&repo, "merge.log", "true");

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        let head = merge(&worktree, &candidate, "integrate F-001 into INT-001")
            .expect("the merge must succeed");

        assert_eq!(
            message_of(&worktree, &head),
            "integrate F-001 into INT-001",
            "the commit must carry exactly the message the harness passed"
        );
    }

    #[test]
    fn a_commit_stage_hook_cannot_block_an_integration_merge() {
        let (temp, repo, base, candidate) = repository_with_candidate();
        hook(
            &repo,
            "commit-msg",
            r#"echo "subject must reference a ticket" >&2; exit 1"#,
        );
        hook(&repo, "pre-merge-commit", "exit 1");

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        let head = merge(&worktree, &candidate, "integrate F-001 into INT-001")
            .expect("a project hook must not be able to veto an integration");

        assert_ne!(head, base, "the integration head must have moved");
    }

    #[test]
    fn an_integration_merge_is_never_signed_however_the_operator_configured_git() {
        let (temp, repo, base, candidate) = repository_with_candidate();

        // An SSH signing key, which needs no keyring and no agent.
        let key = temp.path().join("signing-key");
        let generated = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "fixture", "-f"])
            .arg(&key)
            .output()
            .expect("ssh-keygen should run");
        assert!(
            generated.status.success(),
            "the fixture needs a signing key: {}",
            String::from_utf8_lossy(&generated.stderr)
        );
        configure(&repo, "gpg.format", "ssh");
        configure(
            &repo,
            "user.signingkey",
            &key.with_extension("pub").display().to_string(),
        );
        configure(&repo, "commit.gpgsign", "true");

        // The guard that keeps this from passing vacuously: prove signing
        // actually works in this fixture before asserting the harness avoids
        // it. Without this, a host where SSH signing silently did nothing would
        // make the assertion below true against the unfixed code.
        run_ok(
            &GitScope::work_tree(&repo),
            [
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "signed by the operator",
            ],
        )
        .expect("a signed commit");
        let signed = run_ok(&GitScope::work_tree(&repo), ["rev-parse", "HEAD"])
            .expect("HEAD")
            .trimmed_stdout()
            .to_owned();
        assert!(
            is_signed(&repo, &signed),
            "the fixture must actually sign, or this test proves nothing"
        );

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        let head = merge(&worktree, &candidate, "integrate F-001 into INT-001")
            .expect("the merge must succeed");
        assert!(
            !is_signed(&worktree, &head),
            "an object the harness authored must not carry the operator's signature"
        );

        // The other half: a signer that cannot run must not stop the harness
        // either. On its own this case would pass against a fix that merely
        // swallowed signing errors, which is why it is not on its own.
        configure(&repo, "gpg.format", "openpgp");
        configure(&repo, "gpg.program", "/nonexistent/gpg");
        let second = path_for(&temp.path().join("worktrees"), "INT-002");
        create(&repo, &second, &base).expect("a worktree");
        merge(&second, &candidate, "integrate F-001 into INT-002")
            .expect("an unusable signer must not stop an integration");
    }

    #[test]
    fn a_signature_policy_on_the_repository_does_not_block_an_integration() {
        // `merge.verifySignatures` stops `git merge` before any content is
        // combined, so it survives every neutralisation aimed at the commit.
        // The harness decides what may be integrated from the plan and the
        // review record; D-006 and D-013 put candidate provenance outside this
        // tool's trust model, so this is not the check that would enforce it.
        let (temp, repo, base, candidate) = repository_with_candidate();
        configure(&repo, "merge.verifySignatures", "true");

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        merge(&worktree, &candidate, "integrate F-001 into INT-001")
            .expect("a signature policy must not stop an integration");
    }

    #[test]
    fn a_reference_transaction_hook_cannot_block_an_integration_merge() {
        // The hook the porcelain/plumbing split does *not* dispose of. Writing
        // the commit object never consults a hook, but moving the integration
        // head is a ref update, and `reference-transaction` can abort one
        // (verified: exit 128, "ref updates aborted by hook"). This is the only
        // thing `core.hooksPath` in `authoring_overrides` is still holding
        // shut, so it is the only thing that can prove that flag is carrying
        // weight rather than decorating the call.
        //
        // The hook is installed after `create`, and that ordering is a finding
        // rather than a convenience: `git worktree add` also writes a ref, and
        // it is deliberately left running the project's hooks so that
        // `post-checkout` still materialises the tree. `core.hooksPath` is
        // all-or-nothing, so there is no way to have one without the other
        // there. A project whose `reference-transaction` hook refuses cannot
        // have an integration worktree created at all — recorded at `create`.
        let (temp, repo, base, candidate) = repository_with_candidate();
        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        hook(&repo, "reference-transaction", "exit 1");
        let head = merge(&worktree, &candidate, "integrate F-001 into INT-001")
            .expect("a ref hook must not be able to veto an integration");

        assert_ne!(head, base, "the integration head must have moved");
    }

    #[test]
    fn a_merge_refused_before_any_content_is_combined_is_not_reported_as_a_conflict() {
        // Reachable in production: the smoke gates run inside this worktree
        // between merges, so a gate that drops a build artifact leaves exactly
        // this state for the next one. Calling it a conflict sends the operator
        // to `integration preflight`, which merges in memory and reports clean.
        let (temp, repo, base, candidate) = repository_with_candidate();
        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        fs::write(worktree.join("a.txt"), "left behind by a smoke gate\n").unwrap();

        let error = merge(&worktree, &candidate, "integrate F-001 into INT-001")
            .expect_err("Git will not overwrite an untracked file");

        assert_eq!(
            error.code(),
            ErrorCode::PreconditionMergeRefused,
            "nothing was combined, so this is not a conflict: {error}"
        );
        assert_eq!(
            run_ok(&GitScope::work_tree(&worktree), ["ls-files", "-u"])
                .expect("ls-files")
                .trimmed_stdout(),
            "",
            "the fixture must leave no unmerged entries, or it is testing a conflict"
        );
    }

    #[test]
    fn an_integration_merge_does_not_stash_the_worktree_out_from_under_the_cleanliness_check() {
        // Reachable the same way as the untracked-file case: the smoke gates
        // run in this worktree between merges. With `merge.autoStash` set and
        // no commit to restore after, Git stashes the gate's modification and
        // never brings it back — so the residue disappears and `is_clean`, the
        // check that exists to notice residue, starts reporting clean.
        let (temp, repo, base, candidate) = repository_with_candidate();
        fs::write(repo.join("g.txt"), "committed\n").unwrap();
        let base = {
            let scope = GitScope::work_tree(&repo);
            run_ok(&scope, ["add", "-A"]).expect("stage");
            run_ok(&scope, ["commit", "-q", "-m", "add g"]).expect("commit");
            let _ = base;
            run_ok(&scope, ["rev-parse", "HEAD"])
                .expect("HEAD")
                .trimmed_stdout()
                .to_owned()
        };
        configure(&repo, "merge.autoStash", "true");

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        fs::write(worktree.join("g.txt"), "left behind by a smoke gate\n").unwrap();

        merge(&worktree, &candidate, "integrate F-001 into INT-001").expect("a merge");

        assert_eq!(
            fs::read_to_string(worktree.join("g.txt")).unwrap(),
            "left behind by a smoke gate\n",
            "the gate's modification must still be in the worktree"
        );
        assert!(
            !is_clean(&worktree).expect("a status"),
            "residue must remain visible to the cleanliness check"
        );
        assert_eq!(
            run_ok(&GitScope::work_tree(&worktree), ["stash", "list"])
                .expect("stash list")
                .trimmed_stdout(),
            "",
            "nothing may be filed away into a stash entry"
        );
    }

    #[test]
    fn an_integration_merge_still_runs_the_projects_post_merge_hook() {
        // The guard on the opposite failure. Suppressing hooks wholesale would
        // also suppress this one, and `post-merge` is what git-lfs and its kind
        // install to finish materialising a tree — the same tree the smoke
        // gates then judge. The marker is written outside the worktree so the
        // test observes the hook rather than the residue it would leave.
        let (temp, repo, base, candidate) = repository_with_candidate();
        let marker = temp.path().join("post-merge-ran");
        hook(
            &repo,
            "post-merge",
            &format!("echo \"squash=$1\" > {}", marker.display()),
        );

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        merge(&worktree, &candidate, "integrate F-001 into INT-001").expect("a merge");

        assert_eq!(
            fs::read_to_string(&marker).unwrap_or_default().trim_end(),
            "squash=0",
            "the project's post-merge hook must still run, with Git's own argument"
        );
    }

    #[test]
    fn the_authored_object_is_checked_rather_than_the_flags_that_produced_it() {
        // `verify_authored` is a tripwire: on a correct implementation it never
        // fires, so nothing else in this file can observe it working. Driving
        // it directly is what keeps it from decaying into a comment. It is what
        // would catch a future Git in which `commit-tree` began honouring
        // `commit.gpgsign`, or in which a merge message picked up a shortlog.
        let (temp, repo, base, candidate) = repository_with_candidate();
        let scope = GitScope::work_tree(&repo);
        let tree = tree_of(&repo, &candidate).expect("a tree");
        let authored = commit_tree(&repo, &tree, &[&base, &candidate], "the harness message")
            .expect("a commit");

        verify_authored(&scope, &authored, &base, &candidate, "the harness message")
            .expect("an object the harness authored must be accepted");

        let wrong_message = verify_authored(&scope, &authored, &base, &candidate, "something else")
            .expect_err("a message the harness did not write must be refused");
        assert!(
            wrong_message.to_string().contains("message"),
            "{wrong_message}"
        );

        let wrong_parents =
            verify_authored(&scope, &authored, &candidate, &base, "the harness message")
                .expect_err("a shape other than base-then-candidate must be refused");
        assert!(
            wrong_parents.to_string().contains("parents"),
            "{wrong_parents}"
        );

        // And a signed object, built the only way that produces one here.
        let key = temp.path().join("signing-key");
        assert!(
            std::process::Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "fixture", "-f"])
                .arg(&key)
                .output()
                .expect("ssh-keygen should run")
                .status
                .success()
        );
        configure(&repo, "gpg.format", "ssh");
        configure(
            &repo,
            "user.signingkey",
            &key.with_extension("pub").display().to_string(),
        );
        let signed = run_ok(
            &scope,
            [
                "commit-tree",
                "-S",
                &tree,
                "-p",
                &base,
                "-p",
                &candidate,
                "-m",
                "the harness message",
            ],
        )
        .expect("a signed commit")
        .trimmed_stdout()
        .to_owned();
        assert!(
            is_signed(&repo, &signed),
            "the fixture must actually sign, or the assertion below proves nothing"
        );
        let error = verify_authored(&scope, &signed, &base, &candidate, "the harness message")
            .expect_err("a signature the harness did not ask for must be refused");
        assert!(error.to_string().contains("signature"), "{error}");

        let encoded = run_with_config_ok(
            &scope,
            &[OsString::from("i18n.commitEncoding=ISO-8859-1")],
            [
                "commit-tree",
                &tree,
                "-p",
                &base,
                "-p",
                &candidate,
                "-m",
                "the harness message",
            ],
        )
        .expect("an encoded commit")
        .trimmed_stdout()
        .to_owned();
        let error = verify_authored(&scope, &encoded, &base, &candidate, "the harness message")
            .expect_err("an encoding header the harness did not ask for must be refused");
        assert!(error.to_string().contains("encoding"), "{error}");
    }

    #[test]
    fn a_repository_setting_a_commit_encoding_still_integrates() {
        let (temp, repo, base, candidate) = repository_with_candidate();
        // The guard on `verify_authored`'s encoding refusal, and the reason that
        // refusal cannot ship without `i18n.commitEncoding` in
        // `authoring_overrides`. Without the override this fails 100% of the
        // time on any host where the setting exists — reported as
        // `InternalControlCorrupt`, which tells the operator the harness is
        // broken and points them at the wrong repository. A reviewer reproduced
        // it three ways: repository config, `~/.gitconfig`, and an `includeIf`
        // with no repository configuration at all.
        //
        // This is the fifth time in this project a fix would have refused every
        // valid case, and the second on this defect alone.
        crate::git::command::run_ok(
            &GitScope::work_tree(&repo),
            ["config", "i18n.commitEncoding", "ISO-8859-1"],
        )
        .expect("config");
        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        merge(&worktree, &candidate, "integrate F-001 into INT-001")
            .expect("a repository setting i18n.commitEncoding must still integrate");
    }

    #[test]
    fn post_merge_hook_children_do_not_delay_an_integration() {
        let (temp, repo, base, candidate) = repository_with_candidate();
        // `output()` holds capture pipes open until this child exits; `status()`
        // with null streams returns when the hook shell exits, as Git does.
        // The child sleeps far longer than any plausible merge so the margin is
        // wide. At `sleep 2` this test asserted 1s wall-clock and failed 1 run
        // in 7 on an unloaded host — a flaky assertion in this card's own
        // integration gate.
        hook(&repo, "post-merge", "sleep 30 &");
        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");

        let started = std::time::Instant::now();
        merge(&worktree, &candidate, "integrate F-001 into INT-001").expect("a merge");
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "a post-merge hook child must not delay the integration: took {elapsed:?}"
        );
    }

    #[test]
    fn worktree_creation_still_runs_the_projects_checkout_hooks() {
        // The guard that holds the line against moving neutrality into
        // `git::command::run`. This invocation materialises a working tree and
        // must keep the project's automation; only the invocations that author
        // an object are neutralised.
        let (temp, repo, base) = repository();
        let marker = temp.path().join("post-checkout-ran");
        hook(
            &repo,
            "post-checkout",
            &format!("touch {}", marker.display()),
        );

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");

        assert!(
            marker.exists(),
            "creating a worktree must still run the project's checkout hooks"
        );
    }

    #[test]
    fn neutrality_is_per_invocation_and_never_written_into_the_project() {
        // The third opposite failure: "fixing" the operator's machine. Turning
        // signing or hooks off in the developer's own repository would change
        // what their commits do, which is worse than the defect.
        let (temp, repo, base, candidate) = repository_with_candidate();
        configure(&repo, "commit.gpgsign", "true");
        hook(&repo, "post-merge", "true");
        let hooks_before = fs::read_to_string(repo.join(".git/hooks/post-merge")).unwrap();

        let worktree = path_for(&temp.path().join("worktrees"), "INT-001");
        create(&repo, &worktree, &base).expect("a worktree");
        merge(&worktree, &candidate, "integrate F-001 into INT-001").expect("a merge");

        let scope = GitScope::work_tree(&repo);
        assert_eq!(
            run_ok(&scope, ["config", "--local", "--get", "commit.gpgsign"])
                .expect("the value must still be there")
                .trimmed_stdout(),
            "true",
            "the developer's signing setting must be untouched"
        );
        assert!(
            !run(&scope, ["config", "--local", "--get", "core.hooksPath"])
                .expect("git should run")
                .success(),
            "the harness must not write core.hooksPath into the project"
        );
        assert_eq!(
            fs::read_to_string(repo.join(".git/hooks/post-merge")).unwrap(),
            hooks_before,
            "the project's hooks must be byte-identical afterwards"
        );
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
