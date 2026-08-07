//! Confirms every `change-harness ...` command shape written in specific
//! `SKILL.md` sections actually exists on the CLI.
//!
//! `ErrorCode::convergence_recovery` in `src/error.rs` told operators the
//! disposition command "is not part of this release" for two releases after
//! it shipped, because nothing pinned that text against reality. This test
//! exists so the same thing cannot happen to any guide section listed in
//! [`COVERED_SECTIONS`]: every `change-harness ...` invocation written there
//! is parsed out and actually run with `--help` appended, so a renamed flag,
//! a misspelled subcommand, or an invented option fails this test rather
//! than quietly misleading whoever reads the guide next.
//!
//! # How to write a section so this test can find its commands
//!
//! - A section starts at its exact heading text, as listed in
//!   [`COVERED_SECTIONS`], and runs until the next line starting with `## `
//!   (or end of file). A `### ` subheading does not end the section.
//! - Inside that span, only lines inside ` ```bash ` fences are read.
//! - Within a fenced block, only lines starting with `change-harness ` are
//!   treated as invocations. A line ending in `\` continues onto the next
//!   line, which is joined to it whatever that line starts with; a fenced
//!   block that ends mid-continuation panics rather than dropping the
//!   invocation it was building.
//! - A value containing spaces (a `--rationale` or `--risk`) must be wrapped
//!   in `"..."`; the splitter below is quote-aware but otherwise splits on
//!   whitespace.
//!
//! Each invocation is run verbatim with `--help` appended. `clap` validates
//! every token it is given — an unknown flag, an unrecognized subcommand,
//! and an invalid `value_enum` value (such as a `--dimension` that does not
//! exist) all fail argument parsing before `--help` gets a chance to
//! short-circuit anything, so this catches all three without needing a real
//! control repository, project, or card on disk. It cannot prove the prose
//! around a command is accurate, only that the command shape itself is
//! real — confirmed by running it, not by reading the CLI's source.
//!
//! # Covering a new section
//!
//! Add its exact heading text to [`COVERED_SECTIONS`]. A heading not listed
//! there is not covered, however plausible the commands under it look — this
//! list, not the prose, is what makes a section's commands real. Add the new
//! entry in the same change that adds the section to `SKILL.md`, so the
//! commands are never nominally documented without ever being run.

use std::process::Command;

/// Every `SKILL.md` section heading this test checks, exact text including
/// the leading `## `. Order does not matter; each heading is resolved and
/// scanned independently.
const COVERED_SECTIONS: &[&str] = &[
    "## Convergence budgets and escalation",
    "## What the Harness guarantees",
    "## Lifecycle",
    "## Orient first",
    "## Repository adoption",
];

/// Reads `SKILL.md` from the repository root.
fn skill_md() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/SKILL.md");
    std::fs::read_to_string(path).expect("SKILL.md should be readable at the repository root")
}

/// Slices out one section: from `heading` up to the next top-level (`## `)
/// heading, or to end of file if it is the last section.
fn section<'a>(skill_md: &'a str, heading: &str) -> &'a str {
    let start = skill_md
        .find(heading)
        .unwrap_or_else(|| panic!("SKILL.md should contain the heading {heading:?}"));
    let after_heading = &skill_md[start + heading.len()..];
    let end = after_heading.find("\n## ").unwrap_or(after_heading.len());
    &after_heading[..end]
}

/// Splits one line into argv-style tokens, honoring `"..."` spans so a
/// `--rationale "several words"` value survives as one token. No escaping
/// and no nesting: sections are written in plain prose, never with a
/// literal quote inside a quoted value.
fn split_invocation(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Parses every `change-harness ...` invocation out of fenced `bash` blocks
/// in `section`, in source order. Each returned entry is the full token
/// list, including the leading literal `"change-harness"`.
fn command_invocations(section: &str) -> Vec<Vec<String>> {
    let mut invocations = Vec::new();
    let mut in_bash_block = false;
    // `Some` only between a line ending in `\` and the line that completes it.
    let mut continued: Option<String> = None;
    for line in section.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```bash") {
            in_bash_block = true;
            continue;
        }
        if in_bash_block && trimmed.starts_with("```") {
            if let Some(unterminated) = continued.take() {
                panic!(
                    "a SKILL.md fenced block ends with an unterminated `\\` continuation, \
                     leaving this invocation unverified: {unterminated}"
                );
            }
            in_bash_block = false;
            continue;
        }
        if !in_bash_block {
            continue;
        }
        // A pending continuation absorbs this line whatever it starts with —
        // that is the point of the `\`. Otherwise only an invocation opens one.
        let invocation = match continued.take() {
            Some(prefix) => format!("{prefix} {trimmed}"),
            None if trimmed.starts_with("change-harness ") => trimmed.to_string(),
            None => continue,
        };
        match invocation.strip_suffix('\\') {
            Some(prefix) => continued = Some(prefix.trim_end().to_string()),
            None => invocations.push(split_invocation(&invocation)),
        }
    }
    invocations
}

#[test]
fn every_command_in_a_covered_skill_guide_section_really_exists() {
    let skill_md = skill_md();

    for heading in COVERED_SECTIONS {
        let body = section(&skill_md, heading);
        let invocations = command_invocations(body);

        // A parser that silently matches nothing would let this test pass
        // vacuously no matter what the section said — worse than no test at
        // all. Fail loudly instead of falling through an empty loop below.
        assert!(
            !invocations.is_empty(),
            "found zero `change-harness` invocations under heading {heading:?}; either the \
             section lost its commands or the heading/fence convention documented at the top of \
             this file was not followed"
        );

        for tokens in invocations {
            // `tokens[0]` is always the literal `change-harness`; everything
            // after it is the subcommand path plus that invocation's own
            // flags and values.
            let args = &tokens[1..];
            let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
                .args(args)
                .arg("--help")
                .output()
                .expect("the CLI binary should start");

            assert!(
                output.status.success(),
                "`{}` (under heading {heading:?}) is not a real command shape (exit {:?}):\nstdout: {}\nstderr: {}",
                tokens.join(" "),
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
}
