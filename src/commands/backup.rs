//! Creating and verifying backups of the repositories that hold the record.
//!
//! Two repositories matter here. The authority owns the protected ref, and the
//! control repository owns every card, review, receipt, and decision — losing
//! the second means the history of *why* the first looks the way it does is
//! gone, which is the part that cannot be reconstructed from the code.

use clap::{Args, Subcommand};

use crate::{
    cli::output::CommandOutcome,
    commands::CONTROL_ENV,
    control::repository::ControlRepository,
    domain::clock::Clock,
    error::HarnessError,
    git::backup::{
        BundleReport, create_bundle, fsck, has_refs_matching, require_independent, verify_bundle,
    },
};

/// Subcommands under `backup`.
#[derive(Debug, Subcommand)]
pub enum BackupCommand {
    /// Write a verified backup of the authority and control repositories.
    Create(CreateArgs),
    /// Re-verify backups that were written earlier.
    Verify(CreateArgs),
}

impl BackupCommand {
    /// Its dotted command path, as the result envelope reports it.
    ///
    /// The error envelope used to carry only the group — `backup` — while a
    /// success carried the full path, so a consumer matching on `command` got a
    /// different granularity depending on whether the command worked.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        match self {
            Self::Create(..) => "backup.create",
            Self::Verify(..) => "backup.verify",
        }
    }
}

/// Arguments accepted by `backup create` and `backup verify`.
#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Path to the control repository.
    #[arg(long, env = CONTROL_ENV)]
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

/// The archive namespace, as `git for-each-ref` matches it.
///
/// A trailing slash rather than `refs/archive/*`, because `for-each-ref` uses
/// fnmatch and its `*` does not cross a `/` — so the glob form silently matches
/// nothing while looking correct. `git bundle`'s `--glob` is rev-list globbing,
/// where `*` does cross, so the two spellings below are deliberately different.
const ARCHIVE_REF_PREFIX: &str = "refs/archive/";

/// One thing worth backing up: where it lives and which refs it contributes.
struct Subject {
    name: &'static str,
    source: std::path::PathBuf,
    revisions: &'static [&'static str],
}

/// What a backup covers.
///
/// The authority owns the protected ref and control owns every record, and for
/// a long time those were the whole list. They are not: `archive close` deletes
/// card branches and worktrees, and the only thing that justifies it is the
/// archive refs keeping those commits reachable. Those refs live in the
/// candidate repository, so the evidence authorizing the deletion was in no
/// backup at all — and the test named
/// `a_backup_contains_every_archive_ref_the_source_holds` checked the authority
/// bundle for `refs/heads/main` and never looked at an archive ref.
///
/// The candidate is bundled selectively rather than whole. Its working history
/// is the operator's own repository, backed up however they back up source; the
/// archive namespace is the part only this harness knows to keep.
fn subjects(config: &crate::config::ProjectConfig) -> [Subject; 3] {
    [
        Subject {
            name: "authority",
            source: config.authority_repository.clone(),
            revisions: &["--all"],
        },
        Subject {
            name: "control",
            source: config.control_repository.clone(),
            revisions: &["--all"],
        },
        Subject {
            name: "archive",
            source: config.repository.clone(),
            revisions: &["--glob=refs/archive/*"],
        },
    ]
}

/// Whether this subject currently has anything to bundle.
fn has_content(subject: &Subject) -> Result<bool, HarnessError> {
    if subject.revisions == ["--all"] {
        return Ok(true);
    }
    has_refs_matching(&subject.source, ARCHIVE_REF_PREFIX)
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
        for subject in subjects(&config) {
            if !has_content(&subject)? {
                continue;
            }
            let weakness = check_destination(args, &subject.source)?;
            planned.push(serde_json::json!({
                "subject": subject.name,
                "source": subject.source,
                "bundle": args.destination.join(format!("{}.bundle", subject.name)),
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
    for subject in subjects(&config) {
        // A project that has archived nothing yet has no archive bundle to
        // write, and `git bundle create` refuses an empty one. The skip is
        // recorded rather than silent, and `verify` re-derives the same
        // condition, so an absent bundle is only ever accepted when there is
        // genuinely nothing it should have held.
        if !has_content(&subject)? {
            warnings.push(format!(
                "{} holds no {ARCHIVE_REF_PREFIX} refs yet; no {} bundle was written",
                subject.source.display(),
                subject.name
            ));
            continue;
        }
        if let Some(weakness) = check_destination(args, &subject.source)? {
            warnings.push(weakness);
        }
        // The source is checked before it is copied. Bundling a repository that
        // is already damaged produces a backup of the damage.
        fsck(&subject.source)?;

        let bundle = args.destination.join(format!("{}.bundle", subject.name));
        create_bundle(&subject.source, &bundle, subject.revisions)?;
        // Verified immediately, by reading the bundle back. An unverified
        // backup is the thing this command exists to stop producing.
        let report = verify_bundle(&subject.source, &bundle)?;
        reports.push((subject.name, report));
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
    for subject in subjects(&config) {
        let bundle = args.destination.join(format!("{}.bundle", subject.name));
        if !bundle.exists() {
            // Absent is acceptable only when the source holds nothing this
            // bundle would have carried. Anything else is a missing backup.
            if !has_content(&subject)? {
                continue;
            }
            return Err(HarnessError::Control {
                reason: format!("no {} backup at {}", subject.name, bundle.display()),
                code: crate::error::ErrorCode::PreconditionNotFound,
            });
        }
        reports.push((subject.name, verify_bundle(&subject.source, &bundle)?));
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
