//! Creating and verifying backups of the repositories that hold the record.
//!
//! Two repositories matter here. The authority owns the protected ref, and the
//! control repository owns every card, review, receipt, and decision — losing
//! the second means the history of *why* the first looks the way it does is
//! gone, which is the part that cannot be reconstructed from the code.

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    control::repository::ControlRepository,
    domain::clock::Clock,
    error::HarnessError,
    git::backup::{BundleReport, create_bundle, fsck, require_independent, verify_bundle},
};

/// Subcommands under `backup`.
#[derive(Debug, Subcommand)]
pub enum BackupCommand {
    /// Write a verified backup of the authority and control repositories.
    Create(CreateArgs),
    /// Re-verify backups that were written earlier.
    Verify(CreateArgs),
}

/// Arguments accepted by `backup create` and `backup verify`.
#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Path to the control repository.
    #[arg(long)]
    pub control: std::path::PathBuf,
    /// Directory the bundles are written to. Must be on a different device.
    #[arg(long)]
    pub destination: std::path::PathBuf,
    /// Proceed even when the destination shares a device with its source.
    ///
    /// Exists because a developer laptop often has one disk, and refusing
    /// outright would push people to skip backups rather than take a weak one.
    /// The weakness is named in the result either way.
    #[arg(long)]
    pub allow_same_device: bool,
    /// Report without writing or reading anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Executes a `backup` subcommand.
///
/// # Errors
///
/// Returns a policy error when the destination is not independent or a backup
/// fails verification.
pub fn execute(command: &BackupCommand, clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    match command {
        BackupCommand::Create(args) => run_create(args, clock),
        BackupCommand::Verify(args) => run_verify(args),
    }
}

/// The two repositories worth backing up, with the bundle name each gets.
fn subjects(config: &crate::config::ProjectConfig) -> [(&'static str, std::path::PathBuf); 2] {
    [
        ("authority", config.authority_repository.clone()),
        ("control", config.control_repository.clone()),
    ]
}

/// Checks independence unless the caller accepted the weaker guarantee.
fn check_destination(
    args: &CreateArgs,
    source: &std::path::Path,
) -> Result<Option<String>, HarnessError> {
    match require_independent(source, &args.destination) {
        Ok(()) => Ok(None),
        Err(error) if args.allow_same_device => Ok(Some(error.to_string())),
        Err(error) => Err(error),
    }
}

fn run_create(args: &CreateArgs, _clock: &dyn Clock) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;

    if args.dry_run {
        let mut planned = Vec::new();
        for (name, source) in subjects(&config) {
            let weakness = check_destination(args, &source)?;
            planned.push(serde_json::json!({
                "subject": name,
                "source": source,
                "bundle": args.destination.join(format!("{name}.bundle")),
                "independence_warning": weakness,
            }));
        }
        return Ok(CommandOutcome::new(
            "backup.create",
            format!(
                "Dry run: would write {} bundle(s) to {}\nnothing was changed",
                planned.len(),
                args.destination.display()
            ),
            serde_json::json!({ "dry_run": true, "planned": planned }),
        )
        .with_project(config.project_id));
    }

    let mut reports = Vec::new();
    let mut warnings = Vec::new();
    for (name, source) in subjects(&config) {
        if let Some(weakness) = check_destination(args, &source)? {
            warnings.push(weakness);
        }
        // The source is checked before it is copied. Bundling a repository that
        // is already damaged produces a backup of the damage.
        fsck(&source)?;

        let bundle = args.destination.join(format!("{name}.bundle"));
        create_bundle(&source, &bundle)?;
        // Verified immediately, by reading the bundle back. An unverified
        // backup is the thing this command exists to stop producing.
        let report = verify_bundle(&source, &bundle)?;
        reports.push((name, report));
    }

    Ok(report_backups(
        "backup.create",
        &reports,
        &warnings,
        &config,
    ))
}

fn run_verify(args: &CreateArgs) -> Result<CommandOutcome, HarnessError> {
    let control = ControlRepository::open(&args.control)?;
    let config = control.project()?;

    let mut reports = Vec::new();
    for (name, source) in subjects(&config) {
        let bundle = args.destination.join(format!("{name}.bundle"));
        if !bundle.exists() {
            return Err(HarnessError::Control {
                reason: format!("no {name} backup at {}", bundle.display()),
                code: crate::error::ErrorCode::PreconditionNotFound,
            });
        }
        reports.push((name, verify_bundle(&source, &bundle)?));
    }

    Ok(report_backups("backup.verify", &reports, &[], &config))
}

/// Turns verified bundles into the command's outcome.
fn report_backups(
    command: &str,
    reports: &[(&str, BundleReport)],
    warnings: &[String],
    config: &crate::config::ProjectConfig,
) -> CommandOutcome {
    let mut text = format!("{} backup(s) verified", reports.len());
    for (name, report) in reports {
        let _ = std::fmt::Write::write_fmt(
            &mut text,
            format_args!(
                "\n  {name}: {} ({} bytes, {} ref(s))\n    {}",
                report.path.display(),
                report.bytes,
                report.refs.len(),
                report.digest
            ),
        );
    }

    let mut outcome = CommandOutcome::new(
        command,
        text,
        serde_json::json!({
            "backups": reports.iter().map(|(name, report)| serde_json::json!({
                "subject": name,
                "path": report.path,
                "bytes": report.bytes,
                "refs": report.refs,
                "digest": report.digest,
            })).collect::<Vec<_>>(),
        }),
    )
    .with_project(config.project_id.clone());
    for warning in warnings {
        outcome = outcome.with_warning(warning.clone());
    }
    outcome
}
