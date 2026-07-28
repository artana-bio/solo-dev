//! `WP-230` acceptance: safe worktree allocation and the resume link.

mod support;

use std::fs;

use serde_json::Value;
use support::Workspace;

/// A workspace with an activated card ready to allocate.
fn with_ready_card() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/a.rs"]);
    workspace
}

#[test]
fn allocation_creates_a_branch_at_the_exact_card_base() {
    let workspace = with_ready_card();
    let envelope = workspace.work_json(&["start", "--card-id", "F-001"]);

    let base = envelope["data"]["base_sha"].as_str().unwrap();
    assert_eq!(
        base,
        workspace.authority_head(),
        "the branch must start at the card's declared base"
    );
    assert_eq!(envelope["data"]["branch"], "card/F-001");
    assert_eq!(
        workspace.candidate_rev("card/F-001"),
        base,
        "the created branch must point at exactly that commit"
    );
}

#[test]
fn allocation_creates_a_locked_worktree_checked_out_and_clean() {
    let workspace = with_ready_card();
    let envelope = workspace.work_json(&["start", "--card-id", "F-001"]);
    let path = workspace.worktrees.join("F-001");

    assert_eq!(
        envelope["data"]["worktree_path"].as_str().unwrap(),
        path.display().to_string()
    );
    assert!(
        path.join("README.md").exists(),
        "the worktree is checked out"
    );

    let entries = workspace.candidate_worktrees();
    let entry = entries
        .iter()
        .find(|line| line.contains("F-001"))
        .expect("the worktree must be registered");
    assert!(
        entry.contains("locked"),
        "it must be locked against pruning: {entry}"
    );
}

#[test]
fn allocation_writes_an_ignored_locator() {
    let workspace = with_ready_card();
    let envelope = workspace.work_json(&["start", "--card-id", "F-001"]);
    let path = workspace.worktrees.join("F-001");

    let link: Value =
        serde_json::from_str(&fs::read_to_string(path.join(".agent/project.json")).unwrap())
            .unwrap();
    assert_eq!(link["schema"], "harness.worktree-link/v1");
    assert_eq!(link["card_id"], "F-001");
    assert_eq!(link["lease_id"], envelope["data"]["lease_id"]);
    assert_eq!(
        link["control_repository"],
        workspace.control.display().to_string()
    );

    // The locator must not appear as a change in the candidate worktree.
    let status = support::capture(&path, &["status", "--porcelain"]);
    assert!(
        status.is_empty(),
        "the worktree must be clean; `.agent/` should be excluded: {status}"
    );
}

#[test]
fn the_exclude_rule_goes_to_info_exclude_not_the_committed_gitignore() {
    let workspace = with_ready_card();
    workspace.work(&["start", "--card-id", "F-001"]);

    let exclude = fs::read_to_string(workspace.repository.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains(".agent/"), "{exclude}");
    assert!(
        !workspace.repository.join(".gitignore").exists(),
        "Section 9.3 forbids harness bookkeeping in project history"
    );
}

#[test]
fn allocation_is_idempotent_in_the_sense_that_a_duplicate_is_refused() {
    let workspace = with_ready_card();
    workspace.work(&["start", "--card-id", "F-001"]);

    let output = workspace.work_raw(&["start", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-LEASE-HELD");
}

#[test]
fn an_existing_branch_blocks_allocation() {
    let workspace = with_ready_card();
    support::git(&workspace.repository, &["branch", "card/F-001"]);

    let output = workspace.work_raw(&["start", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(4));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-PRECONDITION-BRANCH-EXISTS");
}

#[test]
fn an_existing_worktree_path_blocks_allocation() {
    let workspace = with_ready_card();
    fs::create_dir_all(workspace.worktrees.join("F-001")).unwrap();

    let output = workspace.work_raw(&["start", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(4));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-PRECONDITION-WORKTREE-EXISTS");
}

#[test]
fn a_card_whose_base_is_missing_is_refused_before_anything_is_created() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    // Activate a card whose base is a well-formed but absent object ID.
    workspace.activate_card_with_base("F-001", &["src/a.rs"], &"a".repeat(40));

    let output = workspace.work_raw(&["start", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(4));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-PRECONDITION-BASE-MISSING");
    assert!(
        !workspace.worktrees.join("F-001").exists(),
        "a refusal must create nothing"
    );
    assert!(!workspace.candidate_branch_exists("card/F-001"));
}

#[test]
fn a_refused_allocation_leaves_no_partial_state() {
    let workspace = with_ready_card();
    support::git(&workspace.repository, &["branch", "card/F-001"]);
    let before = workspace.control_head();

    let output = workspace.work_raw(&["start", "--card-id", "F-001"]);
    assert!(!output.status.success());

    assert!(!workspace.worktrees.join("F-001").exists());
    assert_eq!(
        workspace.control_head(),
        before,
        "a refusal must not advance control"
    );
    // The journal records the refusal as clean, so recovery stays quiet.
    let recover = Workspace::run(&[
        "project".into(),
        "recover".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        workspace.control.display().to_string(),
    ]);
    let envelope: Value = serde_json::from_slice(&recover.stdout).unwrap();
    assert_eq!(
        envelope["data"]["recovery_required"], false,
        "an ordinary refusal is not a recovery situation"
    );
}

#[test]
fn resume_confirms_a_matching_locator() {
    let workspace = with_ready_card();
    workspace.work(&["start", "--card-id", "F-001"]);

    let envelope = workspace.work_json(&["resume", "--card-id", "F-001"]);
    assert_eq!(envelope["data"]["locator_matches"], true);
    assert_eq!(envelope["data"]["clean"], true);
    assert_eq!(envelope["data"]["branch"], "card/F-001");
    assert_eq!(envelope["data"]["head_sha"], workspace.authority_head());
}

#[test]
fn resume_rejects_a_locator_that_disagrees_with_control() {
    let workspace = with_ready_card();
    workspace.work(&["start", "--card-id", "F-001"]);
    let link_path = workspace.worktrees.join("F-001/.agent/project.json");

    // The locator lives inside a tree the actor can edit, which is exactly why
    // it is never trusted. Point it at a different card.
    let mut link: Value = serde_json::from_str(&fs::read_to_string(&link_path).unwrap()).unwrap();
    link["card_id"] = serde_json::json!("F-999");
    fs::write(&link_path, serde_json::to_string_pretty(&link).unwrap()).unwrap();

    let output = workspace.work_raw(&["resume", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-LOCATOR-MISMATCH");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("card_id"),
        "the failure must name the disagreeing field"
    );
}

#[test]
fn resume_rejects_a_locator_naming_the_wrong_lease() {
    let workspace = with_ready_card();
    workspace.work(&["start", "--card-id", "F-001"]);
    let link_path = workspace.worktrees.join("F-001/.agent/project.json");

    let mut link: Value = serde_json::from_str(&fs::read_to_string(&link_path).unwrap()).unwrap();
    link["lease_id"] = serde_json::json!("L-000999");
    fs::write(&link_path, serde_json::to_string_pretty(&link).unwrap()).unwrap();

    let output = workspace.work_raw(&["resume", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
}

#[test]
fn checkpoint_records_progress_against_the_lease() {
    let workspace = with_ready_card();
    workspace.work(&["start", "--card-id", "F-001"]);

    workspace.work(&["checkpoint", "--card-id", "F-001", "--note", "scaffolded"]);
    workspace.work(&["checkpoint", "--card-id", "F-001", "--note", "tests green"]);

    let status = workspace.work_json(&["status", "--card-id", "F-001"]);
    let progress = status["data"]["held_lease"]["progress"].as_array().unwrap();
    assert_eq!(progress.len(), 2);
    assert_eq!(progress[0]["note"], "scaffolded");
    assert_eq!(progress[1]["note"], "tests green");
    assert_eq!(
        progress[0]["head_sha"], progress[1]["head_sha"],
        "no commits were made between the two notes"
    );
}

#[test]
fn checkpoint_captures_the_moving_candidate_head() {
    let workspace = with_ready_card();
    workspace.work(&["start", "--card-id", "F-001"]);
    let path = workspace.worktrees.join("F-001");

    workspace.work(&["checkpoint", "--card-id", "F-001", "--note", "before"]);
    fs::write(path.join("src-a.txt"), "work\n").unwrap();
    support::git(&path, &["add", "-A"]);
    support::git(&path, &["commit", "-q", "-m", "progress"]);
    workspace.work(&["checkpoint", "--card-id", "F-001", "--note", "after"]);

    let status = workspace.work_json(&["status", "--card-id", "F-001"]);
    let progress = status["data"]["held_lease"]["progress"].as_array().unwrap();
    assert_ne!(
        progress[0]["head_sha"], progress[1]["head_sha"],
        "a checkpoint must record the candidate head at that moment"
    );
}

#[test]
fn checkpoint_without_a_lease_is_a_precondition_failure() {
    let workspace = with_ready_card();
    let output = workspace.work_raw(&["checkpoint", "--card-id", "F-001", "--note", "nothing"]);
    assert_eq!(output.status.code(), Some(4));
}

#[test]
fn block_moves_the_card_and_records_the_reason() {
    let workspace = with_ready_card();
    workspace.work(&["start", "--card-id", "F-001"]);

    let envelope = workspace.work_json(&[
        "block",
        "--card-id",
        "F-001",
        "--reason",
        "waiting on a decision",
    ]);
    assert_eq!(envelope["data"]["state"], "blocked");

    let blocked = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "work.blocked")
        .expect("the block must be recorded");
    assert_eq!(blocked["metadata"]["reason"], "waiting on a decision");
    assert_eq!(blocked["next_state"], "blocked");
}

#[test]
fn allocation_never_force_removes_anything() {
    let workspace = with_ready_card();
    workspace.work(&["start", "--card-id", "F-001"]);
    let path = workspace.worktrees.join("F-001");

    // Leave uncommitted work, then attempt a second allocation. Nothing in this
    // package may reclaim the worktree by force.
    fs::write(path.join("uncommitted.txt"), "precious\n").unwrap();
    let output = workspace.work_raw(&["start", "--card-id", "F-001"]);
    assert!(!output.status.success());
    assert!(
        path.join("uncommitted.txt").exists(),
        "uncommitted work must survive a refused re-allocation"
    );
}

#[test]
fn a_dry_run_changes_nothing() {
    let workspace = with_ready_card();
    let before = workspace.control_head();

    let envelope = workspace.work_json(&["start", "--card-id", "F-001", "--dry-run"]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(envelope["data"]["branch"], "card/F-001");

    assert!(!workspace.worktrees.join("F-001").exists());
    assert!(!workspace.candidate_branch_exists("card/F-001"));
    assert_eq!(workspace.control_head(), before);
}

#[test]
fn the_lease_and_allocation_are_recorded_in_control_history() {
    let workspace = with_ready_card();
    let envelope = workspace.work_json(&["start", "--card-id", "F-001"]);
    let lease_id = envelope["data"]["lease_id"].as_str().unwrap();

    let tracked = workspace.control_tracked_files();
    assert!(
        tracked.contains(&format!("leases/{lease_id}.json")),
        "{tracked:?}"
    );

    let started = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "work.started")
        .expect("the allocation must be recorded");
    assert_eq!(started["metadata"]["lease_id"], lease_id);
    assert_eq!(started["metadata"]["branch"], "card/F-001");
    assert_eq!(started["head_sha"], workspace.authority_head());
    assert_eq!(started["next_state"], "active");
}
