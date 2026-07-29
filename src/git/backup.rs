//! Backups that can be verified rather than assumed.
//!
//! A backup nobody has read is a belief, not a backup. Everything here is built
//! so the belief is checkable: the bundle is verified by reading the bundle, the
//! refs are listed from the bundle, and a restore drill reconstructs a working
//! repository from the file alone with the source unavailable.
//!
//! The independence check exists for the same reason. A copy on the same device
//! survives an accidental `rm` and nothing else — not the disk failure, not the
//! filesystem corruption, not the laptop going in a river. Calling that a backup
//! is the failure mode, because it stops people from making a real one.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{
    error::{ErrorCode, HarnessError},
    git::command::{GitScope, run, run_ok},
};

/// What a backup destination turned out to be.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Independence {
    /// Source and destination are on different devices.
    Independent,
    /// Both are on the same device, so one failure takes both.
    SameDevice,
    /// The device could not be established for one or both paths.
    Unknown,
}

/// One verified backup of one repository.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BundleReport {
    /// Where the bundle was written.
    pub path: std::path::PathBuf,
    /// Its size in bytes.
    pub bytes: u64,
    /// Every ref the bundle carries, read back from the bundle itself.
    pub refs: Vec<String>,
    /// SHA-256 of the bundle file.
    pub digest: String,
}

/// Reports whether two paths sit on different devices.
///
/// Uses `df` rather than a filesystem crate: the crate forbids unsafe code, and
/// this needs the mount device, which is what `df` reports on every supported
/// host. An unreadable answer is `Unknown` rather than `Independent`, so an
/// unverifiable destination is never silently accepted as a good one.
#[must_use]
pub fn independence(source: &Path, destination: &Path) -> Independence {
    let device = |path: &Path| -> Option<String> {
        // The destination usually does not exist yet, so the nearest existing
        // ancestor is what determines the device it will land on.
        let mut probe = path;
        loop {
            if probe.exists() {
                break;
            }
            probe = probe.parent()?;
        }
        let output = std::process::Command::new("df")
            .env("LC_ALL", "C")
            .arg("-P")
            .arg(probe)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().nth(1)?;
        line.split_whitespace().next().map(ToOwned::to_owned)
    };

    match (device(source), device(destination)) {
        (Some(left), Some(right)) if left != right => Independence::Independent,
        (Some(_), Some(_)) => Independence::SameDevice,
        _ => Independence::Unknown,
    }
}

/// Refuses a destination that would not survive what it is protecting against.
///
/// # Errors
///
/// Returns a policy error when the destination shares a device with the source,
/// or when independence could not be established.
pub fn require_independent(source: &Path, destination: &Path) -> Result<(), HarnessError> {
    match independence(source, destination) {
        Independence::Independent => Ok(()),
        Independence::SameDevice => Err(HarnessError::Control {
            reason: format!(
                "{} is on the same device as {}; a copy there survives an accidental deletion and nothing else",
                destination.display(),
                source.display()
            ),
            code: ErrorCode::PolicyBackupNotIndependent,
        }),
        Independence::Unknown => Err(HarnessError::Control {
            reason: format!(
                "could not establish whether {} is on a different device from {}",
                destination.display(),
                source.display()
            ),
            code: ErrorCode::PolicyBackupNotIndependent,
        }),
    }
}

/// Writes a bundle of every ref in a repository.
///
/// # Errors
///
/// Returns an external-tool error when Git refuses, or an I/O error when the
/// destination directory cannot be created.
pub fn create_bundle(
    repository: &Path,
    destination: &Path,
    revisions: &[&str],
) -> Result<(), HarnessError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|source| HarnessError::ControlIo {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "bundle".as_ref(),
        "create".as_ref(),
        destination.as_os_str(),
    ];
    args.extend(
        revisions
            .iter()
            .map(|revision| std::ffi::OsStr::new(*revision)),
    );
    run_ok(&GitScope::work_tree(repository), args)?;
    Ok(())
}

/// Whether a repository holds any ref matching a glob.
///
/// `git bundle create` refuses to write an empty bundle, so a selective backup
/// has to know whether there is anything to write before it asks.
///
/// # Errors
///
/// Returns an error when Git cannot be executed.
pub fn has_refs_matching(repository: &Path, glob: &str) -> Result<bool, HarnessError> {
    let listed = run_ok(
        &GitScope::work_tree(repository),
        ["for-each-ref", "--format=%(refname)", glob],
    )?;
    Ok(!listed.trimmed_stdout().is_empty())
}

/// Verifies a bundle by restoring it and checking what came out.
///
/// `git bundle verify` alone is not enough, and finding that out is the reason
/// this function looks the way it does. It validates the header and the
/// prerequisite commits, then reports success — a bundle truncated in the
/// middle of its pack passes it cleanly. A backup verifier that accepts a
/// half-written file is the exact failure this module exists to prevent.
///
/// So verification restores the bundle into a throwaway repository and runs
/// `fsck` over the result. That reads every object, and it makes "verified"
/// mean the thing an operator actually cares about: it restores. The cost is a
/// full read of the backup, which is the correct price for the claim.
///
/// # Errors
///
/// Returns a policy error when the bundle is unreadable, fails verification, or
/// cannot be restored.
pub fn verify_bundle(repository: &Path, bundle: &Path) -> Result<BundleReport, HarnessError> {
    let scope = GitScope::work_tree(repository);
    let verified = run(
        &scope,
        [
            "bundle".as_ref(),
            "verify".as_ref(),
            "--quiet".as_ref(),
            bundle.as_os_str(),
        ],
    )?;
    if !verified.success() {
        return Err(HarnessError::Control {
            reason: format!(
                "backup {} did not verify: {}",
                bundle.display(),
                verified.diagnostic()
            ),
            code: ErrorCode::PolicyBackupCorrupt,
        });
    }

    // The part `bundle verify` does not do.
    let drill = tempfile::tempdir().map_err(|source| HarnessError::ControlIo {
        path: bundle.to_path_buf(),
        source,
    })?;
    let restored = drill.path().join("restored.git");
    restore_from_bundle(bundle, &restored).map_err(|error| HarnessError::Control {
        reason: format!(
            "backup {} verified its header but could not be restored: {error}",
            bundle.display()
        ),
        code: ErrorCode::PolicyBackupCorrupt,
    })?;
    fsck(&restored).map_err(|error| HarnessError::Control {
        reason: format!(
            "backup {} restored but failed integrity checking: {error}",
            bundle.display()
        ),
        code: ErrorCode::PolicyBackupCorrupt,
    })?;

    // Refs are listed from the restored copy, so the report describes what came
    // out rather than what the file claims to contain.
    let listed = run_ok(
        &GitScope::git_dir(&restored),
        ["for-each-ref", "--format=%(refname)"],
    )?;
    let refs: Vec<String> = listed
        .trimmed_stdout()
        .lines()
        .map(ToOwned::to_owned)
        .collect();

    let bytes = std::fs::metadata(bundle)
        .map_err(|source| HarnessError::ControlIo {
            path: bundle.to_path_buf(),
            source,
        })?
        .len();
    let contents = std::fs::read(bundle).map_err(|source| HarnessError::ControlIo {
        path: bundle.to_path_buf(),
        source,
    })?;

    Ok(BundleReport {
        path: bundle.to_path_buf(),
        bytes,
        refs,
        digest: crate::domain::digest::Digest::of_bytes(&contents)
            .as_str()
            .to_owned(),
    })
}

/// Runs `git fsck` against a repository.
///
/// # Errors
///
/// Returns a policy error naming what Git found.
pub fn fsck(repository: &Path) -> Result<(), HarnessError> {
    let output = run(
        &GitScope::work_tree(repository),
        ["fsck", "--no-progress", "--connectivity-only"],
    )?;
    if output.success() {
        return Ok(());
    }
    Err(HarnessError::Control {
        reason: format!(
            "{} failed integrity checking: {}",
            repository.display(),
            output.diagnostic()
        ),
        code: ErrorCode::PolicyBackupCorrupt,
    })
}

/// Reconstructs a repository from a bundle alone.
///
/// Takes no source repository on purpose. A drill that could reach the original
/// would not be a drill.
///
/// # Errors
///
/// Returns an external-tool error when the clone fails.
pub fn restore_from_bundle(bundle: &Path, destination: &Path) -> Result<(), HarnessError> {
    run_ok(
        &GitScope::None,
        [
            "clone".as_ref(),
            "--quiet".as_ref(),
            "--mirror".as_ref(),
            bundle.as_os_str(),
            destination.as_os_str(),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn repository() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main", "."],
            vec!["config", "user.email", "f@local.invalid"],
            vec!["config", "user.name", "Fixture"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&path)
                    .args(&args)
                    .output()
                    .expect("git should run")
                    .status
                    .success()
            );
        }
        fs::write(path.join("f.txt"), "base\n").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "base"]] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&path)
                    .args(&args)
                    .output()
                    .expect("git should run")
                    .status
                    .success()
            );
        }
        (temp, path)
    }

    #[test]
    fn a_sibling_directory_is_not_independent() {
        let (temp, repo) = repository();
        assert_eq!(
            independence(&repo, &temp.path().join("backup.bundle")),
            Independence::SameDevice
        );
        let error = require_independent(&repo, &temp.path().join("backup.bundle"))
            .expect_err("a same-device destination is refused");
        assert!(error.to_string().contains("same device"), "{error}");
    }

    #[test]
    fn a_bundle_carries_every_ref_and_verifies_from_the_file() {
        let (temp, repo) = repository();
        let bundle = temp.path().join("out").join("repo.bundle");
        create_bundle(&repo, &bundle, &["--all"]).expect("a bundle");

        let report = verify_bundle(&repo, &bundle).expect("verification");
        assert!(report.refs.iter().any(|name| name == "refs/heads/main"));
        assert!(report.bytes > 0);
        assert!(report.digest.starts_with("sha256:"));
    }

    #[test]
    fn a_corrupted_bundle_fails_verification() {
        let (temp, repo) = repository();
        let bundle = temp.path().join("repo.bundle");
        create_bundle(&repo, &bundle, &["--all"]).expect("a bundle");

        // Truncate the middle rather than the header, so the file still looks
        // like a bundle to anything that only checks the first bytes.
        let mut contents = fs::read(&bundle).unwrap();
        let midpoint = contents.len() / 2;
        contents.truncate(midpoint);
        fs::write(&bundle, &contents).unwrap();

        let error = verify_bundle(&repo, &bundle).expect_err("a truncated bundle must fail");
        // Asserted on the stable code rather than the prose, because which
        // stage catches it is an implementation detail: `bundle verify` passes
        // a truncated pack, and the restore is what actually notices.
        assert_eq!(error.code(), ErrorCode::PolicyBackupCorrupt);
        assert!(
            error.to_string().contains("repo.bundle"),
            "the failing backup must be named: {error}"
        );
    }

    #[test]
    fn git_bundle_verify_alone_would_accept_a_truncated_bundle() {
        // The reason `verify_bundle` restores rather than trusting Git's own
        // verify. If this ever starts failing, Git got stricter and the
        // restore step could be reconsidered — but not before.
        let (temp, repo) = repository();
        let bundle = temp.path().join("repo.bundle");
        create_bundle(&repo, &bundle, &["--all"]).expect("a bundle");
        let mut contents = fs::read(&bundle).unwrap();
        contents.truncate(contents.len() / 2);
        fs::write(&bundle, &contents).unwrap();

        let shallow = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["bundle", "verify", "--quiet"])
            .arg(&bundle)
            .output()
            .expect("git should run");
        assert!(
            shallow.status.success(),
            "git bundle verify is expected to accept this; if it now rejects it, revisit the restore step"
        );
    }

    #[test]
    fn verification_reads_the_bundle_and_not_the_source() {
        // The failure that matters: a verifier that consulted the source would
        // pass a bundle of zeroes, because the source is perfectly fine.
        let (temp, repo) = repository();
        let bundle = temp.path().join("repo.bundle");
        create_bundle(&repo, &bundle, &["--all"]).expect("a bundle");
        fs::write(&bundle, vec![0u8; 4096]).unwrap();

        verify_bundle(&repo, &bundle).expect_err("a bundle of zeroes must fail");
    }

    #[test]
    fn a_restore_drill_reconstructs_the_repository_from_the_bundle_alone() {
        let (temp, repo) = repository();
        let bundle = temp.path().join("repo.bundle");
        create_bundle(&repo, &bundle, &["--all"]).expect("a bundle");
        let expected = String::from_utf8_lossy(
            &std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["rev-parse", "refs/heads/main"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_owned();

        // The source is removed before the restore, so nothing can quietly
        // satisfy the drill from the original.
        fs::remove_dir_all(&repo).unwrap();
        let restored = temp.path().join("restored.git");
        restore_from_bundle(&bundle, &restored).expect("a restore");

        let actual = String::from_utf8_lossy(
            &std::process::Command::new("git")
                .arg("--git-dir")
                .arg(&restored)
                .args(["rev-parse", "refs/heads/main"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_owned();
        assert_eq!(actual, expected);
    }
}
