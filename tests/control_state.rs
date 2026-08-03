//! `WP-120` acceptance: control repository, locking, and the journal.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use change_harness::{
    cli::exit::ExitCategory,
    control::{
        journal::{JOURNAL_DIR, Journal, OperationState},
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
fn occupied_control_init_refuses_before_journal_and_allows_clean_retry() {
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.control).unwrap();
    let foreign = fixture.control.join("external-sensitive.txt");
    fs::write(&foreign, "external sensitive file\n").unwrap();

    let mut args = fixture.init_args();
    args.extend(["--output".into(), "json".into()]);
    let refused = Fixture::run(&args);
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );

    assert_eq!(refused.status.code(), Some(4), "{diagnostic}");
    assert!(diagnostic.contains("CH-PRECONDITION-OCCUPIED-PATH"));
    assert!(foreign.exists());
    let git_exists = fixture.control.join(".git").exists();
    let lock_exists = fixture.control.join(LOCK_FILE).exists();
    let journal_path = fixture.control.join(JOURNAL_DIR);
    let journal_exists = journal_path.exists();
    let journal_has_failed_partial = fs::read_dir(&journal_path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .any(|record| record.contains("failed_partial"));
    let project_exists = fixture.control.join(PROJECT_FILE).exists();
    assert!(
        !git_exists && !lock_exists && !journal_exists && !project_exists,
        "unexpected init residue: git={git_exists} lock={lock_exists} journal={journal_exists} failed_partial={journal_has_failed_partial} project={project_exists}"
    );

    fs::remove_file(&foreign).unwrap();
    let retry = Fixture::run(&args);
    assert!(
        retry.status.success(),
        "identical retry failed: {}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(
        ControlRepository::open(&fixture.control)
            .unwrap()
            .is_initialized()
    );
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
fn a_utf16_control_blob_is_refused_before_it_reaches_git_history() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before = control.head().unwrap();
    let token = "ghp_utf16-token-must-not-reach-history";

    // UTF-16LE is the concrete bypass: Git stores these bytes, while the old
    // GitOutput projection decoded them lossily and the scanner never saw the
    // token as contiguous text.
    let contents = format!(
        r#"{{"note":"{token}"}}
"#
    );
    let utf16le: Vec<u8> = contents.encode_utf16().flat_map(u16::to_le_bytes).collect();
    fs::create_dir_all(fixture.control.join("cards")).unwrap();
    fs::write(fixture.control.join("cards/utf16.json"), utf16le).unwrap();
    let would_be_blob = Command::new("git")
        .arg("-C")
        .arg(&fixture.control)
        .args(["hash-object", "cards/utf16.json"])
        .output()
        .unwrap();
    assert!(would_be_blob.status.success());
    let would_be_blob = String::from_utf8_lossy(&would_be_blob.stdout)
        .trim()
        .to_owned();
    assert!(
        !Command::new("git")
            .arg("-C")
            .arg(&fixture.control)
            .args(["cat-file", "-e", &would_be_blob])
            .status()
            .unwrap()
            .success()
    );

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ]);
    assert_eq!(output.status.code(), Some(5), "encoding refusal expected");
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        rendered.contains("CH-POLICY-CONTROL-ENCODING"),
        "the refusal must identify the policy: {rendered}"
    );
    assert_eq!(control.head().unwrap(), before, "HEAD must not advance");
    assert!(
        !Command::new("git")
            .arg("-C")
            .arg(&fixture.control)
            .args(["cat-file", "-e", &would_be_blob])
            .status()
            .unwrap()
            .success(),
        "the rejected blob must not enter Git's object database"
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&fixture.control)
            .args(["diff", "--cached", "--quiet"])
            .status()
            .unwrap()
            .success(),
        "a refusal must leave no staged Git object behind"
    );
    let history = Command::new("git")
        .arg("-C")
        .arg(&fixture.control)
        .args(["log", "--all", "-p"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&history.stdout).contains(token),
        "the credential must not be durable: {}",
        String::from_utf8_lossy(&history.stdout)
    );
}

#[test]
fn a_non_utf8_control_blob_is_refused_before_it_reaches_git_history() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before = control.head().unwrap();

    fs::create_dir_all(fixture.control.join("cards")).unwrap();
    fs::write(
        fixture.control.join("cards/invalid.json"),
        b"{\"note\":\"\xff\"}\n",
    )
    .unwrap();
    let invalid_bytes = fs::read(fixture.control.join("cards/invalid.json")).unwrap();
    let would_be_blob = git_hash_object(&fixture.control, &invalid_bytes);
    assert!(!git_has_object(&fixture.control, &would_be_blob));

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ]);
    assert_eq!(output.status.code(), Some(5), "encoding refusal expected");
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        rendered.contains("CH-POLICY-CONTROL-ENCODING"),
        "the refusal must identify the policy: {rendered}"
    );
    assert_eq!(control.head().unwrap(), before, "HEAD must not advance");
    assert!(
        !git_has_object(&fixture.control, &would_be_blob),
        "the rejected blob must not enter Git's object database"
    );
}

#[test]
fn a_recognized_github_token_in_staged_utf8_is_refused_without_becoming_durable() {
    const TOKEN: &str = "ghp_0123456789abcdef0123456789abcdef0123";

    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before_head = control.head().unwrap();
    let before_index = fs::read(fixture.control.join(".git/index")).unwrap();

    let contents = format!(
        r#"{{"note":"{TOKEN}"}}
"#
    );
    fs::create_dir_all(fixture.control.join("cards")).unwrap();
    fs::write(
        fixture.control.join("cards/external-credential.json"),
        contents.as_bytes(),
    )
    .unwrap();
    let would_be_blob = git_hash_object(&fixture.control, contents.as_bytes());
    assert!(!git_has_object(&fixture.control, &would_be_blob));

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ]);
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // This assertion is deliberately first: bypassing the staged-content
    // classifier must make the mutation fail by observing durable leakage.
    assert!(
        !git_has_object(&fixture.control, &would_be_blob),
        "the recognized token became a durable Git object"
    );
    assert_eq!(output.status.code(), Some(5), "policy refusal expected");
    assert!(
        rendered.contains("CH-POLICY-SENSITIVE-VALUE"),
        "the refusal must identify the narrow credential policy"
    );
    assert!(
        !rendered.contains(TOKEN),
        "the refusal output must not echo the token"
    );
    assert_eq!(
        control.head().unwrap(),
        before_head,
        "HEAD must not advance"
    );
    assert_eq!(
        fs::read(fixture.control.join(".git/index")).unwrap(),
        before_index,
        "the durable index must remain byte-identical"
    );
    let history = Command::new("git")
        .arg("-C")
        .arg(&fixture.control)
        .args(["log", "--all", "-p"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&history.stdout).contains(TOKEN),
        "the recognized token must be absent from every ref"
    );
}

#[test]
fn a_credential_shaped_gitlink_path_is_refused_before_mode_dispatch() {
    const TOKEN: &str = "ghp_0123456789abcdef0123456789abcdef0123";

    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before_head = control.head().unwrap();
    let before_index = fs::read(fixture.control.join(".git/index")).unwrap();

    let relative = format!("cards/fixture_{TOKEN}_gitlink");
    let gitlink = fixture.control.join(&relative);
    fs::create_dir_all(&gitlink).unwrap();
    git(&gitlink, &["init", "-q", "-b", "main"]);
    git(&gitlink, &["config", "user.email", "gitlink@local.invalid"]);
    git(&gitlink, &["config", "user.name", "Gitlink"]);
    fs::write(gitlink.join("README.md"), "gitlink\n").unwrap();
    git(&gitlink, &["add", "-A"]);
    git(&gitlink, &["commit", "-q", "-m", "gitlink"]);
    let nested_head = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&gitlink)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let nested_head = nested_head.trim();
    assert!(!git_has_object(&fixture.control, nested_head));

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ]);
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !git_has_object(&fixture.control, nested_head),
        "the rejected gitlink commit became a durable object"
    );
    assert_eq!(output.status.code(), Some(5), "policy refusal expected");
    assert!(rendered.contains("CH-POLICY-SENSITIVE-VALUE"));
    assert!(!rendered.contains(TOKEN));
    assert!(!rendered.contains(&relative));
    assert_eq!(
        control.head().unwrap(),
        before_head,
        "HEAD must not advance"
    );
    assert_eq!(
        fs::read(fixture.control.join(".git/index")).unwrap(),
        before_index,
        "the durable index must remain byte-identical"
    );
    let cached = Command::new("git")
        .arg("-C")
        .arg(&fixture.control)
        .args(["diff", "--cached", "--quiet"])
        .output()
        .unwrap();
    assert!(
        cached.status.success(),
        "the durable index must remain clean"
    );
}

#[test]
fn incomplete_staged_json_is_refused_without_durable_state_or_payload_leak() {
    const PAYLOAD: &str = "malformed-json-control-sentinel";

    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before_head = control.head().unwrap();
    let before_index = fs::read(fixture.control.join(".git/index")).unwrap();

    let contents = format!(r#"{{"note":"{PAYLOAD}"}} trailing"#);
    fs::create_dir_all(fixture.control.join("cards")).unwrap();
    fs::write(
        fixture.control.join("cards/incomplete.json"),
        contents.as_bytes(),
    )
    .unwrap();
    let would_be_blob = git_hash_object(&fixture.control, contents.as_bytes());
    assert!(!git_has_object(&fixture.control, &would_be_blob));

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ]);
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !git_has_object(&fixture.control, &would_be_blob),
        "incompletely inspected JSON became a durable Git object"
    );
    assert_eq!(output.status.code(), Some(5), "policy refusal expected");
    assert!(
        rendered.contains("CH-POLICY-CONTROL-JSON-INSPECTION"),
        "the refusal must identify incomplete JSON inspection: {rendered}"
    );
    assert!(
        !rendered.contains(PAYLOAD),
        "the refusal must not echo the rejected JSON payload: {rendered}"
    );
    assert_eq!(
        control.head().unwrap(),
        before_head,
        "HEAD must not advance"
    );
    assert_eq!(
        fs::read(fixture.control.join(".git/index")).unwrap(),
        before_index,
        "the durable index must remain byte-identical"
    );
}

#[test]
fn an_external_sensitive_json_is_refused_before_cycle_transaction_state() {
    const TOKEN: &str = "ghp_0123456789abcdef0123456789abcdef0123";

    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before_head = control.head().unwrap();
    let before_index = fs::read(fixture.control.join(".git/index")).unwrap();
    let before_objects = file_snapshot(&fixture.control.join(".git/objects"));
    let before_cycles = file_snapshot(&fixture.control.join("cycles"));
    let before_events = file_snapshot(&fixture.control.join("events"));
    let before_journal = file_snapshot(&fixture.control.join("journal"));

    let contents = format!(
        r#"{{"note":"{TOKEN}"}}
"#
    );
    let external = fixture.control.join("cards/external-sensitive.json");
    fs::create_dir_all(external.parent().unwrap()).unwrap();
    fs::write(&external, contents.as_bytes()).unwrap();
    let would_be_blob = git_hash_object(&fixture.control, contents.as_bytes());
    assert!(!git_has_object(&fixture.control, &would_be_blob));

    let args = vec![
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ];
    let refused = Fixture::run(&args);
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );

    assert_eq!(refused.status.code(), Some(5), "policy refusal expected");
    assert!(rendered.contains("CH-POLICY-SENSITIVE-VALUE"));
    assert!(!rendered.contains(TOKEN));
    assert!(!git_has_object(&fixture.control, &would_be_blob));
    assert_eq!(
        control.head().unwrap(),
        before_head,
        "HEAD must not advance"
    );
    assert_eq!(
        fs::read(fixture.control.join(".git/index")).unwrap(),
        before_index,
        "the durable index must remain unchanged"
    );
    assert_eq!(
        file_snapshot(&fixture.control.join(".git/objects")),
        before_objects,
        "durable objects must remain unchanged"
    );
    assert_eq!(
        file_snapshot(&fixture.control.join("cycles")),
        before_cycles,
        "cycle files must not be authored"
    );
    assert_eq!(
        file_snapshot(&fixture.control.join("events")),
        before_events,
        "event files must not be authored"
    );
    assert_eq!(
        file_snapshot(&fixture.control.join("journal")),
        before_journal,
        "the journal inventory must remain unchanged"
    );

    fs::remove_file(&external).unwrap();
    let retry = Fixture::run(&args);
    assert!(
        retry.status.success(),
        "removing only the external file must allow the identical retry: {}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(fixture.control.join("cycles/C-001.json").exists());
    assert!(Journal::new(&control).unresolved().unwrap().is_empty());
}

#[test]
fn an_rsa_private_key_header_line_is_refused_but_inline_prose_commits() {
    const HEADER: &str = "-----BEGIN RSA PRIVATE KEY-----";

    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before_head = control.head().unwrap();
    let before_index = fs::read(fixture.control.join(".git/index")).unwrap();
    let before_objects = file_snapshot(&fixture.control.join(".git/objects"));
    let before_cycles = file_snapshot(&fixture.control.join("cycles"));
    let before_events = file_snapshot(&fixture.control.join("events"));
    let before_journal = file_snapshot(&fixture.control.join("journal"));

    let rows = [
        (
            "cards/rsa-header.json",
            format!(r#"{{"note":"before\n  {HEADER}\nafter"}}"#),
            true,
        ),
        (
            "cards/inline-prose.json",
            format!(r#"{{"note":"quoted \"{HEADER}\nunrelated newline"}}"#),
            false,
        ),
    ];
    let args = vec![
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ];

    for (relative, contents, should_refuse) in rows {
        let path = fixture.control.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents.as_bytes()).unwrap();
        let would_be_blob = git_hash_object(&fixture.control, contents.as_bytes());
        assert!(!git_has_object(&fixture.control, &would_be_blob));

        let output = Fixture::run(&args);
        let rendered = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        if should_refuse {
            assert_eq!(output.status.code(), Some(5), "{rendered}");
            assert!(rendered.contains("CH-POLICY-SENSITIVE-VALUE"));
            assert!(!git_has_object(&fixture.control, &would_be_blob));
            assert_eq!(control.head().unwrap(), before_head);
            assert_eq!(
                fs::read(fixture.control.join(".git/index")).unwrap(),
                before_index
            );
            assert_eq!(
                file_snapshot(&fixture.control.join(".git/objects")),
                before_objects
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("cycles")),
                before_cycles
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("events")),
                before_events
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("journal")),
                before_journal
            );
            fs::remove_file(path).unwrap();
        } else {
            assert!(output.status.success(), "{rendered}");
            assert!(git_has_object(&fixture.control, &would_be_blob));
            assert_ne!(control.head().unwrap(), before_head);
            assert!(fixture.control.join("cycles/C-001.json").exists());
            assert!(Journal::new(&control).unresolved().unwrap().is_empty());
        }
    }
}

#[test]
fn an_anthropic_api_key_shape_is_refused_but_benign_sk_prose_commits() {
    const SENSITIVE: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
    const BENIGN: &str = "sk-learn-regression-suite";

    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before_head = control.head().unwrap();
    let before_index = fs::read(fixture.control.join(".git/index")).unwrap();
    let before_objects = file_snapshot(&fixture.control.join(".git/objects"));
    let before_cycles = file_snapshot(&fixture.control.join("cycles"));
    let before_events = file_snapshot(&fixture.control.join("events"));
    let before_journal = file_snapshot(&fixture.control.join("journal"));

    let rows = [
        (
            "cards/anthropic.json",
            format!(r#"{{"note":"{SENSITIVE}"}}"#),
            true,
        ),
        (
            "cards/benign-sk.json",
            format!(r#"{{"{SENSITIVE}":"ordinary","note":"{BENIGN}"}}"#),
            false,
        ),
    ];
    let args = vec![
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ];

    for (relative, contents, should_refuse) in rows {
        let path = fixture.control.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents.as_bytes()).unwrap();
        let would_be_blob = git_hash_object(&fixture.control, contents.as_bytes());
        assert!(!git_has_object(&fixture.control, &would_be_blob));

        let output = Fixture::run(&args);
        let rendered = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        if should_refuse {
            assert_eq!(output.status.code(), Some(5), "{rendered}");
            assert!(rendered.contains("CH-POLICY-SENSITIVE-VALUE"));
            assert!(!git_has_object(&fixture.control, &would_be_blob));
            assert_eq!(control.head().unwrap(), before_head);
            assert_eq!(
                fs::read(fixture.control.join(".git/index")).unwrap(),
                before_index
            );
            assert_eq!(
                file_snapshot(&fixture.control.join(".git/objects")),
                before_objects
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("cycles")),
                before_cycles
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("events")),
                before_events
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("journal")),
                before_journal
            );
            fs::remove_file(path).unwrap();
        } else {
            assert!(output.status.success(), "{rendered}");
            assert!(git_has_object(&fixture.control, &would_be_blob));
            assert_ne!(control.head().unwrap(), before_head);
            assert!(fixture.control.join("cycles/C-001.json").exists());
            assert!(Journal::new(&control).unresolved().unwrap().is_empty());
        }
    }
}

#[test]
fn json_key_shape_matrix_preserves_existing_key_checks() {
    const ANTHROPIC: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
    const GITHUB: &str = "ghp_0123456789abcdef0123456789abcdef0123";
    const RSA: &str = "-----BEGIN RSA PRIVATE KEY-----";

    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let rows = [
        (
            "cards/anthropic-key.json",
            format!(r#"{{"{ANTHROPIC}":"ordinary"}}"#),
            true,
        ),
        (
            "cards/github-key.json",
            format!(r#"{{"{GITHUB}":"ordinary"}}"#),
            false,
        ),
        (
            "cards/rsa-key.json",
            format!(r#"{{"{RSA}":"ordinary"}}"#),
            false,
        ),
    ];

    for (index, (relative, contents, should_commit)) in rows.into_iter().enumerate() {
        let before_head = control.head().unwrap();
        let before_index = fs::read(fixture.control.join(".git/index")).unwrap();
        let before_objects = file_snapshot(&fixture.control.join(".git/objects"));
        let before_cycles = file_snapshot(&fixture.control.join("cycles"));
        let before_events = file_snapshot(&fixture.control.join("events"));
        let before_journal = file_snapshot(&fixture.control.join("journal"));
        let path = fixture.control.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents.as_bytes()).unwrap();
        let would_be_blob = git_hash_object(&fixture.control, contents.as_bytes());
        assert!(!git_has_object(&fixture.control, &would_be_blob));

        let output = Fixture::run(&[
            "cycle".into(),
            "create".into(),
            "--output".into(),
            "json".into(),
            "--control".into(),
            fixture.control.display().to_string(),
            "--cycle-id".into(),
            format!("C-{:03}", index + 1),
            "--objective".into(),
            "ordinary".into(),
        ]);
        let rendered = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        if should_commit {
            assert!(output.status.success(), "{rendered}");
            assert!(git_has_object(&fixture.control, &would_be_blob));
            assert_ne!(control.head().unwrap(), before_head);
        } else {
            assert_eq!(output.status.code(), Some(5), "{rendered}");
            assert!(rendered.contains("CH-POLICY-SENSITIVE-VALUE"));
            assert!(!git_has_object(&fixture.control, &would_be_blob));
            assert_eq!(control.head().unwrap(), before_head);
            assert_eq!(
                fs::read(fixture.control.join(".git/index")).unwrap(),
                before_index
            );
            assert_eq!(
                file_snapshot(&fixture.control.join(".git/objects")),
                before_objects
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("cycles")),
                before_cycles
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("events")),
                before_events
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("journal")),
                before_journal
            );
            fs::remove_file(path).unwrap();
        }
    }
}

#[test]
fn an_https_userinfo_password_is_refused_but_compact_json_like_text_commits() {
    const SENSITIVE: &str = "https://deploy:hunter2@internal.example/repo.git";
    const BENIGN: &str =
        r#"{"note":"{\"url\":\"https://internal.example\",\"owner\":\"ops@example.com\"}"}"#;

    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before_head = control.head().unwrap();
    let before_index = fs::read(fixture.control.join(".git/index")).unwrap();
    let before_objects = file_snapshot(&fixture.control.join(".git/objects"));
    let before_cycles = file_snapshot(&fixture.control.join("cycles"));
    let before_events = file_snapshot(&fixture.control.join("events"));
    let before_journal = file_snapshot(&fixture.control.join("journal"));

    let rows = [
        (
            "cards/https-userinfo.json",
            format!(r#"{{"url":"{SENSITIVE}"}}"#),
            true,
        ),
        ("cards/compact-json-like.json", BENIGN.to_owned(), false),
    ];
    let args = vec![
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ];

    for (relative, contents, should_refuse) in rows {
        let path = fixture.control.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents.as_bytes()).unwrap();
        let would_be_blob = git_hash_object(&fixture.control, contents.as_bytes());
        assert!(!git_has_object(&fixture.control, &would_be_blob));

        let output = Fixture::run(&args);
        let rendered = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        if should_refuse {
            assert_eq!(output.status.code(), Some(5), "{rendered}");
            assert!(rendered.contains("CH-POLICY-SENSITIVE-VALUE"));
            assert!(!rendered.contains("hunter2"));
            assert!(!rendered.contains(SENSITIVE));
            assert!(!git_has_object(&fixture.control, &would_be_blob));
            assert_eq!(control.head().unwrap(), before_head);
            assert_eq!(
                fs::read(fixture.control.join(".git/index")).unwrap(),
                before_index
            );
            assert_eq!(
                file_snapshot(&fixture.control.join(".git/objects")),
                before_objects
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("cycles")),
                before_cycles
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("events")),
                before_events
            );
            assert_eq!(
                file_snapshot(&fixture.control.join("journal")),
                before_journal
            );
            fs::remove_file(path).unwrap();
        } else {
            assert!(output.status.success(), "{rendered}");
            assert!(git_has_object(&fixture.control, &would_be_blob));
            assert_ne!(control.head().unwrap(), before_head);
            assert!(fixture.control.join("cycles/C-001.json").exists());
            assert!(Journal::new(&control).unresolved().unwrap().is_empty());
        }
    }
}

#[test]
fn a_nested_escaped_github_token_is_refused_before_durability() {
    const TOKEN: &str = "ghp_0123456789abcdef0123456789abcdef0123";
    const ESCAPED_TOKEN: &str = "\\u0067hp_0123456789abcdef0123456789abcdef0123";

    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before_head = control.head().unwrap();
    let before_index = fs::read(fixture.control.join(".git/index")).unwrap();

    let contents = format!(r#"{{"outer":[{{"inner":{{"note":"{ESCAPED_TOKEN}"}}}}]}}"#);
    assert!(!contents.contains(TOKEN));
    fs::create_dir_all(fixture.control.join("cards")).unwrap();
    fs::write(
        fixture.control.join("cards/escaped-nested.json"),
        contents.as_bytes(),
    )
    .unwrap();
    let would_be_blob = git_hash_object(&fixture.control, contents.as_bytes());
    assert!(!git_has_object(&fixture.control, &would_be_blob));

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ]);
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !git_has_object(&fixture.control, &would_be_blob),
        "decoded escaped token became a durable Git object"
    );
    assert_eq!(output.status.code(), Some(5), "policy refusal expected");
    assert!(output.stderr.is_empty());
    assert!(
        rendered.contains("CH-POLICY-SENSITIVE-VALUE"),
        "the refusal must identify the existing token policy: {rendered}"
    );
    assert!(!rendered.contains(TOKEN));
    assert!(!rendered.contains(ESCAPED_TOKEN));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        envelope["error"]["message"],
        "control state: control entry `cards/escaped-nested.json:outer[0].inner.note` contains a sensitive value"
    );
    assert_eq!(
        envelope["error"]["details"]["reason"],
        "control entry `cards/escaped-nested.json:outer[0].inner.note` contains a sensitive value"
    );
    assert_eq!(control.head().unwrap(), before_head);
    assert_eq!(
        fs::read(fixture.control.join(".git/index")).unwrap(),
        before_index
    );
}

#[test]
fn a_json_recursion_limit_refusal_names_the_path_without_payload_leak() {
    const PAYLOAD: &str = "recursion-json-control-sentinel";

    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before_head = control.head().unwrap();
    let before_index = fs::read(fixture.control.join(".git/index")).unwrap();

    let mut contents = String::new();
    for _ in 0..150 {
        contents.push('[');
    }
    contents.push('"');
    contents.push_str(PAYLOAD);
    contents.push('"');
    for _ in 0..150 {
        contents.push(']');
    }
    fs::create_dir_all(fixture.control.join("cards")).unwrap();
    fs::write(
        fixture.control.join("cards/recursion-limit.json"),
        contents.as_bytes(),
    )
    .unwrap();
    let would_be_blob = git_hash_object(&fixture.control, contents.as_bytes());
    assert!(!git_has_object(&fixture.control, &would_be_blob));

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ]);
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(!git_has_object(&fixture.control, &would_be_blob));
    assert_eq!(output.status.code(), Some(5), "policy refusal expected");
    assert!(rendered.contains("CH-POLICY-CONTROL-JSON-INSPECTION"));
    assert!(rendered.contains("cards/recursion-limit.json"));
    assert!(rendered.contains("recursion limit"));
    assert!(!rendered.contains(PAYLOAD));
    assert_eq!(control.head().unwrap(), before_head);
    assert_eq!(
        fs::read(fixture.control.join(".git/index")).unwrap(),
        before_index
    );
}

#[test]
fn a_split_index_refusal_does_not_create_or_change_shared_indexes() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before = control.head().unwrap();

    git(&fixture.control, &["config", "core.splitIndex", "true"]);
    git(&fixture.control, &["update-index", "--split-index"]);
    let before_index = fs::read(fixture.control.join(".git/index")).unwrap();
    let before_shared_indexes = shared_index_snapshot(&fixture.control);
    assert!(
        !before_shared_indexes.is_empty(),
        "the fixture must exercise Git's split-index path"
    );

    fs::create_dir_all(fixture.control.join("cards")).unwrap();
    let invalid_bytes = b"{\"note\":\"\xff\"}\n";
    fs::write(
        fixture.control.join("cards/split-invalid.json"),
        invalid_bytes,
    )
    .unwrap();
    let would_be_blob = git_hash_object(&fixture.control, invalid_bytes);
    assert!(!git_has_object(&fixture.control, &would_be_blob));

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ]);
    assert_eq!(output.status.code(), Some(5), "encoding refusal expected");
    assert_eq!(control.head().unwrap(), before, "HEAD must not advance");
    assert_eq!(
        fs::read(fixture.control.join(".git/index")).unwrap(),
        before_index
    );
    assert_eq!(
        shared_index_snapshot(&fixture.control),
        before_shared_indexes,
        "a refusal must not create, change, or delete durable shared indexes"
    );
    assert!(
        !git_has_object(&fixture.control, &would_be_blob),
        "the rejected blob must not enter Git's object database"
    );
}

#[test]
fn a_clean_filter_cannot_create_a_non_text_control_blob() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before = control.head().unwrap();

    // The worktree bytes are valid UTF-8, but the configured clean filter
    // replaces them with one invalid byte at the Git staging boundary.
    fs::create_dir_all(fixture.control.join(".git/info")).unwrap();
    fs::write(
        fixture.control.join(".git/info/attributes"),
        "cards/** filter=inject\n",
    )
    .unwrap();
    git(
        &fixture.control,
        &["config", "filter.inject.clean", r"printf '\377'"],
    );
    fs::create_dir_all(fixture.control.join("cards")).unwrap();
    fs::write(
        fixture.control.join("cards/filtered.json"),
        b"{\"note\":\"valid worktree bytes\"}\n",
    )
    .unwrap();
    let transformed_blob = git_hash_object(&fixture.control, &[0xff]);
    assert!(!git_has_object(&fixture.control, &transformed_blob));

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ]);
    assert_eq!(output.status.code(), Some(5), "encoding refusal expected");
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        rendered.contains("CH-POLICY-CONTROL-ENCODING"),
        "the refusal must identify the policy: {rendered}"
    );
    assert_eq!(control.head().unwrap(), before, "HEAD must not advance");
    assert!(
        !git_has_object(&fixture.control, &transformed_blob),
        "filter output must not enter Git's durable object database"
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&fixture.control)
            .args(["diff", "--cached", "--quiet"])
            .status()
            .unwrap()
            .success(),
        "a refusal must leave no staged Git object behind"
    );
}

#[test]
fn a_preexisting_index_lock_preserves_the_durable_index_during_promotion() {
    let fixture = Fixture::new();
    assert!(fixture.init().status.success());
    let control = ControlRepository::open(&fixture.control).unwrap();
    let before = control.head().unwrap();
    let durable_index = fixture.control.join(".git/index");
    let before_index = fs::read(&durable_index).unwrap();
    let index_lock = fixture.control.join(".git/index.lock");
    fs::write(&index_lock, b"another writer owns this lock").unwrap();

    fs::create_dir_all(fixture.control.join("cards")).unwrap();
    let valid_bytes = b"{\"note\":\"must stay quarantined\"}\n";
    fs::write(fixture.control.join("cards/locked.json"), valid_bytes).unwrap();
    let would_be_blob = git_hash_object(&fixture.control, valid_bytes);
    assert!(!git_has_object(&fixture.control, &would_be_blob));

    let output = Fixture::run(&[
        "cycle".into(),
        "create".into(),
        "--output".into(),
        "json".into(),
        "--control".into(),
        fixture.control.display().to_string(),
        "--cycle-id".into(),
        "C-001".into(),
        "--objective".into(),
        "ordinary".into(),
    ]);
    assert!(
        !output.status.success(),
        "an existing index lock must refuse"
    );
    assert_eq!(control.head().unwrap(), before, "HEAD must not advance");
    assert_eq!(fs::read(&durable_index).unwrap(), before_index);
    assert_eq!(
        fs::read(&index_lock).unwrap(),
        b"another writer owns this lock"
    );
    assert!(
        !git_has_object(&fixture.control, &would_be_blob),
        "a lock refusal must not promote quarantine objects"
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&fixture.control)
            .args(["diff", "--cached", "--quiet"])
            .status()
            .unwrap()
            .success(),
        "a lock refusal must leave the durable index unstaged"
    );
}

fn file_snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn visit(root: &Path, current: &Path, files: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(current) else {
            return;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                visit(root, &path, files);
            } else if metadata.is_file() {
                files.push((
                    path.strip_prefix(root).unwrap().display().to_string(),
                    fs::read(&path).unwrap(),
                ));
            }
        }
    }

    let mut files = Vec::new();
    if root.exists() {
        visit(root, root, &mut files);
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn git_hash_object(repo: &Path, bytes: &[u8]) -> String {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn git_has_object(repo: &Path, object: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-e", object])
        .status()
        .unwrap()
        .success()
}

fn shared_index_snapshot(repo: &Path) -> Vec<(String, Vec<u8>)> {
    let mut snapshot = fs::read_dir(repo.join(".git"))
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            if name.starts_with("sharedindex.") && entry.metadata().unwrap().is_file() {
                Some((name, fs::read(entry.path()).unwrap()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
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

/// A `HOME` carrying the Git configuration an ordinary developer is told to
/// set, plus the global hook and template routes that reach a fresh repository.
///
/// Delivered as `HOME` and `XDG_CONFIG_HOME` deliberately. `GIT_CONFIG_GLOBAL`
/// looks equivalent and is not: the Git layer strips it from every child
/// environment, so a fixture built that way passes against unfixed code while
/// proving nothing. `~/.gitconfig` has no such shield.
fn operator_home(root: &Path) -> PathBuf {
    let home = root.join("operator-home");
    for relative in ["template/hooks", "global-hooks", ".config"] {
        fs::create_dir_all(home.join(relative)).unwrap();
    }
    for relative in ["template/hooks/pre-commit", "global-hooks/pre-commit"] {
        let path = home.join(relative);
        fs::write(&path, "#!/bin/sh\necho refused >&2\nexit 1\n").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
    fs::write(
        home.join(".gitconfig"),
        format!(
            "[init]\n\ttemplateDir = {template}\n[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = /nonexistent/gpg\n[core]\n\thooksPath = {hooks}\n",
            template = home.join("template").display(),
            hooks = home.join("global-hooks").display(),
        ),
    )
    .unwrap();
    home
}

#[test]
fn the_control_repository_commits_under_an_operators_signing_and_hook_configuration() {
    // Tier 4. The harness's own audit trail was subject to whatever the
    // operator's `~/.gitconfig` said, so `commit.gpgsign = true` — which
    // GitHub's own documentation tells developers to set — made `project init`
    // fail outright with CH-EXTERNAL-GIT-COMMAND and left control history
    // unborn. A global template or `core.hooksPath` carrying a `pre-commit` did
    // the same. The control repository is created and written only by the
    // harness, so it now says so in its own local configuration.
    let fixture = Fixture::new();
    let home = operator_home(&fixture.root);

    // The fixture must actually be hostile. Without this, a host where the
    // configuration silently did nothing would make the assertions below hold
    // against the unfixed code too.
    let scratch = fixture.root.join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "s@local.invalid"],
        vec!["config", "user.name", "S"],
    ] {
        Command::new("git")
            .arg("-C")
            .arg(&scratch)
            .args(&args)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .output()
            .unwrap();
    }
    let refused = Command::new("git")
        .arg("-C")
        .arg(&scratch)
        .args(["commit", "-q", "--allow-empty", "-m", "probe"])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .output()
        .unwrap();
    assert!(
        !refused.status.success(),
        "the fixture must break an ordinary commit, or it proves nothing"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(fixture.init_args())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .output()
        .expect("the CLI should start");
    assert!(
        output.status.success(),
        "project init must not depend on the operator's Git configuration: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let control = ControlRepository::open(&fixture.control).unwrap();
    assert_eq!(
        control.commit_count().unwrap(),
        1,
        "the audit trail must actually have a commit in it"
    );
    let raw = Command::new("git")
        .arg("-C")
        .arg(&fixture.control)
        .args(["cat-file", "commit", "HEAD"])
        .env("HOME", &home)
        .output()
        .unwrap();
    let raw = String::from_utf8_lossy(&raw.stdout);
    let headers = raw
        .split_once("\n\n")
        .map_or(raw.as_ref(), |(head, _)| head);
    assert!(
        !headers.lines().any(|line| line.starts_with("gpgsig")),
        "a control commit must not carry the operator's signature: {headers}"
    );
    assert!(
        !fixture.control.join(".git/hooks/pre-commit").exists(),
        "the operator's template must not install scripts into the audit trail"
    );

    // An already-initialized control repository is the case local settings
    // alone cannot repair: `project init` exits before `initialize_git`.
    // Strip them to model an external edit, then make a real control write
    // under the same hostile HOME. The invocation-level overrides above must
    // carry this commit; without them the global pre-commit/signing setup wins.
    git(&fixture.control, &["config", "--unset", "commit.gpgsign"]);
    git(&fixture.control, &["config", "--unset", "core.hooksPath"]);
    let control_write = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args([
            "cycle",
            "create",
            "--control",
            &fixture.control.display().to_string(),
            "--cycle-id",
            "C-001",
            "--objective",
            "control write under hostile configuration",
        ])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .output()
        .expect("the CLI should start");
    assert!(
        control_write.status.success(),
        "an existing control repository must commit despite hostile Git configuration: {}",
        String::from_utf8_lossy(&control_write.stderr)
    );
    assert_eq!(
        control.commit_count().unwrap(),
        2,
        "the real control write must create its audit commit"
    );
}
