//! End-to-end CLI behavior.

use std::process::{Command, Output};

fn harness_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
}

fn run(args: &[&str]) -> Output {
    harness_command()
        .args(args)
        .output()
        .expect("the CLI should start")
}

fn stdout_json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).expect("stdout should be JSON")
}

#[test]
fn help_identifies_the_cli() {
    let output = run(&["--help"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help should be UTF-8");
    assert!(stdout.contains("Coordinate bounded changes"));
    assert!(stdout.contains("doctor"));
}

#[test]
fn help_does_not_advertise_unimplemented_commands() {
    let stdout = String::from_utf8(run(&["--help"]).stdout).expect("help should be UTF-8");
    // Section 12.3 lists these, but they must stay absent until their owning
    // work packages implement them.
    for absent in ["card", "work", "gate", "handoff", "review", "integration"] {
        assert!(
            !stdout.contains(&format!("  {absent}")),
            "help advertises unimplemented `{absent}`"
        );
    }
}

#[test]
fn doctor_reports_the_repository_as_json() {
    let output = run(&[
        "doctor",
        "--workspace",
        env!("CARGO_MANIFEST_DIR"),
        "--format",
        "json",
    ]);

    assert!(output.status.success());
    let report = stdout_json(&output);
    assert_eq!(report["schema"], "harness.doctor/v1");
    assert_eq!(
        report["repository_root"],
        env!("CARGO_MANIFEST_DIR"),
        "doctor should find this repository"
    );
}

#[test]
fn deprecated_format_option_warns_on_stderr_but_keeps_its_payload() {
    let output = run(&[
        "doctor",
        "--workspace",
        env!("CARGO_MANIFEST_DIR"),
        "--format",
        "json",
    ]);

    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr.clone()).expect("stderr should be UTF-8");
    assert!(stderr.contains("`--format` is deprecated"));
    // The advisory must not contaminate stdout, which a consumer pipes to a
    // JSON parser.
    assert_eq!(stdout_json(&output)["schema"], "harness.doctor/v1");
}

#[test]
fn output_option_emits_the_stable_result_envelope() {
    let output = run(&[
        "doctor",
        "--workspace",
        env!("CARGO_MANIFEST_DIR"),
        "--output",
        "json",
    ]);

    assert!(output.status.success());
    let envelope = stdout_json(&output);
    assert_eq!(envelope["schema"], "harness.command-result/v1");
    assert_eq!(envelope["status"], "success");
    assert_eq!(envelope["command"], "doctor");
    assert!(envelope["project_id"].is_null());
    assert!(envelope["operation_id"].is_null());
    assert_eq!(envelope["warnings"].as_array().unwrap().len(), 0);
    assert_eq!(envelope["data"]["schema"], "harness.doctor/v1");
    assert_eq!(
        envelope["data"]["repository_root"],
        env!("CARGO_MANIFEST_DIR")
    );
}

#[test]
fn doctor_reports_version_compliance_worktree_support_and_role() {
    let output = run(&[
        "doctor",
        "--workspace",
        env!("CARGO_MANIFEST_DIR"),
        "--output",
        "json",
    ]);

    assert!(output.status.success());
    let data = &stdout_json(&output)["data"];
    assert!(data["meets_minimum_git_version"].as_bool().unwrap());
    assert_eq!(data["minimum_git_version"], "2.50.0");
    assert!(data["supports_worktrees"].as_bool().unwrap());
    assert_eq!(data["repository"]["kind"], "repository");
    let role = data["workspace_role"].as_str().unwrap();
    assert!(
        role == "main worktree" || role == "linked worktree",
        "unexpected role: {role}"
    );
}

#[test]
fn doctor_text_reports_the_extended_diagnostics() {
    let stdout =
        String::from_utf8(run(&["doctor", "--workspace", env!("CARGO_MANIFEST_DIR")]).stdout)
            .expect("stdout should be UTF-8");
    assert!(stdout.contains("meets minimum 2.50.0"));
    assert!(stdout.contains("worktree support: yes"));
    assert!(stdout.contains("role: "));
}

#[test]
fn output_text_matches_the_default_rendering() {
    let explicit = run(&[
        "doctor",
        "--workspace",
        env!("CARGO_MANIFEST_DIR"),
        "--output",
        "text",
    ]);
    let default = run(&["doctor", "--workspace", env!("CARGO_MANIFEST_DIR")]);

    assert!(explicit.status.success() && default.status.success());
    assert_eq!(explicit.stdout, default.stdout);
    let stdout = String::from_utf8(explicit.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("Change Harness doctor"));
    assert!(!stdout.contains('{'), "text mode must not emit JSON");
}

#[test]
fn combining_output_and_format_is_a_usage_error() {
    let output = run(&[
        "doctor",
        "--workspace",
        env!("CARGO_MANIFEST_DIR"),
        "--output",
        "json",
        "--format",
        "json",
    ]);

    assert_eq!(output.status.code(), Some(2), "usage category is exit 2");
    let envelope = stdout_json(&output);
    assert_eq!(envelope["error"]["code"], "CH-USAGE-CONFLICTING-OPTIONS");
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

#[test]
fn missing_workspace_uses_the_precondition_exit_code() {
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

    assert_eq!(
        output.status.code(),
        Some(4),
        "precondition category is exit 4"
    );
}

#[test]
fn json_mode_renders_failures_as_the_error_envelope() {
    let missing = tempfile::tempdir()
        .expect("temporary directory should be created")
        .path()
        .join("missing");
    let output = harness_command()
        .args(["doctor", "--output", "json", "--workspace"])
        .arg(&missing)
        .output()
        .expect("the CLI should start");

    assert_eq!(output.status.code(), Some(4));
    let envelope = stdout_json(&output);
    assert_eq!(envelope["schema"], "harness.command-error/v1");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["command"], "doctor");
    assert_eq!(
        envelope["error"]["code"],
        "CH-PRECONDITION-WORKSPACE-MISSING"
    );
    assert!(!envelope["error"]["message"].as_str().unwrap().is_empty());
    assert!(!envelope["error"]["recovery"].as_str().unwrap().is_empty());
    assert!(envelope["error"]["details"]["path"].is_string());
}

#[test]
fn invalid_arguments_use_the_usage_exit_code() {
    let output = run(&["doctor", "--output", "yaml"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "clap argument rejection is the usage category"
    );
}

#[test]
fn unknown_subcommand_is_rejected() {
    let output = run(&["cycle", "create"]);
    assert_eq!(output.status.code(), Some(2));
}
