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
    sync::atomic::{AtomicU64, Ordering},
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
    match process_liveness(pid) {
        Liveness::Running(start) => Some(start),
        Liveness::Gone | Liveness::Unknown(_) => None,
    }
}

/// What could be established about a process.
///
/// Three outcomes, and collapsing them was defect 13. `ps` failing to run at
/// all — not installed, not on `PATH`, a slim container, a restricted
/// environment — produced the same `None` as `ps` reporting the process gone,
/// and `diagnose` read that as proof of death. So on any host without `ps`,
/// every lock read as stale and `recover` deleted one whose holder was still
/// writing: exactly the interleaving D-056 exists to prevent, reached by the
/// mechanism built to prevent it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Liveness {
    /// The process exists, and this is when it started.
    Running(String),
    /// `ps` was consulted and reported no such process.
    Gone,
    /// `ps` could not be consulted, so nothing was established.
    Unknown(String),
}

/// Classifies what running `ps` produced.
///
/// Separate from the invocation so it can be tested: making `ps` unavailable
/// inside a test process means changing `PATH` globally, which is unsound with
/// tests running in parallel and would break every other command besides.
fn classify(outcome: std::io::Result<std::process::Output>) -> Liveness {
    let output = match outcome {
        Ok(output) => output,
        Err(error) => return Liveness::Unknown(format!("`ps` could not be run: {error}")),
    };
    if !output.status.success() {
        // `ps -p` exits non-zero for a pid that does not exist, which is the
        // answer we want, and also for a usage error, which is not. The two are
        // distinguishable only by stderr: a working `ps` says nothing there.
        let complaint = String::from_utf8_lossy(&output.stderr);
        let complaint = complaint.trim();
        if complaint.is_empty() {
            return Liveness::Gone;
        }
        return Liveness::Unknown(format!("`ps` did not answer: {complaint}"));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() {
        return Liveness::Gone;
    }
    Liveness::Running(value)
}

/// Asks the system whether a process is alive, and when it started.
#[must_use]
pub fn process_liveness(pid: u32) -> Liveness {
    classify(
        std::process::Command::new("ps")
            .env("LC_ALL", "C")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .output(),
    )
}

/// Distinguishes one acquisition attempt's scratch file from another's.
static SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

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

/// What reading the lock file established, for `diagnose_with`.
///
/// A plain `Option<LockHolder>` cannot tell "no lock file" apart from "a
/// lock file present with contents that didn't parse" — and those two need
/// different diagnoses, `Free` and `Ambiguous` respectively.
enum HolderRead {
    /// No lock file exists.
    Missing,
    /// A lock file exists but its contents did not parse.
    Unreadable,
    /// A lock file exists and named this holder.
    Present(LockHolder),
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

        // The contents are written to a scratch file first and linked into
        // place, rather than creating the lock and filling it in afterwards.
        // `create_new` makes acquisition exclusive either way, but it also
        // makes the file *visible* before it has contents — and a contender
        // that reads it in that window sees an unparseable holder and reports
        // the lock as ambiguous, telling an operator to check whether a
        // command is running while one plainly is. `hard_link` fails when the
        // destination exists, so it keeps the exclusion and the file is
        // complete the instant it appears.
        //
        // The scratch name is unique per *attempt*, not per process: threads
        // inside one process share a pid, and two of them racing on one name
        // would each delete the other's file mid-flight.
        let attempt = SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let scratch = path.with_extension(format!("staging.{pid}.{attempt}"));
        let encoded = serde_json::to_string_pretty(&holder)?;
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&scratch)
                .map_err(|source| HarnessError::ControlIo {
                    path: scratch.clone(),
                    source,
                })?;
            file.write_all(encoded.as_bytes())
                .map_err(|source| HarnessError::ControlIo {
                    path: scratch.clone(),
                    source,
                })?;
            file.sync_all().map_err(|source| HarnessError::ControlIo {
                path: scratch.clone(),
                source,
            })?;
        }

        let linked = fs::hard_link(&scratch, &path);
        // The scratch file has done its job either way; leaving it behind
        // would make the next acquisition by this pid see stale contents.
        let _ = fs::remove_file(&scratch);
        match linked {
            Ok(()) => Ok(Self { path, holder }),
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
        Self::diagnose_with(control, &process_liveness)
    }

    /// `diagnose`, with liveness checking injected so the race it has to
    /// survive can be driven deterministically instead of merely hoped not
    /// to happen.
    ///
    /// The race, exactly as issue #78 hit it: `diagnose` reads the holder
    /// record, then asks `liveness` whether that process is still running.
    /// For the real `process_liveness` that means shelling out to `ps` —
    /// milliseconds, not microseconds. If the recorded holder finishes and
    /// its `Drop` removes the lock file inside that window, `ps` truthfully
    /// reports the process gone, but the file it was gone from is no longer
    /// there to be diagnosed. Reporting `Stale` anyway — what this used to
    /// do — reaches `acquire`, which maps `Stale` to `PolicyStaleLock`: no
    /// retry, and a message sending a healthy operator to run `project
    /// recover --resume` against a lock that already released itself.
    ///
    /// The fix: a diagnosis computed from the pre-`liveness` read is not
    /// trusted until the lock file is read again and still names that exact
    /// holder. A holder that changed was diagnosed against a lock that, by
    /// the time the answer came back, was already someone else's business,
    /// so it is rediagnosed against whoever holds it now — for up to two
    /// passes, which bounds this to at most two calls to `liveness` no
    /// matter how fast the lock churns. A lock still changing after both
    /// passes is reported `Held`, not `Ambiguous` — see the final return
    /// below for why that is a truthful answer rather than a guess.
    fn diagnose_with(control: &Path, liveness: &dyn Fn(u32) -> Liveness) -> LockDiagnosis {
        let path = control.join(LOCK_FILE);

        let mut holder = match Self::read_current(&path) {
            HolderRead::Missing => return LockDiagnosis::Free,
            HolderRead::Unreadable => {
                return LockDiagnosis::Ambiguous {
                    holder: None,
                    reason: "the lock file could not be read".to_owned(),
                };
            }
            HolderRead::Present(holder) => holder,
        };

        for _ in 0..2 {
            let diagnosis = Self::diagnose_holder(&holder, liveness);
            match Self::read_current(&path) {
                HolderRead::Missing => return LockDiagnosis::Free,
                // Unchanged since it was read: the diagnosis just computed
                // describes the lock as it actually is right now, not as it
                // was milliseconds ago.
                HolderRead::Present(reread) if reread == holder => return diagnosis,
                // Changed hands mid-check: the diagnosis above is an answer
                // to a question that is no longer being asked. Ask again,
                // about whoever holds it now.
                HolderRead::Present(reread) => holder = reread,
                // No holder to retry with, and in this module "present but
                // unreadable" has never meant `Free`.
                HolderRead::Unreadable => {
                    return LockDiagnosis::Ambiguous {
                        holder: None,
                        reason: "the lock changed while it was being inspected, and its new contents could not be read".to_owned(),
                    };
                }
            }
        }

        // A holder record that changed on reread was written *inside* the
        // inspection window: whoever holds it took the lock milliseconds
        // ago. Abandonment leaves the file still; this file is the opposite
        // of still, so `Held` is the truthful reading, not a guess reached
        // for want of a better one.
        //
        // It self-corrects, too. If this last holder turns out to have died
        // right after acquiring, `Held` maps to `PolicyLockHeld` in
        // `acquire`, which the retry loops in `gate.rs` cover; the next
        // attempt reads that same, now-unchanging file, liveness reports
        // `Gone`, and `Stale` comes out correctly then. Reporting
        // `Ambiguous` here instead would map to `PolicyLockAmbiguous`, which
        // none of those retry loops cover, so a healthy system busy enough
        // to keep changing the lock during inspection would fail outright
        // with "confirm no harness command is running" — the exact failure
        // #78 exists to remove, reappearing at higher contention.
        LockDiagnosis::Held(holder)
    }

    /// Reads whatever the lock file currently says.
    fn read_current(path: &Path) -> HolderRead {
        if !path.exists() {
            return HolderRead::Missing;
        }
        Self::read_holder(path).map_or(HolderRead::Unreadable, HolderRead::Present)
    }

    /// Diagnoses one holder record via `liveness`.
    ///
    /// Says nothing about whether that record still describes the lock file
    /// by the time it returns — confirming that is `diagnose_with`'s job,
    /// because it is the one with a second read to compare against.
    fn diagnose_holder(holder: &LockHolder, liveness: &dyn Fn(u32) -> Liveness) -> LockDiagnosis {
        match liveness(holder.pid) {
            // No such process: whoever held it is gone.
            Liveness::Gone => LockDiagnosis::Stale {
                reason: format!("process {} is no longer running", holder.pid),
                holder: holder.clone(),
            },
            // Nothing was established, so nothing may be concluded. Clearing a
            // lock whose holder might still be writing is the failure this
            // whole module exists to prevent, and "the tool was missing" is not
            // evidence of death.
            Liveness::Unknown(reason) => LockDiagnosis::Ambiguous {
                reason: format!(
                    "process {} could not be checked: {reason}. Confirm by hand whether it is still running",
                    holder.pid
                ),
                holder: Some(holder.clone()),
            },
            Liveness::Running(current) => match &holder.process_start {
                // Same PID, same start instant: it is the same process.
                Some(recorded) if *recorded == current => LockDiagnosis::Held(holder.clone()),
                // Same PID, different start instant: the number was recycled
                // and this is an unrelated program.
                Some(recorded) => LockDiagnosis::Stale {
                    reason: format!(
                        "process {} started at {current}, but the lock was taken by a process started at {recorded}; the PID was reused",
                        holder.pid
                    ),
                    holder: holder.clone(),
                },
                // A lock written before start times were recorded. A live PID
                // might be the holder or might be a reuse, and there is no way
                // to tell, so a person decides.
                None => LockDiagnosis::Ambiguous {
                    reason: format!(
                        "process {} is running, but the lock records no start time to compare against",
                        holder.pid
                    ),
                    holder: Some(holder.clone()),
                },
            },
        }
    }

    /// Removes a lock file that has been established as stale.
    ///
    /// Takes the diagnosis rather than re-deriving it, so a caller cannot skip
    /// the check by accident.
    ///
    /// Removal is conditional on the lock file still naming the exact holder
    /// that was diagnosed. `diagnose` and `clear_stale` are two separate
    /// calls — in `project recover --resume` there is a whole command's
    /// worth of work between them — and the file is free to change in that
    /// window. The sequence that motivates this: `diagnose` reports `Stale`
    /// for holder A, correctly, because A died and left the file behind; A's
    /// entry is then cleared and a live process B legitimately acquires the
    /// same path before this function runs; an unconditional delete — what
    /// this used to do — removes B's live lock anyway, because all it
    /// checked was that *some* diagnosis of `Stale` had been handed to it.
    /// B keeps mutating the control repository with no lock protecting it,
    /// and a third process is now free to acquire the same path concurrently
    /// and mutate alongside B. That is data corruption, not a false alarm:
    /// strictly worse than the stale-lock misdiagnosis this module was built
    /// to fix, because it destroys a real exclusion instead of merely
    /// reporting one that was not real. So a stale verdict about one specific
    /// abandoned lock only ever removes that lock: if the file now names
    /// someone else, is unreadable, or is gone, there is nothing left that
    /// this diagnosis makes safe to clear.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be removed.
    pub fn clear_stale(control: &Path, diagnosis: &LockDiagnosis) -> Result<bool, HarnessError> {
        let LockDiagnosis::Stale { holder, .. } = diagnosis else {
            return Ok(false);
        };
        let path = control.join(LOCK_FILE);
        match Self::read_holder(&path) {
            Some(current) if current == *holder => match fs::remove_file(&path) {
                Ok(()) => Ok(true),
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(source) => Err(HarnessError::ControlIo { path, source }),
            },
            // Absent, unreadable, or someone else's now: the diagnosed lock
            // no longer exists, so there is nothing safe to remove.
            Some(_) | None => Ok(false),
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

    use std::os::unix::process::ExitStatusExt as _;

    fn output(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn ps_being_unavailable_is_not_evidence_of_death() {
        // Tier 3, defect 13. `ps` failing to run produced the same `None` as
        // `ps` reporting the process gone, and `diagnose` read that as proof of
        // death — so on any host without `ps`, every lock read as stale and
        // recovery deleted one whose holder was still writing.
        let missing = classify(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No such file or directory",
        )));
        assert!(
            matches!(missing, Liveness::Unknown(_)),
            "a missing tool establishes nothing: {missing:?}"
        );

        let complained = classify(Ok(output(1, "", "usage: ps [-AaCcEefhjlMmrSTvwXx]")));
        assert!(
            matches!(complained, Liveness::Unknown(_)),
            "a usage error is not an answer either: {complained:?}"
        );
    }

    #[test]
    fn a_silent_non_zero_ps_means_the_process_is_gone() {
        // The guard. `ps -p <pid>` exits non-zero and says nothing on stderr
        // for a pid that does not exist, which is the answer stale-lock
        // clearing depends on. Treating every non-zero exit as unknown would
        // make a stale lock permanent.
        assert_eq!(classify(Ok(output(1, "", ""))), Liveness::Gone);
        assert_eq!(classify(Ok(output(0, "", ""))), Liveness::Gone);
    }

    #[test]
    fn a_running_process_reports_its_start_instant() {
        assert_eq!(
            classify(Ok(output(0, "Tue Jul 28 09:12:03 2026\n", ""))),
            Liveness::Running("Tue Jul 28 09:12:03 2026".to_owned())
        );
    }
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
        // A pid that is in range and absent. Not an enormous one: `ps` answers
        // "process id too large" on stderr for those, which is a refusal to
        // answer rather than a report of death — and treating it as death was
        // defect 13. This fixture asserted the bug's behaviour, so it had to
        // change with the fix.
        let holder = LockHolder {
            pid: 99_999,
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

    #[test]
    fn clear_stale_refuses_when_the_lock_is_no_longer_the_diagnosed_one() {
        // `diagnose` and `clear_stale` are two separate calls, with a much
        // wider window between them than the one closed inside
        // `diagnose_with` itself. Diagnose a dead holder's lock as stale,
        // then let a legitimate successor take the same path before
        // `clear_stale` runs — exactly the sequence from issue #78's
        // follow-up: A dies and is diagnosed stale, A's entry clears and B
        // legitimately acquires, and an unconditional delete would strand
        // B's live mutation unprotected and leave a third process free to
        // acquire concurrently.
        let temp = tempfile::tempdir().unwrap();
        let abandoned = LockHolder {
            pid: 99_999,
            acquired_at: clock().now(),
            operation: "abandoned".to_owned(),
            process_start: Some("whenever".to_owned()),
        };
        fs::write(
            temp.path().join(LOCK_FILE),
            serde_json::to_string(&abandoned).unwrap(),
        )
        .unwrap();

        let diagnosis = ProjectLock::diagnose(temp.path());
        assert!(matches!(&diagnosis, LockDiagnosis::Stale { .. }));

        // A legitimate successor — this process's own pid and real start
        // time — now occupies the same path.
        let successor = LockHolder {
            pid: process::id(),
            acquired_at: clock().now(),
            operation: "live successor".to_owned(),
            process_start: process_start_time(process::id()),
        };
        fs::write(
            temp.path().join(LOCK_FILE),
            serde_json::to_string(&successor).unwrap(),
        )
        .unwrap();

        assert!(
            !ProjectLock::clear_stale(temp.path(), &diagnosis).unwrap(),
            "a stale verdict about one lock must not clear a different one"
        );
        assert!(
            ProjectLock::is_held(temp.path()),
            "the successor's lock must survive"
        );
    }

    #[test]
    fn a_holder_that_finishes_during_the_liveness_check_is_not_reported_stale() {
        // The exact race from issue #78: `diagnose` reads the holder, then
        // spends milliseconds inside `ps`. If the real holder finishes and
        // its `Drop` removes the lock file inside that window, `ps`
        // truthfully reports the process gone — but by the time the answer
        // comes back, the file it was gone *from* no longer exists to be
        // diagnosed as `Stale`. Before this fix, `diagnose` never looked
        // again and reported `Stale` anyway, sending a healthy operator to
        // run `project recover --resume`.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(LOCK_FILE);
        let holder = LockHolder {
            pid: 12_345,
            acquired_at: clock().now(),
            operation: "finishing".to_owned(),
            process_start: Some("whenever".to_owned()),
        };
        fs::write(&path, serde_json::to_string(&holder).unwrap()).unwrap();

        let lock_path = path.clone();
        let liveness = move |_pid: u32| {
            fs::remove_file(&lock_path).unwrap();
            Liveness::Gone
        };

        let diagnosis = ProjectLock::diagnose_with(temp.path(), &liveness);
        assert_eq!(diagnosis, LockDiagnosis::Free);
    }

    #[test]
    fn a_lock_replaced_during_inspection_is_diagnosed_against_the_new_holder() {
        // Not the finishing race above: here the lock does not empty out, it
        // changes hands. The first holder's liveness check must not be
        // trusted to describe whoever holds the lock by the time that check
        // returns.
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(LOCK_FILE);
        let original = LockHolder {
            pid: 11_111,
            acquired_at: clock().now(),
            operation: "original".to_owned(),
            process_start: Some("original start".to_owned()),
        };
        fs::write(&path, serde_json::to_string(&original).unwrap()).unwrap();

        let replacement = LockHolder {
            pid: 22_222,
            acquired_at: clock().now(),
            operation: "replacement".to_owned(),
            process_start: Some("replacement start".to_owned()),
        };

        let original_pid = original.pid;
        let lock_path = path.clone();
        let injected = replacement.clone();
        let liveness = move |pid: u32| {
            if pid == original_pid {
                fs::write(&lock_path, serde_json::to_string(&injected).unwrap()).unwrap();
                Liveness::Gone
            } else {
                Liveness::Running(injected.process_start.clone().unwrap())
            }
        };

        let diagnosis = ProjectLock::diagnose_with(temp.path(), &liveness);
        let LockDiagnosis::Held(held) = diagnosis else {
            panic!("expected the replacement holder to be held, got {diagnosis:?}");
        };
        assert_eq!(held, replacement);
    }

    #[test]
    fn a_lock_that_keeps_changing_under_inspection_is_held_not_stale() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(LOCK_FILE);
        let initial = LockHolder {
            pid: 0,
            acquired_at: clock().now(),
            operation: "generation 0".to_owned(),
            process_start: Some("start 0".to_owned()),
        };
        fs::write(&path, serde_json::to_string(&initial).unwrap()).unwrap();

        let lock_path = path.clone();
        let calls = std::cell::Cell::new(0u32);
        let liveness = |_pid: u32| {
            let generation = calls.get() + 1;
            calls.set(generation);
            let next = LockHolder {
                pid: generation,
                acquired_at: clock().now(),
                operation: format!("generation {generation}"),
                process_start: Some(format!("start {generation}")),
            };
            fs::write(&lock_path, serde_json::to_string(&next).unwrap()).unwrap();
            Liveness::Gone
        };

        let diagnosis = ProjectLock::diagnose_with(temp.path(), &liveness);
        assert_eq!(
            calls.get(),
            2,
            "must consult liveness exactly twice, never loop"
        );

        // A lock that changed on both rereads was written inside the
        // inspection window each time, so its holder is milliseconds old,
        // not abandoned. Reporting `Ambiguous` here would map to
        // `PolicyLockAmbiguous`, which no retry loop in `gate.rs` covers —
        // it would fail a command that only lost a race to a busy lock,
        // without retry, which is #78's defect again at higher contention.
        // `Held` is both the truthful answer and the retryable one.
        let LockDiagnosis::Held(held) = diagnosis else {
            panic!("expected held, got {diagnosis:?}");
        };
        let last_generation = calls.get();
        assert_eq!(
            held,
            LockHolder {
                pid: last_generation,
                acquired_at: clock().now(),
                operation: format!("generation {last_generation}"),
                process_start: Some(format!("start {last_generation}")),
            },
            "must be held by the last generation observed, not the first"
        );
    }
}
