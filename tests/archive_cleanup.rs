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
    }
    for index in 1..=count {
        let card = format!("F-{index:03}");
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

/// The lease record a card holds, read straight off disk.
///
/// Read as a file rather than through `work status`, so these tests assert
/// what was actually persisted rather than what one reader chose to report.
fn lease_record(workspace: &Workspace, lease_id: &str) -> serde_json::Value {
    serde_json::from_slice(
        &fs::read(workspace.control.join(format!("leases/{lease_id}.json"))).unwrap(),
    )
    .unwrap()
}

/// The id of the lease a card currently holds, or `None` once released.
fn held_lease_id(workspace: &Workspace, card: &str) -> Option<String> {
    workspace.work_json(&["status", "--card-id", card])["data"]["held_lease"]["lease_id"]
        .as_str()
        .map(ToOwned::to_owned)
}

#[test]
fn archiving_creates_a_ref_for_the_landing_and_every_candidate() {
    let (workspace, id) = promoted(2);
    let landing = workspace.authority_head();
    let candidates: std::collections::BTreeMap<String, String> =
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|member| {
                (
                    member["card_id"].as_str().unwrap().to_owned(),
                    member["candidate_sha"].as_str().unwrap().to_owned(),
                )
            })
            .collect();

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
        assert_eq!(
            archived, candidates[card],
            "card {card}'s archive ref must name its candidate, not merely a commit"
        );
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
fn closing_releases_the_lease_it_cleaned_up() {
    // `LeaseStatus::Released` existed, serialized, and was exercised in
    // `lease.rs`'s unit tests while no production path ever constructed it,
    // so `is_held()` was true for every lease that had ever been granted and
    // `held_lease` never returned `None` once a card was started. The
    // allocation a lease names is what `archive close` removes, so this is
    // where it ends.
    let (workspace, id) = archived(2);

    let leases: Vec<String> = ["F-001", "F-002"]
        .iter()
        .map(|card| held_lease_id(&workspace, card).expect("the fixture must hold a lease"))
        .collect();
    for lease_id in &leases {
        let before = lease_record(&workspace, lease_id);
        assert_eq!(before["status"], "held");
        assert_eq!(before["released_at"], serde_json::Value::Null);
    }

    workspace.archive(&["close", "--integration-id", &id]);

    for (card, lease_id) in ["F-001", "F-002"].iter().zip(&leases) {
        let after = lease_record(&workspace, lease_id);
        assert_eq!(
            after["status"], "released",
            "closing must release {lease_id}: {after}"
        );
        assert!(
            after["released_at"].is_string(),
            "a released lease must record when: {after}"
        );
        // The record survives its release. A lease is the answer to "who held
        // this card, and where" long after the allocation is gone, so release
        // is a state change and never a deletion.
        assert_eq!(after["card_id"], *card);
        assert_eq!(
            held_lease_id(&workspace, card),
            None,
            "a released lease must stop reading as held for {card}"
        );
    }
}

#[test]
fn a_close_dry_run_leaves_the_lease_held() {
    // `preview_close` mirrors every refusal the real close makes without any
    // of its writes. Releasing is a write.
    let (workspace, id) = archived(1);
    let lease_id = held_lease_id(&workspace, "F-001").unwrap();

    workspace.archive(&["close", "--integration-id", &id, "--dry-run"]);

    let record = lease_record(&workspace, &lease_id);
    assert_eq!(
        record["status"], "held",
        "a dry run must not release the lease: {record}"
    );
    assert_eq!(record["released_at"], serde_json::Value::Null);
    assert_eq!(
        held_lease_id(&workspace, "F-001").as_deref(),
        Some(&*lease_id)
    );
}

#[test]
fn a_refused_close_leaves_the_lease_held() {
    // Ordering, stated as a test: the lease is released last, after every
    // removal has succeeded, because `held_lease` is where cleanup finds the
    // worktree to remove. A close that refuses leaves the worktree on disk,
    // so the lease naming it has to still be held — otherwise the allocation
    // survives with nothing pointing at it, and the next close would walk
    // straight past it.
    let (workspace, id) = archived(1);
    let lease_id = held_lease_id(&workspace, "F-001").unwrap();
    fs::write(
        workspace.worktrees.join("F-001").join("in-progress.txt"),
        "not committed\n",
    )
    .unwrap();

    let output = workspace.archive_raw(&["close", "--integration-id", &id]);
    assert_eq!(error_code(&output), "CH-PRECONDITION-WORKTREE-DIRTY");

    let record = lease_record(&workspace, &lease_id);
    assert_eq!(
        record["status"], "held",
        "a refused close must leave the lease holding its worktree: {record}"
    );
    assert!(workspace.worktrees.join("F-001").exists());

    // Not retried here. `cleanup-started` is journaled `outside_control`, so
    // a failure past it is recorded partial whatever control looks like, and
    // `project recover --resume` settles only `integration.promote`
    // (`PROMOTE_COMMAND`, `src/commands/project.rs`) — so the retry would be
    // a test of journal recovery, which this change does not touch.
    // `closing_releases_the_lease_it_cleaned_up` is what proves a close that
    // succeeds does release.
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
    // Remove every other ref that could retain a candidate. This makes the
    // fixture discriminate: each candidate must now be retained by its own
    // card archive ref, not incidentally by the landing merge or main branch.
    let refs = support::capture(
        &workspace.repository,
        &["for-each-ref", "--format=%(refname)"],
    );
    for reference in refs
        .lines()
        .filter(|reference| !reference.starts_with("refs/archive/cards/"))
    {
        support::git(&workspace.repository, &["update-ref", "-d", reference]);
    }

    for (index, candidate) in candidates.iter().enumerate() {
        let card = format!("F-{:03}", index + 1);
        let containing = support::capture(
            &workspace.repository,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "--contains",
                candidate,
            ],
        );
        assert_eq!(
            containing.lines().collect::<Vec<_>>(),
            vec![format!("refs/archive/cards/{card}")],
            "candidate {candidate} must be retained solely by its own card archive ref"
        );
    }
    // The branches are gone; collection must not be able to take the commits.
    support::git(&workspace.repository, &["gc", "--prune=now", "--quiet"]);

    for candidate in &candidates {
        // `cat-file -e`, not `rev-parse`: given a 40-character hex string
        // `rev-parse` echoes it and exits 0 whether or not the object is there,
        // so comparing its output to the SHA proved nothing at all.
        assert!(
            support::object_exists(&workspace.repository, candidate),
            "an archived candidate must survive cleanup and collection"
        );
    }
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

#[test]
fn a_close_dry_run_removes_nothing() {
    // Tier 2, defect 7. `archive close` accepted `--dry-run` and never read it:
    // the dispatch arm passed only `args.common` to a function that has no
    // access to the flag. The most destructive command in the harness — it
    // removes worktrees, deletes branches, and closes cards — performed the
    // real close when asked to preview it, and reported success as though it
    // had previewed.
    let (workspace, id) = archived(1);
    let worktree = workspace.worktrees.join("F-001");
    assert!(
        worktree.exists(),
        "the fixture must have a worktree to lose, or this proves nothing"
    );

    let envelope = workspace.archive_json(&["close", "--integration-id", &id, "--dry-run"]);
    assert_eq!(envelope["data"]["dry_run"], true);

    assert!(
        worktree.exists(),
        "a dry run must not remove the worktree at {}",
        worktree.display()
    );
    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(workspace.control.join(format!("integrations/{id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(
        record["status"], "promoted",
        "a dry run must not advance the integration to archived"
    );

    // And the real close still works, so the fix is not "refuse everything".
    workspace.archive(&["close", "--integration-id", &id]);
    assert!(!worktree.exists(), "the real close must still remove it");
}

#[test]
fn a_close_dry_run_reports_the_same_refusal_a_real_close_would() {
    // A preview that skips the checks is worse than no preview: it tells an
    // operator the destructive command will succeed when it will not.
    let (workspace, id) = promoted(1);

    // No archive refs were created, so a real close must refuse.
    let real = workspace.archive_raw(&["close", "--integration-id", &id]);
    assert!(
        !real.status.success(),
        "the fixture must be a refusable one"
    );
    let refusal = error_code(&real);

    let preview = workspace.archive_raw(&["close", "--integration-id", &id, "--dry-run"]);
    assert!(!preview.status.success(), "the preview must refuse too");
    assert_eq!(
        error_code(&preview),
        refusal,
        "the preview must give the same reason the real command would"
    );
}
