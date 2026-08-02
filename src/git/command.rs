//! Typed Git process invocation.
//!
//! Invariant 7.2.1 and 7.2.2: the CLI never builds a shell command string from
//! configuration, and Git is always invoked with an explicit executable and
//! argument array. Nothing in this module accepts a command line as text, so a
//! configuration value can never become a shell metacharacter.

use std::{
    ffi::{OsStr, OsString},
    path::Path,
    process::Command,
};

use crate::error::HarnessError;

/// Longest diagnostic retained from a failing Git invocation.
///
/// Git can emit unbounded output, and that text reaches error envelopes and
/// logs, so it is capped rather than propagated whole.
const MAX_DIAGNOSTIC_LEN: usize = 512;

/// One completed Git invocation.
///
/// Unlike the foundation probe this retains the exit status and stderr, so a
/// caller can distinguish "Git answered no" from "Git refused to answer".
/// Discarding that distinction was R-013.
#[derive(Clone, Debug)]
pub struct GitOutput {
    /// Exit code, or `None` when the process was terminated by a signal.
    pub code: Option<i32>,
    /// Captured standard output without decoding, for byte-exact Git data.
    pub stdout_bytes: Vec<u8>,
    /// Captured standard output, lossily decoded.
    pub stdout: String,
    /// Captured standard error, lossily decoded.
    pub stderr: String,
}

impl GitOutput {
    /// True when Git exited zero.
    #[must_use]
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Standard output with trailing newlines removed.
    #[must_use]
    pub fn trimmed_stdout(&self) -> &str {
        self.stdout.trim_end_matches(['\n', '\r'])
    }

    /// A bounded, single-line rendering of standard error, safe to embed in an
    /// error envelope.
    #[must_use]
    pub fn diagnostic(&self) -> String {
        sanitize_diagnostic(&self.stderr)
    }

    /// Converts a non-zero exit into an error, leaving success untouched.
    ///
    /// # Errors
    ///
    /// Returns [`HarnessError::GitCommand`] when Git did not exit zero.
    pub fn require_success(self) -> Result<Self, HarnessError> {
        if self.success() {
            Ok(self)
        } else {
            Err(HarnessError::GitCommand(self.diagnostic()))
        }
    }
}

/// Collapses whitespace and bounds length so a diagnostic stays printable.
#[must_use]
pub fn sanitize_diagnostic(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_DIAGNOSTIC_LEN {
        return collapsed;
    }
    let mut truncated: String = collapsed.chars().take(MAX_DIAGNOSTIC_LEN).collect();
    truncated.push('…');
    truncated
}

/// Where a Git invocation should run.
#[derive(Clone, Debug)]
pub enum GitScope {
    /// Run without repository context, for `git --version` and similar.
    None,
    /// Run inside a working tree via `-C`.
    WorkTree(OsString),
    /// Run against a bare repository via `--git-dir`.
    GitDir(OsString),
}

impl GitScope {
    /// Scope an invocation to a working tree.
    #[must_use]
    pub fn work_tree(path: &Path) -> Self {
        Self::WorkTree(path.as_os_str().to_owned())
    }

    /// Scope an invocation to a Git directory.
    #[must_use]
    pub fn git_dir(path: &Path) -> Self {
        Self::GitDir(path.as_os_str().to_owned())
    }

    /// The leading arguments this scope contributes.
    fn prefix(&self) -> Vec<OsString> {
        match self {
            Self::None => Vec::new(),
            Self::WorkTree(path) => vec![OsString::from("-C"), path.clone()],
            Self::GitDir(path) => vec![OsString::from("--git-dir"), path.clone()],
        }
    }
}

/// Environment variables that would override where and how Git operates.
///
/// Every scope this module builds says which repository to use with `-C`, and
/// `GIT_DIR` beats `-C`. A shell that exported it — an ordinary thing to do
/// while working on a bare repository — silently pointed every harness command
/// at a different repository, and the command then succeeded against the wrong
/// one. Nothing downstream could catch that, because nothing was wrong except
/// which repository it was.
///
/// The identity variables are here for a second reason. Section 9.2 keeps
/// workflow actor identity in the authoritative event rather than in Git author
/// configuration, so control history stays byte-identical regardless of who ran
/// the command — and `initialize_git` sets that identity in the repository
/// config. Environment identity outranks repository config, so an exported
/// `GIT_AUTHOR_NAME` quietly undid it.
///
/// `GIT_CONFIG_COUNT` is enough to disable the `GIT_CONFIG_KEY_<n>` and
/// `GIT_CONFIG_VALUE_<n>` pairs it counts, which cannot be enumerated here.
///
/// The gate runner clears its environment outright for this class of reason;
/// Git needs `PATH` and `HOME` to work at all, so this removes what redirects
/// rather than everything.
const AMBIENT_OVERRIDES: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_CONFIG",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_COUNT",
    // Stripped for the same reason as `GIT_CONFIG_COUNT`, and previously absent
    // by oversight: `GIT_CONFIG_PARAMETERS` injects arbitrary config into every
    // invocation including `commit-tree`, verified to reach
    // `i18n.commitEncoding`. `GIT_TEMPLATE_DIR` outranks `-c init.templateDir=`,
    // so without it a template hook really is written into the control
    // repository — inert, because hooks are neutralised per invocation, but the
    // claim that nothing is written there was false.
    "GIT_CONFIG_PARAMETERS",
    "GIT_TEMPLATE_DIR",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
];

/// The `-c` overrides for a sequence in which the harness authors the object.
///
/// Read this together with the porcelain/plumbing split at
/// [`crate::git::integration_worktree::merge`]. That split is what does the
/// work: the commit object is written by `commit-tree`, which composes no
/// message and runs no hook, so `commit.gpgsign`, `gpg.*`, `merge.log`,
/// `commit.template`, `prepare-commit-msg`, `commit-msg`, `pre-merge-commit`
/// and `post-commit` are all out of the picture by construction rather than by
/// being listed. Enumerating configuration keys is how the previous attempt at
/// this failed — the list had two entries and there were four.
///
/// Two things survive that split, and only these two are listed:
///
/// - `core.hooksPath`, pointed at a directory holding no hooks. The one hook
///   still reachable in the sequence is `reference-transaction`, which fires on
///   every ref the sequence writes — `MERGE_HEAD` as well as the `update-ref`
///   that moves the integration head — and can abort any of them (verified:
///   exit 128, "ref updates aborted by hook"). Writing those refs is
///   bookkeeping the harness owns outright, so nothing is lost by suppressing
///   hooks across the whole sequence — and a Git that grows a new commit-stage
///   hook is covered in advance.
/// - `merge.verifySignatures=false`. This one is genuinely a configuration key
///   and genuinely has to be named, because it stops `git merge` *before* any
///   content is combined (verified: exit 128, no `MERGE_HEAD`). The harness
///   decides what may be integrated from the plan and the review record; it
///   does not delegate that to whether a developer happened to sign a candidate
///   commit. D-006 and D-013 already put candidate provenance outside this
///   tool's trust model.
///
/// `hook_sink` must be absolute: a relative `core.hooksPath` resolves against
/// the top of the working tree, which is the candidate repository. It should
/// also be a directory the harness created rather than a path under the
/// project's `.git`, so that "the project cannot alter what the harness
/// authors" is true of the mechanism and not only of its intent.
#[must_use]
pub fn authoring_overrides(hook_sink: &Path) -> Vec<OsString> {
    let mut hooks_path = OsString::from("core.hooksPath=");
    hooks_path.push(hook_sink.as_os_str());
    vec![
        hooks_path,
        OsString::from("merge.verifySignatures=false"),
        // `commit-tree` honours `i18n.commitEncoding`, and `verify_authored`
        // refuses a commit carrying an `encoding` header it did not ask for.
        // Adding that refusal without this key made every integration fail on
        // any host where the setting exists — reported as an internal harness
        // defect, pointing the operator at the wrong repository. Reproduced via
        // repository config, `~/.gitconfig`, and an `includeIf` with no
        // repository configuration at all.
        //
        // Pinned to UTF-8 rather than emptied: an empty value makes Git write a
        // literal empty `encoding ` header, which the refusal then catches.
        OsString::from("i18n.commitEncoding=UTF-8"),
    ]
}

/// Removes the environment variables that would redirect or re-identify Git.
///
/// Exposed so a process the harness spawns *instead of* Git — a repository hook
/// the harness runs deliberately — starts from the same sanitised environment
/// every Git invocation gets. Setting a variable back afterwards is the
/// caller's business; this only takes away.
pub fn strip_ambient_overrides(command: &mut Command) {
    for name in AMBIENT_OVERRIDES {
        command.env_remove(name);
    }
}

/// Runs Git and captures its result without interpreting the exit status.
///
/// A non-zero exit is a normal answer for many queries, so classification is
/// left to the caller.
///
/// # Errors
///
/// Returns [`HarnessError::GitUnavailable`] when the process could not be
/// spawned at all.
pub fn run<I, S>(scope: &GitScope, args: I) -> Result<GitOutput, HarnessError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_with_config(scope, &[], args)
}

/// Runs Git with explicit `-c` overrides ahead of the subcommand.
///
/// The overrides sit between the scope prefix and the subcommand, which is
/// where Git accepts them, and they outrank `.git/config` and the operator's
/// `~/.gitconfig` without writing anything to either. Writing is the
/// alternative and it is much worse: turning off signing or hooks in a
/// developer's own repository to suit the harness would change what *their*
/// commits do.
///
/// # Errors
///
/// Returns [`HarnessError::GitUnavailable`] when the process could not be
/// spawned at all.
pub fn run_with_config<I, S>(
    scope: &GitScope,
    overrides: &[OsString],
    args: I,
) -> Result<GitOutput, HarnessError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_with_config_and_environment(scope, overrides, &[], args)
}

/// Runs Git with explicit `-c` overrides and a narrowly controlled environment.
///
/// Ambient redirecting variables are removed exactly as they are for ordinary
/// invocations. The supplied entries are then restored deliberately for
/// workflows that need Git's isolated index/object locations; callers must not
/// pass operator-controlled environment wholesale.
///
/// # Errors
///
/// Returns [`HarnessError::GitUnavailable`] when Git cannot be spawned.
pub fn run_with_config_and_environment<I, S>(
    scope: &GitScope,
    overrides: &[OsString],
    environment: &[(&OsStr, &OsStr)],
    args: I,
) -> Result<GitOutput, HarnessError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command.args(scope.prefix());
    for value in overrides {
        command.arg("-c");
        command.arg(value);
    }
    command.args(args);
    // Git may otherwise open a pager, an editor, or a credential prompt and
    // block a non-interactive command forever.
    command.env("GIT_PAGER", "cat");
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_OPTIONAL_LOCKS", "0");
    strip_ambient_overrides(&mut command);
    for (name, value) in environment {
        command.env(name, value);
    }

    let output = command.output().map_err(HarnessError::GitUnavailable)?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(GitOutput {
        code: output.status.code(),
        stdout_bytes: output.stdout,
        stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Runs Git with `-c` overrides and requires a zero exit.
///
/// # Errors
///
/// Returns an error when the process could not be spawned or exited non-zero.
pub fn run_with_config_ok<I, S>(
    scope: &GitScope,
    overrides: &[OsString],
    args: I,
) -> Result<GitOutput, HarnessError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_with_config(scope, overrides, args)?.require_success()
}

/// Runs Git and requires a zero exit.
///
/// # Errors
///
/// Returns an error when the process could not be spawned or exited non-zero.
pub fn run_ok<I, S>(scope: &GitScope, args: I) -> Result<GitOutput, HarnessError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run(scope, args)?.require_success()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_installed_version() {
        let output = run(&GitScope::None, ["--version"]).unwrap();
        assert!(output.success());
        assert!(output.trimmed_stdout().starts_with("git version"));
    }

    #[test]
    fn retains_exit_status_and_stderr_for_a_failing_invocation() {
        let output = run(
            &GitScope::None,
            ["rev-parse", "--verify", "definitely-not-a-ref"],
        )
        .unwrap();
        assert!(!output.success());
        assert_ne!(output.code, Some(0));
        assert!(
            !output.stderr.is_empty(),
            "stderr must be retained, not discarded"
        );
    }

    #[test]
    fn require_success_converts_failure_into_a_git_command_error() {
        let error = run(&GitScope::None, ["rev-parse", "--verify", "nope"])
            .unwrap()
            .require_success()
            .expect_err("must fail");
        assert_eq!(error.code().as_string(), "CH-EXTERNAL-GIT-COMMAND");
    }

    #[test]
    fn scope_contributes_only_documented_prefix_arguments() {
        assert!(GitScope::None.prefix().is_empty());
        assert_eq!(
            GitScope::work_tree(Path::new("/tmp/x")).prefix(),
            vec![OsString::from("-C"), OsString::from("/tmp/x")]
        );
        assert_eq!(
            GitScope::git_dir(Path::new("/tmp/x.git")).prefix(),
            vec![OsString::from("--git-dir"), OsString::from("/tmp/x.git")]
        );
    }

    #[test]
    fn argument_metacharacters_are_never_interpreted() {
        // If any layer built a shell string, this would try to run `whoami`.
        let output = run(
            &GitScope::None,
            ["rev-parse", "--verify", "$(whoami); rm -rf /"],
        )
        .unwrap();
        assert!(!output.success(), "the ref must simply not resolve");
        assert!(!output.stderr.contains("root"));
    }

    #[test]
    fn diagnostics_are_collapsed_and_bounded() {
        assert_eq!(sanitize_diagnostic("  a\n\n b \t c "), "a b c");
        let long = "x".repeat(MAX_DIAGNOSTIC_LEN * 2);
        let sanitized = sanitize_diagnostic(&long);
        assert_eq!(sanitized.chars().count(), MAX_DIAGNOSTIC_LEN + 1);
        assert!(sanitized.ends_with('…'));
    }

    #[test]
    fn empty_diagnostic_stays_empty() {
        assert_eq!(sanitize_diagnostic("   \n  "), "");
    }
}
