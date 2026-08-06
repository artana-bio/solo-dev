//! Confirms every command reference in `ErrorCode` recovery text — the
//! operator-facing guidance `src/error.rs` returns from
//! `ErrorCode::recovery()` — actually exists on the CLI.
//!
//! `ErrorCode::convergence_recovery` told operators that the command
//! resolving a spent convergence budget "is not part of this release; see
//! issue #74" for two releases after `disposition renew` shipped, because
//! nothing checked that text against the built binary. `tests/skill_guide.rs`
//! solves the identical problem for `SKILL.md`'s prose; this file solves it
//! for `src/error.rs`'s recovery text with the same mechanism — extract a
//! command reference, then run it with `--help` appended so `clap` itself
//! validates the subcommand path and every flag, exactly as
//! `tests/skill_guide.rs` does.
//!
//! # Why the extraction rule differs from `skill_guide.rs`
//!
//! `SKILL.md` writes runnable invocations on their own line, each starting
//! with the literal `change-harness `, inside ` ```bash ` fences. Recovery
//! text is prose embedded in Rust string literals — a full sentence such as
//! "Run `project recover --resume` to clear the lock..." — so there is no
//! fence and no line prefix to key off. Reading every recovery string in
//! `src/error.rs` (66 of them, at the time this test was written) shows what
//! convention the text actually uses: a command reference is wrapped in
//! backticks, the same inline-code convention `SKILL.md` itself renders
//! with. But not every backtick span in that text names an invocation:
//!
//! - `` `--help` `` and `` `--invariant-holds` `` are bare flags, mentioned
//!   without the subcommand they belong to. Running either alone as a
//!   command is not what the sentence means, and `--invariant-holds` alone
//!   is not even a valid top-level flag, so treating it as one would fail
//!   this test for a reason that has nothing to do with staleness.
//! - `` `human_reviewer: true` `` names a recorded field and its value, not
//!   a command.
//!
//! So the rule below is: extract every backtick span, then keep only the
//! ones whose first token does not start with `-` (rules out a bare flag)
//! and that contain no `:` (rules out a field/value pair). See
//! [`names_a_command`]. Every span the rule keeps is run as
//! `change-harness <span> --help` against the built binary.
//!
//! # What this rule would miss
//!
//! Of the 66 recovery strings in `src/error.rs`, 9 name a command by this
//! rule and 57 name none — most guidance is corrective prose ("check
//! permissions", "narrow one card's write scope") with nothing to check
//! against a binary, which is expected and not a gap. The real gap is
//! narrower and worth naming explicitly: `convergence_recovery`'s original
//! text — "...The command that records one is not part of this release; see
//! issue #74." — named no command in backticks at all; it only alluded to
//! one in prose. A backtick-based rule extracts nothing from a sentence like
//! that, so it cannot distinguish a correct prose reference from a stale
//! one. That is not a bug in the rule below so much as it is the reason
//! every recovery arm should name its command in backticks in the first
//! place, the same discipline `SKILL.md` already follows. A recovery string
//! that never names its command in backticks stays invisible to this test,
//! exactly as `convergence_recovery` was to every test run before this one.
//!
//! # Scope of the scan
//!
//! Only string literals returned as recovery guidance are in scope — not
//! doc comments, not code comments, not the `#[error("...")]` display text
//! on `HarnessError`, and not the assertion messages inside `error.rs`'s own
//! `#[cfg(test)]` module. [`recovery_strings`] enforces this by reading only
//! the file text before the `#[cfg(test)]` marker, and by keeping only
//! lines whose trimmed text opens with `"` — which a Rust comment or doc
//! comment never does (they open with `//` or `///`), and which the
//! `#[error(...)]` attribute lines never do either (they open with
//! `#[error(`). Confirmed by reading `src/error.rs` in full while writing
//! this test: every line that convention selects carries exactly one
//! open/close quote pair, so slicing from the first `"` to the last `"` on
//! the line recovers the literal's contents exactly, with no embedded
//! quotes or line continuations to trip over.

use std::process::Command;

/// Reads `src/error.rs` from the repository root.
fn error_rs() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/error.rs");
    std::fs::read_to_string(path).expect("src/error.rs should be readable at the repository root")
}

/// Extracts every recovery-guidance string literal from `source`, in source
/// order. See the module doc comment for why this line-level rule is safe
/// for this file.
fn recovery_strings(source: &str) -> Vec<String> {
    let scanned = source
        .split("#[cfg(test)]")
        .next()
        .expect("str::split always yields at least one piece");

    scanned
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let rest = trimmed.strip_prefix('"')?;
            let end = rest.rfind('"')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

/// Extracts every backtick-delimited span from `text`, in source order.
fn backtick_spans(text: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('`') else {
            break;
        };
        spans.push(&after_open[..close]);
        rest = &after_open[close + 1..];
    }
    spans
}

/// True when a backtick span names a `change-harness` subcommand path
/// rather than a bare flag or a `field: value` mention. See the module doc
/// comment for the two shapes this excludes and why.
fn names_a_command(span: &str) -> bool {
    let first_token = span.split_whitespace().next().unwrap_or("");
    !first_token.is_empty() && !first_token.starts_with('-') && !span.contains(':')
}

#[test]
fn every_command_named_in_recovery_text_really_exists() {
    let source = error_rs();
    let strings = recovery_strings(&source);

    // A parser that silently matched nothing would let this test pass
    // vacuously no matter what `error.rs` said — worse than no test at all.
    assert!(
        !strings.is_empty(),
        "found zero recovery-guidance string literals in src/error.rs; the scan boundary or the \
         opening-quote convention documented at the top of this file no longer matches the file"
    );

    let mut checked = 0usize;
    for text in &strings {
        for span in backtick_spans(text) {
            if !names_a_command(span) {
                continue;
            }
            checked += 1;

            let args: Vec<&str> = span.split_whitespace().collect();
            let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
                .args(&args)
                .arg("--help")
                .output()
                .expect("the CLI binary should start");

            assert!(
                output.status.success(),
                "recovery text names `{span}`, which is not a real command shape (exit {:?}):\n\
                 full string: {text:?}\nstdout: {}\nstderr: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }

    // Same vacuity guard as above, this time at the level of the extraction
    // rule rather than the file scan: even with recovery strings found, a
    // `names_a_command` that silently excluded everything would leave the
    // loop above a no-op that never runs the binary at all.
    assert!(
        checked > 0,
        "zero recovery strings named a command by the rule this test uses (a backtick span whose \
         first token is not a flag and that contains no `:`); either every recovery string really \
         is command-free, which would make this test unable to ever fail, or the extraction rule \
         stopped matching text that is actually there"
    );
}
