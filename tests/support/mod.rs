//! Shared fixtures for integration tests.
//!
//! Builds a complete project — candidate repository, control repository, and
//! bare authority — so tests exercise the real command surface rather than
//! internal functions.
//!
//! # Plumbing versus a governed step
//!
//! A fixture may supply plumbing the operator does not type (`--control`,
//! `--output json`). It may not perform a governed step the operator must
//! perform themselves, unless some test also drives that path unaided.
//!
//! `Workspace::gate`'s `run` branch is the one exception, and the reason
//! this rule is written down: it silently runs `gate reserve` on the
//! caller's behalf whenever a `run` call omits `--reservation-id`, so no
//! test written against `gate` alone can reach the refusal an operator gets
//! for that same omission. `tests/gate_runner.rs`'s
//! `gate_run_without_a_reservation_names_the_command_that_makes_one` drives
//! that unhelped path instead, through `gate_raw`; a guard next to it fails
//! if that test is ever deleted or repointed at `gate`.
//!
//! Approved verdicts sent through `Workspace::review` receive a real
//! executable mutation receipt when they do not name evidence themselves.
//! `tests/review_recording.rs` drives the fail-closed production path through
//! `review_raw`, including the missing-evidence refusal. Exemption policies
//! are never installed by shared setup; tests that exercise exemptions must
//! call `install_fixture_mutation_exemption_policy` explicitly.

#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::TempDir;

use change_harness::domain::digest::Digest;

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

    /// Explicitly installs the closed exemption policy used by tests whose
    /// subject is exemption authorization. Ordinary fixtures remain
    /// fail-closed and use executable mutation receipts instead.
    pub fn install_fixture_mutation_exemption_policy(&self) {
        let policy = self.root.join("mutation-exemption-policy.json");
        fs::write(
            &policy,
            r#"{
  "version": "harness.mutation-exemption-policy/v1",
  "rules": [
    {"code":"fixture-no-mutation","approved_by":"independent-attestor","approver_principal_id":"attestor-principal","approver_session_id":"attestor-session"},
    {"code":"fixture","approved_by":"independent-attestor","approver_principal_id":"attestor-principal","approver_session_id":"attestor-session"},
    {"code":"fixture","approved_by":"independent-approver","approver_principal_id":"approver-principal","approver_session_id":"approver-session"},
    {"code":"reference_fixture","approved_by":"reference","approver_principal_id":"reference-principal","approver_session_id":"reference-session"},
    {"code":"example_fixture","approved_by":"example-generator","approver_principal_id":"example-principal","approver_session_id":"example-session"}
  ]
}"#,
        )
        .unwrap();
        let output = Self::run(&[
            "project".into(),
            "set-mutation-exemption-policy".into(),
            "--control".into(),
            self.control.display().to_string(),
            "--policy".into(),
            policy.display().to_string(),
        ]);
        assert!(
            output.status.success(),
            "mutation exemption policy install failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Executes a real disposable mutation and returns its persisted receipt
    /// ID. Callers still have to name the receipt explicitly in the verdict.
    pub fn create_fixture_mutation_receipt(
        &self,
        card_id: &str,
        reviewer_actor_id: &str,
        reviewer_principal_id: &str,
        reviewer_session_id: &str,
    ) -> String {
        const ORACLE: &str = "gate.fixture-mutation";
        if !self.control.join(format!("gates/{ORACLE}.json")).exists() {
            self.register_gate(
                ORACLE,
                &["sh", "-c", "test ! -e .change-harness-mutation-marker"],
            );
        }

        let inspection = self.review_json(&["inspect", "--card-id", card_id]);
        let candidate_sha = inspection["data"]["candidate_sha"]
            .as_str()
            .expect("a handed-off candidate")
            .to_owned();
        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(self.control.join(format!("cards/{card_id}/state.json"))).unwrap(),
        )
        .unwrap();
        let revision = state["current_revision"]
            .as_u64()
            .expect("a current card revision");
        let receipt_count = fs::read_dir(self.control.join("mutation-receipts"))
            .map_or(0, std::iter::Iterator::count);
        let receipt_id = format!("MR-FIXTURE-{:06}", receipt_count + 1);
        let output = Self::run(&[
            "mutation".into(),
            "create".into(),
            "--output".into(),
            "json".into(),
            "--control".into(),
            self.control.display().to_string(),
            "--receipt-id".into(),
            receipt_id.clone(),
            "--card-revision".into(),
            format!("{card_id}-r{revision}"),
            "--candidate-sha".into(),
            candidate_sha,
            "--reviewer-actor-id".into(),
            reviewer_actor_id.to_owned(),
            "--reviewer-principal-id".into(),
            reviewer_principal_id.to_owned(),
            "--reviewer-session-id".into(),
            reviewer_session_id.to_owned(),
            "--mutation-digest".into(),
            Digest::of_bytes(b"fixture mutation command").to_string(),
            "--patch-digest".into(),
            Digest::of_bytes(b"fixture mutation patch").to_string(),
            format!("--command={}", "sh"),
            format!("--command={}", "-c"),
            format!("--command={}", "touch .change-harness-mutation-marker"),
            "--gate-oracle".into(),
            ORACLE.into(),
            "--expected-failure".into(),
            "fixture oracle rejects the mutation marker".into(),
            "--observed-result".into(),
            "fixture mutation executed".into(),
            "--failed-at-oracle".into(),
            "--restoration-proof".into(),
            "disposable worktree restored to the exact candidate".into(),
        ]);
        assert!(
            output.status.success(),
            "mutation receipt creation failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        receipt_id
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
            "schema: harness.gate/v1\ngate_id: {gate_id}\nrevision: {revision}\nargv: [{list}]\nworking_directory: \".\"\ntimeout_seconds: 60\nenvironment:\n  allow: [PATH]\n  set: {{}}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\nmigration: legacy_v1\n"
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
    ///
    /// `run` is the one case that silently performs an extra governed step
    /// first (see this module's "Plumbing versus a governed step" doc): it
    /// reserves on the caller's behalf when `--reservation-id` is missing.
    /// Use `gate_raw` to drive the exact invocation an operator types.
    pub fn gate(&self, args: &[&str]) -> Output {
        let output = if args.first() == Some(&"run") && !args.contains(&"--reservation-id") {
            let value_after = |flag: &str| {
                args.windows(2)
                    .find_map(|pair| (pair[0] == flag).then_some(pair[1]))
                    .expect("gate run fixture must name its card and gate")
            };
            let card_id = value_after("--card-id");
            let gate_id = value_after("--gate-id");
            let actor = args
                .windows(2)
                .find_map(|pair| (pair[0] == "--actor").then_some(pair[1]))
                .unwrap_or("operator");
            let reserve = self.gate_raw(&[
                "reserve",
                "--card-id",
                card_id,
                "--gate-id",
                gate_id,
                "--actor",
                actor,
            ]);
            if reserve.status.success() {
                let reservation: serde_json::Value =
                    serde_json::from_slice(&reserve.stdout).unwrap();
                let reservation_id = reservation["data"]["reservation"]["reservation_id"]
                    .as_str()
                    .expect("reservation id");
                let mut owned = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
                owned.extend(["--reservation-id".to_owned(), reservation_id.to_owned()]);
                let actual = owned.iter().map(String::as_str).collect::<Vec<_>>();
                self.gate_raw(&actual)
            } else {
                // Never fall back to an unreserved execution. A fixture that
                // cannot obtain the capability must surface that refusal.
                reserve
            }
        } else {
            self.gate_raw(args)
        };
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
        let body = if body.contains("schema: harness.gate/v1") && !body.contains("migration:") {
            format!("{body}migration: legacy_v1\n")
        } else {
            body.to_owned()
        };
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

    /// Reproduces a control repository created before plan-required cycle
    /// provenance existed. This is deliberately an explicit fixture mutation,
    /// not a production migration path.
    pub fn mark_cycle_pre_upgrade(&self, cycle_id: &str) {
        let cycle_path = self.control.join(format!("cycles/{cycle_id}.json"));
        let mut cycle: serde_json::Value =
            serde_json::from_slice(&fs::read(&cycle_path).unwrap()).unwrap();
        cycle
            .as_object_mut()
            .unwrap()
            .remove("creation_plan_policy");
        fs::write(
            &cycle_path,
            format!("{}\n", serde_json::to_string_pretty(&cycle).unwrap()),
        )
        .unwrap();
        let events_dir = self.control.join("events");
        for entry in fs::read_dir(events_dir).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let mut event: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            if event["cycle_id"].as_str() == Some(cycle_id)
                && event["event_type"] == "cycle.created"
            {
                event["metadata"]
                    .as_object_mut()
                    .unwrap()
                    .remove("creation_plan_policy");
                fs::write(
                    &path,
                    format!("{}\n", serde_json::to_string_pretty(&event).unwrap()),
                )
                .unwrap();
            }
        }
        git(&self.control, &["add", "-A"]);
        git(
            &self.control,
            &["commit", "-q", "-m", "fixture: pre-upgrade cycle"],
        );
    }

    /// Runs a `work` subcommand in JSON mode without asserting success.
    pub fn work_raw(&self, args: &[&str]) -> Output {
        let mut normalized = args.to_vec();
        if matches!(args.first(), Some(&("start" | "resume")))
            && let Some(card_id) = args
                .windows(2)
                .find_map(|pair| (pair[0] == "--card-id").then_some(pair[1]))
        {
            let card_path = self.control.join(format!("cards/{card_id}/r1.json"));
            if card_path.exists() {
                let card: serde_json::Value =
                    serde_json::from_slice(&fs::read(card_path).unwrap()).unwrap();
                let cycle_id = card["cycle_id"].as_str().unwrap();
                let cycle_path = self.control.join(format!("cycles/{cycle_id}.json"));
                let cycle: serde_json::Value =
                    serde_json::from_slice(&fs::read(cycle_path).unwrap()).unwrap();
                if cycle["plan_id"].is_string() {
                    self.ensure_default_cycle_plan_for(cycle_id);
                    if !normalized.contains(&"--actor-principal-id") {
                        normalized.extend(["--actor-principal-id", "implementer-principal"]);
                    }
                    if !normalized.contains(&"--actor-session-id") {
                        normalized.extend(["--actor-session-id", "implementer-session"]);
                    }
                }
            }
        }
        let mut full = vec![
            "work".to_owned(),
            normalized[0].to_owned(),
            "--output".to_owned(),
            "json".to_owned(),
            "--control".to_owned(),
            self.control.display().to_string(),
        ];
        full.extend(normalized[1..].iter().map(|arg| (*arg).to_owned()));
        Self::run(&full)
    }

    /// Runs a `work` subcommand, asserting success.
    pub fn work(&self, args: &[&str]) -> Output {
        let mut normalized = args.to_vec();
        if matches!(args.first(), Some(&("start" | "resume"))) {
            let card_id = args
                .windows(2)
                .find_map(|pair| (pair[0] == "--card-id").then_some(pair[1]))
                .expect("work fixture must name a card");
            let card: serde_json::Value = serde_json::from_slice(
                &fs::read(self.control.join(format!("cards/{card_id}/r1.json"))).unwrap(),
            )
            .unwrap();
            self.ensure_default_cycle_plan_for(card["cycle_id"].as_str().unwrap());
            if !args.contains(&"--actor-principal-id") {
                normalized.extend(["--actor-principal-id", "implementer-principal"]);
            }
            if !args.contains(&"--actor-session-id") {
                normalized.extend(["--actor-session-id", "implementer-session"]);
            }
        }
        let output = self.work_raw(&normalized);
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
        self.activate_card_with_base(card_id, include, &self.cycle_baseline());
    }

    /// The frozen cycle baseline is the only valid implicit card base. The
    /// authority branch may move while a cycle stays active.
    fn cycle_baseline(&self) -> String {
        self.cycle_json(&["status", "--cycle-id", "C-001"])["data"]["baseline_sha"]
            .as_str()
            .expect("active fixture cycle has a frozen baseline")
            .to_owned()
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
            base = self.cycle_baseline(),
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
        if risk != "low" {
            // Elevated-risk fixtures declare a handoff-stage check. Keep this
            // registration local: tests that exercise gate revisions own the
            // registry themselves.
            self.register_gate("gate.review", &["true"]);
        }
        let inc = include
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let proof_map = if risk == "low" {
            String::new()
        } else {
            "proof_map:\n  schema: harness.proof-map/v1\n  entries:\n    - id: proof-behavior\n      invariant: behavior remains correct\n      precondition: valid fixture\n      assertion: focused test passes\n      mutation: bypass assertion fails\n      gate_oracle: gate.review\n  claim_boundary: only this fixture\n".to_owned()
        };
        let review_gates = if risk == "low" { "[]" } else { "[gate.review]" };
        let body = format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: {risk}\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{inc}]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: {review_gates}\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n{proof_map}",
            base = self.cycle_baseline(),
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
            base = self.cycle_baseline(),
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
        let mut normalized = args.to_vec();
        if args.first() == Some(&"begin") && !args.contains(&"--actor") {
            normalized.extend(["--actor", "reviewer"]);
        }
        if let Some(index) = args.iter().position(|arg| *arg == "--verdict")
            && let Some(path) = args.get(index + 1)
        {
            let verdict_path = PathBuf::from(path);
            if let Ok(mut body) = fs::read_to_string(&verdict_path)
                && body.contains("decision: approved")
            {
                if !body.contains("reviewer_kind:") {
                    body.push_str(
                        "reviewer_kind: agent\nreviewer_provenance:\n  provider: fixture\n  model: fixture\n  session_id: reviewer-session\n  principal_id: reviewer-principal\n",
                    );
                }
                if !body.contains("mutation_receipt_ids:") && !body.contains("mutation_exemption:")
                {
                    let card_id = args
                        .windows(2)
                        .find_map(|pair| (pair[0] == "--card-id").then_some(pair[1]))
                        .expect("approved review fixture must name a card");
                    let verdict: serde_yaml_ng::Value =
                        serde_yaml_ng::from_str(&body).expect("valid fixture verdict");
                    let reviewer = verdict["reviewer_actor_id"]
                        .as_str()
                        .expect("approved fixture reviewer");
                    let principal = verdict["reviewer_provenance"]["principal_id"]
                        .as_str()
                        .expect("approved fixture reviewer principal");
                    let session = verdict["reviewer_provenance"]["session_id"]
                        .as_str()
                        .expect("approved fixture reviewer session");
                    let receipt =
                        self.create_fixture_mutation_receipt(card_id, reviewer, principal, session);
                    body.push_str("mutation_receipt_ids: [");
                    body.push_str(&receipt);
                    body.push_str("]\n");
                }
                fs::write(&verdict_path, body).unwrap();
            }
        }
        let output = self.review_raw(&normalized);
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
        if args.first() == Some(&"exception") {
            let mut full = vec![
                "integration".to_owned(),
                "exception".to_owned(),
                "--output".to_owned(),
                "json".to_owned(),
                args[1].to_owned(),
                "--control".to_owned(),
                self.control.display().to_string(),
            ];
            full.extend(args[2..].iter().map(|arg| (*arg).to_owned()));
            return Self::run(&full);
        }
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
        if args.first() == Some(&"prepare") {
            // Keep legacy fixture setup explicit while preserving raw-command
            // tests that assert a planless cycle is refused.
            let cycle_id = args
                .windows(2)
                .find_map(|pair| (pair[0] == "--cycle-id").then_some(pair[1]))
                .unwrap_or("C-001");
            self.ensure_default_cycle_plan_for(cycle_id);
        }
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
            base = self.cycle_baseline(),
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
            base = self.cycle_baseline(),
            inc = list(include),
            deps = list(depends_on),
        );
        let path = self.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        self.card(&["create", "--draft", &path.display().to_string()]);
        self.card(&["activate", "--card-id", card_id]);
    }

    /// Activates a dependent card branched at an exact commit.
    ///
    /// Section 10.2: a dependent uses the accepted dependency SHA. The default
    /// `activate_card_depending_on` branches from the cycle baseline instead,
    /// which is a declared dependency the candidate does not incorporate — a
    /// different case, and both are exercised.
    pub fn activate_card_depending_on_at(
        &self,
        card_id: &str,
        include: &[&str],
        depends_on: &[&str],
        base: &str,
    ) {
        let list = |values: &[&str]| {
            values
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let body = format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id}\nnon_goals: []\nrisk: low\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{inc}]\n  exclude: []\ndepends_on: [{deps}]\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\n",
            inc = list(include),
            deps = list(depends_on),
        );
        let path = self.root.join(format!("{card_id}.yaml"));
        fs::write(&path, body).unwrap();
        self.card(&["create", "--draft", &path.display().to_string()]);
        self.card(&["activate", "--card-id", card_id]);
    }

    /// Re-works an approved card and approves it again at a new commit.
    ///
    /// `rewrite` chooses how: `true` amends the tip, so the previous candidate
    /// is no longer in the branch's history; `false` commits on top, so it
    /// still is. The distinction is the whole subject of the dependency
    /// binding check, so it is a parameter rather than two near-identical
    /// helpers.
    pub fn rework_and_reapprove(&self, card_id: &str, file: &str, rewrite: bool) -> String {
        self.handoff(&["revoke", "--card-id", card_id, "--reason", "rework"]);
        let worktree = self.worktrees.join(card_id);
        let path = worktree.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "// reworked\n").unwrap();
        git(&worktree, &["add", "-A"]);
        if rewrite {
            git(&worktree, &["commit", "-q", "--amend", "-m", "rework"]);
        } else {
            git(&worktree, &["commit", "-q", "-m", "rework"]);
        }
        self.gate(&["run", "--card-id", card_id, "--gate-id", "gate.unit"]);

        let head = capture(&worktree, &["rev-parse", "HEAD"]);
        let declaration = self.root.join(format!("{card_id}-rework.yaml"));
        fs::write(
            &declaration,
            format!(
                "delivered_sha: {head}\nbehavior_delivered: reworked\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
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
        let receipt = self.create_fixture_mutation_receipt(
            card_id,
            "reviewer-session",
            "reviewer-principal",
            "reviewer-session",
        );
        let verdict = self.root.join(format!("{card_id}-rework-verdict.yaml"));
        fs::write(
            &verdict,
            format!("reviewer_actor_id: reviewer-session\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: demonstrated\n    mutation: added the fixture mutation marker\n    failing_test: fixture oracle rejects the marker\n    oracle: gate.fixture-mutation\n    authorship: reviewer_devised\nresidual_risks: []\nreview_conduct: separate_process\nmutation_receipt_ids: [{receipt}]\n"),
        )
        .unwrap();
        // #120: `--actor` must agree with the verdict's `reviewer_actor_id`.
        self.review(&[
            "record",
            "--card-id",
            card_id,
            "--verdict",
            &verdict.display().to_string(),
            "--actor",
            "reviewer-session",
        ]);
        head
    }

    /// Carries an activated card all the way to `approved`.
    ///
    /// Work, gate, handoff, and an approval by a reviewer distinct from the
    /// feature actor — the whole pre-integration path, which every integration
    /// test needs and none of them is testing.
    pub fn approve_card(&self, card_id: &str, file: &str) {
        self.approve_card_with_fixture_evidence(card_id, file, false);
    }

    /// Approves through the explicitly installed exemption policy. Callers
    /// must install that policy themselves so the authorization is visible in
    /// the owning test.
    pub fn approve_card_with_fixture_mutation_exemption(&self, card_id: &str, file: &str) {
        self.approve_card_with_fixture_evidence(card_id, file, true);
    }

    fn approve_card_with_fixture_evidence(&self, card_id: &str, file: &str, exempt: bool) {
        // The fixture may add cards after an earlier plan was pinned. Publish
        // an explicit revised complete plan before lifecycle work starts.
        self.ensure_default_cycle_plan_for("C-001");
        self.work(&[
            "start",
            "--card-id",
            card_id,
            "--actor-principal-id",
            "implementer-principal",
            "--actor-session-id",
            "implementer-session",
        ]);

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
        let evidence = if exempt {
            "gate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: fixture exemption authorization\n  mutation_evidence:\n    status: exempt\n    reason: this test explicitly exercises exemption-backed approval\nresidual_risks: []\nreview_conduct: separate_process\nmutation_exemption:\n  code: fixture-no-mutation\n  reason: this test explicitly exercises exemption-backed approval\n  approved_by: independent-attestor\n".to_owned()
        } else {
            let receipt = self.create_fixture_mutation_receipt(
                card_id,
                "reviewer-session",
                "reviewer-principal",
                "reviewer-session",
            );
            format!(
                "gate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probed each acceptance behavior directly\n  mutation_evidence:\n    status: demonstrated\n    mutation: added the fixture mutation marker\n    failing_test: fixture oracle rejects the marker\n    oracle: gate.fixture-mutation\n    authorship: reviewer_devised\nresidual_risks: []\nreview_conduct: separate_process\nmutation_receipt_ids: [{receipt}]\n"
            )
        };
        let verdict = self.root.join(format!("{card_id}-verdict.yaml"));
        fs::write(
            &verdict,
            format!(
                "reviewer_actor_id: reviewer-session\ndecision: approved\nfindings: []\n{evidence}"
            ),
        )
        .unwrap();
        // #120: `--actor` must agree with the verdict's `reviewer_actor_id`.
        self.review(&[
            "record",
            "--card-id",
            card_id,
            "--verdict",
            &verdict.display().to_string(),
            "--actor",
            "reviewer-session",
        ]);
        self.ensure_default_cycle_plan_for("C-001");
    }

    /// Gives legacy lifecycle fixtures an explicit, real plan before they
    /// enter integration. This is test plumbing, not a production bypass:
    /// the command under test still refuses a cycle that has no plan.
    #[allow(clippy::too_many_lines)]
    fn ensure_default_cycle_plan_for(&self, cycle_id: &str) {
        let cycle_record: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(self.control.join(format!("cycles/{cycle_id}.json"))).unwrap(),
        )
        .unwrap();
        if cycle_record["plan_migration_provenance"].is_string() {
            return;
        }
        let cycle = self.cycle_json(&["status", "--cycle-id", cycle_id]);
        let card_ids = cycle["data"]["card_ids"]
            .as_array()
            .expect("cycle status exposes card membership");
        let current_plan_covers_cards = cycle_record["plan_id"]
            .as_str()
            .and_then(|plan_id| {
                let path = self.control.join(format!("plans/{plan_id}.json"));
                fs::read_to_string(path).ok()
            })
            .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok())
            .and_then(|plan| plan["cards"].as_array().cloned())
            .is_some_and(|planned_cards| {
                let planned_ids = planned_cards
                    .iter()
                    .filter_map(|card| card["card_id"].as_str())
                    .collect::<std::collections::BTreeSet<_>>();
                let cycle_ids = card_ids
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<std::collections::BTreeSet<_>>();
                planned_ids == cycle_ids
                    && planned_cards.iter().all(|planned| {
                        let Some(card_id) = planned["card_id"].as_str() else {
                            return false;
                        };
                        let card_path = self.control.join(format!("cards/{card_id}/r1.json"));
                        let Ok(raw) = fs::read(card_path) else {
                            return false;
                        };
                        let Ok(card) = serde_json::from_slice::<serde_json::Value>(&raw) else {
                            return false;
                        };
                        planned["card_revision"] == card["revision"]
                    })
            });
        if current_plan_covers_cards {
            return;
        }
        assert!(
            cycle_record["plan_id"].is_null(),
            "fixture attempted to rewrite an authoritative pinned plan; create all cards and revisions before the first governed plan command"
        );
        let plan_id = (1..10_000)
            .map(|revision| format!("PLAN-TEST-{revision:03}"))
            .find(|candidate| {
                !self
                    .control
                    .join(format!("plans/{candidate}.json"))
                    .exists()
            })
            .expect("a fixture plan id should be available");
        let cards = card_ids
            .iter()
            .map(|id| {
                let card_id = id.as_str().unwrap();
                let card: serde_json::Value = serde_json::from_str(
                    &fs::read_to_string(self.control.join(format!("cards/{card_id}/r1.json")))
                        .unwrap(),
                )
                .unwrap();
                let proof_entries = card["proof_map"]["entries"]
                    .as_array()
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
                            .collect::<Vec<_>>()
                    })
                    .filter(|entries| !entries.is_empty())
                    .unwrap_or_else(|| vec!["fixture-proof".to_owned()]);
                serde_json::json!({
                    "card_id": card_id,
                    "card_revision": card["revision"],
                    "scope": card["write_scope"]["include"],
                    "scope_exclude": card["write_scope"]["exclude"],
                    "depends_on": card["depends_on"],
                    "proof_entries": proof_entries,
                    "mutation_plan": ["fixture mutation"],
                    "risk": card["risk"],
                    "reviewer_requirements": ["independent"],
                    "assignment": "operator",
                    "assignment_principal_id": "implementer-principal",
                    "assignment_session_id": "implementer-session",
                    "distribution": "parallel",
                    "acceptance_behaviors": card["acceptance"]["behaviors"],
                })
            })
            .collect::<Vec<_>>();
        let plan = serde_json::json!({
            "schema": "harness.cycle-plan/v1",
            "plan_id": &plan_id,
            "cycle_id": cycle_id,
            "objective": "fixture distribution",
            "cards": cards,
        });
        let path = self.root.join(format!("{plan_id}.json"));
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&plan).unwrap()),
        )
        .unwrap();
        self.cycle(&[
            "plan",
            "--plan-id",
            &plan_id,
            "--file",
            &path.display().to_string(),
        ]);
        fs::remove_file(path).expect("the disposable plan input should be removable");
    }

    /// Binds a complete fixture plan with one explicit execution class.
    pub fn bind_fixture_plan(&self, plan_id: &str, distribution: &str) {
        self.bind_fixture_plan_with_assignment(plan_id, distribution, "operator");
    }

    /// Binds a complete fixture plan for a caller-declared implementer.
    pub fn bind_fixture_plan_with_assignment(
        &self,
        plan_id: &str,
        distribution: &str,
        assignment: &str,
    ) {
        let card_count = self.cycle_json(&["status", "--cycle-id", "C-001"])["data"]["card_ids"]
            .as_array()
            .unwrap()
            .len();
        let distributions = vec![distribution; card_count];
        self.bind_fixture_plan_with_distributions(plan_id, &distributions, assignment);
    }

    /// Binds a complete fixture plan with one execution class per card.
    pub fn bind_fixture_plan_with_distributions(
        &self,
        plan_id: &str,
        distributions: &[&str],
        assignment: &str,
    ) {
        let cycle_record: serde_json::Value =
            serde_json::from_slice(&fs::read(self.control.join("cycles/C-001.json")).unwrap())
                .unwrap();
        let bound_plan_id = cycle_record["plan_id"]
            .as_str()
            .map_or_else(|| plan_id.to_owned(), str::to_owned);
        let cycle = self.cycle_json(&["status", "--cycle-id", "C-001"]);
        let card_ids = cycle["data"]["card_ids"].as_array().unwrap();
        let cards = card_ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                let card_id = id.as_str().unwrap();
                let card: serde_json::Value = serde_json::from_slice(
                    &fs::read(self.control.join(format!("cards/{card_id}/r1.json"))).unwrap(),
                )
                .unwrap();
                let proof_entries = card["proof_map"]["entries"]
                    .as_array()
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
                            .collect::<Vec<_>>()
                    })
                    .filter(|entries| !entries.is_empty())
                    .unwrap_or_else(|| vec!["fixture-proof".to_owned()]);
                serde_json::json!({
                    "card_id": card_id,
                    "card_revision": card["revision"],
                    "scope": card["write_scope"]["include"],
                    "scope_exclude": card["write_scope"]["exclude"],
                    "depends_on": card["depends_on"],
                    "proof_entries": proof_entries,
                    "mutation_plan": ["fixture mutation"],
                    "risk": card["risk"],
                    "reviewer_requirements": ["independent"],
                    "assignment": assignment,
                    "assignment_principal_id": "implementer-principal",
                    "assignment_session_id": "implementer-session",
                    "distribution": distributions
                        .get(index)
                        .copied()
                        .unwrap_or("parallel"),
                    "acceptance_behaviors": card["acceptance"]["behaviors"],
                })
            })
            .collect::<Vec<_>>();
        let plan = serde_json::json!({
            "schema": "harness.cycle-plan/v1",
            "plan_id": bound_plan_id,
            "cycle_id": "C-001",
            "objective": "fixture distribution",
            "cards": cards,
        });
        if cycle_record["plan_id"].is_string() {
            let stored_plan = self.control.join(format!("plans/{bound_plan_id}.json"));
            let existing: serde_json::Value =
                serde_json::from_slice(&fs::read(stored_plan).unwrap()).unwrap();
            assert_eq!(
                existing, plan,
                "fixture attempted to replace an authoritative pinned plan"
            );
            return;
        }
        let path = self.root.join(format!("{bound_plan_id}.json"));
        fs::write(&path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
        self.cycle(&[
            "plan",
            "--plan-id",
            &bound_plan_id,
            "--file",
            &path.display().to_string(),
        ]);
        fs::remove_file(path).unwrap();
    }

    /// Revises a card, moving its digest.
    pub fn revise_card(&self, card_id: &str, include: &[&str], reason: &str) {
        let list = include
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            "card_id: {card_id}\ncycle_id: C-001\ntitle: Implement {card_id}\ngoal: Deliver {card_id} differently\nnon_goals: []\nrisk: medium\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [{list}]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: []\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert the commit\nproof_map:\n  schema: harness.proof-map/v1\n  entries:\n    - id: proof-behavior\n      invariant: behavior remains correct\n      precondition: valid fixture\n      assertion: focused test passes\n      mutation: bypass assertion fails\n      gate_oracle: gate.review\n  claim_boundary: only this fixture\n",
            base = self.cycle_baseline(),
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

    /// Writes a convergence policy into the authoritative project document.
    ///
    /// `card_limit` is set across all four counted dimensions at all four
    /// risk levels, and `integration_limit` for the one cycle-level
    /// dimension — every convergence test needs only "a policy is
    /// configured", not any particular limit, so one value stands in for
    /// all four card dimensions. Committed with the existing `git()` helper
    /// so the control tree stays clean afterward. Unlike `tamper_card_state`
    /// and `tamper_cycle_status`, which leave the tree dirty on purpose to
    /// simulate an external edit, this simulates an operator setting project
    /// configuration through the normal, committed path.
    pub fn configure_convergence_policy(&self, card_limit: u32, integration_limit: u32) {
        let path = self.control.join("project/project.json");
        let raw = fs::read_to_string(&path).unwrap();
        let mut document: serde_json::Value = serde_json::from_str(&raw).unwrap();

        let card_limits = serde_json::json!({
            "review_returns": card_limit,
            "repair_attempts": card_limit,
            "gate_failures": card_limit,
            "material_scope_revisions": card_limit,
        });
        document["convergence_policy"] = serde_json::json!({
            "version": "harness.convergence-policy/v1",
            "card_limits": {
                "low": card_limits.clone(),
                "medium": card_limits.clone(),
                "high": card_limits.clone(),
                "critical": card_limits,
            },
            "cycle_limits": { "integration_failures": integration_limit },
        });

        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&document).unwrap()),
        )
        .unwrap();
        git(&self.control, &["add", "-A"]);
        git(
            &self.control,
            &["commit", "-q", "-m", "test: configure convergence policy"],
        );
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
/// Whether a Git object actually exists in a repository.
///
/// Not `git rev-parse`. Given a 40-character hex string, `rev-parse` echoes it
/// back and exits 0 whether or not the object is there — `--verify` too — so
/// `assert_eq!(capture(repo, &["rev-parse", sha]), sha)` is a tautology. Two
/// tests written to prove that commits survive garbage collection used exactly
/// that, which meant deleting the ref-retention mechanism they exist to defend
/// failed nothing.
///
/// `cat-file -e` is the check that answers the question.
#[must_use]
pub fn object_exists(repo: &Path, object: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-e", object])
        .output()
        .expect("git should run")
        .status
        .success()
}

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
