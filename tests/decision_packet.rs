//! Decision packets expose final-cycle evidence without creating authority.

mod support;

use support::Workspace;

fn final_cycle() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Release the bounded change",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    for card in ["F-002", "F-001"] {
        workspace.activate_card(card, &[&format!("src/{card}/**")]);
    }
    for card in ["F-002", "F-001"] {
        workspace.approve_card(card, &format!("src/{card}/a.rs"));
    }
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);
    workspace.integration(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
        "--final",
    ]);
    workspace
}

#[test]
fn prepared_final_packet_is_read_only_and_truthfully_names_no_landing_or_evidence() {
    let workspace = final_cycle();
    let before = workspace.control_head();
    let envelope = workspace.integration_json(&["decision-packet", "--integration-id", "INT-001"]);

    assert_eq!(envelope["command"], "integration.decision-packet");
    assert_eq!(
        envelope["data"]["packet_schema"],
        "harness.decision-packet/v1"
    );
    assert_eq!(
        envelope["data"]["cycle"]["objective"],
        "Release the bounded change"
    );
    assert_eq!(
        envelope["data"]["landing"],
        serde_json::json!({"state":"not_built","sha":null,"tree":null})
    );
    assert!(envelope["data"]["verification"].is_null());
    assert!(envelope["data"]["review"].is_null());
    assert_eq!(
        envelope["data"]["decision_readiness"]["next_permitted_action"],
        "integration.merge"
    );
    assert_eq!(
        envelope["data"]["accounting"]["selected_card_ids"],
        serde_json::json!(["F-001", "F-002"])
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "packet must not write control state"
    );
}

#[test]
fn ordinary_integration_cannot_be_presented_as_a_final_decision() {
    let workspace = Workspace::initialized();
    workspace.cycle(&["create", "--cycle-id", "C-001", "--objective", "ordinary"]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.approve_card("F-001", "src/a.rs");
    workspace.integration(&[
        "prepare",
        "--cycle-id",
        "C-001",
        "--actor-id",
        "coordinator",
    ]);
    let before = workspace.control_head();
    let output = workspace.integration_raw(&["decision-packet", "--integration-id", "INT-001"]);
    assert_eq!(output.status.code(), Some(5));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["error"]["code"],
        "CH-POLICY-DECISION-PACKET-FINAL-ONLY"
    );
    assert_eq!(workspace.control_head(), before);
}

#[test]
fn packet_tracks_exact_landing_then_verification_and_review_without_writing() {
    let workspace = final_cycle();
    let id = "INT-001";
    for step in ["merge", "land"] {
        workspace.integration(&[step, "--integration-id", id, "--actor-id", "coordinator"]);
    }
    let inspected = workspace.integration_json(&["inspect", "--integration-id", id]);
    let landing_sha = inspected["data"]["landing_sha"]
        .as_str()
        .expect("a real landing SHA")
        .to_owned();

    let before = workspace.control_head();
    let landed = workspace.integration_json(&["decision-packet", "--integration-id", id]);
    assert_eq!(landed["data"]["landing"]["state"], "built");
    assert_eq!(landed["data"]["landing"]["sha"], landing_sha);
    assert_eq!(
        landed["data"]["decision_readiness"]["next_permitted_action"],
        "integration.verify"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "packet after land must be read-only"
    );

    workspace.integration(&["verify", "--integration-id", id, "--actor-id", "verifier"]);
    let before = workspace.control_head();
    let verified = workspace.integration_json(&["decision-packet", "--integration-id", id]);
    assert_eq!(verified["data"]["verification"]["landing_sha"], landing_sha);
    assert!(verified["data"]["verification"]["receipt_ids"].is_array());
    assert_eq!(
        verified["data"]["decision_readiness"]["next_permitted_action"],
        "integration.review"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "packet after verify must be read-only"
    );

    workspace.integration(&[
        "review",
        "--integration-id",
        id,
        "--reviewer-actor-id",
        "integration-reviewer",
    ]);
    let before = workspace.control_head();
    let reviewed = workspace.integration_json(&["decision-packet", "--integration-id", id]);
    assert_eq!(reviewed["data"]["review"]["state"], "recorded");
    assert_eq!(
        reviewed["data"]["decision_readiness"]["next_permitted_action"],
        "acceptance.record"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "packet after review must be read-only"
    );
}
