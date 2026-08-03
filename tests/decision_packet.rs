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
