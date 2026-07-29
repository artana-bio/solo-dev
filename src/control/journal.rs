//! The operation journal.
//!
//! Invariant 7.2.8: every mutating operation records enough state to resume or
//! safely diagnose an interruption. The journal is that record. It is written
//! before the mutation it describes, so a process killed at any point leaves
//! evidence of what it was part-way through.
//!
//! Journal entries are written atomically and are deliberately *not* committed
//! at every step. A journal entry describes work in flight; committing it would
//! imply the work is authoritative, which is exactly what it is not yet.

use std::{fs, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        clock::{Clock, Timestamp},
        ids::OperationId,
    },
    error::{ErrorCode, HarnessError},
};

use super::repository::ControlRepository;

/// Directory holding journal entries, relative to the control repository.
pub const JOURNAL_DIR: &str = "journal";

/// Schema identifier for a journal entry.
pub const JOURNAL_SCHEMA: &str = "harness.operation/v1";

/// How far a journaled operation progressed.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    /// The operation began and has not reported completion.
    Started,
    /// Every step completed and the operation is authoritative.
    Completed,
    /// The operation failed cleanly, leaving nothing partial behind.
    FailedClean,
    /// The operation failed after a partial mutation and needs recovery.
    FailedPartial,
}

impl OperationState {
    /// True when this state requires operator attention before further work.
    #[must_use]
    pub const fn needs_recovery(self) -> bool {
        matches!(self, Self::Started | Self::FailedPartial)
    }
}

/// Environment variable naming a journal step that must fail deliberately.
///
/// `WP-500` needs every journaled boundary to be interruptible on demand.
/// Waiting for a natural interruption tests whichever boundary happens to be
/// slow, which is not the same as testing all of them.
///
/// The affordance is compiled in rather than hidden behind a feature flag, so
/// the code under test is the code that ships. It is safe to leave in: it can
/// only cause a command to *fail* at a boundary it already journals, which is
/// a state the harness is required to handle, and it can never cause a silent
/// success or a partial write that recovery cannot see.
pub const INJECT_FAILURE_VAR: &str = "CHANGE_HARNESS_FAIL_AT";

/// Fails when the named step was selected for deliberate interruption.
fn check_injected_failure(step: &str) -> Result<(), HarnessError> {
    let Ok(target) = std::env::var(INJECT_FAILURE_VAR) else {
        return Ok(());
    };
    if target != step {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!(
            "deliberate interruption at journal step `{step}`, requested by {INJECT_FAILURE_VAR}"
        ),
        code: ErrorCode::RecoveryIncomplete,
    })
}

/// One journaled mutating operation.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperationRecord {
    /// Always [`JOURNAL_SCHEMA`].
    pub schema: String,
    /// Identifies this operation.
    pub operation_id: OperationId,
    /// The command that opened it, such as `project.init`.
    pub command: String,
    /// How far it progressed.
    pub state: OperationState,
    /// Named boundaries the operation passed, in order.
    ///
    /// Recovery reads these to decide what already happened, which is why each
    /// one is recorded before the step it names rather than after.
    pub steps: Vec<String>,
    /// Control commit the operation started from, if any.
    pub expected_control_head: Option<String>,
    /// When the operation began.
    pub started_at: Timestamp,
    /// When it reached a terminal state.
    pub finished_at: Option<Timestamp>,
    /// Why it failed, when it did.
    pub failure: Option<String>,
    /// Whether a boundary outside the control repository was reached.
    ///
    /// The clean/partial decision used to rest entirely on whether the control
    /// repository was dirty, which cannot see a branch, a worktree, or the
    /// authority. `work start` creates all of the first two before it writes
    /// the lease, so a failure in that stretch left control genuinely clean,
    /// journalled `FailedClean`, and `recover` reporting nothing was wrong —
    /// over a card holding a branch and a worktree it has no lease for, whose
    /// branch name is now taken so the command cannot be retried.
    ///
    /// Recorded here rather than derived, so recovery reads the same fact the
    /// decision was made on.
    #[serde(default)]
    pub touched_outside_control: bool,
}

impl OperationRecord {
    /// Relative path of this record inside the control repository.
    #[must_use]
    pub fn relative_path(operation_id: &OperationId) -> String {
        format!("{JOURNAL_DIR}/{operation_id}.json")
    }
}

/// Writes and reads journal entries.
#[derive(Debug)]
pub struct Journal<'a> {
    control: &'a ControlRepository,
}

impl<'a> Journal<'a> {
    /// Binds a journal to a control repository.
    #[must_use]
    pub const fn new(control: &'a ControlRepository) -> Self {
        Self { control }
    }

    /// Allocates the next operation identifier.
    ///
    /// Identifiers are dense and monotonic so the journal reads chronologically
    /// without consulting timestamps, which a clock change could reorder.
    ///
    /// # Errors
    ///
    /// Returns an error when the journal directory cannot be read.
    pub fn next_id(&self) -> Result<OperationId, HarnessError> {
        let directory = self.control.path(JOURNAL_DIR);
        let highest = if directory.exists() {
            let entries = fs::read_dir(&directory).map_err(|source| HarnessError::ControlIo {
                path: directory.clone(),
                source,
            })?;
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    entry
                        .path()
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .and_then(|stem| stem.strip_prefix("OP-"))
                        .and_then(|digits| digits.parse::<u64>().ok())
                })
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        format!("OP-{:06}", highest + 1).parse()
    }

    /// Opens a new operation, writing its record before any mutation occurs.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be written.
    pub fn begin(
        &self,
        command: &str,
        expected_control_head: Option<String>,
        clock: &dyn Clock,
    ) -> Result<OperationRecord, HarnessError> {
        let record = OperationRecord {
            schema: JOURNAL_SCHEMA.to_owned(),
            operation_id: self.next_id()?,
            command: command.to_owned(),
            state: OperationState::Started,
            steps: Vec::new(),
            expected_control_head,
            touched_outside_control: false,
            started_at: clock.now(),
            finished_at: None,
            failure: None,
        };
        self.write(&record)?;
        Ok(record)
    }

    /// Records that an operation reached a named boundary.
    ///
    /// The step is written *before* the mutation it names, so an interruption
    /// is attributable to a boundary rather than guessed at. That ordering is
    /// also what makes [`INJECT_FAILURE_VAR`] useful: a failure injected here
    /// lands after the boundary was recorded and before its work happened,
    /// which is the hardest case for recovery to get right.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be written, or when this step
    /// was named for deliberate failure.
    pub fn step(&self, record: &mut OperationRecord, step: &str) -> Result<(), HarnessError> {
        record.steps.push(step.to_owned());
        self.write(record)?;
        check_injected_failure(step)
    }

    /// Marks an operation terminal.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be written.
    pub fn finish(
        &self,
        record: &mut OperationRecord,
        state: OperationState,
        failure: Option<String>,
        clock: &dyn Clock,
    ) -> Result<(), HarnessError> {
        record.state = state;
        record.failure = failure;
        record.finished_at = Some(clock.now());
        self.write(record)
    }

    /// Writes one record atomically.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization or the write fails.
    pub fn write(&self, record: &OperationRecord) -> Result<(), HarnessError> {
        self.control.write_atomic(
            &OperationRecord::relative_path(&record.operation_id),
            &format!("{}\n", serde_json::to_string_pretty(record)?),
        )
    }

    /// Reads one record.
    ///
    /// # Errors
    ///
    /// Returns an error when the record is missing or malformed.
    pub fn read(&self, operation_id: &OperationId) -> Result<OperationRecord, HarnessError> {
        let relative = OperationRecord::relative_path(operation_id);
        let raw = self.control.read(&relative)?;
        serde_json::from_str(&raw).map_err(|source| HarnessError::Control {
            reason: format!("journal entry {operation_id} is malformed: {source}"),
            code: ErrorCode::InternalControlCorrupt,
        })
    }

    /// Every journal entry, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be read or an entry is
    /// malformed.
    pub fn all(&self) -> Result<Vec<OperationRecord>, HarnessError> {
        let directory: PathBuf = self.control.path(JOURNAL_DIR);
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&directory).map_err(|source| HarnessError::ControlIo {
            path: directory,
            source,
        })?;
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension()? == "json")
                    .then(|| path.file_stem()?.to_str().map(ToOwned::to_owned))?
            })
            .collect();
        names.sort();

        names
            .iter()
            .map(|name| {
                let id: OperationId = name.parse()?;
                self.read(&id)
            })
            .collect()
    }

    /// Entries that stopped before reaching a safe terminal state.
    ///
    /// # Errors
    ///
    /// Returns an error when the journal cannot be read.
    pub fn unresolved(&self) -> Result<Vec<OperationRecord>, HarnessError> {
        Ok(self
            .all()?
            .into_iter()
            .filter(|record| record.state.needs_recovery())
            .collect())
    }

    /// Fails when any operation is unresolved.
    ///
    /// Called before starting new work: proceeding over an unresolved mutation
    /// would build authoritative state on top of an unknown partial state.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::RecoveryIncomplete`] when work is outstanding.
    pub fn require_settled(&self) -> Result<(), HarnessError> {
        let unresolved = self.unresolved()?;
        if unresolved.is_empty() {
            return Ok(());
        }
        let names: Vec<String> = unresolved
            .iter()
            .map(|record| format!("{} ({})", record.operation_id, record.command))
            .collect();
        Err(HarnessError::Control {
            reason: format!("unresolved operations block new work: {}", names.join(", ")),
            code: ErrorCode::RecoveryIncomplete,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::FixedClock;

    fn clock() -> FixedClock {
        FixedClock::at_unix_seconds(1_785_196_800).unwrap()
    }

    fn control() -> (tempfile::TempDir, ControlRepository) {
        let temp = tempfile::tempdir().unwrap();
        let control = ControlRepository::at(temp.path());
        control.initialize_git().unwrap();
        (temp, control)
    }

    #[test]
    fn identifiers_are_dense_and_monotonic() {
        let (_temp, control) = control();
        let journal = Journal::new(&control);
        assert_eq!(journal.next_id().unwrap().as_str(), "OP-000001");

        let first = journal.begin("project.init", None, &clock()).unwrap();
        assert_eq!(first.operation_id.as_str(), "OP-000001");
        assert_eq!(journal.next_id().unwrap().as_str(), "OP-000002");
    }

    #[test]
    fn a_record_is_written_before_any_mutation_and_is_readable() {
        let (_temp, control) = control();
        let journal = Journal::new(&control);
        let record = journal
            .begin("project.init", Some("abc".into()), &clock())
            .unwrap();

        let read_back = journal.read(&record.operation_id).unwrap();
        assert_eq!(read_back, record);
        assert_eq!(read_back.state, OperationState::Started);
        assert_eq!(read_back.expected_control_head.as_deref(), Some("abc"));
        assert!(read_back.finished_at.is_none());
    }

    #[test]
    fn steps_accumulate_in_order() {
        let (_temp, control) = control();
        let journal = Journal::new(&control);
        let mut record = journal.begin("project.init", None, &clock()).unwrap();
        journal.step(&mut record, "git-initialized").unwrap();
        journal.step(&mut record, "project-written").unwrap();

        let read_back = journal.read(&record.operation_id).unwrap();
        assert_eq!(read_back.steps, vec!["git-initialized", "project-written"]);
    }

    #[test]
    fn a_started_operation_needs_recovery_and_blocks_new_work() {
        let (_temp, control) = control();
        let journal = Journal::new(&control);
        let _record = journal.begin("project.init", None, &clock()).unwrap();

        assert_eq!(journal.unresolved().unwrap().len(), 1);
        let error = journal.require_settled().expect_err("must block");
        assert_eq!(error.code(), ErrorCode::RecoveryIncomplete);
        assert_eq!(
            error.category(),
            crate::cli::exit::ExitCategory::RecoveryRequired
        );
    }

    #[test]
    fn a_completed_operation_does_not_block_new_work() {
        let (_temp, control) = control();
        let journal = Journal::new(&control);
        let mut record = journal.begin("project.init", None, &clock()).unwrap();
        journal
            .finish(&mut record, OperationState::Completed, None, &clock())
            .unwrap();

        assert!(journal.unresolved().unwrap().is_empty());
        journal.require_settled().expect("settled journal");
        assert!(
            journal
                .read(&record.operation_id)
                .unwrap()
                .finished_at
                .is_some()
        );
    }

    #[test]
    fn a_clean_failure_does_not_block_but_a_partial_one_does() {
        let (_temp, control) = control();
        let journal = Journal::new(&control);

        let mut clean = journal.begin("project.init", None, &clock()).unwrap();
        journal
            .finish(
                &mut clean,
                OperationState::FailedClean,
                Some("validation failed".into()),
                &clock(),
            )
            .unwrap();
        journal
            .require_settled()
            .expect("a clean failure left nothing behind");

        let mut partial = journal.begin("project.init", None, &clock()).unwrap();
        journal
            .finish(
                &mut partial,
                OperationState::FailedPartial,
                Some("interrupted after git init".into()),
                &clock(),
            )
            .unwrap();
        assert!(journal.require_settled().is_err());
    }

    #[test]
    fn all_returns_entries_oldest_first() {
        let (_temp, control) = control();
        let journal = Journal::new(&control);
        for _ in 0..3 {
            let mut record = journal.begin("project.init", None, &clock()).unwrap();
            journal
                .finish(&mut record, OperationState::Completed, None, &clock())
                .unwrap();
        }
        let ids: Vec<String> = journal
            .all()
            .unwrap()
            .iter()
            .map(|record| record.operation_id.to_string())
            .collect();
        assert_eq!(ids, vec!["OP-000001", "OP-000002", "OP-000003"]);
    }

    #[test]
    fn an_empty_journal_is_settled() {
        let (_temp, control) = control();
        let journal = Journal::new(&control);
        assert!(journal.all().unwrap().is_empty());
        journal.require_settled().expect("nothing has run yet");
    }

    #[test]
    fn a_malformed_entry_is_reported_as_corruption_not_ignored() {
        let (_temp, control) = control();
        let journal = Journal::new(&control);
        control
            .write_atomic(&format!("{JOURNAL_DIR}/OP-000001.json"), "not json\n")
            .unwrap();

        let error = journal.all().expect_err("must fail loudly");
        assert_eq!(error.code(), ErrorCode::InternalControlCorrupt);
    }
}
