//! Executable disposable assurance probes.

use std::{
    fs,
    path::Path,
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

/// Runs every disposable probe independently.
///
/// # Errors
///
/// Returns an error only when the result collection itself cannot be built.
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
pub(crate) fn probe_require(exe: &Path, args: Vec<String>) -> Result<Output, HarnessError> {
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

pub(crate) fn disposable_git(path: &Path, args: &[&str]) -> Result<(), HarnessError> {
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

pub(crate) fn git_text(path: &Path, args: &[&str]) -> Result<String, HarnessError> {
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
    let mut control_digest = Sha256::new();
    hash_tree(control, &mut control_digest, "control")?;
    let mut candidate_digest = Sha256::new();
    hash_tree(candidate, &mut candidate_digest, "candidate")?;
    Ok(format!(
        "control_head={};control_status={};control_tree={:x};candidate_head={};candidate_status={};candidate_tree={:x}",
        git_text(control, &["rev-parse", "HEAD"])?,
        git_text(control, &["status", "--porcelain"])?,
        control_digest.finalize(),
        git_text(candidate, &["rev-parse", "HEAD"])?,
        git_text(candidate, &["status", "--porcelain"])?,
        candidate_digest.finalize(),
    ))
}

fn run_disposable_probe(kind: ProbeKind) -> Result<ProbeResult, HarnessError> {
    if kind == ProbeKind::DeniedNetwork {
        return Ok(ProbeResult {
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
    let prepared = super::assurance_probe_fixture::prepare_disposable_project(kind)?;
    execute_prepared_probe(prepared, kind)
}

fn execute_prepared_probe(
    prepared: super::assurance_probe_fixture::PreparedProbe,
    kind: ProbeKind,
) -> Result<ProbeResult, HarnessError> {
    let root = prepared.project.root.clone();
    let control = prepared.project.control.clone();
    let worktree = prepared.project.worktree.clone();
    let candidate_case =
        DisposableProbeProject::prepare_candidate_case(kind, &prepared.base, &prepared.candidate);
    let declaration = root.join("declaration.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {}\nbehavior_delivered: probe\nimplementation_decisions: [probe]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n",
            candidate_case.delivered_sha
        ),
    )
    .map_err(|source| HarnessError::WorkspaceAccess {
        path: declaration.clone(),
        source,
    })?;
    if !matches!(kind, ProbeKind::OutOfScopeWrite | ProbeKind::StaleSha) {
        prepare_review_case(&prepared.exe, &control, &root, &declaration, kind)?;
    }
    let before = state_snapshot(&control, &worktree)?;
    let args = measured_args(&control, &root, &declaration, kind);
    let output = Command::new(&prepared.exe)
        .args(&args)
        .output()
        .map_err(|source| HarnessError::WorkspaceAccess {
            path: root.clone(),
            source,
        })?;
    let after = state_snapshot(&control, &worktree)?;
    let observed = probe_error_code(&output);
    let refused = observed.as_deref() == Some(probe_expected(kind));
    let cleanup_path = root.clone();
    drop(prepared.temp);
    let cleanup_completed = !cleanup_path.exists();
    let unchanged = before == after;
    let classification = if refused && unchanged && cleanup_completed {
        "executed_passed"
    } else {
        "executed_failed"
    };
    Ok(ProbeResult {
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
            "governed_state_before={before};governed_state_after={after};unchanged={unchanged}"
        ),
        cleanup_completed,
        detail: if output.status.success() {
            "command unexpectedly succeeded".to_owned()
        } else {
            "disposable command executed".to_owned()
        },
    })
}

fn prepare_review_case(
    exe: &Path,
    control: &Path,
    root: &Path,
    declaration: &Path,
    kind: ProbeKind,
) -> Result<(), HarnessError> {
    let review_case = DisposableProbeProject::prepare_review_case(kind);
    probe_require(
        exe,
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
    if !matches!(
        kind,
        ProbeKind::MissingMutationReceipt | ProbeKind::MissingHumanAttestation
    ) {
        return Ok(());
    }
    probe_require(
        exe,
        vec![
            "review".into(),
            "begin".into(),
            "--control".into(),
            control.display().to_string(),
            "--card-id".into(),
            "F-001".into(),
            "--actor".into(),
            review_case.actor.to_owned(),
            "--actor-principal-id".into(),
            review_case.principal.to_owned(),
            "--actor-session-id".into(),
            review_case.session.to_owned(),
            "--output".into(),
            "json".into(),
        ],
    )?;
    let reviewer_kind = if kind == ProbeKind::MissingHumanAttestation {
        "human"
    } else {
        "agent"
    };
    let verdict = root.join("verdict.yaml");
    fs::write(
        &verdict,
        format!(
            "reviewer_actor_id: reviewer\nreviewer_kind: {reviewer_kind}\nreviewer_provenance:\n  provider: probe\n  model: probe\n  session_id: reviewer-session\n  principal_id: reviewer-principal\ndecision: approved\nfindings: []\ngate_adequacy:\n  gates_observe_acceptance: true\n  unobserved_behaviors: []\n  basis: probe\n  mutation_evidence:\n    status: exempt\n    reason: probe deliberately omits the typed receipt or exemption\nresidual_risks: []\nreview_conduct: separate_process\n"
        ),
    )
    .map_err(|source| HarnessError::WorkspaceAccess {
        path: verdict,
        source,
    })
}

fn measured_args(control: &Path, root: &Path, declaration: &Path, kind: ProbeKind) -> Vec<String> {
    match kind {
        ProbeKind::OutOfScopeWrite | ProbeKind::StaleSha => vec![
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
        ProbeKind::SelfReview | ProbeKind::SameSessionReview => {
            let case = DisposableProbeProject::prepare_review_case(kind);
            vec![
                "review".into(),
                "begin".into(),
                "--control".into(),
                control.display().to_string(),
                "--card-id".into(),
                "F-001".into(),
                "--actor".into(),
                case.actor.to_owned(),
                "--actor-principal-id".into(),
                case.principal.to_owned(),
                "--actor-session-id".into(),
                case.session.to_owned(),
                "--output".into(),
                "json".into(),
            ]
        }
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
    }
}
