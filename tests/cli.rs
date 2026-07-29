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
fn help_advertises_every_section_12_3_command() {
    let stdout = String::from_utf8(run(&["--help"]).stdout).expect("help should be UTF-8");
    // The inverse of what this test used to assert. Through `WP-450` it named
    // the commands that must stay absent until their owning package shipped;
    // `WP-460` was the last of them, so the whole Section 12.3 surface is now
    // present and the check is that none of it went missing again.
    for command in [
        "doctor",
        "project",
        "cycle",
        "card",
        "work",
        "gate",
        "handoff",
        "review",
        "integration",
        "acceptance",
        "archive",
    ] {
        assert!(
            stdout.contains(&format!("  {command}")),
            "help omits `{command}`"
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
    // Any non-bare role is admissible. This crate is developed from linked
    // worktrees and its own integration gates run in a detached one, so
    // pinning the value would make the test a statement about where it happens
    // to run. Both narrower forms of this assertion have already failed that
    // way — see D-052 and D-054.
    let role = data["workspace_role"].as_str().unwrap();
    assert!(
        matches!(
            role,
            "main worktree" | "linked worktree" | "detached worktree"
        ),
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

#[test]
fn the_control_path_may_come_from_the_environment() {
    // Twenty-one commands require an absolute control path. Across eleven
    // self-hosted releases it was typed several hundred times, and each
    // repetition is a chance to point a command at the wrong project.
    let temp = tempfile::tempdir().expect("temp dir");
    let control = temp.path().join("control");

    let output = harness_command()
        .env("CHANGE_HARNESS_CONTROL", &control)
        .args(["project", "status", "--output", "json"])
        .output()
        .expect("the CLI should start");

    // The path is wrong on purpose: what matters is that it was *used*, which
    // a message naming it proves and a usage error would not.
    let envelope = stdout_json(&output);
    assert_eq!(envelope["schema"], "harness.command-error/v1");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&control.display().to_string()),
        "the environment path must be the one it tried: {envelope}"
    );
}

#[test]
fn an_explicit_control_flag_overrides_the_environment() {
    let temp = tempfile::tempdir().expect("temp dir");
    let from_env = temp.path().join("from-env");
    let from_flag = temp.path().join("from-flag");

    let output = harness_command()
        .env("CHANGE_HARNESS_CONTROL", &from_env)
        .args(["project", "status", "--output", "json", "--control"])
        .arg(&from_flag)
        .output()
        .expect("the CLI should start");

    let message = stdout_json(&output)["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        message.contains(&from_flag.display().to_string()),
        "the flag must win: {message}"
    );
    assert!(
        !message.contains(&from_env.display().to_string()),
        "the environment must not leak through: {message}"
    );
}

#[test]
fn neither_flag_nor_environment_is_a_usage_error() {
    let output = harness_command()
        .env_remove("CHANGE_HARNESS_CONTROL")
        .args(["project", "status"])
        .output()
        .expect("the CLI should start");

    assert_eq!(output.status.code(), Some(2), "usage category is exit 2");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--control"),
        "the missing argument must be named"
    );
}

#[test]
fn help_names_the_environment_variable_as_well_as_the_flag() {
    // The terse missing-argument error names only the flag, because clap does
    // not let a required argument's error be customised. `--help` is therefore
    // where someone learns the other way, so it must actually say so.
    let stdout = String::from_utf8(run(&["project", "status", "--help"]).stdout)
        .expect("help should be UTF-8");
    assert!(stdout.contains("--control"), "unexpected: {stdout}");
    assert!(
        stdout.contains("CHANGE_HARNESS_CONTROL"),
        "help must name the environment variable: {stdout}"
    );
}

#[test]
fn project_init_deliberately_ignores_the_environment() {
    // `init` decides where a control repository is *created*. Defaulting that
    // from a variable exported for another project is how someone initializes
    // into the wrong place, so this one flag stays required.
    let output = harness_command()
        .env("CHANGE_HARNESS_CONTROL", "/somewhere/else")
        .args(["project", "init", "--help"])
        .output()
        .expect("the CLI should start");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("CHANGE_HARNESS_CONTROL"),
        "init must not advertise an environment fallback: {stdout}"
    );
}
