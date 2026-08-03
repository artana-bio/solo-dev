//! Advisory convergence signals, end to end through the real command surface.
//!
//! Both checks are report-only, and that is the property most worth testing:
//! every case below asserts the command still *succeeded*. A signal that could
//! block would be a different feature, and a worse one — counting findings is
//! mechanical, deciding to split a card is judgment.

mod support;

use std::{fmt::Write as _, fs};

use support::Workspace;

/// Warnings are advisories on stderr; the envelope carries them too.
fn warnings(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn opened() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace
}

/// Stores a draft with an arbitrary include list, without activating it.
fn draft_with_scope(workspace: &Workspace, card_id: &str, include: &[String]) {
    let list = include
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{list}]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        base = workspace.authority_head(),
    );
    let path = workspace.root.join(format!("{card_id}.yaml"));
    fs::write(&path, body).unwrap();
    workspace.card(&["create", "--draft", &path.display().to_string()]);
}

/// Activates a card with an arbitrary include list.
fn activate_with_scope(
    workspace: &Workspace,
    card_id: &str,
    include: &[&str],
) -> std::process::Output {
    let owned: Vec<String> = include.iter().map(|value| (*value).to_owned()).collect();
    draft_with_scope(workspace, card_id, &owned);
    workspace.card_raw(&["activate", "--card-id", card_id])
}

#[test]
fn an_ordinary_card_activates_without_an_advisory() {
    // The half that keeps the other half usable. A signal that fires on
    // ordinary work is one people learn to skip.
    let workspace = opened();
    let output = activate_with_scope(
        &workspace,
        "F-001",
        &["src/policy/actors.rs", "tests/promotion.rs"],
    );

    assert!(output.status.success());
    assert!(
        !warnings(&output).contains("independently reviewable outcome"),
        "no advisory for a narrow card: {}",
        warnings(&output)
    );
}

#[test]
fn a_broad_card_is_flagged_at_activation_and_still_activates() {
    // F-027's declared scope, which ran eight review rounds and a split. The
    // whole point is that this was knowable before any work started.
    let workspace = opened();
    let output = activate_with_scope(
        &workspace,
        "F-001",
        &[
            ".claude/skills/change-harness/SKILL.md",
            "docs/IMPLEMENTATION_PLAN.md",
            "src/cli/output.rs",
            "src/commands/acceptance.rs",
            "src/commands/gate.rs",
            "src/commands/integration.rs",
            "src/control/repository.rs",
            "src/domain/card.rs",
            "src/domain/gate.rs",
            "src/domain/review.rs",
            "src/error.rs",
            "src/main.rs",
            "src/policy/actors.rs",
            "src/runner/mod.rs",
            "tests/authority.rs",
            "tests/promotion.rs",
        ],
    );

    assert!(
        output.status.success(),
        "the advisory must never block activation"
    );
    let warned = warnings(&output);
    assert!(warned.contains("16 path(s)"), "{warned}");
    assert!(
        warned.contains("splitting now is far cheaper"),
        "an advisory has to say what to do about it: {warned}"
    );

    // And the card really is activated, not half-written.
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "ready"
    );
}

#[test]
fn the_activation_envelope_carries_the_scope_counts() {
    // A program driving this CLI cannot read a warning off stderr and act on
    // it, so the envelope is the only place these numbers are usable.
    //
    // Round 1 of this card's own review: the test that claimed this coverage
    // never opened the activation output. It asserted that a later `card
    // status` succeeded, which stayed true with both fields deleted.
    let workspace = opened();
    let output = activate_with_scope(
        &workspace,
        "F-001",
        &["src/policy/a.rs", "src/runner/b.rs", "tests/c.rs"],
    );
    assert!(output.status.success());

    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("activation must emit the JSON envelope on stdout");
    assert_eq!(envelope["data"]["scope_paths"], 3, "{envelope}");
    assert_eq!(envelope["data"]["scope_areas"], 3, "{envelope}");
}

#[test]
#[cfg(unix)]
fn a_warning_that_cannot_be_printed_leaves_the_exit_status_alone() {
    // Round 1 of this card's own review, and the finding that mattered most.
    //
    // `eprintln!` panics when the write fails. The advisory is printed after
    // the command has succeeded and its state change is committed, so a closed
    // stderr turned a card that really had been activated into exit 101. An
    // advisory that can change the exit status is not an advisory, and this
    // card's whole claim is that neither check can refuse anything.
    //
    // Stderr is a pipe whose read end is already closed, so the write fails
    // with `EPIPE` and stays failed. Closing descriptor 2 instead does *not*
    // reproduce this: the first file the harness opens is handed the lowest
    // free descriptor, which is then 2, and the advisory writes into it
    // successfully. That version of this test passed against the unfixed
    // binary, which is the only reason this note exists.
    let workspace = opened();
    let broad: Vec<String> = (0..20).map(|index| format!("src/f{index}.rs")).collect();
    draft_with_scope(&workspace, "F-001", &broad);

    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);

    let child = std::process::Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args([
            "card",
            "activate",
            "--output",
            "json",
            "--control",
            &workspace.control.display().to_string(),
            "--card-id",
            "F-001",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::from(writer))
        .spawn()
        .expect("the CLI should start");
    let output = child.wait_with_output().expect("the CLI should finish");

    assert_eq!(
        output.status.code(),
        Some(0),
        "an unprintable advisory must not change the exit status (101 is a panic)"
    );

    // And the half that makes the assertion above mean something: the command
    // really did reach the advisory, having already committed its work.
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the envelope still reaches stdout");
    assert_eq!(envelope["status"], "success", "{envelope}");
    assert!(
        envelope["warnings"]
            .as_array()
            .is_some_and(|warnings| warnings.iter().any(|warning| warning
                .as_str()
                .is_some_and(|text| text.contains("independently reviewable outcome")))),
        "the warning that could not be printed is still in the envelope: {envelope}"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "ready",
        "the activation it could not report stands"
    );
}

#[test]
#[cfg(unix)]
fn a_result_that_cannot_be_printed_does_not_turn_a_committed_activation_into_a_panic() {
    // The result is emitted after `card activate` has committed. A consumer
    // which has stopped reading stdout must not make that completed mutation
    // look like a process crash (exit 101), because a caller could then retry
    // an activation that already landed.
    let workspace = opened();
    draft_with_scope(&workspace, "F-001", &["src/policy/actors.rs".to_owned()]);

    // A pipe with no reader is required to create a stable EPIPE. Closing
    // descriptor 1 instead can cause a later file open to reuse it.
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);

    let child = std::process::Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args([
            "card",
            "activate",
            "--output",
            "json",
            "--control",
            &workspace.control.display().to_string(),
            "--card-id",
            "F-001",
        ])
        .stdout(std::process::Stdio::from(writer))
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the CLI should start");
    let output = child.wait_with_output().expect("the CLI should finish");

    assert_eq!(
        output.status.code(),
        Some(0),
        "a broken result pipe must not turn completed work into exit 101: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "ready",
        "the activation that could not be printed stands"
    );
}

/// Drives a card to a recorded review carrying the given open finding
/// locations, leaving it ready for the next round.
fn review_round(
    workspace: &Workspace,
    card_id: &str,
    round: usize,
    prior_open: &[&str],
    open: &[&str],
) -> std::process::Output {
    let worktree = workspace.worktrees.join(card_id);
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(
        worktree.join("src/a.rs"),
        format!("// round {round}\nfn main() {{}}\n"),
    )
    .unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(
        &worktree,
        &["commit", "-q", "-m", &format!("feat: round {round}")],
    );
    workspace.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: round {round}\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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
    workspace.review(&["begin", "--card-id", card_id, "--actor", "reviewer"]);

    // A re-review may not silently drop the previous round's open findings:
    // each must reappear at the same location with a non-blocking disposition.
    // A location that is *still* open therefore appears twice — resolved, to
    // satisfy the carry rule, and open, to state the truth. That the harness
    // has no way to say "still open" directly is filed as
    // artana-bio/solo-dev#14; here it is just the shape a fixture must take.
    let mut findings = String::new();
    for location in prior_open {
        writeln!(
            findings,
            "  - severity: medium\n    location: {location}\n    detail: carried forward from the previous round\n    disposition: resolved"
        )
        .unwrap();
    }
    for location in open {
        writeln!(
            findings,
            "  - severity: medium\n    location: {location}\n    detail: something is wrong\n    disposition: open"
        )
        .unwrap();
    }
    let verdict_body = format!(
        "reviewer_actor_id: reviewer\ndecision: changes_requested\nfindings:\n{findings}gate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n"
    );
    let verdict = workspace.root.join("verdict.yaml");
    fs::write(&verdict, verdict_body).unwrap();

    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        card_id,
        "--verdict",
        &verdict.display().to_string(),
        "--actor",
        "reviewer",
    ]);
    assert!(
        output.status.success(),
        "round {round} must record: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    workspace.work(&["resume", "--card-id", card_id]);
    output
}

#[test]
fn the_first_rounds_say_nothing_and_a_converging_card_stays_quiet() {
    let workspace = opened();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let first = review_round(
        &workspace,
        "F-001",
        1,
        &[],
        &["src/a.rs", "src/b.rs", "src/c.rs"],
    );
    assert!(
        !warnings(&first).contains("review round"),
        "one round is not a trend"
    );

    let second = review_round(
        &workspace,
        "F-001",
        2,
        &["src/a.rs", "src/b.rs", "src/c.rs"],
        &["src/a.rs", "src/b.rs"],
    );
    assert!(
        !warnings(&second).contains("review round"),
        "two is not either"
    );

    // Third round exists, but the count is falling and nothing new appeared.
    let third = review_round(
        &workspace,
        "F-001",
        3,
        &["src/a.rs", "src/b.rs"],
        &["src/a.rs"],
    );
    assert!(
        !warnings(&third).contains("review round"),
        "a card that is settling must not be nagged: {}",
        warnings(&third)
    );
}

#[test]
fn review_history_is_ordered_across_an_identifier_width_boundary() {
    // Round 5 of this card's own review. The unit test binds to
    // `sort_oldest_first`, so replacing the call inside `reviews_for` with a
    // plain `names.sort()` left the helper tests, the convergence tests and
    // the review tests all green while restoring the defect in full — clippy
    // noticed only that the helper had become dead code. This drives the real
    // reader instead of the function it is supposed to use.
    let workspace = opened();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    review_round(&workspace, "F-001", 1, &[], &["src/a.rs"]);

    // Move the first review to the last identifier before the width changes,
    // so the allocator's next id is RV-1000000 — which sorts *before*
    // RV-999999 as text, and after it as a number.
    let reviews = workspace.control.join("reviews");
    let first = reviews.join("RV-000001.json");
    let body = fs::read_to_string(&first)
        .unwrap()
        .replace("RV-000001", "RV-999999");
    fs::write(reviews.join("RV-999999.json"), body).unwrap();
    fs::remove_file(&first).unwrap();

    review_round(&workspace, "F-001", 2, &["src/a.rs"], &["src/a.rs"]);

    let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let ids: Vec<&str> = inspected["data"]["reviews"]
        .as_array()
        .expect("reviews are an array")
        .iter()
        .map(|review| review["review_id"].as_str().expect("each carries its id"))
        .collect();
    assert_eq!(
        ids,
        vec!["RV-999999", "RV-1000000"],
        "oldest first; as text RV-1000000 would come first"
    );
}

#[test]
fn findings_that_stay_flat_are_flagged_and_the_record_is_still_written() {
    let workspace = opened();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    review_round(&workspace, "F-001", 1, &[], &["src/a.rs", "src/b.rs"]);
    review_round(
        &workspace,
        "F-001",
        2,
        &["src/a.rs", "src/b.rs"],
        &["src/a.rs", "src/b.rs"],
    );
    let third = review_round(
        &workspace,
        "F-001",
        3,
        &["src/a.rs", "src/b.rs"],
        &["src/a.rs", "src/b.rs"],
    );

    let warned = warnings(&third);
    assert!(warned.contains("review round 3"), "{warned}");
    assert!(warned.contains("2 → 2 → 2"), "{warned}");
    assert!(warned.contains("not falling"), "{warned}");
    assert!(
        warned.contains("not a refusal"),
        "the advisory must say it is advisory: {warned}"
    );

    // Report-only: the verdict is in the record regardless.
    let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    assert_eq!(
        inspected["data"]["reviews"].as_array().unwrap().len(),
        3,
        "every round was recorded"
    );
}

#[test]
fn findings_moving_to_new_areas_are_flagged_even_while_the_count_falls() {
    // The F-027 shape, and the reason a bare count is the wrong measure: the
    // total came down every round while each round found a defect somewhere
    // the last had not looked.
    let workspace = opened();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    review_round(
        &workspace,
        "F-001",
        1,
        &[],
        &["src/a.rs", "src/b.rs", "src/c.rs"],
    );
    review_round(
        &workspace,
        "F-001",
        2,
        &["src/a.rs", "src/b.rs", "src/c.rs"],
        &["src/a.rs", "src/d.rs"],
    );
    let third = review_round(
        &workspace,
        "F-001",
        3,
        &["src/a.rs", "src/d.rs"],
        &["src/e.rs"],
    );

    let warned = warnings(&third);
    assert!(
        warned.contains("no earlier round named"),
        "spreading is a signal on its own: {warned}"
    );
    assert!(warned.contains("src/e.rs"), "and it names where: {warned}");
}
