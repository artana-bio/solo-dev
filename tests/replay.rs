//! `cycle replay` over a genuinely promoted cycle.
//!
//! The deriver's mapping is unit-tested in `src/cli/replay.rs` against
//! synthetic events; what only a real lifecycle can prove is that the events
//! the harness actually journals — their types, metadata keys, and SHAs —
//! are the ones the deriver reads. A drive-through here would have caught,
//! for example, a renamed metadata key that left every stamp blank.
//!
//! Every invocation runs with piped stdio, so the animation always skips and
//! what is under test is the honest degradation: the timeline on stdout, the
//! JSON envelope, warnings on stderr, and a stdout free of escape bytes.

mod support;

use std::{fs, process::Output};

use support::Workspace;

/// Runs `cycle replay` in default text mode.
///
/// The shared `cycle_raw` helper always injects `--output json`; the honest
/// degradation under test here — the plain timeline — only renders in text
/// mode, so this builds the invocation itself.
fn replay_text(workspace: &Workspace, cycle_id: &str) -> Output {
    Workspace::run(&[
        "cycle".to_owned(),
        "replay".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--cycle-id".to_owned(),
        cycle_id.to_owned(),
    ])
}

/// Drives one complete cycle to promotion, condensed from the Section 19.3
/// lifecycle drive: cycle, card, work, one passing gate, handoff, approving
/// review, integration through promotion.
fn promote_one_cycle(workspace: &Workspace, cycle: &str, card: &str) {
    workspace.cycle(&[
        "create",
        "--cycle-id",
        cycle,
        "--objective",
        "Replay the lifecycle",
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
    fs::write(
        worktree.join(format!("src/{card}/feature.txt")),
        format!("// {card}\n"),
    )
    .unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", &format!("feat: {card}")]);
    workspace.gate(&["run", "--card-id", card, "--gate-id", "gate.unit"]);

    let delivered = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join(format!("{card}-declaration.yaml"));
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {delivered}\nbehavior_delivered: adds a file\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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

    let integration =
        workspace.integration_json(&["prepare", "--cycle-id", cycle, "--actor-id", "coordinator"])
            ["data"]["integration_id"]
            .as_str()
            .unwrap()
            .to_owned();
    for step in ["merge", "land"] {
        workspace.integration(&[
            step,
            "--integration-id",
            &integration,
            "--actor-id",
            "coordinator",
        ]);
    }
    workspace.integration(&[
        "verify",
        "--integration-id",
        &integration,
        "--actor-id",
        "verifier",
    ]);
    workspace.integration(&[
        "review",
        "--integration-id",
        &integration,
        "--reviewer-actor-id",
        "integration-reviewer",
    ]);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &integration,
        "--acceptance-owner",
        "acceptance-owner",
    ]);
    workspace.integration(&[
        "promote",
        "--integration-id",
        &integration,
        "--actor-id",
        "promoter",
    ]);
}

#[test]
fn replay_of_a_promoted_cycle_reports_the_real_history() {
    let workspace = Workspace::initialized();
    promote_one_cycle(&workspace, "C-500", "F-500");

    // JSON mode: the machine-readable timeline.
    let envelope = workspace.cycle_json(&["replay", "--cycle-id", "C-500"]);
    assert_eq!(envelope["schema"], "harness.command-result/v1");
    assert_eq!(envelope["command"], "cycle.replay");
    assert_eq!(envelope["status"], "success");

    let data = &envelope["data"];
    assert_eq!(data["schema"], "harness.cycle-replay/v1");
    assert_eq!(data["cycle_id"], "C-500");
    assert_eq!(data["played"], false);
    assert_eq!(data["skip_reason"], "json_output");
    assert_eq!(data["discrepancies"].as_array().unwrap().len(), 0);

    let timeline = data["timeline"].as_array().unwrap();
    assert_eq!(
        timeline.len() as u64,
        data["event_count"].as_u64().unwrap(),
        "one timeline entry per journaled event"
    );
    assert!(
        data["beat_count"].as_u64().unwrap() >= data["event_count"].as_u64().unwrap(),
        "every event produces at least one beat"
    );

    let types: Vec<&str> = timeline
        .iter()
        .map(|entry| entry["event_type"].as_str().unwrap())
        .collect();
    for expected in [
        "cycle.created",
        "cycle.activated",
        "card.activated",
        "work.started",
        "gate.ran",
        "handoff.created",
        "review.recorded",
        "integration.prepared",
        "integration.verified",
        "integration.promoted",
    ] {
        assert!(types.contains(&expected), "timeline is missing {expected}");
    }

    // The promotion entry carries the real authority transition: the cycle's
    // frozen baseline on the left of the arrow.
    let baseline = data["baseline_sha"].as_str().unwrap();
    let promoted = timeline
        .iter()
        .find(|entry| entry["event_type"] == "integration.promoted")
        .expect("the promotion is in the timeline");
    let description = promoted["description"].as_str().unwrap();
    assert!(
        description.contains(&format!("authority advanced {}", &baseline[..7])),
        "promotion must name the real from-SHA: {description}"
    );

    // The handoff entry carries the real candidate SHA, not a placeholder.
    let stamped = timeline
        .iter()
        .find(|entry| entry["event_type"] == "handoff.created")
        .expect("the handoff is in the timeline");
    let stamped_description = stamped["description"].as_str().unwrap();
    assert!(
        stamped_description.contains("sealed F-500 at candidate"),
        "{stamped_description}"
    );
    assert!(
        !stamped_description.contains("candidate  "),
        "the candidate SHA must not be blank: {stamped_description}"
    );
}

#[test]
fn piped_text_mode_prints_the_timeline_and_keeps_the_streams_clean() {
    let workspace = Workspace::initialized();
    promote_one_cycle(&workspace, "C-510", "F-510");

    let output = replay_text(&workspace, "C-510");
    assert!(
        output.status.success(),
        "replay failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(stdout.contains("Replay of cycle C-510"), "{stdout}");
    assert!(
        stdout.contains("every recorded digest and commit still resolves"),
        "{stdout}"
    );
    assert!(stdout.contains("promoted — authority advanced"), "{stdout}");
    assert!(
        !stdout.contains('\u{1b}'),
        "stdout must never carry escape bytes: {stdout:?}"
    );
    assert!(
        output.stderr.is_empty(),
        "a clean replay writes nothing to stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn damaged_evidence_becomes_a_warning_and_a_reported_discrepancy_not_a_failure() {
    let workspace = Workspace::initialized();
    promote_one_cycle(&workspace, "C-520", "F-520");

    // Damage the evidence the way retention actually would: the receipt
    // still claims its logs, but the directory is gone.
    let logs = workspace.control.join("logs").join("F-520");
    assert!(logs.exists(), "the gate run must have written logs");
    fs::remove_dir_all(&logs).unwrap();

    let output = replay_text(&workspace, "C-520");
    assert!(
        output.status.success(),
        "replay is a viewer, not an auditor; a broken history must still show: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(stdout.contains("1 discrepancy(ies):"), "{stdout}");
    assert!(stdout.contains("the log directory is gone"), "{stdout}");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(
        stderr.contains("warning: evidence: receipt"),
        "the discrepancy must also warn on stderr: {stderr}"
    );

    // And the JSON payload carries it structurally.
    let envelope = workspace.cycle_json(&["replay", "--cycle-id", "C-520"]);
    let discrepancies = envelope["data"]["discrepancies"].as_array().unwrap();
    assert_eq!(discrepancies.len(), 1);
    assert!(
        discrepancies[0]["subject"]
            .as_str()
            .unwrap()
            .starts_with("receipt "),
        "{discrepancies:?}"
    );
}

#[test]
fn replaying_a_cycle_that_does_not_exist_is_an_error() {
    let workspace = Workspace::initialized();
    let output = replay_text(&workspace, "C-999");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("does not exist"), "{stderr}");
}
