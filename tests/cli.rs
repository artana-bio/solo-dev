//! End-to-end CLI behavior.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn harness_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
}

/// Runs `git` against `repo`, panicking with its stderr on failure.
///
/// `tests/support/mod.rs` has an identical helper, but pulling in that module
/// here — via `mod support;` — would drag in its full three-repository
/// `Workspace` fixture, which this file has deliberately never needed for
/// anything else it tests. The one test below that needs Git needs exactly
/// two bare `git init`s, so it gets this narrow copy instead.
fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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

// --- card 114: text mode prints the code and the recovery --------------
//
// All three tests below drive the same deterministic refusal as
// `doctor_rejects_a_missing_workspace` and `json_mode_renders_failures_as_the_error_envelope`
// above: `doctor --workspace <a path that does not exist>`. No control
// repository or other fixture is needed to trigger it, and it was already
// established in this file as the refusal these `--output json` assertions
// use, so reusing it keeps the three tests below narrowly about the layout
// card 114 adds rather than about standing up a new failure.

#[test]
fn a_text_mode_refusal_prints_its_error_code() {
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

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    // The specific code this refusal carries, not a bare `contains("CH-")`:
    // that weaker check would already pass on the pre-card rendering the
    // moment any other line happened to mention a code-shaped string, and
    // proves nothing about whether this refusal's own code is present.
    assert!(
        stderr.contains("CH-PRECONDITION-WORKSPACE-MISSING"),
        "text mode must print the stable error code: {stderr}"
    );
}

#[test]
fn a_text_mode_refusal_prints_its_recovery() {
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

    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(
        stderr.contains("Create the path or pass an existing one."),
        "text mode must print the recovery text: {stderr}"
    );
}

#[test]
fn the_json_envelope_is_unchanged() {
    // The regression guard for card 114: this card touches only text-mode
    // rendering, so `--output json` for this exact refusal must still
    // produce exactly today's fields and values. Beyond the specific values
    // (also covered non-exhaustively by `json_mode_renders_failures_as_the_error_envelope`
    // above), this asserts the exact key count at every level — root, error
    // body, and details — so a field added anywhere in the envelope changes
    // one of those counts even if its value is never individually checked.
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
    assert!(output.stderr.is_empty());
    let envelope = stdout_json(&output);

    assert_eq!(envelope["schema"], "harness.command-error/v1");
    assert_eq!(envelope["command"], "doctor");
    assert_eq!(envelope["status"], "error");
    assert_eq!(
        envelope["error"]["code"],
        "CH-PRECONDITION-WORKSPACE-MISSING"
    );
    assert_eq!(
        envelope["error"]["message"],
        format!("workspace does not exist: {}", missing.display())
    );
    assert_eq!(
        envelope["error"]["recovery"],
        "Create the path or pass an existing one."
    );
    assert_eq!(
        envelope["error"]["details"]["path"],
        missing.display().to_string()
    );

    assert_eq!(
        envelope.as_object().unwrap().len(),
        4,
        "envelope root must carry exactly schema, command, status, error: {envelope}"
    );
    assert_eq!(
        envelope["error"].as_object().unwrap().len(),
        4,
        "error body must carry exactly code, message, details, recovery: {envelope}"
    );
    assert_eq!(
        envelope["error"]["details"].as_object().unwrap().len(),
        1,
        "details must carry exactly path: {envelope}"
    );
}

#[test]
fn a_long_recovery_string_survives_text_mode_rendering_unbroken() {
    // The two text-mode tests above only exercise short recovery strings —
    // "Create the path or pass an existing one." (40 characters) and
    // "Supply an identifier matching its documented prefix and shape." (62
    // characters) — so neither would notice a rendering defect that only
    // truncates past roughly 80 characters. The recovery that actually
    // matters for that class of defect is
    // `ErrorCode::PolicyConvergenceEscalated`'s `convergence_recovery`: 332
    // characters, and the one an independent cold-start reviewer resolved a
    // refusal from unaided. Reaching it needs a card with a spent
    // convergence budget — a configured convergence policy, an active
    // cycle, an activated card with a real worktree, a delivered candidate,
    // and a review round recording a declared return, the fixture
    // `tests/convergence.rs::escalate_via_review_returns` builds against
    // `tests/support::Workspace`. This file has never needed that module or
    // a real project at all; every refusal it triggers elsewhere fails
    // before one would exist. Standing up a full project, cycle, card, and
    // review round here to test one rendering property would be
    // disproportionate, so this test does not do it, and the 332-character
    // case stays untested by this file.
    //
    // What it uses instead is the longest recovery string reachable at the
    // same fixture cost this file's other tests already pay:
    // `ErrorCode::ConfigAuthorityIncompatible`'s recovery, 99 characters.
    // `project init` refuses before creating anything once the authority
    // path turns out to have a working tree, so the only setup is two `git
    // init`s and no project, cycle, card, or `Workspace`. 99 characters is
    // comfortably past the ~80-character truncation this test exists to
    // catch, even though it is far short of 332.
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    let repository = root.join("repository");
    let authority = root.join("authority.git");
    let control = root.join("control");
    let worktrees = root.join("worktrees");

    fs::create_dir_all(&repository).unwrap();
    git(&repository, &["init", "-q", "-b", "main"]);
    git(&repository, &["config", "user.email", "f@local.invalid"]);
    git(&repository, &["config", "user.name", "Fixture"]);
    fs::write(repository.join("README.md"), "hello\n").unwrap();
    git(&repository, &["add", "-A"]);
    git(&repository, &["commit", "-q", "-m", "initial"]);

    // A working tree, not the bare repository `project init` requires, is
    // what triggers `ConfigAuthorityIncompatible`.
    fs::create_dir_all(&authority).unwrap();
    git(&authority, &["init", "-q", "-b", "main"]);

    let output = harness_command()
        .arg("project")
        .arg("init")
        .arg("--project-id")
        .arg("example")
        .arg("--repository")
        .arg(&repository)
        .arg("--control")
        .arg(&control)
        .arg("--authority")
        .arg(&authority)
        .arg("--worktree-root")
        .arg(&worktrees)
        .output()
        .expect("the CLI should start");

    assert_eq!(
        output.status.code(),
        Some(3),
        "configuration category is exit 3"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    let code = change_harness::error::ErrorCode::ConfigAuthorityIncompatible;
    let recovery = code.recovery();
    assert_eq!(
        recovery.len(),
        99,
        "this test's premise is that the string stays well past the ~80-character \
         truncation it exists to catch; if this fails, `ErrorCode::ConfigAuthorityIncompatible`'s \
         recovery text changed and the premise needs rechecking"
    );

    // The direct proof against truncation: `.contains` on the *complete*
    // string (not a prefix) cannot pass if any character — including the
    // last one — was dropped by rendering.
    assert!(
        stderr.contains(recovery),
        "the full recovery string must appear unbroken, not truncated: {stderr}"
    );

    // The stronger regression guard: the entire line-by-line rendering,
    // exact, the same rigor `text_cycle_id_error_redacts_github_token_exactly`
    // above applies to its own (much shorter) recovery string.
    assert_eq!(
        stderr,
        format!(
            "error: control state: authority {} has a working tree; promotion into a checked-out branch would desynchronize its index and files\ncode: {}\nrecovery: {recovery}\n",
            authority.display(),
            code.as_string(),
        )
    );
}

#[test]
fn invalid_cycle_id_json_redacts_github_token_from_message_and_details() {
    const TOKEN: &str = "ghp_0123456789abcdef0123456789abcdef0123";
    let missing = tempfile::tempdir()
        .expect("temporary directory should be created")
        .path()
        .join("missing");

    let output = harness_command()
        .args([
            "cycle",
            "create",
            "--cycle-id",
            TOKEN,
            "--objective",
            "ordinary",
            "--control",
        ])
        .arg(&missing)
        .args(["--output", "json"])
        .output()
        .expect("the CLI should start");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let rendered = String::from_utf8(output.stdout).expect("JSON output should be UTF-8");
    assert!(!rendered.contains(TOKEN), "raw token leaked: {rendered}");
    let envelope: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(envelope["schema"], "harness.command-error/v1");
    assert_eq!(envelope["command"], "cycle.create");
    assert_eq!(envelope["error"]["code"], "CH-USAGE-INVALID-ID");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("[redacted:github-token]")
    );
    assert_eq!(
        envelope["error"]["details"]["value"],
        "[redacted:github-token]"
    );
    assert_eq!(
        envelope["error"]["details"]["reason"],
        "expected prefix `C-`"
    );
    assert_eq!(
        envelope["error"]["recovery"],
        "Supply an identifier matching its documented prefix and shape."
    );
}

#[test]
fn text_cycle_id_error_redacts_github_token_exactly() {
    const TOKEN: &str = "ghp_0123456789abcdef0123456789abcdef0123";
    let missing = tempfile::tempdir()
        .expect("temporary directory should be created")
        .path()
        .join("missing");

    let output = harness_command()
        .args([
            "cycle",
            "create",
            "--cycle-id",
            TOKEN,
            "--objective",
            "ordinary",
            "--control",
        ])
        .arg(&missing)
        .output()
        .expect("the CLI should start");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Card 114 added the `code:` and `recovery:` lines below the message
    // line this test used to check alone; the exact match now covers all
    // three so a redaction gap in either new line would still fail this
    // test the same way a gap in the message line always did.
    assert_eq!(
        stderr,
        "error: invalid identifier `[redacted:github-token]`: expected prefix `C-`\n\
        code: CH-USAGE-INVALID-ID\n\
        recovery: Supply an identifier matching its documented prefix and shape.\n"
    );
    let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), stderr);
    assert!(!combined.contains(TOKEN));
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

#[test]
fn an_ambient_git_dir_does_not_redirect_the_harness() {
    // Tier 3, defect 16. Every Git call goes through one helper that sets three
    // environment variables and clears none. `GIT_DIR` and `GIT_WORK_TREE`
    // override `-C`, so a shell that exported either — a common thing to do
    // while working on a bare repository — silently pointed every harness
    // command at a different repository. The gate runner clears its environment
    // for exactly this class of reason; the Git layer never did.
    let elsewhere = tempfile::tempdir().expect("temp dir");
    let output = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(elsewhere.path())
        .output()
        .expect("git should run");
    assert!(output.status.success(), "the decoy repository must exist");

    let output = harness_command()
        .env("GIT_DIR", elsewhere.path().join(".git"))
        .env("GIT_WORK_TREE", elsewhere.path())
        .args([
            "doctor",
            "--workspace",
            env!("CARGO_MANIFEST_DIR"),
            "--output",
            "json",
        ])
        .output()
        .expect("the CLI should start");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        stdout_json(&output)["data"]["repository_root"],
        env!("CARGO_MANIFEST_DIR"),
        "the workspace flag must win over an exported GIT_DIR"
    );
}

#[test]
fn the_error_envelope_names_the_same_command_a_success_would() {
    // Tier 4. The success envelope carries the full dotted path — `card.status`
    // — and the error envelope carried only the group, `card`. A consumer
    // matching on `command` therefore got a different granularity depending on
    // whether the command worked, which makes the field unusable for the
    // routing it exists to support.
    let missing = tempfile::tempdir()
        .expect("temporary directory should be created")
        .path()
        .join("missing");

    let output = harness_command()
        .args(["card", "status", "--output", "json", "--card-id", "F-001"])
        .arg("--control")
        .arg(&missing)
        .output()
        .expect("the CLI should start");

    assert!(!output.status.success());
    let envelope = stdout_json(&output);
    assert_eq!(envelope["schema"], "harness.command-error/v1");
    assert_eq!(
        envelope["command"], "card.status",
        "the error envelope must name the subcommand, not just its group"
    );
}

#[test]
fn every_subcommand_group_reports_a_dotted_path_on_failure() {
    // The same for one failing invocation per group, so a single fixed match
    // arm does not look like a fixed contract.
    let missing = tempfile::tempdir()
        .expect("temporary directory should be created")
        .path()
        .join("missing");
    let control = missing.display().to_string();

    for (args, expected) in [
        (vec!["project", "status"], "project.status"),
        (
            vec!["cycle", "status", "--cycle-id", "C-001"],
            "cycle.status",
        ),
        (vec!["card", "status", "--card-id", "F-001"], "card.status"),
        (vec!["work", "status", "--card-id", "F-001"], "work.status"),
        (vec!["gate", "list"], "gate.list"),
        (
            vec!["handoff", "inspect", "--card-id", "F-001"],
            "handoff.inspect",
        ),
        (
            vec!["review", "inspect", "--card-id", "F-001"],
            "review.inspect",
        ),
        (
            vec!["integration", "inspect", "--integration-id", "INT-001"],
            "integration.inspect",
        ),
        (
            vec!["acceptance", "inspect", "--integration-id", "INT-001"],
            "acceptance.inspect",
        ),
        (vec!["audit", "cycle", "--cycle-id", "C-001"], "audit.cycle"),
    ] {
        let mut full: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        full.extend(["--output".to_owned(), "json".to_owned()]);
        full.extend(["--control".to_owned(), control.clone()]);
        let output = harness_command()
            .args(&full)
            .output()
            .expect("the CLI should start");
        assert!(
            !output.status.success(),
            "{expected}: the fixture must fail, or it proves nothing"
        );
        assert_eq!(
            stdout_json(&output)["command"],
            expected,
            "wrong command path for {full:?}"
        );
    }
}

#[test]
fn an_argument_error_still_honours_the_json_contract() {
    // Tier 4. `Cli::parse()` exits inside clap, before anything can render, so
    // a caller that asked for `--output json` got clap's usage text on stderr
    // and nothing at all on stdout. An agent driving this CLI — the interface
    // the JSON envelope exists for — cannot parse that, and the one failure it
    // is most likely to hit is a malformed invocation.
    let output = harness_command()
        .args(["project", "status", "--output", "json"])
        .env_remove("CHANGE_HARNESS_CONTROL")
        .output()
        .expect("the CLI should start");

    assert_eq!(output.status.code(), Some(2), "usage category is exit 2");
    let envelope = stdout_json(&output);
    assert_eq!(envelope["schema"], "harness.command-error/v1");
    assert_eq!(envelope["status"], "error");
    assert_eq!(
        envelope["command"], "project.status",
        "the envelope must name what was attempted"
    );
    assert_eq!(envelope["error"]["code"], "CH-USAGE-INVALID-ARGUMENTS");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--control"),
        "and must carry clap's own diagnostic: {envelope}"
    );
}

#[test]
fn an_argument_error_without_json_keeps_claps_own_output() {
    // The guard. Clap's usage text is far better for a human than a one-line
    // envelope, and text mode is the default, so this must not become JSON for
    // everyone.
    let output = harness_command()
        .args(["project", "status"])
        .env_remove("CHANGE_HARNESS_CONTROL")
        .output()
        .expect("the CLI should start");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "text mode writes nothing to stdout"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--control"), "{stderr}");
    assert!(
        stderr.contains("Usage:"),
        "clap's help must survive: {stderr}"
    );
}

// --- demo ---------------------------------------------------------------
//
// `demo` needs no control repository and no `--control` flag: it is a
// self-contained animation, not a lifecycle command. `Command::output()`
// captures the child's standard error into a pipe rather than a TTY, so
// every test here exercises the skip path by construction — the same path a
// script or an agent piping this command's output would take. That is also
// what makes these tests fast rather than an ~18-second real playback: the
// frame-by-frame animation logic itself is unit-tested in
// `src/cli/floor.rs` and `src/commands/demo.rs` against an injectable sink
// and a zero delay, without a real terminal or a real wait. What only a
// real subprocess can prove is what these tests check: that stdout carries
// no escape bytes, that stderr is left completely clean rather than merely
// "no visible animation," and that `--no-animation` is actually wired
// through clap.

#[test]
fn help_advertises_demo() {
    let stdout = String::from_utf8(run(&["--help"]).stdout).expect("help should be UTF-8");
    assert!(stdout.contains("  demo"), "help omits `demo`");
}

#[test]
fn demo_needs_no_control_repository() {
    let output = harness_command()
        .args(["demo"])
        .env_remove("CHANGE_HARNESS_CONTROL")
        .output()
        .expect("the CLI should start");

    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn demo_over_a_pipe_skips_the_animation_and_leaves_both_streams_clean() {
    let output = run(&["demo"]);

    assert_eq!(output.status.code(), Some(0));

    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("animation skipped"), "{stdout}");
    assert!(stdout.contains("not a terminal"), "{stdout}");
    assert!(
        stdout.contains("No repository was read or changed"),
        "{stdout}"
    );
    assert!(
        !stdout.contains('\u{1b}'),
        "stdout must never carry raw escape bytes: {stdout:?}"
    );

    // The regression this pins: the animation's `TerminalSink` used to be
    // constructed unconditionally, so even a skipped run wrote the
    // hide-cursor escape to standard error (and immediately the show-cursor
    // escape back on drop) before anything checked whether playback should
    // happen at all. A caller piping standard error somewhere — a log file,
    // a terminal multiplexer pane that is not the active one — would see
    // stray bytes from a command that is documented to do nothing when
    // skipped.
    assert!(
        output.stderr.is_empty(),
        "a skipped run must write nothing at all to standard error: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn demo_output_json_returns_the_stable_envelope_and_touches_no_streams_it_should_not() {
    let output = run(&["demo", "--output", "json"]);

    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stderr.is_empty(),
        "JSON mode must never write to standard error: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );

    let envelope = stdout_json(&output);
    assert_eq!(envelope["schema"], "harness.command-result/v1");
    assert_eq!(envelope["status"], "success");
    assert_eq!(envelope["command"], "demo");
    assert!(envelope["project_id"].is_null());
    assert!(envelope["operation_id"].is_null());
    assert_eq!(envelope["warnings"].as_array().unwrap().len(), 0);

    let data = &envelope["data"];
    assert_eq!(data["schema"], "harness.demo/v1");
    assert_eq!(data["played"], false);
    assert_eq!(data["skip_reason"], "json_output");

    let stations = data["stations"]
        .as_array()
        .expect("stations should be an array");
    assert_eq!(stations.len(), 6);
    assert_eq!(stations[0]["station"], "INTAKE");
    assert_eq!(stations[0]["command"], "work start");
    assert_eq!(stations[5]["station"], "SHIP");
    assert_eq!(stations[5]["command"], "promote");
}

#[test]
fn demo_no_animation_flag_is_wired_through_clap() {
    // Unit tests already cover `skip_reason`'s precedence in isolation; this
    // instead proves the flag clap parses out of `--no-animation` actually
    // reaches it, which only a real invocation can show. Text mode, not
    // JSON: `--output json` is itself a skip cause, so running both together
    // would leave this proving nothing about the flag specifically.
    let output = run(&["demo", "--no-animation"]);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("--no-animation was passed"), "{stdout}");
}
