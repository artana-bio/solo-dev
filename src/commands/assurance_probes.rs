//! Executable disposable assurance probes.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use sha2::{Digest, Sha256};

use crate::{
    domain::assurance::{ProbeKind, ProbeResult},
    error::{ErrorCode, HarnessError},
};

use super::assurance_probe_result::{
    failed_probe, next_run_id, probe_command, probe_error_code, probe_expected,
};
use super::assurance_probe_scenarios::DisposableProbeProject;

///
/// # Errors
///
/// Individual setup or command failures are represented as failed probe
/// results; this returns an error only if result collection itself fails.
pub fn run_all() -> Result<Vec<ProbeResult>, HarnessError> {
    ProbeKind::ALL
        .into_iter()
        .map(|kind| match run_disposable_probe(kind) {
            Ok(result) => Ok(result),
            Err(error) => Ok(failed_probe(kind, &error)),
        })
        .collect()
}

#[allow(clippy::needless_pass_by_value)]
fn probe_require(exe: &Path, args: Vec<String>) -> Result<Output, HarnessError> {
    let output =
        Command::new(exe)
            .args(&args)
            .output()
            .map_err(|source| HarnessError::WorkspaceAccess {
                path: exe.to_owned(),
                source,
            })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(HarnessError::Control {
            reason: format!(
                "assurance setup {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stdout)
            ),
            code: ErrorCode::InternalControlCorrupt,
        })
    }
}

fn disposable_git(path: &Path, args: &[&str]) -> Result<(), HarnessError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|source| HarnessError::WorkspaceAccess {
            path: path.to_owned(),
            source,
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(HarnessError::Control {
            reason: format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            ),
            code: ErrorCode::InternalControlCorrupt,
        })
    }
}

fn git_text(path: &Path, args: &[&str]) -> Result<String, HarnessError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|source| HarnessError::WorkspaceAccess {
            path: path.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(HarnessError::Control {
            reason: format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            ),
            code: ErrorCode::InternalControlCorrupt,
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn hash_tree(path: &Path, digest: &mut Sha256, prefix: &str) -> Result<(), HarnessError> {
    let mut entries = fs::read_dir(path)
        .map_err(|source| HarnessError::WorkspaceAccess {
            path: path.to_owned(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| HarnessError::WorkspaceAccess {
            path: path.to_owned(),
            source,
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let entry_path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if matches!(
            name.as_str(),
            ".git" | "journal" | "logs" | "validation-executions" | "harness.lock"
        ) {
            continue;
        }
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let metadata = entry
            .metadata()
            .map_err(|source| HarnessError::WorkspaceAccess {
                path: entry_path.clone(),
                source,
            })?;
        digest.update(relative.as_bytes());
        if metadata.is_dir() {
            hash_tree(&entry_path, digest, &relative)?;
        } else {
            digest.update(fs::read(&entry_path).map_err(|source| {
                HarnessError::WorkspaceAccess {
                    path: entry_path,
                    source,
                }
            })?);
        }
    }
    Ok(())
}

fn state_snapshot(control: &Path, candidate: &Path) -> Result<String, HarnessError> {
    let mut digest = Sha256::new();
    hash_tree(control, &mut digest, "control")?;
    let mut candidate_digest = Sha256::new();
    hash_tree(candidate, &mut candidate_digest, "candidate")?;
    Ok(format!(
        "control_head={};control_status={};control_tree={:x};candidate_head={};candidate_status={};candidate_tree={:x}",
        git_text(control, &["rev-parse", "HEAD"])?,
        git_text(control, &["status", "--porcelain"])?,
        digest.finalize(),
        git_text(candidate, &["rev-parse", "HEAD"])?,
        git_text(candidate, &["status", "--porcelain"])?,
        candidate_digest.finalize(),
    ))
}

#[allow(clippy::too_many_lines)]
fn run_disposable_probe(
    kind: crate::domain::assurance::ProbeKind,
) -> Result<crate::domain::assurance::ProbeResult, HarnessError> {
    if kind == crate::domain::assurance::ProbeKind::DeniedNetwork {
        return Ok(crate::domain::assurance::ProbeResult {
            run_id: next_run_id(kind),
            probe_id: kind.name().to_owned(),
            probe: kind.name().to_owned(),
            oracle: "network policy".to_owned(),
            expected_error_code: Some("not_tested".to_owned()),
            observed_error_code: None,
            command_path: "gate.run".to_owned(),
            refused: false,
            classification: "not_tested".to_owned(),
            network_declared: Some("denied".to_owned()),
            network_enforced: Some(false),
            state_change_evidence: "not tested: no sandbox enforcement".to_owned(),
            cleanup_completed: true,
            detail: "network denial is declared but not host-enforced".to_owned(),
        });
    }
    let temp = tempfile::tempdir().map_err(|source| HarnessError::WorkspaceAccess {
        path: PathBuf::from("assurance-probe"),
        source,
    })?;
    let project = DisposableProbeProject::new(temp.path().to_path_buf());
    let root = project.root.clone();
    let repository = root.join("repository");
    let control = project.control.clone();
    let authority = root.join("authority.git");
    let worktree = project.worktree.clone();
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
    if init.status.success() {
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
            fs::write(&gate, format!("schema: harness.gate/v1\ngate_id: {gate_id}\npurpose: assurance probe\nsemantics: command must pass\n{reuse}migration: legacy_v1\nrevision: 1\nargv: {argv}\nworking_directory: .\ntimeout_seconds: 30\nenvironment:\n  allow: []\n  set: {{}}\nnetwork_policy: denied\nretry_policy:\n  max_attempts: 1\nartifacts: []\n"))
                .map_err(|source| HarnessError::WorkspaceAccess { path: gate.clone(), source })?;
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
        let base = String::from_utf8_lossy(
            &Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&repository)
                .output()
                .map_err(|source| HarnessError::WorkspaceAccess {
                    path: repository.clone(),
                    source,
                })?
                .stdout,
        )
        .trim()
        .to_owned();
        let draft = root.join("card.yaml");
        fs::write(&draft, format!("card_id: F-001\ncycle_id: C-001\ntitle: Assurance\ngoal: Probe\nnon_goals: []\nrisk: high\nchange_kind: feature\nbase_sha: {base}\nwrite_scope:\n  include: [src/F-001/**]\n  exclude: []\nnamed_gates:\n  feature: [gate.unit]\n  review: [gate.review]\n  integration: [gate.all]\nacceptance:\n  behaviors: [it works]\n  regressions: []\nreview_policy: independent\nrollback_strategy: revert\nproof_map:\n  schema: harness.proof-map/v1\n  entries:\n    - id: proof-behavior\n      invariant: it works\n      precondition: fixture\n      assertion: gate passes\n      mutation: gate fails\n      gate_oracle: gate.unit\n  claim_boundary: fixture\n"))
            .map_err(|source| HarnessError::WorkspaceAccess { path: draft.clone(), source })?;
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
            .map_err(|source| HarnessError::WorkspaceAccess { path: plan.clone(), source })?;
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
            path: probe_file.clone(),
            source,
        })?;
        disposable_git(&worktree, &["add", "-A"])?;
        disposable_git(&worktree, &["commit", "-q", "-m", "probe candidate"])?;
        let candidate = String::from_utf8_lossy(
            &Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&worktree)
                .output()
                .map_err(|source| HarnessError::WorkspaceAccess {
                    path: worktree.clone(),
                    source,
                })?
                .stdout,
        )
        .trim()
        .to_owned();
        let candidate_case =
            DisposableProbeProject::prepare_candidate_case(kind, &base, &candidate);
        let reservation = probe_require(
            &exe,
            vec![
                "gate".into(),
                "reserve".into(),
                "--control".into(),
                control.display().to_string(),
                "--card-id".into(),
                "F-001".into(),
                "--gate-id".into(),
                "gate.unit".into(),
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
                reason: "assurance gate reserve omitted reservation_id".to_owned(),
                code: ErrorCode::InternalControlCorrupt,
            })?;
        probe_require(
            &exe,
            vec![
                "gate".into(),
                "run".into(),
                "--control".into(),
                control.display().to_string(),
                "--card-id".into(),
                "F-001".into(),
                "--gate-id".into(),
                "gate.unit".into(),
                "--reservation-id".into(),
                reservation_id,
                "--output".into(),
                "json".into(),
            ],
        )?;
        let review_reservation = probe_require(
            &exe,
            vec![
                "gate".into(),
                "reserve".into(),
                "--control".into(),
                control.display().to_string(),
                "--card-id".into(),
                "F-001".into(),
                "--gate-id".into(),
                "gate.review".into(),
                "--output".into(),
                "json".into(),
            ],
        )?;
        let review_reservation_id =
            serde_json::from_slice::<serde_json::Value>(&review_reservation.stdout)
                .ok()
                .and_then(|value| {
                    value["data"]["reservation"]["reservation_id"]
                        .as_str()
                        .map(ToOwned::to_owned)
                })
                .ok_or_else(|| HarnessError::Control {
                    reason: "assurance review gate reserve omitted reservation_id".to_owned(),
                    code: ErrorCode::InternalControlCorrupt,
                })?;
        probe_require(
            &exe,
            vec![
                "gate".into(),
                "run".into(),
                "--control".into(),
                control.display().to_string(),
                "--card-id".into(),
                "F-001".into(),
                "--gate-id".into(),
                "gate.review".into(),
                "--reservation-id".into(),
                review_reservation_id,
                "--output".into(),
                "json".into(),
            ],
        )?;
        let declaration = root.join("declaration.yaml");
        let delivered = candidate_case.delivered_sha;
        fs::write(&declaration, format!("delivered_sha: {delivered}\nbehavior_delivered: probe\nimplementation_decisions: [probe]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"))
            .map_err(|source| HarnessError::WorkspaceAccess { path: declaration.clone(), source })?;
        if !matches!(kind, ProbeKind::OutOfScopeWrite | ProbeKind::StaleSha) {
            probe_require(
                &exe,
                vec![
                    "handoff".into(),
                    "create".into(),
                    "--control".into(),
                    control.display().to_string(),
                    "--card-id".into(),
                    "F-001".into(),
                    "--declaration".into(),
                    declaration.display().to_string(),
                    "--output".into(),
                    "json".into(),
                ],
            )?;
            let review_case = DisposableProbeProject::prepare_review_case(kind);
            let begin = vec![
                "review".into(),
                "begin".into(),
                "--control".into(),
                control.display().to_string(),
                "--card-id".into(),
                "F-001".into(),
                "--actor".into(),
                review_case.actor.into(),
                "--actor-principal-id".into(),
                review_case.principal.into(),
                "--actor-session-id".into(),
                review_case.session.into(),
                "--output".into(),
                "json".into(),
            ];
            if matches!(
                kind,
                ProbeKind::MissingMutationReceipt | ProbeKind::MissingHumanAttestation
            ) {
                probe_require(&exe, begin)?;
            }
            if matches!(
                kind,
                ProbeKind::SelfReview
                    | ProbeKind::SameSessionReview
                    | ProbeKind::MissingMutationReceipt
                    | ProbeKind::MissingHumanAttestation
            ) {
                let verdict = root.join("verdict.yaml");
                let actor = if kind == ProbeKind::SelfReview {
                    "operator"
                } else {
                    "reviewer"
                };
                let provenance = if kind == ProbeKind::SameSessionReview {
                    "session-implementer"
                } else {
                    "reviewer-session"
                };
                let kind_line = if kind == ProbeKind::MissingHumanAttestation {
                    "reviewer_kind: human\n"
                } else {
                    "reviewer_kind: agent\n"
                };
                let mutation = if matches!(
                    kind,
                    ProbeKind::SelfReview | ProbeKind::SameSessionReview
                ) {
                    "mutation_exemption:\n  code: probe-no-mutation\n  reason: valid probe exemption\n  approved_by: independent-attestor\n"
                } else {
                    ""
                };
                let mutation_evidence = if matches!(
                    kind,
                    ProbeKind::MissingMutationReceipt | ProbeKind::MissingHumanAttestation
                ) {
                    "  mutation_evidence:\n    status: exempt\n    reason: probe deliberately omits the typed receipt or exemption\n"
                } else {
                    ""
                };
                fs::write(&verdict, format!("reviewer_actor_id: {actor}\n{kind_line}reviewer_provenance:\n  provider: probe\n  model: probe\n  session_id: {provenance}\n  principal_id: reviewer-principal\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probe\n{mutation_evidence}residual_risks: []\nreview_conduct: separate_process\n{mutation}"))
                    .map_err(|source| HarnessError::WorkspaceAccess { path: verdict.clone(), source })?;
            }
        }
    }
    let before = state_snapshot(&control, &worktree)?;
    let args: Vec<String> = match kind {
        crate::domain::assurance::ProbeKind::OutOfScopeWrite
        | crate::domain::assurance::ProbeKind::StaleSha => vec![
            "handoff".into(),
            "create".into(),
            "--control".into(),
            control.display().to_string(),
            "--card-id".into(),
            "F-001".into(),
            "--declaration".into(),
            root.join("declaration.yaml").display().to_string(),
            "--output".into(),
            "json".into(),
        ],
        ProbeKind::SelfReview | ProbeKind::SameSessionReview => vec![
            "review".into(),
            "begin".into(),
            "--control".into(),
            control.display().to_string(),
            "--card-id".into(),
            "F-001".into(),
            "--actor".into(),
            if kind == ProbeKind::SelfReview {
                "operator".into()
            } else {
                "reviewer".into()
            },
            "--actor-principal-id".into(),
            if kind == ProbeKind::SelfReview {
                "implementer-principal".into()
            } else {
                "reviewer-principal".into()
            },
            "--actor-session-id".into(),
            "session-implementer".into(),
            "--output".into(),
            "json".into(),
        ],
        _ => vec![
            "review".into(),
            "record".into(),
            "--control".into(),
            control.display().to_string(),
            "--card-id".into(),
            "F-001".into(),
            "--verdict".into(),
            root.join("verdict.yaml").display().to_string(),
            "--actor".into(),
            "reviewer".into(),
            "--output".into(),
            "json".into(),
        ],
    };
    let output = if init.status.success() {
        Command::new(&exe)
            .args(&args)
            .output()
            .map_err(|source| HarnessError::WorkspaceAccess {
                path: root.clone(),
                source,
            })?
    } else {
        init
    };
    let after = state_snapshot(&control, &worktree)?;
    let observed = probe_error_code(&output);
    let refused = observed.as_deref() == Some(probe_expected(kind));
    let cleanup_path = root.clone();
    drop(temp);
    let cleanup_completed = !cleanup_path.exists();
    let side_effect_free = before == after;
    let classification = if refused && side_effect_free && cleanup_completed {
        "executed_passed"
    } else {
        "executed_failed"
    };
    Ok(crate::domain::assurance::ProbeResult {
        run_id: next_run_id(kind),
        probe_id: kind.name().to_owned(),
        probe: kind.name().to_owned(),
        oracle: probe_command(kind).to_owned(),
        expected_error_code: Some(probe_expected(kind).to_owned()),
        observed_error_code: observed,
        command_path: probe_command(kind).to_owned(),
        refused,
        classification: classification.to_owned(),
        network_declared: None,
        network_enforced: None,
        state_change_evidence: format!(
            "governed_state_before={before};governed_state_after={after};unchanged={side_effect_free}"
        ),
        cleanup_completed,
        detail: if output.status.success() {
            "command unexpectedly succeeded".to_owned()
        } else {
            "disposable command executed".to_owned()
        },
    })
}
