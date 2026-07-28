//! The control repository: authoritative state, versioned in Git.
//!
//! Section 9.2 makes the control repository the authority for configuration and
//! every workflow record, deliberately outside any candidate worktree so a
//! candidate actor cannot rewrite the policy that judges it.

use std::{
    fs::{self, File},
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::{
    config::ProjectConfig,
    domain::clock::Clock,
    error::{ErrorCode, HarnessError},
    git::command::{GitScope, run, run_ok},
};

/// Path of the project document inside the control repository.
pub const PROJECT_FILE: &str = "project/project.json";

/// Fixed identity used for every control commit.
///
/// Section 9.2: workflow actor identity lives in the authoritative event, not in
/// Git author configuration, so control history stays reproducible regardless of
/// whose shell ran the command.
pub const CONTROL_AUTHOR_NAME: &str = "Change Harness";
/// Email paired with [`CONTROL_AUTHOR_NAME`].
pub const CONTROL_AUTHOR_EMAIL: &str = "change-harness@local.invalid";

/// Files the control repository never tracks.
///
/// The lock is transient by definition. The journal is excluded for a subtler
/// reason: an entry describes a mutation *in flight*, and committing it would
/// place non-authoritative state into authoritative history. Recovery reads the
/// journal from the working tree precisely because a crashed process leaves it
/// there uncommitted. See D-029.
const CONTROL_IGNORE: &str = "harness.lock\njournal/\n";

/// A control repository on disk.
#[derive(Clone, Debug)]
pub struct ControlRepository {
    root: PathBuf,
}

impl ControlRepository {
    /// Opens an existing control repository.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is not an initialized control repository.
    pub fn open(root: &Path) -> Result<Self, HarnessError> {
        let repository = Self {
            root: root.to_path_buf(),
        };
        if !repository.is_initialized() {
            return Err(HarnessError::Control {
                reason: format!(
                    "no control repository at {}; run `project init` first",
                    root.display()
                ),
                code: ErrorCode::ConfigControlIncompatible,
            });
        }
        Ok(repository)
    }

    /// Wraps a path without requiring it to be initialized yet.
    #[must_use]
    pub fn at(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// The control repository's root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute path of a control-relative file.
    #[must_use]
    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// True when both a Git repository and a project document are present.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.root.join(".git").exists() && self.path(PROJECT_FILE).exists()
    }

    /// The Git scope for this repository.
    #[must_use]
    pub fn scope(&self) -> GitScope {
        GitScope::work_tree(&self.root)
    }

    /// Reads the stored project configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the document is missing or malformed.
    pub fn project(&self) -> Result<ProjectConfig, HarnessError> {
        let path = self.path(PROJECT_FILE);
        let raw =
            fs::read_to_string(&path).map_err(|source| HarnessError::ControlIo { path, source })?;
        ProjectConfig::from_json(&raw)
    }

    /// Creates the Git repository and its ignore rules, without writing state.
    ///
    /// Separate from writing the project document so `project init` can journal
    /// between the two and recover if interrupted.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created or Git fails.
    pub fn initialize_git(&self) -> Result<(), HarnessError> {
        fs::create_dir_all(&self.root).map_err(|source| HarnessError::ControlIo {
            path: self.root.clone(),
            source,
        })?;
        if !self.root.join(".git").exists() {
            run_ok(&self.scope(), ["init", "-q", "-b", "main"])?;
        }
        // Identity is configured on the repository, not read from the operator's
        // global config, so control history is byte-identical regardless of who
        // ran the command.
        run_ok(&self.scope(), ["config", "user.name", CONTROL_AUTHOR_NAME])?;
        run_ok(
            &self.scope(),
            ["config", "user.email", CONTROL_AUTHOR_EMAIL],
        )?;
        self.write_atomic(".gitignore", CONTROL_IGNORE)?;
        Ok(())
    }

    /// Writes a control-relative file atomically.
    ///
    /// The write goes to a temporary file in the same directory and is renamed
    /// over the target. Rename within a filesystem is atomic, so a reader or an
    /// interrupted process sees either the old content or the new content, never
    /// a half-written file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be written or renamed.
    pub fn write_atomic(&self, relative: &str, contents: &str) -> Result<(), HarnessError> {
        let target = self.path(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| HarnessError::ControlIo {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let temporary = target.with_extension("tmp");

        let mut file = File::create(&temporary).map_err(|source| HarnessError::ControlIo {
            path: temporary.clone(),
            source,
        })?;
        file.write_all(contents.as_bytes())
            .map_err(|source| HarnessError::ControlIo {
                path: temporary.clone(),
                source,
            })?;
        // fsync before rename: a rename that lands before the data reaches disk
        // would survive a crash pointing at an empty file.
        file.sync_all().map_err(|source| HarnessError::ControlIo {
            path: temporary.clone(),
            source,
        })?;
        drop(file);

        fs::rename(&temporary, &target).map_err(|source| HarnessError::ControlIo {
            path: target,
            source,
        })
    }

    /// Reads a control-relative file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read.
    pub fn read(&self, relative: &str) -> Result<String, HarnessError> {
        let path = self.path(relative);
        fs::read_to_string(&path).map_err(|source| HarnessError::ControlIo { path, source })
    }

    /// The current control commit, or `None` before the first commit.
    ///
    /// # Errors
    ///
    /// Returns an error when Git cannot be executed.
    pub fn head(&self) -> Result<Option<String>, HarnessError> {
        let output = run(&self.scope(), ["rev-parse", "--verify", "HEAD"])?;
        Ok(output.success().then(|| output.trimmed_stdout().to_owned()))
    }

    /// True when nothing is staged or modified.
    ///
    /// # Errors
    ///
    /// Returns an error when Git cannot be executed.
    pub fn is_clean(&self) -> Result<bool, HarnessError> {
        Ok(run_ok(&self.scope(), ["status", "--porcelain"])?
            .trimmed_stdout()
            .is_empty())
    }

    /// Commits every pending change, requiring control state not to have moved.
    ///
    /// `expected_head` is a compare-and-swap: the caller states the commit it
    /// read state from, and the commit is refused if control advanced since.
    /// The project lock already excludes concurrent harness writers, so this
    /// exists to catch an external edit rather than a race between commands.
    ///
    /// Returns `None` when there was nothing to commit, which makes repeated
    /// initialization idempotent rather than producing empty commits.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::ConflictControlHeadMoved`] when control advanced,
    /// or an external-tool error when Git fails.
    pub fn commit(
        &self,
        expected_head: Option<&str>,
        message: &str,
    ) -> Result<Option<String>, HarnessError> {
        let actual = self.head()?;
        if actual.as_deref() != expected_head {
            return Err(HarnessError::Control {
                reason: format!(
                    "control head is {} but {} was expected",
                    actual.as_deref().unwrap_or("unborn"),
                    expected_head.unwrap_or("unborn")
                ),
                code: ErrorCode::ConflictControlHeadMoved,
            });
        }

        run_ok(&self.scope(), ["add", "-A"])?;
        if self.is_clean()? {
            return Ok(None);
        }
        run_ok(&self.scope(), ["commit", "-q", "-m", message])?;
        self.head()
    }

    /// Number of commits in control history.
    ///
    /// # Errors
    ///
    /// Returns an error when Git cannot be executed.
    pub fn commit_count(&self) -> Result<usize, HarnessError> {
        let output = run(&self.scope(), ["rev-list", "--count", "HEAD"])?;
        if !output.success() {
            return Ok(0);
        }
        output
            .trimmed_stdout()
            .parse()
            .map_err(|_| HarnessError::Control {
                reason: "could not read control history length".to_owned(),
                code: ErrorCode::InternalControlCorrupt,
            })
    }
}

/// Writes the project document and commits it, given an already-created
/// repository.
///
/// # Errors
///
/// Returns an error when the document cannot be written or committed.
pub fn write_project(
    control: &ControlRepository,
    config: &ProjectConfig,
    expected_head: Option<&str>,
    clock: &dyn Clock,
) -> Result<Option<String>, HarnessError> {
    control.write_atomic(PROJECT_FILE, &format!("{}\n", config.to_json()?))?;
    control.commit(
        expected_head,
        &format!(
            "project: initialize {} at {}",
            config.project_id,
            clock.now()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_leaves_no_temporary_behind() {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control
            .write_atomic("nested/deep/file.json", "{}\n")
            .unwrap();

        assert_eq!(control.read("nested/deep/file.json").unwrap(), "{}\n");
        assert!(
            !temp.path().join("nested/deep/file.tmp").exists(),
            "the temporary file must be renamed, not left behind"
        );
    }

    #[test]
    fn atomic_write_replaces_existing_content_wholesale() {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control
            .write_atomic("f.json", "old content that is long\n")
            .unwrap();
        control.write_atomic("f.json", "new\n").unwrap();
        assert_eq!(control.read("f.json").unwrap(), "new\n");
    }

    #[test]
    fn initializing_git_configures_the_fixed_identity() {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control.initialize_git().unwrap();

        let name = run_ok(&control.scope(), ["config", "user.name"]).unwrap();
        let email = run_ok(&control.scope(), ["config", "user.email"]).unwrap();
        assert_eq!(name.trimmed_stdout(), CONTROL_AUTHOR_NAME);
        assert_eq!(email.trimmed_stdout(), CONTROL_AUTHOR_EMAIL);
    }

    #[test]
    fn the_lock_file_is_never_tracked() {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control.initialize_git().unwrap();
        fs::write(temp.path().join("harness.lock"), "{}").unwrap();
        control.write_atomic("state.json", "{}\n").unwrap();
        control.commit(None, "initial").unwrap();

        let tracked = run_ok(&control.scope(), ["ls-files"]).unwrap();
        assert!(
            !tracked.trimmed_stdout().contains("harness.lock"),
            "the transient lock must stay untracked: {}",
            tracked.trimmed_stdout()
        );
    }

    #[test]
    fn head_is_none_before_the_first_commit() {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control.initialize_git().unwrap();
        assert!(control.head().unwrap().is_none());
    }

    #[test]
    fn committing_with_a_stale_expected_head_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control.initialize_git().unwrap();
        control.write_atomic("a.json", "1\n").unwrap();
        let first = control.commit(None, "first").unwrap().unwrap();

        control.write_atomic("b.json", "2\n").unwrap();
        // Claiming control is still unborn, when it is actually at `first`.
        let error = control.commit(None, "second").expect_err("must refuse");
        assert_eq!(error.code(), ErrorCode::ConflictControlHeadMoved);
        assert_eq!(control.head().unwrap().as_deref(), Some(first.as_str()));
    }

    #[test]
    fn committing_with_the_correct_expected_head_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control.initialize_git().unwrap();
        control.write_atomic("a.json", "1\n").unwrap();
        let first = control.commit(None, "first").unwrap().unwrap();

        control.write_atomic("b.json", "2\n").unwrap();
        let second = control.commit(Some(&first), "second").unwrap().unwrap();
        assert_ne!(first, second);
        assert_eq!(control.commit_count().unwrap(), 2);
    }

    #[test]
    fn committing_nothing_produces_no_empty_commit() {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control.initialize_git().unwrap();
        control.write_atomic("a.json", "1\n").unwrap();
        let first = control.commit(None, "first").unwrap().unwrap();

        // This is what makes repeated initialization idempotent.
        assert!(control.commit(Some(&first), "again").unwrap().is_none());
        assert_eq!(control.commit_count().unwrap(), 1);
    }

    #[test]
    fn opening_an_uninitialized_directory_fails() {
        let temp = tempfile::tempdir().unwrap();
        let error = ControlRepository::open(temp.path()).expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::ConfigControlIncompatible);
    }
}
