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
    let mut command = Command::new("git");
    command.args(scope.prefix());
    command.args(args);
    // Git may otherwise open a pager, an editor, or a credential prompt and
    // block a non-interactive command forever.
    command.env("GIT_PAGER", "cat");
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_OPTIONAL_LOCKS", "0");

    let output = command.output().map_err(HarnessError::GitUnavailable)?;
    Ok(GitOutput {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
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
