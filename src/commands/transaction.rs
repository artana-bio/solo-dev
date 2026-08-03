//! The shared shape of a mutating command.
//!
//! Every command that changes authoritative state does the same four things:
//! take the project lock, refuse to proceed over an unresolved operation, open
//! a journal entry, and close it with an honest terminal state. Centralizing
//! that means a new command cannot forget one of them.

use std::path::Path;

use crate::{
    cli::{exit::ExitCategory, output::CommandOutcome},
    control::{
        event_store::EventStore,
        journal::{Journal, OperationRecord, OperationState},
        lock::ProjectLock,
        repository::ControlRepository,
    },
    domain::clock::Clock,
    error::HarnessError,
};

/// Records named boundaries within one operation.
///
/// Handed to the transaction body so a command can say where it got to. Every
/// step is written before the work it names, and any step can be made to fail
/// deliberately through [`crate::control::journal::INJECT_FAILURE_VAR`], which
/// is how `WP-500` reaches boundaries a natural interruption would rarely hit.
pub struct Steps<'a> {
    journal: &'a Journal<'a>,
    record: &'a mut OperationRecord,
}

impl Steps<'_> {
    /// Records that the operation reached a named boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the journal cannot be written, or when this step
    /// was named for deliberate interruption.
    pub fn at(&mut self, step: &str) -> Result<(), HarnessError> {
        self.journal.step(self.record, step)
    }

    /// Records a boundary whose mutation lands outside the control repository.
    ///
    /// A branch, a worktree, the authority: nothing the clean/partial decision
    /// can see by asking Git about control. Reaching one of these makes the
    /// operation partial by definition, whatever control looks like afterwards.
    /// The step is journaled before the mutation it names, so recording it
    /// first is what makes the flag safe.
    ///
    /// # Errors
    ///
    /// Returns an error when the journal cannot be written, or when this step
    /// was named for deliberate interruption.
    pub fn outside_control(&mut self, step: &str) -> Result<(), HarnessError> {
        self.record.touched_outside_control = true;
        self.journal.step(self.record, step)
    }
}

/// Decides how a failed operation is recorded.
///
/// Extracted so it can be tested directly. It could not be tested through
/// `CHANGE_HARNESS_FAIL_AT`, because an injected failure carries
/// `RecoveryIncomplete` and so takes the first arm whatever the other two
/// inputs are — every injection test in the suite is blind to this decision,
/// which is how the `work start` case survived.
fn terminal_state(
    error: &HarnessError,
    control_is_clean: bool,
    touched_outside_control: bool,
) -> OperationState {
    // An error that asks for recovery is taken at its word: Section 13.6's
    // authority-promoted, local-sync-pending case reports it that way.
    if error.category() == ExitCategory::RecoveryRequired {
        return OperationState::FailedPartial;
    }
    // A branch, a worktree, or a moved authority is invisible to a clean
    // control repository. `work start` creates the first two before it writes
    // the lease, so this arm is the whole difference between recovery seeing a
    // half-made allocation and reporting that nothing is wrong.
    if touched_outside_control {
        return OperationState::FailedPartial;
    }
    // A failure that left the working tree clean wrote nothing, so it needs no
    // recovery. Recording the difference is what lets `project recover` stay
    // quiet about ordinary rejections.
    if control_is_clean {
        OperationState::FailedClean
    } else {
        OperationState::FailedPartial
    }
}

/// Runs a mutating command inside the lock and the journal.
///
/// The closure receives the control repository, an event store, the control
/// head the operation started from — which it passes back to `commit` as the
/// compare-and-swap expectation — and a [`Steps`] recorder for naming the
/// boundaries it passes.
///
/// # Errors
///
/// Propagates the closure's error after recording a terminal journal state, or
/// fails earlier if the lock is held or the journal is unsettled.
pub fn with_transaction<F>(
    control_path: &Path,
    command_name: &str,
    clock: &dyn Clock,
    body: F,
) -> Result<CommandOutcome, HarnessError>
where
    F: FnOnce(
        &ControlRepository,
        &EventStore<'_>,
        Option<&str>,
        &mut Steps<'_>,
    ) -> Result<CommandOutcome, HarnessError>,
{
    let control = ControlRepository::open(control_path)?;
    let _lock = ProjectLock::acquire(control.root(), command_name, clock)?;
    let journal = Journal::new(&control);
    journal.require_settled()?;
    control.validate_hygiene()?;

    let expected_head = control.head()?;
    let mut operation = journal.begin(command_name, expected_head.clone(), clock)?;
    let events = EventStore::new(&control);

    let outcome = {
        let mut steps = Steps {
            journal: &journal,
            record: &mut operation,
        };
        body(&control, &events, expected_head.as_deref(), &mut steps)
    };

    match outcome {
        Ok(outcome) => {
            journal.finish(&mut operation, OperationState::Completed, None, clock)?;
            Ok(outcome.with_operation(operation.operation_id.clone()))
        }
        Err(error) => {
            // A failure that left the working tree clean wrote nothing, so it
            // does not need recovery. Recording the difference is what lets
            // `project recover` stay quiet about ordinary rejections.
            //
            // The clean tree is not sufficient on its own: `integration
            // promote` mutates the authority repository, which this check
            // cannot see, so a failure after the protected branch moved would
            // look clean. An error that asks for recovery is taken at its
            // word. Section 13.6 requires exactly this for the
            // authority-promoted, local-sync-pending case.
            let state = terminal_state(
                &error,
                control.is_clean().unwrap_or(false),
                operation.touched_outside_control,
            );
            journal.finish(&mut operation, state, Some(error.to_string()), clock)?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn error(code: ErrorCode) -> HarnessError {
        HarnessError::Control {
            reason: "test".to_owned(),
            code,
        }
    }

    #[test]
    fn an_ordinary_rejection_that_wrote_nothing_is_clean() {
        // This is what keeps `project recover` quiet about a card that simply
        // failed a precondition.
        assert_eq!(
            terminal_state(&error(ErrorCode::PreconditionNotFound), true, false),
            OperationState::FailedClean
        );
    }

    #[test]
    fn a_boundary_outside_control_makes_a_clean_control_repository_irrelevant() {
        // Tier 2, defect 11. `work start` creates a branch, a worktree, a
        // worktree lock and a locator before it writes the lease, so a failure
        // in that stretch leaves control genuinely clean. The decision rested
        // on control alone, recorded `FailedClean`, and `recover` reported that
        // nothing was wrong — over a card holding a branch and a worktree it
        // has no lease for, whose branch name is taken so the command cannot be
        // retried.
        assert_eq!(
            terminal_state(&error(ErrorCode::PreconditionNotFound), true, true),
            OperationState::FailedPartial
        );
    }

    #[test]
    fn a_dirty_control_repository_is_still_partial_on_its_own() {
        assert_eq!(
            terminal_state(&error(ErrorCode::PreconditionNotFound), false, false),
            OperationState::FailedPartial
        );
    }

    #[test]
    fn an_error_asking_for_recovery_is_taken_at_its_word() {
        // Section 13.6: the authority moved and the local fast-forward did not.
        // Nothing the harness can inspect afterwards shows that.
        assert_eq!(
            terminal_state(&error(ErrorCode::RecoveryIncomplete), true, false),
            OperationState::FailedPartial
        );
    }

    #[test]
    fn injected_failures_cannot_exercise_this_decision() {
        // The reason defect 11 survived a suite with failure injection at every
        // journaled boundary. `CHANGE_HARNESS_FAIL_AT` raises
        // `RecoveryIncomplete`, which takes the first arm regardless — so every
        // injection test records `FailedPartial` and none of them can tell
        // whether the rest of the decision is right. Stated as a test so the
        // next person reaching for injection to cover this sees why it will
        // not work.
        for clean in [true, false] {
            for touched in [true, false] {
                assert_eq!(
                    terminal_state(&error(ErrorCode::RecoveryIncomplete), clean, touched),
                    OperationState::FailedPartial,
                );
            }
        }
    }
}
