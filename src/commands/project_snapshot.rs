//! `project snapshot` command adapter.

use std::{io::Write, thread, time::Duration};

use crate::{
    cli::output::CommandOutcome,
    commands::project::{DEFAULT_WATCH_INTERVAL_MS, SnapshotArgs},
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
    if args.watch {
        return Err(HarnessError::ConflictingOptions(
            "`project snapshot --watch` is rendered by the CLI watch loop; use the command-line binary"
                .to_owned(),
        ));
    }
    if args.interval_ms.is_some() {
        return Err(HarnessError::ConflictingOptions(
            "`--interval-ms` requires `--watch`".to_owned(),
        ));
    }
    run_frame(args, clock)
}

/// Collects and renders one frame through the approved snapshot path.
fn run_frame(args: &SnapshotArgs, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let snapshot = ProjectSnapshot::collect(&control, clock)?;
    let data = serde_json::to_value(&snapshot)?;
    Ok(
        CommandOutcome::new("project.snapshot", snapshot.to_text(), data)
            .with_project(snapshot.project_id.parse()?),
    )
}

/// The reason a watch loop stopped without an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchTermination {
    /// stdout was not a TTY, so exactly one frame was emitted.
    NonTtyAfterOneFrame,
    /// stdout closed, normally because a pipe consumer stopped reading.
    OutputClosed,
}

/// Collects and renders snapshots repeatedly for an interactive human.
///
/// JSON is refused before the control repository is opened. For non-TTY
/// output, one plain frame is emitted and the command exits; this makes
/// capture and piping deterministic and prevents escape sequences from
/// appearing in logs. A TTY receives the same rendered frame after each
/// collection, with a clear-and-home sequence only between frames.
///
/// Ctrl-C is deliberately left to the operating system's normal SIGINT
/// disposition: the process stops immediately without manufacturing a
/// harness error. Broken pipes are treated as normal output termination for
/// the same reason.
///
/// # Errors
///
/// Returns a snapshot or rendering error if the control repository cannot be
/// read consistently or the frame cannot be serialized.
pub fn run_watch<W: Write>(
    args: &SnapshotArgs,
    format: crate::cli::output::OutputFormat,
    clock: &dyn Clock,
    writer: &mut W,
    stdout_is_terminal: bool,
) -> Result<WatchTermination, HarnessError> {
    if format == crate::cli::output::OutputFormat::Json {
        return Err(HarnessError::ConflictingOptions(
            "`--watch` cannot be combined with `--output json`; omit `--watch` for one JSON snapshot"
                .to_owned(),
        ));
    }
    if args.interval_ms.is_some() && !args.watch {
        return Err(HarnessError::ConflictingOptions(
            "`--interval-ms` requires `--watch`".to_owned(),
        ));
    }

    let interval = Duration::from_millis(args.interval_ms.unwrap_or(DEFAULT_WATCH_INTERVAL_MS));
    let mut first_frame = true;
    loop {
        let frame = run_frame(args, clock)?.render(format)?;
        if !first_frame && stdout_is_terminal && writer.write_all(b"\x1b[2J\x1b[H").is_err() {
            return Ok(WatchTermination::OutputClosed);
        }
        if writer.write_all(frame.as_bytes()).is_err() || writer.write_all(b"\n").is_err() {
            return Ok(WatchTermination::OutputClosed);
        }
        if writer.flush().is_err() {
            return Ok(WatchTermination::OutputClosed);
        }

        if !stdout_is_terminal {
            return Ok(WatchTermination::NonTtyAfterOneFrame);
        }
        first_frame = false;
        thread::sleep(interval);
    }
}
