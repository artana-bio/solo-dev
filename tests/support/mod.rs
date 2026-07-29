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

        // `project init` creates the bare authority, registers its remote, and
        // seeds the protected branch, so the fixture leaves that to the harness.

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
        // Cards may only name registered gates, so the fixtures every test uses
        // must exist before any card is activated.
        workspace.register_gate("gate.unit", &["true"]);
        workspace.register_gate("gate.all", &["true"]);
        workspace
    }

    /// Registers a gate at revision 1 with the given argv.
    pub fn register_gate(&self, gate_id: &str, argv: &[&str]) {
        self.register_gate_revision(gate_id, 1, argv);
    }

    /// Registers a specific revision of a gate, so a definition can change.
    pub fn register_gate_revision(&self, gate_id: &str, revision: u32, argv: &[&str]) {
        let list = argv
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: {revision}\nargv: [{list}]\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set: {{}}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
        );
        let path = self.root.join(format!("{gate_id}.yaml"));
        fs::write(&path, body).unwrap();
        let output = Self::run(&[
            "gate".into(),
            "register".into(),
            "--control".into(),
            self.control.display().to_string(),
            "--definition".into(),
            path.display().to_string(),
        ]);
        assert!(
            output.status.success(),
            "gate register {gate_id} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Runs a `gate` subcommand in JSON mode without asserting success.
    pub fn gate_raw(&self, args: &[&str]) -> Output {
        let mut full = vec![
            "gate".to_owned(),
            args[0].to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
        ];
        if args[0] != "validate" {
            full.push("--control".to_owned());
            full.push(self.control.display().to_string());
        }
        full.extend(args[1..].iter().map(|arg| (*arg).to_owned()));
        Self::run(&full)
    }

    /// Runs a `gate` subcommand, asserting success.
    pub fn gate(&self, args: &[&str]) -> Output {
        let output = self.gate_raw(args);
        assert!(
            output.status.success(),
            "gate {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Runs a `gate` subcommand and parses its JSON envelope.
    pub fn gate_json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_slice(&self.gate(args).stdout).expect("the JSON envelope")
    }

    /// Writes a gate definition file and returns its path.
    pub fn gate_definition(&self, name: &str, body: &str) -> String {
        let path = self.root.join(format!("{name}.yaml"));
        fs::write(&path, body).unwrap();
        path.display().to_string()
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

    /// Runs a `work` subcommand in JSON mode without asserting success.
    pub fn work_raw(&self, args: &[&str]) -> Output {
        let mut full = vec![
            "work".to_owned(),
            args[0].to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            self.control.display().to_string(),
        ];
        full.extend(args[1..].iter().map(|arg| (*arg).to_owned()));
        Self::run(&full)
    }

    /// Runs a `work` subcommand, asserting success.
    pub fn work(&self, args: &[&str]) -> Output {
        let output = self.work_raw(args);
        assert!(
            output.status.success(),
            "work {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Runs a `work` subcommand and parses its JSON envelope.
    pub fn work_json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_slice(&self.work(args).stdout).expect("the JSON envelope")
    }

    /// Creates and activates a card claiming the given paths.
    pub fn activate_card(&self, card_id: &str, include: &[&str]) {
        self.activate_card_with_base(card_id, include, &self.authority_head());
    }

    /// Creates and activates a card naming explicit feature gates.
    pub fn activate_card_with_gates(&self, card_id: &str, include: &[&str], gates: &[&str]) {
        let list = |values: &[&str]| {
            values
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let body = format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{inc}]\n  exclude: []\nnamed_gates:\n  feature: [{gates}]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
            base = self.authority_head(),
            inc = list(include),
            gates = list(gates),
        );
        let path = self.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        self.card(&["create", "--draft", &path.display().to_string()]);
        self.card(&["activate", "--card-id", card_id]);
    }

    /// Creates and activates a card at a named risk level.
    pub fn activate_card_with_risk(&self, card_id: &str, include: &[&str], risk: &str) {
        let inc = include
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: {risk}\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{inc}]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
            base = self.authority_head(),
        );
        let path = self.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        self.card(&["create", "--draft", &path.display().to_string()]);
        self.card(&["activate", "--card-id", card_id]);
    }

    /// Creates and activates a card with an explicit exclude list.
    pub fn activate_card_excluding(&self, card_id: &str, include: &[&str], exclude: &[&str]) {
        let list = |values: &[&str]| {
            values
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let body = format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{inc}]\n  exclude: [{exc}]\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
            base = self.authority_head(),
            inc = list(include),
            exc = list(exclude),
        );
        let path = self.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        self.card(&["create", "--draft", &path.display().to_string()]);
        self.card(&["activate", "--card-id", card_id]);
    }

    /// Creates and activates a card declaring an explicit base commit.
    pub fn activate_card_with_base(&self, card_id: &str, include: &[&str], base: &str) {
        let includes = include
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{includes}]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n"
        );
        let path = self.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        self.card(&["create", "--draft", &path.display().to_string()]);
        self.card(&["activate", "--card-id", card_id]);
    }

    /// Runs a `handoff` subcommand in JSON mode without asserting success.
    pub fn handoff_raw(&self, args: &[&str]) -> Output {
        let mut full = vec![
            "handoff".to_owned(),
            args[0].to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            self.control.display().to_string(),
        ];
        full.extend(args[1..].iter().map(|arg| (*arg).to_owned()));
        Self::run(&full)
    }

    /// Runs a `handoff` subcommand, asserting success.
    pub fn handoff(&self, args: &[&str]) -> Output {
        let output = self.handoff_raw(args);
        assert!(
            output.status.success(),
            "handoff {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Runs a `handoff` subcommand and parses its JSON envelope.
    pub fn handoff_json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_slice(&self.handoff(args).stdout).expect("the JSON envelope")
    }

    /// Runs a `review` subcommand in JSON mode without asserting success.
    pub fn review_raw(&self, args: &[&str]) -> Output {
        let mut full = vec![
            "review".to_owned(),
            args[0].to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            self.control.display().to_string(),
        ];
        full.extend(args[1..].iter().map(|arg| (*arg).to_owned()));
        Self::run(&full)
    }

    /// Runs a `review` subcommand, asserting success.
    pub fn review(&self, args: &[&str]) -> Output {
        let output = self.review_raw(args);
        assert!(
            output.status.success(),
            "review {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Runs a `review` subcommand and parses its JSON envelope.
    pub fn review_json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_slice(&self.review(args).stdout).expect("the JSON envelope")
    }

    /// Runs an `integration` subcommand, returning the raw output.
    pub fn integration_raw(&self, args: &[&str]) -> Output {
        let mut full = vec![
            "integration".to_owned(),
            args[0].to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            self.control.display().to_string(),
        ];
        full.extend(args[1..].iter().map(|arg| (*arg).to_owned()));
        Self::run(&full)
    }

    /// Runs an `integration` subcommand, asserting success.
    pub fn integration(&self, args: &[&str]) -> Output {
        let output = self.integration_raw(args);
        assert!(
            output.status.success(),
            "integration {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Runs an `integration` subcommand and parses its JSON envelope.
    pub fn integration_json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_slice(&self.integration(args).stdout).expect("the JSON envelope")
    }

    /// Runs an `archive` subcommand, returning the raw output.
    pub fn archive_raw(&self, args: &[&str]) -> Output {
        let mut full = vec![
            "archive".to_owned(),
            args[0].to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            self.control.display().to_string(),
        ];
        full.extend(args[1..].iter().map(|arg| (*arg).to_owned()));
        Self::run(&full)
    }

    /// Runs an `archive` subcommand, asserting success.
    pub fn archive(&self, args: &[&str]) -> Output {
        let output = self.archive_raw(args);
        assert!(
            output.status.success(),
            "archive {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Runs an `archive` subcommand and parses its JSON envelope.
    pub fn archive_json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_slice(&self.archive(args).stdout).expect("the JSON envelope")
    }

    /// Runs the harness and parses its JSON envelope, asserting nothing.
    pub fn run_json(args: &[String]) -> serde_json::Value {
        let output = Self::run(args);
        serde_json::from_slice(&output.stdout).expect("the JSON envelope")
    }

    /// Runs an `acceptance` subcommand, returning the raw output.
    pub fn acceptance_raw(&self, args: &[&str]) -> Output {
        let mut full = vec![
            "acceptance".to_owned(),
            args[0].to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            self.control.display().to_string(),
        ];
        full.extend(args[1..].iter().map(|arg| (*arg).to_owned()));
        Self::run(&full)
    }

    /// Runs an `acceptance` subcommand, asserting success.
    pub fn acceptance(&self, args: &[&str]) -> Output {
        let output = self.acceptance_raw(args);
        assert!(
            output.status.success(),
            "acceptance {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Runs an `acceptance` subcommand and parses its JSON envelope.
    pub fn acceptance_json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_slice(&self.acceptance(args).stdout).expect("the JSON envelope")
    }

    /// Activates a card naming explicit feature and integration gate sets.
    pub fn activate_card_with_gate_sets(
        &self,
        card_id: &str,
        include: &[&str],
        feature: &[&str],
        integration: &[&str],
    ) {
        let list = |values: &[&str]| {
            values
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let body = format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{inc}]\n  exclude: []\nnamed_gates:\n  feature: [{feat}]\n  review: []\n  integration: [{integ}]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
            base = self.authority_head(),
            inc = list(include),
            feat = list(feature),
            integ = list(integration),
        );
        let path = self.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        self.card(&["create", "--draft", &path.display().to_string()]);
        self.card(&["activate", "--card-id", card_id]);
    }

    /// Activates a card that declares dependencies on other cards.
    pub fn activate_card_depending_on(&self, card_id: &str, include: &[&str], depends_on: &[&str]) {
        let list = |values: &[&str]| {
            values
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let body = format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{inc}]\n  exclude: []\ndepends_on: [{deps}]\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
            base = self.authority_head(),
            inc = list(include),
            deps = list(depends_on),
        );
        let path = self.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        self.card(&["create", "--draft", &path.display().to_string()]);
        self.card(&["activate", "--card-id", card_id]);
    }

    /// Carries an activated card all the way to `approved`.
    ///
    /// Work, gate, handoff, and an approval by a reviewer distinct from the
    /// feature actor — the whole pre-integration path, which every integration
    /// test needs and none of them is testing.
    pub fn approve_card(&self, card_id: &str, file: &str) {
        self.work(&["start", "--card-id", card_id]);

        let worktree = self.worktrees.join(card_id);
        let path = worktree.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("// {card_id}\n")).unwrap();
        git(&worktree, &["add", "-A"]);
        git(
            &worktree,
            &["commit", "-q", "-m", &format!("feat: {card_id}")],
        );

        self.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

        let head = capture(&worktree, &["rev-parse", "HEAD"]);
        let declaration = self.root.join(format!("{card_id}-declaration.yaml"));
        fs::write(
            &declaration,
            format!(
                "delivered_sha: {head}\nbehavior_delivered: adds {file}\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
            ),
        )
        .unwrap();
        self.handoff(&[
            "create",
            "--card-id",
            card_id,
            "--declaration",
            &declaration.display().to_string(),
        ]);

        self.review(&["begin", "--card-id", card_id]);
        let verdict = self.root.join(format!("{card_id}-verdict.yaml"));
        fs::write(
            &verdict,
            "reviewer_actor_id: reviewer-session\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\nresidual_risks: []\n",
        )
        .unwrap();
        self.review(&[
            "record",
            "--card-id",
            card_id,
            "--verdict",
            &verdict.display().to_string(),
        ]);
    }

    /// Revises a card, moving its digest.
    pub fn revise_card(&self, card_id: &str, include: &[&str], reason: &str) {
        let list = include
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id} differently\nnon_goals: []\nrisk: medium\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{list}]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
            base = self.authority_head(),
        );
        let path = self.root.join(format!("{card_id}-revised.yaml"));
        fs::write(&path, body).unwrap();
        let output = Self::run(&[
            "card".into(),
            "revise".into(),
            "--control".into(),
            self.control.display().to_string(),
            "--card-id".into(),
            card_id.into(),
            "--draft".into(),
            path.display().to_string(),
            "--reason".into(),
            reason.into(),
        ]);
        assert!(
            output.status.success(),
            "card revise failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Resolves a revision in the candidate repository.
    pub fn candidate_rev(&self, revision: &str) -> String {
        capture(&self.repository, &["rev-parse", revision])
    }

    /// True when a branch exists in the candidate repository.
    pub fn candidate_branch_exists(&self, branch: &str) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(&self.repository)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .expect("git should run")
            .status
            .success()
    }

    /// The candidate repository's worktree inventory, as porcelain lines.
    pub fn candidate_worktrees(&self) -> Vec<String> {
        capture(&self.repository, &["worktree", "list", "--porcelain"])
            .split("\n\n")
            .map(|block| block.replace('\n', " "))
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

    /// Rewrites a stored card's state, simulating a transition made by a
    /// command that does not exist yet.
    pub fn tamper_card_state(&self, card_id: &str, state: &str) {
        let path = self.control.join(format!("cards/{card_id}/state.json"));
        let raw = fs::read_to_string(&path).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["state"] = serde_json::json!(state);
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
        )
        .unwrap();
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

/// Concatenates every gate log stdout under a control repository.
pub fn capture_stdout_of_logs(control: &Path) -> String {
    fn walk(path: &Path, into: &mut String) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, into);
            } else if let Ok(text) = fs::read_to_string(&path) {
                into.push_str(&text);
            }
        }
    }
    let mut collected = String::new();
    walk(&control.join("logs"), &mut collected);
    collected
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
