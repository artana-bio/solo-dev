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
