//! `demo`: a self-contained assembly-line animation.
//!
//! Plays a fixed, fictional cycle through six stations named after the real
//! commands they stand in for — `work start`, `gate run`, `handoff create`,
//! `review record`, `integration verify`, `promote` — so the shape of the
//! harness (linear stations, a station that can reject, rework looping a
//! card backwards, a weld step that gives the combination its own gate, a
//! final promotion) is visible without a control repository, a candidate, or
//! any state to set up. It reads nothing and writes nothing; the animation is
//! the entire effect. See [`crate::cli::floor`] for the animation itself.
//!
//! The animation always goes to standard error, never standard out, for the
//! same reason warnings do (Section 12.4): stdout is the stable result a
//! machine consumer parses, and an animation is exactly the kind of thing
//! that must never land inside it. [`crate::cli::tty::skip_reason`] decides
//! whether it plays at all: JSON output, `--no-animation`, `NO_COLOR`,
//! `TERM=dumb`, and a non-interactive standard error all skip it, silently
//! and without changing the command's success.

use std::{io, time::Duration};

use clap::Args;

use crate::{
    cli::{
        floor,
        output::{CommandOutcome, OutputFormat},
        tty::{Environment, SkipReason, skip_reason, terminal_width},
    },
    error::HarnessError,
};

/// Schema identifier for the demo payload.
pub const DEMO_SCHEMA: &str = "harness.demo/v1";

/// How long each frame is shown before the next one replaces it.
const FRAME_DELAY: Duration = Duration::from_millis(90);

/// Arguments accepted by `demo`.
#[derive(Debug, Args)]
pub struct DemoArgs {
    /// Skip the animation and print only the summary, even on a terminal.
    #[arg(long)]
    pub no_animation: bool,
}

/// Plays the assembly-line animation on standard error, when the environment
/// allows it, and builds the command's result either way.
///
/// The [`floor::TerminalSink`] that touches the real terminal — even its
/// constructor writes the hide-cursor escape — is only ever built on the
/// branch that has already decided to play. Building it unconditionally and
/// then skipping playback would still hide and immediately re-show the
/// cursor on every skip path, leaking stray escape bytes into a stream a
/// caller may be asking to keep clean precisely because it asked to skip.
///
/// # Errors
///
/// Never fails. The `Result` return matches every other command module so
/// callers can dispatch uniformly; nothing this command does — building the
/// JSON payload, playing the animation — has a failure mode. A write error to
/// standard error during playback is swallowed rather than surfaced, for the
/// same reason a warning write failure is swallowed in `main`: an animation
/// is cosmetic, and cosmetic output must never turn a command that did
/// nothing wrong into a failure.
pub fn execute(
    args: &DemoArgs,
    format: OutputFormat,
    environment: &dyn Environment,
) -> Result<CommandOutcome, HarnessError> {
    let skip = skip_reason(format, args.no_animation, environment);
    let outcome = if skip.is_none() {
        let mut sink = floor::TerminalSink::new(io::stderr());
        build_outcome(skip, environment, &mut sink, FRAME_DELAY)
    } else {
        build_outcome(skip, environment, &mut floor::NullSink, FRAME_DELAY)
    };
    Ok(outcome)
}

/// The testable core of [`execute`]: takes the already-decided skip reason
/// and the frame sink and pacing as parameters instead of hardcoding a real
/// terminal and a real delay, so tests can drive it with
/// [`floor::RecordingSink`] and a zero delay and assert on exactly what would
/// have been shown, without a TTY and without waiting through real playback.
fn build_outcome(
    skip: Option<SkipReason>,
    environment: &dyn Environment,
    sink: &mut dyn floor::FrameSink,
    frame_delay: Duration,
) -> CommandOutcome {
    if skip.is_none() {
        let width = terminal_width(environment);
        floor::play(&floor::frames(width), sink, frame_delay);
    }

    let text = skip.map_or_else(
        || {
            "Change Harness demo — a scripted assembly-line walkthrough of \
             work start -> gate run -> handoff create -> review record -> \
             integration verify -> promote.\nNo repository was read or changed."
                .to_owned()
        },
        |reason| {
            format!(
                "Change Harness demo (animation skipped: {}).\n\
                 Run this in an interactive terminal to see the assembly-line walkthrough.\n\
                 No repository was read or changed.",
                reason.message()
            )
        },
    );

    let stations: Vec<serde_json::Value> = floor::station_commands()
        .into_iter()
        .map(|(station, command)| serde_json::json!({"station": station, "command": command}))
        .collect();

    let data = serde_json::json!({
        "schema": DEMO_SCHEMA,
        "played": skip.is_none(),
        "skip_reason": skip.map(SkipReason::code),
        "stations": stations,
    });

    CommandOutcome::new("demo", text, data)
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
    }

    impl Environment for FakeEnvironment {
        fn stderr_is_terminal(&self) -> bool {
            self.terminal
        }

        fn var(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned()
        }
    }

    fn json_data(outcome: &CommandOutcome) -> serde_json::Value {
        let rendered = outcome.render(OutputFormat::Json).unwrap();
        serde_json::from_str::<serde_json::Value>(&rendered).unwrap()["data"].clone()
    }

    /// Mirrors [`execute`] exactly — computes the skip decision, then builds
    /// the outcome — but with an injectable sink and a zero delay, so tests
    /// exercise the real decision path without a TTY or a real wait. Unlike
    /// [`execute`], this always accepts a sink even on the skip path,
    /// because tests need to see when a sink was wrongly touched, not have
    /// that silently swapped for a null one.
    fn run(
        args: &DemoArgs,
        format: OutputFormat,
        environment: &dyn Environment,
        sink: &mut dyn floor::FrameSink,
    ) -> CommandOutcome {
        let skip = skip_reason(format, args.no_animation, environment);
        build_outcome(skip, environment, sink, Duration::ZERO)
    }

    #[test]
    fn json_output_skips_the_animation_and_presents_no_frames() {
        let mut sink = floor::RecordingSink::default();
        let outcome = run(
            &DemoArgs {
                no_animation: false,
            },
            OutputFormat::Json,
            &FakeEnvironment::terminal(),
            &mut sink,
        );
        assert!(
            sink.frames.is_empty(),
            "JSON output must never trigger the animation"
        );
        let data = json_data(&outcome);
        assert_eq!(data["schema"], DEMO_SCHEMA);
        assert_eq!(data["played"], false);
        assert_eq!(data["skip_reason"], "json_output");
    }

    #[test]
    fn a_non_terminal_skips_and_explains_why_in_text() {
        let mut sink = floor::RecordingSink::default();
        let outcome = run(
            &DemoArgs {
                no_animation: false,
            },
            OutputFormat::Text,
            &FakeEnvironment::default(),
            &mut sink,
        );
        assert!(sink.frames.is_empty());
        let rendered = outcome.render(OutputFormat::Text).unwrap();
        assert!(rendered.contains("animation skipped"));
        assert!(rendered.contains("not a terminal"));
    }

    #[test]
    fn an_interactive_terminal_plays_every_frame() {
        let mut sink = floor::RecordingSink::default();
        let outcome = run(
            &DemoArgs {
                no_animation: false,
            },
            OutputFormat::Text,
            &FakeEnvironment::terminal(),
            &mut sink,
        );
        assert_eq!(sink.frames, floor::frames(80));
        let data = json_data(&outcome);
        assert_eq!(data["played"], true);
        assert!(data["skip_reason"].is_null());
    }

    #[test]
    fn the_no_animation_flag_wins_even_on_a_terminal() {
        let mut sink = floor::RecordingSink::default();
        let outcome = run(
            &DemoArgs { no_animation: true },
            OutputFormat::Text,
            &FakeEnvironment::terminal(),
            &mut sink,
        );
        assert!(sink.frames.is_empty());
        let data = json_data(&outcome);
        assert_eq!(data["played"], false);
        assert_eq!(data["skip_reason"], "explicitly_disabled");
    }

    #[test]
    fn playback_honors_the_measured_terminal_width() {
        let mut sink = floor::RecordingSink::default();
        let environment = FakeEnvironment {
            terminal: true,
            vars: HashMap::from([("COLUMNS".to_owned(), "40".to_owned())]),
        };
        run(
            &DemoArgs {
                no_animation: false,
            },
            OutputFormat::Text,
            &environment,
            &mut sink,
        );
        assert_eq!(sink.frames, floor::frames(40));
    }

    #[test]
    fn every_station_names_its_real_command() {
        let mut sink = floor::RecordingSink::default();
        let outcome = run(
            &DemoArgs { no_animation: true },
            OutputFormat::Text,
            &FakeEnvironment::terminal(),
            &mut sink,
        );
        let data = json_data(&outcome);
        let stations = data["stations"].as_array().unwrap();
        assert_eq!(stations.len(), 6);
        assert_eq!(stations[0]["station"], "INTAKE");
        assert_eq!(stations[0]["command"], "work start");
        assert_eq!(stations[5]["station"], "SHIP");
        assert_eq!(stations[5]["command"], "promote");
    }

    #[test]
    fn text_mode_never_contains_raw_json() {
        let mut sink = floor::RecordingSink::default();
        let outcome = run(
            &DemoArgs { no_animation: true },
            OutputFormat::Text,
            &FakeEnvironment::terminal(),
            &mut sink,
        );
        let rendered = outcome.render(OutputFormat::Text).unwrap();
        assert!(
            !rendered.contains('{'),
            "text mode must not emit JSON: {rendered}"
        );
    }

    #[test]
    fn every_documented_json_key_is_present_whether_or_not_it_played() {
        for (format, no_animation) in [
            (OutputFormat::Text, false),
            (OutputFormat::Json, false),
            (OutputFormat::Text, true),
        ] {
            let mut sink = floor::RecordingSink::default();
            let outcome = run(
                &DemoArgs { no_animation },
                format,
                &FakeEnvironment::terminal(),
                &mut sink,
            );
            let data = json_data(&outcome);
            for key in ["schema", "played", "skip_reason", "stations"] {
                assert!(
                    data.get(key).is_some(),
                    "missing `{key}` for {format:?}/{no_animation}"
                );
            }
        }
    }

    #[test]
    fn the_public_entry_point_reaches_the_skip_branch_without_touching_a_real_terminal() {
        // Exercises `execute` itself, not just `build_outcome` through the
        // `run` test seam above: real playback sleeps `FRAME_DELAY` per
        // frame against real standard error, so a test cannot safely take
        // `execute`'s play branch (over 200 frames, ~18s) without either
        // hanging the suite or adding a seam purely to dodge that — not
        // justified for a three-line `if`. The skip branch has no such cost,
        // so this pins that `execute` reaches it (`--no-animation`, still
        // `Ok`, still reports `played: false`) rather than only ever being
        // covered indirectly through `build_outcome`.
        let outcome = execute(
            &DemoArgs { no_animation: true },
            OutputFormat::Text,
            &FakeEnvironment::terminal(),
        )
        .unwrap();
        let data = json_data(&outcome);
        assert_eq!(data["played"], false);
        assert_eq!(data["skip_reason"], "explicitly_disabled");
    }
}
