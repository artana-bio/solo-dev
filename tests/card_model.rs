//! `WP-210` acceptance: card schema, canonicalization, and digest.

mod support;

use std::fs;

use change_harness::domain::card::CardRecord;
use serde_json::Value;
use support::Workspace;

/// A workspace with an active cycle, ready to accept cards.
fn with_active_cycle() -> Workspace {
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

/// Writes a draft file and returns its path.
fn write_draft(workspace: &Workspace, name: &str, body: &str) -> String {
    let path = workspace.root.join(name);
    fs::write(&path, body).unwrap();
    path.display().to_string()
}

/// A valid draft naming the active cycle's baseline.
fn draft_yaml(workspace: &Workspace, card_id: &str, include: &str) -> String {
    format!(
        "\
card_id: {card_id}
cycle_id: C-001
title: Implement {card_id}
goal: Deliver the {card_id} behavior
non_goals: []
risk: low
change_kind: feature
base_sha: {baseline}
write_scope:
  include: [{include}]
  exclude: []
named_gates:
  feature: [gate.unit]
  review: []
  integration: [gate.all]
acceptance:
  behaviors: [it works]
  regressions: []
review_policy: independent
rollback_strategy: revert the commit
",
        baseline = workspace.authority_head()
    )
}

fn with_proof_map(draft: &str) -> String {
    format!(
        "{draft}proof_map:\n  schema: harness.proof-map/v1\n  entries:\n    - invariant: intended behavior remains true\n      precondition: the focused fixture is installed\n      assertion: the named check observes the behavior\n      mutation: bypassing the behavior makes the check fail\n  claim_boundary: only the named focused behavior\n"
    )
}

#[test]
fn a_valid_draft_validates_without_touching_control() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    let before = workspace.control_head();

    let output = Workspace::run(&[
        "card".into(),
        "validate".into(),
        "--output".into(),
        "json".into(),
        "--draft".into(),
        path,
    ]);
    assert!(output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["data"]["valid"], true);
    assert_eq!(workspace.control_head(), before);
}

#[test]
fn activation_produces_an_immutable_revision_with_a_digest() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);

    let envelope = workspace.card_json(&["activate", "--card-id", "F-001"]);
    assert_eq!(envelope["data"]["revision"], 1);
    assert_eq!(envelope["data"]["state"], "ready");
    let digest = envelope["data"]["digest"].as_str().unwrap();
    assert!(digest.starts_with("sha256:"));

    let status = workspace.card_json(&["status", "--card-id", "F-001"]);
    assert_eq!(status["data"]["digest"], digest);
    assert_eq!(status["data"]["digest_intact"], true);
    assert_eq!(
        status["data"]["canonical_algorithm"], "harness.canonical-json/v1",
        "a record must name the algorithm its digest was computed under"
    );
}

#[test]
fn medium_risk_requires_a_complete_proof_map_at_activation_in_preview_and_real_modes() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs").replace("risk: low", "risk: medium"),
    );
    workspace.card(&["create", "--draft", &path]);

    for extra in [Vec::<&str>::new(), vec!["--dry-run"]] {
        let mut arguments = vec!["activate", "--card-id", "F-001"];
        arguments.extend(extra);
        let output = workspace.card_raw(&arguments);
        assert_eq!(output.status.code(), Some(5));
        assert!(String::from_utf8_lossy(&output.stdout).contains("CH-POLICY-INVALID-CARD"));
    }
    assert!(
        !workspace
            .control_tracked_files()
            .contains(&"cards/F-001/r1.json".to_owned()),
        "refusal writes no activated card"
    );
}

#[test]
fn a_complete_proof_map_is_frozen_into_a_medium_risk_card_and_moves_its_digest() {
    let workspace = with_active_cycle();
    let valid = with_proof_map(
        &draft_yaml(&workspace, "F-001", "src/a.rs").replace("risk: low", "risk: medium"),
    );
    let path = write_draft(&workspace, "F-001.yaml", &valid);
    workspace.card(&["create", "--draft", &path]);
    let activated = workspace.card_json(&["activate", "--card-id", "F-001"]);
    let digest = activated["data"]["digest"].as_str().unwrap().to_owned();
    let record: CardRecord = serde_json::from_str(
        &std::fs::read_to_string(workspace.control.join("cards/F-001/r1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        record.proof_map.as_ref().unwrap().schema,
        "harness.proof-map/v1"
    );
    assert_eq!(
        record.proof_map.as_ref().unwrap().entries[0].invariant,
        "intended behavior remains true"
    );

    let mut only_proof_map_changed = record.clone();
    only_proof_map_changed
        .proof_map
        .as_mut()
        .unwrap()
        .claim_boundary = "only a narrower named focused behavior".to_owned();
    assert_ne!(
        only_proof_map_changed.digest().unwrap().as_str(),
        digest,
        "the immutable card digest must bind the proof map itself"
    );
}

#[test]
fn proof_map_requirement_has_dry_run_and_real_revision_parity() {
    let workspace = with_active_cycle();
    let initial = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &initial]);
    workspace.card(&["activate", "--card-id", "F-001"]);
    let before = workspace.control_head();
    let medium_without_map = write_draft(
        &workspace,
        "F-001-r2.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs").replace("risk: low", "risk: medium"),
    );

    for extra in [Vec::<&str>::new(), vec!["--dry-run"]] {
        let mut arguments = vec![
            "revise",
            "--card-id",
            "F-001",
            "--draft",
            &medium_without_map,
            "--reason",
            "risk reassessed",
        ];
        arguments.extend(extra);
        let output = workspace.card_raw(&arguments);
        assert_eq!(output.status.code(), Some(5));
        assert!(String::from_utf8_lossy(&output.stdout).contains("CH-POLICY-INVALID-CARD"));
    }
    assert_eq!(
        workspace.control_head(),
        before,
        "refused revision writes no event"
    );
    assert!(
        !workspace
            .control_tracked_files()
            .contains(&"cards/F-001/r2.json".to_owned()),
        "refused revision creates no replacement record"
    );
}

#[test]
fn an_activated_card_can_be_abandoned_with_a_reason() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);

    let envelope =
        workspace.card_json(&["abandon", "--card-id", "F-001", "--reason", "superseded"]);
    assert_eq!(envelope["data"]["state"], "abandoned");
    assert_eq!(envelope["data"]["reason"], "superseded");
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "abandoned"
    );

    let event = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "card.abandoned")
        .expect("abandonment must be recorded");
    assert_eq!(event["card_id"], "F-001");
    assert_eq!(event["previous_state"], "ready");
    assert_eq!(event["next_state"], "abandoned");
    assert_eq!(event["metadata"]["reason"], "superseded");
}

#[test]
fn a_card_being_worked_can_also_be_abandoned() {
    // The committed suite otherwise proves abandonment only from `ready`
    // (immediately after activation). Section 11.2 permits it from every
    // non-terminal state except `landed`; `active` is the state most cards
    // actually spend their time in, and it was unproven.
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "active",
        "fixture: the card must reach the state this test is meant to abandon from"
    );

    let envelope = workspace.card_json(&["abandon", "--card-id", "F-001", "--reason", "descoped"]);
    assert_eq!(envelope["data"]["state"], "abandoned");
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "abandoned"
    );
}

#[test]
fn abandoning_a_card_dry_run_changes_nothing() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);
    let before = workspace.control_head();

    let envelope = workspace.card_json(&[
        "abandon",
        "--card-id",
        "F-001",
        "--reason",
        "superseded",
        "--dry-run",
    ]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(workspace.control_head(), before);
    assert_eq!(
        workspace.card_json(&["status", "--card-id", "F-001"])["data"]["state"],
        "ready"
    );
}

#[test]
fn a_reordered_json_draft_parses_identically_to_its_yaml_form() {
    // Order-independence of the digest itself is proven at unit level with a
    // fixed clock. Here the point is narrower: the two authoring formats must
    // reach the same draft. Two *workspaces* cannot be compared, because
    // `base_sha` and `created_at` are material fields that legitimately differ.
    let workspace = with_active_cycle();
    let yaml = write_draft(
        &workspace,
        "a.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    let json = serde_json::json!({
        "rollback_strategy": "revert the commit",
        "review_policy": "independent",
        "acceptance": {"regressions": [], "behaviors": ["it works"]},
        "named_gates": {"integration": ["gate.all"], "review": [], "feature": ["gate.unit"]},
        "write_scope": {"exclude": [], "include": ["src/a.rs"]},
        "base_sha": workspace.authority_head(),
        "change_kind": "feature",
        "risk": "low",
        "non_goals": [],
        "goal": "Deliver the F-001 behavior",
        "title": "Implement F-001",
        "cycle_id": "C-001",
        "card_id": "F-001",
    });
    let json_path = write_draft(
        &workspace,
        "a.json",
        &serde_json::to_string_pretty(&json).unwrap(),
    );

    let from_yaml = Workspace::run(&[
        "card".into(),
        "validate".into(),
        "--output".into(),
        "json".into(),
        "--draft".into(),
        yaml,
    ]);
    let from_json = Workspace::run(&[
        "card".into(),
        "validate".into(),
        "--output".into(),
        "json".into(),
        "--draft".into(),
        json_path,
    ]);

    assert!(from_yaml.status.success() && from_json.status.success());
    let a: Value = serde_json::from_slice(&from_yaml.stdout).unwrap();
    let b: Value = serde_json::from_slice(&from_json.stdout).unwrap();
    assert_eq!(a["data"], b["data"]);
}

#[test]
fn a_material_change_produces_a_different_digest() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    let first = workspace.card_json(&["activate", "--card-id", "F-001"])["data"]["digest"]
        .as_str()
        .unwrap()
        .to_owned();

    let revised = write_draft(
        &workspace,
        "F-001-v2.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs")
            .replace("goal: Deliver", "goal: Substantially different"),
    );
    let second = workspace.card_json(&[
        "revise",
        "--card-id",
        "F-001",
        "--draft",
        &revised,
        "--reason",
        "scope corrected",
    ])["data"]["digest"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_ne!(first, second);
}

#[test]
fn an_unknown_draft_field_fails() {
    let workspace = with_active_cycle();
    let body = draft_yaml(&workspace, "F-001", "src/a.rs") + "surprise: 1\n";
    let path = write_draft(&workspace, "bad.yaml", &body);

    let output = Workspace::run(&[
        "card".into(),
        "validate".into(),
        "--output".into(),
        "json".into(),
        "--draft".into(),
        path,
    ]);
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn activation_rejects_a_draft_missing_goals_acceptance_or_gates() {
    let workspace = with_active_cycle();
    for (name, mutation) in [
        ("no-goal", ("goal: Deliver the F-001 behavior", "goal: ''")),
        ("no-acceptance", ("behaviors: [it works]", "behaviors: []")),
        ("no-gate", ("feature: [gate.unit]", "feature: []")),
    ] {
        let body = draft_yaml(&workspace, "F-001", "src/a.rs").replace(mutation.0, mutation.1);
        let path = write_draft(&workspace, &format!("{name}.yaml"), &body);
        let output = Workspace::run(&["card".into(), "validate".into(), "--draft".into(), path]);
        assert_eq!(
            output.status.code(),
            Some(5),
            "{name} must be a policy failure"
        );
    }
}

#[test]
fn a_revision_supersedes_and_invalidates_the_previous_digest() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    let first = workspace.card_json(&["activate", "--card-id", "F-001"]);
    let first_digest = first["data"]["digest"].as_str().unwrap().to_owned();

    let revised = write_draft(
        &workspace,
        "F-001-v2.yaml",
        &with_proof_map(
            &draft_yaml(&workspace, "F-001", "src/a.rs").replace("risk: low", "risk: medium"),
        ),
    );
    let envelope = workspace.card_json(&[
        "revise",
        "--card-id",
        "F-001",
        "--draft",
        &revised,
        "--reason",
        "risk reassessed",
    ]);

    assert_eq!(envelope["data"]["revision"], 2);
    assert_eq!(envelope["data"]["superseded_revision"], 1);
    assert_eq!(envelope["data"]["superseded_digest"], first_digest);
    assert_eq!(
        envelope["data"]["state"], "ready",
        "a revision returns the card to ready because prior work no longer describes it"
    );

    // The superseded revision record survives; it is never rewritten.
    let tracked = workspace.control_tracked_files();
    assert!(
        tracked.contains(&"cards/F-001/r1.json".to_owned()),
        "{tracked:?}"
    );
    assert!(
        tracked.contains(&"cards/F-001/r2.json".to_owned()),
        "{tracked:?}"
    );
}

#[test]
fn the_invalidation_is_recorded_in_the_event_log() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    let first_digest = workspace.card_json(&["activate", "--card-id", "F-001"])["data"]["digest"]
        .as_str()
        .unwrap()
        .to_owned();

    let revised = write_draft(
        &workspace,
        "F-001-v2.yaml",
        &with_proof_map(
            &draft_yaml(&workspace, "F-001", "src/a.rs").replace("risk: low", "risk: high"),
        ),
    );
    workspace.card(&[
        "revise",
        "--card-id",
        "F-001",
        "--draft",
        &revised,
        "--reason",
        "risk reassessed",
    ]);

    let revision_event = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "card.revised")
        .expect("a revision must be recorded");
    assert_eq!(revision_event["metadata"]["invalidated_revision"], 1);
    assert_eq!(
        revision_event["metadata"]["invalidated_digest"],
        first_digest
    );
    assert_eq!(revision_event["metadata"]["reason"], "risk reassessed");
}

#[test]
fn revisions_increase_by_exactly_one() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);

    for (index, risk) in ["medium", "high", "critical"].iter().enumerate() {
        let body = with_proof_map(
            &draft_yaml(&workspace, "F-001", "src/a.rs")
                .replace("risk: low", &format!("risk: {risk}")),
        );
        let revised = write_draft(&workspace, &format!("r{index}.yaml"), &body);
        let envelope = workspace.card_json(&[
            "revise",
            "--card-id",
            "F-001",
            "--draft",
            &revised,
            "--reason",
            "iterating",
        ]);
        assert_eq!(envelope["data"]["revision"], index + 2);
    }
}

#[test]
fn an_activated_card_cannot_be_activated_again() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);

    let output = workspace.card_raw(&["activate", "--card-id", "F-001"]);
    assert_eq!(output.status.code(), Some(5));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("immutable")
    );
}

#[test]
fn an_edited_immutable_revision_is_detected() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);

    // Edit the supposedly immutable record behind the harness's back.
    let record_path = workspace.control.join("cards/F-001/r1.json");
    let mut value: Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    value["goal"] = serde_json::json!("quietly changed");
    fs::write(&record_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

    let output = workspace.card_raw(&["status", "--card-id", "F-001"]);
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["digest_intact"], false,
        "the digest is recomputed, not trusted"
    );
    assert!(
        envelope["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("altered outside the harness"))
    );
}

#[test]
fn a_card_cannot_be_created_against_a_draft_cycle() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Not yet active",
    ]);
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );

    let output = workspace.card_raw(&["create", "--draft", &path]);
    assert_eq!(output.status.code(), Some(5));
}

#[test]
fn activation_declares_the_card_in_its_cycle() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    workspace.card(&["activate", "--card-id", "F-001"]);

    let cycle = workspace.cycle_json(&["status", "--cycle-id", "C-001"]);
    assert_eq!(cycle["data"]["card_ids"][0], "F-001");
}

#[test]
fn a_dry_run_changes_nothing() {
    let workspace = with_active_cycle();
    let path = write_draft(
        &workspace,
        "F-001.yaml",
        &draft_yaml(&workspace, "F-001", "src/a.rs"),
    );
    workspace.card(&["create", "--draft", &path]);
    let before = workspace.control_head();

    let envelope = workspace.card_json(&["activate", "--card-id", "F-001", "--dry-run"]);
    assert_eq!(envelope["data"]["dry_run"], true);
    assert!(
        envelope["data"]["digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(workspace.control_head(), before);
}
