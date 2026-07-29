//! Git access.
//!
//! Split by responsibility: [`command`] owns process invocation, [`inspect`]
//! answers read-only questions, [`diff`] parses change sets, and [`worktree`]
//! performs the bounded branch and worktree mutations `WP-230` needs.
//!
//! Everything outside [`worktree`] is read-only. Mutation is confined to one
//! module so the safety rules in invariant 7.2 have a single place to hold.

pub mod archive;
pub mod authority;
pub mod backup;
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
        // This asserted `meets_minimum_version == (parsed_version >= MINIMUM)`,
        // which restates the implementation rather than checking it: replacing
        // the field with a hardcoded `true` left it passing, because on any
        // supported host the right-hand side is also true. It now asserts the
        // value itself, which is at least a claim about the world.
        //
        // What a hardcoded `true` would still slip past here is unavoidable
        // without an old Git to test against. The check that has teeth is
        // `check_git_version`, which refuses a project whose configured minimum
        // exceeds the installed version, and
        // `an_unsatisfiable_minimum_git_version_fails_explicitly` exercises it
        // in both directions. This field only feeds `doctor`'s report.
        let probe = GitClient::probe(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert_eq!(probe.minimum_version, MINIMUM_GIT_VERSION);
        assert!(probe.meets_minimum_version);
        assert!(probe.parsed_version >= MINIMUM_GIT_VERSION);
        assert!(probe.supports_worktrees);
    }

    #[test]
    fn version_comparison_discriminates_around_the_minimum() {
        // The ordering the compliance field rests on, exercised with values
        // this host cannot supply. Without this, nothing anywhere asserts that
        // an older Git compares as older.
        let minimum: GitVersion = format!("git version {MINIMUM_GIT_VERSION}")
            .parse()
            .unwrap();
        let older: GitVersion = "git version 2.39.5".parse().unwrap();
        let newer: GitVersion = "git version 2.99.0".parse().unwrap();

        assert!(older < minimum, "an older Git must compare as older");
        assert!(newer >= minimum);
        assert!(minimum >= minimum);
        // The compliance field is exactly this comparison, so an older Git has
        // to land on its failing side.
        let compliant = |version: GitVersion| version >= minimum;
        assert!(!compliant(older), "an older Git must fail compliance");
        assert!(compliant(newer));
    }

    #[test]
    fn probe_classifies_this_path_as_a_non_bare_repository() {
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
        // Non-bare is the whole claim, and the name says so. Neither
        // `linked_worktree` nor `detached_head` is pinned: this crate is
        // developed from linked worktrees and gated from a detached one, so
        // both depend on where the suite happens to run. Asserting either
        // would make the test a statement about the environment rather than
        // the code — which is exactly how it failed the first time the harness
        // ran its own gates against itself (D-052). The name was left claiming
        // more than that until D-061.
        let _ = linked_worktree;
        let _ = detached_head;
    }
}
