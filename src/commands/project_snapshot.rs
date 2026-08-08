//! `project snapshot` command adapter.

use crate::{
    cli::output::CommandOutcome,
    commands::project::SnapshotArgs,
    control::repository::ControlRepository,
    domain::{clock::Clock, project_snapshot::ProjectSnapshot},
    error::HarnessError,
};

/// Collects one typed snapshot and renders both command views from it.
///
/// # Errors
///
/// Returns an error when the control repository cannot be opened or its
/// captured records are malformed or inconsistent.
pub fn run(args: &SnapshotArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let snapshot = ProjectSnapshot::collect(&control, clock)?;
    let data = serde_json::to_value(&snapshot)?;
    Ok(
        CommandOutcome::new("project.snapshot", snapshot.to_text(), data)
            .with_project(snapshot.project_id.parse()?),
    )
}
