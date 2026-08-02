//! The control repository: authoritative state, versioned in Git.
//!
//! Section 9.2 makes the control repository the authority for configuration and
//! every workflow record, deliberately outside any candidate worktree so a
//! candidate actor cannot rewrite the policy that judges it.

use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::{
    config::ProjectConfig,
    domain::clock::Clock,
    error::{ErrorCode, HarnessError},
    git::command::{GitScope, run, run_ok, run_with_config_and_environment, run_with_config_ok},
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
///
/// Gate logs are excluded because Section 14.3 gives them retention windows
/// rather than permanence, and a passing gate's output is large and
/// uninteresting. Invariant 7.4.2 is satisfied by the receipt, which records
/// each log's location and digest.
///
/// The lock's scratch file and the atomic-write temporary are both listed
/// because both exist for a window on every write, and a crash inside that
/// window leaves one behind. `harness.lock` alone did not match
/// `harness.lock.staging.<pid>.<n>`.
const CONTROL_IGNORE: &str = "harness.lock\nharness.lock.staging.*\n*.tmp\njournal/\nlogs/\n";

/// Every path the control repository stages.
///
/// Staging used to be `git add -A`, which swept whatever happened to be sitting
/// in the control directory into authoritative history: a crashed gate's dump,
/// a half-written record, the lock's own scratch file. Naming the paths makes
/// inclusion deliberate, the way everything else in this crate is
/// deny-by-default.
///
/// The trade is real and runs the other way: a record type added here and not
/// added to this list would be written to disk and never committed, so control
/// state would diverge from control history silently. That is worse than the
/// residue, which is why `control_history_holds_everything_a_lifecycle_writes`
/// exists — it runs a full lifecycle and refuses to let anything the harness
/// wrote stay untracked.
const CONTROL_TRACKED_PATHS: &[&str] = &[
    ".gitignore",
    "project",
    "cards",
    "cycles",
    "events",
    "gates",
    "handoffs",
    "reviews",
    "receipts",
    "leases",
    "integrations",
    "verifications",
    "acceptances",
    "archives",
];

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
            // `init.templateDir=` (empty) copies no template. A global template
            // is an ordinary thing for an operator to have, and one carrying a
            // `pre-commit` gets it installed into the control repository, where
            // it refuses the very first commit and `project init` fails
            // outright — verified on git 2.50.1. `core.hooksPath` below already
            // makes such a hook inert; this stops it being written at all,
            // because the audit trail should not contain someone else's scripts.
            run_ok(
                &self.scope(),
                ["-c", "init.templateDir=", "init", "-q", "-b", "main"],
            )?;
        }
        // Identity is configured on the repository, not read from the operator's
        // global config, so control history is byte-identical regardless of who
        // ran the command.
        run_ok(&self.scope(), ["config", "user.name", CONTROL_AUTHOR_NAME])?;
        run_ok(
            &self.scope(),
            ["config", "user.email", CONTROL_AUTHOR_EMAIL],
        )?;
        // The same reasoning one layer down. Control history is the audit
        // trail: the harness writes every commit in it and nobody edits them by
        // hand. Local configuration outranks `~/.gitconfig`, so this is where a
        // repository the harness owns outright records that no hook and no
        // signing key applies to it — while touching no configuration belonging
        // to the operator or to the candidate repository.
        //
        // Written into this repository rather than passed as `-c` on every
        // invocation precisely because this repository is the harness's. The
        // candidate repository is the developer's and gets the opposite
        // treatment: see `git::command::authoring_overrides`.
        //
        // Verified: without these, a `~/.gitconfig` carrying
        // `commit.gpgsign = true` with an unusable signer, or a global
        // `core.hooksPath` whose `pre-commit` refuses, makes `project init`
        // fail with CH-EXTERNAL-GIT-COMMAND and leaves control history unborn.
        // A hooks directory that does not exist is how Git is told there are no
        // hooks; it is not created, so nothing can later appear in it.
        run_ok(&self.scope(), ["config", "commit.gpgsign", "false"])?;
        let hook_sink = self.root.join(".git").join("change-harness-no-hooks");
        run_ok(
            &self.scope(),
            [
                "config".as_ref(),
                "core.hooksPath".as_ref(),
                hook_sink.as_os_str(),
            ],
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

        // `git add` must apply the repository's real clean filters so the bytes
        // we validate are exactly the bytes that would be committed. It runs
        // against a copied index and a quarantined object database; a refusal
        // therefore cannot leave a rejected blob in the durable repository.
        let quarantine = tempfile::tempdir_in(self.root.join(".git")).map_err(|source| {
            HarnessError::ControlIo {
                path: self.root.join(".git"),
                source,
            }
        })?;
        let quarantine_objects = quarantine.path().join("objects");
        fs::create_dir_all(&quarantine_objects).map_err(|source| HarnessError::ControlIo {
            path: quarantine_objects.clone(),
            source,
        })?;
        let durable_index = self.root.join(".git/index");
        let durable_index_lock = self.root.join(".git/index.lock");
        let mut index_lock = DurableIndexLock::acquire(&durable_index_lock)?;
        let quarantine_index = quarantine.path().join("index");
        if durable_index.exists() {
            fs::copy(&durable_index, &quarantine_index).map_err(|source| {
                HarnessError::ControlIo {
                    path: durable_index.clone(),
                    source,
                }
            })?;
        }
        let durable_objects = self.root.join(".git/objects");
        let quarantine_environment = [
            (OsStr::new("GIT_INDEX_FILE"), quarantine_index.as_os_str()),
            (
                OsStr::new("GIT_OBJECT_DIRECTORY"),
                quarantine_objects.as_os_str(),
            ),
            (
                OsStr::new("GIT_ALTERNATE_OBJECT_DIRECTORIES"),
                durable_objects.as_os_str(),
            ),
        ];
        let quarantine_overrides = [OsString::from("core.splitIndex=false")];
        if quarantine_index.exists() {
            // A copied split index can make Git write a new sharedindex file
            // beside the real repository even when GIT_INDEX_FILE is
            // quarantined. Materialize a full quarantine index first; this
            // command writes only the quarantine index path.
            run_with_config_and_environment(
                &self.scope(),
                &quarantine_overrides,
                &quarantine_environment,
                ["update-index", "--no-split-index"],
            )?
            .require_success()?;
        }

        // `--all` within each named path, so a deletion inside one is staged as
        // well as an addition. Paths absent from disk are dropped first: `git
        // add` treats a pathspec matching nothing as a fatal error, and most of
        // these do not exist until the lifecycle stage that creates them.
        let present: Vec<&str> = CONTROL_TRACKED_PATHS
            .iter()
            .copied()
            .filter(|relative| self.path(relative).exists())
            .collect();
        if !present.is_empty() {
            let mut stage: Vec<&str> = vec!["add", "--all", "--"];
            stage.extend_from_slice(&present);
            run_with_config_and_environment(
                &self.scope(),
                &quarantine_overrides,
                &quarantine_environment,
                stage,
            )?
            .require_success()?;
        }
        // Whether anything is *staged*, not whether the worktree is clean.
        // Staging is selective now, so residue outside the allowlist leaves the
        // worktree permanently dirty; asking `is_clean` would send every commit
        // into `git commit` with nothing staged, which fails.
        let staged = run_with_config_and_environment(
            &self.scope(),
            &quarantine_overrides,
            &quarantine_environment,
            ["diff", "--cached", "--quiet"],
        )?;
        if staged.success() {
            return Ok(None);
        }
        refuse_non_text_control_entries(
            &self.scope(),
            &quarantine_overrides,
            &quarantine_environment,
        )?;
        index_lock.write_from(&quarantine_index)?;
        let promoted_objects = promote_quarantine_objects(&quarantine_objects, &durable_objects)?;
        if let Err(error) = index_lock.install(&durable_index) {
            rollback_promoted_objects(&promoted_objects)?;
            return Err(error);
        }
        // This remains an invocation-level defence as well as the local
        // configuration set during initialization. An existing control
        // repository can have those local settings removed, and `project init`
        // intentionally short-circuits before `initialize_git` in that case.
        // The audit write must still be independent of an operator's global
        // hooks and signing configuration.
        let hook_sink = self.root.join(".git").join("change-harness-no-hooks");
        let mut hooks_path = std::ffi::OsString::from("core.hooksPath=");
        hooks_path.push(hook_sink.as_os_str());
        let overrides = vec![hooks_path, std::ffi::OsString::from("commit.gpgsign=false")];
        run_with_config_ok(&self.scope(), &overrides, ["commit", "-q", "-m", message])?;
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

/// Refuses the exact bytes Git staged for the intended control entries.
///
/// The control repository stores JSON and other text records. NUL is rejected
/// separately because UTF-16LE ASCII text is technically valid UTF-8 with an
/// embedded NUL, and that is the concrete encoding that bypassed the prior
/// scanner. Git is invoked with the existing [`CONTROL_TRACKED_PATHS`] path
/// boundary, while its index and newly created objects remain quarantined until
/// this check passes.
fn refuse_non_text_control_entries(
    scope: &GitScope,
    overrides: &[OsString],
    environment: &[(&OsStr, &OsStr)],
) -> Result<(), HarnessError> {
    let paths = run_with_config_and_environment(
        scope,
        overrides,
        environment,
        ["diff", "--cached", "--name-only", "-z"],
    )?
    .require_success()?;
    for raw_path in paths.stdout_bytes.split(|byte| *byte == 0) {
        if raw_path.is_empty() {
            continue;
        }
        let relative = std::str::from_utf8(raw_path).map_err(|_| HarnessError::Control {
            reason: "a staged control path is not valid UTF-8".to_owned(),
            code: ErrorCode::PolicyControlEncoding,
        })?;
        let spec = format!(":{relative}");
        let blob = run_with_config_and_environment(scope, overrides, environment, ["show", &spec])?;
        if blob.success() {
            refuse_non_text_control_bytes(relative, &blob.stdout_bytes)?;
        }
    }
    Ok(())
}

fn refuse_non_text_control_bytes(relative: &str, contents: &[u8]) -> Result<(), HarnessError> {
    let valid_utf8 = std::str::from_utf8(contents).is_ok();
    let has_nul = contents.contains(&0);
    if !valid_utf8 || has_nul {
        let reason = if valid_utf8 {
            "contains embedded NUL bytes"
        } else {
            "is not valid UTF-8"
        };
        return Err(HarnessError::Control {
            reason: format!("control entry `{relative}` {reason}"),
            code: ErrorCode::PolicyControlEncoding,
        });
    }
    Ok(())
}

/// An exclusively-created Git index lock held for the duration of promotion.
///
/// The file is created before quarantine staging, so a pre-existing lock fails
/// the operation before any durable object or index mutation. Dropping an
/// uninstalled lock removes only the path this operation created.
struct DurableIndexLock {
    path: PathBuf,
    file: Option<File>,
    installed: bool,
}

impl DurableIndexLock {
    fn acquire(path: &Path) -> Result<Self, HarnessError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| HarnessError::ControlIo {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
            installed: false,
        })
    }

    fn write_from(&mut self, source: &Path) -> Result<(), HarnessError> {
        let contents = fs::read(source).map_err(|source_error| HarnessError::ControlIo {
            path: source.to_path_buf(),
            source: source_error,
        })?;
        let file = self.file.as_mut().expect("index lock file is held");
        file.write_all(&contents)
            .map_err(|source_error| HarnessError::ControlIo {
                path: self.path.clone(),
                source: source_error,
            })?;
        file.sync_all()
            .map_err(|source_error| HarnessError::ControlIo {
                path: self.path.clone(),
                source: source_error,
            })
    }

    fn install(mut self, destination: &Path) -> Result<(), HarnessError> {
        if let Some(file) = self.file.take() {
            file.sync_all().map_err(|source| HarnessError::ControlIo {
                path: self.path.clone(),
                source,
            })?;
        }
        fs::rename(&self.path, destination).map_err(|source| HarnessError::ControlIo {
            path: destination.to_path_buf(),
            source,
        })?;
        self.installed = true;
        Ok(())
    }
}

impl Drop for DurableIndexLock {
    fn drop(&mut self) {
        if !self.installed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn promote_quarantine_objects(
    source: &Path,
    destination: &Path,
) -> Result<Vec<PathBuf>, HarnessError> {
    if !source.exists() {
        return Ok(Vec::new());
    }
    fs::create_dir_all(destination).map_err(|source_error| HarnessError::ControlIo {
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    let mut promoted = Vec::new();
    if let Err(error) = promote_quarantine_tree(source, destination, &mut promoted) {
        rollback_promoted_objects(&promoted)?;
        return Err(error);
    }
    Ok(promoted)
}

fn promote_quarantine_tree(
    source: &Path,
    destination: &Path,
    promoted: &mut Vec<PathBuf>,
) -> Result<(), HarnessError> {
    let metadata =
        fs::symlink_metadata(source).map_err(|source_error| HarnessError::ControlIo {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    if metadata.file_type().is_dir() {
        fs::create_dir_all(destination).map_err(|source_error| HarnessError::ControlIo {
            path: destination.to_path_buf(),
            source: source_error,
        })?;
        let entries = fs::read_dir(source).map_err(|source_error| HarnessError::ControlIo {
            path: source.to_path_buf(),
            source: source_error,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source_error| HarnessError::ControlIo {
                path: source.to_path_buf(),
                source: source_error,
            })?;
            promote_quarantine_tree(
                &entry.path(),
                &destination.join(entry.file_name()),
                promoted,
            )?;
        }
        return Ok(());
    }
    if !metadata.file_type().is_file() {
        return Err(HarnessError::ControlIo {
            path: source.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "quarantine object is not a regular file",
            ),
        });
    }
    if destination.exists() {
        fs::remove_file(source).map_err(|source_error| HarnessError::ControlIo {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source_error| HarnessError::ControlIo {
                path: parent.to_path_buf(),
                source: source_error,
            })?;
        }
        fs::rename(source, destination).map_err(|source_error| HarnessError::ControlIo {
            path: destination.to_path_buf(),
            source: source_error,
        })?;
        promoted.push(destination.to_path_buf());
    }
    Ok(())
}

fn rollback_promoted_objects(paths: &[PathBuf]) -> Result<(), HarnessError> {
    for path in paths.iter().rev() {
        if path.exists() {
            fs::remove_file(path).map_err(|source| HarnessError::ControlIo {
                path: path.clone(),
                source,
            })?;
        }
    }
    Ok(())
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
        // Real control paths: staging is by allowlist, so a file at a path the
        // harness never writes is deliberately not tracked.
        control.write_atomic("cards/a.json", "1\n").unwrap();
        let first = control.commit(None, "first").unwrap().unwrap();

        control.write_atomic("cards/b.json", "2\n").unwrap();
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
        // Real control paths: staging is by allowlist, so a file written where
        // the harness never writes is deliberately not tracked.
        control.write_atomic("cards/a.json", "1\n").unwrap();
        let first = control.commit(None, "first").unwrap().unwrap();

        control.write_atomic("cards/b.json", "2\n").unwrap();
        let second = control.commit(Some(&first), "second").unwrap().unwrap();
        assert_ne!(first, second);
        assert_eq!(control.commit_count().unwrap(), 2);
    }

    #[test]
    fn committing_nothing_produces_no_empty_commit() {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control.initialize_git().unwrap();
        control.write_atomic("cards/a.json", "1\n").unwrap();
        let first = control.commit(None, "first").unwrap().unwrap();

        // This is what makes repeated initialization idempotent.
        assert!(control.commit(Some(&first), "again").unwrap().is_none());
        assert_eq!(control.commit_count().unwrap(), 1);
    }

    #[test]
    fn a_file_at_an_unowned_path_is_not_committed() {
        // Tier 2, defect 9. Staging was `git add -A`, so anything in the
        // control directory when a mutation committed went into authoritative
        // history — a crashed gate's dump, a half-written record, the lock's
        // own scratch file.
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control.initialize_git().unwrap();
        control.write_atomic("cards/a.json", "1\n").unwrap();
        let first = control.commit(None, "first").unwrap().unwrap();

        std::fs::write(temp.path().join("crash-dump.txt"), "secrets\n").unwrap();
        std::fs::write(temp.path().join("harness.lock.staging.1.0"), "pid\n").unwrap();
        assert!(
            control.commit(Some(&first), "again").unwrap().is_none(),
            "residue must not produce a commit at all"
        );

        let listed = run(&control.scope(), ["ls-files"]).unwrap();
        let tracked = listed.trimmed_stdout();
        assert!(tracked.contains("cards/a.json"), "{tracked}");
        assert!(!tracked.contains("crash-dump"), "{tracked}");
        assert!(!tracked.contains("staging"), "{tracked}");
    }

    #[test]
    fn opening_an_uninitialized_directory_fails() {
        let temp = tempfile::tempdir().unwrap();
        let error = ControlRepository::open(temp.path()).expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::ConfigControlIncompatible);
    }
}
