//! Single-writer project lock.
//!
//! One project accepts one mutating command at a time. The lock is a file
//! created with `O_EXCL`, which the operating system makes atomic even between
//! unrelated processes, so two commands racing to create it cannot both win.
//!
//! This is a coordination mechanism, not a security boundary. Invariant 7.2 and
//! D-013 are explicit that the same operating-system account can remove the
//! file. It prevents accidental concurrent mutation, which is the actual
//! failure mode.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    process,
};

use serde::{Deserialize, Serialize};

use crate::{
    domain::clock::{Clock, Timestamp},
    error::{ErrorCode, HarnessError},
};

/// File name of the project lock, relative to the control repository.
pub const LOCK_FILE: &str = "harness.lock";

/// What a held lock records about its holder.
///
/// Written so a stale lock can be diagnosed by a human rather than guessed at.
/// Automatic stale-lock reclamation is `WP-510`; this package only records
/// enough to make that possible.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct LockHolder {
    /// Process that acquired the lock.
    pub pid: u32,
    /// When it was acquired.
    pub acquired_at: Timestamp,
    /// What the holder was doing.
    pub operation: String,
}

/// A held project lock, released when dropped.
///
/// Release happens in `Drop` so an early return or a panic cannot strand the
/// lock. A process killed outright still leaves the file behind, which is what
/// `project recover` is for.
#[derive(Debug)]
pub struct ProjectLock {
    path: PathBuf,
    holder: LockHolder,
}

impl ProjectLock {
    /// Acquires the lock, failing if another process holds it.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::PolicyLockHeld`] when the lock is already held, or
    /// an I/O error when the control directory is not writable.
    pub fn acquire(
        control: &Path,
        operation: &str,
        clock: &dyn Clock,
    ) -> Result<Self, HarnessError> {
        let path = control.join(LOCK_FILE);
        let holder = LockHolder {
            pid: process::id(),
            acquired_at: clock.now(),
            operation: operation.to_owned(),
        };

        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let encoded = serde_json::to_string_pretty(&holder)?;
                file.write_all(encoded.as_bytes())
                    .map_err(|source| HarnessError::ControlIo {
                        path: path.clone(),
                        source,
                    })?;
                file.sync_all().map_err(|source| HarnessError::ControlIo {
                    path: path.clone(),
                    source,
                })?;
                Ok(Self { path, holder })
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = Self::read_holder(&path);
                Err(HarnessError::Control {
                    reason: existing.map_or_else(
                        || "another command holds the project lock".to_owned(),
                        |held| {
                            format!(
                                "process {} has held the project lock since {} for `{}`",
                                held.pid, held.acquired_at, held.operation
                            )
                        },
                    ),
                    code: ErrorCode::PolicyLockHeld,
                })
            }
            Err(source) => Err(HarnessError::ControlIo { path, source }),
        }
    }

    /// Reads the holder recorded in an existing lock file, if it is readable.
    #[must_use]
    pub fn read_holder(path: &Path) -> Option<LockHolder> {
        let raw = fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// The recorded holder of this lock.
    #[must_use]
    pub const fn holder(&self) -> &LockHolder {
        &self.holder
    }

    /// True when a lock file exists at the given control repository.
    #[must_use]
    pub fn is_held(control: &Path) -> bool {
        control.join(LOCK_FILE).exists()
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        // A failure here cannot be reported, and retrying would not help; the
        // stale file is recoverable by design.
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::FixedClock;

    fn clock() -> FixedClock {
        FixedClock::at_unix_seconds(1_785_196_800).unwrap()
    }

    #[test]
    fn acquiring_writes_a_readable_holder_record() {
        let temp = tempfile::tempdir().unwrap();
        let lock = ProjectLock::acquire(temp.path(), "project.init", &clock()).unwrap();
        assert_eq!(lock.holder().pid, process::id());
        assert_eq!(lock.holder().operation, "project.init");

        let recorded = ProjectLock::read_holder(&temp.path().join(LOCK_FILE)).unwrap();
        assert_eq!(&recorded, lock.holder());
    }

    #[test]
    fn a_second_acquisition_fails_as_a_policy_violation() {
        let temp = tempfile::tempdir().unwrap();
        let _first = ProjectLock::acquire(temp.path(), "first", &clock()).unwrap();
        let error = ProjectLock::acquire(temp.path(), "second", &clock()).expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::PolicyLockHeld);
        assert_eq!(error.category(), crate::cli::exit::ExitCategory::Policy);
    }

    #[test]
    fn the_failure_names_the_current_holder() {
        let temp = tempfile::tempdir().unwrap();
        let _first = ProjectLock::acquire(temp.path(), "project.init", &clock()).unwrap();
        let error = ProjectLock::acquire(temp.path(), "second", &clock()).expect_err("must fail");
        assert!(error.to_string().contains("project.init"));
        assert!(error.to_string().contains(&process::id().to_string()));
    }

    #[test]
    fn dropping_releases_the_lock_so_the_next_command_proceeds() {
        let temp = tempfile::tempdir().unwrap();
        {
            let _lock = ProjectLock::acquire(temp.path(), "first", &clock()).unwrap();
            assert!(ProjectLock::is_held(temp.path()));
        }
        assert!(!ProjectLock::is_held(temp.path()));
        ProjectLock::acquire(temp.path(), "second", &clock()).expect("lock should be free");
    }

    #[test]
    fn a_panic_does_not_strand_the_lock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let result = std::panic::catch_unwind(move || {
            let _lock = ProjectLock::acquire(&path, "panicking", &clock()).unwrap();
            panic!("simulated failure mid-operation");
        });
        assert!(result.is_err());
        assert!(
            !ProjectLock::is_held(temp.path()),
            "Drop must release the lock even on unwind"
        );
    }

    #[test]
    fn an_unreadable_lock_file_still_blocks_acquisition() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(LOCK_FILE), "not json").unwrap();
        let error = ProjectLock::acquire(temp.path(), "second", &clock()).expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::PolicyLockHeld);
    }
}
