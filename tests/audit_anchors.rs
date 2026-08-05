//! `#88` acceptance: verifying every anchored control head is still an
//! ancestor of the control record.
//!
//! `#87` writes the control repository's head into every landing commit as a
//! `Change-Harness-Control` trailer. An anchor nothing checks is just a
//! string in a commit message — every test here is about the one thing that
//! turns it into a check: does the anchor still hold, told apart correctly
//! from "it never held" and from "there is nothing here to worry about".

mod support;

use std::fs;

use support::Workspace;

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

/// Trailer values under one key, read from a command's JSON envelope.
///
/// Copied from `landing_commit.rs`'s helper of the same name and shape.
/// Fixture helpers are not shared across files in this suite (see also
/// `promotion.rs`'s `reviewed`/`accepted` versus `landing_commit.rs`'s
/// `merged`), so this is a deliberate, small duplication rather than an
/// oversight.
fn trailer_values(envelope: &serde_json::Value, key: &str) -> Vec<String> {
    envelope["data"]["trailers"]
        .as_array()
        .expect("trailers")
        .iter()
        .filter(|entry| entry[0] == key)
        .map(|entry| entry[1].as_str().unwrap().to_owned())
        .collect()
}

/// Drives one complete cycle to a promoted landing commit on an existing
/// workspace, and returns the integration id, the landing commit's own SHA,
/// and the control head it anchored.
///
/// Mirrors `lifecycle.rs`'s `run_cycle` fixture (itself proof that a second,
/// independent cycle composes correctly against a protected branch the first
/// one already moved), minus archiving — this file has no use for archive
/// state. `card`/`cycle` are parameterized, unlike `support::Workspace`'s own
/// `activate_card`, which hardcodes `cycle_id: C-001` into the card body it
/// writes; a second landing under its own cycle needs the real one.
fn landed(workspace: &Workspace, cycle: &str, card: &str, file: &str) -> (String, String, String) {
    workspace.cycle(&[
        "create",
        "--cycle-id",
        cycle,
        "--objective",
        "Deliver one bounded change",
    ]);
    workspace.cycle(&["activate", "--cycle-id", cycle]);

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
    workspace.integration(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    let land_envelope =
        workspace.integration_json(&["land", "--integration-id", &id, "--actor-id", "coordinator"]);
    let anchor = trailer_values(&land_envelope, "Change-Harness-Control")
        .into_iter()
        .next()
        .expect("a landing commit must carry exactly one control anchor trailer");

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

    let landing_sha = workspace.authority_head();
    (id, landing_sha, anchor)
}

/// A fresh project with a single landed, promoted integration.
fn single_landed() -> (Workspace, String, String, String) {
    let workspace = Workspace::initialized();
    let (id, landing_sha, anchor) = landed(&workspace, "C-001", "F-001", "src/F-001/a.rs");
    (workspace, id, landing_sha, anchor)
}

fn audit_anchors_raw(workspace: &Workspace) -> std::process::Output {
    Workspace::run(&[
        "audit".into(),
        "anchors".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ])
}

fn audit_anchors_json(workspace: &Workspace) -> serde_json::Value {
    let output = audit_anchors_raw(workspace);
    assert!(
        output.status.success(),
        "audit anchors failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the JSON envelope")
}

#[test]
fn a_healthy_project_reports_no_anchor_discrepancy() {
    let (workspace, _id, _landing_sha, _anchor) = single_landed();

    let envelope = audit_anchors_json(&workspace);
    assert_eq!(envelope["schema"], "harness.command-result/v1");
    assert_eq!(envelope["command"], "audit.anchors");
    assert_eq!(
        envelope["data"]["discrepancies"].as_array().unwrap().len(),
        0
    );
    // Exact counts, not just "some": a check that examined nothing would also
    // report zero discrepancies, and that must not read as healthy.
    assert_eq!(envelope["data"]["landing_commits_examined"], 1);
    assert_eq!(envelope["data"]["anchors_checked"], 1);
}

#[test]
fn a_rewritten_control_history_is_detected() {
    let workspace = Workspace::initialized();
    let (_first_id, first_landing_sha, first_anchor) =
        landed(&workspace, "C-001", "F-001", "src/F-001/a.rs");
    // A second landing, so the orphaned anchor sits on an OLDER landing than
    // the newest one reachable from the protected branch. An enumeration that
    // only inspected the newest landing would never see the corrupted one and
    // this test would pass for the wrong reason.
    let (_second_id, _second_landing_sha, second_anchor) =
        landed(&workspace, "C-002", "F-002", "src/F-002/a.rs");
    assert_ne!(
        first_anchor, second_anchor,
        "sanity: the two landings must anchor different control heads"
    );

    // Narrowly corrupt exactly the first anchor: a sibling commit with the
    // same tree (so `project/project.json`, and everything else the control
    // repository needs in order to stay readable, survives intact) built on
    // the anchor's own parent, so the anchor itself is no longer reachable
    // from `main` — the way #85's fixtures corrupt exactly one thing rather
    // than rebuilding history wholesale.
    let parent = support::capture(
        &workspace.control,
        &["rev-parse", &format!("{first_anchor}^")],
    );
    let tree = support::capture(
        &workspace.control,
        &["rev-parse", &format!("{first_anchor}^{{tree}}")],
    );
    let rewritten = support::capture(
        &workspace.control,
        &[
            "commit-tree",
            &tree,
            "-p",
            &parent,
            "-m",
            "test: rewritten control history",
        ],
    );
    support::git(&workspace.control, &["reset", "--hard", &rewritten]);

    // Sanity: the fixture must actually have built the case it claims to —
    // the object still present, just no longer an ancestor of the new head.
    assert!(support::object_exists(&workspace.control, &first_anchor));
    let still_ancestor = std::process::Command::new("git")
        .arg("-C")
        .arg(&workspace.control)
        .args(["merge-base", "--is-ancestor", &first_anchor, &rewritten])
        .status()
        .expect("git should run")
        .success();
    assert!(
        !still_ancestor,
        "the fixture must actually orphan the anchor"
    );

    let output = audit_anchors_raw(&workspace);
    assert!(!output.status.success(), "a rewritten anchor must not pass");
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-AUDIT-DISCREPANCY");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&first_anchor),
        "the orphaned anchor's SHA must be named: {message}"
    );
    assert!(
        message.contains(&first_landing_sha),
        "the landing commit that claimed it must be named: {message}"
    );
}

#[test]
fn appending_to_the_control_record_never_reports_a_discrepancy() {
    let workspace = Workspace::initialized();
    landed(&workspace, "C-001", "F-001", "src/F-001/a.rs");

    // Several further control-mutating commands, none of them another
    // landing — proving the check tolerates ordinary continued work, not
    // just more landings.
    workspace.register_gate("gate.extra", &["true"]);
    workspace.activate_card("F-002", &["src/F-002/**"]);
    workspace.approve_card("F-002", "src/F-002/a.rs");
    workspace.configure_convergence_policy(5, 5);

    // And a second landing too: growth by landing again must be exactly as
    // clean as growth by anything else.
    landed(&workspace, "C-002", "F-003", "src/F-003/a.rs");

    let envelope = audit_anchors_json(&workspace);
    assert!(
        envelope["data"]["discrepancies"]
            .as_array()
            .unwrap()
            .is_empty(),
        "ordinary further work must never be reported as tampering: {envelope}"
    );
    assert_eq!(envelope["data"]["landing_commits_examined"], 2);
    assert_eq!(envelope["data"]["anchors_checked"], 2);
}

#[test]
fn an_anchored_head_absent_from_the_control_repository_is_a_discrepancy_not_a_crash() {
    let (workspace, _id, _landing_sha, anchor) = single_landed();

    // Make the anchored control commit genuinely gone, not merely
    // unreachable. `reset --hard` alone leaves it reachable through the
    // reflog, which `gc --prune=now` respects (verified directly: a fresh
    // reflog entry survives `--prune=now` on its own), so the reflog is
    // expired first. `object_exists` below is not decoration; it is what
    // tells this test it actually built the case it claims to.
    support::git(
        &workspace.control,
        &["reset", "--hard", &format!("{anchor}^")],
    );
    support::git(
        &workspace.control,
        &["reflog", "expire", "--expire=now", "--all"],
    );
    support::git(&workspace.control, &["gc", "--prune=now", "--quiet"]);
    assert!(
        !support::object_exists(&workspace.control, &anchor),
        "the fixture must actually remove the anchored commit"
    );

    let output = audit_anchors_raw(&workspace);
    assert!(!output.status.success(), "an absent anchor must not pass");
    assert_eq!(
        output.status.code(),
        Some(5),
        "a discrepancy, not a crash or an unrelated tool failure, must be reported: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(error_code(&output), "CH-POLICY-AUDIT-DISCREPANCY");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&anchor),
        "the missing anchor must be named: {message}"
    );
    assert!(
        !message.contains("ancestor"),
        "case 3 (absent entirely) must read differently from case 2 (orphaned by a \
         rewrite), even though both are tampering: {message}"
    );
}

#[test]
fn the_command_changes_nothing() {
    let (workspace, _id, _landing_sha, _anchor) = single_landed();

    let authority_before = workspace.authority_head();
    let control_before = workspace.control_head();

    audit_anchors_json(&workspace);

    assert_eq!(
        workspace.authority_head(),
        authority_before,
        "the protected branch must not move"
    );
    assert_eq!(
        workspace.control_head(),
        control_before,
        "the control head must not move"
    );
    assert_eq!(
        support::capture(&workspace.control, &["status", "--porcelain"]),
        "",
        "the control worktree must stay clean"
    );
}
