//! Git access.
//!
//! Split by responsibility: [`command`] owns process invocation, [`inspect`]
//! answers read-only questions, [`diff`] parses change sets, and [`worktree`]
//! performs the bounded branch and worktree mutations `WP-230` needs.
//!
//! Everything outside [`worktree`] is read-only. Mutation is confined to one
//! module so the safety rules in invariant 7.2 have a single place to hold.

pub mod authority;
pub mod command;
pub mod diff;
pub mod inspect;
pub mod integration_worktree;
pub mod landing;
pub mod merge;
pub mod worktree;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::HarnessError;

use inspect::{GitVersion, MINIMUM_GIT_VERSION, RepositoryClass};

/// A read-only summary of the host Git installation and one inspected path.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GitProbe {
    /// The raw `git --version` banner.
    pub version: String,
    /// The parsed version.
    pub parsed_version: GitVersion,
    /// The lowest version the harness supports.
    pub minimum_version: GitVersion,
    /// True when the installed version satisfies the minimum.
    pub meets_minimum_version: bool,
    /// True when the installed Git supports the worktree subcommands.
    pub supports_worktrees: bool,
    /// What the inspected path turned out to be.
    pub repository: RepositoryClass,
    /// The working-tree root, when one was detected.
    ///
    /// Retained as a top-level field because the foundation `doctor` payload
    /// exposed it and `WP-100` froze that shape.
    pub repository_root: Option<PathBuf>,
}

/// Entry point for read-only Git inspection.
pub struct GitClient;

impl GitClient {
    /// Inspects the installed Git executable and the repository containing a
    /// path.
    ///
    /// Unlike the foundation probe, a Git refusal such as `safe.directory` is
    /// reported as [`RepositoryClass::GitError`] rather than silently becoming
    /// "no repository detected". That conflation was R-013.
    ///
    /// # Errors
    ///
    /// Returns an error when Git cannot be executed or its version banner is
    /// unrecognized.
    pub fn probe(workspace: &Path) -> Result<GitProbe, HarnessError> {
        let banner = command::run_ok(&command::GitScope::None, ["--version"])?
            .trimmed_stdout()
            .to_owned();
        let parsed_version: GitVersion = banner.parse()?;
        let repository = inspect::classify(workspace)?;

        let repository_root = match &repository {
            RepositoryClass::Repository { top_level, .. } => top_level.clone(),
            RepositoryClass::NotRepository | RepositoryClass::GitError { .. } => None,
        };

        Ok(GitProbe {
            version: banner,
            parsed_version,
            minimum_version: MINIMUM_GIT_VERSION,
            meets_minimum_version: parsed_version >= MINIMUM_GIT_VERSION,
            supports_worktrees: inspect::supports_worktrees()?,
            repository,
            repository_root,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probing_this_repository_reports_a_repository_and_a_root() {
        let probe = GitClient::probe(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(probe.repository.is_repository());
        assert_eq!(
            probe.repository_root.as_deref(),
            Some(Path::new(env!("CARGO_MANIFEST_DIR")))
        );
        assert!(probe.version.starts_with("git version"));
    }

    #[test]
    fn probe_reports_minimum_version_compliance_and_worktree_support() {
        let probe = GitClient::probe(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert_eq!(probe.minimum_version, MINIMUM_GIT_VERSION);
        assert_eq!(
            probe.meets_minimum_version,
            probe.parsed_version >= MINIMUM_GIT_VERSION
        );
        assert!(probe.supports_worktrees);
    }

    #[test]
    fn probe_identifies_this_path_as_a_linked_worktree_or_the_main_one() {
        let probe = GitClient::probe(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let RepositoryClass::Repository {
            bare,
            linked_worktree,
            detached_head,
            ..
        } = probe.repository
        else {
            panic!("expected a repository");
        };
        assert!(!bare, "the source checkout is not bare");
        assert!(!detached_head, "the source checkout is on a branch");
        // This crate is developed from linked worktrees, so only assert the
        // field is populated rather than pinning a value that depends on where
        // the test runs.
        let _ = linked_worktree;
    }
}
