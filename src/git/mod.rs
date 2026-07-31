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
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    const UNSUPPORTED_GIT_CHILD: &str = "CHANGE_HARNESS_UNSUPPORTED_GIT_CHILD";

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
        if std::env::var_os(UNSUPPORTED_GIT_CHILD).is_some() {
            let probe = GitClient::probe(Path::new(".")).unwrap();
            assert!(
                probe.parsed_version < MINIMUM_GIT_VERSION,
                "the fake old Git must exercise the non-compliant side"
            );
            assert!(
                !probe.supports_worktrees,
                "a Git that rejects the worktree subcommand must not report worktree support"
            );
            return;
        }

        // The host assertions exercise the supported side. The child process
        // below gives the probe an old Git that rejects `worktree`, without
        // changing process-global PATH while sibling tests are running.
        let probe = GitClient::probe(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert_eq!(probe.minimum_version, MINIMUM_GIT_VERSION);
        assert!(probe.meets_minimum_version);
        assert!(probe.parsed_version >= MINIMUM_GIT_VERSION);
        assert!(probe.supports_worktrees);

        let temp = tempfile::tempdir().unwrap();
        let fake_git = temp.path().join("git");
        fs::write(
            &fake_git,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then\n\
               printf '%s\\n' 'git version 2.4.0'\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = \"-C\" ]; then\n\
               printf '%s\\n' 'fatal: not a git repository' >&2\n\
               exit 128\n\
             fi\n\
             printf '%s\\n' \"git: 'worktree' is not a git command. See 'git --help'.\" >&2\n\
             exit 1\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake_git).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_git, permissions).unwrap();

        let output = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("git::tests::probe_reports_minimum_version_compliance_and_worktree_support")
            .arg("--nocapture")
            .env(UNSUPPORTED_GIT_CHILD, "1")
            .env("PATH", temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "the unsupported-Git discriminator failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
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
