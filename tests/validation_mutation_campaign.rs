//! #63 frozen proof: declared mutations run sequentially from one baseline.

mod support;

use std::{fs, process::Command};

use support::Workspace;

#[test]
fn a_reserved_declared_campaign_runs_two_mutations_from_one_restored_baseline() {
    let workspace = Workspace::initialized();
    workspace.register_gate("gate.mutation", &["false"]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Mutate once",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["README.md"], &["gate.mutation"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    let campaign = workspace.root.join("campaign.json");
    fs::write(
        &campaign,
        r#"{
  "schema":"harness.declared-mutation-campaign/v1",
  "mutations":[
    {"id":"M-001","path":"README.md","expected_utf8":"hello\n","replacement_utf8":"mutant-one\n"},
    {"id":"M-002","path":"README.md","expected_utf8":"hello\n","replacement_utf8":"mutant-two\n"}
  ]
}"#,
    )
    .unwrap();
    let reserve = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.mutation",
        "--execution-mode",
        "declared-mutations",
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    let reservation_id = reserve["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    let output = workspace.gate_raw(&[
        "mutate",
        "--reservation-id",
        reservation_id,
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let witnesses = result["data"]["mutation_witnesses"].as_array().unwrap();
    assert_eq!(witnesses.len(), 2);
    assert_eq!(witnesses[0]["mutation_id"], "M-001");
    assert_eq!(witnesses[1]["mutation_id"], "M-002");
    assert_eq!(witnesses[0]["observed_verdict"], "failed");
    assert_eq!(witnesses[1]["observed_verdict"], "failed");
    assert_eq!(
        witnesses[0]["restoration_digest"],
        witnesses[1]["baseline_digest"]
    );
    assert_eq!(
        result["data"]["final_baseline_digest"],
        witnesses[1]["restoration_digest"]
    );
}

#[test]
fn a_surviving_declared_mutation_refuses_without_receipt_or_settlement() {
    let workspace = Workspace::initialized();
    workspace.register_gate("gate.mutation", &["true"]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Reject survivor",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["README.md"], &["gate.mutation"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    let campaign = workspace.root.join("surviving-campaign.json");
    fs::write(
        &campaign,
        r#"{"schema":"harness.declared-mutation-campaign/v1","mutations":[{"id":"M-001","path":"README.md","expected_utf8":"hello\n","replacement_utf8":"mutant\n"}]}"#,
    )
    .unwrap();
    let reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.mutation",
        "--execution-mode",
        "declared-mutations",
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    let reservation_id = reservation["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    let output = workspace.gate_raw(&[
        "mutate",
        "--reservation-id",
        reservation_id,
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    assert!(!output.status.success());
    assert!(
        workspace.gate_json(&["status", "--card-id", "F-001"])["data"]["receipts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        !workspace
            .control
            .join(format!(
                "validation-reservation-settlements/{reservation_id}.json"
            ))
            .exists()
    );
    assert!(
        !workspace
            .control
            .join(format!(
                "validation-mutation-witnesses/{reservation_id}.json"
            ))
            .exists()
    );
}

#[test]
fn a_changed_candidate_invalidates_a_declared_campaign_reservation() {
    let workspace = Workspace::initialized();
    workspace.register_gate("gate.mutation", &["false"]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Reject stale campaign",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["README.md"], &["gate.mutation"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    let campaign = workspace.root.join("stale-campaign.json");
    fs::write(
        &campaign,
        r#"{"schema":"harness.declared-mutation-campaign/v1","mutations":[{"id":"M-001","path":"README.md","expected_utf8":"hello\n","replacement_utf8":"mutant\n"}]}"#,
    ).unwrap();
    let reservation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.mutation",
        "--execution-mode",
        "declared-mutations",
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    let reservation_id = reservation["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    let worktree = workspace.work_json(&["status", "--card-id", "F-001"])["data"]
        ["held_lease"]["worktree_path"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::write(
        std::path::Path::new(&worktree).join("README.md"),
        "changed\n",
    )
    .unwrap();
    assert!(
        Command::new("git")
            .args(["-C", &worktree, "add", "README.md"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["-C", &worktree, "commit", "-m", "change candidate"])
            .status()
            .unwrap()
            .success()
    );
    let output = workspace.gate_raw(&[
        "mutate",
        "--reservation-id",
        reservation_id,
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    assert!(!output.status.success());
    assert!(
        !workspace
            .control
            .join(format!(
                "validation-mutation-witnesses/{reservation_id}.json"
            ))
            .exists()
    );
}
