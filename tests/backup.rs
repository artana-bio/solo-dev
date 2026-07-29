//! `WP-520` acceptance: backup creation, independence, and verification.
//!
//! A backup nobody has read is a belief. Every test here is about turning the
//! belief into something checkable, and about refusing the two comfortable
//! lies: that a copy beside the original is a backup, and that a write which
//! returned success produced a readable file.

mod support;

use std::{fs, process::Command};

use support::Workspace;

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

/// Runs a `backup` subcommand, returning raw output.
fn backup_raw(workspace: &Workspace, args: &[&str]) -> std::process::Output {
    let mut full = vec![
        "backup".to_owned(),
        args[0].to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
    ];
    full.extend(args[1..].iter().map(|arg| (*arg).to_owned()));
    Workspace::run(&full)
}

fn backup_json(workspace: &Workspace, args: &[&str]) -> serde_json::Value {
    let output = backup_raw(workspace, args);
    assert!(
        output.status.success(),
        "backup {args:?} failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the JSON envelope")
}

/// A project with one landed, archived integration, so the archive refs exist.
fn with_archive() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let id = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }
    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    workspace.integration(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
    ]);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "owner",
    ]);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    workspace.archive(&["create", "--integration-id", &id]);
    workspace
}

#[test]
fn a_same_device_destination_is_refused() {
    let workspace = Workspace::initialized();
    let destination = workspace.root.join("backups");

    let output = backup_raw(
        &workspace,
        &[
            "create",
            "--destination",
            &destination.display().to_string(),
        ],
    );
    assert!(!output.status.success());
    assert_eq!(error_code(&output), "CH-POLICY-BACKUP-NOT-INDEPENDENT");
    assert!(
        !destination.exists(),
        "a refused backup must not leave a directory behind"
    );
}

#[test]
fn the_refusal_explains_what_a_same_disk_copy_does_not_protect_against() {
    let workspace = Workspace::initialized();
    let output = backup_raw(
        &workspace,
        &[
            "create",
            "--destination",
            &workspace.root.join("backups").display().to_string(),
        ],
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("same device"),
        "unexpected: {envelope}"
    );
    assert!(
        envelope["error"]["recovery"]
            .as_str()
            .unwrap()
            .contains("different device"),
        "unexpected: {envelope}"
    );
}

#[test]
fn an_override_takes_the_backup_but_says_it_is_weak() {
    let workspace = Workspace::initialized();
    let destination = workspace.root.join("backups");

    let output = backup_raw(
        &workspace,
        &[
            "create",
            "--destination",
            &destination.display().to_string(),
            "--allow-same-device",
        ],
    );
    assert!(output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let warnings = envelope["warnings"].as_array().unwrap();
    assert!(
        !warnings.is_empty(),
        "an overridden weakness must still be reported: {envelope}"
    );
    assert!(
        warnings[0].as_str().unwrap().contains("same device"),
        "unexpected: {warnings:?}"
    );
}

#[test]
fn a_backup_contains_every_archive_ref_the_source_holds() {
    let workspace = with_archive();
    let destination = workspace.root.join("backups");
    let envelope = backup_json(
        &workspace,
        &[
            "create",
            "--destination",
            &destination.display().to_string(),
            "--allow-same-device",
        ],
    );

    let backups = envelope["data"]["backups"].as_array().unwrap();
    assert_eq!(backups.len(), 2, "authority and control: {backups:?}");
    let authority = backups
        .iter()
        .find(|entry| entry["subject"] == "authority")
        .expect("an authority backup");
    let refs: Vec<&str> = authority["refs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert!(
        refs.iter().any(|name| name.contains("refs/heads/main")),
        "the protected branch must be in the backup: {refs:?}"
    );
}

#[test]
fn a_corrupted_backup_fails_verification_and_names_itself() {
    let workspace = Workspace::initialized();
    let destination = workspace.root.join("backups");
    backup_json(
        &workspace,
        &[
            "create",
            "--destination",
            &destination.display().to_string(),
            "--allow-same-device",
        ],
    );

    // Truncate mid-pack: the header still parses, so anything that only reads
    // the first bytes would call this fine.
    let bundle = destination.join("control.bundle");
    let mut contents = fs::read(&bundle).unwrap();
    contents.truncate(contents.len() / 2);
    fs::write(&bundle, &contents).unwrap();

    let output = backup_raw(
        &workspace,
        &[
            "verify",
            "--destination",
            &destination.display().to_string(),
        ],
    );
    assert!(!output.status.success());
    assert_eq!(error_code(&output), "CH-POLICY-BACKUP-CORRUPT");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("control.bundle"),
        "the failing backup must be named: {envelope}"
    );
}

#[test]
fn verification_fails_when_the_backup_is_replaced_by_zeroes() {
    // The check that catches a verifier reading the source instead of the
    // backup: the source is perfectly healthy here.
    let workspace = Workspace::initialized();
    let destination = workspace.root.join("backups");
    backup_json(
        &workspace,
        &[
            "create",
            "--destination",
            &destination.display().to_string(),
            "--allow-same-device",
        ],
    );
    fs::write(destination.join("authority.bundle"), vec![0u8; 8192]).unwrap();

    let output = backup_raw(
        &workspace,
        &[
            "verify",
            "--destination",
            &destination.display().to_string(),
        ],
    );
    assert_eq!(error_code(&output), "CH-POLICY-BACKUP-CORRUPT");
}

#[test]
fn a_restore_drill_reconstructs_authority_and_control_from_the_backup_alone() {
    let workspace = with_archive();
    let destination = workspace.root.join("backups");
    backup_json(
        &workspace,
        &[
            "create",
            "--destination",
            &destination.display().to_string(),
            "--allow-same-device",
        ],
    );

    let authority_head = workspace.authority_head();
    let control_head = workspace.control_head();

    // The originals are removed, so nothing can quietly satisfy the drill from
    // the source.
    fs::remove_dir_all(&workspace.authority).unwrap();
    fs::remove_dir_all(&workspace.control).unwrap();

    for (name, expected) in [("authority", &authority_head), ("control", &control_head)] {
        let restored = workspace.root.join(format!("restored-{name}.git"));
        let clone = Command::new("git")
            .args(["clone", "--quiet", "--mirror"])
            .arg(destination.join(format!("{name}.bundle")))
            .arg(&restored)
            .output()
            .expect("git should run");
        assert!(
            clone.status.success(),
            "restoring {name} failed: {}",
            String::from_utf8_lossy(&clone.stderr)
        );

        let head = Command::new("git")
            .arg("--git-dir")
            .arg(&restored)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .expect("git should run");
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            expected,
            "{name} did not come back at the commit it was backed up from"
        );
    }
}

#[test]
fn verifying_an_absent_backup_is_a_precondition_failure() {
    let workspace = Workspace::initialized();
    let output = backup_raw(
        &workspace,
        &[
            "verify",
            "--destination",
            &workspace.root.join("nowhere").display().to_string(),
        ],
    );
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-NOT-FOUND");
}

#[test]
fn a_backup_dry_run_writes_nothing() {
    let workspace = Workspace::initialized();
    let destination = workspace.root.join("backups");
    let envelope = backup_json(
        &workspace,
        &[
            "create",
            "--destination",
            &destination.display().to_string(),
            "--allow-same-device",
            "--dry-run",
        ],
    );
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(envelope["data"]["planned"].as_array().unwrap().len(), 2);
    assert!(!destination.exists(), "a dry run must write nothing");
}
