//! Project configuration validation.
//!
//! Every check here is read-only. Section 9.1 requires initialization to fail
//! without mutation, so validation must be able to reject a configuration
//! before anything is created.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    error::{ErrorCode, HarnessError},
    git::inspect::{self, GitVersion, RepositoryClass},
};

use super::{FieldError, ProjectConfig};

/// One configured path, with the symlink resolution that was observed.
///
/// Section 9.2 requires symlink resolution to be recorded, because a later
/// change in the resolved target must block mutations until the project is
/// revalidated. Recording only the configured path would hide that change.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct ResolvedPath {
    /// The field that supplied this path.
    pub field: String,
    /// The path exactly as configured.
    pub configured: PathBuf,
    /// The fully resolved path, with symlinks followed.
    pub resolved: PathBuf,
    /// True when resolution changed the path, meaning a symlink was traversed.
    pub via_symlink: bool,
}

/// The result of validating one project configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ValidationReport {
    /// Schema identifier for this report.
    pub schema: String,
    /// The project that was validated.
    pub project_id: String,
    /// Every configured path and its resolution.
    pub paths: Vec<ResolvedPath>,
    /// The installed Git version.
    pub git_version: String,
    /// The minimum the project requires.
    pub minimum_git_version: String,
    /// The protected branch, and the commit it resolves to.
    pub protected_branch: String,
    /// Exact commit the protected branch points at.
    pub protected_branch_sha: String,
    /// The host operating system.
    pub host_os: String,
}

/// Schema identifier for the validation report.
pub const VALIDATION_SCHEMA: &str = "harness.project-validation/v1";

/// Which paths are allowed to be absent.
///
/// `project init` creates the control repository, the bare authority, and the
/// worktree root, so requiring them to exist would make initialization
/// impossible. Every other command validates an established project, where
/// absence of control or authority is a real fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// Validating before initialization: control and worktree root may be absent.
    Initializing,
    /// Validating an established project: every configured path must exist.
    Established,
}

impl Mode {
    /// True when the named field is allowed to point at a path that does not
    /// exist yet.
    const fn allows_absent(self, field: &str) -> bool {
        match self {
            Self::Initializing => matches!(
                field.as_bytes(),
                b"control_repository" | b"authority_repository" | b"worktree_root"
            ),
            Self::Established => matches!(field.as_bytes(), b"worktree_root"),
        }
    }
}

/// Validates a project configuration without mutating anything.
///
/// Checks run in a deliberate order: cheap structural checks first, then
/// filesystem resolution, then Git queries. An operator fixing an absolute-path
/// mistake should not have to wait for a repository probe to discover it.
///
/// # Errors
///
/// Returns a configuration error naming the exact offending field.
pub fn validate(config: &ProjectConfig) -> Result<ValidationReport, HarnessError> {
    validate_in_mode(config, Mode::Established)
}

/// Validates a project configuration under an explicit mode.
///
/// # Errors
///
/// Returns a configuration error naming the exact offending field.
pub fn validate_in_mode(
    config: &ProjectConfig,
    mode: Mode,
) -> Result<ValidationReport, HarnessError> {
    validate_scalars(config)?;
    let paths = resolve_paths(config, mode)?;
    check_aliases(&paths)?;
    check_nesting(&paths)?;
    let (git_version, minimum) = check_git_version(config)?;
    check_host(config)?;
    let protected_branch_sha = check_repository_and_branch(config)?;

    Ok(ValidationReport {
        schema: VALIDATION_SCHEMA.to_owned(),
        project_id: config.project_id.to_string(),
        paths,
        git_version: git_version.to_string(),
        minimum_git_version: minimum.to_string(),
        protected_branch: config.protected_branch.clone(),
        protected_branch_sha,
        host_os: std::env::consts::OS.to_owned(),
    })
}

/// Checks values that need neither the filesystem nor Git.
fn validate_scalars(config: &ProjectConfig) -> Result<(), HarnessError> {
    if config.authority_remote.trim().is_empty() {
        return Err(FieldError::new(
            "authority_remote",
            "must not be empty",
            ErrorCode::ConfigInvalidValue,
        )
        .into_error());
    }
    if config.protected_branch.trim().is_empty() {
        return Err(FieldError::new(
            "protected_branch",
            "must not be empty",
            ErrorCode::ConfigInvalidValue,
        )
        .into_error());
    }
    if !matches!(config.default_output.as_str(), "text" | "json") {
        return Err(FieldError::new(
            "default_output",
            format!(
                "expected `text` or `json`, found `{}`",
                config.default_output
            ),
            ErrorCode::ConfigInvalidValue,
        )
        .into_error());
    }
    if config.host_policy.supported_os.is_empty() {
        return Err(FieldError::new(
            "host_policy.supported_os",
            "must list at least one operating system",
            ErrorCode::ConfigInvalidValue,
        )
        .into_error());
    }
    Ok(())
}

/// Requires every path to be absolute and existing, and records its resolution.
fn resolve_paths(config: &ProjectConfig, mode: Mode) -> Result<Vec<ResolvedPath>, HarnessError> {
    let mut resolved = Vec::new();
    for (field, path) in config.labeled_paths() {
        if !path.is_absolute() {
            return Err(FieldError::new(
                field,
                format!("expected an absolute path, found `{}`", path.display()),
                ErrorCode::ConfigPathNotAbsolute,
            )
            .into_error());
        }
        if !path.exists() {
            if !mode.allows_absent(field) {
                return Err(FieldError::new(
                    field,
                    format!("path does not exist: {}", path.display()),
                    ErrorCode::ConfigPathMissing,
                )
                .into_error());
            }
            resolved.push(ResolvedPath {
                field: field.to_owned(),
                configured: path.clone(),
                resolved: path.clone(),
                via_symlink: false,
            });
            continue;
        }
        let canonical = path.canonicalize().map_err(|source| {
            FieldError::new(
                field,
                format!("cannot resolve path: {source}"),
                ErrorCode::ConfigPathMissing,
            )
            .into_error()
        })?;
        resolved.push(ResolvedPath {
            field: field.to_owned(),
            configured: path.clone(),
            via_symlink: canonical != *path,
            resolved: canonical,
        });
    }
    Ok(resolved)
}

/// Rejects two roles that resolve to the same location.
///
/// Aliasing is checked after resolution because two different configured paths
/// can reach the same directory through a symlink, and that is exactly the
/// case a naive string comparison misses.
fn check_aliases(paths: &[ResolvedPath]) -> Result<(), HarnessError> {
    let mut seen: HashMap<&Path, &str> = HashMap::new();
    for entry in paths {
        if let Some(previous) = seen.insert(entry.resolved.as_path(), &entry.field) {
            return Err(FieldError::new(
                &entry.field,
                format!(
                    "resolves to the same location as `{previous}`: {}",
                    entry.resolved.display()
                ),
                ErrorCode::ConfigPathAlias,
            )
            .into_error());
        }
    }
    Ok(())
}

/// Rejects a control or authority repository nested inside the candidate.
///
/// Nesting would place authoritative state inside the tree a candidate actor
/// can rewrite, which invariant 7.1.1 forbids.
fn check_nesting(paths: &[ResolvedPath]) -> Result<(), HarnessError> {
    let Some(repository) = paths.iter().find(|entry| entry.field == "repository") else {
        return Ok(());
    };
    for entry in paths {
        if entry.field == "repository" {
            continue;
        }
        if entry.resolved.starts_with(&repository.resolved) {
            return Err(FieldError::new(
                &entry.field,
                format!(
                    "must not be nested inside the candidate repository at {}",
                    repository.resolved.display()
                ),
                ErrorCode::ConfigPathNested,
            )
            .into_error());
        }
    }
    Ok(())
}

/// Requires the installed Git to satisfy the configured minimum.
fn check_git_version(config: &ProjectConfig) -> Result<(GitVersion, GitVersion), HarnessError> {
    let minimum: GitVersion = format!("git version {}", config.host_policy.minimum_git_version)
        .parse()
        .map_err(|_| {
            FieldError::new(
                "host_policy.minimum_git_version",
                format!(
                    "expected `major.minor.patch`, found `{}`",
                    config.host_policy.minimum_git_version
                ),
                ErrorCode::ConfigInvalidValue,
            )
            .into_error()
        })?;

    let installed = inspect::version()?;
    if installed < minimum {
        return Err(FieldError::new(
            "host_policy.minimum_git_version",
            format!("installed Git {installed} is older than the required {minimum}"),
            ErrorCode::ConfigGitVersion,
        )
        .into_error());
    }
    Ok((installed, minimum))
}

/// Requires the host operating system to be declared supported.
fn check_host(config: &ProjectConfig) -> Result<(), HarnessError> {
    let host = std::env::consts::OS;
    if config
        .host_policy
        .supported_os
        .iter()
        .any(|candidate| candidate == host)
    {
        return Ok(());
    }
    Err(FieldError::new(
        "host_policy.supported_os",
        format!(
            "host `{host}` is not listed; configured: {}",
            config.host_policy.supported_os.join(", ")
        ),
        ErrorCode::ConfigUnsupportedHost,
    )
    .into_error())
}

/// Requires the candidate to be a repository whose protected branch resolves to
/// exactly one commit.
fn check_repository_and_branch(config: &ProjectConfig) -> Result<String, HarnessError> {
    let class = inspect::classify(&config.repository)?;
    match class {
        RepositoryClass::Repository { bare: false, .. } => {}
        RepositoryClass::Repository { bare: true, .. } => {
            return Err(FieldError::new(
                "repository",
                "the candidate repository must have a working tree, not be bare",
                ErrorCode::ConfigNotRepository,
            )
            .into_error());
        }
        RepositoryClass::NotRepository => {
            return Err(FieldError::new(
                "repository",
                format!("not a Git repository: {}", config.repository.display()),
                ErrorCode::ConfigNotRepository,
            )
            .into_error());
        }
        RepositoryClass::GitError { diagnostic } => {
            return Err(FieldError::new(
                "repository",
                format!("Git refused to inspect the path: {diagnostic}"),
                ErrorCode::ConfigNotRepository,
            )
            .into_error());
        }
    }

    let scope = crate::git::command::GitScope::work_tree(&config.repository);
    inspect::resolve_commit(&scope, &format!("refs/heads/{}", config.protected_branch)).map_err(
        |_| {
            FieldError::new(
                "protected_branch",
                format!(
                    "branch `{}` does not resolve to one commit",
                    config.protected_branch
                ),
                ErrorCode::ConfigProtectedBranch,
            )
            .into_error()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_detection_reports_the_second_field() {
        let paths = vec![
            ResolvedPath {
                field: "repository".to_owned(),
                configured: PathBuf::from("/a"),
                resolved: PathBuf::from("/same"),
                via_symlink: true,
            },
            ResolvedPath {
                field: "control_repository".to_owned(),
                configured: PathBuf::from("/b"),
                resolved: PathBuf::from("/same"),
                via_symlink: true,
            },
        ];
        let error = check_aliases(&paths).expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::ConfigPathAlias);
        assert_eq!(error.details()["field"], "control_repository");
    }

    #[test]
    fn distinct_paths_are_accepted() {
        let paths = vec![
            ResolvedPath {
                field: "repository".to_owned(),
                configured: PathBuf::from("/a"),
                resolved: PathBuf::from("/a"),
                via_symlink: false,
            },
            ResolvedPath {
                field: "control_repository".to_owned(),
                configured: PathBuf::from("/b"),
                resolved: PathBuf::from("/b"),
                via_symlink: false,
            },
        ];
        assert!(check_aliases(&paths).is_ok());
    }

    #[test]
    fn nesting_inside_the_candidate_is_rejected() {
        let paths = vec![
            ResolvedPath {
                field: "repository".to_owned(),
                configured: PathBuf::from("/repo"),
                resolved: PathBuf::from("/repo"),
                via_symlink: false,
            },
            ResolvedPath {
                field: "control_repository".to_owned(),
                configured: PathBuf::from("/repo/control"),
                resolved: PathBuf::from("/repo/control"),
                via_symlink: false,
            },
        ];
        let error = check_nesting(&paths).expect_err("must reject");
        assert_eq!(error.code(), ErrorCode::ConfigPathNested);
        assert_eq!(error.details()["field"], "control_repository");
    }

    #[test]
    fn a_sibling_of_the_candidate_is_accepted() {
        let paths = vec![
            ResolvedPath {
                field: "repository".to_owned(),
                configured: PathBuf::from("/work/repo"),
                resolved: PathBuf::from("/work/repo"),
                via_symlink: false,
            },
            ResolvedPath {
                field: "control_repository".to_owned(),
                configured: PathBuf::from("/work/control"),
                resolved: PathBuf::from("/work/control"),
                via_symlink: false,
            },
        ];
        assert!(check_nesting(&paths).is_ok());
    }
}
