//! `WP-530` acceptance: cycle audit and evidence cross-checking.
//!
//! The report's whole value is the discrepancies. A summary of records that all
//! agree tells a reader nothing they could not get by listing files; what they
//! cannot get any other way is whether the evidence still describes the objects
//! it names. So every test here is about what happens when it does not.

mod support;

use std::fs;

use change_harness::{
    domain::{
        digest::Digest,
        integration::{IntegrationRecord, VerificationRecord},
    },
    runner::receipt::{ProvenanceDimension, Receipt},
};
use support::{Workspace, git};

fn error_code(output: &std::process::Output) -> String {
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("an error envelope");
    envelope["error"]["code"].as_str().unwrap().to_owned()
}

fn audit_raw(workspace: &Workspace, cycle: &str) -> std::process::Output {
    Workspace::run(&[
        "audit".into(),
        "cycle".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        cycle.into(),
        "--output".into(),
        "json".into(),
    ])
}

fn audit_json(workspace: &Workspace, cycle: &str) -> serde_json::Value {
    let output = audit_raw(workspace, cycle);
    assert!(
        output.status.success(),
        "audit failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).expect("the JSON envelope")
}

fn install_lesson_authorizer(workspace: &Workspace) {
    let path = workspace.root.join("audit-final-authorization.json");
    fs::write(
        &path,
        r#"{
  "version": "harness.final-authorization-policy/v1",
  "authorization_unit": "sealed_cycle",
  "authorizer_actor_ids": ["owner"]
}
"#,
    )
    .unwrap();
    let output = Workspace::run(&[
        "project".into(),
        "set-final-authorization-policy".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--policy".into(),
        path.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        output.status.success(),
        "installing final authorization failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn activate_audit_lesson(workspace: &Workspace) {
    let definition = workspace.root.join("audit-lesson.yaml");
    fs::write(
        &definition,
        "title: Audit current policy without rewriting history\nrule: Inspect the historical lesson binding\nrationale: Later policy changes are not evidence tampering\nselectors:\n  paths: [src/**]\n  contracts: []\n  change_kinds: []\n  minimum_risk: null\nenforcement: required\nobligations:\n  feature_gates: []\n  integration_gates: []\n  review_checks: [historical-binding]\nprovenance:\n  source_kind: review\n  source_id: RV-000001\n  evidence: Prior audit confused current policy with historical evidence\n",
    )
    .unwrap();
    let proposed = Workspace::run(&[
        "lesson".into(),
        "propose".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--definition".into(),
        definition.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(proposed.status.success());
    let proposed: serde_json::Value = serde_json::from_slice(&proposed.stdout).unwrap();
    let lesson_id = proposed["data"]["lesson"]["lesson_id"].as_str().unwrap();
    let activated = Workspace::run(&[
        "lesson".into(),
        "activate".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--lesson-id".into(),
        lesson_id.into(),
        "--actor".into(),
        "owner".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        activated.status.success(),
        "activating lesson failed: {}{}",
        String::from_utf8_lossy(&activated.stdout),
        String::from_utf8_lossy(&activated.stderr)
    );
}

/// A cycle carried all the way to a promoted, archived integration.
fn completed() -> Workspace {
    completed_with_id().0
}

/// A completed integration whose immutable records can be queried by an
/// integration-level compatibility request.
fn completed_with_id() -> (Workspace, String) {
    completed_with_id_using_exemption(false)
}

fn completed_with_exemption_id() -> (Workspace, String) {
    completed_with_id_using_exemption(true)
}

fn completed_with_id_using_exemption(exempt: bool) -> (Workspace, String) {
    let workspace = Workspace::initialized();
    if exempt {
        workspace.install_fixture_mutation_exemption_policy();
    }
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    if exempt {
        workspace.approve_card_with_fixture_mutation_exemption("F-001", "src/F-001/a.rs");
    } else {
        workspace.approve_card("F-001", "src/F-001/a.rs");
    }
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
    (workspace, id)
}

#[test]
fn clean_promoted_production_evidence_has_no_audit_contradictions() {
    let (workspace, _) = completed_with_id();
    let report = Workspace::run(&[
        "audit".into(),
        "report".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        report.status.success(),
        "clean production evidence must not contradict itself: {}",
        String::from_utf8_lossy(&report.stdout)
    );
    let report: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(report["data"]["contradiction_count"], 0);
    assert_eq!(
        report["data"]["receipt_reuse_evidence"]["classification"],
        "not_tested"
    );
    assert_eq!(
        report["data"]["receipt_reuse_evidence"]["unsupported_dimensions"],
        serde_json::json!(["toolchain", "inputs", "fixtures", "cache", "trust_mode"])
    );
}

/// Completes only the privacy-safe dimensions that the current runner cannot
/// yet collect. This is deliberately test-fixture data: production receipts
/// remain incomplete and must produce `rerun_required` until #56's remaining
/// collection slices land.
fn complete_fixture_provenance(receipt: &mut Receipt) {
    let provenance = receipt
        .provenance
        .as_mut()
        .expect("the receipt provenance extension is present");
    for (dimension, value) in [
        (ProvenanceDimension::Toolchain, b"test-toolchain".as_slice()),
        (ProvenanceDimension::Inputs, b"test-inputs".as_slice()),
        (ProvenanceDimension::Fixtures, b"test-fixtures".as_slice()),
        (ProvenanceDimension::Cache, b"test-cache-policy".as_slice()),
        (
            ProvenanceDimension::TrustMode,
            b"test-trust-mode".as_slice(),
        ),
    ] {
        provenance
            .dimensions
            .insert(dimension, Digest::of_bytes(value));
    }
}

fn complete_integration_request(workspace: &Workspace, integration_id: &str) -> serde_json::Value {
    let integration: IntegrationRecord = serde_json::from_str(
        &fs::read_to_string(
            workspace
                .control
                .join(format!("integrations/{integration_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let verification: VerificationRecord = serde_json::from_str(
        &fs::read_to_string(
            workspace
                .control
                .join(format!("verifications/{integration_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let receipt_id = verification
        .receipt_ids
        .first()
        .expect("verified integration must pin a final gate receipt");
    let receipt_path = workspace
        .control
        .join(format!("receipts/{receipt_id}.json"));
    let mut receipt: Receipt =
        serde_json::from_str(&fs::read_to_string(&receipt_path).unwrap()).unwrap();
    complete_fixture_provenance(&mut receipt);
    fs::write(
        &receipt_path,
        format!("{}\n", serde_json::to_string_pretty(&receipt).unwrap()),
    )
    .unwrap();
    git(&workspace.control, &["add", "receipts"]);
    git(
        &workspace.control,
        &["commit", "-qm", "complete fixture provenance"],
    );

    let expected = receipt.provenance.expect("fixture provenance");
    serde_json::json!({
        "schema": "harness.receipt-compatibility/v1",
        "context": {
            "integration_id": integration_id,
            "cycle_id": integration.cycle_id,
            "landing_sha": integration.landing_sha,
            "baseline_sha": integration.baseline_sha,
            "integration_digest": integration.substantive_digest().unwrap(),
            "verification_digest": verification.digest().unwrap(),
            "policy_digest": expected.policy_digest,
        },
        "stage": "final_integration",
        "check": {
            "gate_id": receipt.gate_id,
            "gate_digest": receipt.gate_digest,
            "receipt_schema": receipt.schema,
            "max_attempts": 1,
        },
        "expected": expected,
    })
}

fn integration_audit_projection(
    workspace: &Workspace,
    request_path: &std::path::Path,
) -> serde_json::Value {
    let output = Workspace::run(&[
        "audit".into(),
        "cycle".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--compatibility-request".into(),
        request_path.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        output.status.success(),
        "audit failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn a_clean_cycle_reports_no_discrepancies() {
    let workspace = completed();
    let envelope = audit_json(&workspace, "C-001");
    assert_eq!(
        envelope["data"]["discrepancies"].as_array().unwrap().len(),
        0,
        "unexpected: {}",
        envelope["data"]["discrepancies"]
    );
    assert!(envelope["data"]["events"].as_u64().unwrap() > 0);
}

#[test]
fn later_lesson_policy_changes_are_informational_not_historical_tampering() {
    let workspace = Workspace::initialized();
    install_lesson_authorizer(&workspace);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Freeze historical lesson evidence",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/F-001/**"]);
    workspace.approve_card("F-001", "src/F-001/a.rs");

    activate_audit_lesson(&workspace);

    let envelope = audit_json(&workspace, "C-001");
    assert!(
        envelope["data"]["discrepancies"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a current policy change must not invalidate frozen evidence: {envelope}"
    );
    let observations = envelope["data"]["policy_observations"].as_array().unwrap();
    assert_eq!(
        observations.len(),
        1,
        "the policy drift must remain visible"
    );
    assert_eq!(
        observations[0]["kind"],
        "lesson_policy_changed_since_review"
    );
    assert_ne!(
        observations[0]["frozen_manifest_digest"], observations[0]["current_manifest_digest"],
        "the observation must prove it compared two distinct manifests"
    );
}

#[test]
fn audit_projects_the_same_frozen_receipt_compatibility_decision_as_status() {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Compatibility projection",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace.gate_json(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    let preflight = workspace.gate_json(&["preflight", "--card-id", "F-001"]);
    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let receipt = status["data"]["receipts"][0].clone();
    let request = serde_json::json!({
        "plan": {
            "schema": preflight["data"]["schema"],
            "card_revision": preflight["data"]["card_revision"],
            "card_digest": preflight["data"]["card_digest"],
            "base_sha": preflight["data"]["base_sha"],
            "risk": preflight["data"]["risk"],
            "policy_digest": preflight["data"]["policy_digest"],
            "proof_map_digest": preflight["data"]["proof_map_digest"],
            "stages": preflight["data"]["stages"],
            "next_permitted_stage": preflight["data"]["next_permitted_stage"],
        },
        "stage": "narrow",
        "check": preflight["data"]["stages"][0]["checks"][0],
        "expected": receipt["provenance"],
    });
    let request_path = workspace.root.join("compatibility-request.json");
    fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
    let status_projection = workspace.gate_json(&[
        "status",
        "--card-id",
        "F-001",
        "--compatibility-request",
        request_path.to_str().unwrap(),
    ]);
    let audit = Workspace::run(&[
        "audit".into(),
        "cycle".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--compatibility-request".into(),
        request_path.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        audit.status.success(),
        "audit failed: {}",
        String::from_utf8_lossy(&audit.stdout)
    );
    let audit: serde_json::Value = serde_json::from_slice(&audit.stdout).unwrap();
    assert_eq!(
        audit["data"]["receipt_compatibility"], status_projection["data"]["receipt_compatibility"],
        "status and audit must expose the exact same read-only compatibility decision"
    );
}

#[test]
fn integration_status_and_audit_share_the_exact_verified_receipt_decision() {
    let (workspace, integration_id) = completed_with_id();
    let request = complete_integration_request(&workspace, &integration_id);
    let request_path = workspace
        .root
        .join("integration-compatibility-request.json");
    fs::write(&request_path, serde_json::to_vec_pretty(&request).unwrap()).unwrap();

    let status = workspace.gate_json(&[
        "status",
        "--integration-id",
        request["context"]["integration_id"].as_str().unwrap(),
        "--compatibility-request",
        request_path.to_str().unwrap(),
    ]);
    let audit = integration_audit_projection(&workspace, &request_path);
    assert_eq!(
        status["data"]["receipt_compatibility"], audit["data"]["receipt_compatibility"],
        "both read paths must expose the exact same verification-pinned decision"
    );
    assert_eq!(
        status["data"]["receipt_compatibility"]["disposition"]["kind"],
        "compatible_reuse"
    );

    let mut changed_fixture_request = request;
    changed_fixture_request["expected"]["dimensions"]["fixtures"] =
        serde_json::json!(Digest::of_bytes(b"different-fixture"));
    let changed_fixture_path = workspace.root.join("changed-fixture-request.json");
    fs::write(
        &changed_fixture_path,
        serde_json::to_vec_pretty(&changed_fixture_request).unwrap(),
    )
    .unwrap();
    let stale = workspace.gate_json(&[
        "status",
        "--integration-id",
        integration_id.as_str(),
        "--compatibility-request",
        changed_fixture_path.to_str().unwrap(),
    ]);
    assert_eq!(
        stale["data"]["receipt_compatibility"]["disposition"]["kind"],
        "rerun_required"
    );
    assert_eq!(
        stale["data"]["receipt_compatibility"]["disposition"]["reasons"],
        serde_json::json!(["fixtures"])
    );
}

#[test]
fn audit_report_surfaces_missing_verification_receipt_and_cannot_claim_machine_checked() {
    let (workspace, integration_id) = completed_with_id();
    let verification_path = workspace
        .control
        .join(format!("verifications/{integration_id}.json"));
    let verification: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&verification_path).unwrap()).unwrap();
    let receipt_id = verification["receipt_ids"][0].as_str().unwrap();
    fs::remove_file(
        workspace
            .control
            .join(format!("receipts/{receipt_id}.json")),
    )
    .unwrap();
    let report = Workspace::run(&[
        "audit".into(),
        "report".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        !report.status.success(),
        "contradictory report must fail closed"
    );
    let report: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(
        report["error"]["code"], "CH-POLICY-AUDIT-DISCREPANCY",
        "report failure envelope: {report}"
    );
}

#[test]
fn audit_report_surfaces_a_review_mutation_receipt_deleted_after_approval() {
    let (workspace, integration_id) = completed_with_id();
    let reviews_dir = workspace.control.join("reviews");
    let review_path = fs::read_dir(&reviews_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.file_name().unwrap().to_string_lossy().contains("RV-"))
        .expect("approved review record");
    let mut review: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&review_path).unwrap()).unwrap();
    review["mutation_receipt_ids"] = serde_json::json!(["MR-AUDIT-DELETED"]);
    review["mutation_exemption"] = serde_json::Value::Null;
    fs::write(&review_path, serde_json::to_vec_pretty(&review).unwrap()).unwrap();
    git(&workspace.control, &["add", "-A"]);
    git(
        &workspace.control,
        &["commit", "-q", "-m", "delete review mutation evidence"],
    );

    let report = Workspace::run(&[
        "audit".into(),
        "report".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        !report.status.success(),
        "audit must fail on lost review evidence"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-AUDIT-DISCREPANCY");
    let details = &envelope["error"]["details"];
    assert!(
        details["discrepancies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["claim"]
                .as_str()
                .unwrap_or_default()
                .contains("mutation")),
        "review mutation evidence discrepancy must be explicit: {details}"
    );
    assert_eq!(details["cycle_id"], "C-001");
    let _ = integration_id;
}

#[test]
fn audit_report_surfaces_a_review_exemption_policy_discrepancy() {
    let (workspace, _) = completed_with_exemption_id();
    let project_path = workspace.control.join("project/project.json");
    let mut project: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["mutation_exemption_policy"] = serde_json::Value::Null;
    fs::write(&project_path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();
    git(&workspace.control, &["add", "-A"]);
    git(
        &workspace.control,
        &["commit", "-q", "-m", "remove exemption policy"],
    );

    let report = Workspace::run(&[
        "audit".into(),
        "report".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        !report.status.success(),
        "audit must fail on lost exemption policy"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-AUDIT-DISCREPANCY");
    let discrepancies = &envelope["error"]["details"]["discrepancies"];
    assert!(
        discrepancies
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["claim"]
                .as_str()
                .unwrap_or_default()
                .contains("mutation")),
        "audit must identify exemption evidence: {discrepancies}"
    );
}

fn report_error_details(workspace: &Workspace) -> serde_json::Value {
    let report = Workspace::run(&[
        "audit".into(),
        "report".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(!report.status.success());
    let report: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(
        report["error"]["code"], "CH-POLICY-AUDIT-DISCREPANCY",
        "corruption report envelope: {report}"
    );
    report["error"]["details"].clone()
}

#[test]
#[allow(clippy::too_many_lines)]
fn audit_report_corruption_matrix_fails_closed_with_exact_findings() {
    for corruption in [
        "foreign_receipt",
        "failed_receipt",
        "wrong_landing_sha",
        "wrong_landing_tree",
        "proof_id_mismatch",
        "oracle_mismatch",
        "policy_digest_drift",
    ] {
        let (workspace, integration_id) = completed_with_id();
        let verification_path = workspace
            .control
            .join(format!("verifications/{integration_id}.json"));
        let mut verification: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&verification_path).unwrap()).unwrap();
        let receipt_id = verification["receipt_ids"][0].as_str().unwrap().to_owned();
        verification["invariants"] = serde_json::json!([{
            "proof_entry_id": "proof-behavior",
            "invariant": "it works",
            "machine_checked": true,
            "observed_receipt_ids": [receipt_id]
        }]);
        let card_path = workspace.control.join("cards/F-001/r1.json");
        let mut card: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&card_path).unwrap()).unwrap();
        card["proof_map"] = serde_json::json!({
            "schema": "harness.proof-map/v1",
            "entries": [{
                "id": "proof-behavior",
                "invariant": "it works",
                "precondition": "fixture",
                "assertion": "gate passes",
                "mutation": "gate fails",
                "gate_oracle": "gate.unit"
            }],
            "claim_boundary": "fixture"
        });
        fs::write(&card_path, serde_json::to_vec_pretty(&card).unwrap()).unwrap();
        match corruption {
            "foreign_receipt" | "failed_receipt" | "wrong_landing_sha" => {
                let path = workspace
                    .control
                    .join(format!("receipts/{receipt_id}.json"));
                let mut receipt: serde_json::Value =
                    serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
                match corruption {
                    "foreign_receipt" => receipt["integration_id"] = serde_json::json!("INT-999"),
                    "failed_receipt" => receipt["passed"] = serde_json::json!(false),
                    _ => receipt["evaluated_sha"] = serde_json::json!("stale-sha"),
                }
                fs::write(&path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
            }
            "wrong_landing_tree" => verification["landing_tree"] = serde_json::json!("wrong-tree"),
            "proof_id_mismatch" => {
                verification["invariants"][0]["proof_entry_id"] =
                    serde_json::json!("missing-proof");
            }
            "oracle_mismatch" => {
                let path = workspace.control.join("cards/F-001/r1.json");
                let mut card: serde_json::Value =
                    serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
                card["proof_map"]["entries"][0]["gate_oracle"] = serde_json::json!("gate.missing");
                fs::write(path, serde_json::to_vec_pretty(&card).unwrap()).unwrap();
            }
            "policy_digest_drift" => {
                let path = fs::read_dir(workspace.control.join("acceptances"))
                    .unwrap()
                    .find_map(Result::ok)
                    .unwrap()
                    .path();
                let mut acceptance: serde_json::Value =
                    serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
                acceptance["final_authorization_policy_digest"] =
                    serde_json::json!(format!("sha256:{}", "0".repeat(64)));
                fs::write(path, serde_json::to_vec_pretty(&acceptance).unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        fs::write(
            &verification_path,
            serde_json::to_vec_pretty(&verification).unwrap(),
        )
        .unwrap();
        let details = report_error_details(&workspace);
        assert_eq!(
            details["all_claims_supported"], false,
            "{corruption}: {details}"
        );
        assert!(details["contradiction_count"].as_u64().unwrap() > 0);
        assert!(
            details["claims"]
                .as_array()
                .unwrap()
                .iter()
                .all(|claim| claim["classification"] != "machine_checked")
        );
        let expected_classification = match corruption {
            "foreign_receipt"
            | "failed_receipt"
            | "wrong_landing_sha"
            | "wrong_landing_tree"
            | "policy_digest_drift"
            | "oracle_mismatch"
            | "proof_id_mismatch" => "failed",
            _ => unreachable!(),
        };
        assert!(
            details["claims"]
                .as_array()
                .unwrap()
                .iter()
                .all(|claim| claim["classification"] == expected_classification),
            "{corruption}: expected {expected_classification}: {details}"
        );
        let subjects: Vec<&str> = details["discrepancies"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["subject"].as_str())
            .collect();
        let expected_subject = match corruption {
            "policy_digest_drift" => "acceptance ACC-000001",
            "wrong_landing_tree" => "verification INT-001",
            "proof_id_mismatch" => "missing-proof",
            "oracle_mismatch" => "proof-behavior",
            _ => &format!("receipt {receipt_id}"),
        };
        assert!(
            subjects.contains(&expected_subject),
            "{corruption}: {subjects:?}"
        );
    }
}

#[test]
fn the_timeline_reconstructs_the_cycle_in_order() {
    let workspace = completed();
    let envelope = audit_json(&workspace, "C-001");
    let timeline = envelope["data"]["timeline"].as_array().unwrap();
    let events = workspace.events();

    let types: Vec<&str> = timeline
        .iter()
        .map(|entry| entry["type"].as_str().unwrap())
        .collect();
    // Order matters, not just membership: a set would not show that review
    // followed handoff rather than preceding it.
    let handoff = types.iter().position(|t| *t == "handoff.created");
    let review = types.iter().position(|t| *t == "review.recorded");
    let promoted = types.iter().position(|t| *t == "integration.promoted");
    assert!(handoff < review, "review must follow handoff: {types:?}");
    assert!(review < promoted, "promotion must follow review: {types:?}");
    for entry in timeline {
        let event = events
            .iter()
            .find(|event| event["event_id"] == entry["event_id"])
            .expect("every timeline entry must name an event");
        assert_eq!(
            entry["at"], event["occurred_at"],
            "timeline timestamp must reproduce its source event: {entry}"
        );
    }
}

#[test]
fn the_report_names_the_exact_protected_branch_transition() {
    let workspace = completed();
    let envelope = audit_json(&workspace, "C-001");
    let events = workspace.events();
    let transitions = envelope["data"]["protected_branch_transitions"]
        .as_array()
        .unwrap();

    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0]["to"], workspace.authority_head());
    assert!(
        transitions[0]["from"].as_str().unwrap() != workspace.authority_head(),
        "both ends of the transition must be named: {}",
        transitions[0]
    );
    assert!(
        transitions[0]["acceptance_id"]
            .as_str()
            .unwrap()
            .starts_with("ACC-"),
        "the authorizing decision must be named: {}",
        transitions[0]
    );
    let promotion = events
        .iter()
        .find(|event| event["event_type"] == "integration.promoted")
        .expect("the promoted integration must have an event");
    assert_eq!(
        transitions[0]["at"], promotion["occurred_at"],
        "protected-branch transition timestamp must reproduce its source event: {}",
        transitions[0]
    );
}

#[test]
fn a_card_revision_that_no_longer_matches_its_review_is_reported() {
    let workspace = completed();
    // Tamper with the stored revision so its digest no longer matches what the
    // review was bound to. This is what an edited immutable record looks like.
    let path = workspace.control.join("cards/F-001/r1.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    value["title"] = serde_json::json!("something else entirely");
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();

    let output = audit_raw(&workspace, "C-001");
    assert!(!output.status.success(), "a tampered record must not pass");
    assert_eq!(error_code(&output), "CH-POLICY-AUDIT-DISCREPANCY");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("review RV-"),
        "the affected record must be named: {envelope}"
    );
}

#[test]
fn evidence_a_record_refers_to_but_which_is_absent_is_reported() {
    let workspace = completed();
    // Gate logs live outside control history and have a retention window, so
    // this is the ordinary way evidence goes missing rather than a contrived
    // corruption.
    fs::remove_dir_all(workspace.control.join("logs")).unwrap();

    let output = audit_raw(&workspace, "C-001");
    assert!(!output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("receipt R-"),
        "the receipt whose logs are gone must be named: {envelope}"
    );
}

#[test]
fn a_missing_card_revision_is_reported_rather_than_skipped() {
    let workspace = completed();
    fs::remove_file(workspace.control.join("cards/F-001/r1.json")).unwrap();

    let output = audit_raw(&workspace, "C-001");
    assert!(
        !output.status.success(),
        "a record whose subject is gone must not read as clean"
    );
    assert_eq!(error_code(&output), "CH-POLICY-AUDIT-DISCREPANCY");
}

#[test]
fn gate_log_contents_never_appear_in_the_report() {
    let workspace = Workspace::initialized();
    // A gate that prints something a reader must never find in a report.
    workspace.register_gate(
        "gate.leaky",
        &["sh", "-c", "echo 'AKIAIOSFODNN7EXAMPLE super-secret'"],
    );
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gate_sets(
        "F-001",
        &["src/F-001/**"],
        &["gate.leaky"],
        &["gate.all"],
    );
    workspace.work(&["start", "--card-id", "F-001"]);
    let worktree = workspace.worktrees.join("F-001");
    fs::create_dir_all(worktree.join("src/F-001")).unwrap();
    fs::write(worktree.join("src/F-001/a.rs"), "// work\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: work"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.leaky"]);

    // The secret really is on disk, or this test proves nothing.
    let logs = support::capture_stdout_of_logs(&workspace.control);
    assert!(
        logs.contains("AKIAIOSFODNN7EXAMPLE"),
        "the fixture must actually have written the secret"
    );

    let output = audit_raw(&workspace, "C-001");
    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(
        !rendered.contains("AKIAIOSFODNN7EXAMPLE"),
        "gate output must never reach the report"
    );
    assert!(
        !rendered.contains("super-secret"),
        "gate output must never reach the report"
    );
}

#[test]
fn auditing_an_unknown_cycle_is_a_precondition_failure() {
    let workspace = Workspace::initialized();
    let output = audit_raw(&workspace, "C-404");
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(error_code(&output), "CH-PRECONDITION-NOT-FOUND");
}

#[test]
fn the_report_is_reproducible_from_control_state_alone() {
    let workspace = completed();
    let first = audit_json(&workspace, "C-001");
    let second = audit_json(&workspace, "C-001");
    assert_eq!(
        first["data"], second["data"],
        "two runs over unchanged state must agree exactly"
    );
}

#[test]
fn a_crashed_operations_residue_never_reaches_control_history() {
    // Tier 2, defect 9. Control commits staged with `git add -A`, so anything
    // sitting in the control directory when a mutation committed was swept into
    // authoritative history. The ignore list covered `harness.lock` exactly and
    // therefore missed the lock's own scratch file, `harness.lock.staging.<pid>
    // .<n>`, which exists for a window on every single acquisition. A gate's
    // crash dump or a half-written record went in the same way.
    let workspace = Workspace::initialized();

    // Residue of three kinds: the lock's scratch file, an interrupted atomic
    // write, and something a crashed process left behind.
    fs::write(
        workspace.control.join("harness.lock.staging.4242.0"),
        "pid: 4242\n",
    )
    .unwrap();
    fs::create_dir_all(workspace.control.join("cards")).unwrap();
    fs::write(workspace.control.join("cards/F-001.tmp"), "half a card\n").unwrap();
    fs::write(workspace.control.join("crash-dump.txt"), "secrets\n").unwrap();

    // Any mutating command commits control state.
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);

    let tracked = workspace.control_tracked_files();
    // The fixture must have committed something, or "nothing was swept in" is
    // true for the wrong reason.
    assert!(
        tracked.iter().any(|path| path.starts_with("cycles/")),
        "the cycle must actually have been recorded: {tracked:?}"
    );
    for residue in [
        "harness.lock.staging.4242.0",
        "cards/F-001.tmp",
        "crash-dump.txt",
    ] {
        assert!(
            !tracked.iter().any(|path| path == residue),
            "{residue} reached authoritative history: {tracked:?}"
        );
    }
}

#[test]
fn control_history_holds_everything_a_lifecycle_writes() {
    // The guard on the fix above. Staging by allowlist trades sweeping things
    // in for leaving things out, and leaving a record out is worse: control
    // state on disk would diverge from control state in Git, silently. After a
    // full lifecycle nothing the harness wrote may be untracked.
    let workspace = completed();

    let status = support::capture(
        &workspace.control,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    );
    assert!(
        status.trim().is_empty(),
        "control state on disk must match control state in Git:\n{status}"
    );
}

#[test]
fn recorded_timestamps_come_from_the_host_clock() {
    // The audit's fourth surviving mutation: replacing `SystemClock::now` with
    // a fixed constant passed the entire suite. Every timestamp in the audit
    // trail — when a card was activated, when a gate ran, when an authority
    // moved — became the same fabricated instant, and nothing noticed. The
    // whole value of the trail is that it says when things happened.
    //
    // Every other test uses `FixedClock` on purpose, for determinism. That is
    // correct and is exactly why nothing was left checking the real one.
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();

    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);

    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();

    let recorded = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "cycle.created")
        .expect("the cycle must have been recorded");
    let stamp = recorded["occurred_at"]
        .as_str()
        .expect("a timestamp on the event");

    // Parsed as seconds since the epoch by hand rather than by pulling in a
    // date library: the point is only that it sits between two readings of the
    // host clock taken around the command.
    let seconds = unix_seconds(stamp);
    assert!(
        seconds >= before && seconds <= after,
        "recorded {stamp} ({seconds}) is outside [{before}, {after}], so it did not come from this machine's clock"
    );
}

/// Converts an RFC 3339 UTC timestamp to seconds since the epoch.
fn unix_seconds(stamp: &str) -> u64 {
    let (date, rest) = stamp.split_once('T').expect("an RFC 3339 timestamp");
    let time = rest.trim_end_matches('Z');
    let part = |text: &str, index: usize, separator: char| -> u64 {
        text.split(separator)
            .nth(index)
            .expect("a component")
            .parse()
            .expect("a number")
    };
    let (year, month, day) = (part(date, 0, '-'), part(date, 1, '-'), part(date, 2, '-'));
    let time = time.split('.').next().expect("a time");
    let (hour, minute, second) = (part(time, 0, ':'), part(time, 1, ':'), part(time, 2, ':'));

    // Days since the epoch, by the civil-from-days algorithm.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    days * 86_400 + hour * 3_600 + minute * 60 + second
}
