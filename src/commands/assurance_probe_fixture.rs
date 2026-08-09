//! Disposable lifecycle fixtures used by executable assurance probes.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    domain::assurance::ProbeKind,
    error::{ErrorCode, HarnessError},
};

use super::{
    assurance_probe_scenarios::DisposableProbeProject,
    assurance_probes::{disposable_git, git_text, probe_require},
};

pub(crate) struct PreparedProbe {
    pub(super) temp: tempfile::TempDir,
    pub(super) project: DisposableProbeProject,
    pub(super) exe: PathBuf,
    pub(super) base: String,
    pub(super) candidate: String,
}

/// Creates the complete governed lifecycle before a probe-specific condition
/// is introduced. Every command is the production CLI executable.
#[allow(clippy::too_many_lines)]
pub(crate) fn prepare_disposable_project(kind: ProbeKind) -> Result<PreparedProbe, HarnessError> {
    let temp = tempfile::tempdir().map_err(|source| HarnessError::WorkspaceAccess {
        path: PathBuf::from("assurance-probe"),
        source,
    })?;
    let project = DisposableProbeProject::new(temp.path().to_path_buf());
    let root = project.root.clone();
    let repository = root.join("repository");
    fs::create_dir_all(&repository).map_err(|source| HarnessError::WorkspaceAccess {
        path: repository.clone(),
        source,
    })?;
    disposable_git(&repository, &["init", "-q", "-b", "main"])?;
    disposable_git(
        &repository,
        &["config", "user.email", "assurance@local.invalid"],
    )?;
    disposable_git(&repository, &["config", "user.name", "Assurance Probe"])?;
    fs::write(repository.join("README.md"), "assurance probe\n").map_err(|source| {
        HarnessError::WorkspaceAccess {
            path: repository.join("README.md"),
            source,
        }
    })?;
    disposable_git(&repository, &["add", "-A"])?;
    disposable_git(&repository, &["commit", "-q", "-m", "probe baseline"])?;
    let exe = std::env::current_exe().map_err(|source| HarnessError::WorkspaceAccess {
        path: root.clone(),
        source,
    })?;
    let control = project.control.clone();
    let authority = root.join("authority.git");
    let init = Command::new(&exe)
        .args([
            "project",
            "init",
            "--project-id",
            "assurance",
            "--repository",
        ])
        .arg(&repository)
        .args(["--control"])
        .arg(&control)
        .args(["--authority"])
        .arg(&authority)
        .args(["--worktree-root"])
        .arg(root.join("assurance-worktrees"))
        .args(["--output", "json"])
        .output()
        .map_err(|source| HarnessError::WorkspaceAccess {
            path: root.clone(),
            source,
        })?;
    if !init.status.success() {
        return Err(HarnessError::Control {
            reason: format!(
                "assurance setup {:?} failed: {}",
                ["project", "init"],
                String::from_utf8_lossy(&init.stdout)
            ),
            code: ErrorCode::InternalControlCorrupt,
        });
    }
    for gate_id in ["gate.unit", "gate.review", "gate.all"] {
        let gate = root.join(format!("{gate_id}.yaml"));
        let reuse = if gate_id == "gate.unit" {
            ""
        } else {
            "reuse_justification: assurance integration oracle reuses the same executable command\n"
        };
        let argv = if gate_id == "gate.review" {
            "[\"true\", \"review\"]"
        } else {
            "[\"true\"]"
        };
        fs::write(
            &gate,
            format!(
                "schema: harness.gate/v1\ngate_id: {gate_id}\npurpose: assurance probe\nsemantics: command must pass\n{reuse}migration: legacy_v1\nrevision: 1\nargv: {argv}\nworking_directory: .\ntimeout_seconds: 30\nenvironment:\n  allow: []\n  set: {{}}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"
            ),
        )
        .map_err(|source| HarnessError::WorkspaceAccess {
            path: gate.clone(),
            source,
        })?;
        probe_require(
            &exe,
            vec![
                "gate".into(),
                "register".into(),
                "--control".into(),
                control.display().to_string(),
                "--definition".into(),
                gate.display().to_string(),
                "--output".into(),
                "json".into(),
            ],
        )?;
    }
    probe_require(
        &exe,
        vec![
            "cycle".into(),
            "create".into(),
            "--control".into(),
            control.display().to_string(),
            "--cycle-id".into(),
            "C-001".into(),
            "--objective".into(),
            "assurance".into(),
            "--output".into(),
            "json".into(),
        ],
    )?;
    probe_require(
        &exe,
        vec![
            "cycle".into(),
            "activate".into(),
            "--control".into(),
            control.display().to_string(),
            "--cycle-id".into(),
            "C-001".into(),
            "--output".into(),
            "json".into(),
        ],
    )?;
    let base = git_text(&repository, &["rev-parse", "HEAD"])?;
    let draft = root.join("card.yaml");
    fs::write(
        &draft,
        format!(
            "card_id: F-001\ncycle_id: C-001\ntitle: Assurance\ngoal: Probe\nnon_goals: []\nrisk: high\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [src/F-001/**]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: [gate.review]\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert\nproof_map:\n  schema: harness.proof-map/v1\n  entries:\n    - id: proof-behavior\n      invariant: it works\n      precondition: fixture\n      assertion: gate passes\n      mutation: gate fails\n      gate_oracle: gate.unit\n  claim_boundary: fixture\n"
        ),
    )
    .map_err(|source| HarnessError::WorkspaceAccess {
        path: draft.clone(),
        source,
    })?;
    probe_require(
        &exe,
        vec![
            "card".into(),
            "create".into(),
            "--control".into(),
            control.display().to_string(),
            "--draft".into(),
            draft.display().to_string(),
            "--output".into(),
            "json".into(),
        ],
    )?;
    probe_require(
        &exe,
        vec![
            "card".into(),
            "activate".into(),
            "--control".into(),
            control.display().to_string(),
            "--card-id".into(),
            "F-001".into(),
            "--output".into(),
            "json".into(),
        ],
    )?;
    let plan = root.join("plan.json");
    fs::write(&plan, r#"{"schema":"harness.cycle-plan/v1","plan_id":"PLAN-ASSURANCE","cycle_id":"C-001","objective":"assurance","cards":[{"card_id":"F-001","card_revision":1,"scope":["src/F-001/**"],"scope_exclude":[],"depends_on":[],"proof_entries":["proof-behavior"],"mutation_plan":["gate fails"],"risk":"high","reviewer_requirements":["independent"],"assignment":"operator","assignment_principal_id":"implementer","assignment_session_id":"session-implementer","distribution":"parallel","acceptance_behaviors":["it works"]}]}"#)
        .map_err(|source| HarnessError::WorkspaceAccess {
            path: plan.clone(),
            source,
        })?;
    probe_require(
        &exe,
        vec![
            "cycle".into(),
            "plan".into(),
            "--control".into(),
            control.display().to_string(),
            "--plan-id".into(),
            "PLAN-ASSURANCE".into(),
            "--file".into(),
            plan.display().to_string(),
            "--output".into(),
            "json".into(),
        ],
    )?;
    probe_require(
        &exe,
        vec![
            "work".into(),
            "start".into(),
            "--control".into(),
            control.display().to_string(),
            "--card-id".into(),
            "F-001".into(),
            "--actor".into(),
            "operator".into(),
            "--actor-principal-id".into(),
            "implementer".into(),
            "--actor-session-id".into(),
            "session-implementer".into(),
            "--output".into(),
            "json".into(),
        ],
    )?;
    let probe_file = project.probe_file(kind);
    if let Some(parent) = probe_file.parent() {
        fs::create_dir_all(parent).map_err(|source| HarnessError::WorkspaceAccess {
            path: parent.to_owned(),
            source,
        })?;
    }
    fs::write(&probe_file, "probe\n").map_err(|source| HarnessError::WorkspaceAccess {
        path: probe_file,
        source,
    })?;
    disposable_git(&project.worktree, &["add", "-A"])?;
    disposable_git(
        &project.worktree,
        &["commit", "-q", "-m", "probe candidate"],
    )?;
    let candidate = git_text(&project.worktree, &["rev-parse", "HEAD"])?;
    run_gate(&exe, &control, "gate.unit")?;
    run_gate(&exe, &control, "gate.review")?;
    Ok(PreparedProbe {
        temp,
        project,
        exe,
        base,
        candidate,
    })
}

fn run_gate(exe: &Path, control: &Path, gate_id: &str) -> Result<(), HarnessError> {
    let reservation = probe_require(
        exe,
        vec![
            "gate".into(),
            "reserve".into(),
            "--control".into(),
            control.display().to_string(),
            "--card-id".into(),
            "F-001".into(),
            "--gate-id".into(),
            gate_id.to_owned(),
            "--output".into(),
            "json".into(),
        ],
    )?;
    let reservation_id = serde_json::from_slice::<serde_json::Value>(&reservation.stdout)
        .ok()
        .and_then(|value| {
            value["data"]["reservation"]["reservation_id"]
                .as_str()
                .map(ToOwned::to_owned)
        })
        .ok_or_else(|| HarnessError::Control {
            reason: format!("assurance gate reserve omitted reservation_id for {gate_id}"),
            code: ErrorCode::InternalControlCorrupt,
        })?;
    probe_require(
        exe,
        vec![
            "gate".into(),
            "run".into(),
            "--control".into(),
            control.display().to_string(),
            "--card-id".into(),
            "F-001".into(),
            "--gate-id".into(),
            gate_id.to_owned(),
            "--reservation-id".into(),
            reservation_id,
            "--output".into(),
            "json".into(),
        ],
    )?;
    Ok(())
}
