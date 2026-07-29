//! The landing commit: what promotion will move the protected branch to.
//!
//! Section 13.5 fixes its shape — the authority baseline as first parent, the
//! integration head as second, and the exact verified integration tree. Built
//! with `commit-tree`, so creating it moves no branch: it exists as an object
//! and is held alive by a harness ref until acceptance decides its fate.
//!
//! That ref matters. "Unreachable from the protected branch" is the
//! requirement, not "unreachable"; an object nothing points at is a candidate
//! for garbage collection, and losing the landing commit between construction
//! and promotion would mean rebuilding it and re-verifying everything.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{
    error::{ErrorCode, HarnessError},
    git::command::{GitScope, run, run_ok},
};

/// Prefix for the refs that keep landing commits alive.
pub const LANDING_REF_PREFIX: &str = "refs/harness/landing";

/// Trailer key naming the integration.
pub const TRAILER_INTEGRATION: &str = "Change-Harness-Integration";
/// Trailer key naming the integration record's digest.
pub const TRAILER_INTEGRATION_DIGEST: &str = "Change-Harness-Integration-Digest";
/// Trailer key naming the cycle.
pub const TRAILER_CYCLE: &str = "Change-Harness-Cycle";
/// Trailer key naming one landed card and its exact candidate.
pub const TRAILER_CARD: &str = "Change-Harness-Card";
/// Trailer key naming one gate receipt the landing rests on.
pub const TRAILER_RECEIPT: &str = "Change-Harness-Receipt";

/// The ref that holds one integration's landing commit.
#[must_use]
pub fn landing_ref(integration_id: &str) -> String {
    format!("{LANDING_REF_PREFIX}/{integration_id}")
}

/// What a landing commit actually is, read back from Git.
///
/// Read from the object rather than remembered from construction: promotion
/// has to verify the commit it is about to publish, and verifying it against
/// what the harness intended to build proves nothing.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LandingObject {
    /// The commit's own SHA.
    pub sha: String,
    /// The tree it carries.
    pub tree: String,
    /// Its parents, in order.
    pub parents: Vec<String>,
    /// Its subject line.
    pub subject: String,
    /// Its trailers, in the order they appear.
    pub trailers: Vec<(String, String)>,
}

impl LandingObject {
    /// The first parent, which promotion compares to the authority branch.
    #[must_use]
    pub fn first_parent(&self) -> Option<&str> {
        self.parents.first().map(String::as_str)
    }

    /// The second parent, which is the integration head.
    #[must_use]
    pub fn second_parent(&self) -> Option<&str> {
        self.parents.get(1).map(String::as_str)
    }

    /// Every value recorded under one trailer key.
    pub fn trailer_values<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> {
        self.trailers
            .iter()
            .filter(move |(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

/// Builds the landing commit message from its subject and trailers.
///
/// The subject is a function of the integration alone, so rebuilding the same
/// plan produces the same subject. Trailers follow Git's own convention — a
/// blank line, then `Key: value` lines — so `git interpret-trailers` and every
/// tool that understands it can read them without knowing about this harness.
#[must_use]
pub fn compose_message(subject: &str, trailers: &[(String, String)]) -> String {
    let mut message = subject.to_owned();
    message.push_str("\n\n");
    for (key, value) in trailers {
        message.push_str(key);
        message.push_str(": ");
        message.push_str(value);
        message.push('\n');
    }
    message
}

/// Creates the landing commit without moving any branch.
///
/// # Errors
///
/// Returns an external-tool error when Git refuses to build the commit.
pub fn create(
    repository: &Path,
    tree: &str,
    first_parent: &str,
    second_parent: &str,
    message: &str,
) -> Result<String, HarnessError> {
    let output = run_ok(
        &GitScope::work_tree(repository),
        [
            "commit-tree",
            tree,
            "-p",
            first_parent,
            "-p",
            second_parent,
            "-m",
            message,
        ],
    )?;
    Ok(output.trimmed_stdout().to_owned())
}

/// Points the integration's landing ref at a commit, keeping it alive.
///
/// # Errors
///
/// Returns an external-tool error when Git refuses.
pub fn retain(repository: &Path, integration_id: &str, sha: &str) -> Result<(), HarnessError> {
    run_ok(
        &GitScope::work_tree(repository),
        ["update-ref", &landing_ref(integration_id), sha],
    )?;
    Ok(())
}

/// Removes an integration's landing ref.
///
/// # Errors
///
/// Returns an external-tool error when Git refuses to delete an existing ref.
pub fn release(repository: &Path, integration_id: &str) -> Result<(), HarnessError> {
    let scope = GitScope::work_tree(repository);
    let reference = landing_ref(integration_id);
    let existing = run(&scope, ["rev-parse", "--verify", "--quiet", &reference])?;
    if !existing.success() {
        return Ok(());
    }
    run_ok(&scope, ["update-ref", "-d", &reference])?;
    Ok(())
}

/// Reads a landing commit back from the object database.
///
/// # Errors
///
/// Returns a precondition error when the commit does not exist, or an
/// external-tool error when Git cannot read it.
pub fn inspect(repository: &Path, sha: &str) -> Result<LandingObject, HarnessError> {
    let scope = GitScope::work_tree(repository);
    // `%x00` separators keep a multi-line body from being confused with the
    // field boundaries, which a newline-delimited format could not manage.
    let output = run(
        &scope,
        [
            "show",
            "--no-patch",
            "--format=%H%x00%T%x00%P%x00%s%x00%B",
            sha,
        ],
    )?;
    if !output.success() {
        return Err(HarnessError::Control {
            reason: format!(
                "landing commit {sha} could not be read: {}",
                output.diagnostic()
            ),
            code: ErrorCode::PreconditionNotFound,
        });
    }

    let fields: Vec<&str> = output.stdout.split('\0').collect();
    let [commit, tree, parents, subject, body, ..] = fields.as_slice() else {
        return Err(HarnessError::Control {
            reason: format!("git described landing commit {sha} in an unexpected shape"),
            code: ErrorCode::ExternalGitCommand,
        });
    };

    Ok(LandingObject {
        sha: (*commit).trim().to_owned(),
        tree: (*tree).trim().to_owned(),
        parents: parents.split_whitespace().map(ToOwned::to_owned).collect(),
        subject: (*subject).trim().to_owned(),
        trailers: parse_trailers(body),
    })
}

/// Extracts `Key: value` trailers from a commit body.
///
/// Only lines in the final block are considered, which is Git's own rule: a
/// colon-bearing line in the middle of a description is prose, not a trailer.
fn parse_trailers(body: &str) -> Vec<(String, String)> {
    let mut trailers = Vec::new();
    for line in body.trim_end().lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            break;
        };
        if key.is_empty() || key.contains(char::is_whitespace) {
            break;
        }
        trailers.push((key.to_owned(), value.trim().to_owned()));
    }
    trailers.reverse();
    trailers
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

    fn tree_of(repository: &Path, commit: &str) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["rev-parse", &format!("{commit}^{{tree}}")])
            .output()
            .expect("git should run");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn a_landing_commit_has_both_parents_in_order_and_the_exact_tree() {
        let (_temp, repo, base) = repository();
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        let integration = commit(&repo, "integration");
        let tree = tree_of(&repo, &integration);

        let message = compose_message(
            "Land INT-001",
            &[(TRAILER_INTEGRATION.to_owned(), "INT-001".to_owned())],
        );
        let landing = create(&repo, &tree, &base, &integration, &message).expect("a landing");

        let object = inspect(&repo, &landing).expect("readable");
        assert_eq!(object.first_parent(), Some(base.as_str()));
        assert_eq!(object.second_parent(), Some(integration.as_str()));
        assert_eq!(object.tree, tree);
        assert_eq!(object.subject, "Land INT-001");
    }

    #[test]
    fn creating_a_landing_commit_moves_no_branch() {
        let (_temp, repo, base) = repository();
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        let integration = commit(&repo, "integration");
        let tree = tree_of(&repo, &integration);
        let main_before = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .expect("git should run");
        let main_before = String::from_utf8_lossy(&main_before.stdout)
            .trim()
            .to_owned();

        let landing =
            create(&repo, &tree, &base, &integration, "Land INT-001\n").expect("a landing");

        let main_after = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .expect("git should run");
        assert_eq!(
            String::from_utf8_lossy(&main_after.stdout).trim(),
            main_before,
            "building a landing commit must not move a branch"
        );
        assert_ne!(landing, main_before);
    }

    #[test]
    fn a_retained_landing_commit_survives_aggressive_collection() {
        let (_temp, repo, base) = repository();
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        let integration = commit(&repo, "integration");
        let tree = tree_of(&repo, &integration);
        let landing =
            create(&repo, &tree, &base, &integration, "Land INT-001\n").expect("a landing");

        retain(&repo, "INT-001", &landing).expect("retention");
        let output = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["gc", "--prune=now", "--quiet"])
            .output()
            .expect("git should run");
        assert!(output.status.success());

        // Without the ref, collection would have taken it.
        inspect(&repo, &landing).expect("the landing commit must survive collection");

        release(&repo, "INT-001").expect("release");
        let resolved = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "--verify", "--quiet", &landing_ref("INT-001")])
            .output()
            .expect("git should run");
        assert!(!resolved.status.success(), "the ref should be gone");
    }

    #[test]
    fn releasing_an_absent_ref_is_not_an_error() {
        let (_temp, repo, _base) = repository();
        release(&repo, "INT-404").expect("releasing nothing succeeds");
    }

    #[test]
    fn trailers_round_trip_through_the_commit_object() {
        let (_temp, repo, base) = repository();
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        let integration = commit(&repo, "integration");
        let tree = tree_of(&repo, &integration);

        let trailers = vec![
            (TRAILER_INTEGRATION.to_owned(), "INT-001".to_owned()),
            (TRAILER_CYCLE.to_owned(), "C-001".to_owned()),
            (TRAILER_CARD.to_owned(), "F-001 r1 abc123".to_owned()),
            (TRAILER_CARD.to_owned(), "F-002 r1 def456".to_owned()),
            (TRAILER_RECEIPT.to_owned(), "R-000001".to_owned()),
        ];
        let message = compose_message("Land INT-001", &trailers);
        let landing = create(&repo, &tree, &base, &integration, &message).expect("a landing");

        let object = inspect(&repo, &landing).expect("readable");
        assert_eq!(object.trailers, trailers);
        let cards: Vec<&str> = object.trailer_values(TRAILER_CARD).collect();
        assert_eq!(
            cards,
            ["F-001 r1 abc123", "F-002 r1 def456"],
            "repeated trailers must all survive, in order"
        );
    }

    #[test]
    fn prose_containing_a_colon_is_not_read_as_a_trailer() {
        let (_temp, repo, base) = repository();
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        let integration = commit(&repo, "integration");
        let tree = tree_of(&repo, &integration);

        let message =
            "Land INT-001\n\nnote: this line is prose\n\nChange-Harness-Integration: INT-001\n";
        let landing = create(&repo, &tree, &base, &integration, message).expect("a landing");

        let object = inspect(&repo, &landing).expect("readable");
        assert_eq!(
            object.trailers,
            [(TRAILER_INTEGRATION.to_owned(), "INT-001".to_owned())],
            "only the final block is a trailer block"
        );
    }

    #[test]
    fn an_absent_commit_is_a_precondition_error() {
        let (_temp, repo, _base) = repository();
        let error = inspect(&repo, "0000000000000000000000000000000000000000")
            .expect_err("a missing commit is refused");
        assert!(error.to_string().contains("could not be read"), "{error}");
    }
}
