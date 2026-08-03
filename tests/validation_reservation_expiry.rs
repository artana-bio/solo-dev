//! #64 frozen proof: an expired reservation is recovery-required, not executable.

mod support;

use std::{
    fs,
    process::{Command, Output},
};

use support::Workspace;

fn marker_gate(workspace: &Workspace, gate_id: &str, marker: &std::path::Path, verdict: &str) {
    let command = format!("printf invoked > \"$MARKER_PATH\"; {verdict}");
    let argv = serde_json::to_string(&["sh", "-c", &command]).unwrap();
    let marker = serde_json::to_string(&marker.display().to_string()).unwrap();
    let body = format!(
        "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: 1\nargv: {argv}\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set:\n    MARKER_PATH: {marker}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n",
    );
    let definition = workspace.gate_definition(gate_id, &body);
    workspace.gate(&["register", "--definition", &definition]);
}

fn expire(workspace: &Workspace, reservation_id: &str) {
    let path = workspace
        .control
        .join(format!("validation-reservations/{reservation_id}.json"));
    let mut reservation: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    reservation["expires_at"] = serde_json::json!("1970-01-01T00:00:00Z");
    fs::write(path, serde_json::to_vec_pretty(&reservation).unwrap()).unwrap();
    assert!(
        Command::new("git")
            .args(["-C", workspace.control.to_str().unwrap(), "add", "-A"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-C",
                workspace.control.to_str().unwrap(),
                "commit",
                "-m",
                "expire reservation fixture",
            ])
            .status()
            .unwrap()
            .success()
    );
}

fn gate_with_post_acquire_failure(workspace: &Workspace, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .env("CHANGE_HARNESS_FAIL_AT", "governed-execution-after-acquire")
        .args(["gate", args[0], "--output", "json", "--control"])
        .arg(&workspace.control)
        .args(&args[1..])
        .output()
        .unwrap()
}

#[test]
fn expired_unsettled_reservations_require_recovery_before_reserve_run_or_mutate() {
    let workspace = Workspace::initialized();
    let named_marker = workspace.root.join("named-ran");
    let mutation_marker = workspace.root.join("mutation-ran");
    marker_gate(&workspace, "gate.named", &named_marker, "true");
    marker_gate(&workspace, "gate.mutation", &mutation_marker, "false");
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Expire permits",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/named/**"], &["gate.named"]);
    workspace.activate_card_with_gates("F-002", &["src/mutation/**"], &["gate.mutation"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace.work(&["start", "--card-id", "F-002"]);

    let campaign = workspace.root.join("campaign.json");
    fs::write(
        &campaign,
        r#"{"schema":"harness.declared-mutation-campaign/v1","mutations":[{"id":"M-001","path":"README.md","expected_utf8":"hello\n","replacement_utf8":"mutant\n"}]}"#,
    ).unwrap();
    let named = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.named",
        "--actor",
        "holder",
    ]);
    let mutation = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-002",
        "--gate-id",
        "gate.mutation",
        "--execution-mode",
        "declared-mutations",
        "--campaign",
        campaign.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    let named_id = named["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    let mutation_id = mutation["data"]["reservation"]["reservation_id"]
        .as_str()
        .unwrap();
    expire(&workspace, named_id);
    expire(&workspace, mutation_id);

    let preview = workspace.gate_raw(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.named",
        "--actor",
        "other",
        "--dry-run",
    ]);
    let real = workspace.gate_raw(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.named",
        "--actor",
        "other",
    ]);
    assert!(preview.status.success());
    assert!(real.status.success());
    let preview: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    let real: serde_json::Value = serde_json::from_slice(&real.stdout).unwrap();
    assert_eq!(
        preview["data"]["disposition"]["kind"],
        "expired_recovery_required"
    );
    assert_eq!(
        real["data"]["disposition"]["kind"],
        "expired_recovery_required"
    );
    assert_eq!(preview["data"]["dry_run"], true);
    assert_eq!(real["data"]["dry_run"], false);
    assert_eq!(preview["data"]["reservation"]["reservation_id"], named_id);
    assert_eq!(real["data"]["reservation"]["reservation_id"], named_id);
    assert_eq!(
        fs::read_dir(workspace.control.join("validation-reservations"))
            .unwrap()
            .count(),
        2,
        "expiry must not create a replacement reservation",
    );

    let run = gate_with_post_acquire_failure(
        &workspace,
        &[
            "run",
            "--card-id",
            "F-001",
            "--gate-id",
            "gate.named",
            "--reservation-id",
            named_id,
            "--actor",
            "holder",
        ],
    );
    let mutate = gate_with_post_acquire_failure(
        &workspace,
        &[
            "mutate",
            "--reservation-id",
            mutation_id,
            "--campaign",
            campaign.to_str().unwrap(),
            "--actor",
            "holder",
        ],
    );
    for output in [&run, &mutate] {
        assert!(!output.status.success());
        let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["error"]["code"], "CH-POLICY-INVALID-TRANSITION");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("expired"),
            "expiry refusal must win before the injected execution-setup failure: {error}",
        );
    }
    assert!(!named_marker.exists());
    assert!(!mutation_marker.exists());
    for card_id in ["F-001", "F-002"] {
        assert!(
            workspace.gate_json(&["status", "--card-id", card_id])["data"]["receipts"]
                .as_array()
                .unwrap()
                .is_empty(),
        );
    }
    for reservation_id in [named_id, mutation_id] {
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
}
