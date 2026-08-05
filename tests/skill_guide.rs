//! Confirms `SKILL.md`'s convergence section names only commands the CLI
//! actually has.
//!
//! `ErrorCode::convergence_recovery` in `src/error.rs` told operators the
//! disposition command "is not part of this release" for two releases after
//! it shipped, because nothing pinned that text against reality. This test
//! exists so the same thing cannot happen to the guide's own convergence
//! section: every `change-harness ...` invocation written there is parsed
//! out and actually run with `--help` appended, so a renamed flag, a
//! misspelled subcommand, or an invented seventh disposition fails this
//! test rather than quietly misleading whoever reads the guide next.
//!
//! # How to write the section so this test can find its commands
//!
//! - The section starts at the exact heading text in [`SECTION_HEADING`]
//!   and runs until the next line starting with `## ` (or end of file).
//! - Inside that span, only lines inside ` ```bash ` fences are read.
//! - Within a fenced block, only lines starting with `change-harness ` are
//!   treated as invocations — one invocation per line, no `\` continuations.
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

use std::process::Command;

/// Exact heading the section starts at. Kept as one constant so the test
/// and the doc comment above agree on where the section begins.
const SECTION_HEADING: &str = "## Convergence budgets and escalation";

/// Reads `SKILL.md` from the repository root.
fn skill_md() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/SKILL.md");
    std::fs::read_to_string(path).expect("SKILL.md should be readable at the repository root")
}

/// Slices out the convergence section: from [`SECTION_HEADING`] up to the
/// next top-level (`## `) heading, or to end of file if it is the last
/// section.
fn convergence_section(skill_md: &str) -> &str {
    let start = skill_md
        .find(SECTION_HEADING)
        .expect("SKILL.md should contain the convergence budgets and escalation heading");
    let after_heading = &skill_md[start + SECTION_HEADING.len()..];
    let end = after_heading.find("\n## ").unwrap_or(after_heading.len());
    &after_heading[..end]
}

/// Splits one line into argv-style tokens, honoring `"..."` spans so a
/// `--rationale "several words"` value survives as one token. No escaping
/// and no nesting: the section is written in plain prose, never with a
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
    for line in section.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```bash") {
            in_bash_block = true;
            continue;
        }
        if in_bash_block && trimmed.starts_with("```") {
            in_bash_block = false;
            continue;
        }
        if in_bash_block && trimmed.starts_with("change-harness ") {
            invocations.push(split_invocation(trimmed));
        }
    }
    invocations
}

#[test]
fn every_convergence_command_in_the_skill_guide_really_exists() {
    let skill_md = skill_md();
    let section = convergence_section(&skill_md);
    let invocations = command_invocations(section);

    // A parser that silently matches nothing would let this test pass
    // vacuously no matter what the section said — worse than no test at
    // all. Fail loudly instead of falling through an empty loop below.
    assert!(
        !invocations.is_empty(),
        "found zero `change-harness` invocations in the convergence section; either the \
         section lost its commands or the heading/fence convention documented at the top of \
         this file was not followed"
    );

    for tokens in invocations {
        // `tokens[0]` is always the literal `change-harness`; everything
        // after it is the subcommand path plus that invocation's own flags
        // and values.
        let args = &tokens[1..];
        let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
            .args(args)
            .arg("--help")
            .output()
            .expect("the CLI binary should start");

        assert!(
            output.status.success(),
            "`{}` is not a real command shape (exit {:?}):\nstdout: {}\nstderr: {}",
            tokens.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
