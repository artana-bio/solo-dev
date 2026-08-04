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
const RETURN_WITH_ACCEPTANCE_DEFECT_REASON: &str = "reviewer_actor_id: reviewer\ndecision: changes_requested\nreason_category: acceptance_defect\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: something is wrong\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n";

/// A `changes_requested` verdict with no `reason_category` declared at all.
const RETURN_WITH_NO_REASON: &str = "reviewer_actor_id: reviewer\ndecision: changes_requested\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: something is wrong\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n";

/// A `changes_requested` verdict declaring `reason_category: scope_change`,
/// which is `MaterialScopeRevision`'s reason, not a review return's.
const RETURN_WITH_SCOPE_CHANGE_REASON: &str = "reviewer_actor_id: reviewer\ndecision: changes_requested\nreason_category: scope_change\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: something is wrong\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n";

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
const RETURN_WITH_REGRESSION_REASON_FOR_HANDOFF: &str = "reviewer_actor_id: reviewer\ndecision: changes_requested\nreason_category: regression\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: something is wrong\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n";

/// A `changes_requested` verdict declaring `reason_category:
/// non_blocking_improvement` — admissible for a review return, but not for
/// the repair attempt that answers it.
const RETURN_WITH_NON_BLOCKING_REASON_FOR_HANDOFF: &str = "reviewer_actor_id: reviewer\ndecision: changes_requested\nreason_category: non_blocking_improvement\nfindings:\n  - severity: medium\n    location: src/a.rs\n    detail: something is wrong\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed directly\nresidual_risks: []\n";

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
