//! Read-only Git inspection.
//!
//! Nothing in this module mutates a repository. Every function either answers a
//! question or reports why Git refused to answer it.

use std::{
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use crate::error::HarnessError;

use super::command::{GitOutput, GitScope, run, run_ok, sanitize_diagnostic};

/// Minimum Git version the harness supports.
///
/// `merge-tree --write-tree`, used later for non-destructive merge preflight,
/// is only reliable from this version onward.
pub const MINIMUM_GIT_VERSION: GitVersion = GitVersion {
    major: 2,
    minor: 50,
    patch: 0,
};

/// A parsed Git version.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct GitVersion {
    /// Major component.
    pub major: u32,
    /// Minor component.
    pub minor: u32,
    /// Patch component, zero when Git omitted it.
    pub patch: u32,
}

impl fmt::Display for GitVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for GitVersion {
    type Err = HarnessError;

    /// Parses the `git --version` banner.
    ///
    /// Distributions append vendor text such as `(Apple Git-155)`, and some
    /// builds omit the patch component, so only the leading numeric components
    /// are read.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let rest = value.trim().strip_prefix("git version ").ok_or_else(|| {
            HarnessError::GitCommand(format!(
                "unrecognized version banner: {}",
                sanitize_diagnostic(value)
            ))
        })?;
        let numeric = rest
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .split(|c: char| !c.is_ascii_digit() && c != '.')
            .next()
            .unwrap_or_default();
        let mut parts = numeric.split('.').filter(|part| !part.is_empty());
        let mut component = |name: &str| -> Result<u32, HarnessError> {
            parts.next().unwrap_or("0").parse::<u32>().map_err(|_| {
                HarnessError::GitCommand(format!(
                    "unrecognized {name} component in: {}",
                    sanitize_diagnostic(value)
                ))
            })
        };
        let major = component("major")?;
        if major == 0 && !numeric.starts_with('0') {
            return Err(HarnessError::GitCommand(format!(
                "unrecognized version banner: {}",
                sanitize_diagnostic(value)
            )));
        }
        Ok(Self {
            major,
            minor: component("minor")?,
            patch: component("patch")?,
        })
    }
}

/// Reads the installed Git version.
///
/// # Errors
///
/// Returns an error when Git cannot be executed or its banner is unrecognized.
pub fn version() -> Result<GitVersion, HarnessError> {
    run_ok(&GitScope::None, ["--version"])?
        .trimmed_stdout()
        .parse()
}

/// What a path turned out to be.
///
/// The three-way split is the point: invariant-critical callers must never see
/// a Git refusal reported as "no repository here". That conflation was R-013.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepositoryClass {
    /// The path is inside a repository.
    Repository {
        /// Absolute path to the working-tree root, absent for a bare repository.
        top_level: Option<PathBuf>,
        /// Absolute path to the repository's Git directory.
        git_dir: PathBuf,
        /// Absolute path to the common Git directory shared by linked worktrees.
        common_dir: PathBuf,
        /// True when the repository has no working tree.
        bare: bool,
        /// True when the inspected path is a linked worktree, not the main one.
        linked_worktree: bool,
        /// True when `HEAD` is detached.
        detached_head: bool,
    },
    /// The path genuinely is not inside any repository.
    NotRepository,
    /// Git refused to answer, for example under `safe.directory`.
    GitError {
        /// Bounded, single-line diagnostic.
        diagnostic: String,
    },
}

impl RepositoryClass {
    /// True when the path resolved to a repository.
    #[must_use]
    pub const fn is_repository(&self) -> bool {
        matches!(self, Self::Repository { .. })
    }
}

/// Substrings Git uses when a path is genuinely outside any repository.
///
/// Anything else on a failing `rev-parse` is a refusal, not an absence.
const NOT_REPOSITORY_MARKERS: [&str; 2] = [
    "not a git repository",
    "this operation must be run in a work tree",
];

/// Classifies a path as a repository, a non-repository, or a Git refusal.
///
/// # Errors
///
/// Returns an error only when Git could not be executed at all.
pub fn classify(path: &Path) -> Result<RepositoryClass, HarnessError> {
    let scope = GitScope::work_tree(path);
    let probe = run(
        &scope,
        [
            "rev-parse",
            "--absolute-git-dir",
            "--path-format=absolute",
            "--git-common-dir",
            "--is-bare-repository",
        ],
    )?;

    if !probe.success() {
        let lowered = probe.stderr.to_lowercase();
        if NOT_REPOSITORY_MARKERS
            .iter()
            .any(|marker| lowered.contains(marker))
        {
            return Ok(RepositoryClass::NotRepository);
        }
        return Ok(RepositoryClass::GitError {
            diagnostic: probe.diagnostic(),
        });
    }

    let mut lines = probe.trimmed_stdout().lines();
    let git_dir = PathBuf::from(lines.next().unwrap_or_default());
    let common_dir = PathBuf::from(lines.next().unwrap_or_default());
    let bare = lines.next().unwrap_or("false").trim() == "true";

    let top_level = if bare {
        None
    } else {
        let output = run(&scope, ["rev-parse", "--show-toplevel"])?;
        output
            .success()
            .then(|| PathBuf::from(output.trimmed_stdout()))
    };

    // A linked worktree has its own git dir nested under the common dir; the
    // main worktree's git dir *is* the common dir.
    let linked_worktree = git_dir != common_dir;

    let head = run(&scope, ["symbolic-ref", "--quiet", "HEAD"])?;
    let detached_head = !bare && !head.success();

    Ok(RepositoryClass::Repository {
        top_level,
        git_dir,
        common_dir,
        bare,
        linked_worktree,
        detached_head,
    })
}

/// One entry from the worktree inventory.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct WorktreeEntry {
    /// Absolute path to the worktree.
    pub path: PathBuf,
    /// The commit `HEAD` resolves to, absent for a worktree with no commits.
    pub head: Option<String>,
    /// The checked-out branch, absent when detached or bare.
    pub branch: Option<String>,
    /// True when this entry is a bare repository.
    pub bare: bool,
    /// True when `HEAD` is detached.
    pub detached: bool,
    /// True when the worktree is locked against pruning.
    pub locked: bool,
}

/// Lists every worktree registered with a repository.
///
/// # Errors
///
/// Returns an error when Git cannot be executed or refuses the request.
pub fn worktrees(scope: &GitScope) -> Result<Vec<WorktreeEntry>, HarnessError> {
    let output = run_ok(scope, ["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_porcelain(&output.stdout))
}

/// Parses `git worktree list --porcelain`.
///
/// Records are newline-separated and blank-line-delimited; a value-less
/// attribute line such as `bare` or `detached` is a flag.
#[must_use]
pub fn parse_worktree_porcelain(raw: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;

    for line in raw.lines() {
        if line.is_empty() {
            entries.extend(current.take());
            continue;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "worktree" => {
                entries.extend(current.replace(WorktreeEntry {
                    path: PathBuf::from(value),
                    head: None,
                    branch: None,
                    bare: false,
                    detached: false,
                    locked: false,
                }));
            }
            "HEAD" => {
                if let Some(entry) = current.as_mut() {
                    entry.head = Some(value.to_owned());
                }
            }
            "branch" => {
                if let Some(entry) = current.as_mut() {
                    entry.branch = Some(value.to_owned());
                }
            }
            "bare" => {
                if let Some(entry) = current.as_mut() {
                    entry.bare = true;
                }
            }
            "detached" => {
                if let Some(entry) = current.as_mut() {
                    entry.detached = true;
                }
            }
            "locked" => {
                if let Some(entry) = current.as_mut() {
                    entry.locked = true;
                }
            }
            _ => {}
        }
    }
    entries.extend(current);
    entries
}

/// True when the installed Git supports the worktree subcommands the harness
/// relies on.
///
/// # Errors
///
/// Returns an error when Git cannot be executed.
pub fn supports_worktrees() -> Result<bool, HarnessError> {
    Ok(run(&GitScope::None, ["worktree", "list", "-h"])?
        .stdout
        .contains("worktree")
        || run(&GitScope::None, ["worktree", "-h"])?
            .stderr
            .contains("worktree"))
}

/// Resolves a revision to one exact commit object ID.
///
/// # Errors
///
/// Returns an error when the revision does not name a commit.
pub fn resolve_commit(scope: &GitScope, revision: &str) -> Result<String, HarnessError> {
    let output = run(
        scope,
        ["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )?;
    if output.success() {
        Ok(output.trimmed_stdout().to_owned())
    } else {
        Err(HarnessError::GitCommand(format!(
            "revision does not name a commit: {revision}"
        )))
    }
}

/// Returns the type of an object, such as `commit`, `tree`, or `blob`.
///
/// # Errors
///
/// Returns an error when the object does not exist.
pub fn object_type(scope: &GitScope, object: &str) -> Result<String, HarnessError> {
    Ok(run_ok(scope, ["cat-file", "-t", object])?
        .trimmed_stdout()
        .to_owned())
}

/// True when `ancestor` is reachable from `descendant`.
///
/// # Errors
///
/// Returns an error when Git cannot be executed or either revision is unknown.
pub fn is_ancestor(
    scope: &GitScope,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, HarnessError> {
    let output = run(scope, ["merge-base", "--is-ancestor", ancestor, descendant])?;
    match output.code {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(HarnessError::GitCommand(output.diagnostic())),
    }
}

/// Returns the best common ancestor of two revisions.
///
/// # Errors
///
/// Returns an error when the revisions share no history.
pub fn merge_base(scope: &GitScope, left: &str, right: &str) -> Result<String, HarnessError> {
    Ok(run_ok(scope, ["merge-base", left, right])?
        .trimmed_stdout()
        .to_owned())
}

/// A worktree's cleanliness.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct WorktreeState {
    /// True when no tracked or untracked change is present.
    pub clean: bool,
    /// Paths reported as changed or untracked.
    pub dirty_paths: Vec<String>,
}

/// Reports whether a worktree is clean, counting untracked files.
///
/// Untracked files count because an untracked file can silently become part of
/// a candidate the moment someone stages it.
///
/// # Errors
///
/// Returns an error when Git cannot be executed or refuses the request.
pub fn worktree_state(scope: &GitScope) -> Result<WorktreeState, HarnessError> {
    let output = run_ok(
        scope,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let dirty_paths: Vec<String> = output
        .stdout
        .split('\0')
        .filter(|record| !record.is_empty())
        .map(|record| record.get(3..).unwrap_or(record).to_owned())
        .collect();
    Ok(WorktreeState {
        clean: dirty_paths.is_empty(),
        dirty_paths,
    })
}

/// A Git operation left half-finished in the worktree.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InProgressOperation {
    /// A merge is in progress.
    Merge,
    /// A rebase is in progress.
    Rebase,
    /// A cherry-pick is in progress.
    CherryPick,
    /// A revert is in progress.
    Revert,
    /// A bisect is in progress.
    Bisect,
}

/// Marker paths, relative to the Git directory, that indicate each operation.
const OPERATION_MARKERS: [(InProgressOperation, &[&str]); 5] = [
    (InProgressOperation::Merge, &["MERGE_HEAD"]),
    (
        InProgressOperation::Rebase,
        &["rebase-merge", "rebase-apply"],
    ),
    (InProgressOperation::CherryPick, &["CHERRY_PICK_HEAD"]),
    (InProgressOperation::Revert, &["REVERT_HEAD"]),
    (InProgressOperation::Bisect, &["BISECT_LOG"]),
];

/// Detects merge, rebase, cherry-pick, revert, or bisect state.
///
/// A worktree with an unresolved operation cannot be reasoned about, so callers
/// treat any result here as a blocking precondition.
///
/// # Errors
///
/// Returns an error when Git cannot be executed.
pub fn in_progress_operations(scope: &GitScope) -> Result<Vec<InProgressOperation>, HarnessError> {
    let mut found = Vec::new();
    for (operation, markers) in OPERATION_MARKERS {
        for marker in markers {
            // `--path-format=absolute` is required: the default is relative to
            // the *process* working directory, not the repository, so a bare
            // `--git-path` yields `.git/MERGE_HEAD` and an existence check
            // silently tests the wrong filesystem location.
            let resolved = run(
                scope,
                ["rev-parse", "--path-format=absolute", "--git-path", marker],
            )?;
            if !resolved.success() {
                continue;
            }
            if Path::new(resolved.trimmed_stdout()).exists() {
                found.push(operation);
                break;
            }
        }
    }
    Ok(found)
}

/// Reads the raw output of a Git invocation without classifying it.
///
/// Exposed so higher layers can add queries without widening this module.
///
/// # Errors
///
/// Returns an error when Git could not be executed.
pub fn raw<I, S>(scope: &GitScope, args: I) -> Result<GitOutput, HarnessError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    run(scope, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vendor_decorated_version_banners() {
        assert_eq!(
            "git version 2.50.1 (Apple Git-155)"
                .parse::<GitVersion>()
                .unwrap(),
            GitVersion {
                major: 2,
                minor: 50,
                patch: 1
            }
        );
        assert_eq!(
            "git version 2.39.5".parse::<GitVersion>().unwrap(),
            GitVersion {
                major: 2,
                minor: 39,
                patch: 5
            }
        );
        assert_eq!(
            "git version 2.51".parse::<GitVersion>().unwrap(),
            GitVersion {
                major: 2,
                minor: 51,
                patch: 0
            }
        );
        assert_eq!(
            "git version 2.50.1.windows.1"
                .parse::<GitVersion>()
                .unwrap(),
            GitVersion {
                major: 2,
                minor: 50,
                patch: 1
            }
        );
    }

    #[test]
    fn rejects_unrecognized_version_banners() {
        for bad in ["", "2.50.1", "git version abc", "not a banner"] {
            assert!(
                bad.parse::<GitVersion>().is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn version_ordering_supports_a_minimum_check() {
        let older = GitVersion {
            major: 2,
            minor: 39,
            patch: 5,
        };
        let newer = GitVersion {
            major: 2,
            minor: 50,
            patch: 1,
        };
        assert!(older < MINIMUM_GIT_VERSION);
        assert!(newer >= MINIMUM_GIT_VERSION);
    }

    #[test]
    fn installed_version_is_readable() {
        let installed = version().unwrap();
        assert!(installed.major >= 2);
    }

    #[test]
    fn worktree_porcelain_parses_main_linked_detached_and_bare() {
        let raw = "worktree /repo\nHEAD abc123\nbranch refs/heads/main\n\n\
                   worktree /repo/wt\nHEAD def456\ndetached\nlocked\n\n\
                   worktree /bare.git\nbare\n\n";
        let entries = parse_worktree_porcelain(raw);
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].path, PathBuf::from("/repo"));
        assert_eq!(entries[0].branch.as_deref(), Some("refs/heads/main"));
        assert!(!entries[0].detached && !entries[0].bare);

        assert!(entries[1].detached);
        assert!(entries[1].locked);
        assert_eq!(entries[1].head.as_deref(), Some("def456"));
        assert!(entries[1].branch.is_none());

        assert!(entries[2].bare);
    }

    #[test]
    fn worktree_porcelain_tolerates_a_missing_trailing_blank_line() {
        let raw = "worktree /repo\nHEAD abc123\nbranch refs/heads/main";
        assert_eq!(parse_worktree_porcelain(raw).len(), 1);
    }

    #[test]
    fn worktree_porcelain_of_empty_input_is_empty() {
        assert!(parse_worktree_porcelain("").is_empty());
    }

    #[test]
    fn this_repository_classifies_as_a_repository() {
        let class = classify(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(class.is_repository(), "{class:?}");
    }

    #[test]
    fn a_non_repository_is_not_reported_as_a_git_error() {
        let temp = tempfile::tempdir().unwrap();
        // A temporary directory can still sit inside a repository on some
        // hosts; only assert the classification is never a refusal.
        let class = classify(temp.path()).unwrap();
        assert!(
            !matches!(class, RepositoryClass::GitError { .. }),
            "an ordinary directory must not be a Git refusal: {class:?}"
        );
    }

    #[test]
    fn worktree_support_is_detected() {
        assert!(supports_worktrees().unwrap());
    }
}
