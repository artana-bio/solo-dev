//! Terminal-environment probing for the optional `demo` animation.
//!
//! Mirrors [`crate::domain::clock::Clock`]: production reads the real
//! environment, tests inject a fixed one, so the decision of whether to
//! animate is assertable without a real TTY and without mutating process-wide
//! environment variables from parallel test threads.

use std::io::IsTerminal as _;

use super::output::OutputFormat;

/// What the presentation layer needs to know about where it is running.
pub trait Environment {
    /// True when standard error is attached to an interactive terminal.
    fn stderr_is_terminal(&self) -> bool;
    /// Reads an environment variable, as the process sees it.
    fn var(&self, key: &str) -> Option<String>;
}

/// Reads the real process environment.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemEnvironment;

impl Environment for SystemEnvironment {
    fn stderr_is_terminal(&self) -> bool {
        std::io::stderr().is_terminal()
    }

    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Why the assembly-line animation did not play.
///
/// Ordered by how deliberate the cause was: a JSON request or an explicit
/// flag says more about intent than an unset `TERM`, so [`skip_reason`]
/// checks in this order and the first match wins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipReason {
    /// `--output json` was requested; stdout must stay the stable envelope.
    JsonOutput,
    /// `--no-animation` was passed explicitly.
    ExplicitlyDisabled,
    /// `NO_COLOR` is set, per the <https://no-color.org> convention.
    NoColor,
    /// `TERM=dumb`.
    DumbTerminal,
    /// Standard error is not attached to an interactive terminal.
    NotATerminal,
}

impl SkipReason {
    /// A short, human-readable explanation for the command's text output.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::JsonOutput => "JSON output was requested",
            Self::ExplicitlyDisabled => "--no-animation was passed",
            Self::NoColor => "NO_COLOR is set",
            Self::DumbTerminal => "TERM is `dumb`",
            Self::NotATerminal => "standard error is not a terminal",
        }
    }

    /// The stable machine-readable token for the JSON payload.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::JsonOutput => "json_output",
            Self::ExplicitlyDisabled => "explicitly_disabled",
            Self::NoColor => "no_color",
            Self::DumbTerminal => "dumb_terminal",
            Self::NotATerminal => "not_a_terminal",
        }
    }
}

/// Decides whether the animation should play, and why not when it should not.
///
/// `format` and `no_animation` are read first because they say more about
/// intent than the ambient environment: a caller that asked for JSON, or
/// passed the flag, gets that reason back even when standard error happens to
/// be a terminal too.
#[must_use]
pub fn skip_reason(
    format: OutputFormat,
    no_animation: bool,
    environment: &dyn Environment,
) -> Option<SkipReason> {
    if format == OutputFormat::Json {
        return Some(SkipReason::JsonOutput);
    }
    if no_animation {
        return Some(SkipReason::ExplicitlyDisabled);
    }
    if environment.var("NO_COLOR").is_some() {
        return Some(SkipReason::NoColor);
    }
    if environment.var("TERM").as_deref() == Some("dumb") {
        return Some(SkipReason::DumbTerminal);
    }
    if !environment.stderr_is_terminal() {
        return Some(SkipReason::NotATerminal);
    }
    None
}

/// Lowest terminal width the full-frame layout is drawn at.
///
/// Below this, [`crate::cli::floor`] falls back to its compact layout rather
/// than truncating station cells into illegibility.
pub const MIN_FULL_WIDTH: usize = 78;

/// Width assumed when `COLUMNS` is unset, empty, or not a positive integer.
pub const DEFAULT_WIDTH: usize = 80;

/// Reads the terminal width the animation should draw at.
///
/// `unsafe_code` is forbidden crate-wide, which rules out a direct `ioctl`
/// query, and this is a cosmetic feature that does not warrant a new
/// dependency just to read a column count. `$COLUMNS` is what an interactive
/// shell already exports; anything else falls back to [`DEFAULT_WIDTH`].
#[must_use]
pub fn terminal_width(environment: &dyn Environment) -> usize {
    environment
        .var("COLUMNS")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width > 0)
        .unwrap_or(DEFAULT_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeEnvironment {
        terminal: bool,
        vars: HashMap<String, String>,
    }

    impl FakeEnvironment {
        fn terminal() -> Self {
            Self {
                terminal: true,
                vars: HashMap::new(),
            }
        }

        fn with(mut self, key: &str, value: &str) -> Self {
            self.vars.insert(key.to_owned(), value.to_owned());
            self
        }
    }

    impl Environment for FakeEnvironment {
        fn stderr_is_terminal(&self) -> bool {
            self.terminal
        }

        fn var(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned()
        }
    }

    #[test]
    fn json_output_wins_even_on_a_real_terminal() {
        let environment = FakeEnvironment::terminal();
        assert_eq!(
            skip_reason(OutputFormat::Json, false, &environment),
            Some(SkipReason::JsonOutput)
        );
    }

    #[test]
    fn explicit_flag_wins_over_a_real_terminal() {
        let environment = FakeEnvironment::terminal();
        assert_eq!(
            skip_reason(OutputFormat::Text, true, &environment),
            Some(SkipReason::ExplicitlyDisabled)
        );
    }

    #[test]
    fn no_color_is_honored() {
        let environment = FakeEnvironment::terminal().with("NO_COLOR", "1");
        assert_eq!(
            skip_reason(OutputFormat::Text, false, &environment),
            Some(SkipReason::NoColor)
        );
    }

    #[test]
    fn dumb_terminal_is_honored() {
        let environment = FakeEnvironment::terminal().with("TERM", "dumb");
        assert_eq!(
            skip_reason(OutputFormat::Text, false, &environment),
            Some(SkipReason::DumbTerminal)
        );
    }

    #[test]
    fn a_non_terminal_stderr_is_skipped() {
        let environment = FakeEnvironment::default();
        assert_eq!(
            skip_reason(OutputFormat::Text, false, &environment),
            Some(SkipReason::NotATerminal)
        );
    }

    #[test]
    fn an_interactive_terminal_with_nothing_set_plays() {
        let environment = FakeEnvironment::terminal();
        assert_eq!(skip_reason(OutputFormat::Text, false, &environment), None);
    }

    #[test]
    fn a_non_dumb_term_value_does_not_block() {
        let environment = FakeEnvironment::terminal().with("TERM", "xterm-256color");
        assert_eq!(skip_reason(OutputFormat::Text, false, &environment), None);
    }

    #[test]
    fn width_reads_columns_when_present() {
        let environment = FakeEnvironment::default().with("COLUMNS", "120");
        assert_eq!(terminal_width(&environment), 120);
    }

    #[test]
    fn width_falls_back_when_columns_is_unset() {
        let environment = FakeEnvironment::default();
        assert_eq!(terminal_width(&environment), DEFAULT_WIDTH);
    }

    #[test]
    fn width_falls_back_when_columns_is_not_a_positive_integer() {
        for bad in ["", "0", "-12", "wide"] {
            let environment = FakeEnvironment::default().with("COLUMNS", bad);
            assert_eq!(
                terminal_width(&environment),
                DEFAULT_WIDTH,
                "input {bad:?} should fall back"
            );
        }
    }

    #[test]
    fn every_skip_reason_has_a_non_empty_message_and_code() {
        for reason in [
            SkipReason::JsonOutput,
            SkipReason::ExplicitlyDisabled,
            SkipReason::NoColor,
            SkipReason::DumbTerminal,
            SkipReason::NotATerminal,
        ] {
            assert!(!reason.message().is_empty());
            assert!(!reason.code().is_empty());
        }
    }
}
