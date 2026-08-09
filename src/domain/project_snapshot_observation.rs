//! Ephemeral repository and lock observations for a snapshot.

use crate::{
    config::ProjectConfig,
    control::lock::LockDiagnosis,
    git::{
        authority::inspect_authority,
        command::{GitScope, run},
    },
};

pub(super) fn candidate_head(
    config: &ProjectConfig,
    diagnostics: &mut Vec<String>,
) -> Option<String> {
    let output = run(
        &GitScope::work_tree(&config.repository),
        ["rev-parse", "--verify", "HEAD"],
    )
    .ok()?;
    if output.success() {
        Some(output.trimmed_stdout().to_owned())
    } else {
        diagnostics.push("project_head_unavailable".to_owned());
        None
    }
}

pub(super) fn authority_head(
    config: &ProjectConfig,
    diagnostics: &mut Vec<String>,
) -> Option<String> {
    if let Ok(state) = inspect_authority(&config.authority_repository, &config.protected_branch) {
        state.protected_sha
    } else {
        diagnostics.push("authority_head_unavailable".to_owned());
        None
    }
}

pub(super) fn lock_state(diagnosis: &LockDiagnosis) -> String {
    match diagnosis {
        LockDiagnosis::Free => "free",
        LockDiagnosis::Held(_) => "held",
        LockDiagnosis::Stale { .. } => "stale",
        LockDiagnosis::Ambiguous { .. } => "ambiguous",
    }
    .to_owned()
}
