//! `WP-460` acceptance: archive, close, and cleanup.
//!
//! Cleanup and data loss look identical from outside: a branch is gone either
//! way. Every test here is about the difference — whether the commits are still
//! findable afterwards, and whether the harness refuses when they would not be.

mod support;

use std::fs;

use support::Workspace;

/// A cycle carried all the way to a promoted integration.
fn promoted(count: usize) -> (Workspace, String) {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    for index in 1..=count {
        let card = format!("F-{index:03}");
        workspace.activate_card(&card, &[&format!("src/{card}/**")]);
        workspace.approve_card(&card, &format!("src/{card}/a.rs"));
    }

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
    (workspace, id)
}

/// The same, with archive refs already created.
fn archived(count: usize) -> (Workspace, String) {
    let (workspace, id) = promoted(count);
    workspace.archive(&["create", "--integration-id", &id]);
    (workspace, id)
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn archiving_creates_a_ref_for_the_landing_and_every_candidate() {
    let (workspace, id) = promoted(2);
    let landing = workspace.authority_head();

    let envelope = workspace.archive_json(&["create", "--integration-id", &id]);
    let refs = envelope["data"]["refs"].as_array().unwrap();
    assert_eq!(refs.len(), 3, "one landing plus two candidates: {refs:?}");

    assert_eq!(
        support::capture(
            &workspace.repository,
            &["rev-parse", &format!("refs/archive/integrations/{id}")]
        ),
        landing
    );
    for card in ["F-001", "F-002"] {
        let archived = support::capture(
            &workspace.repository,
            &["rev-parse", &format!("refs/archive/cards/{card}")],
        );
        assert_eq!(archived.len(), 40, "card {card} must be archived");
    }
}

#[test]
fn archiving_releases_the_temporary_landing_ref() {
    let (workspace, id) = promoted(1);
    // The landing ref kept the commit alive before promotion; the archive ref
    // now does it durably, so the temporary one has no further job.
    assert!(
        !support::capture(
            &workspace.repository,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/harness/landing"
            ]
        )
        .is_empty()
    );

    workspace.archive(&["create", "--integration-id", &id]);
    assert!(
        support::capture(
            &workspace.repository,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/harness/landing"
            ]
        )
        .is_empty(),
        "the temporary landing ref must be released"
    );
}

#[test]
fn an_unpromoted_integration_cannot_be_archived() {
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

    let output = workspace.archive_raw(&["create", "--integration-id", &id]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
}

#[test]
fn verification_passes_for_an_intact_archive() {
    let (workspace, id) = archived(2);
    let envelope = workspace.archive_json(&["verify", "--integration-id", &id]);
    assert_eq!(envelope["data"]["intact"], true);
    assert_eq!(envelope["data"]["checked"], 3);
}

#[test]
fn verification_reports_a_ref_that_was_moved_out_from_under_it() {
    let (workspace, id) = archived(1);
    // Someone repoints the archive ref by hand.
    let elsewhere = workspace.candidate_rev("HEAD~1");
    support::git(
        &workspace.repository,
        &["update-ref", "refs/archive/cards/F-001", &elsewhere],
    );

    let output = workspace.archive_raw(&["verify", "--integration-id", &id]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-ARCHIVE-BROKEN");
}

#[test]
fn verification_reports_a_ref_that_vanished() {
    let (workspace, id) = archived(1);
    support::git(
        &workspace.repository,
        &["update-ref", "-d", "refs/archive/cards/F-001"],
    );

    let output = workspace.archive_raw(&["verify", "--integration-id", &id]);
    assert_eq!(output.status.code(), Some(5));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("refs/archive/cards/F-001"),
        "the broken ref must be named: {envelope}"
    );
}

#[test]
fn closing_removes_the_worktrees_and_branches() {
    let (workspace, id) = archived(2);
    let worktree = workspace.worktrees.join("F-001");
    assert!(worktree.exists());
    assert!(workspace.candidate_branch_exists("card/F-001"));

    let envelope = workspace.archive_json(&["close", "--integration-id", &id]);
    assert_eq!(envelope["data"]["status"], "archived");
    assert_eq!(envelope["data"]["changed"], true);

    assert!(!worktree.exists(), "the worktree must be gone");
    assert!(
        !workspace.candidate_branch_exists("card/F-001"),
        "the branch must be gone"
    );
    for card in ["F-001", "F-002"] {
        assert_eq!(
            workspace.card_json(&["status", "--card-id", card])["data"]["state"],
            "closed"
        );
    }
}

#[test]
fn landed_commits_remain_reachable_after_cleanup() {
    let (workspace, id) = archived(2);
    let candidates: Vec<String> = workspace.integration_json(&["inspect", "--integration-id", &id])
        ["data"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["candidate_sha"].as_str().unwrap().to_owned())
        .collect();

    workspace.archive(&["close", "--integration-id", &id]);
    // The branches are gone; collection must not be able to take the commits.
    support::git(&workspace.repository, &["gc", "--prune=now", "--quiet"]);

    for candidate in &candidates {
        assert_eq!(
            support::capture(&workspace.repository, &["rev-parse", candidate]),
            *candidate,
            "an archived candidate must survive cleanup and collection"
        );
    }
    assert_eq!(
        support::capture(
            &workspace.repository,
            &["rev-parse", &format!("refs/archive/integrations/{id}")]
        ),
        workspace.authority_head()
    );
}

#[test]
fn a_dirty_worktree_blocks_cleanup() {
    let (workspace, id) = archived(1);
    // Uncommitted work in the card's worktree, which removal would destroy.
    fs::write(
        workspace.worktrees.join("F-001").join("in-progress.txt"),
        "not committed\n",
    )
    .unwrap();

    let output = workspace.archive_raw(&["close", "--integration-id", &id]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-WORKTREE-DIRTY");
    assert!(
        workspace
            .worktrees
            .join("F-001")
            .join("in-progress.txt")
            .exists(),
        "the uncommitted work must be untouched"
    );
    assert_eq!(
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["status"],
        "promoted",
        "a refused close must not advance the integration"
    );
}

#[test]
fn unarchived_unique_commits_block_cleanup() {
    let (workspace, id) = archived(1);
    // A commit made on the card branch after archiving: it exists nowhere else
    // and no archive ref covers it, so deleting the branch would lose it.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/F-001/later.rs").as_path(), "// later\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: later"]);

    let output = workspace.archive_raw(&["close", "--integration-id", &id]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-UNMERGED-WORK");
    assert!(
        workspace.candidate_branch_exists("card/F-001"),
        "the branch holding unarchived work must survive"
    );
}

#[test]
fn a_broken_archive_blocks_cleanup_before_anything_is_removed() {
    let (workspace, id) = archived(1);
    support::git(
        &workspace.repository,
        &["update-ref", "-d", "refs/archive/cards/F-001"],
    );

    let output = workspace.archive_raw(&["close", "--integration-id", &id]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-ARCHIVE-BROKEN");
    assert!(
        workspace.candidate_branch_exists("card/F-001"),
        "nothing may be removed while the archive is broken"
    );
    assert!(workspace.worktrees.join("F-001").exists());
}

#[test]
fn closing_is_idempotent() {
    let (workspace, id) = archived(1);
    workspace.archive(&["close", "--integration-id", &id]);
    let control_after_first = workspace.control_head();

    let envelope = workspace.archive_json(&["close", "--integration-id", &id]);
    assert_eq!(envelope["data"]["changed"], false);
    assert_eq!(envelope["data"]["status"], "archived");
    assert_eq!(
        workspace.control_head(),
        control_after_first,
        "a repeated close must write nothing"
    );
}

#[test]
fn the_closed_state_is_terminal() {
    let (workspace, id) = archived(1);
    workspace.archive(&["close", "--integration-id", &id]);

    // Nothing may move an archived integration onward, and Section 11.3 gives
    // it no successors at all.
    let output =
        workspace.integration_raw(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_eq!(output.status.code(), Some(5));
    let verify =
        workspace.integration_raw(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    assert_eq!(verify.status.code(), Some(5));
}

#[test]
fn closing_an_unarchived_integration_is_refused() {
    let (workspace, id) = promoted(1);
    let output = workspace.archive_raw(&["close", "--integration-id", &id]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-NOT-FOUND");
}

#[test]
fn an_archive_dry_run_changes_nothing() {
    let (workspace, id) = promoted(2);
    let before = workspace.control_head();

    let envelope = workspace.archive_json(&["create", "--integration-id", &id, "--dry-run"]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(envelope["data"]["refs"].as_array().unwrap().len(), 3);

    assert_eq!(workspace.control_head(), before);
    assert!(
        support::capture(
            &workspace.repository,
            &["for-each-ref", "--format=%(refname)", "refs/archive"]
        )
        .is_empty(),
        "a dry run must create no refs"
    );
}
