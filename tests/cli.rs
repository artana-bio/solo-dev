use std::process::Command;

fn harness_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
}

#[test]
fn help_identifies_the_cli() {
    let output = harness_command()
        .arg("--help")
        .output()
        .expect("the CLI should start");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help should be UTF-8");
    assert!(stdout.contains("Coordinate bounded changes"));
    assert!(stdout.contains("doctor"));
}

#[test]
fn doctor_reports_the_repository_as_json() {
    let output = harness_command()
        .args([
            "doctor",
            "--workspace",
            env!("CARGO_MANIFEST_DIR"),
            "--format",
            "json",
        ])
        .output()
        .expect("the CLI should start");

    assert!(output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("doctor output should be JSON");
    assert_eq!(report["schema"], "harness.doctor/v1");
    assert_eq!(
        report["repository_root"],
        env!("CARGO_MANIFEST_DIR"),
        "doctor should find this repository"
    );
}

#[test]
fn doctor_rejects_a_missing_workspace() {
    let missing = tempfile::tempdir()
        .expect("temporary directory should be created")
        .path()
        .join("missing");
    let output = harness_command()
        .arg("doctor")
        .arg("--workspace")
        .arg(&missing)
        .output()
        .expect("the CLI should start");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("error output should be UTF-8");
    assert!(stderr.contains("workspace does not exist"));
}
