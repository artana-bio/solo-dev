//! Final-cycle exception events halt only the named integration until a
//! configured authorizer records an explicit continue decision.

mod support;

use std::fs;

use support::Workspace;

fn reviewed_final() -> (Workspace, String) {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "exception flow",
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
    configure(&workspace, &["critical_residual_risk"]);
    (workspace, id)
}

fn configure(workspace: &Workspace, triggers: &[&str]) {
    let path = workspace.control.join("project/project.json");
    let mut project: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    project["final_authorization_policy"] = serde_json::json!({
        "version":"harness.final-authorization-policy/v1",
        "authorization_unit":"sealed_cycle",
        "authorizer_actor_ids":["owner"],
        "exception_triggers":triggers,
    });
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&project).unwrap()),
    )
    .unwrap();
    support::git(&workspace.control, &["add", "-A"]);
    support::git(
        &workspace.control,
        &["commit", "-q", "-m", "configure exceptions"],
    );
}

fn raise(workspace: &Workspace, id: &str) -> serde_json::Value {
    workspace.integration_json(&[
        "exception",
        "raise",
        "--integration-id",
        id,
        "--actor-id",
        "safety-agent",
        "--trigger",
        "critical_residual_risk",
        "--evidence-ref",
        "receipt:R-001",
    ])
}

#[test]
fn raised_exception_is_visible_and_blocks_only_its_final_integration() {
    let (workspace, id) = reviewed_final();
    let raised = raise(&workspace, &id);
    let event_id = raised["data"]["exception_event_id"].as_str().unwrap();
    let packet = workspace.integration_json(&["decision-packet", "--integration-id", &id]);
    assert_eq!(packet["data"]["exceptions"]["state"], "pending");
    assert_eq!(
        packet["data"]["exceptions"]["items"][0]["event_id"],
        event_id
    );
    assert_eq!(
        packet["data"]["decision_readiness"]["next_permitted_action"],
        "integration.exception.resolve"
    );

    let before = workspace.control_head();
    let refused = workspace.acceptance_raw(&[
        "record",
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "owner",
    ]);
    assert_eq!(refused.status.code(), Some(5));
    let envelope: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-EXCEPTION-PENDING");
    assert_eq!(workspace.control_head(), before);
}

#[test]
fn only_final_authorizer_can_continue_and_continue_does_not_authorize() {
    let (workspace, id) = reviewed_final();
    let event_id = raise(&workspace, &id)["data"]["exception_event_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = workspace.control_head();
    let refused = workspace.integration_raw(&[
        "exception",
        "resolve",
        "--integration-id",
        &id,
        "--exception-event-id",
        &event_id,
        "--authorizer-actor-id",
        "outsider",
    ]);
    assert_eq!(refused.status.code(), Some(5));
    assert_eq!(workspace.control_head(), before);
    let resolved = workspace.integration_json(&[
        "exception",
        "resolve",
        "--integration-id",
        &id,
        "--exception-event-id",
        &event_id,
        "--authorizer-actor-id",
        "owner",
    ]);
    assert_eq!(resolved["data"]["authorization"], "not_recorded");
    let inspect = workspace.integration_json(&["inspect", "--integration-id", &id]);
    assert_eq!(inspect["data"]["status"], "reviewed");
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "owner",
    ]);
}

#[test]
fn accepted_final_exception_blocks_promotion_until_authorizer_continues() {
    let (workspace, id) = reviewed_final();
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "owner",
    ]);
    let raised = raise(&workspace, &id);
    let event_id = raised["data"]["exception_event_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let packet = workspace.integration_json(&["decision-packet", "--integration-id", &id]);
    assert_eq!(packet["data"]["decision_readiness"]["current"], "accepted");
    assert_eq!(packet["data"]["exceptions"]["state"], "pending");
    assert_eq!(
        packet["data"]["decision_readiness"]["next_permitted_action"],
        "integration.exception.resolve"
    );
    let control_before = workspace.control_head();
    let refused = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &id,
        "--actor-id",
        "release-agent",
    ]);
    assert_eq!(refused.status.code(), Some(5));
    let envelope: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-EXCEPTION-PENDING");
    assert_eq!(workspace.control_head(), control_before);
    workspace.integration(&[
        "exception",
        "resolve",
        "--integration-id",
        &id,
        "--exception-event-id",
        &event_id,
        "--authorizer-actor-id",
        "owner",
    ]);
    let resolved_packet = workspace.integration_json(&["decision-packet", "--integration-id", &id]);
    assert_eq!(
        resolved_packet["data"]["decision_readiness"]["next_permitted_action"],
        "integration.promote"
    );
    assert_eq!(
        resolved_packet["data"]["exceptions"]["items"][0]["next_action"],
        "integration.promote"
    );
    workspace.integration(&[
        "promote",
        "--integration-id",
        &id,
        "--actor-id",
        "release-agent",
    ]);
}

#[test]
fn forged_outsider_resolution_does_not_release_exception_and_audit_flags_it() {
    let (workspace, id) = reviewed_final();
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "owner",
    ]);
    let raised = raise(&workspace, &id);
    let raised_id = raised["data"]["exception_event_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let raised_event = workspace
        .events()
        .into_iter()
        .find(|event| event["event_id"] == raised_id)
        .unwrap();
    let raised_metadata = raised_event["metadata"].clone();
    let mut forged = raised_event;
    forged["event_id"] = serde_json::json!("E-999999");
    forged["event_type"] = serde_json::json!("integration.exception_resolved");
    forged["actor_id"] = serde_json::json!("outsider");
    forged["metadata"] = serde_json::json!({
        "integration_id": id,
        "exception_event_id": raised_id,
        "resolution": "continue",
        "policy_digest": raised_metadata["policy_digest"],
        "integration_digest": raised_metadata["integration_digest"],
        "sealed_cycle_digest": raised_metadata["sealed_cycle_digest"]
    });
    let path = workspace.control.join("events/E-999999.json");
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&forged).unwrap()),
    )
    .unwrap();
    support::git(&workspace.control, &["add", "-A"]);
    support::git(
        &workspace.control,
        &["commit", "-q", "-m", "forge resolution"],
    );

    let refused = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &id,
        "--actor-id",
        "release-agent",
    ]);
    assert_eq!(refused.status.code(), Some(5));
    let envelope: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-EXCEPTION-PENDING");
    let audit = Workspace::run(&[
        "audit".to_owned(),
        "cycle".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
        "--control".to_owned(),
        workspace.control.display().to_string(),
        "--cycle-id".to_owned(),
        "C-001".to_owned(),
    ]);
    assert_eq!(audit.status.code(), Some(5));
    let audit_envelope: serde_json::Value = serde_json::from_slice(&audit.stdout).unwrap();
    assert_eq!(
        audit_envelope["error"]["code"],
        "CH-POLICY-AUDIT-DISCREPANCY"
    );
}

#[test]
fn changed_exception_policy_keeps_the_pending_gate_fail_closed() {
    let (workspace, id) = reviewed_final();
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "owner",
    ]);
    let event_id = raise(&workspace, &id)["data"]["exception_event_id"]
        .as_str()
        .unwrap()
        .to_owned();
    configure(&workspace, &["critical_residual_risk", "external_effect"]);
    let resolution = workspace.integration_raw(&[
        "exception",
        "resolve",
        "--integration-id",
        &id,
        "--exception-event-id",
        &event_id,
        "--authorizer-actor-id",
        "owner",
    ]);
    let promotion = workspace.integration_raw(&[
        "promote",
        "--integration-id",
        &id,
        "--actor-id",
        "release-agent",
    ]);
    for output in [&resolution, &promotion] {
        assert_eq!(output.status.code(), Some(5));
        let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(envelope["error"]["code"], "CH-POLICY-NOT-ACCEPTED");
    }
}

#[test]
fn disabled_or_duplicate_exception_refuses_without_mutation_and_dry_run_matches() {
    let (workspace, id) = reviewed_final();
    configure(&workspace, &[]);
    let before = workspace.control_head();
    let real = workspace.integration_raw(&[
        "exception",
        "raise",
        "--integration-id",
        &id,
        "--actor-id",
        "safety-agent",
        "--trigger",
        "critical_residual_risk",
        "--evidence-ref",
        "receipt:R-001",
    ]);
    let preview = workspace.integration_raw(&[
        "exception",
        "raise",
        "--integration-id",
        &id,
        "--actor-id",
        "safety-agent",
        "--trigger",
        "critical_residual_risk",
        "--evidence-ref",
        "receipt:R-001",
        "--dry-run",
    ]);
    assert_eq!(real.status.code(), preview.status.code());
    assert_eq!(workspace.control_head(), before);

    configure(&workspace, &["critical_residual_risk"]);
    raise(&workspace, &id);
    let before = workspace.control_head();
    let duplicate = workspace.integration_raw(&[
        "exception",
        "raise",
        "--integration-id",
        &id,
        "--actor-id",
        "safety-agent",
        "--trigger",
        "critical_residual_risk",
        "--evidence-ref",
        "receipt:R-002",
    ]);
    assert_eq!(duplicate.status.code(), Some(5));
    assert_eq!(workspace.control_head(), before);
}

#[test]
fn old_policy_without_exception_field_stays_readable_and_normal_final_flow_unchanged() {
    let (workspace, id) = reviewed_final();
    let path = workspace.control.join("project/project.json");
    let mut project: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    project["final_authorization_policy"]
        .as_object_mut()
        .unwrap()
        .remove("exception_triggers");
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&project).unwrap()),
    )
    .unwrap();
    support::git(&workspace.control, &["add", "-A"]);
    support::git(&workspace.control, &["commit", "-q", "-m", "v1 policy"]);
    workspace.acceptance(&[
        "record",
        "--integration-id",
        &id,
        "--authorizer-actor-id",
        "owner",
    ]);
    let raised = workspace.integration_raw(&[
        "exception",
        "raise",
        "--integration-id",
        &id,
        "--actor-id",
        "safety-agent",
        "--trigger",
        "critical_residual_risk",
        "--evidence-ref",
        "receipt:R-001",
    ]);
    assert_eq!(raised.status.code(), Some(5));
}
