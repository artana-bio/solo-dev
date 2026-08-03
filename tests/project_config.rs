//! `WP-110` acceptance: project configuration against real repositories.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use change_harness::{
    config::{PROJECT_SCHEMA, ProjectConfig, validate::validate},
    error::ErrorCode,
};
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A workspace with a candidate repository, a control directory, and a bare
/// authority, all siblings.
struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    repository: PathBuf,
    control: PathBuf,
    authority: PathBuf,
    worktrees: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().to_path_buf();
        let repository = root.join("repository");
        let control = root.join("control");
        let authority = root.join("authority.git");
        let worktrees = root.join("worktrees");

        fs::create_dir_all(&repository).unwrap();
        fs::create_dir_all(&control).unwrap();
        git(&repository, &["init", "-q", "-b", "main"]);
        git(
            &repository,
            &["config", "user.email", "fixture@local.invalid"],
        );
        git(&repository, &["config", "user.name", "Fixture"]);
        fs::write(repository.join("README.md"), "hello\n").unwrap();
        git(&repository, &["add", "-A"]);
        git(&repository, &["commit", "-q", "-m", "initial"]);

        Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&authority)
            .output()
            .expect("git init --bare");

        Self {
            _temp: temp,
            root,
            repository,
            control,
            authority,
            worktrees,
        }
    }

    fn config(&self) -> ProjectConfig {
        ProjectConfig::from_json(&self.document()).expect("fixture document should parse")
    }

    fn document(&self) -> String {
        format!(
            r#"{{
  "schema": "{PROJECT_SCHEMA}",
  "project_id": "example",
  "repository": {repository},
  "control_repository": {control},
  "authority_repository": {authority},
  "authority_remote": "harness-authority",
  "protected_branch": "main",
  "worktree_root": {worktrees},
  "default_output": "text",
  "host_policy": {{
    "supported_os": ["{os}"],
    "minimum_git_version": "2.50.0"
  }}
}}"#,
            repository = json_string(&self.repository),
            control = json_string(&self.control),
            authority = json_string(&self.authority),
            worktrees = json_string(&self.worktrees),
            os = std::env::consts::OS,
        )
    }
}

fn json_string(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).expect("path should encode")
}

#[test]
fn a_valid_configuration_passes_and_records_every_path() {
    let fixture = Fixture::new();
    let report = validate(&fixture.config()).expect("configuration should be valid");

    assert_eq!(report.project_id, "example");
    assert_eq!(report.protected_branch, "main");
    assert_eq!(report.protected_branch_sha.len(), 40);
    assert_eq!(report.paths.len(), 4);
    assert!(
        report.paths.iter().any(|entry| entry.field == "repository"),
        "every configured path is recorded"
    );
}

#[test]
fn old_project_document_without_final_authorization_policy_remains_readable() {
    let fixture = Fixture::new();
    let config =
        ProjectConfig::from_json(&fixture.document()).expect("old document stays readable");
    assert!(config.final_authorization_policy.is_none());
}

#[test]
fn final_authorization_policy_rejects_unusable_and_normalized_duplicate_actor_ids() {
    let fixture = Fixture::new();
    for actors in [
        serde_json::json!(["owner", "OWNER"]),
        serde_json::json!(["owner", " owner "]),
        serde_json::json!(["owner", "\u{00c1}lvaro"]),
        serde_json::json!(["owner", "bad\nactor"]),
    ] {
        let mut document: serde_json::Value = serde_json::from_str(&fixture.document()).unwrap();
        document["final_authorization_policy"] = serde_json::json!({
            "version": "harness.final-authorization-policy/v1",
            "authorization_unit": "sealed_cycle",
            "authorizer_actor_ids": actors,
        });
        let error = ProjectConfig::from_json(&serde_json::to_string(&document).unwrap())
            .expect_err("unusable or duplicate actor ids must refuse");
        assert_eq!(error.code(), ErrorCode::ConfigInvalidValue);
    }
}

#[test]
fn a_relative_path_fails_and_names_its_field() {
    let fixture = Fixture::new();
    let document = fixture
        .document()
        .replace(&json_string(&fixture.control), "\"relative/control\"");
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigPathNotAbsolute);
    assert_eq!(error.details()["field"], "control_repository");
}

#[test]
fn a_missing_path_fails_without_creating_it() {
    let fixture = Fixture::new();
    let absent = fixture.root.join("does-not-exist");
    let document = fixture
        .document()
        .replace(&json_string(&fixture.control), &json_string(&absent));
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigPathMissing);
    assert_eq!(error.details()["field"], "control_repository");
    assert!(!absent.exists(), "validation must not create anything");
}

#[test]
fn two_roles_at_the_same_location_fail() {
    let fixture = Fixture::new();
    let document = fixture.document().replace(
        &json_string(&fixture.control),
        &json_string(&fixture.authority),
    );
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigPathAlias);
}

#[test]
fn roles_aliased_through_a_symlink_are_detected() {
    // A string comparison would miss this: the configured paths differ, but
    // both resolve to the same directory.
    let fixture = Fixture::new();
    let link = fixture.root.join("control-link");
    std::os::unix::fs::symlink(&fixture.control, &link).unwrap();
    let document = fixture
        .document()
        .replace(&json_string(&fixture.authority), &json_string(&link));
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigPathAlias);
}

#[test]
fn symlinked_paths_are_recorded_as_resolved() {
    let fixture = Fixture::new();
    let link = fixture.root.join("control-link");
    std::os::unix::fs::symlink(&fixture.control, &link).unwrap();
    let document = fixture
        .document()
        .replace(&json_string(&fixture.control), &json_string(&link));
    let config = ProjectConfig::from_json(&document).unwrap();

    let report = validate(&config).expect("a symlinked path is legitimate");
    let control = report
        .paths
        .iter()
        .find(|entry| entry.field == "control_repository")
        .unwrap();
    assert!(control.via_symlink, "symlink traversal must be recorded");
    assert_ne!(control.configured, control.resolved);
}

#[test]
fn control_nested_inside_the_candidate_fails() {
    let fixture = Fixture::new();
    let nested = fixture.repository.join("control");
    fs::create_dir_all(&nested).unwrap();
    let document = fixture
        .document()
        .replace(&json_string(&fixture.control), &json_string(&nested));
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigPathNested);
    assert_eq!(error.details()["field"], "control_repository");
}

#[test]
fn a_candidate_that_is_not_a_repository_fails() {
    let fixture = Fixture::new();
    let plain = fixture.root.join("plain");
    fs::create_dir_all(&plain).unwrap();
    let document = fixture
        .document()
        .replace(&json_string(&fixture.repository), &json_string(&plain));
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigNotRepository);
    assert_eq!(error.details()["field"], "repository");
}

#[test]
fn a_missing_protected_branch_fails() {
    let fixture = Fixture::new();
    let document = fixture.document().replace(
        r#""protected_branch": "main""#,
        r#""protected_branch": "release""#,
    );
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigProtectedBranch);
    assert_eq!(error.details()["field"], "protected_branch");
}

#[test]
fn an_unsatisfiable_minimum_git_version_fails_explicitly() {
    let fixture = Fixture::new();
    let document = fixture.document().replace(
        r#""minimum_git_version": "2.50.0""#,
        r#""minimum_git_version": "99.0.0""#,
    );
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigGitVersion);
    assert!(error.to_string().contains("99.0.0"));
}

#[test]
fn a_malformed_minimum_git_version_is_a_value_error_not_a_version_error() {
    let fixture = Fixture::new();
    let document = fixture.document().replace(
        r#""minimum_git_version": "2.50.0""#,
        r#""minimum_git_version": "latest""#,
    );
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigInvalidValue);
    assert_eq!(error.details()["field"], "host_policy.minimum_git_version");
}

#[test]
fn an_unsupported_host_fails() {
    let fixture = Fixture::new();
    let document = fixture.document().replace(
        &format!(r#""supported_os": ["{}"]"#, std::env::consts::OS),
        r#""supported_os": ["plan9"]"#,
    );
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigUnsupportedHost);
}

#[test]
fn an_invalid_default_output_fails() {
    let fixture = Fixture::new();
    let document = fixture
        .document()
        .replace(r#""default_output": "text""#, r#""default_output": "yaml""#);
    let config = ProjectConfig::from_json(&document).unwrap();

    let error = validate(&config).expect_err("must reject");
    assert_eq!(error.code(), ErrorCode::ConfigInvalidValue);
    assert_eq!(error.details()["field"], "default_output");
}

#[test]
fn an_absent_worktree_root_is_accepted_because_it_is_created_on_demand() {
    let fixture = Fixture::new();
    assert!(!fixture.worktrees.exists());
    let report = validate(&fixture.config()).expect("an absent worktree root is legitimate");
    let entry = report
        .paths
        .iter()
        .find(|entry| entry.field == "worktree_root")
        .unwrap();
    assert!(!entry.via_symlink);
}

#[test]
fn validation_mutates_nothing() {
    let fixture = Fixture::new();
    let before: Vec<PathBuf> = fs::read_dir(&fixture.root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();

    validate(&fixture.config()).unwrap();

    let after: Vec<PathBuf> = fs::read_dir(&fixture.root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        before.len(),
        after.len(),
        "validation created or removed a path"
    );
}

#[test]
fn cli_validate_reports_the_exact_invalid_field_in_json() {
    let fixture = Fixture::new();
    let document = fixture.document().replace(
        r#""protected_branch": "main""#,
        r#""protected_branch": "release""#,
    );
    let path = fixture.root.join("project.json");
    fs::write(&path, document).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(["project", "validate", "--output", "json", "--config"])
        .arg(&path)
        .output()
        .expect("the CLI should start");

    assert_eq!(
        output.status.code(),
        Some(3),
        "configuration category is exit 3"
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["schema"], "harness.command-error/v1");
    assert_eq!(
        envelope["command"], "project.validate",
        "the error envelope names the subcommand; this asserted the group, \
         which was the defect"
    );
    assert_eq!(envelope["error"]["code"], "CH-CONFIG-PROTECTED-BRANCH");
    assert_eq!(envelope["error"]["details"]["field"], "protected_branch");
}

#[test]
fn cli_validate_succeeds_on_a_valid_project() {
    let fixture = Fixture::new();
    let path = fixture.root.join("project.json");
    fs::write(&path, fixture.document()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(["project", "validate", "--output", "json", "--config"])
        .arg(&path)
        .output()
        .expect("the CLI should start");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["status"], "success");
    assert_eq!(envelope["project_id"], "example");
    assert_eq!(envelope["data"]["schema"], "harness.project-validation/v1");
}

#[test]
fn cli_validate_reports_a_text_diagnostic_on_stderr() {
    let fixture = Fixture::new();
    let path = fixture.root.join("project.json");
    fs::write(&path, "{ not json").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(["project", "validate", "--config"])
        .arg(&path)
        .output()
        .expect("the CLI should start");

    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("configuration field"), "{stderr}");
}

#[test]
fn scenario_4_a_dirty_candidate_is_refused() {
    let fixture = Fixture::new();
    fs::write(fixture.repository.join("scratch.txt"), "uncommitted\n").unwrap();

    let error = validate(&fixture.config()).expect_err("a dirty candidate is refused");
    assert_eq!(error.code(), ErrorCode::ConfigCandidateUnsettled);
    assert!(
        error.to_string().contains("scratch.txt"),
        "the offending path must be named: {error}"
    );
}

#[test]
fn scenario_5_an_unresolved_git_operation_is_refused() {
    let fixture = Fixture::new();
    // A real conflicted merge, not a fabricated marker file: the check exists
    // for repositories someone left mid-operation, and that is how they look.
    git(&fixture.repository, &["checkout", "-q", "-b", "other"]);
    fs::write(fixture.repository.join("README.md"), "theirs\n").unwrap();
    git(&fixture.repository, &["commit", "-qam", "theirs"]);
    git(&fixture.repository, &["checkout", "-q", "main"]);
    fs::write(fixture.repository.join("README.md"), "ours\n").unwrap();
    git(&fixture.repository, &["commit", "-qam", "ours"]);
    let merge = std::process::Command::new("git")
        .arg("-C")
        .arg(&fixture.repository)
        .args(["merge", "other"])
        .output()
        .expect("git should run");
    assert!(
        !merge.status.success(),
        "the fixture must actually conflict"
    );

    let error = validate(&fixture.config()).expect_err("an unresolved merge is refused");
    assert_eq!(error.code(), ErrorCode::ConfigCandidateUnsettled);
    assert!(
        error.to_string().contains("Merge"),
        "the operation must be named: {error}"
    );
}
