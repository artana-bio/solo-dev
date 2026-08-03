//! #63 frozen proof: declared mutations run sequentially from one baseline.

mod support;

use std::fs;

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
