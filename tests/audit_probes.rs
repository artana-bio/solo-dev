//! The assurance command must exercise real disposable lifecycle commands.

mod support;

use std::{collections::BTreeSet, process::Command};

#[test]
fn executable_assurance_probes_reach_each_target_oracle() {
    let output = support::Workspace::run(&[
        "audit".into(),
        "probes".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert!(
        output.status.success(),
        "probe report command failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let data = &report["data"];
    assert_eq!(data["all_required_probes_passed"], true);
    assert_eq!(data["failed_probe_count"], 0);
    let probes = data["probes"].as_array().unwrap();
    let required = [
        (
            "out_of_scope_write",
            "CH-POLICY-CANDIDATE-OUT-OF-SCOPE",
            "handoff.create",
        ),
        (
            "stale_sha",
            "CH-POLICY-DELIVERED-SHA-MISMATCH",
            "handoff.create",
        ),
        ("self_review", "CH-POLICY-SELF-REVIEW", "review.begin"),
        (
            "same_session_review",
            "CH-POLICY-SAME-ACTOR",
            "review.begin",
        ),
        (
            "missing_mutation_receipt",
            "CH-POLICY-INCOMPLETE-REVIEW",
            "review.record",
        ),
        (
            "missing_human_attestation",
            "CH-POLICY-RISK-REVIEW",
            "review.record",
        ),
    ];
    let mut run_ids = BTreeSet::new();
    for (probe, expected, command) in required {
        let result = probes.iter().find(|item| item["probe"] == probe).unwrap();
        assert_eq!(result["classification"], "executed_passed");
        assert_eq!(result["observed_error_code"], expected);
        assert_eq!(result["command_path"], command);
        assert_eq!(result["refused"], true);
        assert_eq!(result["cleanup_completed"], true);
        assert!(
            !result["command_path"]
                .as_str()
                .unwrap()
                .starts_with("assurance.synthetic")
        );
        assert!(run_ids.insert(result["run_id"].as_str().unwrap().to_owned()));
        assert!(
            result["state_change_evidence"]
                .as_str()
                .unwrap()
                .contains("unchanged=true")
        );
    }
    let network = probes
        .iter()
        .find(|item| item["probe"] == "denied_network")
        .unwrap();
    assert_eq!(network["classification"], "not_tested");
    assert_eq!(network["network_declared"], "denied");
    assert_eq!(network["network_enforced"], false);
    assert!(run_ids.insert(network["run_id"].as_str().unwrap().to_owned()));
}

#[test]
fn failed_required_probes_return_a_machine_readable_refusal() {
    let empty_path = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(["audit", "probes", "--output", "json"])
        .env("PATH", empty_path.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(5));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "CH-POLICY-AUDIT-DISCREPANCY");
    assert_eq!(
        envelope["error"]["details"]["all_required_probes_passed"],
        false
    );
    assert!(
        envelope["error"]["details"]["failed_probe_count"]
            .as_u64()
            .unwrap()
            > 0
    );
    let network = envelope["error"]["details"]["probes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|probe| probe["probe"] == "denied_network")
        .unwrap();
    assert_eq!(network["classification"], "not_tested");
    assert_eq!(network["network_enforced"], false);
}
