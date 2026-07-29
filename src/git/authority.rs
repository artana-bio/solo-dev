//! The bare authority repository: the canonical protected ref.
//!
//! D-005 and invariant 7.2.4. Promotion must never directly change a branch
//! that is checked out in a working tree, because Git would leave the index and
//! files describing a commit that is no longer HEAD. A separate bare repository
//! has no working tree, so its refs can be updated safely and every clone
//! learns about the change by fetching rather than by being surprised.
//!
//! `update-ref` with an expected old value is the whole mechanism. It is an
//! atomic compare-and-swap at the ref layer, which is what makes "promote only
//! if nobody else moved main" enforceable rather than hopeful.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{
    error::{ErrorCode, HarnessError},
    git::{
        command::{GitScope, run, run_ok},
        inspect,
    },
};

/// Prefix for the temporary refs objects are pushed to before promotion.
///
/// Objects must exist in the authority before its branch can point at them, but
/// transferring them must not touch the protected branch. A staging ref does
/// both: it carries the objects and is deleted afterwards.
pub const INCOMING_REF_PREFIX: &str = "refs/harness/incoming";

/// What an authority repository looks like right now.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AuthorityState {
    /// True when the path is a bare repository.
    pub bare: bool,
    /// The protected branch name.
    pub protected_branch: String,
    /// The commit the protected branch points at, if it exists.
    pub protected_sha: Option<String>,
    /// Every ref present, for detecting residue.
    pub refs: Vec<String>,
}

/// Inspects an authority repository without changing it.
///
/// # Errors
///
/// Returns a configuration error when the path is not a compatible bare
/// repository.
pub fn inspect_authority(
    authority: &Path,
    protected_branch: &str,
) -> Result<AuthorityState, HarnessError> {
    let class = inspect::classify(authority)?;
    match class {
        inspect::RepositoryClass::Repository { bare: true, .. } => {}
        inspect::RepositoryClass::Repository { bare: false, .. } => {
            return Err(HarnessError::Control {
                reason: format!(
                    "authority {} has a working tree; promotion into a checked-out branch would desynchronize its index and files",
                    authority.display()
                ),
                code: ErrorCode::ConfigAuthorityIncompatible,
            });
        }
        inspect::RepositoryClass::NotRepository => {
            return Err(HarnessError::Control {
                reason: format!("authority {} is not a Git repository", authority.display()),
                code: ErrorCode::ConfigAuthorityIncompatible,
            });
        }
        inspect::RepositoryClass::GitError { diagnostic } => {
            return Err(HarnessError::Control {
                reason: format!("Git refused to inspect the authority: {diagnostic}"),
                code: ErrorCode::ConfigAuthorityIncompatible,
            });
        }
    }

    let scope = GitScope::git_dir(authority);
    let protected = run(
        &scope,
        [
            "rev-parse",
            "--verify",
            &format!("refs/heads/{protected_branch}^{{commit}}"),
        ],
    )?;
    let refs = run_ok(&scope, ["for-each-ref", "--format=%(refname)"])?
        .trimmed_stdout()
        .lines()
        .map(ToOwned::to_owned)
        .collect();

    Ok(AuthorityState {
        bare: true,
        protected_branch: protected_branch.to_owned(),
        protected_sha: protected
            .success()
            .then(|| protected.trimmed_stdout().to_owned()),
        refs,
    })
}

/// Creates a bare authority repository, refusing to disturb existing content.
///
/// Section 9.1: an authority path that exists must be an empty directory or a
/// compatible bare repository. Anything else is refused rather than adopted,
/// because adopting a directory whose contents nobody checked is how a
/// promotion ends up writing into the wrong place.
///
/// # Errors
///
/// Returns a configuration error when the path exists and is unsuitable.
pub fn initialize(authority: &Path, protected_branch: &str) -> Result<bool, HarnessError> {
    if authority.exists() {
        let occupied = std::fs::read_dir(authority)
            .map_err(|source| HarnessError::ControlIo {
                path: authority.to_path_buf(),
                source,
            })?
            .next()
            .is_some();

        if occupied {
            // A compatible bare repository is adopted; anything else is refused
            // by `inspect_authority`, which is what keeps this non-destructive.
            inspect_authority(authority, protected_branch)?;
            return Ok(false);
        }
    }

    std::fs::create_dir_all(authority).map_err(|source| HarnessError::ControlIo {
        path: authority.to_path_buf(),
        source,
    })?;
    run_ok(
        &GitScope::None,
        [
            "init".as_ref(),
            "-q".as_ref(),
            "--bare".as_ref(),
            "-b".as_ref(),
            protected_branch.as_ref(),
            authority.as_os_str(),
        ],
    )?;
    Ok(true)
}

/// Transfers a commit's objects into the authority without touching any branch.
///
/// # Errors
///
/// Returns an error when the push fails.
pub fn stage_objects(
    candidate: &Path,
    authority: &Path,
    commit: &str,
    staging_id: &str,
) -> Result<String, HarnessError> {
    let incoming = format!("{INCOMING_REF_PREFIX}/{staging_id}");
    run_ok(
        &GitScope::work_tree(candidate),
        [
            "push".as_ref(),
            "--quiet".as_ref(),
            authority.as_os_str(),
            format!("{commit}:{incoming}").as_ref(),
        ],
    )?;
    Ok(incoming)
}

/// Removes a staging ref, leaving the authority with no residue.
pub fn unstage_objects(authority: &Path, incoming: &str) {
    // Best effort: a leftover staging ref is untidy but harmless, and failing a
    // completed promotion over cleanup would be worse.
    let _ = run(
        &GitScope::git_dir(authority),
        ["update-ref", "-d", incoming],
    );
}

/// Result of an attempted promotion.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum PromotionOutcome {
    /// The protected branch moved to the landing commit.
    Promoted {
        /// What it pointed at before.
        previous_sha: String,
        /// What it points at now.
        current_sha: String,
    },
    /// The authority had moved, so nothing changed.
    Rejected {
        /// What the caller expected.
        expected_sha: String,
        /// What was actually there.
        actual_sha: String,
    },
}

/// Moves the protected branch, but only if it still points where expected.
///
/// This is `update-ref <ref> <new> <old>`, an atomic compare-and-swap. A plain
/// push with `--force` would overwrite whatever arrived in the meantime, and a
/// push without force would refuse only non-fast-forwards, which is a weaker
/// condition than "nothing changed since I decided".
///
/// # Errors
///
/// Returns an error when Git cannot be executed. A rejected promotion is a
/// returned outcome, not an error, because the caller decides what it means.
pub fn promote(
    authority: &Path,
    protected_branch: &str,
    landing_sha: &str,
    expected_sha: &str,
) -> Result<PromotionOutcome, HarnessError> {
    let scope = GitScope::git_dir(authority);
    let reference = format!("refs/heads/{protected_branch}");

    let swap = run(
        &scope,
        ["update-ref", &reference, landing_sha, expected_sha],
    )?;

    if swap.success() {
        let current = inspect::resolve_commit(&scope, &reference)?;
        return Ok(PromotionOutcome::Promoted {
            previous_sha: expected_sha.to_owned(),
            current_sha: current,
        });
    }

    let actual = run(
        &scope,
        ["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )?;
    let actual_sha = if actual.success() {
        actual.trimmed_stdout().to_owned()
    } else {
        "unborn".to_owned()
    };

    // A failed swap was reported as a lost race whatever caused it, and Git's
    // diagnostic was discarded. If the ref still holds exactly what the caller
    // expected, no race was lost: something else stopped the write — a stale
    // `refs/heads/<branch>.lock`, a permissions problem, a damaged ref store —
    // and reporting "the baseline moved" sends the operator to re-plan an
    // integration that was never the problem. A stale lock file does not clear
    // itself, so that message would have repeated forever.
    if actual_sha == expected_sha {
        return Err(HarnessError::Control {
            reason: format!(
                "could not update `{reference}` even though it still holds the expected {expected_sha}: {}",
                swap.diagnostic()
            ),
            code: ErrorCode::ExternalGitCommand,
        });
    }

    Ok(PromotionOutcome::Rejected {
        expected_sha: expected_sha.to_owned(),
        actual_sha,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, process::Command};

    fn candidate_with_commit(root: &Path) -> (PathBuf, String) {
        let repo = root.join("candidate");
        fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "f@local.invalid"],
            vec!["config", "user.name", "F"],
        ] {
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(&args)
                .output()
                .unwrap();
        }
        fs::write(repo.join("README.md"), "hello\n").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "initial"]] {
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(&args)
                .output()
                .unwrap();
        }
        let head = String::from_utf8_lossy(
            &Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_owned();
        (repo, head)
    }

    #[test]
    fn initializing_creates_a_bare_repository() {
        let temp = tempfile::tempdir().unwrap();
        let authority = temp.path().join("authority.git");
        assert!(initialize(&authority, "main").unwrap());

        let state = inspect_authority(&authority, "main").unwrap();
        assert!(state.bare);
        assert!(state.protected_sha.is_none(), "no commits yet");
    }

    #[test]
    fn initializing_twice_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let authority = temp.path().join("authority.git");
        assert!(initialize(&authority, "main").unwrap());
        assert!(
            !initialize(&authority, "main").unwrap(),
            "a compatible authority is adopted, not recreated"
        );
    }

    #[test]
    fn an_unrelated_directory_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let occupied = temp.path().join("occupied");
        fs::create_dir_all(&occupied).unwrap();
        fs::write(occupied.join("someones-file.txt"), "important\n").unwrap();

        let error = initialize(&occupied, "main").expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::ConfigAuthorityIncompatible);
        assert!(
            occupied.join("someones-file.txt").exists(),
            "the refusal must not disturb existing content"
        );
    }

    #[test]
    fn a_non_bare_repository_is_refused_as_authority() {
        let temp = tempfile::tempdir().unwrap();
        let (candidate, _) = candidate_with_commit(temp.path());

        let error = inspect_authority(&candidate, "main").expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::ConfigAuthorityIncompatible);
        assert!(
            error.to_string().contains("desynchronize"),
            "the message must say why a working tree is the problem"
        );
    }

    #[test]
    fn staging_transfers_objects_without_moving_the_branch() {
        let temp = tempfile::tempdir().unwrap();
        let (candidate, head) = candidate_with_commit(temp.path());
        let authority = temp.path().join("authority.git");
        initialize(&authority, "main").unwrap();

        let incoming = stage_objects(&candidate, &authority, &head, "INT-001").unwrap();

        let state = inspect_authority(&authority, "main").unwrap();
        assert!(
            state.protected_sha.is_none(),
            "staging must not touch the protected branch"
        );
        assert!(state.refs.contains(&incoming), "{:?}", state.refs);

        unstage_objects(&authority, &incoming);
        let cleaned = inspect_authority(&authority, "main").unwrap();
        assert!(!cleaned.refs.contains(&incoming), "no residue may remain");
    }

    #[test]
    fn promotion_moves_the_branch_when_the_expectation_holds() {
        let temp = tempfile::tempdir().unwrap();
        let (candidate, first) = candidate_with_commit(temp.path());
        let authority = temp.path().join("authority.git");
        initialize(&authority, "main").unwrap();

        // Seed the authority with the first commit.
        let incoming = stage_objects(&candidate, &authority, &first, "seed").unwrap();
        run_ok(
            &GitScope::git_dir(&authority),
            ["update-ref", "refs/heads/main", &first],
        )
        .unwrap();
        unstage_objects(&authority, &incoming);

        // Land a second commit.
        fs::write(candidate.join("second.txt"), "second\n").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "second"]] {
            Command::new("git")
                .arg("-C")
                .arg(&candidate)
                .args(&args)
                .output()
                .unwrap();
        }
        let second = String::from_utf8_lossy(
            &Command::new("git")
                .arg("-C")
                .arg(&candidate)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_owned();

        let staged = stage_objects(&candidate, &authority, &second, "INT-001").unwrap();
        let outcome = promote(&authority, "main", &second, &first).unwrap();
        unstage_objects(&authority, &staged);

        assert_eq!(
            outcome,
            PromotionOutcome::Promoted {
                previous_sha: first.clone(),
                current_sha: second.clone(),
            }
        );
        assert_eq!(
            inspect_authority(&authority, "main").unwrap().protected_sha,
            Some(second)
        );
    }

    #[test]
    fn a_stale_expectation_is_rejected_and_the_branch_does_not_move() {
        let temp = tempfile::tempdir().unwrap();
        let (candidate, first) = candidate_with_commit(temp.path());
        let authority = temp.path().join("authority.git");
        initialize(&authority, "main").unwrap();

        let incoming = stage_objects(&candidate, &authority, &first, "seed").unwrap();
        run_ok(
            &GitScope::git_dir(&authority),
            ["update-ref", "refs/heads/main", &first],
        )
        .unwrap();
        unstage_objects(&authority, &incoming);

        fs::write(candidate.join("second.txt"), "second\n").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "second"]] {
            Command::new("git")
                .arg("-C")
                .arg(&candidate)
                .args(&args)
                .output()
                .unwrap();
        }
        let second = String::from_utf8_lossy(
            &Command::new("git")
                .arg("-C")
                .arg(&candidate)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_owned();
        let staged = stage_objects(&candidate, &authority, &second, "INT-001").unwrap();

        // Claim the authority is still unborn when it is actually at `first`.
        let outcome = promote(&authority, "main", &second, &"0".repeat(40)).unwrap();
        unstage_objects(&authority, &staged);

        assert_eq!(
            outcome,
            PromotionOutcome::Rejected {
                expected_sha: "0".repeat(40),
                actual_sha: first.clone(),
            }
        );
        assert_eq!(
            inspect_authority(&authority, "main").unwrap().protected_sha,
            Some(first),
            "a rejected promotion must leave the authority exactly as it was"
        );
    }

    #[test]
    fn a_write_that_failed_for_another_reason_is_not_reported_as_a_lost_race() {
        // Tier 3, defect 23. Any `update-ref` failure was reported as
        // `Rejected`, with Git's diagnostic discarded — so a stale
        // `refs/heads/main.lock` told the operator the baseline had moved when
        // it had not, and the record it produced said expected and actual were
        // the same commit. A lock file does not clear itself, so that message
        // would have repeated forever while the operator re-planned an
        // integration that was never the problem.
        let temp = tempfile::tempdir().unwrap();
        let (candidate, first) = candidate_with_commit(temp.path());
        let authority = temp.path().join("authority.git");
        initialize(&authority, "main").unwrap();

        let staged = stage_objects(&candidate, &authority, &first, "seed").unwrap();
        run_ok(
            &GitScope::git_dir(&authority),
            ["update-ref", "refs/heads/main", &first],
        )
        .unwrap();

        // The fixture: a lock file left behind by a process that died mid-write.
        let lock = authority.join("refs/heads/main.lock");
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(&lock, "").unwrap();

        let error = promote(&authority, "main", &first, &first)
            .expect_err("a write that failed for another reason is not a lost race");
        unstage_objects(&authority, &staged);
        let _ = fs::remove_file(&lock);

        let message = error.to_string();
        assert!(
            message.contains("still holds the expected"),
            "the message must say the baseline did not move: {message}"
        );
        assert!(
            message.contains("lock") || message.contains("unable"),
            "and must carry Git's own diagnostic: {message}"
        );
    }
}
