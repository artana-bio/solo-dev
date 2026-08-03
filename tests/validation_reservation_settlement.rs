//! #60 acceptance: terminal facts for validation reservations.

mod support;

use std::{fs, process::Command};

use support::Workspace;

fn allocated() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Settle proof",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace
}

fn reservation(workspace: &Workspace) -> String {
    workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--actor",
        "holder",
    ])["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn external_receipt(workspace: &Workspace) -> String {
    let producer = allocated();
    let producer_reservation = reservation(&producer);
    let receipt = producer.gate_json(&[
        "run",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--reservation-id",
        &producer_reservation,
        "--actor",
        "holder",
    ])["data"]["receipt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let source = producer.control.join(format!("receipts/{receipt}.json"));
    let target = workspace.control.join(format!("receipts/{receipt}.json"));
    assert!(
        source.exists(),
        "producer receipt missing at {}",
        source.display()
    );
    assert!(
        !target.exists(),
        "external fixture must not overwrite a control record"
    );
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    let mut external: serde_json::Value =
        serde_json::from_slice(&fs::read(source).unwrap()).unwrap();
    let reservation: serde_json::Value = serde_json::from_slice(
        &fs::read(
            workspace
                .control
                .join("validation-reservations/VR-000001.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let project: serde_json::Value =
        serde_json::from_slice(&fs::read(workspace.control.join("project/project.json")).unwrap())
            .unwrap();
    let key = &reservation["key"];
    external["project_id"] = project["project_id"].clone();
    external["cycle_id"] = key["cycle_id"].clone();
    external["card_id"] = key["card_id"].clone();
    external["card_digest"] = key["card_digest"].clone();
    external["evaluated_sha"] = key["candidate_sha"].clone();
    external["gate_id"] = key["check"]["gate_id"].clone();
    external["gate_digest"] = key["check"]["gate_digest"].clone();
    let provenance = &mut external["provenance"];
    provenance["gate_definition_digest"] = key["check"]["gate_digest"].clone();
    provenance["policy_digest"] = key["policy_digest"].clone();
    provenance["subject"]["candidate_sha"] = key["candidate_sha"].clone();
    provenance["subject"]["base_sha"] = key["base_sha"].clone();
    provenance["subject"]["cycle_id"] = key["cycle_id"].clone();
    provenance["subject"]["card_id"] = key["card_id"].clone();
    provenance["subject"]["card_revision"] = key["card_revision"].clone();
    provenance["subject"]["card_digest"] = key["card_digest"].clone();
    provenance["subject"]["lease_id"] = key["lease_id"].clone();
    fs::write(target, serde_json::to_vec_pretty(&external).unwrap()).unwrap();
    receipt
}

fn settle_receipt(
    workspace: &Workspace,
    reservation_id: &str,
    receipt_id: &str,
    actor: &str,
) -> std::process::Output {
    workspace.gate_raw(&[
        "settle",
        "--reservation-id",
        reservation_id,
        "--receipt-id",
        receipt_id,
        "--actor",
        actor,
    ])
}

fn settle_with_failure(
    workspace: &Workspace,
    reservation_id: &str,
    receipt_id: &str,
    step: &str,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .env("CHANGE_HARNESS_FAIL_AT", step)
        .args([
            "gate",
            "settle",
            "--output",
            "json",
            "--control",
            workspace.control.to_str().unwrap(),
            "--reservation-id",
            reservation_id,
            "--receipt-id",
            receipt_id,
            "--actor",
            "holder",
        ])
        .output()
        .unwrap()
}

#[test]
fn holder_settles_an_exact_matching_receipt_once() {
    let workspace = allocated();
    let reservation_id = reservation(&workspace);
    let receipt_id = external_receipt(&workspace);
    let output = settle_receipt(&workspace, &reservation_id, &receipt_id, "holder");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["settlement"]["outcome"]["kind"],
        "receipt_recorded"
    );
    assert_eq!(
        envelope["data"]["settlement"]["outcome"]["receipt_id"],
        receipt_id
    );
    assert!(
        workspace
            .control
            .join(format!(
                "validation-reservation-settlements/{reservation_id}.json"
            ))
            .exists()
    );
    let settled = workspace
        .events()
        .into_iter()
        .find(|event| event["event_type"] == "validation.reservation_settled")
        .expect("settlement must be auditable");
    assert_eq!(settled["actor_id"], "holder");
    assert_eq!(settled["metadata"]["reservation_id"], reservation_id);
    assert_eq!(
        settled["metadata"]["reservation_key_digest"],
        envelope["data"]["settlement"]["reservation_key_digest"]
    );

    let next = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--actor",
        "other",
    ]);
    assert_eq!(next["data"]["disposition"]["kind"], "settled");
}

#[test]
fn a_non_holder_or_second_settlement_is_refused_without_rewrite() {
    let workspace = allocated();
    let reservation_id = reservation(&workspace);
    let receipt_id = external_receipt(&workspace);
    let denied = settle_receipt(&workspace, &reservation_id, &receipt_id, "other");
    assert!(!denied.status.success());
    assert!(
        !workspace
            .control
            .join(format!(
                "validation-reservation-settlements/{reservation_id}.json"
            ))
            .exists()
    );

    let workspace = allocated();
    let reservation_id = reservation(&workspace);
    let receipt_id = external_receipt(&workspace);
    assert!(
        settle_receipt(&workspace, &reservation_id, &receipt_id, "holder")
            .status
            .success()
    );
    let path = workspace.control.join(format!(
        "validation-reservation-settlements/{reservation_id}.json"
    ));
    let first = fs::read(&path).unwrap();
    let duplicate = settle_receipt(&workspace, &reservation_id, &receipt_id, "holder");
    assert!(!duplicate.status.success());
    assert_eq!(
        fs::read(&path).unwrap(),
        first,
        "terminal record remains immutable"
    );
}

fn tampered_settlement_is_refused(field: &str, forged: serde_json::Value) {
    let workspace = allocated();
    let reservation_id = reservation(&workspace);
    let receipt_id = external_receipt(&workspace);
    assert!(
        settle_receipt(&workspace, &reservation_id, &receipt_id, "holder")
            .status
            .success()
    );
    let path = workspace.control.join(format!(
        "validation-reservation-settlements/{reservation_id}.json"
    ));
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    tampered[field] = forged;
    fs::write(&path, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();
    let corrupted = workspace.gate_raw(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--actor",
        "other",
    ]);
    assert!(!corrupted.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&corrupted.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-INTERNAL-CONTROL-CORRUPT");
}

#[test]
fn a_tampered_settlement_key_or_holder_is_refused() {
    tampered_settlement_is_refused("holder_actor_id", serde_json::json!("forged-holder"));
    tampered_settlement_is_refused(
        "reservation_key_digest",
        serde_json::json!("sha256:forged-key-digest"),
    );
}

#[test]
fn a_receipt_for_a_moved_candidate_cannot_settle_an_earlier_reservation() {
    let workspace = allocated();
    let reservation_id = reservation(&workspace);
    let receipt_id = external_receipt(&workspace);
    let path = workspace
        .control
        .join(format!("receipts/{receipt_id}.json"));
    let mut moved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    moved["evaluated_sha"] = serde_json::json!("f".repeat(40));
    fs::write(&path, serde_json::to_vec_pretty(&moved).unwrap()).unwrap();
    let output = settle_receipt(&workspace, &reservation_id, &receipt_id, "holder");
    assert!(!output.status.success());
    assert!(
        !workspace
            .control
            .join(format!(
                "validation-reservation-settlements/{reservation_id}.json"
            ))
            .exists()
    );
}

#[test]
fn failed_and_abandoned_are_terminal_without_receipt_links() {
    for outcome in ["failed", "abandoned"] {
        let workspace = allocated();
        let reservation_id = reservation(&workspace);
        let output = workspace.gate_raw(&[
            "settle",
            "--reservation-id",
            &reservation_id,
            "--outcome",
            outcome,
            "--actor",
            "holder",
        ]);
        assert!(output.status.success());
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["data"]["settlement"]["outcome"]["kind"], outcome);
        assert!(value["data"]["settlement"]["outcome"]["receipt_id"].is_null());
        let next = workspace.gate_json(&[
            "reserve",
            "--card-id",
            "F-001",
            "--gate-id",
            "gate.unit",
            "--actor",
            "other",
        ]);
        assert_eq!(next["data"]["disposition"]["kind"], "settled");
        assert_eq!(next["data"]["settlement"]["outcome"]["kind"], outcome);
    }
}

#[test]
fn an_interrupted_settlement_creates_no_terminal_fact_or_false_receipt_link() {
    let workspace = allocated();
    let reservation_id = reservation(&workspace);
    let receipt_id = external_receipt(&workspace);
    let interrupted = settle_with_failure(
        &workspace,
        &reservation_id,
        &receipt_id,
        "reservation-settlement-write",
    );
    assert!(!interrupted.status.success());
    let path = workspace.control.join(format!(
        "validation-reservation-settlements/{reservation_id}.json"
    ));
    assert!(
        !path.exists(),
        "a pre-write interruption must not create a false receipt link"
    );
    let retry = settle_receipt(&workspace, &reservation_id, &receipt_id, "holder");
    assert!(
        !retry.status.success(),
        "the transaction must require explicit project recovery, never silently retry"
    );
    let recovery = Workspace::run_json(&[
        "project".into(),
        "recover".into(),
        "--control".into(),
        workspace.control.display().to_string(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(recovery["data"]["recovery_required"].as_bool().unwrap());
}
