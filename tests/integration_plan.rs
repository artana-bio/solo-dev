//! `WP-410` acceptance: integration selection and dependency order.
//!
//! `SPIKE-001` finding F-3 is the reason `integration ready` is tested as hard
//! as `prepare`. The spike's failure was not that integration went wrong — it
//! was that nothing could say what was waiting for it.

mod support;

use std::fs;

use support::Workspace;

/// A cycle with `count` cards, all activated against the cycle baseline.
fn cycle_with(count: usize) -> Workspace {
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
    workspace
}

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

/// The card identifiers listed as ready, in report order.
fn ready_ids(envelope: &serde_json::Value) -> Vec<String> {
    envelope["data"]["ready"]
        .as_array()
        .expect("a ready list")
        .iter()
        .map(|entry| entry["card_id"].as_str().unwrap().to_owned())
        .collect()
}

/// The card identifiers in an integration's merge order.
fn merge_order(envelope: &serde_json::Value) -> Vec<String> {
    envelope["data"]["members"]
        .as_array()
        .expect("a member list")
        .iter()
        .map(|member| member["card_id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn an_approved_card_appears_in_the_ready_view() {
    let workspace = cycle_with(1);
    workspace.approve_card("F-001", "src/F-001/a.rs");

    let envelope = workspace.integration_json(&["ready", "--cycle-id", "C-001"]);
    assert_eq!(ready_ids(&envelope), ["F-001"]);
    let entry = &envelope["data"]["ready"][0];
    assert_eq!(entry["reviewer_actor_id"], "reviewer-session");
    assert!(
        entry["candidate_sha"].is_string(),
        "the exact approved candidate must be named: {entry}"
    );
}

#[test]
fn a_card_that_is_not_approved_is_listed_with_its_reason() {
    let workspace = cycle_with(2);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    // F-002 is activated but never worked.

    let envelope = workspace.integration_json(&["ready", "--cycle-id", "C-001"]);
    assert_eq!(ready_ids(&envelope), ["F-001"]);

    let waiting = envelope["data"]["not_ready"]
        .as_array()
        .expect("a not-ready list");
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0]["card_id"], "F-002");
    assert_eq!(waiting[0]["state"], "ready");
    assert!(
        waiting[0]["reason"]
            .as_str()
            .unwrap()
            .contains("not `approved`"),
        "a card must be told why it is not integrable: {}",
        waiting[0]
    );
}

#[test]
fn an_invalidated_approval_is_reported_as_stale_rather_than_absent() {
    let workspace = cycle_with(1);
    workspace.approve_card("F-001", "src/F-001/a.rs");

    // Section 15.2: moving the candidate voids the approval. The card is still
    // `approved`, which is exactly the trap — the state alone lies.
    let worktree = workspace.worktrees.join("F-001");
    fs::write(worktree.join("src/F-001/b.rs"), "// more\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: more"]);

    let envelope = workspace.integration_json(&["ready", "--cycle-id", "C-001"]);
    assert!(ready_ids(&envelope).is_empty());
    let waiting = &envelope["data"]["not_ready"][0];
    assert_eq!(waiting["state"], "approved");
    assert!(
        waiting["reason"]
            .as_str()
            .unwrap()
            .contains("no longer describes"),
        "unexpected reason: {waiting}"
    );
}

/// Drives a card to a non-approval verdict recorded *after* the branch moved.
///
/// `F-028` made this reachable on purpose: a verdict that found problems is a
/// true statement about the candidate it was reached against, and refusing to
/// file it once the branch moves destroys the reviewer's work. What `F-028`
/// then declared as one of its own acceptance regressions — that a card left
/// this way stays out of integration — nothing in the durable suite checked.
/// That card's reviewer verified it by hand across eleven scenarios;
/// `artana-bio/solo-dev#15` item 2 is the gap that left behind.
fn left_non_approved_by_a_stale_verdict(workspace: &Workspace, card_id: &str, decision: &str) {
    workspace.work(&["start", "--card-id", card_id]);

    let worktree = workspace.worktrees.join(card_id);
    let path = worktree.join(format!("src/{card_id}/a.rs"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("// {card_id}\n")).unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: reviewed"]);
    workspace.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join(format!("{card_id}-declaration.yaml"));
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: adds a.rs\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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
    workspace.review(&["begin", "--card-id", card_id]);

    // The branch moves while the reviewer is still reading it.
    fs::write(worktree.join(format!("src/{card_id}/b.rs")), "// more\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: moved on"]);
    let moved = support::capture(&worktree, &["rev-parse", "HEAD"]);
    assert_ne!(
        head, moved,
        "the fixture has to actually move the branch, or these tests are \
         about an ordinary non-approval and say nothing about F-028"
    );

    let verdict = workspace.root.join(format!("{card_id}-verdict.yaml"));
    fs::write(
        &verdict,
        format!(
            "reviewer_actor_id: reviewer-session\ndecision: {decision}\nfindings:\n  - severity: high\n    location: src/{card_id}/a.rs\n    detail: the candidate has a defect\n    disposition: open\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: exempt\n    reason: fixture verdict for unrelated review behavior; no mutation performed\nresidual_risks: []\n"
        ),
    )
    .unwrap();
    workspace.review(&[
        "record",
        "--card-id",
        card_id,
        "--verdict",
        &verdict.display().to_string(),
    ]);

    // And the verdict really is bound to the candidate that was read, not to
    // the branch as it now stands. Without this the fixture proves only that
    // a non-approved card is excluded, which `preparing_selects_only_approved_
    // candidates` already covers — deleting the three lines above left every
    // test here green, which is how the first version of this shipped.
    let recorded = workspace.review_json(&["inspect", "--card-id", card_id]);
    let last = recorded["data"]["reviews"]
        .as_array()
        .expect("a review list")
        .last()
        .expect("the verdict just recorded");
    assert_eq!(
        last["candidate_sha"], head,
        "the review names the candidate it read"
    );
    assert_ne!(
        last["candidate_sha"], moved,
        "which is no longer what the branch holds"
    );
}

/// Asserts a card is absent from `ready`, named in `not_ready` with a reason,
/// and refused by `prepare` — the three surfaces that together have to agree
/// before "not integrable" means anything.
fn assert_not_integrable(workspace: &Workspace, card_id: &str, state: &str) {
    let envelope = workspace.integration_json(&["ready", "--cycle-id", "C-001"]);
    assert!(
        !ready_ids(&envelope).contains(&card_id.to_owned()),
        "{card_id} must not be offered for integration: {envelope}"
    );

    let waiting = envelope["data"]["not_ready"]
        .as_array()
        .expect("a not-ready list")
        .iter()
        .find(|entry| entry["card_id"] == card_id)
        .unwrap_or_else(|| panic!("{card_id} must be named with a reason: {envelope}"));
    assert_eq!(waiting["state"], state);
    assert!(
        waiting["reason"]
            .as_str()
            .unwrap()
            .contains("not `approved`"),
        "unexpected reason: {waiting}"
    );

    // And the half that actually protects the protected branch: naming the
    // card explicitly is refused, not quietly dropped from the selection.
    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        card_id,
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-NOT-INTEGRABLE");
}

#[test]
fn a_card_left_in_changes_requested_by_a_stale_verdict_is_not_integrable() {
    let workspace = cycle_with(1);
    left_non_approved_by_a_stale_verdict(&workspace, "F-001", "changes_requested");
    assert_not_integrable(&workspace, "F-001", "changes_requested");
}

#[test]
fn a_card_left_blocked_by_a_stale_verdict_is_not_integrable() {
    let workspace = cycle_with(1);
    left_non_approved_by_a_stale_verdict(&workspace, "F-001", "blocked");
    assert_not_integrable(&workspace, "F-001", "blocked");
}

#[test]
fn a_stale_non_approval_does_not_drag_down_an_approved_sibling() {
    // The other direction, so the two tests above cannot pass by refusing
    // everything: a card left non-approved by a stale verdict must not stop a
    // genuinely approved card in the same cycle from integrating.
    let workspace = cycle_with(2);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    left_non_approved_by_a_stale_verdict(&workspace, "F-002", "changes_requested");

    let envelope = workspace.integration_json(&["ready", "--cycle-id", "C-001"]);
    assert_eq!(ready_ids(&envelope), ["F-001"]);
    assert_not_integrable(&workspace, "F-002", "changes_requested");

    // Appearing in `ready` is not the same as being integrable, and this test
    // only earns its place by reaching the surface where refusing everything
    // would show. Breaking `select` so any non-approved sibling poisoned an
    // omitted-card selection left the earlier version of this test green.
    let prepared = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(
        merge_order(&prepared),
        ["F-001"],
        "the approved sibling integrates on its own: {prepared}"
    );
}

#[test]
fn preparing_selects_only_approved_candidates() {
    let workspace = cycle_with(2);
    workspace.approve_card("F-001", "src/F-001/a.rs");

    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        "F-002",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-NOT-INTEGRABLE");
}

#[test]
fn omitting_card_ids_selects_every_ready_card() {
    let workspace = cycle_with(3);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.approve_card("F-003", "src/F-003/a.rs");

    let envelope = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(merge_order(&envelope), ["F-001", "F-003"]);
    assert_eq!(envelope["data"]["mode"], "batch");
}

#[test]
fn final_prepare_requires_a_sealed_cycle_before_it_changes_state() {
    let workspace = cycle_with(1);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let before = workspace.control_head();

    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--final",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-CYCLE-NOT-SEALED");
    assert_eq!(
        workspace.control_head(),
        before,
        "refusal must not record a plan"
    );
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "approved"
    );
}

#[test]
fn final_prepare_forbids_card_selection_arguments() {
    let workspace = cycle_with(1);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);

    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--final",
        "--card-id",
        "F-001",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(error_code(&output), "CH-USAGE-CONFLICTING-OPTIONS");
}

#[test]
fn final_prepare_refuses_a_sealed_cycle_with_an_unaccounted_member() {
    let workspace = cycle_with(2);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);
    let before = workspace.control_head();

    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--final",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-FINAL-CYCLE-INCOMPLETE");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("F-002"),
        "the refusal must name the unaccounted member"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "refusal must not record a plan"
    );
}

#[test]
fn final_prepare_accounts_for_every_sealed_member_and_records_the_seal_binding() {
    let workspace = cycle_with(2);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.approve_card("F-002", "src/F-002/a.rs");
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);

    let envelope = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--final",
    ]);
    assert_eq!(merge_order(&envelope), ["F-001", "F-002"]);
    assert_eq!(envelope["data"]["mode"], "batch");
    assert_eq!(envelope["data"]["final_for_cycle"], true);
    assert!(
        envelope["data"]["sealed_cycle_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:")),
        "the exact sealed record must be pinned: {envelope}"
    );
    assert_eq!(
        envelope["data"]["abandoned_card_ids"],
        serde_json::json!([])
    );
}

#[test]
fn final_prepare_keeps_an_explicitly_abandoned_member_auditable() {
    let workspace = cycle_with(2);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.card(&[
        "abandon",
        "--card-id",
        "F-002",
        "--reason",
        "superseded by F-001",
    ]);
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);

    let envelope = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--final",
    ]);
    assert_eq!(merge_order(&envelope), ["F-001"]);
    assert_eq!(envelope["data"]["mode"], "batch");
    assert_eq!(
        envelope["data"]["abandoned_card_ids"],
        serde_json::json!(["F-002"])
    );
}

/// #178: a sealed cycle that never had a card declared into it is refused
/// under `--final`, distinctly from the all-abandoned case
/// (`final_prepare_succeeds_and_reports_delivering_nothing_when_every_member_was_abandoned`
/// below), and the refusal names a real command to recover with — not just
/// the bare fact that nothing is there.
#[test]
fn final_prepare_refuses_a_sealed_cycle_that_never_had_a_card() {
    let workspace = cycle_with(0);
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);
    let before = workspace.control_head();

    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--final",
    ]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-NOT-FOUND");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        stdout.contains("nothing was abandoned"),
        "the refusal must say this is not the all-abandoned case: {stdout}"
    );
    assert!(
        stdout.contains("cycle abandon"),
        "the refusal must name a real recovery command, not just state the fact and stop: {stdout}"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "refusal must not record a plan"
    );
}

/// #178 companion to the refusal above: when a sealed cycle's members were
/// all abandoned, `--final` succeeds — and the outcome says so plainly
/// rather than looking like an ordinary integration that happens to have
/// zero members.
#[test]
fn final_prepare_succeeds_and_reports_delivering_nothing_when_every_member_was_abandoned() {
    let workspace = cycle_with(2);
    workspace.card(&["abandon", "--card-id", "F-001", "--reason", "superseded"]);
    workspace.card(&["abandon", "--card-id", "F-002", "--reason", "superseded"]);
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);

    let envelope = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--final",
    ]);
    assert!(
        merge_order(&envelope).is_empty(),
        "an all-abandoned final integration has no members: {envelope}"
    );
    assert_eq!(envelope["data"]["final_for_cycle"], true);
    assert_eq!(
        envelope["data"]["abandoned_card_ids"],
        serde_json::json!(["F-001", "F-002"])
    );
    assert_eq!(
        envelope["data"]["delivers_no_cards"], true,
        "an all-abandoned final integration must say plainly that it delivers no cards, \
         not look like an ordinary integration: {envelope}"
    );
}

/// #178: the `delivers_no_cards` marker is specific to the all-abandoned
/// case, not a blanket property of every `--final` integration.
#[test]
fn ordinary_final_integration_is_not_marked_as_delivering_nothing() {
    let workspace = cycle_with(1);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);

    let envelope = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--final",
    ]);
    assert_eq!(
        envelope["data"]["delivers_no_cards"], false,
        "a final integration that actually delivers a card must not carry the empty marker: {envelope}"
    );
}

/// #178 repair, from review of `4eb45ac`: a mutation that broke only
/// `report_integration`'s text branch (`let mut text = if delivers_no_cards
/// { .. } else { .. }` -> `if false { .. }`, leaving the JSON field intact)
/// left `cargo test --test integration_plan` green, because every other
/// `--final` test in this file reads the outcome through
/// `Workspace::integration_json` / `integration_raw`, which always pass
/// `--output json`. Three independent cold-start operator protocols ran
/// without that flag, so text mode — not JSON — is the channel that
/// actually needed pinning; see the two tests above this one, which cover
/// the JSON payload alone and would not have caught this.
///
/// Deliberately not routed through `Workspace::integration_raw` /
/// `integration_json`: text mode is the entire point. `Workspace::run` is
/// the same unmediated invocation `tests/card_example.rs` already uses for
/// exactly this reason — see
/// `the_emitted_card_draft_example_text_mode_stdout_is_accepted_by_card_create`
/// and `the_emitted_card_draft_example_warns_that_base_sha_must_be_replaced`
/// there. No new helper was added to `tests/support/mod.rs`: `Workspace::run`
/// already exists and already omits `--output` and `--control`, which is
/// exactly what an operator's own invocation would.
///
/// Pins the load-bearing claim ("delivers no cards"), not the surrounding
/// prose verbatim — the exact wording is expected to be reworded over time,
/// and a verbatim test would then be deleted rather than fixed.
///
/// Mutation that must make this fail: `let mut text = if delivers_no_cards`
/// -> `let mut text = if false` in `report_integration`.
#[test]
fn final_prepare_text_mode_says_delivers_no_cards_only_for_the_all_abandoned_case() {
    let all_abandoned = cycle_with(2);
    all_abandoned.card(&["abandon", "--card-id", "F-001", "--reason", "superseded"]);
    all_abandoned.card(&["abandon", "--card-id", "F-002", "--reason", "superseded"]);
    all_abandoned.cycle(&["seal", "--cycle-id", "C-001"]);

    let all_abandoned_output = Workspace::run(&[
        "integration".to_owned(),
        "prepare".to_owned(),
        "--cycle-id".to_owned(),
        "C-001".to_owned(),
        "--actor-id".to_owned(),
        "coordinator".to_owned(),
        "--final".to_owned(),
        "--control".to_owned(),
        all_abandoned.control.display().to_string(),
    ]);
    assert!(
        all_abandoned_output.status.success(),
        "an all-abandoned final integration must still succeed in text mode (exit {:?}): {}",
        all_abandoned_output.status.code(),
        String::from_utf8_lossy(&all_abandoned_output.stderr)
    );
    let all_abandoned_stdout = String::from_utf8_lossy(&all_abandoned_output.stdout).into_owned();
    assert!(
        all_abandoned_stdout.contains("delivers no cards"),
        "the default text-mode output an operator actually sees must say plainly that this \
         integration delivers no cards — the JSON payload's `delivers_no_cards` field alone is \
         not enough, since `--output json` is not the default and three cold-start protocols ran \
         without it: {all_abandoned_stdout}"
    );

    let ordinary = cycle_with(1);
    ordinary.approve_card("F-001", "src/F-001/a.rs");
    ordinary.cycle(&["seal", "--cycle-id", "C-001"]);

    let ordinary_output = Workspace::run(&[
        "integration".to_owned(),
        "prepare".to_owned(),
        "--cycle-id".to_owned(),
        "C-001".to_owned(),
        "--actor-id".to_owned(),
        "coordinator".to_owned(),
        "--final".to_owned(),
        "--control".to_owned(),
        ordinary.control.display().to_string(),
    ]);
    assert!(
        ordinary_output.status.success(),
        "an ordinary final integration must still succeed in text mode (exit {:?}): {}",
        ordinary_output.status.code(),
        String::from_utf8_lossy(&ordinary_output.stderr)
    );
    let ordinary_stdout = String::from_utf8_lossy(&ordinary_output.stdout).into_owned();
    assert!(
        !ordinary_stdout.contains("delivers no cards"),
        "an ordinary final integration's text-mode output must not carry the empty-marker \
         phrase, or the assertion above could pass by matching text that is always present \
         regardless of whether anything was actually abandoned: {ordinary_stdout}"
    );
}

#[test]
fn dependencies_are_merged_before_their_dependents() {
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
    // F-002 depends on F-001, so it must merge second despite sorting first
    // among the ready set only by identifier.
    workspace.activate_card_depending_on("F-002", &["src/F-002/**"], &["F-001"]);
    workspace.approve_card("F-002", "src/F-002/a.rs");
    workspace.approve_card("F-001", "src/F-001/a.rs");

    let envelope = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(merge_order(&envelope), ["F-001", "F-002"]);
}

#[test]
fn a_dependency_that_is_neither_selected_nor_landed_is_refused() {
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
    workspace.activate_card_depending_on("F-002", &["src/F-002/**"], &["F-001"]);
    workspace.approve_card("F-002", "src/F-002/a.rs");
    // F-001 is approved too, but deliberately left out of the selection.
    workspace.approve_card("F-001", "src/F-001/a.rs");

    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        "F-002",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-DEPENDENCY-UNSATISFIED");
}

#[test]
fn ordering_does_not_depend_on_the_order_cards_are_named() {
    let workspace = cycle_with(3);
    for card in ["F-001", "F-002", "F-003"] {
        workspace.approve_card(card, &format!("src/{card}/a.rs"));
    }

    let envelope = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        "F-003",
        "--card-id",
        "F-001",
        "--card-id",
        "F-002",
    ]);
    assert_eq!(
        merge_order(&envelope),
        ["F-001", "F-002", "F-003"],
        "merge order must be a function of the selection, not of the argument order"
    );
}

#[test]
fn an_atomic_group_cannot_be_split() {
    let workspace = cycle_with(2);
    workspace.cycle(&[
        "declare-group",
        "--cycle-id",
        "C-001",
        "--name",
        "schema-and-reader",
        "--card-id",
        "F-001",
        "--card-id",
        "F-002",
    ]);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.approve_card("F-002", "src/F-002/a.rs");

    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        "F-001",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-ATOMIC-GROUP-SPLIT");

    // The whole group is accepted, and the record says which group it carries.
    let envelope = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(
        envelope["data"]["atomic_groups"],
        serde_json::json!(["schema-and-reader"])
    );
    assert_eq!(
        envelope["data"]["members"][0]["atomic_group"],
        "schema-and-reader"
    );
}

#[test]
fn one_integration_holds_a_cycle_at_a_time() {
    let workspace = cycle_with(2);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.approve_card("F-002", "src/F-002/a.rs");

    workspace.integration(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        "F-001",
    ]);

    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--card-id",
        "F-002",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INTEGRATION-OPEN");
}

#[test]
fn a_prepared_integration_records_the_authority_baseline_it_was_built_against() {
    let workspace = cycle_with(1);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let authority = workspace.authority_head();

    let envelope = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(envelope["data"]["expected_main_sha"], authority);
    assert_eq!(envelope["data"]["status"], "prepared");
    assert_eq!(envelope["data"]["mode"], "individual");
}

#[test]
fn preparing_moves_its_cards_to_integrating() {
    let workspace = cycle_with(1);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.integration(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);

    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "integrating"
    );
    let prepared = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "integration.prepared")
        .expect("preparation must be recorded");
    assert_eq!(prepared["next_state"], "prepared");
}

#[test]
fn mode_individual_is_refused_for_a_multi_card_selection() {
    let workspace = cycle_with(2);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.approve_card("F-002", "src/F-002/a.rs");

    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--mode",
        "individual",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(error_code(&output), "CH-USAGE-CONFLICTING-OPTIONS");
}

#[test]
fn a_dry_run_reports_the_plan_and_changes_nothing() {
    let workspace = cycle_with(1);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let before = workspace.control_head();

    let envelope = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--dry-run",
    ]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(
        envelope["data"]["merge_order"],
        serde_json::json!(["F-001"])
    );

    assert_eq!(workspace.control_head(), before);
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "approved",
        "a dry run must not advance the card"
    );
}

#[test]
fn inspect_reproduces_the_recorded_plan() {
    let workspace = cycle_with(1);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    let prepared = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    let integration_id = prepared["data"]["integration_id"].as_str().unwrap();

    let inspected = workspace.integration_json(&["inspect", "--integration-id", integration_id]);
    assert_eq!(inspected["data"], prepared["data"]);
}

#[test]
fn preparing_an_empty_cycle_is_refused() {
    let workspace = cycle_with(1);
    let output = workspace.integration_raw(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-NOT-FOUND");
}

#[test]
fn an_atomic_group_must_name_cards_the_cycle_declares() {
    let workspace = cycle_with(2);
    let output = workspace.cycle_raw(&[
        "declare-group",
        "--cycle-id",
        "C-001",
        "--name",
        "bad",
        "--card-id",
        "F-001",
        "--card-id",
        "F-009",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-CYCLE");
}

#[test]
fn an_abandoned_integration_releases_its_cycle_and_returns_its_cards() {
    let workspace = cycle_with(2);
    workspace.approve_card("F-001", "src/F-001/a.rs");
    workspace.approve_card("F-002", "src/F-002/a.rs");
    let first = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ])["data"]["integration_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "integrating"
    );

    let envelope = workspace.integration_json(&[
        "abandon",
        "--integration-id",
        &first,
        "--actor-id",
        "coordinator",
        "--reason",
        "the combination cannot be made to work in this cycle",
    ]);
    assert_eq!(envelope["data"]["status"], "abandoned");

    // The approvals were never the problem — the combination was — so the
    // cards go back to `approved` rather than back to work.
    for card in ["F-001", "F-002"] {
        assert_eq!(
            workspace.card_json(&["status", "--card-id", card])["data"]["state"],
            "approved"
        );
    }

    // And the cycle is free again, which is the whole point: without this,
    // a plan that cannot land holds its cycle forever.
    let second = workspace.integration_json(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    assert_ne!(second["data"]["integration_id"], first.as_str());
}

#[test]
fn abandoning_releases_the_landing_ref() {
    let workspace = cycle_with(1);
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
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", &id, "--actor-id", "coordinator"]);
    }

    workspace.integration(&[
        "abandon",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
        "--reason",
        "superseded",
    ]);
    // A landing commit nobody will promote must not be kept alive forever.
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
        "the landing ref must be released"
    );
}

#[test]
fn a_promoted_integration_cannot_be_abandoned() {
    let workspace = cycle_with(1);
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

    let output = workspace.integration_raw(&[
        "abandon",
        "--integration-id",
        &id,
        "--actor-id",
        "coordinator",
        "--reason",
        "too late",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-TRANSITION");
}
