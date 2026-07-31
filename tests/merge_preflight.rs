//! `WP-420` acceptance: merge preflight and disposable integration.
//!
//! The preflight's whole point is that it can be wrong about nothing: it
//! reports what would happen without leaving anything behind. These tests hold
//! it to that, and hold `integration merge` to leaving no worktree, no branch,
//! and no half-merged files on either outcome.

mod support;

use std::fs;

use support::Workspace;

/// A cycle with `count` approved cards, each touching its own directory.
fn integrable(count: usize) -> Workspace {
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
    workspace
}

/// An approved card that conflicts with the protected branch.
///
/// Two cards cannot be made to conflict with each other: `WP-230` refuses two
/// active cards claiming the same path, so the ownership model prevents that
/// case upstream. A conflict reaches integration by the other route — the
/// protected branch moving under an approved candidate — which is exactly what
/// `expected_main_sha` exists to detect.
fn conflicting() -> Workspace {
    let workspace = Workspace::initialized();
    // A file the card and the branch will both edit must exist at the baseline.
    fs::write(
        workspace.repository.join("shared.txt"),
        "line1\nline2\nline3\n",
    )
    .unwrap();
    support::git(&workspace.repository, &["add", "-A"]);
    support::git(&workspace.repository, &["commit", "-q", "-m", "add shared"]);
    support::git(
        &workspace.repository,
        &["push", "-q", "harness-authority", "main"],
    );

    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["shared.txt"]);
    workspace.approve_card("F-001", "shared.txt");

    // Someone lands a change to the same file directly on the protected branch.
    fs::write(
        workspace.repository.join("shared.txt"),
        "landed elsewhere\nline2\nline3\n",
    )
    .unwrap();
    support::git(&workspace.repository, &["add", "-A"]);
    support::git(&workspace.repository, &["commit", "-q", "-m", "hotfix"]);
    support::git(
        &workspace.repository,
        &["push", "-q", "harness-authority", "main"],
    );
    workspace
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

/// Prepares every ready card and returns the integration identifier.
fn prepare(workspace: &Workspace) -> String {
    workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn a_clean_sequence_preflights_without_conflicts() {
    let workspace = integrable(2);
    let id = prepare(&workspace);

    let envelope = workspace.integration_json(&["preflight", "--integration-id", &id]);
    assert_eq!(envelope["data"]["clean"], true);
    assert_eq!(envelope["data"]["unevaluated_members"], 0);
    assert_eq!(envelope["data"]["steps"].as_array().unwrap().len(), 2);
    assert!(
        envelope["data"]["resulting_tree"].is_string(),
        "a clean preflight names the tree it would produce: {}",
        envelope["data"]
    );
}

#[test]
fn a_preflight_changes_nothing() {
    let workspace = integrable(2);
    let id = prepare(&workspace);
    let control_before = workspace.control_head();
    let candidate_before = workspace.candidate_head();
    let authority_before = workspace.authority_head();
    let worktrees_before = workspace.candidate_worktrees();

    workspace.integration(&["preflight", "--integration-id", &id]);

    assert_eq!(workspace.control_head(), control_before);
    assert_eq!(workspace.candidate_head(), candidate_before);
    assert_eq!(workspace.authority_head(), authority_before);
    assert_eq!(
        workspace.candidate_worktrees(),
        worktrees_before,
        "a preflight must not register a worktree"
    );
}

#[test]
fn a_candidate_conflicting_with_the_moved_branch_is_reported_as_textual() {
    let workspace = conflicting();
    let id = prepare(&workspace);

    let envelope = workspace.integration_json(&["preflight", "--integration-id", &id]);
    assert_eq!(envelope["data"]["clean"], false);

    let steps = envelope["data"]["steps"].as_array().unwrap();
    let conflicted = steps
        .iter()
        .find(|step| !step["conflicts"].as_array().unwrap().is_empty())
        .expect("one step must conflict");
    assert_eq!(conflicted["textual_conflicts"], 1);
    assert_eq!(conflicted["structural_conflicts"], 0);
    assert_eq!(conflicted["conflicts"][0]["kind"], "content");
    assert_eq!(conflicted["conflicts"][0]["paths"][0], "shared.txt");
}

#[test]
fn the_sequence_stops_at_the_first_conflict_and_says_so() {
    let workspace = conflicting();
    // A second card that would merge cleanly, ordered after the conflict.
    workspace.activate_card("F-002", &["src/F-002/**"]);
    workspace.approve_card("F-002", "src/F-002/a.rs");
    let id = prepare(&workspace);

    let envelope = workspace.integration_json(&["preflight", "--integration-id", &id]);
    assert_eq!(envelope["data"]["clean"], false);
    assert_eq!(
        envelope["data"]["unevaluated_members"], 1,
        "later members must be reported as unevaluated, not as clean: {}",
        envelope["data"]
    );
    assert!(
        envelope["data"]["resulting_tree"].is_null(),
        "a conflicted sequence produces no landing tree"
    );
}

#[test]
fn a_textual_conflict_blocks_the_merge_without_landing_anything() {
    let workspace = conflicting();
    let id = prepare(&workspace);
    let authority_before = workspace.authority_head();
    let control_before = workspace.control_head();

    let output = workspace.integration_raw(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(output.status.code(), Some(6));
    assert_eq!(error_code(&output), "CH-CONFLICT-MERGE-FAILED");

    assert_eq!(workspace.authority_head(), authority_before);
    assert_eq!(
        workspace.control_head(),
        control_before,
        "a refused merge records nothing"
    );
    assert!(
        !workspace.worktrees.join(".integration").join(&id).exists(),
        "no integration worktree may survive a refused merge"
    );
}

#[test]
fn a_clean_merge_records_the_integration_head_and_tree() {
    let workspace = integrable(2);
    let id = prepare(&workspace);
    let authority_before = workspace.authority_head();
    let authority_tree = support::capture(
        &workspace.repository,
        &["rev-parse", &format!("{authority_before}^{{tree}}")],
    );
    let preflight = workspace.integration_json(&["preflight", "--integration-id", &id]);
    assert_eq!(
        preflight["data"]["clean"], true,
        "the fixture must produce a clean combined tree"
    );
    let expected_tree = preflight["data"]["resulting_tree"].as_str().unwrap();
    assert_ne!(
        expected_tree, authority_tree,
        "the fixture must make its candidate content distinguish the integration tree from the authority baseline"
    );

    let envelope = workspace.integration_json(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    let head = envelope["data"]["integration_head"].as_str().unwrap();
    let tree = envelope["data"]["integration_tree"].as_str().unwrap();
    assert_eq!(head.len(), 40);
    assert_eq!(tree.len(), 40);
    assert_ne!(head, authority_before, "the merge must build something new");
    let head_tree = support::capture(
        &workspace.repository,
        &["rev-parse", &format!("{head}^{{tree}}")],
    );
    assert_eq!(
        tree, head_tree,
        "the recorded tree must be the tree carried by the recorded integration head"
    );
    assert_eq!(
        tree, expected_tree,
        "the integration tree must contain the candidate combination proven clean by preflight"
    );

    assert_eq!(
        workspace.authority_head(),
        authority_before,
        "merging is not promotion; the protected branch must not move"
    );

    // The record carries the result forward for WP-430 and WP-440.
    let inspected = workspace.integration_json(&["inspect", "--integration-id", &id]);
    assert_eq!(inspected["data"]["members"], envelope["data"]["members"]);
    assert_eq!(inspected["data"]["integration_head"], head);
    assert_eq!(inspected["data"]["integration_tree"], tree);

    let merged = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "integration.merged")
        .expect("the merge must be recorded");
    assert_eq!(merged["head_sha"], head);
    assert_eq!(merged["metadata"]["integration_tree"], tree);
}

#[test]
fn the_disposable_worktree_is_removed_after_a_successful_merge() {
    let workspace = integrable(2);
    let id = prepare(&workspace);
    let worktrees_before = workspace.candidate_worktrees();

    workspace.integration(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);

    assert!(
        !workspace.worktrees.join(".integration").join(&id).exists(),
        "the integration worktree must not outlive the merge"
    );
    assert_eq!(
        workspace.candidate_worktrees(),
        worktrees_before,
        "its registration must be gone too, not merely its directory"
    );
}

#[test]
fn merging_leaves_no_branch_behind() {
    let workspace = integrable(2);
    let id = prepare(&workspace);

    workspace.integration(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);

    // The integration worktree is detached precisely so nothing named survives.
    assert!(
        !workspace.candidate_branch_exists(&id),
        "an integration must not create a branch named after itself"
    );
    assert!(!workspace.candidate_branch_exists(&format!("integration/{id}")));
}

#[test]
fn a_candidate_whose_branch_moved_cannot_be_merged() {
    let workspace = integrable(1);
    let id = prepare(&workspace);

    // The plan pinned an exact commit; the branch moves after preparation.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/F-001/late.rs"), "// late\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: late"]);

    let output = workspace.integration_raw(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-NOT-INTEGRABLE");
}

#[test]
fn a_merge_dry_run_reports_the_preflight_and_changes_nothing() {
    let workspace = integrable(2);
    let id = prepare(&workspace);
    let before = workspace.control_head();

    let envelope = workspace.integration_json(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
        "--dry-run",
    ]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(envelope["data"]["clean"], true);

    assert_eq!(workspace.control_head(), before);
    assert!(!workspace.worktrees.join(".integration").join(&id).exists());
}

#[test]
fn an_integration_cannot_be_merged_twice() {
    let workspace = integrable(2);
    let id = prepare(&workspace);
    workspace.integration(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);

    let output = workspace.integration_raw(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    // A second merge would build a different head from the same plan, leaving
    // two commits each claiming to be the integration.
    assert_eq!(output.status.code(), Some(5));
}

#[test]
fn an_intermediate_gate_names_the_candidate_that_broke_the_combination() {
    let workspace = integrable(2);
    // Fails only once F-002's file exists, so the first merge passes and the
    // second does not. A single run at the end could not tell them apart.
    workspace.register_gate("gate.smoke", &["sh", "-c", "test ! -f src/F-002/a.rs"]);
    let id = prepare(&workspace);

    let output = workspace.integration_raw(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
        "--smoke-gate",
        "gate.smoke",
    ]);
    assert_eq!(output.status.code(), Some(7));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("F-002"),
        "the failing candidate must be named: {message}"
    );
    assert!(
        !workspace.worktrees.join(".integration").join(&id).exists(),
        "a failed smoke gate must not leave the worktree behind"
    );
}

#[test]
fn a_passing_intermediate_gate_does_not_block_the_merge() {
    let workspace = integrable(2);
    workspace.register_gate("gate.smoke", &["true"]);
    let id = prepare(&workspace);

    let envelope = workspace.integration_json(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
        "--smoke-gate",
        "gate.smoke",
    ]);
    assert!(envelope["data"]["integration_head"].is_string());
}

#[test]
fn untracked_feature_state_cannot_enter_the_integration() {
    let workspace = integrable(1);

    // An untracked file in the feature worktree, present at merge time.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("scratch.txt"), "not committed\n").unwrap();

    let id = prepare(&workspace);
    let planned = workspace.integration_json(&["inspect", "--integration-id", &id]);
    let candidate = planned["data"]["members"][0]["candidate_sha"]
        .as_str()
        .unwrap();
    let candidate_tree = support::capture(
        &workspace.repository,
        &["rev-parse", &format!("{candidate}^{{tree}}")],
    );
    let authority = workspace.authority_head();
    let authority_tree = support::capture(
        &workspace.repository,
        &["rev-parse", &format!("{authority}^{{tree}}")],
    );
    assert_ne!(
        candidate_tree, authority_tree,
        "the fixture must make the committed candidate content distinguishable from the authority baseline"
    );

    let envelope = workspace.integration_json(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);

    // Integration merges the pinned commit, so nothing uncommitted can ride
    // along. Reading the merged tree is the only honest way to check.
    let head = envelope["data"]["integration_head"]
        .as_str()
        .unwrap()
        .to_owned();
    let recorded_tree = envelope["data"]["integration_tree"].as_str().unwrap();
    let head_tree = support::capture(
        &workspace.repository,
        &["rev-parse", &format!("{head}^{{tree}}")],
    );
    assert_eq!(
        recorded_tree, head_tree,
        "the recorded integration tree must identify the merged head's contents"
    );
    assert_eq!(
        head_tree, candidate_tree,
        "a one-member integration must contain exactly the pinned candidate tree, excluding untracked worktree state"
    );
    let listing = support::capture(
        &workspace.repository,
        &["ls-tree", "-r", "--name-only", &head],
    );
    assert!(
        !listing.contains("scratch.txt"),
        "untracked feature state must not reach integration: {listing}"
    );
}

#[test]
fn the_merge_order_follows_the_recorded_plan() {
    let workspace = integrable(3);
    let id = prepare(&workspace);
    let plan = workspace.integration_json(&["inspect", "--integration-id", &id]);
    let planned: Vec<String> = plan["data"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["card_id"].as_str().unwrap().to_owned())
        .collect();

    let envelope = workspace.integration_json(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    let head = envelope["data"]["integration_head"].as_str().unwrap();

    // Merge commits are recorded newest-first, so the plan order reversed is
    // the order the subjects should appear in.
    let subjects = support::capture(
        &workspace.repository,
        &["log", "--format=%s", "--merges", head],
    );
    let observed: Vec<&str> = subjects.lines().collect();
    for (offset, card) in planned.iter().rev().enumerate() {
        assert!(
            observed[offset].contains(card.as_str()),
            "merge {offset} should be {card}, found `{}`",
            observed[offset]
        );
    }
}

#[test]
fn scenario_28_a_semantic_conflict_cannot_be_resolved_in_place() {
    // A "semantic conflict" is two changes that merge cleanly and are wrong
    // together. Git cannot see it; only a gate can. The requirement is that
    // the harness never lets someone fix it inside the integration — the
    // resolution has to become a card, which is reviewable.
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.register_gate(
        "gate.combined",
        &[
            "sh",
            "-c",
            "! (test -f src/F-001/a.rs && test -f src/F-002/a.rs)",
        ],
    );
    for card in ["F-001", "F-002"] {
        workspace.activate_card_with_gate_sets(
            card,
            &[&format!("src/{card}/**")],
            &["gate.unit"],
            &["gate.combined"],
        );
        workspace.approve_card(card, &format!("src/{card}/a.rs"));
    }
    let id = prepare(&workspace);

    // The candidates merge cleanly — the conflict is not textual.
    let preflight = workspace.integration_json(&["preflight", "--integration-id", &id]);
    assert_eq!(
        preflight["data"]["clean"], true,
        "the fixture must be a semantic conflict, not a textual one"
    );
    workspace.integration(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    workspace.integration(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);

    // Verification catches it, and leaves nothing to edit.
    let output =
        workspace.integration_raw(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    assert_eq!(output.status.code(), Some(7));
    assert!(
        !workspace.worktrees.join(".integration").join(&id).exists(),
        "no worktree may survive for someone to resolve the conflict in place"
    );

    // And the integration cannot advance, so the only route forward is a new
    // card whose change goes through review like any other.
    assert_eq!(
        workspace.integration_json(&["inspect", "--integration-id", &id])["data"]["status"],
        "prepared"
    );
}
