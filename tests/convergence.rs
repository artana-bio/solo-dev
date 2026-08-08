//! Advisory convergence signals, end to end through the real command surface.
//!
//! Both checks are report-only, and that is the property most worth testing:
//! every case below asserts the command still *succeeded*. A signal that could
//! block would be a different feature, and a worse one — counting findings is
//! mechanical, deciding to split a card is judgment.

mod support;

use std::{collections::BTreeSet, fmt::Write as _, fs};

use change_harness::{
    control::{event_store::Event, repository::ControlRepository},
    domain::ids::{CycleId, ProjectId},
    policy::convergence::{ProjectConvergence, project},
};
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

/// Like [`opened`], but with a convergence policy configured first.
///
/// A cycle pins the project configuration's digest at the moment it is
/// created — `cycle create` after `configure_convergence_policy` refused
/// every later command in this project with `CH-POLICY-INVALID-CYCLE`,
/// "cycle C-001 was created under project revision ..., but the current
/// project configuration is ...", because the project document had changed
/// out from under an already-created cycle. The policy has to be in place
/// before the cycle is, not after.
fn opened_with_policy(card_limit: u32, integration_limit: u32) -> Workspace {
    let workspace = Workspace::initialized();
    workspace.configure_convergence_policy(card_limit, integration_limit);
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

#[test]
#[cfg(unix)]
fn a_json_error_that_cannot_be_printed_reports_its_category_instead_of_panicking() {
    // `card status --card-id F-001` on a workspace where F-001 was never
    // created fails before touching any state: `stored_draft` finds no
    // draft file on disk and returns `CH-PRECONDITION-NOT-FOUND` (exit 4),
    // without the command having opened anything for writing. The JSON
    // error envelope is written to stdout; a caller that closed stdout must
    // still see PRECONDITION, not a panic that reads as a harness crash.
    let workspace = opened();
    let before = workspace.control_head();

    // A pipe with no reader is required to create a stable EPIPE. Closing
    // descriptor 1 instead can cause a later file open to reuse it.
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);

    let child = std::process::Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args([
            "card",
            "status",
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

    // The assertion the test exists for: a bare `println!` (or a "fix" that
    // still panics on the nested render-failure arm) exits 101 here instead,
    // because the write panics before `main` ever reaches
    // `ExitCode::from(error.category())`.
    assert_eq!(
        output.status.code(),
        Some(4),
        "an unwritable JSON error envelope must still report PRECONDITION (4), not panic (101): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // And the half that makes the assertion above mean something: a
    // reporting path that mutated control state on its way to failing —
    // for instance, one that tried to persist a fallback record before
    // giving up on stdout — would move this SHA, and would be a worse
    // defect than the crash it was trying to avoid.
    assert_eq!(
        workspace.control_head(),
        before,
        "a report that could not be printed must not touch control state either"
    );
}

#[test]
#[cfg(unix)]
fn a_text_error_that_cannot_be_printed_reports_its_category_instead_of_panicking() {
    // Same failure, text mode: `render_text_error` writes the "error: ..."
    // line to stderr instead of the envelope going to stdout (text is the
    // default format, so `--output` is omitted here), so it is stderr that
    // is broken in this test and stdout that is captured for inspection.
    let workspace = opened();
    let before = workspace.control_head();

    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);

    let child = std::process::Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args([
            "card",
            "status",
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
        Some(4),
        "an unwritable text error line must still report PRECONDITION (4), not panic (101); stdout carried: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "a report that could not be printed must not touch control state either"
    );
}

#[test]
#[cfg(unix)]
fn a_malformed_invocation_that_cannot_be_printed_reports_usage_instead_of_panicking() {
    // `parse_or_report` in `src/main.rs` renders the JSON envelope for a clap
    // parse failure through the same locked-handle-with-discarded-result
    // pattern as the write above, but nothing had exercised *this* site under
    // a broken pipe. Unlike the other broken-pipe tests in this file, there is
    // no workspace or control state to check: `Cli::try_parse` fails before
    // any of that is ever resolved, so the exit code is the whole assertion.
    //
    // `project status --output json`, with no `--control` flag and
    // `CHANGE_HARNESS_CONTROL` scrubbed from the environment, is the same
    // malformed invocation `an_argument_error_still_honours_the_json_contract`
    // in `tests/cli.rs` drives under a normal, readable pipe: clap's own
    // "required argument `--control` was not provided", asked for in JSON.
    //
    // The assertion this test exists for: a bare `println!` in place of the
    // locked write here exits 101 instead, because the write panics before
    // `parse_or_report` ever returns `Err(ExitCode::from(ExitCategory::Usage))`.
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);

    let child = std::process::Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(["project", "status", "--output", "json"])
        .env_remove("CHANGE_HARNESS_CONTROL")
        .stdout(std::process::Stdio::from(writer))
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the CLI should start");
    let output = child.wait_with_output().expect("the CLI should finish");

    assert_eq!(
        output.status.code(),
        Some(2),
        "an unwritable JSON envelope for a parse failure must still report USAGE (2), not panic (101): {}",
        String::from_utf8_lossy(&output.stderr)
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
        "reviewer_actor_id: reviewer\ndecision: changes_requested\nfindings:\n{findings}gate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n"
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

#[test]
fn card_status_projects_a_broad_card_as_an_advisory_for_agents() {
    let workspace = opened();
    let paths = (0..13)
        .map(|index| format!("src/policy/file-{index}.rs"))
        .collect::<Vec<_>>();
    draft_with_scope(&workspace, "F-001", &paths);
    workspace.card(&["activate", "--card-id", "F-001"]);

    let status = workspace.card_json(&["status", "--card-id", "F-001"]);
    let bottleneck = &status["data"]["bottleneck"];
    assert_eq!(bottleneck["schema"], "harness.bottleneck-projection/v1");
    assert_eq!(bottleneck["status"], "advisory");
    assert_eq!(bottleneck["attempt_coverage"], "legacy_unassessed");
    assert_eq!(bottleneck["recommended_action"], "consider_card_split");
    assert!(bottleneck["authority_action"].is_null());
    assert_eq!(bottleneck["signals"][0]["kind"], "broad_scope");
    assert_eq!(bottleneck["signals"][0]["severity"], "advisory");
    assert_eq!(bottleneck["signals"][0]["count"], 13);
    assert_eq!(bottleneck["signals"][0]["threshold"], 12);
}

#[test]
fn status_surfaces_a_review_plateau_and_project_status_aggregates_it() {
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
    review_round(
        &workspace,
        "F-001",
        3,
        &["src/a.rs", "src/b.rs"],
        &["src/a.rs", "src/b.rs"],
    );

    let status = workspace.card_json(&["status", "--card-id", "F-001"]);
    let bottleneck = &status["data"]["bottleneck"];
    assert_eq!(bottleneck["status"], "attention_required");
    assert_eq!(bottleneck["recommended_action"], "convene_bottleneck_group");
    assert!(
        bottleneck["signals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|signal| signal["kind"] == "review_plateau")
    );

    let output = Workspace::run(&[
        "project".to_owned(),
        "status".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    assert!(
        output.status.success(),
        "project status failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let project: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(project["data"]["bottleneck_count"], 1);
    assert_eq!(project["data"]["bottlenecks"][0]["card_id"], "F-001");
    assert_eq!(
        project["data"]["bottlenecks"][0]["bottleneck"]["status"],
        "attention_required"
    );
    assert!(
        project["warnings"][0]
            .as_str()
            .unwrap()
            .contains("convene_bottleneck_group")
    );
}

// 71-R2: a `changes_requested` review return, under a configured convergence
// policy, declares its reason and appends exactly one bound
// `convergence.attempt_recorded` fact in the same transaction as
// `review.recorded`. Unlike the advisories above, this is a hard refusal, not
// a warning — the tests below assert failure, not just an absent message.

/// Activates a card, drives one commit through gate and handoff, and opens a
/// review round for it. Returns the candidate commit the handoff bound, for
/// asserting the attempt fact's `head_sha` binds to the same commit.
fn open_review_round(workspace: &Workspace, card_id: &str) -> String {
    workspace.activate_card(card_id, &["src/**"]);
    workspace.work(&["start", "--card-id", card_id]);

    let worktree = workspace.worktrees.join(card_id);
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: implement"]);
    workspace.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join(format!("{card_id}-declaration.yaml"));
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: it works\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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
    head
}

/// Writes a verdict body to disk and returns its path.
fn write_verdict(workspace: &Workspace, card_id: &str, body: &str) -> String {
    let path = workspace.root.join(format!("{card_id}-verdict.yaml"));
    fs::write(&path, body).unwrap();
    path.display().to_string()
}

/// Every `convergence.attempt_recorded` fact in the control repository.
fn attempt_recorded_events(workspace: &Workspace) -> Vec<serde_json::Value> {
    workspace
        .events()
        .into_iter()
        .filter(|event| event["event_type"] == "convergence.attempt_recorded")
        .collect()
}

/// A card's stored review count, read back through `review inspect`.
fn recorded_review_count(workspace: &Workspace, card_id: &str) -> usize {
    workspace.review_json(&["inspect", "--card-id", card_id])["data"]["reviews"]
        .as_array()
        .expect("reviews is an array")
        .len()
}

/// The stable error code from a failed command's JSON envelope.
fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"]
        .as_str()
        .expect("a coded refusal")
        .to_owned()
}

/// A `changes_requested` verdict declaring `reason_category: acceptance_defect`.
const RETURN_WITH_ACCEPTANCE_DEFECT_REASON: &str = "reviewer_actor_id: reviewer\ndecision: changes_requested\nreason_category: acceptance_defect\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: something is wrong\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";

/// A `changes_requested` verdict with no `reason_category` declared at all.
const RETURN_WITH_NO_REASON: &str = "reviewer_actor_id: reviewer\ndecision: changes_requested\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: something is wrong\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";

/// A `changes_requested` verdict declaring `reason_category: scope_change`,
/// which is `MaterialScopeRevision`'s reason, not a review return's.
const RETURN_WITH_SCOPE_CHANGE_REASON: &str = "reviewer_actor_id: reviewer\ndecision: changes_requested\nreason_category: scope_change\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: something is wrong\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";

#[test]
fn a_returned_review_under_a_configured_policy_records_one_bound_attempt_fact() {
    let workspace = opened_with_policy(3, 3);
    let head = open_review_round(&workspace, "F-001");

    let path = write_verdict(&workspace, "F-001", RETURN_WITH_ACCEPTANCE_DEFECT_REASON);
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer",
    ]);
    assert!(
        output.status.success(),
        "a declared, admissible reason must be accepted: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let review_id = envelope["data"]["review"]["review_id"]
        .as_str()
        .expect("the recorded review's id");

    let card_status = workspace.card_json(&["status", "--card-id", "F-001"]);

    let attempts = attempt_recorded_events(&workspace);
    assert_eq!(
        attempts.len(),
        1,
        "exactly one attempt fact must be recorded: {attempts:?}"
    );
    let fact = &attempts[0];
    assert_eq!(fact["card_id"], "F-001", "{fact}");
    assert_eq!(
        fact["card_revision"], card_status["data"]["revision"],
        "{fact}"
    );
    assert_eq!(fact["card_digest"], card_status["data"]["digest"], "{fact}");
    assert_eq!(fact["cycle_id"], "C-001", "{fact}");
    assert_eq!(fact["head_sha"], head, "{fact}");
    assert_eq!(fact["metadata"]["attempt_kind"], "review_return", "{fact}");
    assert_eq!(
        fact["metadata"]["reason_category"], "acceptance_defect",
        "{fact}"
    );
    assert_eq!(
        fact["metadata"]["evidence_ref"],
        format!("review:{review_id}"),
        "{fact}"
    );
    let policy_digest = fact["metadata"]["policy_digest"]
        .as_str()
        .expect("policy_digest is a string");
    assert!(
        policy_digest.starts_with("sha256:") && policy_digest.len() == "sha256:".len() + 64,
        "policy_digest must be a real digest, not a placeholder: {policy_digest}"
    );
}

#[test]
fn a_returned_review_without_a_declared_reason_is_refused_before_anything_is_written() {
    let workspace = opened_with_policy(3, 3);
    open_review_round(&workspace, "F-001");

    let before_head = workspace.control_head();
    let before_reviews = recorded_review_count(&workspace, "F-001");

    let path = write_verdict(&workspace, "F-001", RETURN_WITH_NO_REASON);
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer",
    ]);

    assert!(
        !output.status.success(),
        "a changes-requested verdict with no declared reason must be refused"
    );
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
    assert_eq!(
        workspace.control_head(),
        before_head,
        "the control repository head must not move on refusal"
    );
    assert_eq!(
        recorded_review_count(&workspace, "F-001"),
        before_reviews,
        "no new review record may exist after a refusal"
    );
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "a refused review must not record an attempt fact either"
    );
}

#[test]
fn a_reason_a_review_return_cannot_have_is_refused() {
    let workspace = opened_with_policy(3, 3);
    open_review_round(&workspace, "F-001");

    let path = write_verdict(&workspace, "F-001", RETURN_WITH_SCOPE_CHANGE_REASON);
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer",
    ]);

    assert!(
        !output.status.success(),
        "scope_change is material-scope-revision's reason, not a review return's"
    );
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "a refused review must not record an attempt fact"
    );
}

#[test]
fn an_approval_records_no_attempt_fact() {
    let workspace = opened_with_policy(3, 3);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.approve_card("F-001", "src/a.rs");

    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "approved"
    );
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "an approval must not record an attempt fact"
    );
}

#[test]
fn a_project_without_a_convergence_policy_records_no_attempt_fact() {
    // No `configure_convergence_policy` call: every fixture used elsewhere in
    // this file runs exactly this way, unconfigured, which is what this test
    // asserts is safe by construction rather than by omission.
    let workspace = opened();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    review_round(&workspace, "F-001", 1, &[], &["src/a.rs"]);

    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "no configured policy means no attempt fact, however the decision reads"
    );
}

#[test]
fn the_dry_run_preview_refuses_the_same_missing_reason() {
    let workspace = opened_with_policy(3, 3);
    open_review_round(&workspace, "F-001");

    let path = write_verdict(&workspace, "F-001", RETURN_WITH_NO_REASON);
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer",
        "--dry-run",
    ]);

    assert!(
        !output.status.success(),
        "the dry run must refuse the same missing reason the real command does"
    );
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-REVIEW");
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "a dry run must never write a fact"
    );
    assert_eq!(
        recorded_review_count(&workspace, "F-001"),
        0,
        "a dry run must not record a review either"
    );
}

// 71-R5: a material scope revision, under a configured convergence policy,
// records exactly one bound `convergence.attempt_recorded` fact of class
// `material_scope_revision`, in the same transaction as `card.revised`. A
// revision that leaves every canonical field untouched — write scope,
// acceptance, dependencies, base commit — records nothing, however different
// its title or goal read: rewording is not a scope revision, and counting it
// would exhaust the budget on administrative work.

/// A revision draft for card `F-001`, every field steady except `title`,
/// `goal`, the write-scope `include` list, and `acceptance.behaviors` — the
/// only fields any fixture below needs to move. `write_scope.exclude`,
/// `acceptance.regressions` and `depends_on` never vary in this file, so they
/// stay fixed here rather than threaded through every call.
fn revision_body(
    title: &str,
    goal: &str,
    base_sha: &str,
    include: &[&str],
    behaviors: &[&str],
) -> String {
    let list = |values: &[&str]| {
        values
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "card_id: F-001\ncycle_id: C-001\ntitle: {title}\ngoal: {goal}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base_sha}\nwrite_scope:\n  include: [{inc}]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [{beh}]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        inc = list(include),
        beh = list(behaviors),
    )
}

/// Runs `card revise` against a prepared draft body for `F-001`, returning
/// the raw output so a test can inspect success and the JSON envelope alike.
fn revise_raw(
    workspace: &Workspace,
    body: &str,
    reason: &str,
    dry_run: bool,
) -> std::process::Output {
    let path = workspace.root.join("F-001-revision.yaml");
    fs::write(&path, body).unwrap();
    let draft = path.display().to_string();
    let mut args = vec![
        "revise",
        "--card-id",
        "F-001",
        "--draft",
        draft.as_str(),
        "--reason",
        reason,
    ];
    if dry_run {
        args.push("--dry-run");
    }
    workspace.card_raw(&args)
}

#[test]
fn a_revision_that_widens_write_scope_records_one_bound_material_scope_fact() {
    let workspace = opened_with_policy(3, 3);
    let base = workspace.authority_head();
    workspace.activate_card_with_base("F-001", &["src/a.rs"], &base);

    let widened = revision_body(
        "Implement F-001",
        "Deliver F-001",
        &base,
        &["src/a.rs", "src/b.rs"],
        &["it works"],
    );
    let output = revise_raw(&workspace, &widened, "widen scope", false);
    assert!(
        output.status.success(),
        "a material revision must still succeed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let card_status = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(card_status["data"]["revision"], 2, "{card_status}");

    let attempts = attempt_recorded_events(&workspace);
    assert_eq!(
        attempts.len(),
        1,
        "exactly one attempt fact must be recorded: {attempts:?}"
    );
    let fact = &attempts[0];
    assert_eq!(fact["card_id"], "F-001", "{fact}");
    assert_eq!(fact["card_revision"], 2, "{fact}");
    assert_eq!(fact["card_digest"], card_status["data"]["digest"], "{fact}");
    assert_eq!(fact["cycle_id"], "C-001", "{fact}");
    assert_eq!(fact["head_sha"], base, "{fact}");
    assert_eq!(
        fact["metadata"]["attempt_kind"], "material_scope_revision",
        "{fact}"
    );
    assert_eq!(
        fact["metadata"]["reason_category"], "scope_change",
        "{fact}"
    );
    assert_eq!(
        fact["metadata"]["evidence_ref"], "card-revision:F-001@2",
        "{fact}"
    );
    let policy_digest = fact["metadata"]["policy_digest"]
        .as_str()
        .expect("policy_digest is a string");
    assert!(
        policy_digest.starts_with("sha256:") && policy_digest.len() == "sha256:".len() + 64,
        "policy_digest must be a real digest, not a placeholder: {policy_digest}"
    );
}

#[test]
fn a_revision_that_only_rewords_the_card_records_no_fact() {
    let workspace = opened_with_policy(3, 3);
    let base = workspace.authority_head();
    workspace.activate_card_with_base("F-001", &["src/a.rs"], &base);

    let reworded = revision_body(
        "Implement F-001, take two",
        "Deliver F-001, revisited",
        &base,
        &["src/a.rs"],
        &["it works"],
    );
    let output = revise_raw(&workspace, &reworded, "reword only", false);
    assert!(
        output.status.success(),
        "a non-material revision must still succeed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["revision"],
        2,
        "the revision itself must still be recorded"
    );
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "same scope, acceptance, dependencies and base — only title and goal moved — must record no fact"
    );
}

#[test]
fn a_revision_that_changes_acceptance_behaviors_is_material() {
    let workspace = opened_with_policy(3, 3);
    let base = workspace.authority_head();
    workspace.activate_card_with_base("F-001", &["src/a.rs"], &base);

    let wider_acceptance = revision_body(
        "Implement F-001",
        "Deliver F-001",
        &base,
        &["src/a.rs"],
        &["it works", "it also handles the edge case"],
    );
    let output = revise_raw(&workspace, &wider_acceptance, "widen acceptance", false);
    assert!(
        output.status.success(),
        "a material revision must still succeed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let attempts = attempt_recorded_events(&workspace);
    assert_eq!(
        attempts.len(),
        1,
        "an acceptance change must count even though the write scope did not move: {attempts:?}"
    );
    assert_eq!(
        attempts[0]["metadata"]["attempt_kind"], "material_scope_revision",
        "{:?}",
        attempts[0]
    );
}

#[test]
fn a_project_without_a_convergence_policy_records_no_fact_when_revising() {
    let workspace = opened();
    let base = workspace.authority_head();
    workspace.activate_card_with_base("F-001", &["src/a.rs"], &base);

    let widened = revision_body(
        "Implement F-001",
        "Deliver F-001",
        &base,
        &["src/a.rs", "src/b.rs"],
        &["it works"],
    );

    // Material but unpoliced is the one combination worth pinning here,
    // rather than in a separate test: it is the only case where materiality
    // and recording disagree — the revision is material, but nothing is ever
    // recorded because no policy is configured — so it is the only case that
    // can catch `material_scope_revision_recorded` being computed from
    // materiality alone, with the policy condition dropped. Every other
    // fixture in this file either has a policy configured, where the two
    // coincide, or is not material, where both are trivially `false`.
    let preview = revise_raw(&workspace, &widened, "widen scope", true);
    assert!(
        preview.status.success(),
        "{}{}",
        String::from_utf8_lossy(&preview.stdout),
        String::from_utf8_lossy(&preview.stderr)
    );
    let preview_envelope: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    let preview_recorded = preview_envelope["data"]["material_scope_revision_recorded"].as_bool();
    assert_eq!(
        preview_recorded,
        Some(false),
        "no configured policy means the preview must report no fact would be recorded, however material the revision reads: {preview_envelope}"
    );

    let output = revise_raw(&workspace, &widened, "widen scope", false);
    assert!(
        output.status.success(),
        "revising without a configured policy must still succeed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let real_envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let real_recorded = real_envelope["data"]["material_scope_revision_recorded"].as_bool();
    assert_eq!(
        real_recorded,
        Some(false),
        "no configured policy means the real command must report no fact was recorded, however material the revision reads: {real_envelope}"
    );
    assert_eq!(
        preview_recorded, real_recorded,
        "the preview and the real command must agree, which is the property this field exists to make checkable"
    );

    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "no configured policy means no fact, however material the revision reads"
    );
}

#[test]
fn the_dry_run_reports_the_same_materiality_the_real_command_records() {
    let workspace = opened_with_policy(3, 3);
    let base = workspace.authority_head();
    workspace.activate_card_with_base("F-001", &["src/a.rs"], &base);

    // A material revision: the preview must say so, and the real command must
    // then actually record the one fact it promised.
    let widened = revision_body(
        "Implement F-001",
        "Deliver F-001",
        &base,
        &["src/a.rs", "src/b.rs"],
        &["it works"],
    );
    let preview = revise_raw(&workspace, &widened, "widen scope", true);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stdout)
    );
    let preview_envelope: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(
        preview_envelope["data"]["material_scope_revision_recorded"].as_bool(),
        Some(true),
        "the preview must predict a material revision: {preview_envelope}"
    );
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "a dry run must never write a fact"
    );

    let real = revise_raw(&workspace, &widened, "widen scope", false);
    assert!(
        real.status.success(),
        "{}",
        String::from_utf8_lossy(&real.stdout)
    );
    let real_envelope: serde_json::Value = serde_json::from_slice(&real.stdout).unwrap();
    assert_eq!(
        real_envelope["data"]["material_scope_revision_recorded"].as_bool(),
        Some(true),
        "the real command's own report must match what it predicted: {real_envelope}"
    );
    assert_eq!(
        attempt_recorded_events(&workspace).len(),
        1,
        "the preview predicted a fact and the real command must have recorded exactly one"
    );

    // The non-material direction, against the card the first half just moved
    // to revision 2: reword only.
    let reworded = revision_body(
        "Implement F-001, reworded",
        "Deliver F-001",
        &base,
        &["src/a.rs", "src/b.rs"],
        &["it works"],
    );
    let preview_reword = revise_raw(&workspace, &reworded, "reword only", true);
    assert!(
        preview_reword.status.success(),
        "{}",
        String::from_utf8_lossy(&preview_reword.stdout)
    );
    let preview_reword_envelope: serde_json::Value =
        serde_json::from_slice(&preview_reword.stdout).unwrap();
    assert_eq!(
        preview_reword_envelope["data"]["material_scope_revision_recorded"].as_bool(),
        Some(false),
        "the preview must predict no fact for a reword-only revision: {preview_reword_envelope}"
    );

    let real_reword = revise_raw(&workspace, &reworded, "reword only", false);
    assert!(
        real_reword.status.success(),
        "{}",
        String::from_utf8_lossy(&real_reword.stdout)
    );
    let real_reword_envelope: serde_json::Value =
        serde_json::from_slice(&real_reword.stdout).unwrap();
    assert_eq!(
        real_reword_envelope["data"]["material_scope_revision_recorded"].as_bool(),
        Some(false),
        "{real_reword_envelope}"
    );
    assert_eq!(
        attempt_recorded_events(&workspace).len(),
        1,
        "still exactly the one fact recorded by the first, material revision"
    );
}

// 71-R3: a successful `handoff create`, under a configured convergence
// policy, appends one bound `gate_failure` fact for each gate failure the
// actor declares, and one bound `repair_attempt` fact when the delivery
// answers a prior review return, inheriting that return's declared reason.
// `GateEvidenceStale` and every other refusal path in `handoff create` is
// untouched: a refusal commits no transaction and records nothing.

/// Activates a card naming exactly `feature_gates`, drives one commit through
/// all of them, and returns the candidate's head SHA — the shared setup every
/// 71-R3 handoff fixture below needs before it can write its own declaration.
fn ready_candidate(workspace: &Workspace, card_id: &str, feature_gates: &[&str]) -> String {
    workspace.activate_card_with_gates(card_id, &["src/**"], feature_gates);
    workspace.work(&["start", "--card-id", card_id]);

    let worktree = workspace.worktrees.join(card_id);
    fs::create_dir_all(worktree.join("src")).unwrap();
    fs::write(worktree.join("src/a.rs"), "fn main() {}\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: implement"]);
    for gate_id in feature_gates {
        workspace.gate(&["run", "--card-id", card_id, "--gate-id", gate_id]);
    }
    support::capture(&worktree, &["rev-parse", "HEAD"])
}

/// Writes a handoff declaration for `card_id` at `head`, with `gate_failures_yaml`
/// appended verbatim (already-formatted YAML, or an empty string to declare
/// none at all), and returns its path.
fn declaration_with_gate_failures(
    workspace: &Workspace,
    card_id: &str,
    head: &str,
    gate_failures_yaml: &str,
) -> String {
    let path = workspace.root.join(format!("{card_id}-declaration.yaml"));
    fs::write(
        &path,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: it works\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n{gate_failures_yaml}"
        ),
    )
    .unwrap();
    path.display().to_string()
}

/// Every recorded attempt fact of one `attempt_kind`.
fn attempt_facts_of_kind(workspace: &Workspace, kind: &str) -> Vec<serde_json::Value> {
    attempt_recorded_events(workspace)
        .into_iter()
        .filter(|fact| fact["metadata"]["attempt_kind"] == kind)
        .collect()
}

/// Asserts `policy_digest` looks like a real digest, never a placeholder.
fn assert_real_policy_digest(fact: &serde_json::Value) {
    let policy_digest = fact["metadata"]["policy_digest"]
        .as_str()
        .expect("policy_digest is a string");
    assert!(
        policy_digest.starts_with("sha256:") && policy_digest.len() == "sha256:".len() + 64,
        "policy_digest must be a real digest, not a placeholder: {policy_digest}"
    );
}

#[test]
fn a_delivery_declaring_a_gate_failure_records_one_bound_gate_failure_fact() {
    let workspace = opened_with_policy(3, 3);
    let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
    let declaration = declaration_with_gate_failures(
        &workspace,
        "F-001",
        &head,
        "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
    );

    let output = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration,
    ]);
    assert!(
        output.status.success(),
        "a declared, admissible gate failure must be accepted: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let handoff_id = envelope["data"]["handoff"]["handoff_id"]
        .as_str()
        .expect("the created handoff's id");
    let card_status = workspace.card_json(&["status", "--card-id", "F-001"]);

    let attempts = attempt_recorded_events(&workspace);
    assert_eq!(
        attempts.len(),
        1,
        "exactly one attempt fact must be recorded: {attempts:?}"
    );
    let fact = &attempts[0];
    assert_eq!(fact["card_id"], "F-001", "{fact}");
    assert_eq!(
        fact["card_revision"], card_status["data"]["revision"],
        "{fact}"
    );
    assert_eq!(fact["card_digest"], card_status["data"]["digest"], "{fact}");
    assert_eq!(fact["cycle_id"], "C-001", "{fact}");
    assert_eq!(fact["head_sha"], head, "{fact}");
    assert_eq!(fact["metadata"]["attempt_kind"], "gate_failure", "{fact}");
    assert_eq!(fact["metadata"]["reason_category"], "regression", "{fact}");
    assert_eq!(
        fact["metadata"]["evidence_ref"],
        format!("handoff:{handoff_id}#gate:gate.unit"),
        "{fact}"
    );
    assert_real_policy_digest(fact);
}

#[test]
fn two_declared_gate_failures_record_two_facts_in_declaration_order() {
    let workspace = opened_with_policy(3, 3);
    // A second feature gate distinct from `gate.all`, which is already this
    // fixture's integration gate — Section 10.3 refuses a card that declares
    // one gate in two validation stages.
    workspace.register_gate("gate.extra", &["true"]);
    let head = ready_candidate(&workspace, "F-001", &["gate.unit", "gate.extra"]);
    let declaration = declaration_with_gate_failures(
        &workspace,
        "F-001",
        &head,
        "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n  - gate_id: gate.extra\n    reason_category: security_concern\n",
    );

    let output = workspace.handoff(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration,
    ]);
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let handoff_id = envelope["data"]["handoff"]["handoff_id"]
        .as_str()
        .expect("the created handoff's id");

    let attempts = attempt_recorded_events(&workspace);
    assert_eq!(
        attempts.len(),
        2,
        "both declared failures must be recorded: {attempts:?}"
    );
    assert_eq!(attempts[0]["metadata"]["attempt_kind"], "gate_failure");
    assert_eq!(attempts[0]["metadata"]["reason_category"], "regression");
    assert_eq!(
        attempts[0]["metadata"]["evidence_ref"],
        format!("handoff:{handoff_id}#gate:gate.unit"),
        "the first declared gate must be the first fact: {:?}",
        attempts[0]
    );
    assert_eq!(attempts[1]["metadata"]["attempt_kind"], "gate_failure");
    assert_eq!(
        attempts[1]["metadata"]["reason_category"],
        "security_concern"
    );
    assert_eq!(
        attempts[1]["metadata"]["evidence_ref"],
        format!("handoff:{handoff_id}#gate:gate.extra"),
        "the second declared gate must be the second fact: {:?}",
        attempts[1]
    );
}

#[test]
fn a_gate_failure_naming_an_unregistered_gate_is_refused_before_anything_is_written() {
    let workspace = opened_with_policy(3, 3);
    // Feature gates are `[gate.unit]`; the declaration names a gate this card
    // never registered for its feature stage at all.
    let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
    let declaration = declaration_with_gate_failures(
        &workspace,
        "F-001",
        &head,
        "gate_failures:\n  - gate_id: gate.nonexistent\n    reason_category: regression\n",
    );

    let before_head = workspace.control_head();
    let output = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration,
    ]);

    assert!(
        !output.status.success(),
        "a gate the card never named must be refused"
    );
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-HANDOFF");
    assert_eq!(
        workspace.control_head(),
        before_head,
        "the control repository head must not move on refusal"
    );
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "a refused handoff must not record an attempt fact"
    );
    let inspected = workspace.handoff_raw(&["inspect", "--card-id", "F-001"]);
    assert!(
        !inspected.status.success(),
        "no handoff may exist for this card after the refusal"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "active",
        "the card must not have moved to handed_off"
    );
}

#[test]
fn a_gate_failure_reason_the_kind_cannot_admit_is_refused() {
    let workspace = opened_with_policy(3, 3);
    let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
    let declaration = declaration_with_gate_failures(
        &workspace,
        "F-001",
        &head,
        "gate_failures:\n  - gate_id: gate.unit\n    reason_category: acceptance_defect\n",
    );

    let before_head = workspace.control_head();
    let output = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration,
    ]);

    assert!(
        !output.status.success(),
        "acceptance_defect is not a reason a gate failure may declare"
    );
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INCOMPLETE-HANDOFF");
    assert_eq!(workspace.control_head(), before_head);
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "a refused handoff must not record an attempt fact"
    );
}

/// A `changes_requested` verdict declaring `reason_category: regression`.
const RETURN_WITH_REGRESSION_REASON_FOR_HANDOFF: &str = "reviewer_actor_id: reviewer\ndecision: changes_requested\nreason_category: regression\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: something is wrong\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";

/// A `changes_requested` verdict declaring `reason_category:
/// non_blocking_improvement` — admissible for a review return, but not for
/// the repair attempt that answers it.
const RETURN_WITH_NON_BLOCKING_REASON_FOR_HANDOFF: &str = "reviewer_actor_id: reviewer\ndecision: changes_requested\nreason_category: non_blocking_improvement\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: something is wrong\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";

/// Drives a card through one review round that returns it with the given
/// verdict body, resumes work, and delivers a second commit through gate and
/// handoff — the shape every repair-attempt fixture below needs. Returns the
/// second delivery's `handoff create` output and its candidate SHA.
fn redeliver_after_return(
    workspace: &Workspace,
    card_id: &str,
    verdict_body: &str,
) -> (std::process::Output, String) {
    open_review_round(workspace, card_id);
    let verdict_path = write_verdict(workspace, card_id, verdict_body);
    workspace.review(&[
        "record",
        "--card-id",
        card_id,
        "--verdict",
        &verdict_path,
        "--actor",
        "reviewer",
    ]);
    workspace.work(&["resume", "--card-id", card_id]);

    let worktree = workspace.worktrees.join(card_id);
    fs::write(worktree.join("src/a.rs"), "fn main() { /* fixed */ }\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "fix: address review"]);
    workspace.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = declaration_with_gate_failures(workspace, card_id, &head, "");
    let output = workspace.handoff_raw(&[
        "create",
        "--card-id",
        card_id,
        "--declaration",
        &declaration,
    ]);
    (output, head)
}

#[test]
fn a_delivery_answering_a_review_return_records_one_repair_attempt_inheriting_its_reason() {
    let workspace = opened_with_policy(3, 3);
    let (output, head) = redeliver_after_return(
        &workspace,
        "F-001",
        RETURN_WITH_REGRESSION_REASON_FOR_HANDOFF,
    );
    assert!(
        output.status.success(),
        "the redelivery must succeed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let handoff_id = envelope["data"]["handoff"]["handoff_id"]
        .as_str()
        .expect("the created handoff's id");
    let card_status = workspace.card_json(&["status", "--card-id", "F-001"]);

    let repairs = attempt_facts_of_kind(&workspace, "repair_attempt");
    assert_eq!(
        repairs.len(),
        1,
        "exactly one repair attempt must be recorded: {repairs:?}"
    );
    let fact = &repairs[0];
    assert_eq!(fact["card_id"], "F-001", "{fact}");
    assert_eq!(
        fact["card_revision"], card_status["data"]["revision"],
        "{fact}"
    );
    assert_eq!(fact["card_digest"], card_status["data"]["digest"], "{fact}");
    assert_eq!(fact["cycle_id"], "C-001", "{fact}");
    assert_eq!(fact["head_sha"], head, "{fact}");
    assert_eq!(
        fact["metadata"]["reason_category"], "regression",
        "the repair attempt must inherit the review return's declared reason: {fact}"
    );
    assert_eq!(
        fact["metadata"]["evidence_ref"],
        format!("handoff:{handoff_id}"),
        "{fact}"
    );
    assert_real_policy_digest(fact);

    // And the review return itself is still on record, distinct from the
    // repair attempt it caused.
    let returns = attempt_facts_of_kind(&workspace, "review_return");
    assert_eq!(returns.len(), 1, "{returns:?}");
    assert_eq!(returns[0]["metadata"]["reason_category"], "regression");
}

#[test]
fn a_repair_attempt_is_not_recorded_when_the_return_it_answers_was_non_blocking() {
    let workspace = opened_with_policy(3, 3);
    let (output, _head) = redeliver_after_return(
        &workspace,
        "F-001",
        RETURN_WITH_NON_BLOCKING_REASON_FOR_HANDOFF,
    );
    assert!(
        output.status.success(),
        "the redelivery must still succeed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        attempt_facts_of_kind(&workspace, "repair_attempt").is_empty(),
        "polishing on request is not the convergence failure this budget exists to detect"
    );
}

#[test]
fn a_first_delivery_records_no_repair_attempt() {
    let workspace = opened_with_policy(3, 3);
    let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
    let declaration = declaration_with_gate_failures(&workspace, "F-001", &head, "");

    let output = workspace.handoff(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration,
    ]);
    assert!(output.status.success());
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "a card that was never returned cannot be repairing anything, and nothing else was declared either"
    );
}

#[test]
fn a_project_without_a_convergence_policy_records_no_facts_at_handoff() {
    // No `configure_convergence_policy` call: an unconfigured project must
    // accept and ignore `gate_failures` entirely — not validate it against
    // `admits`, and not record anything — so a gate_id the card never named
    // and a reason `GateFailure` could never admit must still succeed.
    let workspace = opened();
    let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
    let declaration = declaration_with_gate_failures(
        &workspace,
        "F-001",
        &head,
        "gate_failures:\n  - gate_id: gate.never-named\n    reason_category: acceptance_defect\n",
    );

    let output = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration,
    ]);
    assert!(
        output.status.success(),
        "an unconfigured project must not validate declared gate failures, however they read: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "no configured policy means no attempt fact, however the declaration reads"
    );
}

// 71-R4: an `integration merge` that fails with `ConflictMergeFailed` records
// a cycle-bound `integration_failure` fact before it refuses.

/// Prepares every ready card and returns the integration identifier.
fn prepare_integration(workspace: &Workspace) -> String {
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

/// An approved card that conflicts with the protected branch.
///
/// The same fixture shape as `merge_preflight.rs`'s `conflicting()`: `WP-230`
/// refuses two active cards claiming the same path, so two cards can never be
/// made to conflict with each other. A conflict reaches `integration merge`
/// by the other route instead — the protected branch moving under an already
/// approved candidate — which is exactly what `expected_main_sha` exists to
/// pin. Kept as its own copy here rather than shared with that file: each
/// file under `tests/` is its own binary, and `merge_preflight.rs`'s version
/// is a private fn, not part of `support`.
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

/// Like [`conflicting`], but with a convergence policy configured first — see
/// [`opened_with_policy`] for why the policy has to precede `cycle create`.
fn conflicting_under_policy(card_limit: u32, integration_limit: u32) -> Workspace {
    let workspace = Workspace::initialized();
    workspace.configure_convergence_policy(card_limit, integration_limit);

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

/// `count` independent approved cards, each touching its own directory, ready
/// to integrate cleanly, under a configured convergence policy.
fn integrable_under_policy(card_limit: u32, integration_limit: u32, count: usize) -> Workspace {
    let workspace = opened_with_policy(card_limit, integration_limit);
    for index in 1..=count {
        let card = format!("F-{index:03}");
        workspace.activate_card(&card, &[&format!("src/{card}/**")]);
        workspace.approve_card(&card, &format!("src/{card}/a.rs"));
    }
    workspace
}

#[test]
fn a_conflicting_integration_records_one_cycle_bound_failure_fact_and_still_refuses() {
    let workspace = conflicting_under_policy(3, 3);
    let id = prepare_integration(&workspace);
    let authority_head = workspace.authority_head();

    let output = workspace.integration_raw(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert!(
        !output.status.success(),
        "a conflicting merge must still be refused"
    );
    assert_eq!(output.status.code(), Some(6));
    assert_eq!(error_code(&output), "CH-CONFLICT-MERGE-FAILED");

    let attempts = attempt_recorded_events(&workspace);
    assert_eq!(
        attempts.len(),
        1,
        "exactly one attempt fact must be recorded: {attempts:?}"
    );
    let fact = &attempts[0];
    assert_eq!(fact["cycle_id"], "C-001", "{fact}");
    assert!(
        fact["card_id"].is_null(),
        "an integration failure names no card: {fact}"
    );
    assert!(
        fact["card_revision"].is_null(),
        "an integration failure names no card revision: {fact}"
    );
    assert!(
        fact["card_digest"].is_null(),
        "an integration failure names no card digest: {fact}"
    );
    assert_eq!(
        fact["metadata"]["attempt_kind"], "integration_failure",
        "{fact}"
    );
    assert_eq!(
        fact["metadata"]["reason_category"], "integration_conflict",
        "{fact}"
    );
    assert_eq!(
        fact["metadata"]["evidence_ref"],
        format!("integration:{id}"),
        "{fact}"
    );
    let head_sha = fact["head_sha"].as_str().expect("head_sha is a string");
    assert_eq!(
        head_sha.len(),
        40,
        "head_sha must be an exact commit SHA: {fact}"
    );
    assert!(
        head_sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "head_sha must be hex: {fact}"
    );
    assert_eq!(
        head_sha, authority_head,
        "head must name the exact commit the merge was attempted against: {fact}"
    );
    assert_real_policy_digest(fact);
}

#[test]
fn two_conflicting_attempts_record_two_facts() {
    let workspace = conflicting_under_policy(3, 3);
    let id = prepare_integration(&workspace);

    for _ in 0..2 {
        let output = workspace.integration_raw(&[
            "merge",
            "--integration-id",
            &id,
            "--actor-id",
            "coordinator",
        ]);
        assert_eq!(error_code(&output), "CH-CONFLICT-MERGE-FAILED");
    }

    let attempts = attempt_recorded_events(&workspace);
    assert_eq!(
        attempts.len(),
        2,
        "the counter must rise to two: {attempts:?}"
    );
    // R1 already documents that the counter is authoritative and evidence is
    // a set of distinct references: repeated failures of the same
    // integration share one `evidence_ref`.
    let expected_ref = format!("integration:{id}");
    for fact in &attempts {
        assert_eq!(
            fact["metadata"]["evidence_ref"], expected_ref,
            "repeated failures of the same integration share one evidence_ref: {fact}"
        );
    }
}

#[test]
fn a_successful_integration_records_no_failure_fact() {
    let workspace = integrable_under_policy(3, 3, 2);
    let id = prepare_integration(&workspace);

    let output = workspace.integration_raw(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert!(
        output.status.success(),
        "the fixture must merge cleanly: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "a successful merge must not record an integration_failure fact"
    );
}

#[test]
fn a_refusal_that_is_not_a_conflict_records_no_failure_fact() {
    let workspace = opened_with_policy(3, 3);

    // No integration named `INT-999` was ever prepared: a precondition
    // failure, not a conflict.
    let output = workspace.integration_raw(&[
        "merge",
        "--integration-id",
        "INT-999",
        "--actor-id",
        "coordinator",
    ]);
    assert!(!output.status.success());
    assert_eq!(error_code(&output), "CH-PRECONDITION-NOT-FOUND");
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "a refusal that is not a conflict must not record an integration_failure fact"
    );
}

#[test]
fn a_project_without_a_convergence_policy_records_no_fact_when_integration_conflicts() {
    // No `configure_convergence_policy` call, matching
    // `a_project_without_a_convergence_policy_records_no_attempt_fact`
    // elsewhere in this file: the same rejection as today, cero facts.
    let workspace = conflicting();
    let id = prepare_integration(&workspace);
    let control_before = workspace.control_head();

    let output = workspace.integration_raw(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(error_code(&output), "CH-CONFLICT-MERGE-FAILED");
    assert_eq!(
        workspace.control_head(),
        control_before,
        "no configured policy means no additional control write; a refused merge records nothing"
    );
    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "no configured policy means no attempt fact, however the merge failed"
    );
}

#[test]
fn the_recorded_failure_projects_into_the_cycle_counter() {
    let workspace = conflicting_under_policy(3, 3);
    let id = prepare_integration(&workspace);
    workspace.integration_raw(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(
        attempt_recorded_events(&workspace).len(),
        1,
        "the fixture must produce exactly one attempt fact before projecting it"
    );

    // The real proof that the fact is usable by #73 and not merely written:
    // round-trip every event through the actual projection.
    let events: Vec<Event> = workspace
        .events()
        .into_iter()
        .map(|value| serde_json::from_value(value).expect("a well-formed event"))
        .collect();
    let control = ControlRepository::open(&workspace.control).expect("control repository opens");
    let config = control.project().expect("the project document reads");
    let project_id: ProjectId = "example".parse().unwrap();
    let cycle_id: CycleId = "C-001".parse().unwrap();

    let projection = project(
        config.convergence_policy.as_ref(),
        &project_id,
        &cycle_id,
        &events,
    )
    .expect("a well-formed projection");

    let ProjectConvergence::Configured(view) = projection else {
        panic!("a configured policy must project as configured");
    };
    assert_eq!(view.cycle.integration_failures.count, 1);
    assert_eq!(
        view.cycle.integration_failures.evidence,
        BTreeSet::from([format!("integration:{id}")]),
        "the projection must retain the evidence #73 will read"
    );
}

// 73-1: `cycle status` gains a read-only `convergence` report, mirroring
// `card status`'s own (72-3). Neither test below asserts a refusal — that
// is a later card's job — only that the report is present and accurate.

#[test]
fn cycle_status_reports_a_cycle_within_its_budget() {
    let workspace = opened_with_policy(3, 3);

    let envelope = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(
        envelope["data"]["convergence"],
        serde_json::json!({ "status": "within" }),
        "{envelope}"
    );
}

#[test]
fn cycle_status_reports_an_escalated_cycle_with_its_evidence() {
    // A limit of one integration failure, spent by exactly one real
    // conflict driven through the governed `integration merge` command —
    // the same fixture
    // `a_conflicting_integration_records_one_cycle_bound_failure_fact_and_still_refuses`
    // uses above, just with the cycle budget tight enough that this one
    // failure alone exhausts it.
    let workspace = conflicting_under_policy(3, 1);
    let id = prepare_integration(&workspace);

    let output = workspace.integration_raw(&[
        "merge",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(
        error_code(&output),
        "CH-CONFLICT-MERGE-FAILED",
        "the fixture must fail on the conflict it was built to produce"
    );

    let envelope = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(
        envelope["data"]["convergence"],
        serde_json::json!({
            "status": "escalated",
            "exhausted": [
                {
                    "dimension": "integration_failures",
                    "count": 1,
                    "limit": 1,
                    "evidence": [format!("integration:{id}")],
                }
            ],
            "next_permitted_action": "record_authorized_disposition",
        }),
        "data.convergence must be exactly CycleConvergence's own serialization: {envelope}"
    );

    // The human-readable text must say the same thing: which dimension is
    // exhausted and what may happen next, without requiring JSON — the same
    // register `card status`'s own escalation report uses.
    let text_output = Workspace::run(&[
        "cycle".to_owned(),
        "status".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--cycle-id".to_owned(),
        "C-001".to_owned(),
    ]);
    assert!(
        text_output.status.success(),
        "{}",
        String::from_utf8_lossy(&text_output.stderr)
    );
    let text = String::from_utf8_lossy(&text_output.stdout).into_owned();
    assert!(text.contains("integration_failures"), "{text}");
    assert!(text.contains("1/1"), "{text}");
    assert!(text.contains(&format!("integration:{id}")), "{text}");
    assert!(
        text.contains("record_authorized_disposition"),
        "the text must name the next permitted action too: {text}"
    );
}

// 72-2: once `assess_card` reports a card `Escalated`, that card's own
// delivery and review loop stops. `handoff create`, `review begin`, and
// `review record` — the real commands, and the previews for the two whose
// contract calls for preview parity — all refuse before writing anything,
// naming every exhausted dimension with its count, limit, and evidence.
// Other cards in the same cycle are unaffected. Every test below reaches
// escalation the same way: a limit-1 `review_returns` policy and one
// declared return — the shortest path that exists today, using only real
// commands.

/// The human-readable message from a failed command's JSON envelope.
fn error_message(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["message"]
        .as_str()
        .expect("a coded refusal carries a message")
        .to_owned()
}

/// Every long option a remedy string spells, in the order it spells them,
/// whether or not the text wraps it in backticks.
///
/// Scanned out of the text rather than compared against a hard-coded copy of
/// the invocation, so a cross-check measures the remedy against the command's
/// real `--help` surface rather than a second hand-written claim that would
/// rot in lockstep with the first and prove nothing.
///
/// At file scope rather than inside `disposition_renew` because two refusals
/// now make the same promise and are pinned the same way: the escalation
/// remedy (`the_escalation_refusal_names_the_command_that_resolves_it`) and
/// the orphaned-facts refusal
/// (`set_convergence_policy_refuses_to_change_a_policy_once_facts_exist`).
/// Both texts carried an issue number where a command belonged.
fn flags_named_by(remedy: &str) -> Vec<String> {
    remedy
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'))
        .filter(|word| word.starts_with("--"))
        .map(str::to_owned)
        .collect()
}

/// Exhausts a card's `review_returns` budget with one declared return, under
/// a limit-1 policy — the shortest real path to `Escalated`. Leaves the card
/// `changes_requested`, still holding its work lease. Returns the review's
/// id, so a caller can assert the refusal names its evidence reference.
fn escalate_via_review_returns(workspace: &Workspace, card_id: &str) -> String {
    open_review_round(workspace, card_id);
    let path = write_verdict(workspace, card_id, RETURN_WITH_ACCEPTANCE_DEFECT_REASON);
    let output = workspace.review(&[
        "record",
        "--card-id",
        card_id,
        "--verdict",
        &path,
        "--actor",
        "reviewer",
    ]);
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    envelope["data"]["review"]["review_id"]
        .as_str()
        .expect("the recorded review's id")
        .to_owned()
}

/// Resumes work on an escalated card and delivers one more commit through a
/// green gate, without attempting `handoff create` — the shared setup for
/// every fixture below that needs a ready candidate to attempt delivery
/// against.
fn redeliver_candidate(workspace: &Workspace, card_id: &str) -> String {
    workspace.work(&["resume", "--card-id", card_id]);
    let worktree = workspace.worktrees.join(card_id);
    fs::write(worktree.join("src/a.rs"), "fn main() { /* fixed */ }\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "fix: address review"]);
    workspace.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);
    support::capture(&worktree, &["rev-parse", "HEAD"])
}

#[test]
fn an_escalated_card_refuses_a_new_handoff_before_writing_anything() {
    let workspace = opened_with_policy(1, 3);
    let review_id = escalate_via_review_returns(&workspace, "F-001");
    let head = redeliver_candidate(&workspace, "F-001");
    let declaration = declaration_with_gate_failures(&workspace, "F-001", &head, "");

    let before_head = workspace.control_head();
    let output = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration,
    ]);

    assert!(
        !output.status.success(),
        "an escalated card must refuse a new handoff: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-CONVERGENCE-ESCALATED");
    let message = error_message(&output);
    assert!(message.contains("review_returns"), "{message}");
    assert!(message.contains("1/1"), "{message}");
    assert!(
        message.contains(&format!("review:{review_id}")),
        "the operator must see the evidence without opening the control repository: {message}"
    );
    assert_eq!(
        workspace.control_head(),
        before_head,
        "the control repository head must not move on refusal"
    );
    assert!(
        workspace
            .handoff_raw(&["inspect", "--card-id", "F-001"])
            .status
            .success(),
        "the card's earlier handoff must be unaffected"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "active",
        "`work resume` already moved the card to active; the refused handoff must not move it to handed_off"
    );
}

#[test]
fn an_escalated_card_refuses_review_begin() {
    let workspace = opened_with_policy(1, 3);
    let review_id = escalate_via_review_returns(&workspace, "F-001");

    let before_head = workspace.control_head();
    let output = workspace.review_raw(&["begin", "--card-id", "F-001", "--actor", "reviewer"]);

    assert!(
        !output.status.success(),
        "an escalated card must refuse a new review round: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-CONVERGENCE-ESCALATED");
    let message = error_message(&output);
    assert!(message.contains("review_returns"), "{message}");
    assert!(
        message.contains(&format!("review:{review_id}")),
        "{message}"
    );
    assert_eq!(
        workspace.control_head(),
        before_head,
        "the control repository head must not move on refusal"
    );
}

#[test]
fn an_escalated_card_refuses_review_record() {
    let workspace = opened_with_policy(1, 3);
    let review_id = escalate_via_review_returns(&workspace, "F-001");
    let before_reviews = recorded_review_count(&workspace, "F-001");
    let before_head = workspace.control_head();

    // A fresh verdict against the still-open handoff: proof that a new
    // review cannot dodge the escalation through the ordinary CLI surface.
    let path = write_verdict(
        &workspace,
        "F-001",
        RETURN_WITH_REGRESSION_REASON_FOR_HANDOFF,
    );
    let output = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer",
    ]);

    assert!(
        !output.status.success(),
        "a new review must not escape the escalation: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-CONVERGENCE-ESCALATED");
    assert!(
        error_message(&output).contains(&format!("review:{review_id}")),
        "{}",
        error_message(&output)
    );
    assert_eq!(
        workspace.control_head(),
        before_head,
        "the control repository head must not move on refusal"
    );
    assert_eq!(
        recorded_review_count(&workspace, "F-001"),
        before_reviews,
        "no new review record may exist after a refusal"
    );
}

#[test]
fn an_unrelated_card_in_the_same_cycle_still_delivers_and_is_reviewed() {
    let workspace = opened_with_policy(1, 3);
    escalate_via_review_returns(&workspace, "F-001");

    // A second, unrelated card in the same cycle, scoped away from `src/**`
    // so the two can coexist without an ownership-overlap refusal, must be
    // completely unaffected by F-001's escalation.
    workspace.activate_card("F-002", &["docs/f002/**"]);
    workspace.approve_card("F-002", "docs/f002/a.md");

    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-002"])["data"]["state"],
        "approved",
        "an unrelated card must still deliver and be reviewed"
    );

    // And F-001 remains blocked throughout, proving the isolation runs both
    // ways.
    let output = workspace.review_raw(&["begin", "--card-id", "F-001", "--actor", "reviewer"]);
    assert_eq!(error_code(&output), "CH-POLICY-CONVERGENCE-ESCALATED");
}

#[test]
fn a_card_one_return_below_its_limit_still_delivers() {
    // The escalation boundary from the other side: with a limit of two, one
    // recorded return must not spend the budget.
    let workspace = opened_with_policy(2, 3);
    let (output, _head) = redeliver_after_return(
        &workspace,
        "F-001",
        RETURN_WITH_REGRESSION_REASON_FOR_HANDOFF,
    );
    assert!(
        output.status.success(),
        "one return below a limit of two must not escalate: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "handed_off"
    );
}

#[test]
fn a_project_without_a_convergence_policy_never_refuses_for_convergence() {
    // No `configure_convergence_policy` call: five change-requested rounds is
    // more than any policy configured elsewhere in this file, and none of
    // them may refuse for convergence — an unconfigured project has no
    // budget to spend in the first place. `review_round` already drives a
    // full commit/gate/handoff/review-begin/review-record/work-resume cycle
    // per call (asserting each step's success itself), and honors the
    // "carry every open finding forward" rule a hand-rolled loop would not.
    let workspace = opened();
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    review_round(&workspace, "F-001", 1, &[], &["src/a.rs"]);
    review_round(&workspace, "F-001", 2, &["src/a.rs"], &["src/a.rs"]);
    review_round(&workspace, "F-001", 3, &["src/a.rs"], &["src/a.rs"]);
    review_round(&workspace, "F-001", 4, &["src/a.rs"], &["src/a.rs"]);
    review_round(&workspace, "F-001", 5, &["src/a.rs"], &["src/a.rs"]);

    assert!(
        attempt_recorded_events(&workspace).is_empty(),
        "no configured policy means no facts, however many rounds ran"
    );
}

#[test]
fn the_dry_run_preview_refuses_the_same_escalation() {
    let workspace = opened_with_policy(1, 3);
    let review_id = escalate_via_review_returns(&workspace, "F-001");

    // `handoff create --dry-run`.
    let head = redeliver_candidate(&workspace, "F-001");
    let declaration = declaration_with_gate_failures(&workspace, "F-001", &head, "");
    let before_head = workspace.control_head();
    let handoff_preview = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration,
        "--dry-run",
    ]);
    assert!(
        !handoff_preview.status.success(),
        "the dry run must refuse the same escalation the real command does"
    );
    assert_eq!(
        error_code(&handoff_preview),
        "CH-POLICY-CONVERGENCE-ESCALATED"
    );
    assert!(
        error_message(&handoff_preview).contains(&format!("review:{review_id}")),
        "{}",
        error_message(&handoff_preview)
    );
    assert_eq!(
        workspace.control_head(),
        before_head,
        "a dry run must never write"
    );

    // `review record --dry-run`.
    let verdict_path = write_verdict(
        &workspace,
        "F-001",
        RETURN_WITH_REGRESSION_REASON_FOR_HANDOFF,
    );
    let review_preview = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &verdict_path,
        "--actor",
        "reviewer",
        "--dry-run",
    ]);
    assert!(
        !review_preview.status.success(),
        "the dry run must refuse the same escalation the real command does"
    );
    assert_eq!(
        error_code(&review_preview),
        "CH-POLICY-CONVERGENCE-ESCALATED"
    );
    assert!(
        error_message(&review_preview).contains(&format!("review:{review_id}")),
        "{}",
        error_message(&review_preview)
    );
    assert_eq!(
        workspace.control_head(),
        before_head,
        "a dry run must never write"
    );

    // `review begin --dry-run`.
    let begin_preview = workspace.review_raw(&[
        "begin",
        "--card-id",
        "F-001",
        "--actor",
        "reviewer",
        "--dry-run",
    ]);
    assert!(
        !begin_preview.status.success(),
        "the dry run must refuse the same escalation the real command does"
    );
    assert_eq!(
        error_code(&begin_preview),
        "CH-POLICY-CONVERGENCE-ESCALATED"
    );
    assert!(
        error_message(&begin_preview).contains(&format!("review:{review_id}")),
        "{}",
        error_message(&begin_preview)
    );
    assert_eq!(
        workspace.control_head(),
        before_head,
        "a dry run must never write"
    );
}

#[test]
fn a_corrupt_convergence_fact_fails_closed_instead_of_reading_as_unspent_budget() {
    // A policy generous enough (limit 3) that the one recorded return, left
    // intact, would answer `Within` and let this card proceed. Corrupting
    // that one fact must still refuse the next controlled action — a
    // malformed fact must never be read as an empty, unspent budget, which
    // is exactly the failure `project`'s fail-closed refusal exists to
    // prevent.
    let workspace = opened_with_policy(3, 3);
    open_review_round(&workspace, "F-001");
    let path = write_verdict(&workspace, "F-001", RETURN_WITH_ACCEPTANCE_DEFECT_REASON);
    workspace.review(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer",
    ]);
    redeliver_candidate(&workspace, "F-001");

    let events_dir = workspace.control.join("events");
    let mut corrupted = 0;
    for entry in fs::read_dir(&events_dir).unwrap() {
        let entry = entry.unwrap();
        let entry_path = entry.path();
        if entry_path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read_to_string(&entry_path).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        if value["event_type"] == "convergence.attempt_recorded" {
            value["metadata"]["evidence_ref"] = serde_json::json!("");
            fs::write(
                &entry_path,
                format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
            )
            .unwrap();
            corrupted += 1;
        }
    }
    assert_eq!(
        corrupted, 1,
        "the fixture must have produced exactly one convergence fact to corrupt"
    );

    let before_head = workspace.control_head();
    let declaration = declaration_with_gate_failures(
        &workspace,
        "F-001",
        &support::capture(&workspace.worktrees.join("F-001"), &["rev-parse", "HEAD"]),
        "",
    );
    let output = workspace.handoff_raw(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration,
    ]);

    assert!(
        !output.status.success(),
        "a corrupted convergence fact must fail closed, not read as unused budget: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_ne!(
        error_code(&output),
        "CH-POLICY-CONVERGENCE-ESCALATED",
        "this must fail because the fact is unreadable, not because the (otherwise unspent) budget looks exhausted"
    );
    assert_eq!(
        workspace.control_head(),
        before_head,
        "the control repository head must not move on refusal"
    );
}

// 72-3: once a card is `Escalated`, none of the three remaining routes that
// would advance it stay open either — `work start`, `work resume`, and
// `card revise` all refuse before writing anything, the same way 72-2 closed
// `handoff create` and the two `review` commands. `card revise` is the one
// that matters most: 71-R1 already made the convergence counters span
// revisions, so revising never resets a spent budget, but nothing before this
// card stopped a revision from moving an escalated card back to `ready` and
// so back into `work start`'s reach — the escape route this closes. `card
// status`, `work block`, and `work checkpoint` are the other half of the same
// line: convergence blocks what advances a card, not what parks or looks at
// it, so all three stay open on an escalated card, and `card status` reports
// the escalation as data instead of refusing to answer at all.

#[test]
fn an_escalated_card_refuses_work_start() {
    let workspace = opened_with_policy(1, 3);
    let review_id = escalate_via_review_returns(&workspace, "F-001");
    let before_head = workspace.control_head();

    // `work start --dry-run`: the preview must never promise a start the real
    // command would refuse.
    let preview = workspace.work_raw(&["start", "--card-id", "F-001", "--dry-run"]);
    assert!(
        !preview.status.success(),
        "the dry run must refuse the same escalation the real command does: {}",
        String::from_utf8_lossy(&preview.stdout)
    );
    assert_eq!(error_code(&preview), "CH-POLICY-CONVERGENCE-ESCALATED");
    assert!(
        error_message(&preview).contains(&format!("review:{review_id}")),
        "{}",
        error_message(&preview)
    );

    let output = workspace.work_raw(&["start", "--card-id", "F-001"]);
    assert!(
        !output.status.success(),
        "an escalated card must refuse a new work start: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-CONVERGENCE-ESCALATED");
    let message = error_message(&output);
    assert!(message.contains("review_returns"), "{message}");
    assert!(message.contains("1/1"), "{message}");
    assert!(
        message.contains(&format!("review:{review_id}")),
        "the operator must see the evidence without opening the control repository: {message}"
    );
    assert_eq!(
        workspace.control_head(),
        before_head,
        "neither the dry run nor the refusal may move the control repository head"
    );
}

#[test]
fn an_escalated_card_refuses_work_resume() {
    // `resumes_to_active` admits four source states, and its own doc comment
    // explains why: `changes_requested` and `blocked` are an actor
    // continuing work they already own — resuming from there must stay open,
    // or a card could never accumulate the review returns, repair attempts,
    // or gate failures its own budget exists to count, since redelivering
    // after a return requires becoming `active` again. `ready` is different:
    // it is reached only when a revision leaves a lease stranded (`work
    // start` refuses outright because the lease exists), so resuming from
    // `ready` is functionally the same advance `work start` blocks for a
    // fresh assignment, just through its alternate entry point. That is the
    // reachable, unambiguous case this test drives: a material scope
    // revision that itself spends the budget — the revision is allowed, the
    // budget is unspent when it runs — leaves the card exactly there.
    let workspace = opened_with_policy(1, 3);
    let base = workspace.authority_head();
    workspace.activate_card_with_base("F-001", &["src/a.rs"], &base);
    workspace.work(&["start", "--card-id", "F-001"]);

    let widened = revision_body(
        "Implement F-001",
        "Deliver F-001",
        &base,
        &["src/a.rs", "src/b.rs"],
        &["it works"],
    );
    let revise = revise_raw(&workspace, &widened, "widen scope", false);
    assert!(
        revise.status.success(),
        "the revision that spends the budget is itself allowed: {}{}",
        String::from_utf8_lossy(&revise.stdout),
        String::from_utf8_lossy(&revise.stderr)
    );
    let card_status = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(card_status["data"]["state"], "ready");
    assert_eq!(
        card_status["data"]["convergence"],
        serde_json::json!({
            "status": "escalated",
            "exhausted": [
                {
                    "dimension": "material_scope_revisions",
                    "count": 1,
                    "limit": 1,
                    "evidence": ["card-revision:F-001@2"],
                }
            ],
            "next_permitted_action": "record_authorized_disposition",
        }),
        "the revision must leave the card escalated: {card_status}"
    );

    let before_head = workspace.control_head();

    // `work resume --dry-run`.
    let preview = workspace.work_raw(&["resume", "--card-id", "F-001", "--dry-run"]);
    assert!(
        !preview.status.success(),
        "the dry run must refuse the same escalation the real command does: {}",
        String::from_utf8_lossy(&preview.stdout)
    );
    assert_eq!(error_code(&preview), "CH-POLICY-CONVERGENCE-ESCALATED");
    assert!(
        error_message(&preview).contains("material_scope_revisions"),
        "{}",
        error_message(&preview)
    );

    let output = workspace.work_raw(&["resume", "--card-id", "F-001"]);
    assert!(
        !output.status.success(),
        "an escalated card must refuse taking a lease-stranding revision back up: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-CONVERGENCE-ESCALATED");
    let message = error_message(&output);
    assert!(message.contains("material_scope_revisions"), "{message}");
    assert!(message.contains("1/1"), "{message}");
    assert!(
        message.contains("card-revision:F-001@2"),
        "the operator must see the evidence without opening the control repository: {message}"
    );
    assert_eq!(
        workspace.control_head(),
        before_head,
        "neither the dry run nor the refusal may move the control repository head"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "ready",
        "the refused resume must not have moved the card"
    );
}

#[test]
fn an_escalated_card_refuses_a_scope_revision() {
    // This is the escape route: a revision returns the card to `ready`
    // (Section 7.3.4), and from `ready` `work start` looks legal again. 71-R1
    // already keeps the convergence counters from resetting across revisions,
    // so the budget itself cannot be dodged this way — but without this
    // check, the *state* could still be walked back out of its stuck place.
    let workspace = opened_with_policy(1, 3);
    let base = workspace.authority_head();
    let review_id = escalate_via_review_returns(&workspace, "F-001");
    let before_head = workspace.control_head();
    let before_revision =
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["revision"].clone();

    let widened = revision_body(
        "Implement F-001",
        "Deliver F-001",
        &base,
        &["src/**", "docs/**"],
        &["it works"],
    );

    // `card revise --dry-run`.
    let preview = revise_raw(&workspace, &widened, "widen scope", true);
    assert!(
        !preview.status.success(),
        "the dry run must refuse the same escalation the real command does: {}",
        String::from_utf8_lossy(&preview.stdout)
    );
    assert_eq!(error_code(&preview), "CH-POLICY-CONVERGENCE-ESCALATED");
    assert!(
        error_message(&preview).contains(&format!("review:{review_id}")),
        "{}",
        error_message(&preview)
    );

    let output = revise_raw(&workspace, &widened, "widen scope", false);
    assert!(
        !output.status.success(),
        "an escalated card must refuse a scope revision: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-CONVERGENCE-ESCALATED");
    let message = error_message(&output);
    assert!(message.contains("review_returns"), "{message}");
    assert!(message.contains("1/1"), "{message}");
    assert!(
        message.contains(&format!("review:{review_id}")),
        "the operator must see the evidence without opening the control repository: {message}"
    );
    assert_eq!(
        workspace.control_head(),
        before_head,
        "neither the dry run nor the refusal may move the control repository head"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["revision"],
        before_revision,
        "no new revision may exist after the refusal — the card stays exactly where the escalation left it"
    );
}

#[test]
fn card_status_reports_the_escalation_instead_of_refusing() {
    let workspace = opened_with_policy(1, 3);
    let review_id = escalate_via_review_returns(&workspace, "F-001");

    let output = workspace.card_raw(&["status", "--card-id", "F-001"]);
    assert!(
        output.status.success(),
        "card status must never refuse, even for an escalated card: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["convergence"],
        serde_json::json!({
            "status": "escalated",
            "exhausted": [
                {
                    "dimension": "review_returns",
                    "count": 1,
                    "limit": 1,
                    "evidence": [format!("review:{review_id}")],
                }
            ],
            "next_permitted_action": "record_authorized_disposition",
        }),
        "data.convergence must be exactly CardConvergence's own serialization: {envelope}"
    );

    // The human-readable text must say the same thing: which dimension is
    // exhausted and what may happen next, without requiring JSON.
    let text_output = Workspace::run(&[
        "card".to_owned(),
        "status".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--card-id".to_owned(),
        "F-001".to_owned(),
    ]);
    assert!(
        text_output.status.success(),
        "{}",
        String::from_utf8_lossy(&text_output.stderr)
    );
    let text = String::from_utf8_lossy(&text_output.stdout).into_owned();
    assert!(text.contains("review_returns"), "{text}");
    assert!(text.contains("1/1"), "{text}");
    assert!(text.contains(&format!("review:{review_id}")), "{text}");
    assert!(
        text.contains("record_authorized_disposition"),
        "the text must name the next permitted action too: {text}"
    );
}

#[test]
fn card_status_reports_within_budget_for_a_healthy_card() {
    let workspace = opened_with_policy(3, 3);
    workspace.activate_card("F-001", &["src/**"]);

    let envelope = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(
        envelope["data"]["convergence"],
        serde_json::json!({ "status": "within" }),
        "{envelope}"
    );
}

#[test]
fn card_status_reports_legacy_unassessed_without_a_policy() {
    let workspace = opened();
    workspace.activate_card("F-001", &["src/**"]);

    let envelope = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(
        envelope["data"]["convergence"],
        serde_json::json!({ "status": "legacy_unassessed" }),
        "{envelope}"
    );
}

#[test]
fn an_unrelated_card_in_the_same_cycle_still_starts_work() {
    let workspace = opened_with_policy(1, 3);
    escalate_via_review_returns(&workspace, "F-001");

    // A second, unrelated card, scoped away from `src/**` so the two can
    // coexist without an ownership-overlap refusal, must be completely
    // unaffected by F-001's escalation.
    workspace.activate_card("F-002", &["docs/f002/**"]);
    let output = workspace.work_raw(&["start", "--card-id", "F-002"]);
    assert!(
        output.status.success(),
        "an unrelated card in the same cycle must still be able to start work: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-002"])["data"]["state"],
        "active"
    );

    // And F-001 remains blocked throughout, proving the isolation runs both
    // ways.
    let still_blocked = workspace.work_raw(&["start", "--card-id", "F-001"]);
    assert_eq!(
        error_code(&still_blocked),
        "CH-POLICY-CONVERGENCE-ESCALATED"
    );
}

#[test]
fn blocking_and_checkpointing_an_escalated_card_are_still_permitted() {
    let workspace = opened_with_policy(1, 3);
    escalate_via_review_returns(&workspace, "F-001");

    // `escalate_via_review_returns` leaves the card `changes_requested`, and
    // `CardState::successors` only admits `blocked` from `active` — so the
    // actor first takes the returned work back up. That resume is not itself
    // blocked (see the note on `resumes_to_active` in `work.rs`): an actor
    // must be able to continue work they already own even once escalated,
    // since #72-2 already refuses the delivery and review attempts
    // themselves.
    let resume = workspace.work_raw(&["resume", "--card-id", "F-001"]);
    assert!(
        resume.status.success(),
        "resuming already-owned work must stay permitted on an escalated card: {}{}",
        String::from_utf8_lossy(&resume.stdout),
        String::from_utf8_lossy(&resume.stderr)
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "active"
    );

    // From `active`, blocking is a legitimate exit on its own — halting is
    // not advancing.
    let block = workspace.work_raw(&[
        "block",
        "--card-id",
        "F-001",
        "--reason",
        "escalated; awaiting an authorized disposition",
    ]);
    assert!(
        block.status.success(),
        "blocking must stay permitted on an escalated card: {}{}",
        String::from_utf8_lossy(&block.stdout),
        String::from_utf8_lossy(&block.stderr)
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "blocked"
    );

    // A progress note does not advance the card either, and it is the record
    // of why the card is where it is — refusing it would destroy exactly
    // what an operator needs most on a stuck card.
    let checkpoint = workspace.work_raw(&[
        "checkpoint",
        "--card-id",
        "F-001",
        "--note",
        "waiting on a disposition",
    ]);
    assert!(
        checkpoint.status.success(),
        "a progress note must stay permitted on an escalated card: {}{}",
        String::from_utf8_lossy(&checkpoint.stdout),
        String::from_utf8_lossy(&checkpoint.stderr)
    );
}

// Installing a convergence policy at `project init`.
//
// Every fixture above this line installs its policy through
// `Workspace::configure_convergence_policy`, a file-surgery backdoor that
// predates any supported way to turn budgets on. These tests exercise the
// first supported route instead: `project init --convergence-policy <path>`.

/// A valid, minimal convergence policy document: `card_limit` across all
/// four counted dimensions at all four risk levels, `integration_limit` for
/// the one cycle-level dimension. Mirrors the shape
/// `Workspace::configure_convergence_policy` writes directly into
/// `project.json`, except this one is meant to live in its own file, since
/// `--convergence-policy` takes a path and never inline JSON.
fn convergence_policy_document(card_limit: u32, integration_limit: u32) -> serde_json::Value {
    let card_limits = serde_json::json!({
        "review_returns": card_limit,
        "repair_attempts": card_limit,
        "gate_failures": card_limit,
        "material_scope_revisions": card_limit,
    });
    serde_json::json!({
        "version": "harness.convergence-policy/v1",
        "card_limits": {
            "low": card_limits.clone(),
            "medium": card_limits.clone(),
            "high": card_limits.clone(),
            "critical": card_limits,
        },
        "cycle_limits": { "integration_failures": integration_limit },
    })
}

/// Writes a JSON document under the workspace root, returning its path as a
/// CLI-ready string.
fn write_json(workspace: &Workspace, name: &str, document: &serde_json::Value) -> String {
    let path = workspace.root.join(name);
    fs::write(&path, serde_json::to_string_pretty(document).unwrap()).unwrap();
    path.display().to_string()
}

/// The `project init` argv this section exercises, optionally naming a
/// convergence policy file. Deliberately independent of
/// `Workspace::initialized`, which never passes `--convergence-policy`.
fn convergence_init_args(workspace: &Workspace, policy_path: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "project".to_owned(),
        "init".to_owned(),
        "--project-id".to_owned(),
        "example".to_owned(),
        "--repository".to_owned(),
        workspace.repository.display().to_string(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--authority".to_owned(),
        workspace.authority.display().to_string(),
        "--worktree-root".to_owned(),
        workspace.worktrees.display().to_string(),
        "--output".to_owned(),
        "json".to_owned(),
    ];
    if let Some(path) = policy_path {
        args.push("--convergence-policy".to_owned());
        args.push(path.to_owned());
    }
    args
}

/// The project document `project init` just wrote, parsed as JSON.
fn stored_project_document(workspace: &Workspace) -> serde_json::Value {
    let raw = fs::read_to_string(workspace.control.join("project/project.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn project_init_installs_a_declared_convergence_policy() {
    let workspace = Workspace::new();
    let policy = convergence_policy_document(3, 3);
    let policy_path = write_json(&workspace, "policy.json", &policy);

    let output = Workspace::run(&convergence_init_args(&workspace, Some(&policy_path)));
    assert!(
        output.status.success(),
        "project init --convergence-policy failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(
        stored_project_document(&workspace)["convergence_policy"],
        policy,
        "the installed policy must reach the project document unchanged"
    );

    // The installation is worth nothing unless a cycle created afterward
    // actually operates under it.
    workspace.register_gate("gate.unit", &["true"]);
    workspace.register_gate("gate.all", &["true"]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);

    let envelope = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(
        envelope["data"]["convergence"],
        serde_json::json!({ "status": "within" }),
        "a card in a cycle created after installation must see the policy in effect: {envelope}"
    );
}

#[test]
fn project_init_without_the_flag_leaves_no_policy() {
    let workspace = Workspace::new();
    let output = Workspace::run(&convergence_init_args(&workspace, None));
    assert!(
        output.status.success(),
        "project init failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let document = stored_project_document(&workspace);
    assert!(
        !document
            .as_object()
            .expect("project.json is a JSON object")
            .contains_key("convergence_policy"),
        "omitting the flag must omit the key entirely, not write it null: {document}"
    );
}

#[test]
fn an_invalid_convergence_policy_refuses_before_the_project_exists() {
    let workspace = Workspace::new();
    // Any one of the four counted dimensions at zero is enough to fail
    // `CardConvergenceLimits::validate`.
    let policy = convergence_policy_document(0, 3);
    let policy_path = write_json(&workspace, "policy.json", &policy);

    let output = Workspace::run(&convergence_init_args(&workspace, Some(&policy_path)));
    assert!(
        !output.status.success(),
        "a policy with a zero limit must refuse"
    );
    assert_eq!(error_code(&output), "CH-CONFIG-INVALID-VALUE", "{output:?}");
    assert!(
        !workspace.control.exists(),
        "a refused init must not leave the control directory behind at all"
    );
}

#[test]
fn an_unreadable_convergence_policy_file_refuses() {
    let workspace = Workspace::new();
    let missing = workspace.root.join("missing-policy.json");

    let output = Workspace::run(&convergence_init_args(
        &workspace,
        Some(&missing.display().to_string()),
    ));
    assert!(
        !output.status.success(),
        "a nonexistent policy path must refuse"
    );
    assert_eq!(error_code(&output), "CH-CONFIG-MALFORMED", "{output:?}");
    assert!(
        !workspace.control.exists(),
        "a refused init must not leave the control directory behind at all"
    );
}

#[test]
fn a_convergence_policy_with_an_unsupported_version_refuses() {
    let workspace = Workspace::new();
    let mut policy = convergence_policy_document(3, 3);
    policy["version"] = serde_json::json!("harness.convergence-policy/v2");
    let policy_path = write_json(&workspace, "policy.json", &policy);

    let output = Workspace::run(&convergence_init_args(&workspace, Some(&policy_path)));
    assert!(
        !output.status.success(),
        "an unsupported version must refuse"
    );
    assert_eq!(error_code(&output), "CH-CONFIG-INVALID-VALUE", "{output:?}");
    assert!(
        !workspace.control.exists(),
        "a refused init must not leave the control directory behind at all"
    );
}

// Installing a convergence policy on a project that already exists.
//
// 79-1 covers `project init --convergence-policy`, for a project that does
// not exist yet. `project set-convergence-policy` is the other half: it
// changes or first installs a policy on a project that may already have open
// cycles and recorded convergence facts, where a careless change breaks
// silently in two ways `read_convergence_policy` alone cannot catch — see
// `refuse_blocking_cycles` and `refuse_orphaning_facts` in
// `src/commands/project.rs`.

/// The `project set-convergence-policy` argv this section exercises.
fn set_convergence_policy_args(
    workspace: &Workspace,
    policy_path: &str,
    dry_run: bool,
) -> Vec<String> {
    let mut args = vec![
        "project".to_owned(),
        "set-convergence-policy".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--policy".to_owned(),
        policy_path.to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ];
    if dry_run {
        args.push("--dry-run".to_owned());
    }
    args
}

#[test]
fn set_convergence_policy_installs_on_a_project_with_no_open_cycles() {
    let workspace = Workspace::initialized();
    let policy = convergence_policy_document(3, 3);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let before = workspace.control_head();

    let output = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));
    assert!(
        output.status.success(),
        "installing on a project with no open cycles must succeed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        stored_project_document(&workspace)["convergence_policy"],
        policy,
        "the installed policy must reach the project document unchanged"
    );
    // The install must itself commit: a write left uncommitted would still
    // read back correctly here (the file is on disk) but would leave the
    // control repository's tree dirty and its head unmoved, with the change
    // only actually landing later, swept into whatever command happens to
    // commit next — attributed to the wrong operation, and invisible if
    // nothing ever runs again.
    assert_ne!(
        workspace.control_head(),
        before,
        "installing the policy must itself create a control commit"
    );
    assert_eq!(
        support::capture(&workspace.control, &["status", "--porcelain"]),
        "",
        "installing the policy must leave the control tree clean, not written but uncommitted"
    );

    // Worth nothing unless a cycle created afterward actually operates under
    // it — the same proof 79-1's own installation test requires.
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);

    let envelope = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(
        envelope["data"]["convergence"],
        serde_json::json!({ "status": "within" }),
        "a card in a cycle created after installation must see the policy in effect: {envelope}"
    );
}

#[test]
fn set_convergence_policy_refuses_while_a_cycle_could_still_break() {
    // `opened()` leaves cycle C-001 `active` with no policy configured —
    // exactly the shape whose next gate command discovered the mismatch by
    // accident before this command existed, as `opened_with_policy`'s own
    // doc comment records. A second cycle is sealed rather than left active,
    // so the refusal is shown naming two different reachable statuses, not
    // just one.
    let workspace = opened();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-002",
        "--objective",
        "Second slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-002"]);
    workspace.cycle(&["seal", "--cycle-id", "C-002"]);
    let before = workspace.control_head();

    let policy = convergence_policy_document(3, 3);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let output = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        !output.status.success(),
        "an active or sealed cycle must block the install: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-CYCLE", "{output:?}");
    let message = error_message(&output);
    assert!(
        message.contains("C-001") && message.contains("active"),
        "the refusal must name the active offending cycle and its status: {message}"
    );
    assert!(
        message.contains("C-002") && message.contains("sealed"),
        "the refusal must name the sealed offending cycle and its status too: {message}"
    );
    assert!(
        !stored_project_document(&workspace)
            .as_object()
            .expect("project.json is a JSON object")
            .contains_key("convergence_policy"),
        "a refused install must leave the project document unchanged"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "a refused install must not move the control repository's head"
    );
}

#[test]
fn set_convergence_policy_refuses_to_change_a_policy_once_facts_exist() {
    let workspace = opened_with_policy(3, 3);
    open_review_round(&workspace, "F-001");
    let path = write_verdict(&workspace, "F-001", RETURN_WITH_ACCEPTANCE_DEFECT_REASON);
    let recorded = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer",
    ]);
    assert!(
        recorded.status.success(),
        "the fixture return must be recorded: {}",
        String::from_utf8_lossy(&recorded.stdout)
    );
    assert_eq!(
        attempt_recorded_events(&workspace).len(),
        1,
        "the fixture must leave exactly one recorded fact"
    );

    // Abandoning C-001 takes it out of `refuse_blocking_cycles`'s offending
    // set (it is terminal) without erasing the fact just recorded against
    // it, so what follows isolates the facts check from the cycle check:
    // this test's refusal must come from the recorded fact, not from an
    // incidentally still-open cycle.
    workspace.cycle(&[
        "abandon",
        "--cycle-id",
        "C-001",
        "--reason",
        "isolate the facts check from the cycle check",
    ]);
    let before = workspace.control_head();

    // A different `integration_failures` limit is enough to change the
    // digest without touching `card_limits` at all.
    let different = convergence_policy_document(3, 5);
    let policy_path = write_json(&workspace, "different-policy.json", &different);
    let output = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        !output.status.success(),
        "changing the policy once a fact is recorded against the old digest must refuse"
    );
    assert_eq!(
        error_code(&output),
        "CH-CONFIG-CONTROL-INCOMPATIBLE",
        "{output:?}"
    );
    let message = error_message(&output);

    // This refusal used to end "That rebaseline is issue #74; until it
    // ships, this refuses instead of orphaning the recorded facts." — true
    // when written, silently false from the moment `disposition rebaseline`
    // shipped, and pinned in place by an assertion that required the `#74`
    // this now forbids. A test can hold a defect still as easily as it can
    // catch one; the assertion below is the same shape
    // `the_escalation_refusal_names_the_command_that_resolves_it` settled
    // on for the identical defect in `ErrorCode::convergence_recovery`,
    // and it is checked first for the same reason: an operator-facing
    // refusal that hands back an issue reference always contains a `#`,
    // and one that hands back a real command never needs to.
    assert!(
        !message.contains('#'),
        "the refusal must hand the operator a command, not an issue to go read: {message}"
    );
    assert!(
        message.contains("disposition rebaseline"),
        "the refusal must name the command that can make this change: {message}"
    );
    assert!(
        message.contains("entire view"),
        "the refusal must say why: the projection rejects the whole view, not just the mismatched fact: {message}"
    );

    // Cross-checked against the real `--help` surface, walking every flag
    // the message names, so a fabricated flag fails here even though it
    // would satisfy every assertion above. `tests/recovery_text.rs` cannot
    // reach this text at all: it scans `src/error.rs` recovery strings, and
    // this is a `HarnessError::Control` reason built in `src/commands/`.
    let help = Workspace::run(&[
        "disposition".to_owned(),
        "rebaseline".to_owned(),
        "--help".to_owned(),
    ]);
    assert!(
        help.status.success(),
        "the refusal names `disposition rebaseline`, so that command must exist: {}",
        String::from_utf8_lossy(&help.stderr)
    );
    let help = String::from_utf8_lossy(&help.stdout).into_owned();
    let named = flags_named_by(&message);
    assert!(
        named.iter().any(|flag| flag == "--policy")
            && named.iter().any(|flag| flag == "--rationale"),
        "the refusal must spell the flags an operator has to type: named {named:?} in {message}"
    );
    for flag in &named {
        assert!(
            help.contains(flag.as_str()),
            "the refusal spells `{flag}`, which `disposition rebaseline` does not accept: {help}"
        );
    }
    assert_eq!(
        stored_project_document(&workspace)["convergence_policy"]["cycle_limits"]["integration_failures"],
        serde_json::json!(3),
        "the original policy must remain installed, unchanged"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "a refused change must not move the control repository's head"
    );
}

#[test]
fn set_convergence_policy_refuses_when_facts_exist_but_no_policy_is_configured() {
    // `refuse_orphaning_facts` computes its remedy from `existing_digest:
    // Option<&Digest>`, and the two branches say different things on
    // purpose: `disposition rebaseline` retires a *configured* digest, so
    // with none configured it would refuse at its own check 2 ("no policy
    // digest to retire") — naming it here would hand the operator a second
    // dead end, the exact defect this file's sibling test above exists to
    // keep out of the *other* branch. That distinction is a comment in
    // `src/commands/project.rs`, not yet a behavior any test pins — the one
    // above only ever drives `existing_digest` to `Some`. This test drives
    // it to `None` instead, with a fact still on record.
    //
    // No CLI path produces that shape: every writer of an
    // `ATTEMPT_RECORDED_EVENT` is gated on `config.convergence_policy` being
    // `Some`, and nothing removes a policy once installed, so a fact cannot
    // outlive the policy that admitted it — exactly what the function's own
    // doc comment says. It is built here the same way
    // `Workspace::tamper_card_state` and `tamper_cycle_status` build the
    // other shapes no shipped command can reach: direct file surgery on the
    // control repository, standing in for the hand-edited project document
    // that comment names as this branch's only reachable caller.
    let workspace = opened_with_policy(3, 3);
    open_review_round(&workspace, "F-001");
    let path = write_verdict(&workspace, "F-001", RETURN_WITH_ACCEPTANCE_DEFECT_REASON);
    let recorded = workspace.review_raw(&[
        "record",
        "--card-id",
        "F-001",
        "--verdict",
        &path,
        "--actor",
        "reviewer",
    ]);
    assert!(
        recorded.status.success(),
        "the fixture return must be recorded: {}",
        String::from_utf8_lossy(&recorded.stdout)
    );
    assert_eq!(
        attempt_recorded_events(&workspace).len(),
        1,
        "the fixture must leave exactly one recorded fact"
    );

    // Same isolation the sibling test above performs, for the same reason:
    // an active C-001 would trip `refuse_blocking_cycles` first, and this
    // test would never reach the orphaned-facts check it exists to pin.
    workspace.cycle(&[
        "abandon",
        "--cycle-id",
        "C-001",
        "--reason",
        "isolate the facts check from the cycle check",
    ]);

    // The hand edit: strips `convergence_policy` back out of the project
    // document. The fact recorded above stays untouched in the event
    // store — nothing here touches `control/events/` — so the project now
    // has a recorded fact and no policy at all to have recorded it against.
    // Left uncommitted on purpose, matching `tamper_card_state` and
    // `tamper_cycle_status`: this simulates an edit made outside the
    // harness, not a step the harness itself performed. `ControlRepository::
    // project()` reads this file with a plain `fs::read_to_string`, so the
    // command under test sees it whether or not it is committed —
    // `a_cycle_in_a_terminal_state_does_not_block_the_install` already
    // relies on exactly that when it runs a real `set-convergence-policy`
    // straight after `tamper_cycle_status` leaves the tree dirty.
    let project_path = workspace.control.join("project/project.json");
    let mut document = stored_project_document(&workspace);
    document
        .as_object_mut()
        .expect("project.json is a JSON object")
        .remove("convergence_policy");
    fs::write(
        &project_path,
        format!("{}\n", serde_json::to_string_pretty(&document).unwrap()),
    )
    .unwrap();
    assert!(
        !stored_project_document(&workspace)
            .as_object()
            .expect("project.json is a JSON object")
            .contains_key("convergence_policy"),
        "the hand edit must actually remove the configured policy before the command runs"
    );
    let before = workspace.control_head();

    let policy = convergence_policy_document(3, 5);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let output = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        !output.status.success(),
        "installing a policy over facts recorded with none configured must still refuse: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        error_code(&output),
        "CH-CONFIG-CONTROL-INCOMPATIBLE",
        "{output:?}"
    );
    let message = error_message(&output);

    // The property this test exists for. With no policy configured,
    // `disposition rebaseline` has no digest to retire and would refuse at
    // its own second check, so naming it would trade this refusal for
    // another one instead of resolving anything — the same defect the
    // sibling test above pins out of the digest-configured branch, now
    // pinned out of this one too. A mutation that collapses both branches
    // to the digest branch's remedy leaves that sibling test green — it
    // never drives `existing_digest` to `None` — and is caught here.
    assert!(
        !message.contains("disposition rebaseline"),
        "with no policy configured there is no digest to retire, so the refusal must not send the operator to a command that would only refuse again: {message}"
    );

    // Two phrases lifted from the real remedy, chosen only after counting
    // how often each candidate appears across the *whole* message, mutated
    // and not. `"no configured policy"` was the first candidate tried and
    // the one rejected: it is `{currently}`'s value, part of the prefix
    // both branches share, so it reads exactly once in this message both
    // before and after the mutation above and cannot tell the two apart.
    // These two live only inside the branch-specific remedy sentence
    // (verified the same way: zero occurrences elsewhere in this message,
    // zero in the digest-configured branch's message, and zero once the
    // mutation replaces the sentence they come from), so either one
    // disappearing is the mutation, not noise.
    assert!(
        message.contains("no rebaseline applies either"),
        "the refusal must say why naming the command would not help: {message}"
    );
    assert!(
        message.contains("the control repository has to be inspected"),
        "the refusal must send the operator to inspect the control repository instead of to a command that would only refuse again: {message}"
    );
    assert!(
        message.contains("entire view"),
        "the refusal must still say why: the projection rejects the whole view, not just the mismatched fact: {message}"
    );

    assert!(
        !stored_project_document(&workspace)
            .as_object()
            .expect("project.json is a JSON object")
            .contains_key("convergence_policy"),
        "a refused install must leave the hand-edited project document alone"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "a refused change must not move the control repository's head"
    );
}

#[test]
fn reinstalling_an_identical_policy_succeeds_without_changing_anything() {
    // `opened_with_policy` also leaves cycle C-001 `active`, which proves the
    // idempotent path really does skip both refusals below rather than
    // happening to have nothing to refuse.
    let workspace = opened_with_policy(3, 3);
    let before = workspace.control_head();

    let identical = convergence_policy_document(3, 3);
    let policy_path = write_json(&workspace, "same-policy.json", &identical);
    let output = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        output.status.success(),
        "reinstalling a byte-identical policy must succeed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["changed"],
        serde_json::json!(false),
        "{envelope}"
    );
    assert_eq!(
        stored_project_document(&workspace)["convergence_policy"],
        identical
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "an idempotent reinstall must not create a new control commit"
    );
}

#[test]
fn set_convergence_policy_refuses_an_invalid_policy() {
    let workspace = Workspace::initialized();
    // Any one of the four counted dimensions at zero fails
    // `CardConvergenceLimits::validate`, the same as 79-1's equivalent test.
    let policy = convergence_policy_document(0, 3);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let before = workspace.control_head();

    let output = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        !output.status.success(),
        "a policy with a zero limit must refuse"
    );
    assert_eq!(error_code(&output), "CH-CONFIG-INVALID-VALUE", "{output:?}");
    assert!(
        !stored_project_document(&workspace)
            .as_object()
            .expect("project.json is a JSON object")
            .contains_key("convergence_policy"),
        "a refused install must leave the project document unchanged"
    );
    assert_eq!(workspace.control_head(), before);
}

#[test]
fn the_dry_run_makes_every_check_and_writes_nothing() {
    // The success case: nothing blocks, so the dry run reports what the real
    // command would do without doing it.
    let clear = Workspace::initialized();
    let policy = convergence_policy_document(3, 3);
    let policy_path = write_json(&clear, "policy.json", &policy);
    let before = clear.control_head();

    let output = Workspace::run(&set_convergence_policy_args(&clear, &policy_path, true));
    assert!(
        output.status.success(),
        "a dry run with nothing blocking must report success: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["dry_run"],
        serde_json::json!(true),
        "{envelope}"
    );
    assert!(
        !stored_project_document(&clear)
            .as_object()
            .expect("project.json is a JSON object")
            .contains_key("convergence_policy"),
        "a dry run must write nothing even when it would succeed"
    );
    assert_eq!(clear.control_head(), before);

    // The refusal case: an open cycle blocks, and the dry run must hit the
    // exact same refusal the real command would rather than promise success.
    let blocked = opened();
    let policy_path = write_json(&blocked, "policy.json", &policy);
    let before = blocked.control_head();

    let output = Workspace::run(&set_convergence_policy_args(&blocked, &policy_path, true));
    assert!(
        !output.status.success(),
        "a dry run must refuse exactly when the real command would"
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-CYCLE", "{output:?}");
    assert!(
        error_message(&output).contains("C-001"),
        "the dry run's refusal must still name the offending cycle: {}",
        error_message(&output)
    );
    assert!(
        !stored_project_document(&blocked)
            .as_object()
            .expect("project.json is a JSON object")
            .contains_key("convergence_policy"),
        "a dry run must write nothing even when it refuses"
    );
    assert_eq!(blocked.control_head(), before);
}

#[test]
fn a_cycle_in_a_terminal_state_does_not_block_the_install() {
    let workspace = Workspace::initialized();

    // `closed`: no command in this codebase can ever produce it —
    // `src/commands/cycle.rs`'s `store` has exactly five call sites
    // (create/activate/seal/declare-group/abandon) and none of them writes
    // `closed`. `tamper_cycle_status` simulates the future command Section
    // 11.1 already reserves the transition for.
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.tamper_cycle_status("C-001", "closed");

    // `abandoned`: genuinely reachable today, through the very command this
    // refusal's own message recommends running.
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-002",
        "--objective",
        "Second slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-002"]);
    workspace.cycle(&["abandon", "--cycle-id", "C-002", "--reason", "superseded"]);

    let policy = convergence_policy_document(3, 3);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let output = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        output.status.success(),
        "a closed or abandoned cycle must not block installing a convergence policy — refusing one would be refusing for no reason: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        stored_project_document(&workspace)["convergence_policy"],
        policy
    );
}

#[test]
fn a_draft_cycle_does_not_block_while_every_unreachable_status_does() {
    // Tests 2 and 7 pin four of the nine `CycleStatus` values against
    // reachable-today cycles: `active` and `sealed` block,
    // `closed` and `abandoned` do not. That leaves five unpinned —
    // `draft`, and the four statuses no shipped command produces yet
    // (`integrating`, `accepted`, `landed`, `blocked`). An enumeration that
    // is only reported and not regression-protected is a claim, not a
    // guarantee: an independent mutation removing `draft` from the
    // non-blocking set left the whole suite green. This test pins the
    // remaining five so `blocks_convergence_policy_change`'s full
    // classification cannot drift silently.
    //
    // `draft` is exercised through the real `cycle create`, never
    // activated — the ordinary shape of a cycle nobody has started yet, not
    // a tampered one. `integrating`, `accepted`, `landed`, and `blocked` are
    // exercised through `tamper_cycle_status`, the same simulate-a-future-
    // command helper `a_cycle_in_a_terminal_state_does_not_block_the_install`
    // uses for `closed`: no shipped command in this codebase can produce any
    // of the four today (`CycleStatus::successors` already reserves the
    // transitions for a later work package), so this is the only way to
    // construct one, and this test exists so their classification is
    // guarded before that work package ever ships.
    let workspace = Workspace::initialized();

    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Never activated",
    ]);

    let unreachable = [
        ("C-002", "integrating"),
        ("C-003", "accepted"),
        ("C-004", "landed"),
        ("C-005", "blocked"),
    ];
    for (cycle_id, status) in unreachable {
        workspace.cycle(&["create", "--cycle-id", cycle_id, "--objective", "Probe"]);
        workspace.cycle(&["activate", "--cycle-id", cycle_id]);
        workspace.tamper_cycle_status(cycle_id, status);
    }

    let policy = convergence_policy_document(3, 3);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let output = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        !output.status.success(),
        "integrating, accepted, landed, and blocked must all block: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-CYCLE", "{output:?}");
    let message = error_message(&output);
    for (cycle_id, status) in unreachable {
        assert!(
            message.contains(cycle_id) && message.contains(status),
            "the refusal must name {cycle_id} and its status `{status}`: {message}"
        );
    }
    assert!(
        !message.contains("C-001"),
        "a draft cycle must not be named as an offender: {message}"
    );
}

// #107: the refusal above used to end "Close or abandon them, or start a
// new cycle instead, before changing the {policy}." An operator blocked by
// exactly one sealed cycle followed the "start a new cycle" clause, started
// a second cycle, and the very next attempt named two offenders instead of
// one. `refuse_blocking_cycles` walks every cycle in the control repository
// with no notion of "current cycle" (see its own doc comment above
// `all_cycles`), so a freshly created cycle can only ever sit alongside an
// existing offender: it starts `draft`, which does not block, but the
// moment it is activated — the only way to make it useful — it becomes a
// second one.
//
// #107 review round two found a second, narrower defect in the clause that
// was left behind: "Close" names `CycleStatus::Closed`, and no command in
// this codebase can ever produce it (see `remedy_argv`'s doc comment
// below). Naming it is the same category of defect as "start a new cycle"
// — advice nobody can carry out — just milder, since the operator has
// "abandon" to fall back on rather than being driven backwards. The three
// tests below are #107's evidence: the first pins the false "start a new
// cycle" clause gone, the second pins the unreachable "close" gone, the
// third pins that what remains ("abandon") is true.
#[test]
fn the_policy_change_refusal_does_not_advise_starting_a_new_cycle() {
    let workspace = opened();
    let policy = convergence_policy_document(3, 3);
    let policy_path = write_json(&workspace, "policy.json", &policy);

    let output = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        !output.status.success(),
        "a single blocking cycle must still refuse: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-CYCLE", "{output:?}");
    let message = error_message(&output);
    assert!(
        message.contains("C-001") && message.contains("active"),
        "sanity: the refusal must still name the one offending cycle: {message}"
    );
    assert!(
        !message.contains("start a new cycle"),
        "the refusal must not advise an action that can only ever add a second offender \
         alongside the first, never remove it: {message}"
    );
}

// `following_the_refusal_reaches_a_state_where_the_policy_installs` below
// proves that *some* remedy the refusal names actually works — it cannot,
// on its own, prove that *every* named remedy does, because
// `perform_first_named_remedy` stops at the first one it recognizes and
// can execute. A message reading "Close or abandon them" still passes that
// test today (it finds "abandon" and stops looking), even though "close"
// cannot be carried out. This test is what actually pins "close" gone; see
// #107's report for why the two are deliberately separate tests rather
// than one broadened assertion.
#[test]
fn the_policy_change_refusal_does_not_advise_closing_a_cycle() {
    let workspace = opened();
    let policy = convergence_policy_document(3, 3);
    let policy_path = write_json(&workspace, "policy.json", &policy);

    let output = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        !output.status.success(),
        "a single blocking cycle must still refuse: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let message = error_message(&output);
    assert!(
        !message.to_lowercase().contains("close"),
        "the refusal must not name an action no command in this codebase can perform \
         (`CycleStatus::Closed` has no producer — see `remedy_argv`'s doc comment below): \
         {message}"
    );
}

/// Remedies this test can actually carry out against a blocking cycle,
/// mapped to the `cycle` subcommand argv that performs them.
///
/// Deliberately excludes `close`: no command in this codebase can ever
/// produce `CycleStatus::Closed` — `src/commands/cycle.rs`'s `store` has
/// exactly five call sites (create/activate/seal/declare-group/abandon) and
/// none of them writes `closed`, the same fact
/// `a_cycle_in_a_terminal_state_does_not_block_the_install` above already
/// establishes and leans on `tamper_cycle_status` to simulate. The refusal
/// no longer offers `close` as of #107 review round two
/// (`the_policy_change_refusal_does_not_advise_closing_a_cycle` above pins
/// that); `close` stays out of this map on purpose anyway, so that if a
/// future edit reintroduces the word, `perform_first_named_remedy` still
/// cannot act on it and `following_the_refusal_reaches_a_state_where_the_policy_installs`
/// keeps testing only remedies this harness can really perform.
fn remedy_argv<'a>(verb: &str, cycle_id: &'a str) -> Option<Vec<&'a str>> {
    match verb {
        "abandon" => Some(vec![
            "abandon",
            "--cycle-id",
            cycle_id,
            "--reason",
            "following the refusal's own remedy",
        ]),
        "seal" => Some(vec!["seal", "--cycle-id", cycle_id]),
        _ => None,
    }
}

/// Finds the remedy verb `message` names first (earliest byte position,
/// case-insensitively) among the verbs [`remedy_argv`] knows how to
/// execute, and runs it against `cycle_id` — so *what* this performs comes
/// from parsing `message`, not from an assumption hard-coded into the test
/// that calls it.
///
/// #107 §9 mutation 2 (advise a different, non-clearing action, such as
/// sealing) changes what this function does without changing a line of it:
/// it recognizes `seal` too, and sealing an already-`active` cycle succeeds
/// without leaving the blocking set (`Sealed` blocks exactly like `Active`
/// does), so the caller's later re-attempt is refused again and the
/// mutation is caught one level up, at the "policy installs" assertion —
/// not here.
fn perform_first_named_remedy(workspace: &Workspace, message: &str, cycle_id: &str) {
    let lowercase = message.to_lowercase();
    let chosen = ["abandon", "seal"]
        .into_iter()
        .filter_map(|verb| lowercase.find(verb).map(|position| (position, verb)))
        .min_by_key(|&(position, _)| position);

    let Some((_, verb)) = chosen else {
        panic!(
            "the refusal names no remedy this test knows how to execute (checked for \
             \"abandon\" and \"seal\"): {message}"
        );
    };
    let argv = remedy_argv(verb, cycle_id)
        .expect("every verb this function can choose has a matching remedy_argv arm");

    let output = workspace.cycle_raw(&argv);
    assert!(
        output.status.success(),
        "performing the refusal's own remedy (`{verb}`) must itself succeed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn following_the_refusal_reaches_a_state_where_the_policy_installs() {
    // The test that matters (#107 §8.2): it does not replay a fixed script
    // ("call `cycle abandon`") — it reads the refusal's own text and
    // performs whichever remedy the text names first, among the remedies
    // `perform_first_named_remedy` can actually execute. It proves *a*
    // named remedy works, not that *every* named remedy does — that is
    // `the_policy_change_refusal_does_not_advise_closing_a_cycle` above's
    // job, not this test's; see that test and `perform_first_named_remedy`'s
    // doc comment for why `close` is excluded from what this can execute.
    let workspace = opened();
    let policy = convergence_policy_document(3, 3);
    let policy_path = write_json(&workspace, "policy.json", &policy);

    let refused = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));
    assert!(
        !refused.status.success(),
        "the fixture must start out refused, or the rest of this test proves nothing: {}",
        String::from_utf8_lossy(&refused.stdout)
    );
    assert_eq!(
        error_code(&refused),
        "CH-POLICY-INVALID-CYCLE",
        "{refused:?}"
    );
    let message = error_message(&refused);

    perform_first_named_remedy(&workspace, &message, "C-001");

    let retried = Workspace::run(&set_convergence_policy_args(
        &workspace,
        &policy_path,
        false,
    ));
    assert!(
        retried.status.success(),
        "following exactly what the refusal now says must reach a state where the policy \
         installs: {}{}",
        String::from_utf8_lossy(&retried.stdout),
        String::from_utf8_lossy(&retried.stderr),
    );
    assert_eq!(
        stored_project_document(&workspace)["convergence_policy"],
        policy,
        "the policy named in the original, refused attempt must be the one now installed"
    );
}

// 74-2: `disposition renew`, run by an actor authorized under
// `final_authorization_policy.authorizer_actor_ids`, against a card
// currently escalated in the dimension it names, appends one bound
// `convergence.disposition_recorded` fact and the card can be delivered and
// reviewed again. Every other combination — no configured convergence
// policy, a card that is not escalated, a dimension that still has budget,
// an unauthorized actor, or a blank rationale — refuses before writing
// anything. `escalate_via_review_returns` and `redeliver_candidate`, already
// defined above for 72-2's own escalation tests, are reused rather than
// duplicated: they are the shortest real path to `Escalated` this file has,
// and `review_returns` is the first-listed of `--dimension`'s four closed
// values.
//
// Wrapped in its own module because `the_dry_run_makes_every_check_and_writes_nothing`
// is already the name `project.rs`'s own dry-run test uses above for the same
// property on a different command (`project set-convergence-policy`), and
// that existing test must stay exactly as it is — untouched and unrenamed.
// A module gives this card's instance of that same, deliberately recurring
// name a distinct path (`disposition_renew::the_dry_run_makes_every_check_and_writes_nothing`)
// without colliding.
mod disposition_renew {
    use super::*;

    /// Like `Workspace::initialized`, but also installs a final-authorization
    /// policy naming `authorizers` as `disposition renew`'s authorized actors —
    /// `final_authorization_policy.authorizer_actor_ids`, the same field
    /// `commands::acceptance::validate_final_authorization` already resolves to
    /// authorize a sealed cycle's final integration. `support::Workspace` is
    /// outside this card's file scope and its `initialized` helper does not
    /// accept this flag, so this mirrors its body directly rather than editing
    /// it.
    fn initialized_with_authorizers(authorizers: &[&str]) -> Workspace {
        let workspace = Workspace::new();
        let mut args: Vec<String> = vec![
            "project".into(),
            "init".into(),
            "--project-id".into(),
            "example".into(),
            "--repository".into(),
            workspace.repository.display().to_string(),
            "--control".into(),
            workspace.control.display().to_string(),
            "--authority".into(),
            workspace.authority.display().to_string(),
            "--worktree-root".into(),
            workspace.worktrees.display().to_string(),
        ];
        for authorizer in authorizers {
            args.push("--final-authorizer-actor-id".into());
            args.push((*authorizer).to_owned());
        }
        let output = Workspace::run(&args);
        assert!(
            output.status.success(),
            "project init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        workspace.register_gate("gate.unit", &["true"]);
        workspace.register_gate("gate.all", &["true"]);
        workspace
    }

    /// Like [`opened_with_policy`], but with a final-authorization policy
    /// installed too, so `disposition renew`'s authorization check (#4) has a
    /// configured set to resolve. Order matters exactly as it does in
    /// `opened_with_policy`: both policies must be in place before the cycle is
    /// created, which pins the project configuration's digest.
    fn opened_with_disposition_policies(
        card_limit: u32,
        integration_limit: u32,
        authorizers: &[&str],
    ) -> Workspace {
        let workspace = initialized_with_authorizers(authorizers);
        workspace.configure_convergence_policy(card_limit, integration_limit);
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

    /// Runs `disposition renew` in JSON mode, returning the raw output. Mirrors
    /// every other per-group `_raw` helper in this file; kept local because
    /// `support::Workspace` is outside this card's file scope.
    fn disposition_renew_raw(workspace: &Workspace, args: &[&str]) -> std::process::Output {
        let mut full = vec![
            "disposition".to_owned(),
            "renew".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
        ];
        full.extend(args.iter().map(|arg| (*arg).to_owned()));
        Workspace::run(&full)
    }

    /// Every recorded `convergence.disposition_recorded` fact.
    fn disposition_recorded_events(workspace: &Workspace) -> Vec<serde_json::Value> {
        workspace
            .events()
            .into_iter()
            .filter(|event| event["event_type"] == "convergence.disposition_recorded")
            .collect()
    }

    #[test]
    fn an_authorized_renewal_lets_an_escalated_card_deliver_again() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        let review_id = escalate_via_review_returns(&workspace, "F-001");
        let head = redeliver_candidate(&workspace, "F-001");
        let declaration = declaration_with_gate_failures(&workspace, "F-001", &head, "");

        let before = workspace.handoff_raw(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);
        assert!(
            !before.status.success(),
            "an escalated card must not be deliverable before renewal"
        );
        assert_eq!(error_code(&before), "CH-POLICY-CONVERGENCE-ESCALATED");
        assert!(
            error_message(&before).contains(&format!("review:{review_id}")),
            "{}",
            error_message(&before)
        );

        let card_status = workspace.card_json(&["status", "--card-id", "F-001"]);
        let base = workspace.authority_head();
        let pre_renew_head = workspace.control_head();

        let renew = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "authorized renewal for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            renew.status.success(),
            "an authorized renewal of an exhausted dimension must succeed: {}{}",
            String::from_utf8_lossy(&renew.stdout),
            String::from_utf8_lossy(&renew.stderr)
        );

        // 79-2's lesson, restated for this card by the contract: an event
        // written but not committed is invisible by content alone, because the
        // very next transaction (here, the retried `handoff create`) stages the
        // whole control tree and would sweep it in regardless. The only way to
        // catch "wrote but did not commit" is to check, right here, that this
        // command's own commit is what moved the head and left the tree clean —
        // before anything else touches the control repository.
        assert_ne!(
            workspace.control_head(),
            pre_renew_head,
            "disposition renew must commit its own write; the control head must move"
        );
        assert!(
            support::capture(&workspace.control, &["status", "--porcelain"]).is_empty(),
            "the control tree must be porcelain-clean immediately after a successful renewal"
        );

        let dispositions = disposition_recorded_events(&workspace);
        assert_eq!(
            dispositions.len(),
            1,
            "exactly one disposition fact must be recorded: {dispositions:?}"
        );
        let fact = &dispositions[0];
        assert_eq!(
            fact["event_type"], "convergence.disposition_recorded",
            "{fact}"
        );
        assert_eq!(fact["actor_id"], "owner", "{fact}");
        assert_eq!(fact["cycle_id"], "C-001", "{fact}");
        assert_eq!(fact["card_id"], "F-001", "{fact}");
        assert_eq!(
            fact["card_revision"], card_status["data"]["revision"],
            "{fact}"
        );
        assert_eq!(fact["card_digest"], card_status["data"]["digest"], "{fact}");
        assert_eq!(
            fact["head_sha"], base,
            "head must bind to the current revision's own base_sha, the one exact SHA a card is \
         guaranteed to carry in any state: {fact}"
        );
        assert_eq!(fact["metadata"]["disposition"], "renew", "{fact}");
        assert_eq!(fact["metadata"]["dimension"], "review_returns", "{fact}");
        assert_eq!(
            fact["metadata"]["rationale"], "authorized renewal for testing",
            "{fact}"
        );
        assert_eq!(fact["metadata"]["authorized_by"], "owner", "{fact}");
        assert_real_policy_digest(fact);

        // And now it really does deliver again, using the same candidate the
        // refused attempt already proved was otherwise ready.
        let after = workspace.handoff_raw(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);
        assert!(
            after.status.success(),
            "after renewal the card must deliver again: {}{}",
            String::from_utf8_lossy(&after.stdout),
            String::from_utf8_lossy(&after.stderr)
        );
    }

    #[test]
    fn the_escalation_refusal_names_the_command_that_resolves_it() {
        // The remedy handed to an operator when a card escalates has to
        // name the command that resolves it, and every flag that command
        // requires, in a shape `disposition renew` really accepts. For two
        // releases this text named an issue number instead of a command at
        // all — true when written, silently false from the moment
        // `disposition renew` shipped, because nothing tied the text to
        // the command surface. The fix that followed replaced the issue
        // number with the bare command name: an improvement, but not the
        // full promise — an operator who read it still had to go find out
        // which flags to type.
        //
        // The `!remedy.contains('#')` assertion below is this test's
        // single most important line: it is the one that would have
        // caught the original defect directly, because a remedy that
        // hands back an issue reference always contains a `#`, and one
        // that hands back a real command never needs to. Keep it even if
        // the rest of this test changes shape.
        let workspace = opened_with_policy(1, 3);
        escalate_via_review_returns(&workspace, "F-001");

        let output = workspace.review_raw(&["begin", "--card-id", "F-001", "--actor", "reviewer"]);
        assert!(
            !output.status.success(),
            "an escalated card must refuse a new review round: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(error_code(&output), "CH-POLICY-CONVERGENCE-ESCALATED");

        let envelope: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("an error envelope");
        let remedy = envelope["error"]["recovery"]
            .as_str()
            .expect("a coded refusal carries operator guidance")
            .to_owned();

        // Checked ahead of the command-name check on purpose: this is the
        // property the original defect violated, so it is what this test
        // must report first if a future edit ever resurrects an
        // issue-number remedy in place of a command.
        assert!(
            !remedy.contains('#'),
            "the remedy must hand the operator a command, not an issue to go read: {remedy}"
        );
        assert!(
            remedy.contains("disposition renew"),
            "the remedy must name the command that records a disposition: {remedy}"
        );

        // The three required flags must all be named, in the order an
        // operator types them. Filtered to just the required set so an
        // extra, unrelated flag mention elsewhere in the text (as
        // Mutation 3 plants) does not trip this assertion — catching that
        // is the cross-check below's job, and it needs a mutation that
        // leaves this assertion green to prove it is not redundant with
        // it.
        let named = flags_named_by(&remedy);
        let required = vec!["--card-id", "--dimension", "--rationale"];
        let named_required: Vec<&str> = named
            .iter()
            .map(String::as_str)
            .filter(|flag| required.contains(flag))
            .collect();
        assert_eq!(
            named_required, required,
            "the remedy must spell every flag an operator has to type, in the order they type \
             them: named {named:?} in {remedy}"
        );

        // Cross-checked against the command's real `--help` surface,
        // walking every flag the text actually names rather than the
        // filtered, already-known-good `named_required` above — so a
        // fabricated flag fails here even though it would satisfy every
        // earlier assertion. `tests/recovery_text.rs` would not catch it
        // either: that test only runs a backtick span through `--help`
        // when the span's first token is not itself a flag, so a bare
        // `--force` mention — exactly the shape this remedy uses for its
        // real flags — is invisible to it by design.
        let help = Workspace::run(&[
            "disposition".to_owned(),
            "renew".to_owned(),
            "--help".to_owned(),
        ]);
        assert!(
            help.status.success(),
            "the remedy names `disposition renew`, so that command must exist: {}",
            String::from_utf8_lossy(&help.stderr)
        );
        let help = String::from_utf8_lossy(&help.stdout).into_owned();
        for flag in &named {
            assert!(
                help.contains(flag.as_str()),
                "the remedy spells `{flag}`, which `disposition renew` does not accept: {help}"
            );
        }
    }

    #[test]
    fn an_unauthorized_actor_cannot_renew() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "an actor outside the configured set",
                "--actor",
                "intruder",
            ],
        );

        assert!(
            !output.status.success(),
            "an actor outside final_authorization_policy.authorizer_actor_ids must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_card_that_is_not_escalated_cannot_be_renewed() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        workspace.activate_card("F-001", &["src/**"]);

        let before_head = workspace.control_head();
        let output = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "nothing has happened yet",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a card that has never been escalated must not be pre-renewed"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_second_renewal_immediately_after_the_first_is_refused() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let first = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "first renewal",
                "--actor",
                "owner",
            ],
        );
        assert!(
            first.status.success(),
            "the first renewal must succeed: {}{}",
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );

        // 72-2's own boundary: a limit of 1, granted again once, is an effective
        // budget of 2 — and exactly one review-return fact is on record, so the
        // card reads `Within` again with no second attempt having occurred.
        let before_second_head = workspace.control_head();
        let second = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "second renewal, immediately",
                "--actor",
                "owner",
            ],
        );
        assert!(
            !second.status.success(),
            "an immediate second renewal must be refused: the card is Within again, with no extra \
         bookkeeping needed to remember the first renewal happened"
        );
        assert_eq!(error_code(&second), "CH-POLICY-INVALID-TRANSITION");
        assert_eq!(
            workspace.control_head(),
            before_second_head,
            "the control repository head must not move on refusal"
        );
        assert_eq!(
            disposition_recorded_events(&workspace).len(),
            1,
            "only the first renewal's fact may exist"
        );
    }

    #[test]
    fn renewing_a_dimension_that_still_has_budget_is_refused() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "gate-failures",
                "--rationale",
                "the wrong dimension",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "gate-failures still has budget; renewing it would grant budget ahead of exhaustion"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        let message = error_message(&output);
        assert!(
            message.contains("review_returns"),
            "the refusal must name the dimension that really is exhausted: {message}"
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn renewing_without_a_configured_policy_is_refused() {
        // No `configure_convergence_policy` call: an unconfigured project has no
        // budget in the first place, so there is nothing for any dimension to
        // renew.
        let workspace = opened();
        workspace.activate_card("F-001", &["src/**"]);

        let before_head = workspace.control_head();
        let output = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "there is no policy at all",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "renewal must be refused when no convergence policy is configured"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_renewal_without_a_rationale_is_refused() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "   ",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a blank rationale must be refused before anything is written"
        );
        assert_eq!(error_code(&output), "CH-USAGE-INVALID-ARGUMENTS");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn the_dry_run_makes_every_check_and_writes_nothing() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        // For the success the real command would make.
        let before_head = workspace.control_head();
        let success_preview = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "would renew if this were real",
                "--actor",
                "owner",
                "--dry-run",
            ],
        );
        assert!(
            success_preview.status.success(),
            "the dry run must report success when the real command would succeed: {}{}",
            String::from_utf8_lossy(&success_preview.stdout),
            String::from_utf8_lossy(&success_preview.stderr)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head"
        );
        assert!(
            disposition_recorded_events(&workspace).is_empty(),
            "a dry run must never write a fact"
        );

        // For at least one refusal — the same unauthorized-actor refusal test 2
        // exercises for the real command.
        let refusal_preview = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "would renew if this were real",
                "--actor",
                "intruder",
                "--dry-run",
            ],
        );
        assert!(
            !refusal_preview.status.success(),
            "the dry run must refuse the same way the real command would"
        );
        assert_eq!(error_code(&refusal_preview), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head, including on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());

        // Neither dry run consumed anything: the real renewal, run afterward,
        // still succeeds exactly once.
        let real = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "the real renewal",
                "--actor",
                "owner",
            ],
        );
        assert!(
            real.status.success(),
            "the real command must still succeed after both dry runs: {}{}",
            String::from_utf8_lossy(&real.stdout),
            String::from_utf8_lossy(&real.stderr)
        );
        assert_eq!(disposition_recorded_events(&workspace).len(), 1);
    }

    // #179: `disposition renew`'s two `PolicyNotAccepted` sites (no policy
    // configured; actor not authorized) share their recovery text,
    // word-for-word, with `acceptance record`'s own B2 site
    // (`tests/policy_not_accepted_recovery.rs`) — the situations are the
    // same regardless of which command, or which file, triggers them. The
    // two constants live in `src/commands/acceptance.rs` as `pub(crate)`,
    // so they cannot be imported into this external test crate; each
    // expected string below is copied verbatim, the same way
    // `tests/per_site_recovery.rs`'s own `FINAL_INTEGRATION_RECOVERY`
    // copies its migrated site's text rather than importing it.

    /// `FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY`
    /// (`src/commands/acceptance.rs`), copied verbatim.
    const POLICY_NOT_CONFIGURED_RECOVERY: &str = "`final_authorization_policy` is not configured for this project (or was removed since an earlier check relied on it); run `project example-final-authorization` for a complete, valid document, then install one with `project set-final-authorization-policy`.";

    /// `FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY`
    /// (`src/commands/acceptance.rs`), copied verbatim — the exact text
    /// B2 (#179 §1, §8) requires.
    const ACTOR_NOT_AUTHORIZED_RECOVERY: &str = "This actor is not among `final_authorization_policy.authorizer_actor_ids`; run `project example-final-authorization` to see a configured policy's shape, then retry as one of the listed actors or add this one with `project set-final-authorization-policy`.";

    #[test]
    fn renew_with_no_final_authorization_policy_gets_group_1s_recovery() {
        // `opened_with_policy` installs a convergence policy but never a
        // final-authorization one (it runs plain `Workspace::initialized`,
        // which passes no `--final-authorizer-actor-id`) — exactly the
        // "no policy at all" situation group 1 covers.
        let workspace = opened_with_policy(1, 3);
        escalate_via_review_returns(&workspace, "F-001");

        let output = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "no policy configured",
                "--actor",
                "owner",
            ],
        );
        assert!(
            !output.status.success(),
            "a renewal with no final-authorization policy configured must refuse"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");

        let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let recovery = envelope["error"]["recovery"]
            .as_str()
            .expect("a recovery string");
        assert_eq!(
            recovery, POLICY_NOT_CONFIGURED_RECOVERY,
            "a disposition.rs site sharing group 1 must carry byte-identical text to \
             acceptance.rs's own copy of the same constant; got: {recovery:?}"
        );
    }

    #[test]
    fn renew_by_an_unauthorized_actor_gets_group_2s_recovery_identical_to_b2() {
        // Only `owner` is a configured authorizer; `intern` is not.
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let output = disposition_renew_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--rationale",
                "unauthorized actor",
                "--actor",
                "intern",
            ],
        );
        assert!(
            !output.status.success(),
            "a renewal by an actor absent from authorizer_actor_ids must refuse"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");

        let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let recovery = envelope["error"]["recovery"]
            .as_str()
            .expect("a recovery string");
        assert_eq!(
            recovery, ACTOR_NOT_AUTHORIZED_RECOVERY,
            "a disposition.rs site sharing group 2 must carry byte-identical text to B2's own \
             `acceptance record` refusal (tests/policy_not_accepted_recovery.rs); got: \
             {recovery:?}"
        );
    }
}

// 74-4: `disposition rebaseline`, run by an actor authorized under
// `final_authorization_policy.authorizer_actor_ids`, retires the currently
// configured convergence policy digest, installs a new one, and re-pins
// every non-terminal cycle's `project_revision` to it — one transaction,
// three effects. This is the emergency exit `project set-convergence-policy`
// itself refuses to be: 79-1 and 79-2 gave that command a correct refusal
// for exactly the situation this command exists to resolve (an open cycle,
// or a fact already recorded under the current digest), and that refusal
// names this card by number.
//
// Wrapped in its own module for the same reason `disposition_renew` is:
// `the_dry_run_makes_every_check_and_writes_nothing` is already the name
// used above, twice, for the same property on two other commands, and
// neither existing test may be touched or renamed. A module gives this
// card's instance of that recurring name a distinct path
// (`disposition_rebaseline::the_dry_run_makes_every_check_and_writes_nothing`)
// without colliding.
mod disposition_rebaseline {
    use super::*;

    /// Identical in shape to `disposition_renew`'s own copy of this helper:
    /// `support::Workspace` is outside this card's file scope, so each
    /// disposition module keeps its fixture-building local rather than
    /// reaching into a sibling module's private helpers.
    fn initialized_with_authorizers(authorizers: &[&str]) -> Workspace {
        let workspace = Workspace::new();
        let mut args: Vec<String> = vec![
            "project".into(),
            "init".into(),
            "--project-id".into(),
            "example".into(),
            "--repository".into(),
            workspace.repository.display().to_string(),
            "--control".into(),
            workspace.control.display().to_string(),
            "--authority".into(),
            workspace.authority.display().to_string(),
            "--worktree-root".into(),
            workspace.worktrees.display().to_string(),
        ];
        for authorizer in authorizers {
            args.push("--final-authorizer-actor-id".into());
            args.push((*authorizer).to_owned());
        }
        let output = Workspace::run(&args);
        assert!(
            output.status.success(),
            "project init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        workspace.register_gate("gate.unit", &["true"]);
        workspace.register_gate("gate.all", &["true"]);
        workspace
    }

    /// Like [`opened_with_policy`], but with a final-authorization policy
    /// installed too, so `disposition rebaseline`'s authorization check has
    /// a configured set to resolve. Order matters exactly as it does in
    /// `opened_with_policy`: both policies must be in place before the
    /// cycle is created, which pins the project configuration's digest.
    fn opened_with_disposition_policies(
        card_limit: u32,
        integration_limit: u32,
        authorizers: &[&str],
    ) -> Workspace {
        let workspace = initialized_with_authorizers(authorizers);
        workspace.configure_convergence_policy(card_limit, integration_limit);
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

    /// Runs `disposition rebaseline` in JSON mode, returning the raw
    /// output. Mirrors every other per-group `_raw` helper in this file;
    /// kept local because `support::Workspace` is outside this card's file
    /// scope.
    fn disposition_rebaseline_raw(workspace: &Workspace, args: &[&str]) -> std::process::Output {
        let mut full = vec![
            "disposition".to_owned(),
            "rebaseline".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
        ];
        full.extend(args.iter().map(|arg| (*arg).to_owned()));
        Workspace::run(&full)
    }

    /// Every recorded `convergence.disposition_recorded` fact. Every fact
    /// this module's fixtures ever record is a rebaseline (none configures
    /// a renewal too), so this is never filtered further by
    /// `metadata.disposition`.
    fn disposition_recorded_events(workspace: &Workspace) -> Vec<serde_json::Value> {
        workspace
            .events()
            .into_iter()
            .filter(|event| event["event_type"] == "convergence.disposition_recorded")
            .collect()
    }

    /// The stored record for one cycle, read directly from control state —
    /// the same file `support::Workspace::tamper_cycle_status` edits, read
    /// back rather than mutated.
    fn stored_cycle(workspace: &Workspace, cycle_id: &str) -> serde_json::Value {
        let raw =
            fs::read_to_string(workspace.control.join(format!("cycles/{cycle_id}.json"))).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    /// A second, distinct convergence policy document — distinguishable
    /// from whatever `configure_convergence_policy`/`convergence_policy_
    /// document` installed by at least one limit unless `card_limit` and
    /// `integration_limit` are passed identically on purpose — written to
    /// its own file under the workspace root, ready for `--policy`.
    fn other_policy(workspace: &Workspace, card_limit: u32, integration_limit: u32) -> String {
        let policy = convergence_policy_document(card_limit, integration_limit);
        write_json(workspace, "rebaseline-policy.json", &policy)
    }

    #[test]
    // This is the end-to-end proof of the card's whole reason to exist, so
    // it checks the locked exit, the transaction, the fact shape, the
    // re-pin, the headline "still usable" claim, and the counter reset in
    // one place rather than splitting one coherent scenario across tests
    // that would each need to rebuild most of this fixture anyway.
    #[allow(clippy::too_many_lines)]
    fn a_rebaseline_retires_installs_and_repins_so_an_open_cycle_keeps_working() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        let escalating_fact = &attempt_recorded_events(&workspace)[0];
        assert_real_policy_digest(escalating_fact);
        let old_digest = escalating_fact["metadata"]["policy_digest"]
            .as_str()
            .expect("the escalating fact names the policy digest it was recorded under")
            .to_owned();
        let before_cycle = stored_cycle(&workspace, "C-001");

        // The locked exit: today, with C-001 open and a fact already
        // recorded against the current digest, `set-convergence-policy`
        // refuses outright — the exact defect this card exists to resolve.
        let new_policy_path = other_policy(&workspace, 3, 3);
        let before_locked = workspace.control_head();
        let locked = Workspace::run(&set_convergence_policy_args(
            &workspace,
            &new_policy_path,
            false,
        ));
        assert!(
            !locked.status.success(),
            "set-convergence-policy must still refuse while C-001 is open and a fact is recorded, which is exactly the locked exit rebaseline exists to open: {}",
            String::from_utf8_lossy(&locked.stdout)
        );
        assert_eq!(error_code(&locked), "CH-POLICY-INVALID-CYCLE", "{locked:?}");
        assert_eq!(
            workspace.control_head(),
            before_locked,
            "the refused install must not move the control head"
        );

        let pre_head = workspace.control_head();
        let output = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &new_policy_path,
                "--rationale",
                "opening the emergency exit for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            output.status.success(),
            "an authorized rebaseline must succeed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        // 79-2's lesson, restated by the contract for this card: an event
        // written but not committed is invisible by content alone, because
        // the very next transaction would stage the whole control tree and
        // sweep it in regardless. The only way to catch "wrote but did not
        // commit" is to check, right here, that this command's own commit
        // is what moved the head and left the tree clean — before anything
        // else touches the control repository.
        assert_ne!(
            workspace.control_head(),
            pre_head,
            "disposition rebaseline must commit its own write; the control head must move"
        );
        assert!(
            support::capture(&workspace.control, &["status", "--porcelain"]).is_empty(),
            "the control tree must be porcelain-clean immediately after a successful rebaseline"
        );

        // The new policy governs.
        assert_eq!(
            stored_project_document(&workspace)["convergence_policy"],
            convergence_policy_document(3, 3),
            "the new policy must reach the project document unchanged"
        );

        // Exactly one rebaseline fact, bound to C-001, no card.
        let facts = disposition_recorded_events(&workspace);
        assert_eq!(
            facts.len(),
            1,
            "exactly one cycle is non-terminal, so exactly one rebaseline fact must be recorded: {facts:?}"
        );
        let fact = &facts[0];
        assert_eq!(
            fact["event_type"], "convergence.disposition_recorded",
            "{fact}"
        );
        assert_eq!(fact["actor_id"], "owner", "{fact}");
        assert_eq!(fact["cycle_id"], "C-001", "{fact}");
        assert!(
            fact["card_id"].is_null(),
            "a rebaseline names no card: {fact}"
        );
        assert!(fact["card_revision"].is_null(), "{fact}");
        assert!(fact["card_digest"].is_null(), "{fact}");
        assert_eq!(
            fact["head_sha"],
            workspace.authority_head(),
            "head must bind to the protected branch's exact commit at the moment of the decision: {fact}"
        );
        assert_eq!(fact["metadata"]["disposition"], "rebaseline", "{fact}");
        assert_eq!(
            fact["metadata"]["retired_policy_digest"], old_digest,
            "{fact}"
        );
        assert_eq!(
            fact["metadata"]["rationale"], "opening the emergency exit for testing",
            "{fact}"
        );
        assert_eq!(fact["metadata"]["authorized_by"], "owner", "{fact}");
        assert_real_policy_digest(fact);
        assert_ne!(
            fact["metadata"]["policy_digest"], old_digest,
            "the fact's own governing digest must be the newly installed one, not the retired one: {fact}"
        );

        // The cycle itself is re-pinned to the new project revision.
        let after_cycle = stored_cycle(&workspace, "C-001");
        assert_ne!(
            after_cycle["project_revision"], before_cycle["project_revision"],
            "C-001 must be re-pinned to the new project revision"
        );

        // The headline claim: the open cycle keeps working. A new card can
        // be declared in it, and the gate command that actually runs
        // gate.rs:791's `project_revision` comparison does not refuse it
        // with `CH-POLICY-INVALID-CYCLE`.
        // `escalate_via_review_returns` activated F-001 over `src/**`
        // (`open_review_round`'s own fixed scope), so F-002 must claim a
        // disjoint path or `card activate` would correctly refuse on an
        // ownership overlap that has nothing to do with what this test
        // checks.
        let activation = activate_with_scope(&workspace, "F-002", &["tests/promotion.rs"]);
        assert!(
            activation.status.success(),
            "a card must still be declarable in the re-pinned cycle: {}{}",
            String::from_utf8_lossy(&activation.stdout),
            String::from_utf8_lossy(&activation.stderr)
        );
        let preflight = workspace.gate_raw(&["preflight", "--card-id", "F-002"]);
        assert!(
            preflight.status.success(),
            "the re-pinned cycle must not fail gate.rs:791's project_revision comparison with CH-POLICY-INVALID-CYCLE: {}{}",
            String::from_utf8_lossy(&preflight.stdout),
            String::from_utf8_lossy(&preflight.stderr)
        );

        // The counters count only what's after: the fact recorded under the
        // retired digest stops counting toward F-001's budget, but stays in
        // the record rather than being erased.
        let status = workspace.card_json(&["status", "--card-id", "F-001"]);
        assert_eq!(
            status["data"]["convergence"],
            serde_json::json!({ "status": "within" }),
            "the fact recorded under the retired digest must stop counting once rebaselined: {status}"
        );
        assert_eq!(
            attempt_recorded_events(&workspace).len(),
            1,
            "the retired fact must remain in the record, not be erased"
        );
    }

    #[test]
    fn an_unauthorized_actor_cannot_rebaseline() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        let policy_path = other_policy(&workspace, 3, 3);

        let before_head = workspace.control_head();
        let output = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &policy_path,
                "--rationale",
                "an actor outside the configured set",
                "--actor",
                "intruder",
            ],
        );

        assert!(
            !output.status.success(),
            "an actor outside final_authorization_policy.authorizer_actor_ids must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
        assert_eq!(
            stored_project_document(&workspace)["convergence_policy"],
            convergence_policy_document(1, 3),
            "a refused rebaseline must leave the original policy installed, unchanged"
        );
    }

    #[test]
    fn rebaselining_to_an_identical_policy_is_refused() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        // Same card_limit and integration_limit as `opened_with_disposition_
        // policies` just installed: byte-identical once serialized, so this
        // names the same digest already in force.
        let identical_path = other_policy(&workspace, 1, 3);

        let before_head = workspace.control_head();
        let output = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &identical_path,
                "--rationale",
                "retiring a digest to reinstall it unchanged",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "retiring a digest to reinstall it unchanged must be refused: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn rebaselining_without_a_configured_policy_is_refused() {
        // No `configure_convergence_policy` call: an unconfigured project
        // has no digest to retire in the first place.
        let workspace = initialized_with_authorizers(&["owner"]);
        workspace.cycle(&[
            "create",
            "--cycle-id",
            "C-001",
            "--objective",
            "First slice",
        ]);
        workspace.cycle(&["activate", "--cycle-id", "C-001"]);
        let policy_path = other_policy(&workspace, 3, 3);

        let before_head = workspace.control_head();
        let output = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &policy_path,
                "--rationale",
                "there is no policy at all",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "rebaseline must be refused when no convergence policy is configured"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        assert!(
            error_message(&output).contains("project set-convergence-policy"),
            "the refusal must name the command that installs the first policy: {}",
            error_message(&output)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_rebaseline_without_a_rationale_is_refused() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        let policy_path = other_policy(&workspace, 3, 3);

        let before_head = workspace.control_head();
        let output = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &policy_path,
                "--rationale",
                "   ",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a blank rationale must be refused before anything is written"
        );
        assert_eq!(error_code(&output), "CH-USAGE-INVALID-ARGUMENTS");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn terminal_cycles_are_neither_repinned_nor_given_a_fact() {
        let workspace = initialized_with_authorizers(&["owner"]);
        workspace.configure_convergence_policy(1, 3);

        // `closed`: no command in this codebase can produce it yet, the
        // same limitation `a_cycle_in_a_terminal_state_does_not_block_the_
        // install` documents for `set-convergence-policy`'s own equivalent
        // fixture; `tamper_cycle_status` simulates the future command
        // Section 11.1 already reserves the transition for.
        workspace.cycle(&[
            "create",
            "--cycle-id",
            "C-001",
            "--objective",
            "First slice",
        ]);
        workspace.cycle(&["activate", "--cycle-id", "C-001"]);
        workspace.tamper_cycle_status("C-001", "closed");

        // `abandoned`: genuinely reachable today.
        workspace.cycle(&[
            "create",
            "--cycle-id",
            "C-002",
            "--objective",
            "Second slice",
        ]);
        workspace.cycle(&["activate", "--cycle-id", "C-002"]);
        workspace.cycle(&["abandon", "--cycle-id", "C-002", "--reason", "superseded"]);

        let before_c001 = stored_cycle(&workspace, "C-001");
        let before_c002 = stored_cycle(&workspace, "C-002");

        let policy_path = other_policy(&workspace, 3, 3);
        let output = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &policy_path,
                "--rationale",
                "no non-terminal cycle should be touched",
                "--actor",
                "owner",
            ],
        );

        assert!(
            output.status.success(),
            "a rebaseline with only terminal cycles present must still succeed, re-pinning none of them: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            stored_project_document(&workspace)["convergence_policy"],
            convergence_policy_document(3, 3),
            "the new policy must still install even when no cycle needs re-pinning"
        );

        assert_eq!(
            stored_cycle(&workspace, "C-001"),
            before_c001,
            "a closed cycle must not be re-pinned or otherwise touched"
        );
        assert_eq!(
            stored_cycle(&workspace, "C-002"),
            before_c002,
            "an abandoned cycle must not be re-pinned or otherwise touched"
        );
        assert!(
            disposition_recorded_events(&workspace).is_empty(),
            "with only terminal cycles present, no rebaseline fact should be recorded at all"
        );
    }

    #[test]
    fn a_draft_cycle_is_repinned_so_it_does_not_collide_when_it_activates() {
        let workspace = initialized_with_authorizers(&["owner"]);
        workspace.configure_convergence_policy(1, 3);

        // A draft cycle: created under the current policy, never activated.
        // `set_convergence_policy_refuses_while_a_cycle_could_still_break`'s
        // own sibling test already pins that a draft cannot yet block
        // `set-convergence-policy`, because it cannot hold a card that
        // reaches `gate::validation_progress`'s comparison. But its
        // `project_revision` is fixed the moment it is created, so it would
        // collide the instant it later activates, unless this rebaseline
        // re-pins it now.
        workspace.cycle(&[
            "create",
            "--cycle-id",
            "C-001",
            "--objective",
            "Not yet started",
        ]);
        let before = stored_cycle(&workspace, "C-001");
        assert_eq!(before["status"], "draft");

        let policy_path = other_policy(&workspace, 3, 3);
        let output = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &policy_path,
                "--rationale",
                "a draft must be repinned before it collides later",
                "--actor",
                "owner",
            ],
        );
        assert!(
            output.status.success(),
            "rebaseline must succeed with only a draft cycle present: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let after = stored_cycle(&workspace, "C-001");
        assert_ne!(
            after["project_revision"], before["project_revision"],
            "a draft cycle's project_revision must be re-pinned even though it cannot block today"
        );
        let facts = disposition_recorded_events(&workspace);
        assert_eq!(
            facts.len(),
            1,
            "the draft cycle must receive its own rebaseline fact: {facts:?}"
        );
        assert_eq!(facts[0]["cycle_id"], "C-001", "{:?}", facts[0]);

        // And now it really can activate without colliding: `cycle
        // activate` never touches `project_revision`, so the value this
        // rebaseline just set survives activation untouched, and the very
        // next command that depends on it — a card declared and
        // preflighted inside — must not hit `CH-POLICY-INVALID-CYCLE`.
        workspace.cycle(&["activate", "--cycle-id", "C-001"]);
        let activation = activate_with_scope(&workspace, "F-001", &["src/policy/actors.rs"]);
        assert!(
            activation.status.success(),
            "a card must be declarable once the re-pinned draft activates: {}{}",
            String::from_utf8_lossy(&activation.stdout),
            String::from_utf8_lossy(&activation.stderr)
        );
        let preflight = workspace.gate_raw(&["preflight", "--card-id", "F-001"]);
        assert!(
            preflight.status.success(),
            "the re-pinned, now-active cycle must not fail gate.rs:791's project_revision comparison: {}{}",
            String::from_utf8_lossy(&preflight.stdout),
            String::from_utf8_lossy(&preflight.stderr)
        );
    }

    #[test]
    fn the_dry_run_makes_every_check_and_writes_nothing() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);

        // For the success the real command would make.
        let policy_path = other_policy(&workspace, 3, 3);
        let before_head = workspace.control_head();
        let success_preview = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &policy_path,
                "--rationale",
                "would rebaseline if this were real",
                "--actor",
                "owner",
                "--dry-run",
            ],
        );
        assert!(
            success_preview.status.success(),
            "the dry run must report success when the real command would succeed: {}{}",
            String::from_utf8_lossy(&success_preview.stdout),
            String::from_utf8_lossy(&success_preview.stderr)
        );
        let envelope: serde_json::Value = serde_json::from_slice(&success_preview.stdout).unwrap();
        assert_eq!(
            envelope["data"]["dry_run"],
            serde_json::json!(true),
            "{envelope}"
        );
        assert_eq!(
            envelope["data"]["repinned_cycles"],
            serde_json::json!(["C-001"]),
            "the preview must name the cycle that would be re-pinned: {envelope}"
        );
        assert_eq!(
            envelope["data"]["fact_count"],
            serde_json::json!(1),
            "{envelope}"
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head"
        );
        assert!(
            disposition_recorded_events(&workspace).is_empty(),
            "a dry run must never write a fact"
        );
        assert_eq!(
            stored_project_document(&workspace)["convergence_policy"],
            convergence_policy_document(1, 3),
            "a dry run must never install the new policy"
        );

        // For at least one refusal — the same unauthorized-actor refusal
        // test 2 exercises for the real command.
        let refusal_preview = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &policy_path,
                "--rationale",
                "would rebaseline if this were real",
                "--actor",
                "intruder",
                "--dry-run",
            ],
        );
        assert!(
            !refusal_preview.status.success(),
            "the dry run must refuse the same way the real command would"
        );
        assert_eq!(error_code(&refusal_preview), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head, including on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());

        // Neither dry run consumed anything: the real rebaseline, run
        // afterward, still succeeds exactly once.
        let real = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &policy_path,
                "--rationale",
                "the real rebaseline",
                "--actor",
                "owner",
            ],
        );
        assert!(
            real.status.success(),
            "the real command must still succeed after both dry runs: {}{}",
            String::from_utf8_lossy(&real.stdout),
            String::from_utf8_lossy(&real.stderr)
        );
        assert_eq!(disposition_recorded_events(&workspace).len(), 1);
    }
}

// 74-5: `disposition abandon`, run by an actor authorized under
// `final_authorization_policy.authorizer_actor_ids`, permanently ends an
// escalated card by recording one bound `convergence.disposition_recorded`
// fact — the authorized *escalation exit* `renew` is not: instead of
// granting the exhausted dimension one more configured limit, it retires
// the card itself. The property this card actually exists to prove is
// narrower than "the command works": an abandon fact names no `dimension`,
// and `project` must keep it out of the `DispositionMetadata` parse that
// requires one, or one abandoned card would break `card status` and every
// budget-gated command for every *other* card sharing its cycle — #74's own
// "preserves valid unrelated work" criterion.
//
// Wrapped in its own module for the same reason `disposition_rebaseline` is:
// `the_dry_run_makes_every_check_and_writes_nothing` is already the name
// used above, twice, for the same property on two other commands, and
// neither existing test may be touched or renamed. A module gives this
// card's instance of that recurring name a distinct path
// (`disposition_abandon::the_dry_run_makes_every_check_and_writes_nothing`)
// without colliding.
mod disposition_abandon {
    use super::*;

    /// Identical in shape to `disposition_renew`'s and
    /// `disposition_rebaseline`'s own copies of this helper:
    /// `support::Workspace` is outside this card's file scope, so each
    /// disposition module keeps its fixture-building local rather than
    /// reaching into a sibling module's private helpers.
    fn initialized_with_authorizers(authorizers: &[&str]) -> Workspace {
        let workspace = Workspace::new();
        let mut args: Vec<String> = vec![
            "project".into(),
            "init".into(),
            "--project-id".into(),
            "example".into(),
            "--repository".into(),
            workspace.repository.display().to_string(),
            "--control".into(),
            workspace.control.display().to_string(),
            "--authority".into(),
            workspace.authority.display().to_string(),
            "--worktree-root".into(),
            workspace.worktrees.display().to_string(),
        ];
        for authorizer in authorizers {
            args.push("--final-authorizer-actor-id".into());
            args.push((*authorizer).to_owned());
        }
        let output = Workspace::run(&args);
        assert!(
            output.status.success(),
            "project init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        workspace.register_gate("gate.unit", &["true"]);
        workspace.register_gate("gate.all", &["true"]);
        workspace
    }

    /// Like [`opened_with_policy`], but with a final-authorization policy
    /// installed too, so `disposition abandon`'s authorization check (#4)
    /// has a configured set to resolve. Order matters exactly as it does in
    /// `opened_with_policy`: both policies must be in place before the
    /// cycle is created, which pins the project configuration's digest.
    fn opened_with_disposition_policies(
        card_limit: u32,
        integration_limit: u32,
        authorizers: &[&str],
    ) -> Workspace {
        let workspace = initialized_with_authorizers(authorizers);
        workspace.configure_convergence_policy(card_limit, integration_limit);
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

    /// Runs `disposition abandon` in JSON mode, returning the raw output.
    /// Mirrors every other per-group `_raw` helper in this file; kept local
    /// because `support::Workspace` is outside this card's file scope.
    fn disposition_abandon_raw(workspace: &Workspace, args: &[&str]) -> std::process::Output {
        let mut full = vec![
            "disposition".to_owned(),
            "abandon".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
        ];
        full.extend(args.iter().map(|arg| (*arg).to_owned()));
        Workspace::run(&full)
    }

    /// Every recorded `convergence.disposition_recorded` fact. Every fact
    /// this module's fixtures ever record is an abandon (none configures a
    /// renewal or rebaseline too), so this is never filtered further by
    /// `metadata.disposition`.
    fn disposition_recorded_events(workspace: &Workspace) -> Vec<serde_json::Value> {
        workspace
            .events()
            .into_iter()
            .filter(|event| event["event_type"] == "convergence.disposition_recorded")
            .collect()
    }

    #[test]
    fn an_authorized_abandon_ends_an_escalated_card() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let card_status = workspace.card_json(&["status", "--card-id", "F-001"]);
        let base = workspace.authority_head();
        let pre_abandon_head = workspace.control_head();

        let abandon = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "authorized abandon for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            abandon.status.success(),
            "an authorized abandon of an escalated card must succeed: {}{}",
            String::from_utf8_lossy(&abandon.stdout),
            String::from_utf8_lossy(&abandon.stderr)
        );

        // 79-2's lesson, restated by the contract for this card: an event
        // written but not committed is invisible by content alone, because
        // the very next transaction would stage the whole control tree and
        // sweep it in regardless. The only way to catch "wrote but did not
        // commit" is to check, right here, that this command's own commit
        // is what moved the head and left the tree clean — before anything
        // else touches the control repository.
        assert_ne!(
            workspace.control_head(),
            pre_abandon_head,
            "disposition abandon must commit its own write; the control head must move"
        );
        assert!(
            support::capture(&workspace.control, &["status", "--porcelain"]).is_empty(),
            "the control tree must be porcelain-clean immediately after a successful abandon"
        );

        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "abandoned",
            "the card must move to abandoned"
        );

        let dispositions = disposition_recorded_events(&workspace);
        assert_eq!(
            dispositions.len(),
            1,
            "exactly one disposition fact must be recorded: {dispositions:?}"
        );
        let fact = &dispositions[0];
        assert_eq!(
            fact["event_type"], "convergence.disposition_recorded",
            "{fact}"
        );
        assert_eq!(fact["actor_id"], "owner", "{fact}");
        assert_eq!(fact["cycle_id"], "C-001", "{fact}");
        assert_eq!(fact["card_id"], "F-001", "{fact}");
        assert_eq!(
            fact["card_revision"], card_status["data"]["revision"],
            "{fact}"
        );
        assert_eq!(fact["card_digest"], card_status["data"]["digest"], "{fact}");
        assert_eq!(
            fact["head_sha"], base,
            "head must bind to the current revision's own base_sha, the one exact SHA a card is \
         guaranteed to carry in any state: {fact}"
        );
        // `escalate_via_review_returns` leaves the card `changes_requested`;
        // this fact must record that exact transition, unlike `renew`'s own
        // fact, which names no transition at all because renewing spends no
        // state change.
        assert_eq!(fact["previous_state"], "changes_requested", "{fact}");
        assert_eq!(fact["next_state"], "abandoned", "{fact}");
        assert_eq!(fact["metadata"]["disposition"], "abandon", "{fact}");
        assert_eq!(
            fact["metadata"]["rationale"], "authorized abandon for testing",
            "{fact}"
        );
        assert_eq!(fact["metadata"]["authorized_by"], "owner", "{fact}");
        assert_real_policy_digest(fact);
        assert!(
            !fact["metadata"]
                .as_object()
                .expect("metadata is an object")
                .contains_key("dimension"),
            "an abandon fact must name no dimension: {fact}"
        );
    }

    #[test]
    // This is the load-bearing test: the mutation in the contract's own
    // §7 deletes the abandon exclusion from the card-bound
    // `DispositionMetadata` loop's filter, which makes the abandon fact
    // recorded below attempt a `dimension`-shaped parse it was never going
    // to satisfy — refusing not just F-001's own projection but the whole
    // cycle's, which is exactly what `approve_card` below would trip over
    // for F-002, a card the abandon never touched.
    fn an_unrelated_card_in_the_same_cycle_still_works_after_an_abandon() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let abandon = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "ending the escalated card for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            abandon.status.success(),
            "the abandon that sets up this scenario must itself succeed: {}{}",
            String::from_utf8_lossy(&abandon.stdout),
            String::from_utf8_lossy(&abandon.stderr)
        );

        // A second, unrelated card in the same cycle, scoped away from
        // `src/**` so the two can coexist without an ownership-overlap
        // refusal, must be completely unaffected by F-001's abandon.
        // `card status` alone would only prove the read-only projection
        // path still works; `approve_card` drives F-002 through `handoff
        // create`, `review begin`, and `review record` — three separate
        // `require_convergence_budget` call sites — so this proves the
        // budget-gated write path stays open too.
        workspace.activate_card("F-002", &["docs/f002/**"]);
        let status = workspace.card_raw(&["status", "--card-id", "F-002"]);
        assert!(
            status.status.success(),
            "card status for an unrelated card must not be broken by another card's abandon: {}{}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        );

        workspace.approve_card("F-002", "docs/f002/a.md");

        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-002"])["data"]["state"],
            "approved",
            "an unrelated card in the same cycle must still deliver and be reviewed after another \
         card's abandon"
        );
    }

    #[test]
    fn a_second_abandon_of_the_same_card_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let first = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "first abandon",
                "--actor",
                "owner",
            ],
        );
        assert!(
            first.status.success(),
            "the first abandon must succeed: {}{}",
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );

        // The card's recorded facts do not disappear when it is abandoned.
        // `card status` publishes exactly `assess_card`'s own assessment
        // (see `card.rs`'s `card_convergence` doc comment, and
        // `card_status_reports_the_escalation_instead_of_refusing` above,
        // which pins that `data.convergence` is exactly `CardConvergence`'s
        // serialization) — so this is a direct, observed answer to whether
        // `assess_card` still reports the card `Escalated` post-abandon,
        // not an inference from the refusal below.
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"]["status"],
            "escalated",
            "an abandoned card's recorded facts do not disappear; it must still assess as \
         escalated"
        );

        let before_second_head = workspace.control_head();
        let second = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "second abandon, immediately",
                "--actor",
                "owner",
            ],
        );
        assert!(
            !second.status.success(),
            "a second abandon of an already-abandoned card must be refused"
        );
        assert_eq!(error_code(&second), "CH-POLICY-INVALID-TRANSITION");
        // It is check 3 (the lifecycle transition), not check 2 (the
        // escalation check), that refuses the repeat — the assertion above
        // already established the card still reads `Escalated`, so if
        // check 2 had fired instead the message would say the card is not
        // escalated, not name a transition. `CardState::check_transition`'s
        // own message names the exact states it refused to move between.
        assert!(
            error_message(&second).contains("cannot move from `abandoned` to `abandoned`"),
            "{}",
            error_message(&second)
        );
        assert_eq!(
            workspace.control_head(),
            before_second_head,
            "the control repository head must not move on refusal"
        );
        assert_eq!(
            disposition_recorded_events(&workspace).len(),
            1,
            "only the first abandon's fact may exist"
        );
    }

    #[test]
    fn a_card_that_is_not_escalated_cannot_be_abandoned_this_way() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        workspace.activate_card("F-001", &["src/**"]);

        let before_head = workspace.control_head();
        let output = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "nothing has escalated yet",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a card that has never been escalated must not be abandonable through this route"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        assert!(
            error_message(&output).contains("card abandon"),
            "the refusal must name the route that does apply: {}",
            error_message(&output)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "ready",
            "the refused abandon must not have moved the card"
        );
    }

    #[test]
    fn an_unauthorized_actor_cannot_abandon() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "an actor outside the configured set",
                "--actor",
                "intruder",
            ],
        );

        assert!(
            !output.status.success(),
            "an actor outside final_authorization_policy.authorizer_actor_ids must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "changes_requested",
            "the refused abandon must not have moved the card"
        );
    }

    #[test]
    fn an_unconfigured_authorization_policy_refuses() {
        // `opened_with_policy` (outside this module) installs a convergence
        // policy but no `final_authorization_policy` at all — the scenario
        // `opened_with_disposition_policies` above always avoids by
        // construction. Escalating a card only requires the convergence
        // policy; authorizing the abandon requires the other one, which
        // simply does not exist here.
        let workspace = opened_with_policy(1, 3);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "no authorization policy exists at all",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "an abandon must be refused when no final-authorization policy is configured"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_blank_rationale_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "   ",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a blank rationale must be refused before anything is written"
        );
        assert_eq!(error_code(&output), "CH-USAGE-INVALID-ARGUMENTS");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn the_dry_run_makes_every_check_and_writes_nothing() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        // For the success the real command would make.
        let before_head = workspace.control_head();
        let success_preview = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "would abandon if this were real",
                "--actor",
                "owner",
                "--dry-run",
            ],
        );
        assert!(
            success_preview.status.success(),
            "the dry run must report success when the real command would succeed: {}{}",
            String::from_utf8_lossy(&success_preview.stdout),
            String::from_utf8_lossy(&success_preview.stderr)
        );
        let envelope: serde_json::Value = serde_json::from_slice(&success_preview.stdout).unwrap();
        assert_eq!(
            envelope["data"]["dry_run"],
            serde_json::json!(true),
            "{envelope}"
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head"
        );
        assert!(
            disposition_recorded_events(&workspace).is_empty(),
            "a dry run must never write a fact"
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "changes_requested",
            "a dry run must never move the card out of its previous state"
        );

        // For at least one refusal — the same unauthorized-actor refusal
        // exercised for the real command above.
        let refusal_preview = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "would abandon if this were real",
                "--actor",
                "intruder",
                "--dry-run",
            ],
        );
        assert!(
            !refusal_preview.status.success(),
            "the dry run must refuse the same way the real command would"
        );
        assert_eq!(error_code(&refusal_preview), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head, including on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());

        // Neither dry run consumed anything: the real abandon, run
        // afterward, still succeeds exactly once.
        let real = disposition_abandon_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--rationale",
                "the real abandon",
                "--actor",
                "owner",
            ],
        );
        assert!(
            real.status.success(),
            "the real command must still succeed after both dry runs: {}{}",
            String::from_utf8_lossy(&real.stdout),
            String::from_utf8_lossy(&real.stderr)
        );
        assert_eq!(disposition_recorded_events(&workspace).len(), 1);
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "abandoned"
        );
    }
}

// 74-6: `disposition accept-risk`, run by an actor authorized under
// `final_authorization_policy.authorizer_actor_ids`, accepts a disclosed
// risk on one exhausted dimension of an escalated card so it can deliver
// and be reviewed again — without its budget being expanded. That
// distinction from `renew` is the entire point of this command: `renew`
// grants the configured limit again, so a further attempt still counts and
// can escalate the same dimension a second time; `accept-risk` grants
// nothing at all, so the count keeps climbing past the limit forever and
// the dimension simply stops being reported exhausted, because an
// authorized actor accepted that risk.
//
// Wrapped in its own module for the same reason `disposition_abandon` is:
// `the_dry_run_makes_every_check_and_writes_nothing` is already the name
// used above, three times, for the same property on three other commands,
// and none of the existing tests may be touched or renamed beyond what
// this card's own contract requires. A module gives this card's instance
// of that recurring name a distinct path
// (`disposition_accept_risk::the_dry_run_makes_every_check_and_writes_nothing`)
// without colliding.
mod disposition_accept_risk {
    use super::*;

    /// Identical in shape to every sibling module's own copy of this
    /// helper: `support::Workspace` is outside this card's file scope, so
    /// each disposition module keeps its fixture-building local rather
    /// than reaching into a sibling module's private helpers.
    fn initialized_with_authorizers(authorizers: &[&str]) -> Workspace {
        let workspace = Workspace::new();
        let mut args: Vec<String> = vec![
            "project".into(),
            "init".into(),
            "--project-id".into(),
            "example".into(),
            "--repository".into(),
            workspace.repository.display().to_string(),
            "--control".into(),
            workspace.control.display().to_string(),
            "--authority".into(),
            workspace.authority.display().to_string(),
            "--worktree-root".into(),
            workspace.worktrees.display().to_string(),
        ];
        for authorizer in authorizers {
            args.push("--final-authorizer-actor-id".into());
            args.push((*authorizer).to_owned());
        }
        let output = Workspace::run(&args);
        assert!(
            output.status.success(),
            "project init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        workspace.register_gate("gate.unit", &["true"]);
        workspace.register_gate("gate.all", &["true"]);
        workspace
    }

    /// Like [`opened_with_policy`], but with a final-authorization policy
    /// installed too, so `disposition accept-risk`'s authorization check
    /// (#6) has a configured set to resolve. Order matters exactly as it
    /// does in `opened_with_policy`: both policies must be in place before
    /// the cycle is created, which pins the project configuration's
    /// digest.
    fn opened_with_disposition_policies(
        card_limit: u32,
        integration_limit: u32,
        authorizers: &[&str],
    ) -> Workspace {
        let workspace = initialized_with_authorizers(authorizers);
        workspace.configure_convergence_policy(card_limit, integration_limit);
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

    /// Like [`opened_with_disposition_policies`], but lets each of the
    /// four card dimensions carry its own limit rather than one shared
    /// value. Needed only by `an_acceptance_covers_only_the_dimension_it_
    /// names`: that test needs two independent dimensions —
    /// `repair_attempts` and `gate_failures` — to reach their own limits
    /// from the very same handoff, since any gated command run after even
    /// one dimension is already exhausted is refused outright
    /// (`require_convergence_budget`, read by `handoff create`, `review
    /// begin`, and `review record` alike); `review_returns` must stay
    /// comfortably under its own limit so the review return that sets up
    /// the scenario does not exhaust the card before that handoff runs.
    /// Mirrors `Workspace::configure_convergence_policy`'s body directly,
    /// for the same reason `initialized_with_authorizers` mirrors
    /// `Workspace::initialized`'s: `support::Workspace` is outside this
    /// card's file scope.
    fn opened_with_disposition_policies_and_limits(
        authorizers: &[&str],
        review_returns: u32,
        repair_attempts: u32,
        gate_failures: u32,
        material_scope_revisions: u32,
        integration_limit: u32,
    ) -> Workspace {
        let workspace = initialized_with_authorizers(authorizers);
        let path = workspace.control.join("project/project.json");
        let raw = fs::read_to_string(&path).unwrap();
        let mut document: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let card_limits = serde_json::json!({
            "review_returns": review_returns,
            "repair_attempts": repair_attempts,
            "gate_failures": gate_failures,
            "material_scope_revisions": material_scope_revisions,
        });
        document["convergence_policy"] = serde_json::json!({
            "version": "harness.convergence-policy/v1",
            "card_limits": {
                "low": card_limits.clone(),
                "medium": card_limits.clone(),
                "high": card_limits.clone(),
                "critical": card_limits,
            },
            "cycle_limits": { "integration_failures": integration_limit },
        });
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&document).unwrap()),
        )
        .unwrap();
        support::git(&workspace.control, &["add", "-A"]);
        support::git(
            &workspace.control,
            &["commit", "-q", "-m", "test: configure convergence policy"],
        );
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

    /// Runs `disposition accept-risk` in JSON mode, returning the raw
    /// output. Mirrors every other per-group `_raw` helper in this file;
    /// kept local because `support::Workspace` is outside this card's file
    /// scope.
    fn disposition_accept_risk_raw(workspace: &Workspace, args: &[&str]) -> std::process::Output {
        let mut full = vec![
            "disposition".to_owned(),
            "accept-risk".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
        ];
        full.extend(args.iter().map(|arg| (*arg).to_owned()));
        Workspace::run(&full)
    }

    /// Every recorded `convergence.disposition_recorded` fact. Every fact
    /// this module's fixtures ever record is an acceptance (none
    /// configures a renewal, rebaseline, or abandon too), so this is never
    /// filtered further by `metadata.disposition`.
    fn disposition_recorded_events(workspace: &Workspace) -> Vec<serde_json::Value> {
        workspace
            .events()
            .into_iter()
            .filter(|event| event["event_type"] == "convergence.disposition_recorded")
            .collect()
    }

    /// Like the top-level `open_review_round`, but activates the card with
    /// `gate.unit` declared as a feature gate — needed because
    /// `redeliver_after_return_declaring_a_gate_failure`, below, must
    /// declare a gate failure for that gate at handoff time, and
    /// `validate_declared_gate_failures` refuses any `gate_id` absent from
    /// the card's own declared feature gates. `support::Workspace` is
    /// outside this card's file scope, so this mirrors
    /// `open_review_round`'s body directly rather than editing it.
    fn open_review_round_with_gate_unit(workspace: &Workspace, card_id: &str) -> String {
        workspace.activate_card_with_gates(card_id, &["src/**"], &["gate.unit"]);
        workspace.work(&["start", "--card-id", card_id]);

        let worktree = workspace.worktrees.join(card_id);
        fs::create_dir_all(worktree.join("src")).unwrap();
        fs::write(worktree.join("src/a.rs"), "fn main() {}\n").unwrap();
        support::git(&worktree, &["add", "-A"]);
        support::git(&worktree, &["commit", "-q", "-m", "feat: implement"]);
        workspace.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

        let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
        let declaration = workspace.root.join(format!("{card_id}-declaration.yaml"));
        fs::write(
            &declaration,
            format!(
                "delivered_sha: {head}\nbehavior_delivered: it works\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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
        head
    }

    /// Like the top-level `redeliver_after_return`, but also declares a
    /// gate failure on the very same redelivery — needed only by
    /// `an_acceptance_covers_only_the_dimension_it_names`, which needs a
    /// second, independent dimension to reach its own limit in the same
    /// handoff that answers the return: any gated command run afterward,
    /// once either dimension is exhausted, is refused outright. Mirrors
    /// `redeliver_after_return`'s body directly, using
    /// `open_review_round_with_gate_unit` above in place of the top-level
    /// `open_review_round`.
    fn redeliver_after_return_declaring_a_gate_failure(
        workspace: &Workspace,
        card_id: &str,
        verdict_body: &str,
        gate_failures_yaml: &str,
    ) -> std::process::Output {
        open_review_round_with_gate_unit(workspace, card_id);
        let verdict_path = write_verdict(workspace, card_id, verdict_body);
        workspace.review(&[
            "record",
            "--card-id",
            card_id,
            "--verdict",
            &verdict_path,
            "--actor",
            "reviewer",
        ]);
        workspace.work(&["resume", "--card-id", card_id]);

        let worktree = workspace.worktrees.join(card_id);
        fs::write(worktree.join("src/a.rs"), "fn main() { /* fixed */ }\n").unwrap();
        support::git(&worktree, &["add", "-A"]);
        support::git(&worktree, &["commit", "-q", "-m", "fix: address review"]);
        workspace.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

        let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
        let declaration =
            declaration_with_gate_failures(workspace, card_id, &head, gate_failures_yaml);
        workspace.handoff_raw(&[
            "create",
            "--card-id",
            card_id,
            "--declaration",
            &declaration,
        ])
    }

    #[test]
    fn an_authorized_acceptance_lets_an_escalated_card_deliver_again() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let card_status = workspace.card_json(&["status", "--card-id", "F-001"]);
        let base = workspace.authority_head();
        let pre_accept_head = workspace.control_head();

        let accept = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "reviewer has seen this exact defect before and judges it low-impact",
                "--rationale",
                "authorized acceptance for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            accept.status.success(),
            "an authorized acceptance of an exhausted dimension must succeed: {}{}",
            String::from_utf8_lossy(&accept.stdout),
            String::from_utf8_lossy(&accept.stderr)
        );

        // 79-2's lesson, restated by the contract for this card: an event
        // written but not committed is invisible by content alone, because
        // the very next transaction would stage the whole control tree and
        // sweep it in regardless. The only way to catch "wrote but did not
        // commit" is to check, right here, that this command's own commit
        // is what moved the head and left the tree clean — before anything
        // else touches the control repository.
        assert_ne!(
            workspace.control_head(),
            pre_accept_head,
            "disposition accept-risk must commit its own write; the control head must move"
        );
        assert!(
            support::capture(&workspace.control, &["status", "--porcelain"]).is_empty(),
            "the control tree must be porcelain-clean immediately after a successful acceptance"
        );

        // Unlike `abandon`, an acceptance moves no card state at all: the
        // card simply becomes deliverable again where it stands.
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "changes_requested",
            "an acceptance must not move the card's lifecycle state"
        );

        let dispositions = disposition_recorded_events(&workspace);
        assert_eq!(
            dispositions.len(),
            1,
            "exactly one disposition fact must be recorded: {dispositions:?}"
        );
        let fact = &dispositions[0];
        assert_eq!(
            fact["event_type"], "convergence.disposition_recorded",
            "{fact}"
        );
        assert_eq!(fact["actor_id"], "owner", "{fact}");
        assert_eq!(fact["cycle_id"], "C-001", "{fact}");
        assert_eq!(fact["card_id"], "F-001", "{fact}");
        assert_eq!(
            fact["card_revision"], card_status["data"]["revision"],
            "{fact}"
        );
        assert_eq!(fact["card_digest"], card_status["data"]["digest"], "{fact}");
        assert_eq!(
            fact["head_sha"], base,
            "head must bind to the current revision's own base_sha, the one exact SHA a card is \
         guaranteed to carry in any state: {fact}"
        );
        // No transition at all, unlike `abandon`'s own fact: an acceptance
        // moves no card state, so neither field is ever set.
        assert!(fact["previous_state"].is_null(), "{fact}");
        assert!(fact["next_state"].is_null(), "{fact}");
        assert_eq!(fact["metadata"]["disposition"], "accept_risk", "{fact}");
        assert_eq!(fact["metadata"]["dimension"], "review_returns", "{fact}");
        assert_eq!(
            fact["metadata"]["risk"],
            "reviewer has seen this exact defect before and judges it low-impact",
            "{fact}"
        );
        assert_eq!(
            fact["metadata"]["rationale"], "authorized acceptance for testing",
            "{fact}"
        );
        assert_eq!(fact["metadata"]["authorized_by"], "owner", "{fact}");
        assert_real_policy_digest(fact);

        // And the card really is deliverable again: `card status` reports
        // `within`, not `escalated`, even though the review-return count
        // that exhausted it is still 1.
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"],
            serde_json::json!({ "status": "within" }),
            "an accepted risk must make the card deliverable again"
        );
    }

    #[test]
    // The discriminating test (contract §6.2): confirms `accept-risk`
    // behaves nothing like `renew`. Under a renew-shaped implementation —
    // incrementing `renewals` instead of setting `escalation_waived` — the
    // effective budget here would become 2 and the second attempt below
    // would escalate the card again; this asserts it does not.
    //
    // `repair_attempts` is deliberately given more room than
    // `review_returns`: redelivering after `escalate_via_review_returns`
    // answers that still-open, blocking review return, so it records a
    // `repair_attempt` fact of its own (see `a_delivery_answering_a_
    // review_return_records_one_repair_attempt_inheriting_its_reason`,
    // outside this module). A uniform limit-1 policy would let that
    // incidental fact exhaust `repair_attempts` too, refusing the very
    // `review begin` this test needs in order to record the *second*
    // `review_returns` attempt — a confusing failure for a reason
    // unrelated to what this test exists to prove.
    fn an_acceptance_grants_no_further_budget() {
        let workspace = opened_with_disposition_policies_and_limits(&["owner"], 1, 3, 3, 3, 3);
        escalate_via_review_returns(&workspace, "F-001");

        let accept = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "a second review return in this dimension is expected and accepted",
                "--rationale",
                "authorized acceptance for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            accept.status.success(),
            "the acceptance that sets up this scenario must itself succeed: {}{}",
            String::from_utf8_lossy(&accept.stdout),
            String::from_utf8_lossy(&accept.stderr)
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"],
            serde_json::json!({ "status": "within" }),
            "the card must be within budget immediately after the acceptance"
        );

        // Record a second attempt in the very same dimension the risk was
        // accepted on.
        let head = redeliver_candidate(&workspace, "F-001");
        let declaration = declaration_with_gate_failures(&workspace, "F-001", &head, "");
        let handoff = workspace.handoff_raw(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);
        assert!(
            handoff.status.success(),
            "delivery after acceptance must succeed: {}{}",
            String::from_utf8_lossy(&handoff.stdout),
            String::from_utf8_lossy(&handoff.stderr)
        );
        let begin = workspace.review_raw(&["begin", "--card-id", "F-001", "--actor", "reviewer"]);
        assert!(
            begin.status.success(),
            "review begin after acceptance must succeed: {}{}",
            String::from_utf8_lossy(&begin.stdout),
            String::from_utf8_lossy(&begin.stderr)
        );
        // Unlike `escalate_via_review_returns`'s own first-round verdict,
        // this one must also carry forward that first round's finding at
        // `src/a.rs` as `resolved` — a re-review may not silently drop an
        // earlier round's open finding (see `review_round`'s own doc
        // comment, outside this module) — alongside the new finding that
        // triggers this second return.
        let second_return_verdict = "reviewer_actor_id: reviewer\ndecision: changes_requested\nreason_category: acceptance_defect\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: carried forward from the previous round\n    disposition: resolved\n  - severity: medium\n    location: src/a.rs\n    detail: a second defect found on re-review\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
        let verdict = write_verdict(&workspace, "F-001", second_return_verdict);
        let record = workspace.review_raw(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &verdict,
            "--actor",
            "reviewer",
        ]);
        assert!(
            record.status.success(),
            "a second review return in the accepted dimension must still be permitted to \
         record: {}{}",
            String::from_utf8_lossy(&record.stdout),
            String::from_utf8_lossy(&record.stderr)
        );

        // The central assertion: a renew-shaped implementation would read
        // an effective budget of 2 here and escalate again on the second
        // attempt. Acceptance grants nothing, so the card must still read
        // `within`.
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"],
            serde_json::json!({ "status": "within" }),
            "acceptance grants no budget: a second attempt in the same dimension must not \
         escalate the card again"
        );
        // The risk is accepted, not erased: the count must really have
        // reached 2. A projection that silently stopped counting once a
        // risk was accepted would make the assertion above pass for the
        // wrong reason.
        assert_eq!(
            attempt_facts_of_kind(&workspace, "review_return").len(),
            2,
            "the second review return must still be recorded as a real attempt"
        );
    }

    #[test]
    fn an_acceptance_covers_only_the_dimension_it_names() {
        let workspace = opened_with_disposition_policies_and_limits(&["owner"], 5, 1, 1, 5, 3);

        let handoff = redeliver_after_return_declaring_a_gate_failure(
            &workspace,
            "F-001",
            RETURN_WITH_REGRESSION_REASON_FOR_HANDOFF,
            "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
        );
        assert!(
            handoff.status.success(),
            "the redelivery that exhausts both repair_attempts and gate_failures at once must \
         itself succeed: {}{}",
            String::from_utf8_lossy(&handoff.stdout),
            String::from_utf8_lossy(&handoff.stderr)
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"]["status"],
            "escalated",
            "both repair_attempts and gate_failures must be exhausted by the redelivery above"
        );

        let accept = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "repair-attempts",
                "--risk",
                "the repeated repair attempt on this exact defect is accepted",
                "--rationale",
                "authorized acceptance for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            accept.status.success(),
            "accepting repair_attempts must succeed while it is exhausted: {}{}",
            String::from_utf8_lossy(&accept.stdout),
            String::from_utf8_lossy(&accept.stderr)
        );

        // gate_failures was never named: a second, independently exhausted
        // dimension on the same card must still escalate it.
        let convergence =
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"].clone();
        assert_eq!(
            convergence["status"], "escalated",
            "gate_failures is still exhausted; the acceptance named only repair_attempts: \
         {convergence}"
        );
        let exhausted = convergence["exhausted"]
            .as_array()
            .expect("exhausted is an array");
        assert_eq!(
            exhausted.len(),
            1,
            "only one dimension may remain exhausted; repair_attempts was accepted: {exhausted:?}"
        );
        assert_eq!(exhausted[0]["dimension"], "gate_failures", "{exhausted:?}");
        assert_eq!(exhausted[0]["count"], 1, "{exhausted:?}");
        assert_eq!(exhausted[0]["limit"], 1, "{exhausted:?}");
    }

    #[test]
    fn a_second_acceptance_of_the_same_dimension_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let first = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "first accepted risk",
                "--rationale",
                "first acceptance",
                "--actor",
                "owner",
            ],
        );
        assert!(
            first.status.success(),
            "the first acceptance must succeed: {}{}",
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );

        let before_second_head = workspace.control_head();
        let second = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "second accepted risk, immediately",
                "--rationale",
                "second acceptance",
                "--actor",
                "owner",
            ],
        );
        assert!(
            !second.status.success(),
            "a second acceptance of an already-accepted dimension must be refused"
        );
        assert_eq!(error_code(&second), "CH-POLICY-INVALID-TRANSITION");
        assert!(
            error_message(&second).contains("already"),
            "the refusal must name the earlier acceptance: {}",
            error_message(&second)
        );
        assert_eq!(
            workspace.control_head(),
            before_second_head,
            "the control repository head must not move on refusal"
        );
        assert_eq!(
            disposition_recorded_events(&workspace).len(),
            1,
            "only the first acceptance's fact may exist"
        );
    }

    #[test]
    fn a_dimension_that_still_has_budget_cannot_be_pre_accepted() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "gate-failures",
                "--risk",
                "a risk on a dimension that still has budget",
                "--rationale",
                "the wrong dimension",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "gate-failures still has budget; pre-accepting its risk would be silent expansion"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        let message = error_message(&output);
        assert!(
            message.contains("review_returns"),
            "the refusal must name the dimension that really is exhausted: {message}"
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn an_unauthorized_actor_cannot_accept_risk() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "an actor outside the configured set",
                "--rationale",
                "an actor outside the configured set",
                "--actor",
                "intruder",
            ],
        );

        assert!(
            !output.status.success(),
            "an actor outside final_authorization_policy.authorizer_actor_ids must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "changes_requested",
            "the refused acceptance must not have moved the card"
        );
    }

    #[test]
    fn an_unconfigured_authorization_policy_refuses() {
        // `opened_with_policy` (outside this module) installs a
        // convergence policy but no `final_authorization_policy` at all —
        // the scenario `opened_with_disposition_policies` above always
        // avoids by construction. Escalating a card only requires the
        // convergence policy; authorizing the acceptance requires the
        // other one, which simply does not exist here.
        let workspace = opened_with_policy(1, 3);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "no authorization policy exists at all",
                "--rationale",
                "no authorization policy exists at all",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "an acceptance must be refused when no final-authorization policy is configured"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_blank_risk_or_rationale_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let blank_risk = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "   ",
                "--rationale",
                "a valid rationale",
                "--actor",
                "owner",
            ],
        );
        assert!(
            !blank_risk.status.success(),
            "a blank risk must be refused before anything is written"
        );
        assert_eq!(error_code(&blank_risk), "CH-USAGE-INVALID-ARGUMENTS");
        let risk_message = error_message(&blank_risk);
        assert!(risk_message.contains("--risk"), "{risk_message}");

        let blank_rationale = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "a valid risk disclosure",
                "--rationale",
                "   ",
                "--actor",
                "owner",
            ],
        );
        assert!(
            !blank_rationale.status.success(),
            "a blank rationale must be refused before anything is written"
        );
        assert_eq!(error_code(&blank_rationale), "CH-USAGE-INVALID-ARGUMENTS");
        let rationale_message = error_message(&blank_rationale);
        assert!(
            rationale_message.contains("--rationale"),
            "{rationale_message}"
        );

        // Distinct messages, on purpose: an operator who left one blank
        // should not have to guess which.
        assert_ne!(
            risk_message, rationale_message,
            "a blank --risk and a blank --rationale must be refused with distinct messages"
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on either refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn the_dry_run_makes_every_check_and_writes_nothing() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        // For the success the real command would make.
        let before_head = workspace.control_head();
        let success_preview = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "would accept if this were real",
                "--rationale",
                "would accept if this were real",
                "--actor",
                "owner",
                "--dry-run",
            ],
        );
        assert!(
            success_preview.status.success(),
            "the dry run must report success when the real command would succeed: {}{}",
            String::from_utf8_lossy(&success_preview.stdout),
            String::from_utf8_lossy(&success_preview.stderr)
        );
        let envelope: serde_json::Value = serde_json::from_slice(&success_preview.stdout).unwrap();
        assert_eq!(
            envelope["data"]["dry_run"],
            serde_json::json!(true),
            "{envelope}"
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head"
        );
        assert!(
            disposition_recorded_events(&workspace).is_empty(),
            "a dry run must never write a fact"
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "changes_requested",
            "a dry run must never move the card out of its previous state"
        );

        // For at least one refusal — the same unauthorized-actor refusal
        // exercised for the real command above.
        let refusal_preview = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "would accept if this were real",
                "--rationale",
                "would accept if this were real",
                "--actor",
                "intruder",
                "--dry-run",
            ],
        );
        assert!(
            !refusal_preview.status.success(),
            "the dry run must refuse the same way the real command would"
        );
        assert_eq!(error_code(&refusal_preview), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head, including on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());

        // Neither dry run consumed anything: the real acceptance, run
        // afterward, still succeeds exactly once.
        let real = disposition_accept_risk_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--risk",
                "the real risk being accepted",
                "--rationale",
                "the real acceptance",
                "--actor",
                "owner",
            ],
        );
        assert!(
            real.status.success(),
            "the real command must still succeed after both dry runs: {}{}",
            String::from_utf8_lossy(&real.stdout),
            String::from_utf8_lossy(&real.stderr)
        );
        assert_eq!(disposition_recorded_events(&workspace).len(), 1);
    }
}

// 74-7: `disposition split`, run by an actor authorized under
// `final_authorization_policy.authorizer_actor_ids`, moves the remaining
// work behind one exhausted dimension of an escalated card to an
// already-existing follow-up card, so the original card can deliver and be
// reviewed again — without its budget being expanded. Like `accept-risk`,
// it grants no budget: the count keeps climbing past the limit forever and
// the dimension simply stops being reported exhausted, because the
// authorized actor moved the remaining work elsewhere. Unlike
// `accept-risk`, it names exactly where that work went —
// `follow_up_card_id` — and unlike every sibling disposition, it names a
// *second* card, one it never creates and never mutates: the follow-up
// card must already exist, reached through the normal governed path, and
// `split` only records a binding to it.
//
// Wrapped in its own module for the same reason `disposition_abandon` and
// `disposition_accept_risk` are: `the_dry_run_makes_every_check_and_writes_
// nothing` is already the name used above, four times, for the same
// property on four other commands, and none of the existing tests may be
// touched or renamed beyond what this card's own contract requires. A
// module gives this card's instance of that recurring name a distinct path
// (`disposition_split::the_dry_run_makes_every_check_and_writes_nothing`)
// without colliding.
mod disposition_split {
    use super::*;

    /// Identical in shape to every sibling module's own copy of this
    /// helper: `support::Workspace` is outside this card's file scope, so
    /// each disposition module keeps its fixture-building local rather
    /// than reaching into a sibling module's private helpers.
    fn initialized_with_authorizers(authorizers: &[&str]) -> Workspace {
        let workspace = Workspace::new();
        let mut args: Vec<String> = vec![
            "project".into(),
            "init".into(),
            "--project-id".into(),
            "example".into(),
            "--repository".into(),
            workspace.repository.display().to_string(),
            "--control".into(),
            workspace.control.display().to_string(),
            "--authority".into(),
            workspace.authority.display().to_string(),
            "--worktree-root".into(),
            workspace.worktrees.display().to_string(),
        ];
        for authorizer in authorizers {
            args.push("--final-authorizer-actor-id".into());
            args.push((*authorizer).to_owned());
        }
        let output = Workspace::run(&args);
        assert!(
            output.status.success(),
            "project init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        workspace.register_gate("gate.unit", &["true"]);
        workspace.register_gate("gate.all", &["true"]);
        workspace
    }

    /// Like [`opened_with_policy`], but with a final-authorization policy
    /// installed too, so `disposition split`'s authorization check (#10)
    /// has a configured set to resolve. Order matters exactly as it does in
    /// `opened_with_policy`: both policies must be in place before the
    /// cycle is created, which pins the project configuration's digest.
    fn opened_with_disposition_policies(
        card_limit: u32,
        integration_limit: u32,
        authorizers: &[&str],
    ) -> Workspace {
        let workspace = initialized_with_authorizers(authorizers);
        workspace.configure_convergence_policy(card_limit, integration_limit);
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

    /// Like [`opened_with_disposition_policies`], but lets each of the
    /// four card dimensions carry its own limit rather than one shared
    /// value. Needed only by `a_split_grants_no_further_budget` and
    /// `a_split_covers_only_the_dimension_it_names`, for exactly the
    /// reason 74-6's own copy of this helper is needed by its siblings of
    /// the same name: `repair_attempts` needs more room than
    /// `review_returns` because redelivering after
    /// `escalate_via_review_returns` answers that still-open, blocking
    /// review return, so it records a `repair_attempt` fact of its own —
    /// see 74-6's copy, in `disposition_accept_risk`, for the full
    /// explanation. Mirrors `Workspace::configure_convergence_policy`'s
    /// body directly, for the same reason `initialized_with_authorizers`
    /// mirrors `Workspace::initialized`'s: `support::Workspace` is outside
    /// this card's file scope.
    fn opened_with_disposition_policies_and_limits(
        authorizers: &[&str],
        review_returns: u32,
        repair_attempts: u32,
        gate_failures: u32,
        material_scope_revisions: u32,
        integration_limit: u32,
    ) -> Workspace {
        let workspace = initialized_with_authorizers(authorizers);
        let path = workspace.control.join("project/project.json");
        let raw = fs::read_to_string(&path).unwrap();
        let mut document: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let card_limits = serde_json::json!({
            "review_returns": review_returns,
            "repair_attempts": repair_attempts,
            "gate_failures": gate_failures,
            "material_scope_revisions": material_scope_revisions,
        });
        document["convergence_policy"] = serde_json::json!({
            "version": "harness.convergence-policy/v1",
            "card_limits": {
                "low": card_limits.clone(),
                "medium": card_limits.clone(),
                "high": card_limits.clone(),
                "critical": card_limits,
            },
            "cycle_limits": { "integration_failures": integration_limit },
        });
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&document).unwrap()),
        )
        .unwrap();
        support::git(&workspace.control, &["add", "-A"]);
        support::git(
            &workspace.control,
            &["commit", "-q", "-m", "test: configure convergence policy"],
        );
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

    /// Runs `disposition split` in JSON mode, returning the raw output.
    /// Mirrors every other per-group `_raw` helper in this file; kept local
    /// because `support::Workspace` is outside this card's file scope.
    fn disposition_split_raw(workspace: &Workspace, args: &[&str]) -> std::process::Output {
        let mut full = vec![
            "disposition".to_owned(),
            "split".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
        ];
        full.extend(args.iter().map(|arg| (*arg).to_owned()));
        Workspace::run(&full)
    }

    /// Every recorded `convergence.disposition_recorded` fact. Every fact
    /// this module's fixtures ever record is a split (none configures a
    /// renewal, rebaseline, abandon, or acceptance too), so this is never
    /// filtered further by `metadata.disposition`.
    fn disposition_recorded_events(workspace: &Workspace) -> Vec<serde_json::Value> {
        workspace
            .events()
            .into_iter()
            .filter(|event| event["event_type"] == "convergence.disposition_recorded")
            .collect()
    }

    /// Creates and activates a card in an arbitrary cycle, not necessarily
    /// `C-001`. Needed only by `a_follow_up_card_in_another_cycle_refuses`:
    /// `support::Workspace::activate_card`, and every other
    /// `activate_card_*` helper it offers, hard-codes `cycle_id: C-001`,
    /// which that test's follow-up card cannot use since the whole point
    /// is putting it in a different cycle. Mirrors
    /// `activate_card_with_gates`'s body directly, for the same reason
    /// `initialized_with_authorizers` mirrors `Workspace::initialized`'s:
    /// `support::Workspace` is outside this card's file scope.
    fn activate_card_in_cycle(
        workspace: &Workspace,
        card_id: &str,
        cycle_id: &str,
        include: &[&str],
    ) {
        let list = include
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let base =
            workspace.cycle_json(&["status", "--cycle-id", cycle_id])["data"]["baseline_sha"]
                .as_str()
                .expect("cycle has a frozen baseline")
                .to_owned();
        let body = format!(
            "card_id: {card_id}\ncycle_id: {cycle_id}\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{list}]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        );
        let path = workspace.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        workspace.card(&["create", "--draft", &path.display().to_string()]);
        workspace.card(&["activate", "--card-id", card_id]);
    }

    /// Like the top-level `open_review_round`, but activates the card with
    /// `gate.unit` declared as a feature gate. Identical to 74-6's own copy
    /// of this helper, in `disposition_accept_risk` — needed here for the
    /// same reason: `an_split_covers_only_the_dimension_it_names` below
    /// must declare a gate failure for that gate at handoff time, and
    /// `validate_declared_gate_failures` refuses any `gate_id` absent from
    /// the card's own declared feature gates. `support::Workspace` is
    /// outside this card's file scope, so this mirrors
    /// `open_review_round`'s body directly rather than editing it, the
    /// same reason 74-6's copy does.
    fn open_review_round_with_gate_unit(workspace: &Workspace, card_id: &str) -> String {
        workspace.activate_card_with_gates(card_id, &["src/**"], &["gate.unit"]);
        workspace.work(&["start", "--card-id", card_id]);

        let worktree = workspace.worktrees.join(card_id);
        fs::create_dir_all(worktree.join("src")).unwrap();
        fs::write(worktree.join("src/a.rs"), "fn main() {}\n").unwrap();
        support::git(&worktree, &["add", "-A"]);
        support::git(&worktree, &["commit", "-q", "-m", "feat: implement"]);
        workspace.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

        let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
        let declaration = workspace.root.join(format!("{card_id}-declaration.yaml"));
        fs::write(
            &declaration,
            format!(
                "delivered_sha: {head}\nbehavior_delivered: it works\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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
        head
    }

    /// Like the top-level `redeliver_after_return`, but also declares a
    /// gate failure on the very same redelivery. Identical in shape to
    /// 74-6's own copy of this helper, in `disposition_accept_risk`, needed
    /// here for the same reason: `a_split_covers_only_the_dimension_it_
    /// names` needs two independent dimensions — `repair_attempts` and
    /// `gate_failures` — to reach their own limits from the very same
    /// handoff, since any gated command run after even one dimension is
    /// already exhausted is refused outright. Mirrors
    /// `redeliver_after_return`'s body directly, using
    /// `open_review_round_with_gate_unit` above in place of the top-level
    /// `open_review_round`.
    fn redeliver_after_return_declaring_a_gate_failure(
        workspace: &Workspace,
        card_id: &str,
        verdict_body: &str,
        gate_failures_yaml: &str,
    ) -> std::process::Output {
        open_review_round_with_gate_unit(workspace, card_id);
        let verdict_path = write_verdict(workspace, card_id, verdict_body);
        workspace.review(&[
            "record",
            "--card-id",
            card_id,
            "--verdict",
            &verdict_path,
            "--actor",
            "reviewer",
        ]);
        workspace.work(&["resume", "--card-id", card_id]);

        let worktree = workspace.worktrees.join(card_id);
        fs::write(worktree.join("src/a.rs"), "fn main() { /* fixed */ }\n").unwrap();
        support::git(&worktree, &["add", "-A"]);
        support::git(&worktree, &["commit", "-q", "-m", "fix: address review"]);
        workspace.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

        let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
        let declaration =
            declaration_with_gate_failures(workspace, card_id, &head, gate_failures_yaml);
        workspace.handoff_raw(&[
            "create",
            "--card-id",
            card_id,
            "--declaration",
            &declaration,
        ])
    }

    #[test]
    fn an_authorized_split_lets_an_escalated_card_deliver_again() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let card_status = workspace.card_json(&["status", "--card-id", "F-001"]);
        let follow_up_status_before = workspace.card_json(&["status", "--card-id", "F-002"]);
        let base = workspace.authority_head();
        let pre_split_head = workspace.control_head();

        let split = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "authorized split for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            split.status.success(),
            "an authorized split of an exhausted dimension must succeed: {}{}",
            String::from_utf8_lossy(&split.stdout),
            String::from_utf8_lossy(&split.stderr)
        );

        // 79-2's lesson, restated by the contract for this card: an event
        // written but not committed is invisible by content alone, because
        // the very next transaction would stage the whole control tree and
        // sweep it in regardless. The only way to catch "wrote but did not
        // commit" is to check, right here, that this command's own commit
        // is what moved the head and left the tree clean — before anything
        // else touches the control repository.
        assert_ne!(
            workspace.control_head(),
            pre_split_head,
            "disposition split must commit its own write; the control head must move"
        );
        assert!(
            support::capture(&workspace.control, &["status", "--porcelain"]).is_empty(),
            "the control tree must be porcelain-clean immediately after a successful split"
        );

        // Neither card's lifecycle state moves: the original becomes
        // deliverable again where it stands, and the follow-up card is
        // named in the record, never mutated.
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "changes_requested",
            "a split must not move the original card's lifecycle state"
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-002"])["data"]["state"],
            follow_up_status_before["data"]["state"],
            "a split must not move the follow-up card's lifecycle state either"
        );

        let dispositions = disposition_recorded_events(&workspace);
        assert_eq!(
            dispositions.len(),
            1,
            "exactly one disposition fact must be recorded: {dispositions:?}"
        );
        let fact = &dispositions[0];
        assert_eq!(
            fact["event_type"], "convergence.disposition_recorded",
            "{fact}"
        );
        assert_eq!(fact["actor_id"], "owner", "{fact}");
        assert_eq!(fact["cycle_id"], "C-001", "{fact}");
        assert_eq!(fact["card_id"], "F-001", "{fact}");
        assert_eq!(
            fact["card_revision"], card_status["data"]["revision"],
            "{fact}"
        );
        assert_eq!(fact["card_digest"], card_status["data"]["digest"], "{fact}");
        assert_eq!(
            fact["head_sha"], base,
            "head must bind to the current revision's own base_sha, the one exact SHA a card is \
         guaranteed to carry in any state: {fact}"
        );
        // No transition at all, unlike `abandon`'s own fact: a split moves
        // no card state, so neither field is ever set.
        assert!(fact["previous_state"].is_null(), "{fact}");
        assert!(fact["next_state"].is_null(), "{fact}");
        assert_eq!(fact["metadata"]["disposition"], "split", "{fact}");
        assert_eq!(fact["metadata"]["dimension"], "review_returns", "{fact}");
        assert_eq!(fact["metadata"]["follow_up_card_id"], "F-002", "{fact}");
        assert_eq!(
            fact["metadata"]["rationale"], "authorized split for testing",
            "{fact}"
        );
        assert_eq!(fact["metadata"]["authorized_by"], "owner", "{fact}");
        assert_real_policy_digest(fact);

        // And the card really is deliverable again: `card status` reports
        // `within`, not `escalated`, even though the review-return count
        // that exhausted it is still 1.
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"],
            serde_json::json!({ "status": "within" }),
            "a split must make the card deliverable again"
        );
    }

    #[test]
    // The discriminating test (contract §7 item 2): confirms `split`
    // behaves nothing like `renew`. Under a renew-shaped implementation —
    // incrementing `renewals` instead of setting `escalation_waived` — the
    // effective budget here would become 2 and the second attempt below
    // would escalate the card again; this asserts it does not.
    fn a_split_grants_no_further_budget() {
        let workspace = opened_with_disposition_policies_and_limits(&["owner"], 1, 3, 3, 3, 3);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let split = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "authorized split for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            split.status.success(),
            "the split that sets up this scenario must itself succeed: {}{}",
            String::from_utf8_lossy(&split.stdout),
            String::from_utf8_lossy(&split.stderr)
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"],
            serde_json::json!({ "status": "within" }),
            "the card must be within budget immediately after the split"
        );

        // Record a second attempt in the very same dimension the split
        // named.
        let head = redeliver_candidate(&workspace, "F-001");
        let declaration = declaration_with_gate_failures(&workspace, "F-001", &head, "");
        let handoff = workspace.handoff_raw(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);
        assert!(
            handoff.status.success(),
            "delivery after split must succeed: {}{}",
            String::from_utf8_lossy(&handoff.stdout),
            String::from_utf8_lossy(&handoff.stderr)
        );
        let begin = workspace.review_raw(&["begin", "--card-id", "F-001", "--actor", "reviewer"]);
        assert!(
            begin.status.success(),
            "review begin after split must succeed: {}{}",
            String::from_utf8_lossy(&begin.stdout),
            String::from_utf8_lossy(&begin.stderr)
        );
        // Unlike `escalate_via_review_returns`'s own first-round verdict,
        // this one must also carry forward that first round's finding at
        // `src/a.rs` as `resolved` — a re-review may not silently drop an
        // earlier round's open finding.
        let second_return_verdict = "reviewer_actor_id: reviewer\ndecision: changes_requested\nreason_category: acceptance_defect\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: carried forward from the previous round\n    disposition: resolved\n  - severity: medium\n    location: src/a.rs\n    detail: a second defect found on re-review\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\nreview_conduct: separate_process\n";
        let verdict = write_verdict(&workspace, "F-001", second_return_verdict);
        let record = workspace.review_raw(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &verdict,
            "--actor",
            "reviewer",
        ]);
        assert!(
            record.status.success(),
            "a second review return in the split dimension must still be permitted to record: \
         {}{}",
            String::from_utf8_lossy(&record.stdout),
            String::from_utf8_lossy(&record.stderr)
        );

        // The central assertion: a renew-shaped implementation would read
        // an effective budget of 2 here and escalate again on the second
        // attempt. Split grants nothing, so the card must still read
        // `within`.
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"],
            serde_json::json!({ "status": "within" }),
            "split grants no budget: a second attempt in the same dimension must not escalate \
         the card again"
        );
        // The dimension is split off, not erased: the count must really
        // have reached 2. A projection that silently stopped counting once
        // a dimension was split would make the assertion above pass for
        // the wrong reason.
        assert_eq!(
            attempt_facts_of_kind(&workspace, "review_return").len(),
            2,
            "the second review return must still be recorded as a real attempt"
        );
    }

    #[test]
    fn a_split_covers_only_the_dimension_it_names() {
        let workspace = opened_with_disposition_policies_and_limits(&["owner"], 5, 1, 1, 5, 3);
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let handoff = redeliver_after_return_declaring_a_gate_failure(
            &workspace,
            "F-001",
            RETURN_WITH_REGRESSION_REASON_FOR_HANDOFF,
            "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
        );
        assert!(
            handoff.status.success(),
            "the redelivery that exhausts both repair_attempts and gate_failures at once must \
         itself succeed: {}{}",
            String::from_utf8_lossy(&handoff.stdout),
            String::from_utf8_lossy(&handoff.stderr)
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"]["status"],
            "escalated",
            "both repair_attempts and gate_failures must be exhausted by the redelivery above"
        );

        let split = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "repair-attempts",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "authorized split for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            split.status.success(),
            "splitting repair_attempts must succeed while it is exhausted: {}{}",
            String::from_utf8_lossy(&split.stdout),
            String::from_utf8_lossy(&split.stderr)
        );

        // gate_failures was never named: a second, independently exhausted
        // dimension on the same card must still escalate it.
        let convergence =
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"].clone();
        assert_eq!(
            convergence["status"], "escalated",
            "gate_failures is still exhausted; the split named only repair_attempts: {convergence}"
        );
        let exhausted = convergence["exhausted"]
            .as_array()
            .expect("exhausted is an array");
        assert_eq!(
            exhausted.len(),
            1,
            "only one dimension may remain exhausted; repair_attempts was split off: {exhausted:?}"
        );
        assert_eq!(exhausted[0]["dimension"], "gate_failures", "{exhausted:?}");
        assert_eq!(exhausted[0]["count"], 1, "{exhausted:?}");
        assert_eq!(exhausted[0]["limit"], 1, "{exhausted:?}");
    }

    #[test]
    fn a_second_split_of_the_same_dimension_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let first = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "first split",
                "--actor",
                "owner",
            ],
        );
        assert!(
            first.status.success(),
            "the first split must succeed: {}{}",
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );

        let before_second_head = workspace.control_head();
        let second = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "second split, immediately",
                "--actor",
                "owner",
            ],
        );
        assert!(
            !second.status.success(),
            "a second split of an already-waived dimension must be refused"
        );
        assert_eq!(error_code(&second), "CH-POLICY-INVALID-TRANSITION");
        assert!(
            error_message(&second).contains("already"),
            "the refusal must name the earlier waiver: {}",
            error_message(&second)
        );
        assert_eq!(
            workspace.control_head(),
            before_second_head,
            "the control repository head must not move on refusal"
        );
        assert_eq!(
            disposition_recorded_events(&workspace).len(),
            1,
            "only the first split's fact may exist"
        );
    }

    #[test]
    fn a_missing_follow_up_card_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "the follow-up card was never created",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a follow-up card that was never activated must be refused"
        );
        assert_eq!(error_code(&output), "CH-PRECONDITION-NOT-FOUND");
        assert!(
            error_message(&output).contains("F-002"),
            "the refusal must name the missing follow-up card: {}",
            error_message(&output)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_card_cannot_be_its_own_follow_up() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-001",
                "--rationale",
                "a card cannot be its own follow-up",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a card named as its own follow-up must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        assert!(
            error_message(&output).contains("own follow-up"),
            "{}",
            error_message(&output)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_follow_up_card_in_another_cycle_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        workspace.cycle(&[
            "create",
            "--cycle-id",
            "C-002",
            "--objective",
            "Second slice",
        ]);
        workspace.cycle(&["activate", "--cycle-id", "C-002"]);
        activate_card_in_cycle(&workspace, "F-002", "C-002", &["docs/f002/**"]);

        let before_head = workspace.control_head();
        let output = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "cross-cycle follow-up should not be allowed",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a follow-up card in another cycle must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        let message = error_message(&output);
        assert!(
            message.contains("C-001"),
            "the refusal must name the original card's cycle: {message}"
        );
        assert!(
            message.contains("C-002"),
            "the refusal must name the follow-up card's cycle: {message}"
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_terminal_follow_up_card_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        workspace.activate_card("F-002", &["docs/f002/**"]);
        workspace.card(&[
            "abandon",
            "--card-id",
            "F-002",
            "--reason",
            "superseded before it was picked up",
        ]);
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-002"])["data"]["state"],
            "abandoned",
            "the fixture must really have reached a terminal state through a governed command"
        );

        let before_head = workspace.control_head();
        let output = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "a terminal follow-up should not be allowed",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a terminal follow-up card must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        assert!(
            error_message(&output).contains("abandoned"),
            "the refusal must name the follow-up card's terminal state: {}",
            error_message(&output)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn an_unauthorized_actor_cannot_split() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let before_head = workspace.control_head();
        let output = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "an actor outside the configured set",
                "--actor",
                "intruder",
            ],
        );

        assert!(
            !output.status.success(),
            "an actor outside final_authorization_policy.authorizer_actor_ids must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "changes_requested",
            "the refused split must not have moved the card"
        );
    }

    #[test]
    fn an_unconfigured_authorization_policy_refuses() {
        // `opened_with_policy` (outside this module) installs a
        // convergence policy but no `final_authorization_policy` at all —
        // the scenario `opened_with_disposition_policies` above always
        // avoids by construction. Escalating a card only requires the
        // convergence policy; authorizing the split requires the other
        // one, which simply does not exist here.
        let workspace = opened_with_policy(1, 3);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let before_head = workspace.control_head();
        let output = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "no authorization policy exists at all",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a split must be refused when no final-authorization policy is configured"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_blank_rationale_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let before_head = workspace.control_head();
        let output = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "   ",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a blank rationale must be refused before anything is written"
        );
        assert_eq!(error_code(&output), "CH-USAGE-INVALID-ARGUMENTS");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn the_dry_run_makes_every_check_and_writes_nothing() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        // For the success the real command would make.
        let before_head = workspace.control_head();
        let success_preview = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "would split if this were real",
                "--actor",
                "owner",
                "--dry-run",
            ],
        );
        assert!(
            success_preview.status.success(),
            "the dry run must report success when the real command would succeed: {}{}",
            String::from_utf8_lossy(&success_preview.stdout),
            String::from_utf8_lossy(&success_preview.stderr)
        );
        let envelope: serde_json::Value = serde_json::from_slice(&success_preview.stdout).unwrap();
        assert_eq!(
            envelope["data"]["dry_run"],
            serde_json::json!(true),
            "{envelope}"
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head"
        );
        assert!(
            disposition_recorded_events(&workspace).is_empty(),
            "a dry run must never write a fact"
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "changes_requested",
            "a dry run must never move the card out of its previous state"
        );

        // For at least one refusal — the same unauthorized-actor refusal
        // exercised for the real command above.
        let refusal_preview = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "would split if this were real",
                "--actor",
                "intruder",
                "--dry-run",
            ],
        );
        assert!(
            !refusal_preview.status.success(),
            "the dry run must refuse the same way the real command would"
        );
        assert_eq!(error_code(&refusal_preview), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head, including on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());

        // Neither dry run consumed anything: the real split, run
        // afterward, still succeeds exactly once.
        let real = disposition_split_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--dimension",
                "review-returns",
                "--follow-up-card-id",
                "F-002",
                "--rationale",
                "the real split",
                "--actor",
                "owner",
            ],
        );
        assert!(
            real.status.success(),
            "the real command must still succeed after both dry runs: {}{}",
            String::from_utf8_lossy(&real.stdout),
            String::from_utf8_lossy(&real.stderr)
        );
        assert_eq!(disposition_recorded_events(&workspace).len(), 1);
    }
}

// 74-8: `disposition redesign`, run by an actor authorized under
// `final_authorization_policy.authorizer_actor_ids`, permanently ends an
// escalated card because the approach itself was wrong, and names the
// exact card that replaces it — the sixth and last of #74's dispositions.
// Closest to `abandon`: both terminate the original card through the very
// same `CardState::Abandoned` transition. What `redesign` adds is the
// replacement binding, and — deliberately, unlike `split`'s follow-up —
// that binding may name a card in a different cycle, recorded
// self-describingly via `replacement_cycle_id` alongside
// `replacement_card_id`. Unlike `split` and `accept-risk`, it waives
// nothing and never touches `escalation_waived`: the card is terminal, so
// there is no budget question left to answer, and there is no
// `--dimension` either.
//
// Wrapped in its own module for the same reason every sibling disposition
// module is: `the_dry_run_makes_every_check_and_writes_nothing` is already
// the name used above, five times, for the same property on five other
// commands, and none of the existing tests may be touched or renamed
// beyond what this card's own contract requires. A module gives this
// card's instance of that recurring name a distinct path
// (`disposition_redesign::the_dry_run_makes_every_check_and_writes_nothing`)
// without colliding.
mod disposition_redesign {
    use super::*;

    /// Identical in shape to every sibling module's own copy of this
    /// helper: `support::Workspace` is outside this card's file scope, so
    /// each disposition module keeps its fixture-building local rather
    /// than reaching into a sibling module's private helpers.
    fn initialized_with_authorizers(authorizers: &[&str]) -> Workspace {
        let workspace = Workspace::new();
        let mut args: Vec<String> = vec![
            "project".into(),
            "init".into(),
            "--project-id".into(),
            "example".into(),
            "--repository".into(),
            workspace.repository.display().to_string(),
            "--control".into(),
            workspace.control.display().to_string(),
            "--authority".into(),
            workspace.authority.display().to_string(),
            "--worktree-root".into(),
            workspace.worktrees.display().to_string(),
        ];
        for authorizer in authorizers {
            args.push("--final-authorizer-actor-id".into());
            args.push((*authorizer).to_owned());
        }
        let output = Workspace::run(&args);
        assert!(
            output.status.success(),
            "project init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        workspace.register_gate("gate.unit", &["true"]);
        workspace.register_gate("gate.all", &["true"]);
        workspace
    }

    /// Like [`opened_with_policy`], but with a final-authorization policy
    /// installed too, so `disposition redesign`'s authorization check (#7)
    /// has a configured set to resolve. Order matters exactly as it does in
    /// `opened_with_policy`: both policies must be in place before the
    /// cycle is created, which pins the project configuration's digest.
    fn opened_with_disposition_policies(
        card_limit: u32,
        integration_limit: u32,
        authorizers: &[&str],
    ) -> Workspace {
        let workspace = initialized_with_authorizers(authorizers);
        workspace.configure_convergence_policy(card_limit, integration_limit);
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

    /// Runs `disposition redesign` in JSON mode, returning the raw output.
    /// Mirrors every other per-group `_raw` helper in this file; kept local
    /// because `support::Workspace` is outside this card's file scope.
    fn disposition_redesign_raw(workspace: &Workspace, args: &[&str]) -> std::process::Output {
        let mut full = vec![
            "disposition".to_owned(),
            "redesign".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
        ];
        full.extend(args.iter().map(|arg| (*arg).to_owned()));
        Workspace::run(&full)
    }

    /// Every recorded `convergence.disposition_recorded` fact. Every fact
    /// this module's fixtures ever record is a redesign (none configures a
    /// renewal, rebaseline, abandon, acceptance, or split too), so this is
    /// never filtered further by `metadata.disposition`.
    fn disposition_recorded_events(workspace: &Workspace) -> Vec<serde_json::Value> {
        workspace
            .events()
            .into_iter()
            .filter(|event| event["event_type"] == "convergence.disposition_recorded")
            .collect()
    }

    /// Creates and activates a card in an arbitrary cycle, not necessarily
    /// `C-001`. Needed only by
    /// `a_replacement_in_another_cycle_is_recorded_with_its_own_cycle`:
    /// `support::Workspace::activate_card`, and every other
    /// `activate_card_*` helper it offers, hard-codes `cycle_id: C-001`,
    /// which that test's replacement card cannot use since the whole point
    /// is putting it in a different cycle. Mirrors
    /// `activate_card_with_gates`'s body directly, for the same reason
    /// `initialized_with_authorizers` mirrors `Workspace::initialized`'s:
    /// `support::Workspace` is outside this card's file scope.
    fn activate_card_in_cycle(
        workspace: &Workspace,
        card_id: &str,
        cycle_id: &str,
        include: &[&str],
    ) {
        let list = include
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let base =
            workspace.cycle_json(&["status", "--cycle-id", cycle_id])["data"]["baseline_sha"]
                .as_str()
                .expect("cycle has a frozen baseline")
                .to_owned();
        let body = format!(
            "card_id: {card_id}\ncycle_id: {cycle_id}\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{list}]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        );
        let path = workspace.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        workspace.card(&["create", "--draft", &path.display().to_string()]);
        workspace.card(&["activate", "--card-id", card_id]);
    }

    #[test]
    fn an_authorized_redesign_ends_a_card_and_names_its_replacement() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let card_status = workspace.card_json(&["status", "--card-id", "F-001"]);
        let replacement_status_before = workspace.card_json(&["status", "--card-id", "F-002"]);
        let base = workspace.authority_head();
        let pre_redesign_head = workspace.control_head();

        let redesign = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "authorized redesign for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            redesign.status.success(),
            "an authorized redesign of an escalated card must succeed: {}{}",
            String::from_utf8_lossy(&redesign.stdout),
            String::from_utf8_lossy(&redesign.stderr)
        );

        // 79-2's lesson, restated by the contract for this card: an event
        // written but not committed is invisible by content alone, because
        // the very next transaction would stage the whole control tree and
        // sweep it in regardless. The only way to catch "wrote but did not
        // commit" is to check, right here, that this command's own commit
        // is what moved the head and left the tree clean — before anything
        // else touches the control repository.
        assert_ne!(
            workspace.control_head(),
            pre_redesign_head,
            "disposition redesign must commit its own write; the control head must move"
        );
        assert!(
            support::capture(&workspace.control, &["status", "--porcelain"]).is_empty(),
            "the control tree must be porcelain-clean immediately after a successful redesign"
        );

        // The original card is terminated; the replacement is named, not
        // mutated.
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "abandoned",
            "a redesign must move the original card to abandoned"
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-002"])["data"]["state"],
            replacement_status_before["data"]["state"],
            "a redesign must not move the replacement card's lifecycle state"
        );

        let dispositions = disposition_recorded_events(&workspace);
        assert_eq!(
            dispositions.len(),
            1,
            "exactly one disposition fact must be recorded: {dispositions:?}"
        );
        let fact = &dispositions[0];
        assert_eq!(
            fact["event_type"], "convergence.disposition_recorded",
            "{fact}"
        );
        assert_eq!(fact["actor_id"], "owner", "{fact}");
        assert_eq!(fact["cycle_id"], "C-001", "{fact}");
        assert_eq!(fact["card_id"], "F-001", "{fact}");
        assert_eq!(
            fact["card_revision"], card_status["data"]["revision"],
            "{fact}"
        );
        assert_eq!(fact["card_digest"], card_status["data"]["digest"], "{fact}");
        assert_eq!(
            fact["head_sha"], base,
            "head must bind to the current revision's own base_sha, the one exact SHA a card is \
         guaranteed to carry in any state: {fact}"
        );
        assert_eq!(fact["previous_state"], "changes_requested", "{fact}");
        assert_eq!(fact["next_state"], "abandoned", "{fact}");
        assert_eq!(fact["metadata"]["disposition"], "redesign", "{fact}");
        assert_eq!(fact["metadata"]["replacement_card_id"], "F-002", "{fact}");
        assert_eq!(fact["metadata"]["replacement_cycle_id"], "C-001", "{fact}");
        assert_eq!(
            fact["metadata"]["rationale"], "authorized redesign for testing",
            "{fact}"
        );
        assert_eq!(fact["metadata"]["authorized_by"], "owner", "{fact}");
        assert_real_policy_digest(fact);
        assert!(
            !fact["metadata"]
                .as_object()
                .expect("metadata is an object")
                .contains_key("dimension"),
            "a redesign fact must name no dimension: {fact}"
        );
    }

    #[test]
    fn a_replacement_in_another_cycle_is_recorded_with_its_own_cycle() {
        // §5.2: unlike `split`, a redesign's replacement may live in a
        // different cycle — the original card is terminal, so no
        // dual-governance question survives. The fact must still record
        // which cycle the replacement is actually in, not silently assume
        // it shares the original's — 74-7b's review found exactly that gap
        // in `split`'s own bare `follow_up_card_id`.
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        workspace.cycle(&[
            "create",
            "--cycle-id",
            "C-002",
            "--objective",
            "Second slice",
        ]);
        workspace.cycle(&["activate", "--cycle-id", "C-002"]);
        activate_card_in_cycle(&workspace, "F-002", "C-002", &["docs/f002/**"]);

        let redesign = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "the replacement lives in a different cycle",
                "--actor",
                "owner",
            ],
        );
        assert!(
            redesign.status.success(),
            "a replacement card in another cycle must be permitted, unlike split's follow-up: \
         {}{}",
            String::from_utf8_lossy(&redesign.stdout),
            String::from_utf8_lossy(&redesign.stderr)
        );

        let dispositions = disposition_recorded_events(&workspace);
        assert_eq!(dispositions.len(), 1, "{dispositions:?}");
        let fact = &dispositions[0];
        assert_eq!(
            fact["cycle_id"], "C-001",
            "the fact itself is still bound to the original card's own cycle: {fact}"
        );
        assert_eq!(fact["metadata"]["replacement_card_id"], "F-002", "{fact}");
        assert_eq!(
            fact["metadata"]["replacement_cycle_id"], "C-002",
            "the replacement's own cycle must be recorded, not the original's: {fact}"
        );
    }

    #[test]
    // This is the load-bearing test: the mutation in the contract's own
    // §8 deletes the redesign exclusion from the card-bound
    // `DispositionMetadata` loop's filter, which makes the redesign fact
    // recorded below attempt a `dimension`-shaped parse it was never going
    // to satisfy — refusing not just F-001's own projection but the whole
    // cycle's, which is exactly what `approve_card` below would trip over
    // for F-003, a card the redesign never touched.
    fn an_unrelated_card_in_the_same_cycle_still_works_after_a_redesign() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let redesign = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "ending the escalated card for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            redesign.status.success(),
            "the redesign that sets up this scenario must itself succeed: {}{}",
            String::from_utf8_lossy(&redesign.stdout),
            String::from_utf8_lossy(&redesign.stderr)
        );

        // A third card in the same cycle, unrelated to both the original
        // and the replacement, scoped away from both so all three can
        // coexist without an ownership-overlap refusal, must be completely
        // unaffected by F-001's redesign. `card status` alone would only
        // prove the read-only projection path still works; `approve_card`
        // drives F-003 through `handoff create`, `review begin`, and
        // `review record` — three separate `require_convergence_budget`
        // call sites — so this proves the budget-gated write path stays
        // open too.
        workspace.activate_card("F-003", &["docs/f003/**"]);
        let status = workspace.card_raw(&["status", "--card-id", "F-003"]);
        assert!(
            status.status.success(),
            "card status for an unrelated card must not be broken by another card's redesign: \
         {}{}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        );

        workspace.approve_card("F-003", "docs/f003/a.md");

        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-003"])["data"]["state"],
            "approved",
            "an unrelated card in the same cycle must still deliver and be reviewed after \
         another card's redesign"
        );
    }

    #[test]
    fn a_second_redesign_of_the_same_card_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);
        workspace.activate_card("F-003", &["docs/f003/**"]);

        let first = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "first redesign",
                "--actor",
                "owner",
            ],
        );
        assert!(
            first.status.success(),
            "the first redesign must succeed: {}{}",
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );

        // The card's recorded facts do not disappear when it is
        // redesigned. `card status` publishes exactly `assess_card`'s own
        // assessment (see `card.rs`'s `card_convergence` doc comment) — so
        // this is a direct, observed answer to whether `assess_card` still
        // reports the card `Escalated` post-redesign, not an inference
        // from the refusal below.
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["convergence"]["status"],
            "escalated",
            "a redesigned card's recorded facts do not disappear; it must still assess as \
         escalated"
        );

        let before_second_head = workspace.control_head();
        let second = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-003",
                "--rationale",
                "second redesign, immediately",
                "--actor",
                "owner",
            ],
        );
        assert!(
            !second.status.success(),
            "a second redesign of an already-abandoned card must be refused"
        );
        assert_eq!(error_code(&second), "CH-POLICY-INVALID-TRANSITION");
        // It is check 3 (the lifecycle transition), not check 2 (the
        // escalation check), that refuses the repeat — the assertion above
        // already established the card still reads `Escalated`, so if
        // check 2 had fired instead the message would say the card is not
        // escalated, not name a transition.
        assert!(
            error_message(&second).contains("cannot move from `abandoned` to `abandoned`"),
            "{}",
            error_message(&second)
        );
        assert_eq!(
            workspace.control_head(),
            before_second_head,
            "the control repository head must not move on refusal"
        );
        assert_eq!(
            disposition_recorded_events(&workspace).len(),
            1,
            "only the first redesign's fact may exist"
        );
    }

    #[test]
    fn a_card_that_is_not_escalated_cannot_be_redesigned() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        workspace.activate_card("F-001", &["src/**"]);
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let before_head = workspace.control_head();
        let output = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "nothing has escalated yet",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a card that has never been escalated must not be redesignable"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        assert!(
            error_message(&output).contains("card abandon"),
            "the refusal must name the route that does apply: {}",
            error_message(&output)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "ready",
            "the refused redesign must not have moved the card"
        );
    }

    #[test]
    fn a_missing_replacement_card_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "the replacement card was never created",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a replacement card that was never activated must be refused"
        );
        assert_eq!(error_code(&output), "CH-PRECONDITION-NOT-FOUND");
        assert!(
            error_message(&output).contains("F-002"),
            "the refusal must name the missing replacement card: {}",
            error_message(&output)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_card_cannot_replace_itself() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        let before_head = workspace.control_head();
        let output = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-001",
                "--rationale",
                "a card cannot be its own replacement",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a card named as its own replacement must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        assert!(
            error_message(&output).contains("own replacement"),
            "{}",
            error_message(&output)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_terminal_replacement_card_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");

        workspace.activate_card("F-002", &["docs/f002/**"]);
        workspace.card(&[
            "abandon",
            "--card-id",
            "F-002",
            "--reason",
            "superseded before it was picked up",
        ]);
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-002"])["data"]["state"],
            "abandoned",
            "the fixture must really have reached a terminal state through a governed command"
        );

        let before_head = workspace.control_head();
        let output = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "a terminal replacement should not be allowed",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a terminal replacement card must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
        assert!(
            error_message(&output).contains("abandoned"),
            "the refusal must name the replacement card's terminal state: {}",
            error_message(&output)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn an_unauthorized_actor_cannot_redesign() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let before_head = workspace.control_head();
        let output = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "an actor outside the configured set",
                "--actor",
                "intruder",
            ],
        );

        assert!(
            !output.status.success(),
            "an actor outside final_authorization_policy.authorizer_actor_ids must be refused"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "changes_requested",
            "the refused redesign must not have moved the card"
        );
    }

    #[test]
    fn an_unconfigured_authorization_policy_refuses() {
        // `opened_with_policy` (outside this module) installs a
        // convergence policy but no `final_authorization_policy` at all —
        // the scenario `opened_with_disposition_policies` above always
        // avoids by construction. Escalating a card only requires the
        // convergence policy; authorizing the redesign requires the other
        // one, which simply does not exist here.
        let workspace = opened_with_policy(1, 3);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let before_head = workspace.control_head();
        let output = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "no authorization policy exists at all",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a redesign must be refused when no final-authorization policy is configured"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn a_blank_rationale_refuses() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        let before_head = workspace.control_head();
        let output = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "   ",
                "--actor",
                "owner",
            ],
        );

        assert!(
            !output.status.success(),
            "a blank rationale must be refused before anything is written"
        );
        assert_eq!(error_code(&output), "CH-USAGE-INVALID-ARGUMENTS");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());
    }

    #[test]
    fn the_dry_run_makes_every_check_and_writes_nothing() {
        let workspace = opened_with_disposition_policies(1, 3, &["owner"]);
        escalate_via_review_returns(&workspace, "F-001");
        workspace.activate_card("F-002", &["docs/f002/**"]);

        // For the success the real command would make.
        let before_head = workspace.control_head();
        let success_preview = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "would redesign if this were real",
                "--actor",
                "owner",
                "--dry-run",
            ],
        );
        assert!(
            success_preview.status.success(),
            "the dry run must report success when the real command would succeed: {}{}",
            String::from_utf8_lossy(&success_preview.stdout),
            String::from_utf8_lossy(&success_preview.stderr)
        );
        let envelope: serde_json::Value = serde_json::from_slice(&success_preview.stdout).unwrap();
        assert_eq!(
            envelope["data"]["dry_run"],
            serde_json::json!(true),
            "{envelope}"
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head"
        );
        assert!(
            disposition_recorded_events(&workspace).is_empty(),
            "a dry run must never write a fact"
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "changes_requested",
            "a dry run must never move the card out of its previous state"
        );
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-002"])["data"]["state"],
            "ready",
            "a dry run must never move the replacement card's state either"
        );

        // For at least one refusal — the same unauthorized-actor refusal
        // exercised for the real command above.
        let refusal_preview = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "would redesign if this were real",
                "--actor",
                "intruder",
                "--dry-run",
            ],
        );
        assert!(
            !refusal_preview.status.success(),
            "the dry run must refuse the same way the real command would"
        );
        assert_eq!(error_code(&refusal_preview), "CH-POLICY-NOT-ACCEPTED");
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a dry run must never move the control head, including on refusal"
        );
        assert!(disposition_recorded_events(&workspace).is_empty());

        // Neither dry run consumed anything: the real redesign, run
        // afterward, still succeeds exactly once.
        let real = disposition_redesign_raw(
            &workspace,
            &[
                "--card-id",
                "F-001",
                "--replacement-card-id",
                "F-002",
                "--rationale",
                "the real redesign",
                "--actor",
                "owner",
            ],
        );
        assert!(
            real.status.success(),
            "the real command must still succeed after both dry runs: {}{}",
            String::from_utf8_lossy(&real.stdout),
            String::from_utf8_lossy(&real.stderr)
        );
        assert_eq!(disposition_recorded_events(&workspace).len(), 1);
        assert_eq!(
            workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
            "abandoned"
        );
    }
}

// 73-2: the enforcement half of the pattern 73-1 only reported.
// `require_cycle_convergence_budget` (`integration.rs`) refuses every
// governed path that advances an integration toward promotion once a
// cycle's own convergence budget is spent — preparation, the merge/land/
// verify/review sequence, final authorization (`acceptance record`), and
// promotion itself — mirroring 72-2's `require_convergence_budget` at the
// card level. It never touches `integration abandon`, `cycle abandon`, or
// any inspect/report command: those are exits and windows, not advances.
//
// Unlike a card, which has six authorized dispositions (#74) to answer an
// escalation, an escalated *cycle* has exactly two exits: every disposition
// except `rebaseline` takes `--card-id`, so there is no cycle-scoped
// `renew`. The refusal names both, by command, and this module's most
// important test — `a_rebaseline_lets_an_escalated_cycle_integrate_again` —
// proves the first one actually works, not merely that it is named.
mod cycle_convergence_enforcement {
    use super::*;

    /// Escalates C-001's `integration_failures` budget through the governed
    /// `integration merge` path — the same `conflicting_under_policy` /
    /// `prepare_integration` fixture 73-1's own tests use, just with the
    /// cycle limit tight enough that this one real conflict alone exhausts
    /// it. Returns the still-open, never-merged integration id:
    /// `run_merge`'s own conflict handling never sets
    /// `record.integration_head`, so this integration keeps its lease
    /// (`IntegrationStatus::Prepared.holds_lease()`) — the one legitimate
    /// target every "does the gate fire first" test below reuses, rather
    /// than each rebuilding its own conflict.
    fn escalated_cycle(card_limit: u32) -> (Workspace, String) {
        let workspace = conflicting_under_policy(card_limit, 1);
        let id = prepare_integration(&workspace);
        let output = workspace.integration_raw(&[
            "merge",
            "--integration-id",
            &id,
            "--actor-id",
            "coordinator",
        ]);
        assert_eq!(
            error_code(&output),
            "CH-CONFLICT-MERGE-FAILED",
            "the fixture must fail on the conflict it was built to produce, before this \
             card's gate exists to refuse anything else"
        );
        assert_eq!(
            workspace.cycle_json(&["status", "--cycle-id", "C-001"])["data"]["convergence"]["status"],
            "escalated",
            "the fixture must actually exhaust the cycle's budget"
        );
        (workspace, id)
    }

    /// Creates and activates a card in an arbitrary cycle, not necessarily
    /// `C-001`. `support::Workspace::activate_card`, and every other
    /// `activate_card_*` helper it offers, hard-codes `cycle_id: C-001`,
    /// which `an_unrelated_cycle_still_completes`'s second cycle cannot use.
    /// Mirrors `activate_card_with_gates`'s body directly — the same reason
    /// `disposition_split`'s and `disposition_redesign`'s identically-named
    /// copies exist: `support::Workspace` is outside this card's file
    /// scope, so each module that needs a non-`C-001` card keeps its own
    /// copy rather than reaching into a sibling module's private helper.
    fn activate_card_in_cycle(
        workspace: &Workspace,
        card_id: &str,
        cycle_id: &str,
        include: &[&str],
    ) {
        let list = include
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let base =
            workspace.cycle_json(&["status", "--cycle-id", cycle_id])["data"]["baseline_sha"]
                .as_str()
                .expect("cycle has a frozen baseline")
                .to_owned();
        let body = format!(
            "card_id: {card_id}\ncycle_id: {cycle_id}\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{list}]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
        );
        let path = workspace.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        workspace.card(&["create", "--draft", &path.display().to_string()]);
        workspace.card(&["activate", "--card-id", card_id]);
    }

    /// Asserts that a raw command output was refused specifically for the
    /// escalated cycle's convergence budget — by code, and by naming both
    /// exits in the message — and that it left the control repository
    /// untouched. Shared by every "does the gate fire first" test below, so
    /// each one only has to say which command it drove and reuse this for
    /// the assertions they would otherwise all repeat verbatim.
    fn assert_refused_for_convergence(
        workspace: &Workspace,
        output: &std::process::Output,
        before_head: &str,
    ) {
        assert!(
            !output.status.success(),
            "an escalated cycle must refuse this command: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(error_code(output), "CH-POLICY-CONVERGENCE-ESCALATED");
        let message = error_message(output);
        assert!(
            message.contains("disposition rebaseline"),
            "the refusal must name the first exit, by command: {message}"
        );
        assert!(
            message.contains("cycle abandon"),
            "the refusal must name the second exit, by command: {message}"
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "the control repository head must not move on refusal"
        );
    }

    #[test]
    fn an_exhausted_cycle_budget_refuses_further_integration() {
        let (workspace, id) = escalated_cycle(3);
        let before_head = workspace.control_head();

        // The same still-open integration, tried again: this must be
        // refused for the *cycle's* spent convergence budget now, not for
        // the conflict that spent it in the first place — a second attempt
        // never even reaches `integration preflight`'s own simulation.
        let output = workspace.integration_raw(&[
            "merge",
            "--integration-id",
            &id,
            "--actor-id",
            "coordinator",
        ]);
        assert_refused_for_convergence(&workspace, &output, &before_head);
        let message = error_message(&output);
        assert!(message.contains("integration_failures"), "{message}");
        assert!(message.contains("1/1"), "{message}");

        // No second `integration_failure` fact: the gate refused before a
        // real merge attempt could ever be made, so there was nothing new
        // to fail.
        assert_eq!(
            attempt_recorded_events(&workspace).len(),
            1,
            "the gate must refuse before a second conflict could be recorded"
        );
    }

    #[test]
    fn an_unrelated_cycle_still_completes() {
        let (workspace, _id) = escalated_cycle(3);

        // A second cycle in the same project, wholly independent of C-001,
        // must prepare and merge normally while C-001 sits escalated — #73's
        // own acceptance criterion (contract §1): escalation is scoped to
        // the cycle `assess_cycle` was projected against, never the project.
        workspace.cycle(&[
            "create",
            "--cycle-id",
            "C-002",
            "--objective",
            "Second, unrelated slice",
        ]);
        workspace.cycle(&["activate", "--cycle-id", "C-002"]);
        activate_card_in_cycle(&workspace, "F-101", "C-002", &["docs/f101/**"]);
        workspace.approve_card("F-101", "docs/f101/a.md");

        let before_prepare = workspace.control_head();
        let prepared = workspace.integration_json(&[
            "prepare",
            "--cycle-id",
            "C-002",
            "--actor-id",
            "coordinator",
        ]);
        assert_ne!(
            workspace.control_head(),
            before_prepare,
            "prepare must commit its own write"
        );
        let id = prepared["data"]["integration_id"]
            .as_str()
            .expect("a prepared integration id")
            .to_owned();

        let before_merge = workspace.control_head();
        let merged = workspace.integration_raw(&[
            "merge",
            "--integration-id",
            &id,
            "--actor-id",
            "coordinator",
        ]);
        assert!(
            merged.status.success(),
            "an unrelated cycle's own integration must merge normally: {}{}",
            String::from_utf8_lossy(&merged.stdout),
            String::from_utf8_lossy(&merged.stderr)
        );
        assert_ne!(
            workspace.control_head(),
            before_merge,
            "merge must commit its own write"
        );
        assert!(
            support::capture(&workspace.control, &["status", "--porcelain"]).is_empty(),
            "the control tree must be porcelain-clean after the unrelated cycle's merge"
        );
        assert_eq!(
            workspace.cycle_json(&["status", "--cycle-id", "C-002"])["data"]["convergence"],
            serde_json::json!({ "status": "within" }),
            "the unrelated cycle's own budget must be untouched"
        );

        // And C-001 remains blocked throughout, proving the isolation runs
        // both ways.
        let still_blocked = workspace.integration_raw(&[
            "prepare",
            "--cycle-id",
            "C-001",
            "--actor-id",
            "coordinator",
        ]);
        assert_eq!(
            error_code(&still_blocked),
            "CH-POLICY-CONVERGENCE-ESCALATED"
        );
    }

    /// Like `Workspace::initialized`, but declares the final-authorization
    /// policy's authorized actors at `project init` time — needed only by
    /// `a_rebaseline_lets_an_escalated_cycle_integrate_again`, the one test
    /// in this module that exercises `disposition rebaseline` and so needs
    /// an authorizer for it to check. Mirrors
    /// `disposition_rebaseline::initialized_with_authorizers` directly, for
    /// the same reason `activate_card_in_cycle` mirrors its own sibling
    /// copies: `support::Workspace` is outside this card's file scope.
    fn initialized_with_authorizers(authorizers: &[&str]) -> Workspace {
        let workspace = Workspace::new();
        let mut args: Vec<String> = vec![
            "project".into(),
            "init".into(),
            "--project-id".into(),
            "example".into(),
            "--repository".into(),
            workspace.repository.display().to_string(),
            "--control".into(),
            workspace.control.display().to_string(),
            "--authority".into(),
            workspace.authority.display().to_string(),
            "--worktree-root".into(),
            workspace.worktrees.display().to_string(),
        ];
        for authorizer in authorizers {
            args.push("--final-authorizer-actor-id".into());
            args.push((*authorizer).to_owned());
        }
        let output = Workspace::run(&args);
        assert!(
            output.status.success(),
            "project init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        workspace.register_gate("gate.unit", &["true"]);
        workspace.register_gate("gate.all", &["true"]);
        workspace
    }

    /// Runs `disposition rebaseline` in JSON mode, returning the raw
    /// output. Mirrors every other per-module `_raw` helper in this file;
    /// kept local because `support::Workspace` is outside this card's file
    /// scope.
    fn disposition_rebaseline_raw(workspace: &Workspace, args: &[&str]) -> std::process::Output {
        let mut full = vec![
            "disposition".to_owned(),
            "rebaseline".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
        ];
        full.extend(args.iter().map(|arg| (*arg).to_owned()));
        Workspace::run(&full)
    }

    #[test]
    // The most important test in this card (contract §6.3): it proves the
    // first exit the refusal above names actually works, not merely that it
    // is named. If a rebaseline did not free the cycle, the refusal would
    // be naming a way out that does not exist — a finding that changes the
    // design, not something to work around.
    #[allow(clippy::too_many_lines)]
    fn a_rebaseline_lets_an_escalated_cycle_integrate_again() {
        let workspace = initialized_with_authorizers(&["owner"]);
        workspace.configure_convergence_policy(3, 1);

        // The same conflict-on-the-protected-branch fixture
        // `conflicting_under_policy` builds, assembled by hand here because
        // that helper calls `Workspace::initialized`, which cannot declare
        // the `--final-authorizer-actor-id` this test also needs — the same
        // reason `disposition_rebaseline`'s own `opened_with_disposition_
        // policies` cannot reuse the top-level `opened_with_policy` either.
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

        let id = prepare_integration(&workspace);
        let conflict = workspace.integration_raw(&[
            "merge",
            "--integration-id",
            &id,
            "--actor-id",
            "coordinator",
        ]);
        assert_eq!(
            error_code(&conflict),
            "CH-CONFLICT-MERGE-FAILED",
            "the fixture must fail on the conflict it was built to produce"
        );
        assert_eq!(
            workspace.cycle_json(&["status", "--cycle-id", "C-001"])["data"]["convergence"]["status"],
            "escalated"
        );

        // Clear INT-001's lease — abandon is one of the two exits, and stays
        // ungated — then declare a fresh card whose write scope cannot
        // possibly collide with the retired one. The point is to isolate
        // the *cycle's* budget as the thing still blocking progress, not
        // this one integration's own conflict.
        workspace.integration(&[
            "abandon",
            "--integration-id",
            &id,
            "--actor-id",
            "coordinator",
            "--reason",
            "clearing the lease to isolate the cycle-level refusal",
        ]);
        workspace.activate_card("F-002", &["src/f002/**"]);
        workspace.approve_card("F-002", "src/f002/a.rs");

        // The previously-refused path: a fresh `integration prepare` for
        // *only* F-002 is still refused while the cycle stays escalated,
        // even with the conflicting integration cleared out of the way and
        // a clean candidate ready to go. Named explicitly with `--card-id`
        // rather than left to select every ready card: F-001 itself is
        // `approved` again after the abandon above, and selecting it back
        // in would reintroduce the very conflict this test is careful to
        // keep out of the way, for a reason unrelated to what it checks.
        let before_rebaseline = workspace.control_head();
        let still_refused = workspace.integration_raw(&[
            "prepare",
            "--cycle-id",
            "C-001",
            "--actor-id",
            "coordinator",
            "--card-id",
            "F-002",
        ]);
        assert_eq!(
            error_code(&still_refused),
            "CH-POLICY-CONVERGENCE-ESCALATED",
            "prepare must still be refused before the exit below is exercised"
        );
        assert_eq!(workspace.control_head(), before_rebaseline);

        // The first named exit, exercised for real, with a genuinely
        // different policy — reinstalling the same digest unchanged is
        // itself refused by `disposition rebaseline`'s own contract, so
        // this is not merely a formality. The new policy keeps the same
        // tight `integration_limit` of 1, changing only the card limit: a
        // looser cycle limit would let the retried path succeed even if
        // `project` failed to stop counting the retired fact, which would
        // prove nothing about the digest retirement this test exists to
        // check — see the assertion just below that the cycle reports
        // `within`, not merely that it is one attempt short of `escalated`.
        let new_policy_path = write_json(
            &workspace,
            "rebaseline-policy.json",
            &convergence_policy_document(5, 1),
        );
        let rebaseline = disposition_rebaseline_raw(
            &workspace,
            &[
                "--policy",
                &new_policy_path,
                "--rationale",
                "opening the emergency exit for testing",
                "--actor",
                "owner",
            ],
        );
        assert!(
            rebaseline.status.success(),
            "an authorized rebaseline must succeed: {}{}",
            String::from_utf8_lossy(&rebaseline.stdout),
            String::from_utf8_lossy(&rebaseline.stderr)
        );
        assert_ne!(
            workspace.control_head(),
            before_rebaseline,
            "rebaseline must commit its own write"
        );
        assert!(
            support::capture(&workspace.control, &["status", "--porcelain"]).is_empty(),
            "the control tree must be porcelain-clean immediately after rebaseline"
        );

        // The cycle reports `within` again: the retired fact stops counting
        // without being erased from the record.
        assert_eq!(
            workspace.cycle_json(&["status", "--cycle-id", "C-001"])["data"]["convergence"],
            serde_json::json!({ "status": "within" }),
            "the retired fact must stop counting once rebaselined"
        );
        assert_eq!(
            attempt_recorded_events(&workspace).len(),
            1,
            "the retired fact must remain in the record, not be erased"
        );

        // The headline claim: the previously-refused path now succeeds, all
        // the way through a real merge — the cycle can integrate again.
        // Still `--card-id F-002` alone, for the same reason as above: this
        // proves the cycle-level budget was what was blocking, not that
        // F-001's own conflict happened to have gone away.
        let after_rebaseline = workspace.control_head();
        let prepared = workspace.integration_json(&[
            "prepare",
            "--cycle-id",
            "C-001",
            "--actor-id",
            "coordinator",
            "--card-id",
            "F-002",
        ]);
        assert_ne!(
            workspace.control_head(),
            after_rebaseline,
            "prepare must commit its own write"
        );
        let new_id = prepared["data"]["integration_id"]
            .as_str()
            .expect("a prepared integration id")
            .to_owned();

        let before_merge = workspace.control_head();
        let merged = workspace.integration_raw(&[
            "merge",
            "--integration-id",
            &new_id,
            "--actor-id",
            "coordinator",
        ]);
        assert!(
            merged.status.success(),
            "the exit named in the refusal must actually let the cycle integrate again: {}{}",
            String::from_utf8_lossy(&merged.stdout),
            String::from_utf8_lossy(&merged.stderr)
        );
        assert_ne!(
            workspace.control_head(),
            before_merge,
            "merge must commit its own write"
        );
        assert!(
            support::capture(&workspace.control, &["status", "--porcelain"]).is_empty(),
            "the control tree must be porcelain-clean after the post-rebaseline merge"
        );
        assert_eq!(
            attempt_recorded_events(&workspace).len(),
            1,
            "the clean post-rebaseline merge must not add a new failure fact"
        );
    }

    #[test]
    fn an_escalated_cycle_can_still_be_abandoned() {
        let (workspace, _id) = escalated_cycle(3);
        let before_head = workspace.control_head();
        let output = workspace.cycle_raw(&[
            "abandon",
            "--cycle-id",
            "C-001",
            "--reason",
            "closing the escalated cycle",
        ]);
        assert!(
            output.status.success(),
            "cycle abandon must stay usable on an escalated cycle: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_ne!(
            workspace.control_head(),
            before_head,
            "cycle abandon must commit its own write"
        );
        assert!(
            support::capture(&workspace.control, &["status", "--porcelain"]).is_empty(),
            "the control tree must be porcelain-clean after abandoning an escalated cycle"
        );
        assert_eq!(
            workspace.cycle_json(&["status", "--cycle-id", "C-001"])["data"]["status"],
            "abandoned"
        );
    }

    #[test]
    fn an_escalated_cycle_is_still_inspectable() {
        let (workspace, id) = escalated_cycle(3);

        let status = workspace.cycle_raw(&["status", "--cycle-id", "C-001"]);
        assert!(
            status.status.success(),
            "cycle status must stay readable on an escalated cycle: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        let status_envelope: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(
            status_envelope["data"]["convergence"]["status"],
            "escalated"
        );

        let list = workspace.cycle_raw(&["list"]);
        assert!(
            list.status.success(),
            "cycle list must stay readable on an escalated cycle"
        );

        let inspect = workspace.integration_raw(&["inspect", "--integration-id", &id]);
        assert!(
            inspect.status.success(),
            "integration inspect must stay readable on an escalated cycle: {}",
            String::from_utf8_lossy(&inspect.stderr)
        );

        let ready = workspace.integration_raw(&["ready", "--cycle-id", "C-001"]);
        assert!(
            ready.status.success(),
            "integration ready must stay readable on an escalated cycle: {}",
            String::from_utf8_lossy(&ready.stderr)
        );

        // `decision-packet` only ever answers for a `--final` integration;
        // this fixture's is an ordinary per-card one, so its refusal here
        // must be the pre-existing "not the final integration" reason —
        // never the convergence gate, proving decision-packet was never
        // wired into the gated set at all.
        let packet = workspace.integration_raw(&["decision-packet", "--integration-id", &id]);
        assert!(!packet.status.success());
        assert_eq!(error_code(&packet), "CH-POLICY-DECISION-PACKET-FINAL-ONLY");
    }

    // Contract §6 item 6: one test per additional gated call site beyond
    // `integration merge` (already covered above). Every test below reuses
    // `escalated_cycle`'s still-open, never-merged integration and drives
    // exactly one more command against it, proving `require_cycle_
    // convergence_budget` is the first check each of these commands makes —
    // the refusal is `CH-POLICY-CONVERGENCE-ESCALATED`, never whichever
    // other precondition that command would otherwise report first (a
    // missing landing commit, an illegal status transition, and so on),
    // which is exactly what would surface if the gate were missing. None of
    // these integrations are otherwise ready for the command under test —
    // that is deliberate: it proves the gate runs before anything else does,
    // not only when every other precondition happens to already hold.

    #[test]
    fn an_exhausted_cycle_budget_refuses_a_new_integration_prepare() {
        let (workspace, id) = escalated_cycle(3);

        // `integration abandon` stays ungated — it is one of the exits —
        // so clearing the escalating integration's lease first proves this
        // refusal is about the cycle's spent budget, not merely about
        // `PolicyIntegrationOpen`'s pre-existing "one integration at a
        // time" rule.
        let abandon = workspace.integration_raw(&[
            "abandon",
            "--integration-id",
            &id,
            "--actor-id",
            "coordinator",
            "--reason",
            "clearing the lease to prove prepare is still gated",
        ]);
        assert!(
            abandon.status.success(),
            "integration abandon must stay usable on an escalated cycle: {}{}",
            String::from_utf8_lossy(&abandon.stdout),
            String::from_utf8_lossy(&abandon.stderr)
        );

        let before_head = workspace.control_head();
        let output = workspace.integration_raw(&[
            "prepare",
            "--cycle-id",
            "C-001",
            "--actor-id",
            "coordinator",
        ]);
        assert_refused_for_convergence(&workspace, &output, &before_head);
    }

    #[test]
    fn an_exhausted_cycle_budget_refuses_integration_land() {
        let (workspace, id) = escalated_cycle(3);
        let before_head = workspace.control_head();
        let output = workspace.integration_raw(&[
            "land",
            "--integration-id",
            &id,
            "--actor-id",
            "coordinator",
        ]);
        assert_refused_for_convergence(&workspace, &output, &before_head);
    }

    #[test]
    fn an_exhausted_cycle_budget_refuses_integration_verify() {
        let (workspace, id) = escalated_cycle(3);
        let before_head = workspace.control_head();
        let output = workspace.integration_raw(&[
            "verify",
            "--integration-id",
            &id,
            "--actor-id",
            "verifier",
        ]);
        assert_refused_for_convergence(&workspace, &output, &before_head);
    }

    #[test]
    fn an_exhausted_cycle_budget_refuses_integration_review() {
        let (workspace, id) = escalated_cycle(3);
        let before_head = workspace.control_head();
        let output = workspace.integration_raw(&[
            "review",
            "--integration-id",
            &id,
            "--reviewer-actor-id",
            "reviewer",
        ]);
        assert_refused_for_convergence(&workspace, &output, &before_head);
    }

    #[test]
    fn an_exhausted_cycle_budget_refuses_integration_promote() {
        let (workspace, id) = escalated_cycle(3);
        let before_head = workspace.control_head();
        let output = workspace.integration_raw(&[
            "promote",
            "--integration-id",
            &id,
            "--actor-id",
            "release-agent",
        ]);
        assert_refused_for_convergence(&workspace, &output, &before_head);
    }

    #[test]
    fn an_exhausted_cycle_budget_refuses_acceptance_record() {
        let (workspace, id) = escalated_cycle(3);
        let before_head = workspace.control_head();
        let output = workspace.acceptance_raw(&[
            "record",
            "--integration-id",
            &id,
            "--authorizer-actor-id",
            "owner",
        ]);
        assert_refused_for_convergence(&workspace, &output, &before_head);
    }
}

// #85: nine call sites map an unusable convergence projection to
// `ErrorCode::InternalControlCorrupt` instead of letting a malformed,
// duplicate, foreign, or unbound convergence fact read as an empty, unspent
// budget. `project`'s own doc comment says why a partial view cannot be
// tolerated: it "would make an attacker-controlled malformed fact look like
// unused budget." Before this card nothing exercised any of the nine —
// `grep -rn "INTERNAL-CONTROL-CORRUPT" tests/` matched three files unrelated
// to this subsystem, and this file, which owns it, had no occurrence at all
// — so a plausible "be lenient about legacy projects" edit that swallowed
// the refusal and returned `LegacyUnassessed` would have left every
// existing test green while quietly failing *open* on exactly the input the
// design exists to fail closed on.
//
// The nine sites, and which test below pins each:
//
//   - `card.rs`'s `card_convergence`, the `card status` read path, called
//     only by `run_status` — `a_corrupt_fact_refuses_card_status`.
//   - `card.rs`'s `require_convergence_budget`, card enforcement, called
//     from `handoff create`, `review begin`, `review record`, `card
//     revise`, and `work start`/`work resume` (grepped, not assumed — see
//     the test's own comment for the exact call sites) —
//     `a_corrupt_fact_refuses_a_card_advancing_command`.
//   - `cycle.rs`'s `cycle_convergence`, `card_convergence`'s read-path
//     twin, called only by `run_status` —
//     `a_corrupt_fact_refuses_cycle_status`.
//   - `integration.rs`'s `require_cycle_convergence_budget`, cycle
//     enforcement, called from every governed path in `integration.rs` and
//     `acceptance.rs` that advances an integration toward promotion —
//     `a_corrupt_fact_refuses_an_integration_advancing_command`.
//   - `disposition.rs`'s five card-scoped preflights — `require_renewable`,
//     `require_abandonable`, `require_risk_acceptable`,
//     `require_splittable`, and `require_redesignable` — each reads this
//     cycle's events, calls `project`, and maps its refusal to
//     `InternalControlCorrupt` in byte-for-byte the same shape
//     (disposition.rs:420-433, 1048-1061, 1293-1306, 1596-1609,
//     1942-1955), before any of the five reaches its own dimension,
//     authorization, or lifecycle checks. Originally pinned by one
//     representative test standing in for all five; a follow-up repair on
//     this card found that economy was a defect, not a saving — removing
//     only `require_renewable`'s refusal left the full suite green — so
//     each of the five is now pinned by its own test:
//     `a_corrupt_fact_refuses_a_disposition` (`abandon`),
//     `a_corrupt_fact_refuses_a_disposition_renew` (`renew`),
//     `a_corrupt_fact_refuses_a_disposition_accept_risk` (`accept-risk`),
//     `a_corrupt_fact_refuses_a_disposition_split` (`split`), and
//     `a_corrupt_fact_refuses_a_disposition_redesign` (`redesign`). The
//     five blocks are byte-identical text — confirmed the hard way, since
//     an `Edit` aimed at one of them by its surrounding text matches all
//     five — and byte-identical copies drift independently: a fail-closed
//     rule only some call sites enforce is not fail-closed.
//
// Every test asserts the exact `CH-INTERNAL-CONTROL-CORRUPT` code, never
// merely a non-zero exit — a command refused for an unrelated precondition
// would satisfy a looser assertion and prove nothing — and that the
// refusal names the precise branch of `project` this corruption triggers
// ("fact names a foreign policy digest"), not some other malformed-fact
// reason. The two read-path tests additionally assert the response never
// reports `legacy_unassessed`, the exact wrong answer a "be lenient" edit
// would produce, and every test asserts the control repository head did
// not move: a refusal must not write.
mod fail_closed_on_corrupt_projection {
    use super::*;
    use change_harness::domain::digest::Digest;

    /// Rewrites the one recorded convergence fact's `policy_digest` to a
    /// digest that was never configured and never retired, then commits
    /// the edit the way `Workspace::configure_convergence_policy` commits a
    /// control-repository edit (see its own comment): unlike
    /// `tamper_card_state` and `tamper_cycle_status`, which leave the tree
    /// dirty on purpose, a committed edit is what makes the next command
    /// read this corruption as real recorded history rather than an
    /// in-flight, uncommitted change.
    ///
    /// This is deliberately not the bypass #81 removed. #81 was about
    /// *configuration*: a value an operator should have been able to set
    /// through a real command and could not, so the harness was changed to
    /// accept it. This is *corruption*: no governed command has ever
    /// emitted, or could be made to emit, a convergence fact naming a
    /// digest nobody configured and nobody retired — `project`'s own fold
    /// refuses exactly that shape, with "fact names a foreign policy
    /// digest", before any command gets a chance to act on it. The only way
    /// to construct the input this card's tests exist to check is
    /// therefore to write it directly into the control repository, exactly
    /// as `support::Workspace::tamper_card_state` and `tamper_cycle_status`
    /// already do to simulate a transition or edit no command makes. It is
    /// not a shortcut around a command that should have accepted this
    /// value — no command ever will, or should; refusing it is the entire
    /// property this card exists to keep true.
    ///
    /// Panics unless exactly one convergence fact is recorded, so a
    /// fixture that silently produced zero, or more than one, cannot
    /// corrupt the wrong thing — or something in addition to what the
    /// calling test believes it corrupted — and so cannot quietly weaken
    /// every assertion built on top of it.
    fn corrupt_the_recorded_convergence_facts_policy_digest(workspace: &Workspace) {
        let events_dir = workspace.control.join("events");
        let mut corrupted = 0;
        for entry in fs::read_dir(&events_dir).unwrap() {
            let entry_path = entry.unwrap().path();
            if entry_path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let raw = fs::read_to_string(&entry_path).unwrap();
            let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
            let is_convergence_fact = value["event_type"] == "convergence.attempt_recorded"
                || value["event_type"] == "convergence.disposition_recorded";
            if is_convergence_fact {
                value["metadata"]["policy_digest"] = serde_json::json!(
                    Digest::of_bytes(b"a policy digest no rebaseline ever installed or retired")
                        .as_str()
                );
                fs::write(
                    &entry_path,
                    format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
                )
                .unwrap();
                corrupted += 1;
            }
        }
        assert_eq!(
            corrupted, 1,
            "the fixture must have produced exactly one convergence fact to corrupt"
        );
        support::git(&workspace.control, &["add", "-A"]);
        support::git(
            &workspace.control,
            &[
                "commit",
                "-q",
                "-m",
                "test: corrupt one convergence fact's policy_digest",
            ],
        );
    }

    /// Asserts the refusal every test in this module exists to pin: the
    /// exact `CH-INTERNAL-CONTROL-CORRUPT` code — never merely a non-zero
    /// exit, which an unrelated precondition failure would also satisfy —
    /// that the message names the exact `project` branch this fixture's
    /// corruption triggers, and that nothing was written.
    fn assert_refused_for_corruption(
        workspace: &Workspace,
        output: &std::process::Output,
        before_head: &str,
    ) {
        assert!(
            !output.status.success(),
            "a corrupt convergence fact must refuse, not read as an unspent budget: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(error_code(output), "CH-INTERNAL-CONTROL-CORRUPT");
        assert!(
            error_message(output).contains("fact names a foreign policy digest"),
            "the refusal must be the specific `project` branch this fixture's corruption \
             triggers, not some other malformed-fact reason: {}",
            error_message(output)
        );
        assert_eq!(
            workspace.control_head(),
            before_head,
            "a refusal must not write"
        );
    }

    #[test]
    fn a_corrupt_fact_refuses_card_status() {
        // The read path: `card.rs`'s `card_convergence`, called only by
        // `run_status`. A permissive policy (limit 3) rules out escalation
        // as an alternative explanation for the refusal below — one
        // recorded, then corrupted, fact is nowhere near exhausting it.
        let workspace = opened_with_policy(3, 3);
        let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
        let declaration = declaration_with_gate_failures(
            &workspace,
            "F-001",
            &head,
            "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
        );
        workspace.handoff(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);

        corrupt_the_recorded_convergence_facts_policy_digest(&workspace);

        let before_head = workspace.control_head();
        let output = workspace.card_raw(&["status", "--card-id", "F-001"]);
        assert_refused_for_corruption(&workspace, &output, &before_head);

        // The exact wrong answer this card exists to prevent: a lenient
        // edit that swallowed the projection error would report the
        // card's budget as unassessed instead of refusing to answer at
        // all.
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("legacy_unassessed"),
            "a corrupt fact must never be read as an unassessed, and so unspent, budget: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn a_corrupt_fact_refuses_cycle_status() {
        // `cycle.rs`'s `cycle_convergence`, `card_convergence`'s cycle-
        // level twin, called only by `run_status`. The corrupted fact
        // happens to be card-bound, which is deliberate: `project` refuses
        // the whole cycle's projection on any one malformed fact
        // regardless of whether that fact names a card, so this proves the
        // cycle read path shares the same fail-closed projection the card
        // read path does, not a separate check that only distrusts
        // cycle-bound facts.
        let workspace = opened_with_policy(3, 3);
        let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
        let declaration = declaration_with_gate_failures(
            &workspace,
            "F-001",
            &head,
            "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
        );
        workspace.handoff(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);

        corrupt_the_recorded_convergence_facts_policy_digest(&workspace);

        let before_head = workspace.control_head();
        let output = workspace.cycle_raw(&["status", "--cycle-id", "C-001"]);
        assert_refused_for_corruption(&workspace, &output, &before_head);

        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("legacy_unassessed"),
            "a corrupt fact must never be read as an unassessed, and so unspent, budget: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn a_corrupt_fact_refuses_a_card_advancing_command() {
        // `require_convergence_budget` (card.rs) is called from
        // `handoff create` and `review begin`/`review record` (review.rs:
        // 391, 410, 630, 789), `work start` and `work resume` (work.rs:
        // 406, 750, 826), and `card revise` (card.rs: 949, 1038) — grepped
        // directly across `src/`, not assumed. `handoff create` is used
        // here: it is the exact site `a_corrupt_convergence_fact_fails_
        // closed_instead_of_reading_as_unspent_budget` above already
        // exercises for the narrower property that the refusal is merely
        // "not escalated"; this pins the stronger one #85 requires — the
        // exact code — using a corruption built by mutating
        // `policy_digest` rather than blanking `evidence_ref`.
        let workspace = opened_with_policy(3, 3);
        open_review_round(&workspace, "F-001");
        let path = write_verdict(&workspace, "F-001", RETURN_WITH_ACCEPTANCE_DEFECT_REASON);
        workspace.review(&[
            "record",
            "--card-id",
            "F-001",
            "--verdict",
            &path,
            "--actor",
            "reviewer",
        ]);
        redeliver_candidate(&workspace, "F-001");

        corrupt_the_recorded_convergence_facts_policy_digest(&workspace);

        let before_head = workspace.control_head();
        let declaration = declaration_with_gate_failures(
            &workspace,
            "F-001",
            &support::capture(&workspace.worktrees.join("F-001"), &["rev-parse", "HEAD"]),
            "",
        );
        let output = workspace.handoff_raw(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);
        assert_refused_for_corruption(&workspace, &output, &before_head);
    }

    #[test]
    fn a_corrupt_fact_refuses_an_integration_advancing_command() {
        // `require_cycle_convergence_budget` (integration.rs) is called
        // from every governed path in `integration.rs` and `acceptance.rs`
        // that advances an integration toward promotion (its own doc
        // comment names them; `cycle_convergence_enforcement` above
        // already drives six of them under escalation). `integration
        // prepare` is used here, against F-002 — a second card, wholly
        // unrelated to F-001, approved *before* the corruption below so
        // its own approval path (which also reads this cycle's
        // convergence projection) does not itself trip the corruption.
        // F-002 has no conflict and nothing else standing in its way, so
        // it is a clean probe of exactly one thing: whether the corrupted
        // cycle-wide projection alone is what refuses it.
        let workspace = opened_with_policy(3, 3);
        let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
        let declaration = declaration_with_gate_failures(
            &workspace,
            "F-001",
            &head,
            "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
        );
        workspace.handoff(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);

        workspace.activate_card("F-002", &["docs/f002/**"]);
        workspace.approve_card("F-002", "docs/f002/a.md");

        corrupt_the_recorded_convergence_facts_policy_digest(&workspace);

        let before_head = workspace.control_head();
        let output = workspace.integration_raw(&[
            "prepare",
            "--cycle-id",
            "C-001",
            "--actor-id",
            "coordinator",
            "--card-id",
            "F-002",
        ]);
        assert_refused_for_corruption(&workspace, &output, &before_head);
    }

    #[test]
    fn a_corrupt_fact_refuses_a_disposition() {
        // The first of the five card-scoped disposition preflights —
        // `require_renewable`, `require_abandonable`,
        // `require_risk_acceptable`, `require_splittable`, and
        // `require_redesignable` — each pinned by its own test rather than
        // by one representative standing in for all five (see this
        // module's own top comment for why: a follow-up repair on this
        // card measured that the economy was a defect, since the five
        // blocks are byte-identical text that can drift independently). Every one
        // reads this cycle's events, calls `project`, and maps its
        // refusal to `InternalControlCorrupt` in byte-for-byte the same
        // shape (disposition.rs:420-433, 1048-1061, 1293-1306, 1596-1609,
        // 1942-1955), before it reaches its own dimension, authorization,
        // or lifecycle checks — none of which this fixture even sets up
        // (no final-authorization policy is configured, and F-001 is not
        // escalated): the corrupt projection refuses before any of that
        // would matter, which is itself part of what every test in this
        // group proves. This one exercises `disposition abandon`; its
        // four siblings immediately below exercise `renew`, `accept-risk`,
        // `split`, and `redesign` against the identical fixture.
        let workspace = opened_with_policy(3, 3);
        let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
        let declaration = declaration_with_gate_failures(
            &workspace,
            "F-001",
            &head,
            "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
        );
        workspace.handoff(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);

        corrupt_the_recorded_convergence_facts_policy_digest(&workspace);

        let before_head = workspace.control_head();
        let output = Workspace::run(&[
            "disposition".to_owned(),
            "abandon".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
            "--card-id".to_owned(),
            "F-001".to_owned(),
            "--rationale".to_owned(),
            "attempting to abandon under a corrupted projection".to_owned(),
        ]);
        assert_refused_for_corruption(&workspace, &output, &before_head);
    }

    #[test]
    fn a_corrupt_fact_refuses_a_disposition_renew() {
        // `require_renewable`'s own copy of the shared block
        // (disposition.rs:420-433) — see `a_corrupt_fact_refuses_a_disposition`'s
        // comment for why this is pinned on its own rather than assumed
        // identical to that test's `abandon` coverage. Identical fixture:
        // no final-authorization policy configured, F-001 not escalated —
        // the corrupt projection must refuse before either check, or
        // `require_renewable`'s own dimension check, would matter.
        let workspace = opened_with_policy(3, 3);
        let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
        let declaration = declaration_with_gate_failures(
            &workspace,
            "F-001",
            &head,
            "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
        );
        workspace.handoff(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);

        corrupt_the_recorded_convergence_facts_policy_digest(&workspace);

        let before_head = workspace.control_head();
        let output = Workspace::run(&[
            "disposition".to_owned(),
            "renew".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
            "--card-id".to_owned(),
            "F-001".to_owned(),
            "--dimension".to_owned(),
            "gate-failures".to_owned(),
            "--rationale".to_owned(),
            "attempting to renew under a corrupted projection".to_owned(),
        ]);
        assert_refused_for_corruption(&workspace, &output, &before_head);
    }

    #[test]
    fn a_corrupt_fact_refuses_a_disposition_accept_risk() {
        // `require_risk_acceptable`'s own copy of the shared block
        // (disposition.rs:1293-1306) — see `a_corrupt_fact_refuses_a_disposition`'s
        // comment for why this is pinned on its own. Identical fixture:
        // no final-authorization policy configured, F-001 not escalated —
        // the corrupt projection must refuse before either check, or the
        // "already accepted" / dimension-exhausted checks that read the
        // same projection, would matter.
        let workspace = opened_with_policy(3, 3);
        let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
        let declaration = declaration_with_gate_failures(
            &workspace,
            "F-001",
            &head,
            "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
        );
        workspace.handoff(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);

        corrupt_the_recorded_convergence_facts_policy_digest(&workspace);

        let before_head = workspace.control_head();
        let output = Workspace::run(&[
            "disposition".to_owned(),
            "accept-risk".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
            "--card-id".to_owned(),
            "F-001".to_owned(),
            "--dimension".to_owned(),
            "gate-failures".to_owned(),
            "--risk".to_owned(),
            "acting on a corrupted projection".to_owned(),
            "--rationale".to_owned(),
            "attempting to accept risk under a corrupted projection".to_owned(),
        ]);
        assert_refused_for_corruption(&workspace, &output, &before_head);
    }

    #[test]
    fn a_corrupt_fact_refuses_a_disposition_split() {
        // `require_splittable`'s own copy of the shared block
        // (disposition.rs:1596-1609) — see `a_corrupt_fact_refuses_a_disposition`'s
        // comment for why this is pinned on its own. Identical fixture:
        // no final-authorization policy configured, F-001 not escalated.
        //
        // `--follow-up-card-id` names F-999, a card that is never created
        // in this fixture, on purpose: `require_splittable`'s check order
        // (disposition.rs:1563-1753) reads this cycle's projection —
        // where the corruption lives — before it ever loads the follow-up
        // card (check 6, after the shared block). A nonexistent follow-up
        // card is therefore a stronger proof than a real one: if the
        // corrupt-projection check did not fire first, this would fail
        // with `CH-PRECONDITION-NOT-FOUND` (a missing card), not silently
        // pass for the wrong reason — exactly the "different check fired"
        // failure mode this test exists to rule out.
        let workspace = opened_with_policy(3, 3);
        let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
        let declaration = declaration_with_gate_failures(
            &workspace,
            "F-001",
            &head,
            "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
        );
        workspace.handoff(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);

        corrupt_the_recorded_convergence_facts_policy_digest(&workspace);

        let before_head = workspace.control_head();
        let output = Workspace::run(&[
            "disposition".to_owned(),
            "split".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
            "--card-id".to_owned(),
            "F-001".to_owned(),
            "--dimension".to_owned(),
            "gate-failures".to_owned(),
            "--follow-up-card-id".to_owned(),
            "F-999".to_owned(),
            "--rationale".to_owned(),
            "attempting to split under a corrupted projection".to_owned(),
        ]);
        assert_refused_for_corruption(&workspace, &output, &before_head);
    }

    #[test]
    fn a_corrupt_fact_refuses_a_disposition_redesign() {
        // `require_redesignable`'s own copy of the shared block
        // (disposition.rs:1942-1955) — see `a_corrupt_fact_refuses_a_disposition`'s
        // comment for why this is pinned on its own. Identical fixture:
        // no final-authorization policy configured, F-001 not escalated.
        //
        // `--replacement-card-id` names F-999, a card that is never
        // created in this fixture, for the same reason `split`'s test
        // above leaves its follow-up card uncreated: `require_redesignable`'s
        // check order (disposition.rs:1906-2048) reads this cycle's
        // projection before it ever loads the replacement card (check 4,
        // after the shared block), so a nonexistent replacement is a
        // stronger proof than a real one — see that test's own comment.
        let workspace = opened_with_policy(3, 3);
        let head = ready_candidate(&workspace, "F-001", &["gate.unit"]);
        let declaration = declaration_with_gate_failures(
            &workspace,
            "F-001",
            &head,
            "gate_failures:\n  - gate_id: gate.unit\n    reason_category: regression\n",
        );
        workspace.handoff(&[
            "create",
            "--card-id",
            "F-001",
            "--declaration",
            &declaration,
        ]);

        corrupt_the_recorded_convergence_facts_policy_digest(&workspace);

        let before_head = workspace.control_head();
        let output = Workspace::run(&[
            "disposition".to_owned(),
            "redesign".to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            workspace.control.display().to_string(),
            "--card-id".to_owned(),
            "F-001".to_owned(),
            "--replacement-card-id".to_owned(),
            "F-999".to_owned(),
            "--rationale".to_owned(),
            "attempting to redesign under a corrupted projection".to_owned(),
        ]);
        assert_refused_for_corruption(&workspace, &output, &before_head);
    }
}

// #179 review repair: `disposition_renew`'s two tests above pin group 1 and
// group 2's constants against `disposition.rs` sites, but that is only one
// of the three files #179 counted real construction sites in.
// `src/commands/acceptance.rs`'s own doc comment (right above its five
// `*_RECOVERY` constants) credits `integration.rs` with 9 of the 29 sites,
// spread across all five groups — and nothing anywhere pinned any of them:
// a site could be reassigned to the wrong group's constant and every
// existing test (`tests/policy_not_accepted_recovery.rs`'s content
// `.contains()` checks, `tests/policy_not_accepted_coverage.rs`'s "has *a*
// recovery" structural scan, `tests/recovery_override_text.rs`'s
// command-name scan) would keep passing, because none of them compares a
// specific site's *actual* text against the *specific* constant its group
// requires. Confirmed by mutation: reassigning `integration.rs:1911`
// (`validate_exception_authorizer`'s own "no policy" branch) from group 1's
// constant to group 4's changed nothing observable in the full suite.
//
// This module closes that gap the same way `disposition_renew` closes it
// for `disposition.rs`: drive the real CLI to each site and compare its
// `recovery` field against a byte-for-byte local copy of the constant that
// site's group requires. The constants live in `src/commands/acceptance.rs`
// as `pub(crate)`, unreachable from this external test crate, so they are
// copied verbatim here — exactly as `disposition_renew`'s own copies are,
// for the identical reason its own comment gives.
//
// # Coverage: 8 of `integration.rs`'s 9 sites, one test each
//
//   1643 `exceptions_for`               group 1 -- acceptance_record_with_a_pending_exception_after_the_policy_is_removed_gets_group_1s_recovery
//   1694 `exceptions_for`               group 4 -- acceptance_record_after_the_plan_changes_with_a_pending_exception_gets_group_4s_recovery
//   1875 `exception_bindings`           group 1 -- exception_raise_with_no_policy_at_all_gets_group_1s_recovery
//   1897 `exception_bindings`           group 3 -- exception_raise_after_the_cycle_reseals_gets_group_3s_recovery
//   1911 `validate_exception_authorizer` group 1 -- left; see below
//   1922 `validate_exception_authorizer` group 2 -- exception_resolve_by_an_unauthorized_actor_gets_group_2s_recovery
//   1968 `run_exception_raise`          group 5 -- exception_raise_with_a_disabled_trigger_gets_group_5s_recovery
//   3924 `check_promotion` (blocks_promotion_of) group 4 -- promote_after_the_acceptance_is_tampered_to_rejected_gets_group_4s_recovery
//   3941 `check_promotion` (digest mismatch)     group 4 -- promote_after_the_plan_changes_post_acceptance_gets_group_4s_recovery
//
// `validate_exception_authorizer`'s own "no policy" branch (1911) is left
// deliberately, and for a stronger reason than "no fixture reaches it
// without inventing one" — the standard this card sets, and the reason
// `acceptance.rs`'s own two v1/v2 schema-mismatch sites in
// `validate_final_authorization_for_promotion` go untested for recovery
// content by every #179 test file. Line 1911 is provably dead code given
// the current call graph, reachable by no fixture at all, invented or
// otherwise: `validate_exception_authorizer` has exactly one call site
// (`run_exception_resolve`'s `validate` closure, `src/commands/
// integration.rs:2033`), and it always runs immediately after
// `exception_bindings` (line 2032) on that same call's own `config`
// binding. `exception_bindings` already refuses — with group 1's own
// constant, at its own line 1875 — whenever `config.
// final_authorization_policy` is `None`, so by the time `validate_
// exception_authorizer` runs at all on that same `config`, the field has
// already been proven `Some`. Its own `.ok_or_else` at line 1911 has no
// path left to take. Confirmed empirically, not just by reading: the
// review's mutation at 1911 (swapping its `recovery:` reference from group
// 1's constant to group 4's) left `cargo test` unchanged, and it stays
// unchanged after every test this module adds, because none of them — nor
// any fixture reachable from the CLI — can execute that line. Reaching it
// at all would mean calling the private function directly, which this
// external test crate cannot do (it is not even `pub(crate)`), and doing
// so would be inventing a second test mechanism in place of the one this
// module (and `disposition_renew` before it) already uses.
mod policy_not_accepted_integration_groups {
    use super::*;

    /// `FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY`
    /// (`src/commands/acceptance.rs`), copied verbatim.
    const POLICY_NOT_CONFIGURED_RECOVERY: &str = "`final_authorization_policy` is not configured for this project (or was removed since an earlier check relied on it); run `project example-final-authorization` for a complete, valid document, then install one with `project set-final-authorization-policy`.";

    /// `FINAL_AUTHORIZATION_ACTOR_NOT_AUTHORIZED_RECOVERY`
    /// (`src/commands/acceptance.rs`), copied verbatim.
    const ACTOR_NOT_AUTHORIZED_RECOVERY: &str = "This actor is not among `final_authorization_policy.authorizer_actor_ids`; run `project example-final-authorization` to see a configured policy's shape, then retry as one of the listed actors or add this one with `project set-final-authorization-policy`.";

    /// `FINAL_INTEGRATION_SEAL_STALE_RECOVERY`
    /// (`src/commands/acceptance.rs`), copied verbatim.
    const SEAL_STALE_RECOVERY: &str = "This final integration's sealed-cycle binding no longer matches the cycle; run `integration prepare --final` again for this cycle, then retry.";

    /// `FINAL_AUTHORIZATION_STALE_RECOVERY`
    /// (`src/commands/acceptance.rs`), copied verbatim.
    const STALE_RECOVERY: &str = "The reason above names what changed. What was recorded no longer covers the current landing commit, plan, or policy — record a fresh decision against the current state with `acceptance record`, or, for an exception, `integration exception raise`.";

    /// `EXCEPTION_TRIGGER_NOT_ENABLED_RECOVERY`
    /// (`src/commands/acceptance.rs`), copied verbatim.
    const TRIGGER_NOT_ENABLED_RECOVERY: &str = "This trigger is not among `final_authorization_policy.exception_triggers`; add it with `project set-final-authorization-policy`, or raise a trigger the policy already declares.";

    /// The `recovery` field of a coded JSON error envelope.
    fn recovery_of(output: &std::process::Output) -> String {
        let envelope: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("an error envelope");
        envelope["error"]["recovery"]
            .as_str()
            .expect("a coded refusal carries a recovery string")
            .to_owned()
    }

    /// Like `support::Workspace::initialized`, but also installs a
    /// final-authorization policy through the governed
    /// `project set-final-authorization-policy` naming `authorizers` and
    /// enabling `triggers`. `support::Workspace` is outside this card's
    /// file scope and its `initialized` helper does not accept this, so
    /// this mirrors `disposition_renew`'s own `initialized_with_
    /// authorizers` and adds `exception_triggers` — installed the
    /// governed way `tests/exceptions.rs`'s own comment on this exact
    /// field requires ("the bypass this section exists to remove").
    fn initialized_with_final_policy(authorizers: &[&str], triggers: &[&str]) -> Workspace {
        let workspace = Workspace::initialized();
        let policy = serde_json::json!({
            "version": "harness.final-authorization-policy/v1",
            "authorization_unit": "sealed_cycle",
            "authorizer_actor_ids": authorizers,
            "exception_triggers": triggers,
        });
        let path = workspace.root.join("final-authorization-policy.json");
        fs::write(&path, serde_json::to_string_pretty(&policy).unwrap()).unwrap();
        let output = Workspace::run(&[
            "project".into(),
            "set-final-authorization-policy".into(),
            "--control".into(),
            workspace.control.display().to_string(),
            "--policy".into(),
            path.display().to_string(),
            "--actor".into(),
            "operator".into(),
            "--output".into(),
            "json".into(),
        ]);
        assert!(
            output.status.success(),
            "installing the final-authorization policy failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        workspace
    }

    /// Drives one card through a sealed final cycle to a reviewed final
    /// integration — the shared precondition `exception_bindings` (and
    /// everything downstream of it) requires. Mirrors `tests/
    /// policy_not_accepted_recovery.rs`'s own `reviewed_final` exactly,
    /// kept as a local copy for the same file-scope reason every sibling
    /// copy of this fixture in this suite gives.
    fn reviewed_final(workspace: &Workspace) -> String {
        workspace.cycle(&[
            "create",
            "--cycle-id",
            "C-001",
            "--objective",
            "integration group fixture",
        ]);
        workspace.cycle(&["activate", "--cycle-id", "C-001"]);
        workspace.activate_card("F-001", &["src/**"]);
        workspace.approve_card("F-001", "src/a.rs");
        workspace.cycle(&["seal", "--cycle-id", "C-001"]);
        let id = workspace.integration_json(&[
            "prepare",
            "--cycle-id",
            "C-001",
            "--actor-id",
            "coordinator",
            "--final",
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
        id
    }

    /// Drives one card through an ordinary (non-final) cycle to an
    /// accepted v1 integration. `check_promotion`'s `blocks_promotion_of`
    /// and integration-digest checks (group 4, sites 3924 and 3941) apply
    /// identically whether or not `final_for_cycle` is set — confirmed by
    /// reading `check_promotion` itself, which gates neither check behind
    /// it — so neither test that uses this needs a final-authorization
    /// policy at all.
    fn accepted_v1(workspace: &Workspace) -> String {
        workspace.cycle(&[
            "create",
            "--cycle-id",
            "C-001",
            "--objective",
            "integration group fixture",
        ]);
        workspace.cycle(&["activate", "--cycle-id", "C-001"]);
        workspace.activate_card("F-001", &["src/**"]);
        workspace.approve_card("F-001", "src/a.rs");
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
            "--authorizer-actor-id",
            "owner",
        ]);
        id
    }

    /// Raises one exception, asserting success, and returns its event id.
    fn raise_exception(
        workspace: &Workspace,
        integration_id: &str,
        trigger: &str,
        actor: &str,
    ) -> String {
        let output = workspace.integration_json(&[
            "exception",
            "raise",
            "--integration-id",
            integration_id,
            "--actor-id",
            actor,
            "--trigger",
            trigger,
            "--evidence-ref",
            "receipt:R-001",
        ]);
        output["data"]["exception_event_id"]
            .as_str()
            .expect("a raised exception's event id")
            .to_owned()
    }

    /// Rewrites cycle `cycle_id`'s own `objective` field directly on disk,
    /// without going through any governed command or commit — the same
    /// "edit made outside the governed path" technique `support::
    /// Workspace::tamper_cycle_status` uses for its sibling field
    /// (`tests/policy_not_accepted_recovery.rs`'s own `tamper_cycle_
    /// objective` gives the full rationale), applied here to move the
    /// cycle's canonical digest without touching `status`.
    fn tamper_cycle_objective(workspace: &Workspace, cycle_id: &str, objective: &str) {
        let path = workspace.control.join(format!("cycles/{cycle_id}.json"));
        let raw = fs::read_to_string(&path).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["objective"] = serde_json::json!(objective);
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
        )
        .unwrap();
    }

    /// Removes a previously installed `final_authorization_policy` from
    /// `project/project.json` directly, simulating "configured once and is
    /// gone by the time a later recheck runs" — the parenthetical
    /// `FINAL_AUTHORIZATION_POLICY_NOT_CONFIGURED_RECOVERY` itself names.
    /// No governed command can do this: `SetFinalAuthorizationPolicyArgs`
    /// (`src/commands/project.rs`) takes a required `--policy` path with
    /// no way to clear a policy already set, so this mirrors `tests/
    /// promotion.rs`'s own `.remove("final_authorization_policy")`
    /// technique rather than inventing a new one.
    fn remove_final_authorization_policy(workspace: &Workspace) {
        let path = workspace.control.join("project/project.json");
        let mut project: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        project
            .as_object_mut()
            .unwrap()
            .remove("final_authorization_policy");
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&project).unwrap()),
        )
        .unwrap();
    }

    /// Rewrites integration `integration_id`'s own `prepared_by` field
    /// directly on disk — a field `IntegrationRecord::substantive_digest`
    /// covers but no earlier check in either path below reads, so it moves
    /// the plan's digest in isolation. Mirrors `tests/promotion.rs`'s
    /// `final_authorization_refuses_promotion_after_the_substantive_plan_
    /// changes`, which already proves this exact edit reaches `check_
    /// promotion`'s digest comparison.
    fn tamper_integration_prepared_by(
        workspace: &Workspace,
        integration_id: &str,
        prepared_by: &str,
    ) {
        let path = workspace
            .control
            .join(format!("integrations/{integration_id}.json"));
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record["prepared_by"] = serde_json::json!(prepared_by);
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&record).unwrap()),
        )
        .unwrap();
    }

    /// Rewrites acceptance `acceptance_id`'s own `decision` field directly
    /// on disk, decoupling it from the integration's own `status` (which
    /// only a real `acceptance record` call moves, and stays `accepted`
    /// regardless of this edit). That isolation is the point: a *real*
    /// rejection leaves `status` at `reviewed` (proven by `tests/
    /// promotion.rs`'s own `a_rejection_is_recorded_and_blocks_
    /// promotion`), which would trip `check_promotion`'s earlier status
    /// gate before ever reaching `blocks_promotion_of`'s decision check —
    /// exactly the site this test needs to isolate.
    fn tamper_acceptance_decision(workspace: &Workspace, acceptance_id: &str, decision: &str) {
        let path = workspace
            .control
            .join(format!("acceptances/{acceptance_id}.json"));
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record["decision"] = serde_json::json!(decision);
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&record).unwrap()),
        )
        .unwrap();
    }

    // ---------------------------------------------------------------
    // Group 1: no policy configured at all.
    // ---------------------------------------------------------------

    #[test]
    fn exception_raise_with_no_policy_at_all_gets_group_1s_recovery() {
        let workspace = Workspace::initialized();
        let id = reviewed_final(&workspace);
        // Deliberately never installs a final-authorization policy.

        let output = workspace.integration_raw(&[
            "exception",
            "raise",
            "--integration-id",
            &id,
            "--actor-id",
            "owner",
            "--trigger",
            "policy_change",
            "--evidence-ref",
            "receipt:R-001",
        ]);
        assert!(
            !output.status.success(),
            "raising an exception with no final-authorization policy configured must refuse"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert!(
            error_message(&output)
                .contains("final authorization policy does not declare exception triggers"),
            "not exercising `exception_bindings`'s own policy-missing site; got: {}",
            error_message(&output)
        );
        assert_eq!(
            recovery_of(&output),
            POLICY_NOT_CONFIGURED_RECOVERY,
            "integration.rs:1875 (`exception_bindings`) shares group 1 and must carry \
             byte-identical text to acceptance.rs's own copy of the same constant"
        );
    }

    #[test]
    fn acceptance_record_with_a_pending_exception_after_the_policy_is_removed_gets_group_1s_recovery()
     {
        let workspace = initialized_with_final_policy(&["owner"], &["policy_change"]);
        let id = reviewed_final(&workspace);
        raise_exception(&workspace, &id, "policy_change", "owner");
        remove_final_authorization_policy(&workspace);

        let output = workspace.acceptance_raw(&[
            "record",
            "--integration-id",
            &id,
            "--authorizer-actor-id",
            "owner",
        ]);
        assert!(
            !output.status.success(),
            "recording an acceptance with a pending exception and no policy configured must refuse"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert!(
            error_message(&output)
                .contains("final authorization policy is not configured for exception validation"),
            "not exercising `exceptions_for`'s own policy-missing site; got: {}",
            error_message(&output)
        );
        assert_eq!(
            recovery_of(&output),
            POLICY_NOT_CONFIGURED_RECOVERY,
            "integration.rs:1643 (`exceptions_for`) shares group 1 and must carry byte-identical \
             text to acceptance.rs's own copy of the same constant"
        );
    }

    // ---------------------------------------------------------------
    // Group 2: a policy exists, but this actor is not among its
    // `authorizer_actor_ids`.
    // ---------------------------------------------------------------

    #[test]
    fn exception_resolve_by_an_unauthorized_actor_gets_group_2s_recovery() {
        let workspace = initialized_with_final_policy(&["owner"], &["policy_change"]);
        let id = reviewed_final(&workspace);
        let event_id = raise_exception(&workspace, &id, "policy_change", "owner");

        let output = workspace.integration_raw(&[
            "exception",
            "resolve",
            "--integration-id",
            &id,
            "--exception-event-id",
            &event_id,
            "--authorizer-actor-id",
            "intern",
        ]);
        assert!(
            !output.status.success(),
            "resolving an exception as an actor absent from authorizer_actor_ids must refuse"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert!(
            error_message(&output)
                .contains("is not configured to resolve final integration exceptions"),
            "not exercising `validate_exception_authorizer`'s own actor-check site; got: {}",
            error_message(&output)
        );
        assert_eq!(
            recovery_of(&output),
            ACTOR_NOT_AUTHORIZED_RECOVERY,
            "integration.rs:1922 (`validate_exception_authorizer`) shares group 2 and must carry \
             byte-identical text to acceptance.rs's own copy of the same constant"
        );
    }

    // ---------------------------------------------------------------
    // Group 3: this final integration's own binding is stale, before any
    // decision is recorded.
    // ---------------------------------------------------------------

    #[test]
    fn exception_raise_after_the_cycle_reseals_gets_group_3s_recovery() {
        let workspace = initialized_with_final_policy(&["owner"], &["policy_change"]);
        let id = reviewed_final(&workspace);
        tamper_cycle_objective(&workspace, "C-001", "resealed with different content");

        let output = workspace.integration_raw(&[
            "exception",
            "raise",
            "--integration-id",
            &id,
            "--actor-id",
            "owner",
            "--trigger",
            "policy_change",
            "--evidence-ref",
            "receipt:R-001",
        ]);
        assert!(
            !output.status.success(),
            "raising an exception against a final integration whose sealed-cycle binding no \
             longer matches its cycle must refuse"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert!(
            error_message(&output).contains("no longer binds its sealed cycle"),
            "not exercising `exception_bindings`'s own seal-staleness site; got: {}",
            error_message(&output)
        );
        assert_eq!(
            recovery_of(&output),
            SEAL_STALE_RECOVERY,
            "integration.rs:1897 (`exception_bindings`) shares group 3 and must carry \
             byte-identical text to acceptance.rs's own copy of the same constant"
        );
    }

    // ---------------------------------------------------------------
    // Group 4: an existing decision no longer covers the current state.
    // ---------------------------------------------------------------

    #[test]
    fn acceptance_record_after_the_plan_changes_with_a_pending_exception_gets_group_4s_recovery() {
        let workspace = initialized_with_final_policy(&["owner"], &["policy_change"]);
        let id = reviewed_final(&workspace);
        raise_exception(&workspace, &id, "policy_change", "owner");
        tamper_integration_prepared_by(&workspace, &id, "tampered-coordinator");

        let output = workspace.acceptance_raw(&[
            "record",
            "--integration-id",
            &id,
            "--authorizer-actor-id",
            "owner",
        ]);
        assert!(
            !output.status.success(),
            "recording an acceptance once the plan has changed under a pending exception must \
             refuse"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert!(
            error_message(&output).contains("no longer binds final integration"),
            "not exercising `exceptions_for`'s own digest-mismatch site; got: {}",
            error_message(&output)
        );
        assert_eq!(
            recovery_of(&output),
            STALE_RECOVERY,
            "integration.rs:1694 (`exceptions_for`) shares group 4 and must carry byte-identical \
             text to acceptance.rs's own copy of the same constant"
        );
    }

    #[test]
    fn promote_after_the_acceptance_is_tampered_to_rejected_gets_group_4s_recovery() {
        let workspace = Workspace::initialized();
        let id = accepted_v1(&workspace);
        tamper_acceptance_decision(&workspace, "ACC-000001", "rejected");

        let output = workspace.integration_raw(&[
            "promote",
            "--integration-id",
            &id,
            "--actor-id",
            "promoter",
        ]);
        assert!(
            !output.status.success(),
            "promoting an integration whose only recorded acceptance now reads `rejected` must \
             refuse"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert!(
            error_message(&output).contains("promotion is not authorized"),
            "not exercising `check_promotion`'s own `blocks_promotion_of` site; got: {}",
            error_message(&output)
        );
        assert_eq!(
            recovery_of(&output),
            STALE_RECOVERY,
            "integration.rs:3924 (`check_promotion`) shares group 4 and must carry byte-identical \
             text to acceptance.rs's own copy of the same constant"
        );
    }

    #[test]
    fn promote_after_the_plan_changes_post_acceptance_gets_group_4s_recovery() {
        let workspace = Workspace::initialized();
        let id = accepted_v1(&workspace);
        tamper_integration_prepared_by(&workspace, &id, "tampered-coordinator");

        let output = workspace.integration_raw(&[
            "promote",
            "--integration-id",
            &id,
            "--actor-id",
            "promoter",
        ]);
        assert!(
            !output.status.success(),
            "promoting once the accepted plan has changed must refuse"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert!(
            error_message(&output).contains("the plan changed after it was accepted"),
            "not exercising `check_promotion`'s own integration-digest site; got: {}",
            error_message(&output)
        );
        assert_eq!(
            recovery_of(&output),
            STALE_RECOVERY,
            "integration.rs:3941 (`check_promotion`) shares group 4 and must carry byte-identical \
             text to acceptance.rs's own copy of the same constant"
        );
    }

    // ---------------------------------------------------------------
    // Group 5: a policy exists, but this specific exception trigger is not
    // among `exception_triggers`.
    // ---------------------------------------------------------------

    #[test]
    fn exception_raise_with_a_disabled_trigger_gets_group_5s_recovery() {
        let workspace = initialized_with_final_policy(&["owner"], &["policy_change"]);
        let id = reviewed_final(&workspace);

        let output = workspace.integration_raw(&[
            "exception",
            "raise",
            "--integration-id",
            &id,
            "--actor-id",
            "owner",
            "--trigger",
            "critical_residual_risk",
            "--evidence-ref",
            "receipt:R-001",
        ]);
        assert!(
            !output.status.success(),
            "raising a trigger the policy does not declare must refuse"
        );
        assert_eq!(error_code(&output), "CH-POLICY-NOT-ACCEPTED");
        assert!(
            error_message(&output).contains("is not enabled by final authorization policy"),
            "not exercising `run_exception_raise`'s own trigger-not-enabled site; got: {}",
            error_message(&output)
        );
        // `tests/policy_not_accepted_recovery.rs`'s own
        // `exception_trigger_not_enabled_names_exception_triggers_field`
        // already checks this site's recovery *content* (`.contains`); this
        // adds the stronger byte-identical *identity* check the rest of
        // this module gives every other site, so group 5 is not the one
        // group left with presence-only coverage.
        assert_eq!(
            recovery_of(&output),
            TRIGGER_NOT_ENABLED_RECOVERY,
            "integration.rs:1968 (`run_exception_raise`) is group 5's only site and must carry \
             byte-identical text to acceptance.rs's own copy of the same constant"
        );
    }
}
