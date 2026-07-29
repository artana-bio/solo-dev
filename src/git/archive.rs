//! Durable refs that keep landed work reachable after branches are removed.
//!
//! Section 9 gives archive refs one job: an ordinary branch can be deleted and
//! a worktree removed, and the commits they pointed at must still be findable.
//! Without them, cleanup would be indistinguishable from data loss — the
//! objects would survive only until the next garbage collection.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{
    error::{ErrorCode, HarnessError},
    git::command::{GitScope, run, run_ok},
};

/// Prefix for archived card candidates.
pub const CARD_ARCHIVE_PREFIX: &str = "refs/archive/cards";

/// Prefix for archived integration landings.
pub const INTEGRATION_ARCHIVE_PREFIX: &str = "refs/archive/integrations";

/// The archive ref for one card's landed candidate.
#[must_use]
pub fn card_ref(card_id: &str) -> String {
    format!("{CARD_ARCHIVE_PREFIX}/{card_id}")
}

/// The archive ref for one integration's landing commit.
#[must_use]
pub fn integration_ref(integration_id: &str) -> String {
    format!("{INTEGRATION_ARCHIVE_PREFIX}/{integration_id}")
}

/// One archived ref and what it should resolve to.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArchivedRef {
    /// The full ref name.
    pub reference: String,
    /// The commit it must resolve to.
    pub sha: String,
}

/// Why an archived ref does not check out.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum ArchiveDefect {
    /// The ref does not exist.
    Missing,
    /// The ref exists but points somewhere else.
    Moved(String),
    /// The commit itself is gone from the object database.
    Unreachable,
}

/// Creates or updates an archive ref.
///
/// # Errors
///
/// Returns an external-tool error when Git refuses.
pub fn create(repository: &Path, reference: &str, sha: &str) -> Result<(), HarnessError> {
    run_ok(
        &GitScope::work_tree(repository),
        ["update-ref", reference, sha],
    )?;
    Ok(())
}

/// Checks that an archived ref still resolves to its recorded commit.
///
/// Both halves matter. A ref that moved is as bad as one that vanished, and a
/// ref pointing at an object the database no longer holds is worse than either
/// — it looks intact until something tries to read it.
///
/// # Errors
///
/// Returns an external-tool error when Git cannot be executed.
pub fn verify(
    repository: &Path,
    archived: &ArchivedRef,
) -> Result<Option<ArchiveDefect>, HarnessError> {
    let scope = GitScope::work_tree(repository);
    let resolved = run(
        &scope,
        ["rev-parse", "--verify", "--quiet", &archived.reference],
    )?;
    if !resolved.success() {
        return Ok(Some(ArchiveDefect::Missing));
    }
    let actual = resolved.trimmed_stdout().to_owned();
    if actual != archived.sha {
        return Ok(Some(ArchiveDefect::Moved(actual)));
    }

    let object = run(
        &scope,
        ["cat-file", "-e", &format!("{}^{{commit}}", archived.sha)],
    )?;
    if object.success() {
        Ok(None)
    } else {
        Ok(Some(ArchiveDefect::Unreachable))
    }
}

/// Every commit on a branch that no other ref can reach.
///
/// # Errors
///
/// Returns an external-tool error when Git cannot be executed.
pub fn unarchived_commits(repository: &Path, branch: &str) -> Result<Vec<String>, HarnessError> {
    let scope = GitScope::work_tree(repository);
    let refs = run_ok(&scope, ["for-each-ref", "--format=%(refname)"])?;
    let others: Vec<String> = refs
        .trimmed_stdout()
        .lines()
        .filter(|reference| *reference != branch)
        .map(ToOwned::to_owned)
        .collect();

    let mut argv = vec!["rev-list".to_owned(), branch.to_owned(), "--not".to_owned()];
    argv.extend(others);
    let output = run(&scope, argv)?;
    if !output.success() {
        return Err(HarnessError::Control {
            reason: format!(
                "could not determine whether `{branch}` is fully archived: {}",
                output.diagnostic()
            ),
            code: ErrorCode::ExternalGitCommand,
        });
    }
    Ok(output
        .trimmed_stdout()
        .lines()
        .map(ToOwned::to_owned)
        .collect())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, process::Command};

    use super::*;

    fn repository() -> (tempfile::TempDir, PathBuf, String) {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main", "."],
            vec!["config", "user.email", "f@local.invalid"],
            vec!["config", "user.name", "Fixture"],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&path)
                    .args(&args)
                    .output()
                    .expect("git should run")
                    .status
                    .success()
            );
        }
        fs::write(path.join("f.txt"), "base\n").unwrap();
        let base = commit(&path, "base");
        (temp, path, base)
    }

    fn commit(path: &Path, message: &str) -> String {
        for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", message]] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(path)
                    .args(&args)
                    .output()
                    .expect("git should run")
                    .status
                    .success()
            );
        }
        let head = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git should run");
        String::from_utf8_lossy(&head.stdout).trim().to_owned()
    }

    fn git(repo: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .output()
                .expect("git should run")
                .status
                .success()
        );
    }

    #[test]
    fn an_intact_archive_ref_verifies() {
        let (_temp, repo, base) = repository();
        let archived = ArchivedRef {
            reference: card_ref("F-001"),
            sha: base.clone(),
        };
        create(&repo, &archived.reference, &base).expect("creation");
        assert_eq!(verify(&repo, &archived).expect("a verdict"), None);
    }

    #[test]
    fn a_missing_archive_ref_is_reported() {
        let (_temp, repo, base) = repository();
        let archived = ArchivedRef {
            reference: card_ref("F-404"),
            sha: base,
        };
        assert_eq!(
            verify(&repo, &archived).expect("a verdict"),
            Some(ArchiveDefect::Missing)
        );
    }

    #[test]
    fn an_archive_ref_that_moved_is_reported_with_where_it_went() {
        let (_temp, repo, base) = repository();
        fs::write(repo.join("g.txt"), "g\n").unwrap();
        let later = commit(&repo, "later");

        create(&repo, &card_ref("F-001"), &later).expect("creation");
        let archived = ArchivedRef {
            reference: card_ref("F-001"),
            sha: base,
        };
        assert_eq!(
            verify(&repo, &archived).expect("a verdict"),
            Some(ArchiveDefect::Moved(later))
        );
    }

    #[test]
    fn a_branch_whose_commits_exist_nowhere_else_is_not_archived() {
        let (_temp, repo, _base) = repository();
        git(&repo, &["checkout", "-q", "-b", "card/F-001"]);
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        let unique = commit(&repo, "unique work");
        git(&repo, &["checkout", "-q", "main"]);

        let outstanding =
            unarchived_commits(&repo, "refs/heads/card/F-001").expect("a commit list");
        assert_eq!(
            outstanding,
            vec![unique.clone()],
            "deleting this branch would lose that commit"
        );

        // Archiving it makes the branch safe to remove.
        create(&repo, &card_ref("F-001"), &unique).expect("creation");
        let outstanding =
            unarchived_commits(&repo, "refs/heads/card/F-001").expect("a commit list");
        assert!(outstanding.is_empty(), "unexpected: {outstanding:?}");
    }

    #[test]
    fn a_merged_branch_holds_nothing_unique() {
        let (_temp, repo, _base) = repository();
        git(&repo, &["checkout", "-q", "-b", "card/F-001"]);
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        commit(&repo, "work");
        git(&repo, &["checkout", "-q", "main"]);
        git(
            &repo,
            &["merge", "-q", "--no-ff", "-m", "merge", "card/F-001"],
        );

        let outstanding =
            unarchived_commits(&repo, "refs/heads/card/F-001").expect("a commit list");
        assert!(
            outstanding.is_empty(),
            "a merged branch is reachable from main: {outstanding:?}"
        );
    }
}
