//! `WP-120` acceptance: control repository, locking, and the journal.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use change_harness::{
    cli::exit::ExitCategory,
    control::{
        journal::{Journal, OperationState},
        lock::{LOCK_FILE, ProjectLock},
        repository::{ControlRepository, PROJECT_FILE},
    },
    domain::clock::FixedClock,
};
use tempfile::TempDir;

fn clock() -> FixedClock {
    FixedClock::at_unix_seconds(1_785_196_800).unwrap()
}

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

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    repository: PathBuf,
    control: PathBuf,
    authority: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().to_path_buf();
        let repository = root.join("repository");
        let control = root.join("control");
        let authority = root.join("authority.git");

        fs::create_dir_all(&repository).unwrap();
        git(&repository, &["init", "-q", "-b", "main"]);
        git(&repository, &["config", "user.email", "f@local.invalid"]);
        git(&repository, &["config", "user.name", "F"]);
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
        }
    }

    fn init_args(&self) -> Vec<String> {
        vec![
            "project".into(),
            "init".into(),
            "--project-id".into(),
            "example".into(),
            "--repository".into(),
            self.repository.display().to_string(),
            "--control".into(),
            self.control.display().to_string(),
            "--authority".into(),
            self.authority.display().to_string(),
            "--worktree-root".into(),
            self.root.join("worktrees").display().to_string(),
        ]
    }

    fn run(args: &[String]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_change-harness"))
            .args(args)
            .output()
            .expect("the CLI should start")
    }

    fn init(&self) -> std::process::Output {
        Self::run(&self.init_args())
    }
}

#[test]
fn init_creates_a_committed_control_repository() {
    let fixture = Fixture::new();
    let output = fixture.init();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let control = ControlRepository::open(&fixture.control).unwrap();
    assert!(control.is_initialized());
    assert_eq!(control.commit_count().unwrap(), 1);
    assert!(
        control.is_clean().unwrap(),
        "control must be committed clean"
    );
    assert_eq!(control.project().unwrap().project_id.as_str(), "example");
}

#[test]
fn control_commits_use_the_fixed_harness_identity() {
    let fixture = Fixture::new();
    fixture.init();

    let output = Command::new("git")
        .arg("-C")
        .arg(&fixture.control)
        .args(["log", "-1", "--format=%an <%ae>"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "Change Harness <change-harness@local.invalid>",
        "actor identity belongs in the event, not in Git authorship"
    );
}

#[test]
fn init_is_idempotent_for_identical_configuration() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let head_after_first = control.head().unwrap();

    let second = fixture.init();
    assert!(second.status.success(), "a repeat must succeed");
    assert_eq!(
        control.head().unwrap(),
        head_after_first,
        "a repeat must not produce a second commit"
    );
    assert_eq!(control.commit_count().unwrap(), 1);
}

#[test]
fn incompatible_reinitialization_fails_without_altering_anything() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before_head = control.head().unwrap();
    let before_document = control.read(PROJECT_FILE).unwrap();

    let mut args = fixture.init_args();
    let branch_index = args.len();
    args.push("--protected-branch".into());
    args.push("main".into());
    // Rebind the project to a different identifier, which must be refused.
    let id_position = args.iter().position(|a| a == "example").unwrap();
    args[id_position] = "different".into();
    let _ = branch_index;

    let output = Fixture::run(&args);
    assert_eq!(
        output.status.code(),
        Some(3),
        "rebinding is a configuration failure"
    );

    assert_eq!(control.head().unwrap(), before_head);
    assert_eq!(control.read(PROJECT_FILE).unwrap(), before_document);
}

#[test]
fn a_dry_run_reports_planned_mutations_and_creates_nothing() {
    let fixture = Fixture::new();
    let mut args = fixture.init_args();
    args.push("--dry-run".into());
    args.push("--output".into());
    args.push("json".into());

    let output = Fixture::run(&args);
    assert!(output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["data"]["dry_run"], true);
    assert!(
        envelope["data"]["planned_mutations"]
            .as_array()
            .unwrap()
            .len()
            >= 3
    );
    assert!(
        !fixture.control.exists(),
        "a dry run must not create the control repository"
    );
}

#[test]
fn an_invalid_configuration_fails_before_creating_the_control_repository() {
    let fixture = Fixture::new();
    let mut args = fixture.init_args();
    let branch = args.iter().position(|a| a == "--control").unwrap();
    // Point the candidate at a non-repository.
    let repository_index = args.iter().position(|a| a == "--repository").unwrap() + 1;
    args[repository_index] = fixture.root.join("not-a-repo").display().to_string();
    fs::create_dir_all(fixture.root.join("not-a-repo")).unwrap();
    let _ = branch;

    let output = Fixture::run(&args);
    assert_eq!(output.status.code(), Some(3));
    assert!(
        !fixture.control.exists(),
        "validation must run before any mutation"
    );
}

#[test]
fn a_held_lock_makes_a_second_mutation_fail_as_policy() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());

    // Hold the lock as another process would.
    let _held = ProjectLock::acquire(&fixture.control, "other-command", &clock()).unwrap();

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "must contend for the lock".into(),
    ]);
    // Unlike idempotent project initialization, this command would write a
    // cycle record. Its only admissible outcome while the lock is held is the
    // policy refusal from acquisition.
    assert!(fixture.control.join(LOCK_FILE).exists());
    assert!(
        !output.status.success(),
        "a held lock must refuse the mutation"
    );
    assert_eq!(
        output.status.code(),
        Some(5),
        "contention is a policy refusal"
    );
}

#[test]
fn a_second_acquisition_loses_while_the_first_is_held() {
    // Sequential, and named for what it is. Real contention is exercised by
    // `tests/concurrency.rs`, which this used to claim to do and did not: two
    // calls on one thread never contend for anything.
    let temp = tempfile::tempdir().unwrap();
    let control = temp.path().to_path_buf();

    let first = ProjectLock::acquire(&control, "first", &clock());
    let second = ProjectLock::acquire(&control, "second", &clock());

    assert!(first.is_ok(), "one writer must win");
    let error = second.expect_err("the other must lose");
    assert_eq!(
        error.category(),
        change_harness::cli::exit::ExitCategory::Policy
    );
}

#[test]
fn status_reports_control_state_for_an_initialized_project() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());

    let output = Fixture::run(&[
        "project".into(),
        "status".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
    ]);
    assert!(output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["data"]["project_id"], "example");
    assert_eq!(envelope["data"]["control_commits"], 1);
    assert_eq!(envelope["data"]["lock_held"], false);
    assert_eq!(
        envelope["data"]["unresolved_operations"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn status_on_an_uninitialized_control_repository_fails() {
    let fixture = Fixture::new();
    let output = Fixture::run(&[
        "project".into(),
        "status".into(),
        "--control".into(),
        fixture.control.display().to_string(),
    ]);
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn recover_reports_nothing_to_do_on_a_settled_project() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());

    let output = Fixture::run(&[
        "project".into(),
        "recover".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
    ]);
    assert!(output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["data"]["recovery_required"], false);
}

#[test]
fn an_interruption_at_any_journal_boundary_is_diagnosable() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let journal = Journal::new(&control);

    // Simulate a process killed after each recorded boundary. The journal must
    // name the last boundary reached in every case, which is what makes the
    // interruption attributable rather than guessed at.
    for boundary in [
        vec![],
        vec!["control-git-initialized"],
        vec!["control-git-initialized", "project-document-written"],
    ] {
        let mut record = journal.begin("project.init", None, &clock()).unwrap();
        for step in &boundary {
            journal.step(&mut record, step).unwrap();
        }

        let unresolved = journal.unresolved().unwrap();
        let found = unresolved
            .iter()
            .find(|entry| entry.operation_id == record.operation_id)
            .expect("the interrupted operation must be visible");
        assert_eq!(found.state, OperationState::Started);
        assert_eq!(found.steps, boundary);

        let output = Fixture::run(&[
            "project".into(),
            "recover".into(),
            "--output".into(),
            "json".into(),
            "--control".into(),
            fixture.control.display().to_string(),
        ]);
        assert!(output.status.success());
        let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(envelope["data"]["recovery_required"], true);

        // Settle it so the next iteration starts clean.
        journal
            .finish(&mut record, OperationState::Completed, None, &clock())
            .unwrap();
    }
}

#[test]
fn an_unresolved_operation_blocks_further_mutation() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let journal = Journal::new(&control);
    let mut stranded = journal.begin("project.init", None, &clock()).unwrap();
    journal
        .finish(
            &mut stranded,
            OperationState::FailedPartial,
            Some("simulated interruption".into()),
            &clock(),
        )
        .unwrap();

    let error = journal.require_settled().expect_err("must block");
    assert_eq!(
        error.category(),
        change_harness::cli::exit::ExitCategory::RecoveryRequired
    );

    let output = Fixture::run(&[
        "project".into(),
        "status".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
    ]);
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["data"]["unresolved_operations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn control_history_contains_no_partial_authoritative_record() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();

    // Every committed tree must carry a parseable project document; a commit
    // holding a half-written one would make history unreadable.
    let revisions = Command::new("git")
        .arg("-C")
        .arg(&fixture.control)
        .args(["rev-list", "HEAD"])
        .output()
        .unwrap();
    for revision in String::from_utf8_lossy(&revisions.stdout).lines() {
        let show = Command::new("git")
            .arg("-C")
            .arg(&fixture.control)
            .args(["show", &format!("{revision}:{PROJECT_FILE}")])
            .output()
            .unwrap();
        assert!(
            show.status.success(),
            "commit {revision} lacks the project document"
        );
        let parsed: serde_json::Value =
            serde_json::from_slice(&show.stdout).expect("committed document must parse");
        assert_eq!(parsed["schema"], "harness.project/v1");
    }
    let _ = control;
}

#[test]
fn the_transient_lock_never_enters_control_history() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());

    let tracked = Command::new("git")
        .arg("-C")
        .arg(&fixture.control)
        .args(["ls-files"])
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&tracked.stdout);
    assert!(!listing.contains(LOCK_FILE), "{listing}");
    assert!(listing.contains(PROJECT_FILE), "{listing}");
}

#[test]
fn status_reports_a_worktree_belonging_to_a_different_project() {
    // The hazard `CHANGE_HARNESS_CONTROL` introduces: a variable exported for
    // one project drives a command meant for another, and the command succeeds
    // correctly against the wrong records. The locator is the only thing that
    // knows, so `project status` says so.
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());

    // A locator naming some other project, in the directory the command runs
    // from.
    let elsewhere = fixture.root.join("another-control");
    fs::create_dir_all(&elsewhere).unwrap();
    let stage = fixture.root.join("stage");
    fs::create_dir_all(stage.join(".agent")).unwrap();
    fs::write(
        stage.join(".agent/project.json"),
        serde_json::json!({
            "schema": "harness.worktree-link/v1",
            "project_id": "example",
            "card_id": "F-042",
            "card_revision": 1,
            "control_repository": elsewhere,
            "lease_id": "L-000001",
        })
        .to_string(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .current_dir(&stage)
        .args([
            "project",
            "status",
            "--output",
            "json",
            "--control",
            &fixture.control.display().to_string(),
        ])
        .output()
        .expect("the CLI should start");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    assert!(
        output.status.success(),
        "the locator is advisory, so this reports rather than refuses"
    );
    let warning = envelope["warnings"][0]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(warning.contains("F-042"), "unexpected: {envelope}");
    assert!(
        envelope["data"]["locator_disagreement"].is_string(),
        "the disagreement must be in the payload too: {envelope}"
    );
}

#[test]
fn status_says_nothing_about_the_locator_when_there_is_none() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());

    let empty = fixture.root.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .current_dir(&empty)
        .args([
            "project",
            "status",
            "--output",
            "json",
            "--control",
            &fixture.control.display().to_string(),
        ])
        .output()
        .expect("the CLI should start");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(envelope["warnings"].as_array().unwrap().len(), 0);
    assert!(envelope["data"]["locator_disagreement"].is_null());
    // The rest of the report is unchanged.
    assert_eq!(envelope["data"]["project_id"], "example");
    assert!(envelope["data"]["lock"].is_object());
}

#[test]
fn status_says_nothing_when_the_locator_agrees() {
    // The half the "no locator" test does not reach: a locator that names the
    // control repository actually in use must produce no warning. Without
    // this, reporting a disagreement unconditionally would pass every test.
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());

    let stage = fixture.root.join("agreeing");
    fs::create_dir_all(stage.join(".agent")).unwrap();
    fs::write(
        stage.join(".agent/project.json"),
        serde_json::json!({
            "schema": "harness.worktree-link/v1",
            "project_id": "example",
            "card_id": "F-001",
            "card_revision": 1,
            "control_repository": &fixture.control,
            "lease_id": "L-000001",
        })
        .to_string(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .current_dir(&stage)
        .args([
            "project",
            "status",
            "--output",
            "json",
            "--control",
            &fixture.control.display().to_string(),
        ])
        .output()
        .expect("the CLI should start");
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(
        envelope["warnings"].as_array().unwrap().len(),
        0,
        "an agreeing locator must be silent: {envelope}"
    );
    assert!(envelope["data"]["locator_disagreement"].is_null());
}

#[test]
fn an_environment_io_failure_is_not_reported_as_a_harness_defect() {
    // Tier 4. Every `ControlIo` failure was classified `InternalControlCorrupt`
    // — exit 10, the category reserved for "the harness is broken, file a bug".
    // A read-only filesystem, a permissions mistake, a full disk: all
    // environment problems the operator can fix, all reported as harness
    // defects, which sends them to exactly the wrong place.
    use std::os::unix::fs::PermissionsExt as _;

    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().join("control");
    fs::create_dir_all(&root).unwrap();

    // The fixture must genuinely be unwritable, or the classification under
    // test is never reached.
    let mut permissions = fs::metadata(&root).unwrap().permissions();
    permissions.set_mode(0o500);
    fs::set_permissions(&root, permissions).unwrap();
    assert!(
        fs::write(root.join("probe"), "x").is_err(),
        "the fixture must be unwritable"
    );

    let control = ControlRepository::at(&root);
    let error = control
        .write_atomic("cards/F-001.json", "{}\n")
        .expect_err("writing into an unwritable directory must fail");

    // Restore before asserting, so a failure does not leave an undeletable
    // temporary directory behind.
    let mut permissions = fs::metadata(&root).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&root, permissions).unwrap();

    assert_eq!(
        error.category(),
        ExitCategory::Precondition,
        "a permissions problem is the operator's to fix, not a harness defect"
    );
    assert_eq!(error.code().as_string(), "CH-PRECONDITION-CONTROL-ACCESS");
}
