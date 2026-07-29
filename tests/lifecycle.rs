//! Section 19.3 gate evidence: the full lifecycle, twice, in one project.
//!
//! Every other test file checks one work package. This one checks that they
//! compose — that a change really can travel from an empty project to a
//! promoted, archived, closed commit using only harness commands, and that
//! doing it a second time behaves correctly against a protected branch the
//! first cycle already moved.

mod support;

use std::fs;

use support::Workspace;

/// Drives one complete cycle and returns the promoted landing commit.
///
/// Deliberately uses only the CLI. The gate requires the lifecycle to complete
/// "without manual Git mutation", and a fixture that reached into Git to smooth
/// a step over would be proving something weaker than the claim.
fn run_cycle(workspace: &Workspace, cycle: &str, card: &str, file: &str) -> String {
    workspace.cycle(&[
        "create",
        "--cycle-id",
        cycle,
        "--objective",
        "Deliver one bounded change",
    ]);
    workspace.cycle(&["activate", "--cycle-id", cycle]);

    // The card declares its own directory, so successive cycles never overlap.
    let body = format!(
        "card_id: {card}\ncycle_id: {cycle}\ntitle: Implement {card}\ngoal: Deliver {card}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/{card}/**\"]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head()
    );
    let draft = workspace.root.join(format!("{card}.yaml"));
    fs::write(&draft, body).unwrap();
    workspace.card(&["create", "--draft", &draft.display().to_string()]);
    workspace.card(&["activate", "--card-id", card]);

    workspace.work(&["start", "--card-id", card]);
    let worktree = workspace.worktrees.join(card);
    fs::create_dir_all(worktree.join(format!("src/{card}"))).unwrap();
    fs::write(worktree.join(file), format!("// {card}\n")).unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", &format!("feat: {card}")]);
    workspace.gate(&["run", "--card-id", card, "--gate-id", "gate.unit"]);

    let delivered = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join(format!("{card}-declaration.yaml"));
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {delivered}\nbehavior_delivered: adds {file}\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    workspace.handoff(&[
        "create",
        "--card-id",
        card,
        "--declaration",
        &declaration.display().to_string(),
    ]);

    workspace.review(&["begin", "--card-id", card]);
    let verdict = workspace.root.join(format!("{card}-verdict.yaml"));
    fs::write(
        &verdict,
        "reviewer_actor_id: reviewer-session\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\nresidual_risks: []\n",
    )
    .unwrap();
    workspace.review(&[
        "record",
        "--card-id",
        card,
        "--verdict",
        &verdict.display().to_string(),
    ]);

    let id =
        workspace.integration_json(&["prepare", "--cycle-id", cycle, "--actor-id", "coordinator"])
            ["data"]["integration_id"]
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
        "integration-reviewer",
    ]);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "acceptance-owner",
    ]);
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    workspace.archive(&["create", "--integration-id", &id]);
    workspace.archive(&["verify", "--integration-id", &id]);
    workspace.archive(&["close", "--integration-id", &id]);

    workspace.authority_head()
}

#[test]
fn one_project_completes_the_full_lifecycle_twice() {
    let workspace = Workspace::initialized();
    let baseline = workspace.authority_head();

    let first = run_cycle(&workspace, "C-001", "F-001", "src/F-001/a.rs");
    assert_ne!(first, baseline, "the first cycle must move the branch");

    // The second cycle starts from a protected branch the first one moved,
    // which is the case a single-cycle test can never reach.
    let second = run_cycle(&workspace, "C-002", "F-002", "src/F-002/a.rs");
    assert_ne!(second, first, "the second cycle must move it again");

    // Both changes are present, and the branch descends from both landings.
    let listing = support::capture(
        &workspace.repository,
        &["ls-tree", "-r", "--name-only", &second],
    );
    assert!(listing.contains("src/F-001/a.rs"), "unexpected: {listing}");
    assert!(listing.contains("src/F-002/a.rs"), "unexpected: {listing}");
    // `--is-ancestor` exits zero only when it holds, and the helper asserts
    // success, so this is the ancestry claim itself rather than a proxy.
    support::git(
        &workspace.repository,
        &["merge-base", "--is-ancestor", &first, &second],
    );

    for card in ["F-001", "F-002"] {
        assert_eq!(
            workspace.card_json(&["status", "--card-id", card])["data"]["state"],
            "closed",
            "every card must reach the terminal state"
        );
    }
}

#[test]
fn the_second_cycle_rejects_a_plan_built_against_a_stale_main() {
    let workspace = Workspace::initialized();
    run_cycle(&workspace, "C-001", "F-001", "src/F-001/a.rs");

    // Start a second cycle and take it to a prepared integration, which pins
    // the authority commit as `expected_main_sha`.
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-002",
        "--objective",
        "Second slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-002"]);
    let body = format!(
        "card_id: F-002\ncycle_id: C-002\ntitle: Implement F-002\ngoal: Deliver F-002\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/F-002/**\"]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head()
    );
    let draft = workspace.root.join("F-002.yaml");
    fs::write(&draft, body).unwrap();
    workspace.card(&["create", "--draft", &draft.display().to_string()]);
    workspace.card(&["activate", "--card-id", "F-002"]);
    workspace.approve_card("F-002", "src/F-002/a.rs");

    let id = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-002",
        "--actor-id",
        "coordinator",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // The protected branch moves under the plan, exactly as it would if
    // another cycle promoted in between.
    let moved = workspace.advance_authority();

    let output = workspace.integration_raw(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert!(
        !output.status.success(),
        "a plan built against a stale main must not proceed"
    );
    assert_eq!(
        workspace.authority_head(),
        moved,
        "the refusal must leave the protected branch alone"
    );
}

#[test]
fn audit_evidence_identifies_the_exact_authority_transition() {
    let workspace = Workspace::initialized();
    let baseline = workspace.authority_head();
    let landing = run_cycle(&workspace, "C-001", "F-001", "src/F-001/a.rs");

    let promoted = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "integration.promoted")
        .expect("promotion must be recorded");

    // Both ends of the transition, from the record alone: what the branch was
    // and what it became. Either one on its own would leave a reader guessing.
    assert_eq!(promoted["metadata"]["previous_main_sha"], baseline);
    assert_eq!(promoted["metadata"]["landing_sha"], landing);
    assert_eq!(promoted["head_sha"], landing);
    assert!(
        promoted["metadata"]["acceptance_id"]
            .as_str()
            .unwrap()
            .starts_with("ACC-"),
        "the authorizing decision must be identifiable: {promoted}"
    );
    assert_eq!(promoted["actor_id"], "promoter");
}

#[test]
fn no_failure_path_in_the_lifecycle_leaves_the_authority_moved() {
    let workspace = Workspace::initialized();
    let baseline = workspace.authority_head();

    // Every refusal available before promotion, checked against one invariant:
    // the protected branch is untouched.
    let attempts: Vec<Vec<&str>> = vec![
        vec!["prepare", "--cycle-id", "C-404", "--actor-id", "a"],
        vec!["merge", "--integration-id", "INT-404", "--actor-id", "a"],
        vec!["land", "--integration-id", "INT-404", "--actor-id", "a"],
        vec!["verify", "--integration-id", "INT-404", "--actor-id", "a"],
        vec!["promote", "--integration-id", "INT-404", "--actor-id", "a"],
        vec!["inspect", "--integration-id", "INT-404"],
    ];
    for attempt in attempts {
        let output = workspace.integration_raw(&attempt);
        assert!(
            !output.status.success(),
            "{attempt:?} should have been refused"
        );
        assert_ne!(
            output.status.code(),
            Some(1),
            "exit 1 is reserved; {attempt:?} produced an unclassified failure"
        );
        assert_eq!(
            workspace.authority_head(),
            baseline,
            "{attempt:?} disturbed the protected branch"
        );
    }
}
