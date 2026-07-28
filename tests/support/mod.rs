//! Shared fixtures for integration tests.
//!
//! Builds a complete project — candidate repository, control repository, and
//! bare authority — so tests exercise the real command surface rather than
//! internal functions.

#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::TempDir;

/// A temporary project with every repository role populated.
pub struct Workspace {
    _temp: TempDir,
    pub root: PathBuf,
    pub repository: PathBuf,
    pub control: PathBuf,
    pub authority: PathBuf,
    pub worktrees: PathBuf,
}

impl Workspace {
    /// Creates the repositories without running `project init`.
    pub fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().to_path_buf();
        let repository = root.join("repository");
        let control = root.join("control");
        let authority = root.join("authority.git");
        let worktrees = root.join("worktrees");

        fs::create_dir_all(&repository).unwrap();
        git(&repository, &["init", "-q", "-b", "main"]);
        git(&repository, &["config", "user.email", "f@local.invalid"]);
        git(&repository, &["config", "user.name", "Fixture"]);
        fs::write(repository.join("README.md"), "hello\n").unwrap();
        git(&repository, &["add", "-A"]);
        git(&repository, &["commit", "-q", "-m", "initial"]);

        Command::new("git")
            .args(["init", "-q", "--bare", "-b", "main"])
            .arg(&authority)
            .output()
            .expect("git init --bare");
        git(
            &repository,
            &[
                "remote",
                "add",
                "harness-authority",
                authority.to_str().unwrap(),
            ],
        );
        git(&repository, &["push", "-q", "harness-authority", "main"]);

        Self {
            _temp: temp,
            root,
            repository,
            control,
            authority,
            worktrees,
        }
    }

    /// Creates the repositories and runs `project init`.
    pub fn initialized() -> Self {
        let workspace = Self::new();
        let output = Self::run(&[
            "project".into(),
            "init".into(),
            "--project-id".into(),
            "example".into(),
            "--repository".into(),
            workspace.repository.display().to_string(),
            "--control".into(),
            workspace.control.display().to_string(),
            "--authority".into(),
            workspace.authority.display().to_string(),
            "--worktree-root".into(),
            workspace.worktrees.display().to_string(),
        ]);
        assert!(
            output.status.success(),
            "project init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        workspace
    }

    /// Runs the harness binary with the given arguments.
    pub fn run(args: &[String]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_change-harness"))
            .args(args)
            .output()
            .expect("the CLI should start")
    }

    /// Runs a `cycle` subcommand in JSON mode without asserting success.
    pub fn cycle_raw(&self, args: &[&str]) -> Output {
        let mut full = vec![
            "cycle".to_owned(),
            args[0].to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            self.control.display().to_string(),
        ];
        full.extend(args[1..].iter().map(|arg| (*arg).to_owned()));
        Self::run(&full)
    }

    /// Runs a `cycle` subcommand, asserting success.
    pub fn cycle(&self, args: &[&str]) -> Output {
        let output = self.cycle_raw(args);
        assert!(
            output.status.success(),
            "cycle {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Runs a `cycle` subcommand and parses its JSON envelope.
    pub fn cycle_json(&self, args: &[&str]) -> serde_json::Value {
        let output = self.cycle(args);
        serde_json::from_slice(&output.stdout).expect("stdout should be the JSON envelope")
    }

    /// Runs a `card` subcommand in JSON mode without asserting success.
    pub fn card_raw(&self, args: &[&str]) -> Output {
        let mut full = vec![
            "card".to_owned(),
            args[0].to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
        ];
        // `card validate` takes no control repository.
        if args[0] != "validate" {
            full.push("--control".to_owned());
            full.push(self.control.display().to_string());
        }
        full.extend(args[1..].iter().map(|arg| (*arg).to_owned()));
        Self::run(&full)
    }

    /// Runs a `card` subcommand, asserting success.
    pub fn card(&self, args: &[&str]) -> Output {
        let output = self.card_raw(args);
        assert!(
            output.status.success(),
            "card {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Runs a `card` subcommand and parses its JSON envelope.
    pub fn card_json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_slice(&self.card(args).stdout).expect("the JSON envelope")
    }

    /// Every authoritative event recorded in the control repository.
    pub fn events(&self) -> Vec<serde_json::Value> {
        let directory = self.control.join("events");
        if !directory.exists() {
            return Vec::new();
        }
        let mut names: Vec<PathBuf> = fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        names.sort();
        names
            .iter()
            .map(|path| serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap())
            .collect()
    }

    /// The candidate repository's current commit.
    pub fn candidate_head(&self) -> String {
        capture(&self.repository, &["rev-parse", "HEAD"])
    }

    /// The authority repository's protected branch commit.
    pub fn authority_head(&self) -> String {
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(&self.authority)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .expect("git should run");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// The control repository's current commit.
    pub fn control_head(&self) -> String {
        capture(&self.control, &["rev-parse", "HEAD"])
    }

    /// How many commits control history holds.
    pub fn control_commit_count(&self) -> usize {
        capture(&self.control, &["rev-list", "--count", "HEAD"])
            .parse()
            .expect("a commit count")
    }

    /// Every file tracked in the control repository.
    pub fn control_tracked_files(&self) -> Vec<String> {
        capture(&self.control, &["ls-files"])
            .lines()
            .map(ToOwned::to_owned)
            .collect()
    }

    /// Adds a commit to the candidate repository only.
    pub fn commit_candidate(&self, name: &str, contents: &str) -> String {
        fs::write(self.repository.join(name), contents).unwrap();
        git(&self.repository, &["add", "-A"]);
        git(
            &self.repository,
            &["commit", "-q", "-m", &format!("add {name}")],
        );
        self.candidate_head()
    }

    /// Advances the authority's protected branch beyond any frozen baseline.
    pub fn advance_authority(&self) -> String {
        self.commit_candidate("authority-move.txt", "moved\n");
        git(
            &self.repository,
            &["push", "-q", "harness-authority", "main"],
        );
        self.authority_head()
    }

    /// Rewrites a stored cycle's cached status, simulating an external edit.
    pub fn tamper_cycle_status(&self, cycle_id: &str, status: &str) {
        let path = self.control.join(format!("cycles/{cycle_id}.json"));
        let raw = fs::read_to_string(&path).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["status"] = serde_json::json!(status);
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
        )
        .unwrap();
    }
}

/// Runs Git in a fixture, asserting success.
pub fn git(repo: &Path, args: &[&str]) {
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

/// Runs Git and returns its trimmed standard output.
pub fn capture(repo: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
