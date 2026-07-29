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
        journal::{Journal, OperationState},
        lock::ProjectLock,
        repository::ControlRepository,
    },
    domain::clock::Clock,
    error::HarnessError,
};

/// Runs a mutating command inside the lock and the journal.
///
/// The closure receives the control repository, an event store, and the control
/// head the operation started from, which it passes back to `commit` as the
/// compare-and-swap expectation.
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
    ) -> Result<CommandOutcome, HarnessError>,
{
    let control = ControlRepository::open(control_path)?;
    let _lock = ProjectLock::acquire(control.root(), command_name, clock)?;
    let journal = Journal::new(&control);
    journal.require_settled()?;

    let expected_head = control.head()?;
    let mut operation = journal.begin(command_name, expected_head.clone(), clock)?;
    let events = EventStore::new(&control);

    match body(&control, &events, expected_head.as_deref()) {
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
            let state = if error.category() == ExitCategory::RecoveryRequired {
                OperationState::FailedPartial
            } else if control.is_clean().unwrap_or(false) {
                OperationState::FailedClean
            } else {
                OperationState::FailedPartial
            };
            journal.finish(&mut operation, state, Some(error.to_string()), clock)?;
            Err(error)
        }
    }
}
