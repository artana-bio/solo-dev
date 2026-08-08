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
        "reviewer_actor_id: reviewer-session\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n",
    )
    .unwrap();
    workspace.review(&[
        "record",
        "--card-id",
        card,
        "--verdict",
        &verdict.display().to_string(),
        "--actor",
        "reviewer-session",
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

#[test]
fn stateful_dry_runs_do_not_change_state() {
    // A dry-run flag that parses and is then forgotten performs the real
    // mutation. Exercise the stateful lifecycle previews that need this
    // fixture at a state where their real form would succeed, rather than
    // treating a precondition refusal as evidence that no mutation occurred.
    let work = Workspace::initialized();
    assert_dry_run_unchanged(&work, "cycle create", || {
        work.cycle_raw(&[
            "create",
            "--cycle-id",
            "C-001",
            "--objective",
            "Dry-run work commands",
            "--dry-run",
        ])
    });
    work.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Dry-run work commands",
    ]);
    work.cycle(&["activate", "--cycle-id", "C-001"]);
    work.activate_card("F-001", &["src/F-001/**"]);
    work.bind_fixture_plan("plan-dry-run", "parallel");

    assert_dry_run_unchanged(&work, "work start", || {
        work.work_raw(&[
            "start",
            "--card-id",
            "F-001",
            "--actor-principal-id",
            "implementer-principal",
            "--actor-session-id",
            "implementer-session",
            "--dry-run",
        ])
    });
    assert!(
        !work.worktrees.join("F-001").exists(),
        "work start --dry-run must not allocate a worktree"
    );
    work.work(&["start", "--card-id", "F-001"]);
    assert_dry_run_unchanged(&work, "work checkpoint", || {
        work.work_raw(&[
            "checkpoint",
            "--card-id",
            "F-001",
            "--note",
            "progress",
            "--dry-run",
        ])
    });
    work.work(&["checkpoint", "--card-id", "F-001", "--note", "progress"]);
    assert_dry_run_unchanged(&work, "work block", || {
        work.work_raw(&[
            "block",
            "--card-id",
            "F-001",
            "--reason",
            "waiting",
            "--dry-run",
        ])
    });
    work.work(&["block", "--card-id", "F-001", "--reason", "waiting"]);
    assert_dry_run_unchanged(&work, "work resume", || {
        work.work_raw(&[
            "resume",
            "--card-id",
            "F-001",
            "--actor-principal-id",
            "implementer-principal",
            "--actor-session-id",
            "implementer-session",
            "--dry-run",
        ])
    });
    work.work(&["resume", "--card-id", "F-001"]);
    work.activate_card("F-002", &["src/F-002/**"]);
    assert_dry_run_unchanged(&work, "cycle declare-group", || {
        work.cycle_raw(&[
            "declare-group",
            "--cycle-id",
            "C-001",
            "--name",
            "pair",
            "--card-id",
            "F-001",
            "--card-id",
            "F-002",
            "--dry-run",
        ])
    });
    work.cycle(&[
        "declare-group",
        "--cycle-id",
        "C-001",
        "--name",
        "pair",
        "--card-id",
        "F-001",
        "--card-id",
        "F-002",
    ]);

    let abandoned_cycle = Workspace::initialized();
    abandoned_cycle.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Abandon a plan",
    ]);
    assert_dry_run_unchanged(&abandoned_cycle, "cycle abandon", || {
        abandoned_cycle.cycle_raw(&[
            "abandon",
            "--cycle-id",
            "C-001",
            "--reason",
            "not needed",
            "--dry-run",
        ])
    });
    abandoned_cycle.cycle(&["abandon", "--cycle-id", "C-001", "--reason", "not needed"]);

    let drafts = Workspace::initialized();
    drafts.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Draft and revise",
    ]);
    drafts.cycle(&["activate", "--cycle-id", "C-001"]);
    let initial_draft = write_draft(&drafts, "F-001", "Deliver F-001");
    assert_dry_run_unchanged(&drafts, "card create", || {
        drafts.card_raw(&[
            "create",
            "--draft",
            &initial_draft.display().to_string(),
            "--dry-run",
        ])
    });
    drafts.card(&["create", "--draft", &initial_draft.display().to_string()]);
    drafts.card(&["activate", "--card-id", "F-001"]);
    let revised_draft = write_draft(&drafts, "F-001", "Deliver F-001 differently");
    assert_dry_run_unchanged(&drafts, "card revise", || {
        drafts.card_raw(&[
            "revise",
            "--card-id",
            "F-001",
            "--draft",
            &revised_draft.display().to_string(),
            "--reason",
            "changed scope",
            "--dry-run",
        ])
    });
    drafts.card(&[
        "revise",
        "--card-id",
        "F-001",
        "--draft",
        &revised_draft.display().to_string(),
        "--reason",
        "changed scope",
    ]);

    let revocation = Workspace::initialized();
    prepare_handoff(&revocation, "F-001", "src/F-001/a.rs");
    assert_dry_run_unchanged(&revocation, "handoff revoke", || {
        revocation.handoff_raw(&[
            "revoke",
            "--card-id",
            "F-001",
            "--reason",
            "rework",
            "--dry-run",
        ])
    });
    revocation.handoff(&["revoke", "--card-id", "F-001", "--reason", "rework"]);

    let reviewing = Workspace::initialized();
    prepare_handoff(&reviewing, "F-001", "src/F-001/a.rs");
    assert_dry_run_unchanged(&reviewing, "review begin", || {
        reviewing.review_raw(&[
            "begin",
            "--card-id",
            "F-001",
            "--actor",
            "reviewer",
            "--actor-principal-id",
            "reviewer-principal",
            "--actor-session-id",
            "reviewer-session",
            "--dry-run",
        ])
    });
    reviewing.review(&["begin", "--card-id", "F-001"]);

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

    assert_dry_run_unchanged(&workspace, "integration prepare", || {
        workspace.integration_raw(&[
            "prepare",
            "--cycle-id",
            "C-001",
            "--actor-id",
            "coordinator",
            "--dry-run",
        ])
    });
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
    assert_preflight_unchanged(&workspace, &id);
    assert_dry_run_unchanged(&workspace, "integration merge", || {
        workspace.integration_raw(&[
            "merge",
            "--integration-id",
            &id,
            "--actor-id",
            "coordinator",
            "--dry-run",
        ])
    });
    workspace.integration(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert_dry_run_unchanged(&workspace, "integration land", || {
        workspace.integration_raw(&[
            "land",
            "--integration-id",
            &id,
            "--actor-id",
            "coordinator",
            "--dry-run",
        ])
    });
    workspace.integration(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    let verification_preview = assert_dry_run_unchanged(&workspace, "integration verify", || {
        workspace.integration_raw(&[
            "verify",
            "--integration-id",
            &id,
            "--actor-id",
            "verifier",
            "--dry-run",
        ])
    });
    assert_eq!(
        verification_preview["data"]["gates"],
        serde_json::json!(["gate.all", "gate.unit"]),
        "a dry run that does not name the gates is not a preview"
    );
    workspace.integration(&["verify", "--integration-id", &id, "--actor-id", "verifier"]);
    assert_dry_run_unchanged(&workspace, "integration review", || {
        workspace.integration_raw(&[
            "review",
            "--integration-id",
            &id,
            "--reviewer-actor-id",
            "reviewer",
            "--dry-run",
        ])
    });
    workspace.integration(&[
        "review",
        "--integration-id",
        &id,
        "--reviewer-actor-id",
        "reviewer",
    ]);
    assert_dry_run_unchanged(&workspace, "acceptance record", || {
        workspace.acceptance_raw(&[
            "record",
            "--integration-id",
            &id,
            "--acceptance-owner",
            "owner",
            "--dry-run",
        ])
    });
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--acceptance-owner",
        "owner",
    ]);
    assert_dry_run_unchanged(&workspace, "integration promote", || {
        workspace.integration_raw(&[
            "promote",
            "--integration-id",
            &id,
            "--actor-id",
            "promoter",
            "--dry-run",
        ])
    });
    workspace.integration(&["promote", "--integration-id", &id, "--actor-id", "promoter"]);
    assert_dry_run_unchanged(&workspace, "archive create", || {
        workspace.archive_raw(&["create", "--integration-id", &id, "--dry-run"])
    });
    workspace.archive(&["create", "--integration-id", &id]);
    assert_dry_run_unchanged(&workspace, "archive close", || {
        workspace.archive_raw(&["close", "--integration-id", &id, "--dry-run"])
    });

    let abandoned = Workspace::initialized();
    abandoned.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Abandon a plan",
    ]);
    abandoned.cycle(&["activate", "--cycle-id", "C-001"]);
    abandoned.activate_card("F-001", &["src/F-001/**"]);
    abandoned.approve_card("F-001", "src/F-001/a.rs");
    let abandoned_id = abandoned.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_dry_run_unchanged(&abandoned, "integration abandon", || {
        abandoned.integration_raw(&[
            "abandon",
            "--integration-id",
            &abandoned_id,
            "--actor-id",
            "coordinator",
            "--reason",
            "not needed",
            "--dry-run",
        ])
    });
}

fn write_draft(workspace: &Workspace, card_id: &str, goal: &str) -> std::path::PathBuf {
    let draft = workspace.root.join(format!("{card_id}-{goal}.yaml"));
    fs::write(
        &draft,
        format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: {goal}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [\"src/{card_id}/**\"]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
            base = workspace.authority_head(),
        ),
    )
    .unwrap();
    draft
}

fn prepare_handoff(workspace: &Workspace, card_id: &str, file: &str) {
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Prepare a handoff",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card(card_id, &[&format!("src/{card_id}/**")]);
    workspace.work(&["start", "--card-id", card_id]);

    let worktree = workspace.worktrees.join(card_id);
    let path = worktree.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("// {card_id}\n")).unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(
        &worktree,
        &["commit", "-q", "-m", &format!("feat: {card_id}")],
    );
    workspace.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

    let delivered = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join(format!("{card_id}-declaration.yaml"));
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
        card_id,
        "--declaration",
        &declaration.display().to_string(),
    ]);
}

fn assert_dry_run_unchanged(
    workspace: &Workspace,
    command: &str,
    run: impl FnOnce() -> std::process::Output,
) -> serde_json::Value {
    let control_before = workspace.control_head();
    let authority_before = workspace.authority_head();
    let output = run();
    assert!(
        output.status.success(),
        "{command} --dry-run failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        workspace.control_head(),
        control_before,
        "{command} wrote to control state"
    );
    assert_eq!(
        workspace.authority_head(),
        authority_before,
        "{command} moved the protected branch"
    );
    assert_eq!(
        envelope["data"]["dry_run"], true,
        "{command} must identify its preview"
    );
    envelope
}

fn assert_preflight_unchanged(workspace: &Workspace, integration_id: &str) {
    let control_before = workspace.control_head();
    let authority_before = workspace.authority_head();
    let output = workspace.integration_raw(&["preflight", "--integration-id", integration_id]);
    assert!(
        output.status.success(),
        "preflight failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        workspace.control_head(),
        control_before,
        "preflight wrote to control state"
    );
    assert_eq!(
        workspace.authority_head(),
        authority_before,
        "preflight moved the protected branch"
    );
}
