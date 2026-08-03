//! #66 frozen proof: CPU-heavy reservations bind one explicit cost profile.

mod support;

use std::fs;

use support::Workspace;

fn allocated() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Reserve one CPU-heavy validation",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);
    workspace
}

fn profile(workspace: &Workspace, name: &str, duration_seconds: u64) -> std::path::PathBuf {
    let path = workspace.root.join(name);
    fs::write(
        &path,
        format!(
            r#"{{"schema":"harness.cpu-heavy-validation-profile/v1","risk":"high","expected_duration_seconds":{duration_seconds},"resource_cost":{{"cpu_cores":2,"memory_mib":1024}}}}"#
        ),
    )
    .unwrap();
    path
}

#[test]
#[allow(clippy::too_many_lines)]
fn cpu_heavy_reservations_require_and_bind_one_versioned_profile_without_changing_named_gates() {
    let workspace = allocated();
    let before = workspace.gate_raw(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--execution-mode",
        "cpu-heavy",
        "--actor",
        "holder",
    ]);
    assert!(!before.status.success(), "missing CPU profile must refuse");
    assert!(
        !workspace.control.join("validation-reservations").exists(),
        "missing profile must refuse before writing a reservation"
    );

    let malformed = workspace.root.join("malformed-profile.json");
    fs::write(
        &malformed,
        r#"{"schema":"harness.cpu-heavy-validation-profile/v1","risk":"high"}"#,
    )
    .unwrap();
    let malformed_output = workspace.gate_raw(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--execution-mode",
        "cpu-heavy",
        "--cpu-profile",
        malformed.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    assert!(
        !malformed_output.status.success(),
        "malformed profile must refuse"
    );
    assert!(
        !workspace.control.join("validation-reservations").exists(),
        "malformed profile must refuse before writing a reservation"
    );

    let first_profile = profile(&workspace, "cpu-a.json", 300);
    let first = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--execution-mode",
        "cpu-heavy",
        "--cpu-profile",
        first_profile.to_str().unwrap(),
        "--actor",
        "holder",
    ]);
    assert_eq!(first["data"]["disposition"]["kind"], "reserved");
    assert_eq!(
        first["data"]["reservation"]["key"]["execution_mode"],
        "cpu-heavy"
    );
    assert!(
        first["data"]["reservation"]["key"]["cpu_profile_digest"]
            .as_str()
            .is_some(),
        "exact CPU profile digest must be part of the durable key"
    );

    let second_profile = profile(&workspace, "cpu-b.json", 600);
    let second = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--execution-mode",
        "cpu-heavy",
        "--cpu-profile",
        second_profile.to_str().unwrap(),
        "--actor",
        "other",
    ]);
    assert_eq!(second["data"]["disposition"]["kind"], "reserved");
    assert_ne!(
        first["data"]["reservation"]["key_digest"], second["data"]["reservation"]["key_digest"],
        "a different declared cost profile must not share a CPU-heavy run"
    );

    let ordinary = workspace.gate_json(&[
        "reserve",
        "--card-id",
        "F-001",
        "--gate-id",
        "gate.unit",
        "--actor",
        "ordinary",
    ]);
    assert_eq!(ordinary["data"]["disposition"]["kind"], "reserved");
    assert_eq!(
        ordinary["data"]["reservation"]["key"]["execution_mode"],
        "named-gate"
    );
    assert!(ordinary["data"]["reservation"]["key"]["cpu_profile_digest"].is_null());
}
