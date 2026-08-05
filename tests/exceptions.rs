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

// The tests above all reach a configured `exception_triggers` through
// `configure`, which hand-writes `project.json` and commits it directly —
// exactly the bypass this section exists to remove. Nothing below this line
// uses `configure`; every policy is installed through
// `project set-final-authorization-policy` instead, the governed command
// that makes `exception_triggers` reachable at all.

/// A final-authorization policy document in the shape `--policy` reads,
/// naming the given authorizers and enabled exception triggers.
fn final_authorization_policy_document(
    authorizer_actor_ids: &[&str],
    triggers: &[&str],
) -> serde_json::Value {
    serde_json::json!({
        "version": "harness.final-authorization-policy/v1",
        "authorization_unit": "sealed_cycle",
        "authorizer_actor_ids": authorizer_actor_ids,
        "exception_triggers": triggers,
    })
}

/// Writes a JSON document under the workspace root, returning its path as a
/// CLI-ready string.
fn write_json(workspace: &Workspace, name: &str, document: &serde_json::Value) -> String {
    let path = workspace.root.join(name);
    fs::write(&path, serde_json::to_string_pretty(document).unwrap()).unwrap();
    path.display().to_string()
}

/// The `project set-final-authorization-policy` argv this section exercises.
fn set_final_authorization_policy_args(
    workspace: &Workspace,
    policy_path: &str,
    dry_run: bool,
) -> Vec<String> {
    let mut args = vec![
        "project".to_owned(),
        "set-final-authorization-policy".to_owned(),
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

/// The project document currently on disk, read fresh so a test observes
/// exactly what the last command committed rather than a stale in-memory
/// copy taken before it ran.
fn stored_project_document(workspace: &Workspace) -> serde_json::Value {
    let raw = fs::read_to_string(workspace.control.join("project/project.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
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

/// The human-readable message from a failed command's JSON envelope.
fn error_message(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["message"]
        .as_str()
        .expect("a coded refusal carries a message")
        .to_owned()
}

/// Drives a sealed cycle's single card through prepare/merge/land/verify/
/// review to one reviewed final integration — the shared shape every test
/// below needs before it can raise an exception against it.
///
/// Deliberately not shared with `reviewed_final` above: that helper calls
/// `configure`, and the entire point of the tests below is that they never
/// do. Mirrors `reviewed_final`'s own steps exactly instead, so the two
/// fixtures stay identical in everything except how the policy reaches the
/// project — which is exactly the one difference this section is testing.
fn drive_to_reviewed_final(workspace: &Workspace) -> String {
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
    id
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

// `project set-final-authorization-policy` is what makes every test above
// possible to run without `configure`. It mirrors `project
// set-convergence-policy` (src/commands/project.rs) step for step: read and
// validate the policy, short-circuit a byte-identical reinstall to an
// idempotent success, refuse a cycle whose frozen `project_revision` would
// start failing `gate.rs:792`'s comparison the moment this policy moves the
// project's digest, then write, record one event, and commit.

#[test]
fn an_exception_trigger_is_reachable_through_governed_commands() {
    // No `configure` anywhere in this test, and no inline edit of
    // `project.json` either: the policy that makes the trigger below
    // reachable is installed entirely through
    // `project set-final-authorization-policy`. Before that command existed,
    // only a hand-written `project.json` could put anything into
    // `exception_triggers` at all — this is the defect this section exists
    // to remove.
    let workspace = Workspace::initialized();
    let policy = final_authorization_policy_document(&["owner"], &["convergence_budget_exhausted"]);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let before_install = workspace.control_head();

    let installed = Workspace::run(&set_final_authorization_policy_args(
        &workspace,
        &policy_path,
        false,
    ));
    assert!(
        installed.status.success(),
        "installing the final authorization policy through the governed command must succeed: {}{}",
        String::from_utf8_lossy(&installed.stdout),
        String::from_utf8_lossy(&installed.stderr)
    );
    // R9: a write that is never committed is swept away by the next
    // command's transaction, so a success test must show the control
    // repository's head actually moved and the tree is clean, not merely
    // that the CLI printed success.
    assert_ne!(
        workspace.control_head(),
        before_install,
        "installing the policy must itself create a control commit"
    );
    assert_eq!(
        support::capture(&workspace.control, &["status", "--porcelain"]),
        "",
        "installing the policy must leave the control tree clean"
    );
    assert_eq!(
        stored_project_document(&workspace)["final_authorization_policy"],
        policy,
        "the installed policy must reach the project document unchanged"
    );

    let id = drive_to_reviewed_final(&workspace);

    // The single verifiable result this section exists to deliver.
    // `workspace.integration_json` asserts a zero exit status on its own, so
    // a refusal here fails this test right at this line — including under a
    // mutation that lets the write reach the event but not `project.json`,
    // which would send the trigger straight back to being refused.
    let raised = workspace.integration_json(&[
        "exception",
        "raise",
        "--integration-id",
        &id,
        "--actor-id",
        "safety-agent",
        "--trigger",
        "convergence_budget_exhausted",
        "--evidence-ref",
        "receipt:R-001",
    ]);
    assert_eq!(raised["data"]["status"], "pending");
    assert!(raised["data"]["exception_event_id"].as_str().is_some());
}

#[test]
fn installing_the_same_policy_twice_is_an_idempotent_success() {
    let workspace = Workspace::initialized();
    let policy = final_authorization_policy_document(&["owner"], &["convergence_budget_exhausted"]);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let before = workspace.control_head();

    let first = Workspace::run(&set_final_authorization_policy_args(
        &workspace,
        &policy_path,
        false,
    ));
    assert!(
        first.status.success(),
        "the first install must succeed: {}{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let after_first = workspace.control_head();
    assert_ne!(
        after_first, before,
        "the first install must itself create a control commit"
    );
    assert_eq!(
        support::capture(&workspace.control, &["status", "--porcelain"]),
        "",
        "the first install must leave the control tree clean"
    );

    let second = Workspace::run(&set_final_authorization_policy_args(
        &workspace,
        &policy_path,
        false,
    ));
    assert!(
        second.status.success(),
        "reinstalling a byte-identical policy must succeed: {}{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(
        envelope["data"]["changed"],
        serde_json::json!(false),
        "{envelope}"
    );
    assert_eq!(
        workspace.control_head(),
        after_first,
        "an idempotent reinstall must not create a new control commit"
    );
    assert_eq!(
        stored_project_document(&workspace)["final_authorization_policy"],
        policy,
        "the stored policy must remain exactly what was installed"
    );
}

#[test]
fn a_malformed_or_invalid_policy_refuses() {
    let workspace = Workspace::initialized();
    let before = workspace.control_head();

    // Unreadable: the path names no file at all.
    let missing_path = workspace.root.join("missing-policy.json");
    let missing = Workspace::run(&set_final_authorization_policy_args(
        &workspace,
        &missing_path.display().to_string(),
        false,
    ));
    assert!(
        !missing.status.success(),
        "an unreadable policy file must refuse"
    );
    assert_eq!(error_code(&missing), "CH-CONFIG-MALFORMED", "{missing:?}");

    // Unparseable: the file exists but is not valid JSON.
    let garbage_path = workspace.root.join("garbage-policy.json");
    fs::write(&garbage_path, "{ not json").unwrap();
    let garbage = Workspace::run(&set_final_authorization_policy_args(
        &workspace,
        &garbage_path.display().to_string(),
        false,
    ));
    assert!(
        !garbage.status.success(),
        "an unparseable policy file must refuse"
    );
    assert_eq!(error_code(&garbage), "CH-CONFIG-MALFORMED", "{garbage:?}");

    // Parses, but fails `FinalAuthorizationPolicy::validate`: an empty
    // `authorizer_actor_ids` is the cheapest way there.
    let invalid = final_authorization_policy_document(&[], &[]);
    let invalid_path = write_json(&workspace, "invalid-policy.json", &invalid);
    let refused = Workspace::run(&set_final_authorization_policy_args(
        &workspace,
        &invalid_path,
        false,
    ));
    assert!(
        !refused.status.success(),
        "a policy with no declared authorizer must refuse"
    );
    assert_eq!(
        error_code(&refused),
        "CH-CONFIG-INVALID-VALUE",
        "{refused:?}"
    );

    assert_eq!(
        workspace.control_head(),
        before,
        "no refused install may move the control repository's head"
    );
    assert!(
        stored_project_document(&workspace)["final_authorization_policy"].is_null(),
        "no refused install may write a policy into the project document"
    );
}

#[test]
fn an_open_cycle_refuses_the_change() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "exception flow",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    let before = workspace.control_head();

    let policy = final_authorization_policy_document(&["owner"], &["convergence_budget_exhausted"]);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let output = Workspace::run(&set_final_authorization_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        !output.status.success(),
        "an active cycle must block the install: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-CYCLE", "{output:?}");
    let message = error_message(&output);
    assert!(
        message.contains("C-001") && message.contains("active"),
        "the refusal must name the offending cycle and its status: {message}"
    );
    assert!(
        stored_project_document(&workspace)["final_authorization_policy"].is_null(),
        "a refused install must leave the project document unchanged"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "a refused install must not move the control repository's head"
    );
}

// `an_open_cycle_refuses_the_change` above reaches only `active`, since
// `cycle activate` is the last step it takes. `Sealed` is the status this
// section's own doc comment on `run_set_final_authorization_policy` calls
// out by name: it is the one cycle status under which a live v2 acceptance
// record can actually pin this policy's digest
// (`FinalAuthorizationPolicy::authorization_unit` is literally
// `"sealed_cycle"`), so refusing here is exactly the check the doc comment
// says keeps the digest un-orphanable — and this test is what fails if a
// future change to `blocks_convergence_policy_change` ever exempts it.
#[test]
fn a_sealed_cycle_refuses_the_change() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "exception flow",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.cycle(&["seal", "--cycle-id", "C-001"]);
    let before = workspace.control_head();

    let policy = final_authorization_policy_document(&["owner"], &["convergence_budget_exhausted"]);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let output = Workspace::run(&set_final_authorization_policy_args(
        &workspace,
        &policy_path,
        false,
    ));

    assert!(
        !output.status.success(),
        "a sealed cycle must block the install: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(error_code(&output), "CH-POLICY-INVALID-CYCLE", "{output:?}");
    let message = error_message(&output);
    assert!(
        message.contains("C-001") && message.contains("sealed"),
        "the refusal must name the offending cycle and its status: {message}"
    );
    assert!(
        stored_project_document(&workspace)["final_authorization_policy"].is_null(),
        "a refused install must leave the project document unchanged"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "a refused install must not move the control repository's head"
    );
}

#[test]
fn the_dry_run_makes_every_check_and_writes_nothing() {
    // A clean project: the dry run must report success and change nothing.
    let workspace = Workspace::initialized();
    let policy = final_authorization_policy_document(&["owner"], &["convergence_budget_exhausted"]);
    let policy_path = write_json(&workspace, "policy.json", &policy);
    let before = workspace.control_head();
    let events_before = workspace.events().len();

    let preview = Workspace::run(&set_final_authorization_policy_args(
        &workspace,
        &policy_path,
        true,
    ));
    assert!(
        preview.status.success(),
        "a dry run with nothing blocking must succeed: {}{}",
        String::from_utf8_lossy(&preview.stdout),
        String::from_utf8_lossy(&preview.stderr)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(
        envelope["data"]["dry_run"],
        serde_json::json!(true),
        "{envelope}"
    );
    assert_eq!(
        workspace.control_head(),
        before,
        "a dry run must not move the control repository's head"
    );
    assert_eq!(
        support::capture(&workspace.control, &["status", "--porcelain"]),
        "",
        "a dry run must leave the control tree clean"
    );
    assert!(
        stored_project_document(&workspace)["final_authorization_policy"].is_null(),
        "a dry run must not write the policy"
    );
    assert_eq!(
        workspace.events().len(),
        events_before,
        "a dry run must record no event"
    );

    // The same blocked-cycle shape as `an_open_cycle_refuses_the_change`:
    // the preview must refuse too, so it can never promise an install the
    // real command would reject.
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "exception flow",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    let blocked_before = workspace.control_head();
    let blocked_preview = Workspace::run(&set_final_authorization_policy_args(
        &workspace,
        &policy_path,
        true,
    ));
    assert!(
        !blocked_preview.status.success(),
        "the dry run must refuse exactly like the real command once a cycle blocks: {}",
        String::from_utf8_lossy(&blocked_preview.stdout)
    );
    assert_eq!(
        error_code(&blocked_preview),
        "CH-POLICY-INVALID-CYCLE",
        "{blocked_preview:?}"
    );
    assert_eq!(
        workspace.control_head(),
        blocked_before,
        "a refused dry run must not move the control repository's head"
    );
}
