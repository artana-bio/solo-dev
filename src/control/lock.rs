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
/// Written so a stale lock can be diagnosed rather than guessed at.
///
/// The PID alone is not enough. PIDs are recycled, so a lock left by a crashed
/// process can appear held by whatever unrelated program later inherited its
/// number — and the harness would then wait forever on a process that has
/// nothing to do with it. The start time disambiguates: two processes can
/// share a PID, but not a PID and a start instant.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct LockHolder {
    /// Process that acquired the lock.
    pub pid: u32,
    /// When it was acquired.
    pub acquired_at: Timestamp,
    /// What the holder was doing.
    pub operation: String,
    /// The OS-reported start time of the acquiring process.
    ///
    /// Absent when the platform would not report it, which is treated as
    /// "cannot prove staleness" rather than as "stale".
    #[serde(default)]
    pub process_start: Option<String>,
}

/// What can be established about an existing lock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LockDiagnosis {
    /// No lock file exists.
    Free,
    /// The holding process is still running.
    Held(LockHolder),
    /// The holding process is provably gone.
    Stale {
        /// Who left it.
        holder: LockHolder,
        /// How that was established.
        reason: String,
    },
    /// A lock exists but its disposition cannot be established.
    ///
    /// Distinct from `Stale` on purpose. Clearing a lock whose holder might
    /// still be writing is how two processes end up interleaving mutations,
    /// so an unprovable case is escalated to a person rather than resolved by
    /// optimism.
    Ambiguous {
        /// Who left it, when that could be read.
        holder: Option<LockHolder>,
        /// Why the disposition could not be established.
        reason: String,
    },
}

/// The OS-reported start time of a process, when it can be read.
///
/// `ps` is used rather than a platform crate because the crate forbids unsafe
/// code and this is one shell-out on a path that already shells out to Git.
/// A process that has exited yields `None`, which is the signal that matters.
///
/// `LC_ALL=C` is not cosmetic. `lstart` is rendered in the caller's locale —
/// `Tue Jul 28` against `Di. 28 Juli` for the same instant — and this value is
/// compared as a string across two separate invocations that may run under
/// different environments, such as an interactive shell and a cron job. Without
/// pinning, the same live process reads as a *different* one, the lock is
/// declared stale, and recovery clears a lock whose holder is still writing:
/// precisely the interleaving D-056 exists to prevent.
#[must_use]
pub fn process_start_time(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .env("LC_ALL", "C")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
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
        let pid = process::id();
        let holder = LockHolder {
            pid,
            acquired_at: clock.now(),
            operation: operation.to_owned(),
            process_start: process_start_time(pid),
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
                Err(match Self::diagnose(control) {
                    LockDiagnosis::Held(held) => HarnessError::Control {
                        reason: format!(
                            "process {} is running and has held the project lock since {} for `{}`",
                            held.pid, held.acquired_at, held.operation
                        ),
                        code: ErrorCode::PolicyLockHeld,
                    },
                    LockDiagnosis::Stale { holder, reason } => HarnessError::Control {
                        reason: format!(
                            "the project lock was left by process {} during `{}` and is stale: {reason}. Run `project recover --resume` to clear it",
                            holder.pid, holder.operation
                        ),
                        code: ErrorCode::PolicyStaleLock,
                    },
                    LockDiagnosis::Ambiguous { holder, reason } => HarnessError::Control {
                        reason: format!(
                            "the project lock is held by {} and its disposition cannot be established: {reason}. Confirm no harness command is running, then clear it deliberately",
                            holder.map_or_else(
                                || "an unreadable holder".to_owned(),
                                |held| format!("process {}", held.pid)
                            )
                        ),
                        code: ErrorCode::PolicyLockAmbiguous,
                    },
                    // The lock vanished between the failed create and this
                    // read. Reporting it as held is the honest answer: this
                    // attempt did not acquire it, and retrying is cheap.
                    LockDiagnosis::Free => HarnessError::Control {
                        reason: "another command holds the project lock".to_owned(),
                        code: ErrorCode::PolicyLockHeld,
                    },
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

    /// Establishes what can be known about an existing lock.
    #[must_use]
    pub fn diagnose(control: &Path) -> LockDiagnosis {
        let path = control.join(LOCK_FILE);
        if !path.exists() {
            return LockDiagnosis::Free;
        }
        let Some(holder) = Self::read_holder(&path) else {
            return LockDiagnosis::Ambiguous {
                holder: None,
                reason: "the lock file could not be read".to_owned(),
            };
        };

        match process_start_time(holder.pid) {
            // No such process: whoever held it is gone.
            None => LockDiagnosis::Stale {
                reason: format!("process {} is no longer running", holder.pid),
                holder,
            },
            Some(current) => match &holder.process_start {
                // Same PID, same start instant: it is the same process.
                Some(recorded) if *recorded == current => LockDiagnosis::Held(holder),
                // Same PID, different start instant: the number was recycled
                // and this is an unrelated program.
                Some(recorded) => LockDiagnosis::Stale {
                    reason: format!(
                        "process {} started at {current}, but the lock was taken by a process started at {recorded}; the PID was reused",
                        holder.pid
                    ),
                    holder,
                },
                // A lock written before start times were recorded. A live PID
                // might be the holder or might be a reuse, and there is no way
                // to tell, so a person decides.
                None => LockDiagnosis::Ambiguous {
                    reason: format!(
                        "process {} is running, but the lock records no start time to compare against",
                        holder.pid
                    ),
                    holder: Some(holder),
                },
            },
        }
    }

    /// Removes a lock file that has been established as stale.
    ///
    /// Takes the diagnosis rather than re-deriving it, so a caller cannot skip
    /// the check by accident.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be removed.
    pub fn clear_stale(control: &Path, diagnosis: &LockDiagnosis) -> Result<bool, HarnessError> {
        let LockDiagnosis::Stale { .. } = diagnosis else {
            return Ok(false);
        };
        let path = control.join(LOCK_FILE);
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(HarnessError::ControlIo { path, source }),
        }
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
    fn an_unreadable_lock_file_blocks_acquisition_as_ambiguous() {
        // It blocks either way. What changed with `WP-510` is the reason: a
        // lock whose holder cannot be identified is not knowably stale, and
        // clearing it on a guess is how two writers interleave.
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(LOCK_FILE), "not json").unwrap();
        let error = ProjectLock::acquire(temp.path(), "second", &clock()).expect_err("must fail");
        assert_eq!(error.code(), ErrorCode::PolicyLockAmbiguous);
    }

    #[test]
    fn a_lock_left_by_a_dead_process_is_stale() {
        let temp = tempfile::tempdir().unwrap();
        // PID 1 exists; a very high PID reliably does not.
        let holder = LockHolder {
            pid: 4_294_967_294,
            acquired_at: clock().now(),
            operation: "abandoned".to_owned(),
            process_start: Some("whenever".to_owned()),
        };
        fs::write(
            temp.path().join(LOCK_FILE),
            serde_json::to_string(&holder).unwrap(),
        )
        .unwrap();

        let diagnosis = ProjectLock::diagnose(temp.path());
        let LockDiagnosis::Stale { reason, .. } = &diagnosis else {
            panic!("expected stale, got {diagnosis:?}");
        };
        assert!(reason.contains("no longer running"), "{reason}");
        assert!(ProjectLock::clear_stale(temp.path(), &diagnosis).unwrap());
        assert!(!ProjectLock::is_held(temp.path()));
    }

    #[test]
    fn a_reused_pid_is_stale_rather_than_trusted() {
        let temp = tempfile::tempdir().unwrap();
        // This process is alive, but the lock claims a different start
        // instant — which is what PID reuse looks like from here.
        let holder = LockHolder {
            pid: process::id(),
            acquired_at: clock().now(),
            operation: "an older process".to_owned(),
            process_start: Some("a different instant entirely".to_owned()),
        };
        fs::write(
            temp.path().join(LOCK_FILE),
            serde_json::to_string(&holder).unwrap(),
        )
        .unwrap();

        let diagnosis = ProjectLock::diagnose(temp.path());
        let LockDiagnosis::Stale { reason, .. } = &diagnosis else {
            panic!("expected stale, got {diagnosis:?}");
        };
        assert!(reason.contains("PID was reused"), "{reason}");
    }

    #[test]
    fn a_live_holder_is_held_and_never_cleared() {
        let temp = tempfile::tempdir().unwrap();
        let _lock = ProjectLock::acquire(temp.path(), "working", &clock()).unwrap();

        let diagnosis = ProjectLock::diagnose(temp.path());
        assert!(
            matches!(diagnosis, LockDiagnosis::Held(_)),
            "expected held, got {diagnosis:?}"
        );
        assert!(
            !ProjectLock::clear_stale(temp.path(), &diagnosis).unwrap(),
            "a live lock must never be cleared"
        );
        assert!(ProjectLock::is_held(temp.path()));
    }

    #[test]
    fn a_lock_without_a_recorded_start_time_is_ambiguous_not_stale() {
        let temp = tempfile::tempdir().unwrap();
        // A lock written before start times were recorded. The PID is alive,
        // but there is nothing to compare against, so a person decides.
        let holder = LockHolder {
            pid: process::id(),
            acquired_at: clock().now(),
            operation: "legacy".to_owned(),
            process_start: None,
        };
        fs::write(
            temp.path().join(LOCK_FILE),
            serde_json::to_string(&holder).unwrap(),
        )
        .unwrap();

        let diagnosis = ProjectLock::diagnose(temp.path());
        let LockDiagnosis::Ambiguous { reason, .. } = &diagnosis else {
            panic!("expected ambiguous, got {diagnosis:?}");
        };
        assert!(reason.contains("no start time"), "{reason}");
        assert!(
            !ProjectLock::clear_stale(temp.path(), &diagnosis).unwrap(),
            "an unprovable lock must not be cleared"
        );
    }
}
