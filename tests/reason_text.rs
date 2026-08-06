//! Confirms every command reference written into a `reason` field — the
//! text `HarnessError::Control` and `HarnessError::ControlWithRecovery`
//! carry across `src/commands/*.rs` and its neighbors to explain a refusal —
//! actually exists on the CLI.
//!
//! `tests/recovery_text.rs` solves the identical problem for the shared
//! recovery-guidance table in `src/error.rs`; its own module doc describes
//! why it exists: `ErrorCode::convergence_recovery` told operators a shipped
//! command "is not part of this release" for two releases because nothing
//! checked that text against the built binary.
//!
//! The same rot is possible in `reason` strings, and demonstrably was not
//! caught: reviewing #105 turned up a refusal that named a real command
//! with one flag typo'd —
//!
//! ```text
//! run `gate reserve --card-id {card_id} --gate-idd {gate_id}` first
//! ```
//!
//! — and the full suite still passed, because nothing ran that text against
//! `clap`. This file is that check, extended to every `reason` field rather
//! than the one shared table.
//!
//! # Why this is a sibling file, not an extension of `recovery_text.rs`
//!
//! `recovery_text.rs` reads exactly one file, where every recovery string is
//! a `&'static str` literal, one per line. `reason` fields are `format!`
//! calls scattered across 44 files in `src/` (of 69 total, excluding
//! `error.rs`), routinely spanning several source lines and interpolating
//! runtime values — a fundamentally different shape of scan. Reusing
//! `recovery_text.rs`'s line-based extraction here would silently miss most
//! of them; rewriting `recovery_text.rs`'s extraction to handle both shapes
//! would blur a rule that is currently simple, correct, and documented into
//! one that serves neither file well. This file duplicates the tiny amount
//! of genuinely shared logic (`backtick_spans`) rather than import from
//! `recovery_text.rs` or factor it into `tests/support/mod.rs`, so each file
//! stays independently readable and `recovery_text.rs` stays untouched —
//! `tests/skill_guide.rs` already sets this precedent: it duplicates the
//! same "run the span with `--help` appended" mechanism rather than share
//! it.
//!
//! # The extraction rule
//!
//! Two independent decisions make up this rule: **which strings to look
//! at**, and **which backtick spans inside them name a command**.
//!
//! ## Which strings: a literal in `reason:` field position
//!
//! [`reason_strings`] scans each source file's raw text for the exact,
//! word-bounded token `reason:` (ruling out `blocked_reason:`, the one other
//! field in `src/` whose name ends in `reason:`) and takes whatever
//! literal follows: a bare `"..."`, or the first argument to `format!(...)`.
//! Nothing else is inspected — not a type (`reason: String` in a struct
//! definition), not a bare identifier (`reason,` field-init shorthand, or
//! `reason: message` where `message` is computed elsewhere) — because only a
//! literal is something this test can check at all.
//!
//! This one anchor is deliberately narrower than it could be. Two sites —
//! `acceptance.rs`'s and `integration.rs`'s `require_reviewed`/promotion
//! checks — build their text as `let reason = format!(...)` and forward it
//! through a shared `status_gate_refusal` helper, rather than writing
//! `reason:` directly. Matching `reason =` too would double the "what comes
//! next" branching in [`reason_strings`] to protect two sites that, read
//! while writing this test, contain no backtick-quoted command at all (only
//! an interpolated status name, which the placeholder rule below excludes
//! regardless). Widening the anchor now would add real parsing surface for
//! zero current protection; §"What this rule misses" below states the
//! tradeoff plainly instead.
//!
//! Test-module fixtures are excluded the same way `recovery_text.rs`
//! excludes them for `error.rs`: [`reason_strings`] only scans the portion
//! of each file before its first `#[cfg(test)]` marker (if any).
//!
//! ## Which spans: [`names_a_command`]
//!
//! `recovery_text.rs` extracts every backtick span in a candidate string and
//! keeps the ones whose first token does not start with `-` (rules out a
//! bare flag) and that contain no `:` (rules out a `field: value` mention).
//! Both filters are reused here unchanged — but `reason` text interpolates
//! runtime values far more heavily than `error.rs`'s static recovery table
//! ever does, and that surfaces a shape `recovery_text.rs` never had to
//! filter: a bare interpolation placeholder wrapped in backticks purely for
//! display, e.g.
//!
//! ```text
//! "the project lock was left by process {} during `{}` and is stale"
//! ```
//!
//! where the second span is a recorded operation name, not a command. Its
//! whole content is `{}` — no leading `-`, no `:` — so `recovery_text.rs`'s
//! two filters alone would wrongly keep it. [`names_a_command`] adds a
//! third: the first token must not start with `{` either. Every reason
//! string read while building this rule follows the same pattern — an
//! interpolated id, status, or branch name shown in backticks for
//! readability (`` `{gate_id}` ``, `` `{branch}` ``, `` `{}` ``) — never a
//! command whose own subcommand path is itself interpolated; only flag
//! *values* are ever interpolated in a real command reference (see
//! `gate.rs`'s `` `gate reserve --card-id {card_id} --gate-id {gate_id}` ``,
//! kept below). A future reason string that interpolated the subcommand
//! name itself would defeat this test in the other direction — it would be
//! *kept*, then fail as an unrecognized subcommand, for a reason that has
//! nothing to do with staleness — but no current site does this.
//!
//! A fourth condition, not in `recovery_text.rs` at all, is needed because
//! the three filters above still keep bare English words that happen to
//! start neither with `-` nor `{` and contain no `:`. `gate.rs` refuses an
//! unrecognized `--outcome` with a message naming its two accepted values,
//! `failed` and `abandoned`, each wrapped in backticks for readability; an
//! exception-resolution refusal elsewhere similarly names `continue` in
//! backticks. None of the three is a command — all are enum-style values —
//! and none is introduced by the words "run", "use", or "try". Meanwhile
//! genuine multi-word command references appear in this codebase after all
//! kinds of prose — "run", "use", "try", "install the first one with", or no
//! verb at all when the command is simply the grammatical subject of its
//! own clause, as in `disposition.rs`: "...; `disposition abandon` only
//! ends an active escalation...". No small set of verbs covers every one of
//! those; token count does, because a real subcommand path is always at
//! least two words (`work start`, `disposition abandon`), while a bare
//! status/outcome word is exactly one. The one exception is single-word
//! commands — `audit` is the one live example, always written as "run
//! `audit`" — which is why [`names_a_command`]'s fourth condition is an
//! *or*: keep the span if it has two or more whitespace-separated tokens,
//! **or** if it is immediately introduced by `run`, `Run`, `use`, or `try`
//! — the complete set of verbs actually found, by grep, introducing a
//! single bare command word in `reason` text today.
//!
//! (`whoami` does appear after "run" once, in `git/command.rs` — but inside
//! a code comment about shell-injection testing, describing the Unix
//! `whoami`, not a `reason` field at all; it never reaches
//! [`reason_strings`] and is not a command this CLI has. It is also the one
//! member of this card's own baseline count, reported below, that turned
//! out not to be a `reason` string on inspection.)
//!
//! This rule sees flags. A span like
//! `` `gate reserve --card-id {card_id} --gate-id {gate_id}` `` is kept as
//! one unit; every token, flags included, is passed to the built binary, so
//! a corrupted flag name fails exactly as a corrupted subcommand would (see
//! this card's evidence report for both mutations run against
//! [`every_command_named_in_a_reason_field_really_exists`], including the
//! corrupted-flag case at this exact site — the one that survived #105's
//! review). What it cannot see is a flag mentioned in isolation, without
//! the subcommand it belongs to — `` `--resume` `` in `project.rs`'s
//! `` `project recover` without `--resume` `` — because a bare flag fails
//! the leading-`-` filter by design, the same exclusion `recovery_text.rs`
//! documents for `` `--help` `` and `` `--invariant-holds` `` in `error.rs`.
//! Validating an isolated flag name against nothing (no subcommand context)
//! would not mean anything to `clap` regardless.
//!
//! # What this rule misses
//!
//! - **Anything not written as a literal where `reason:` is.** A value
//!   forwarded from elsewhere — `src/config/mod.rs`'s
//!   `reason: message` (`message` comes from `serde_json::Error::to_string`,
//!   a runtime value with no source literal to read at all), or
//!   `work.rs`'s `corrupt` closure (`|reason: String| HarnessError::Control
//!   { reason, .. }`, whose real value is whatever the caller passed in) —
//!   is invisible to a text scan. Same limitation `recovery_text.rs`
//!   documents for `error.rs`, restated for a much wider set of files.
//! - **The two `let reason = format!(...)` sites** named above
//!   (`acceptance.rs:592`, `integration.rs:3878`), a deliberate scope
//!   decision explained above rather than an oversight.
//! - **Per-site `recovery: &'static str` overrides that live outside a
//!   `reason` field.** `gate.rs` declares three —
//!   `ABANDON_LIVE_RESERVATION_RECOVERY`, `RESERVATION_ALREADY_ACQUIRED_RECOVERY`,
//!   `FINAL_INTEGRATION_GATE_RECOVERY` — and `status_gate_refusal` in
//!   `integration.rs`/`acceptance.rs` takes an inline recovery string as a
//!   plain function argument. These are `HarnessError::ControlWithRecovery`'s
//!   per-site sibling to `error.rs`'s shared table (see that variant's own
//!   doc comment, "a refusal that already knows its own fix"), and they
//!   name real commands in backticks — but neither `recovery_text.rs`
//!   (`error.rs` only) nor this file (`reason` only) reads them.
//!   `tests/per_site_recovery.rs` exercises one of the three
//!   (`FINAL_INTEGRATION_GATE_RECOVERY`) end to end through the real CLI, but
//!   only by copying its text verbatim into its own constant and asserting
//!   the two stay equal — proof the override plumbing works, not that
//!   `` `integration prepare` ``, `` `--cycle-id` ``, or `` `--actor-id` ``
//!   are still real; a rename landing in both places at once (the source
//!   constant and that test's copy) would satisfy it either way. All three
//!   `gate.rs` constants were confirmed current while writing this test
//!   (`gate abandon`; `gate settle` with `--reservation-id`/`--outcome`;
//!   `integration prepare` with `--cycle-id`/`--actor-id`), so nothing stale
//!   is going unreported today — but this is a real coverage gap, worth its
//!   own card rather than folded into this one under §6.
//!   `domain/integration.rs`'s `IntegrationStatus::transition_recovery` is
//!   the same shape of gap (a per-variant `&'static str` recovery message
//!   feeding `ControlWithRecovery`, outside any `reason` field) — almost
//!   certainly where this card's own §5 drew its `` `prepared` `` example
//!   from, since that exact word is one of its nine backtick-quoted state
//!   names. Its sibling test,
//!   `transition_recovery_names_every_successor_state`, checks the state
//!   names against `IntegrationStatus::successors`, the same "verbatim
//!   copy, not independent validation" shape as `per_site_recovery.rs`
//!   above — but there is nothing stale to report there either: every span
//!   in all nine messages is a state name, never a command, so this rule's
//!   own scope would have nothing to check even if it did read this
//!   function.
//! - **A single-word command introduced by some other verb.** If a future
//!   reason string named a one-word command without "run", "use", or "try"
//!   before it — "...see `doctor` for details" — this rule would exclude it
//!   (one token, shape otherwise fine, no recognized verb). Widening the
//!   verb list on spec, with nothing in the source to justify a specific
//!   addition, would just be guessing; the honest trade this rule makes is
//!   documented here rather than hidden behind an untested guess.
//!
//! # Scope of the scan
//!
//! Every `.rs` file under `src/` is read, except `error.rs` — that file's
//! recovery text is `recovery_text.rs`'s to check, per this card's
//! instruction not to change or generalize that file's rule.

use std::path::{Path, PathBuf};
use std::process::Command;

/// True for a Rust identifier character. Used to confirm a match is a whole
/// word, not the tail of a longer one (`blocked_reason:`, not `reason:`).
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Recursively collects every `.rs` file under `dir`, in the manner already
/// established by `capture_stdout_of_logs` in `tests/support/mod.rs`.
fn rust_files_under(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            rust_files_under(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// Every `.rs` file under `src/`, excluding `error.rs` (see the module doc
/// comment), relative to the repository root and in a stable order.
/// Filtering on file name alone is safe here: the repository has exactly
/// one file named `error.rs`.
fn source_files() -> Vec<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut absolute = Vec::new();
    rust_files_under(&manifest_dir.join("src"), &mut absolute);
    absolute.retain(|path| path.file_name().and_then(|name| name.to_str()) != Some("error.rs"));

    let mut files: Vec<PathBuf> = absolute
        .iter()
        .map(|path| {
            path.strip_prefix(manifest_dir)
                .unwrap_or(path)
                .to_path_buf()
        })
        .collect();
    files.sort();
    files
}

/// One `reason` string literal found in source, with enough location
/// information to name in a failure message.
struct ReasonString {
    /// Path to the file this literal was read from, relative to the
    /// repository root.
    file: PathBuf,
    /// 1-based line on which the literal's opening quote appears.
    line: usize,
    /// The literal's content, exactly as written (escapes not decoded —
    /// none of this source uses them; see the module doc comment).
    text: String,
}

/// Finds the end of a Rust string literal's content, given the text
/// immediately after its opening quote. Returns the content up to, but not
/// including, the first unescaped `"`, honoring `\`-escapes (including a
/// `\` immediately followed by a newline, this codebase's line-continuation
/// style for long reason strings).
fn closing_quote(content: &str) -> Option<&str> {
    let mut chars = content.char_indices();
    while let Some((idx, ch)) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            '"' => return Some(&content[..idx]),
            _ => {}
        }
    }
    None
}

/// The exact, word-bounded token [`reason_strings`] anchors on: a `reason`
/// struct field in literal-assignment position.
const REASON_FIELD: &str = "reason:";

/// Extracts every `reason`-field string literal from `source`: text written
/// as a bare `"..."` or as the first argument to `format!(...)`, wherever
/// it is the value in a `reason:` struct-literal position. See the module
/// doc comment for exactly what this can and cannot see.
fn reason_strings(file: &Path, source: &str) -> Vec<ReasonString> {
    // Test-module fixtures are not operator-facing guidance; excluded the
    // same way `recovery_text.rs` excludes them for `error.rs`.
    let scanned = source
        .split("#[cfg(test)]")
        .next()
        .expect("str::split always yields at least one piece");

    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = scanned[offset..].find(REASON_FIELD) {
        let abs = offset + rel;
        let boundary_ok = scanned[..abs]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ident_char(c));

        if boundary_ok {
            let after = &scanned[abs + REASON_FIELD.len()..];
            let trimmed = after.trim_start_matches([' ', '\t', '\n', '\r']);
            let literal = trimmed
                .strip_prefix("format!(")
                .map_or(trimmed, |x| x.trim_start_matches([' ', '\t', '\n', '\r']));

            if let Some(content_start) = literal.strip_prefix('"')
                && let Some(text) = closing_quote(content_start)
            {
                let line = scanned[..abs].matches('\n').count() + 1;
                out.push(ReasonString {
                    file: file.to_path_buf(),
                    line,
                    text: text.to_string(),
                });
            }
        }

        offset = abs + REASON_FIELD.len();
    }
    out
}

/// Extracts every backtick-delimited span from `text`, in source order,
/// paired with the byte offset (into `text`) where the span's own content
/// begins — i.e. just after the opening backtick. Mirrors
/// `recovery_text.rs`'s `backtick_spans`; see the module doc comment for why
/// this is an independent copy rather than shared code.
fn backtick_spans(text: &str) -> Vec<(usize, &str)> {
    let mut spans = Vec::new();
    let mut base = 0usize;
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('`') else {
            break;
        };
        let start = base + open + 1;
        spans.push((start, &after_open[..close]));
        rest = &after_open[close + 1..];
        base = start + close + 1;
    }
    spans
}

/// The verbs this codebase actually uses, by grep, to introduce a
/// single bare command word in `reason` text (see the module doc comment).
const COMMAND_VERBS: &[&str] = &["run", "Run", "use", "try"];

/// True when `haystack` ends with the standalone word `word`: `word` is a
/// suffix of `haystack`, and the character immediately before it (if any) is
/// not itself an identifier character. Rules out a coincidental substring
/// match — `"overuse"` does not end with the standalone word `"use"`.
fn ends_with_word(haystack: &str, word: &str) -> bool {
    haystack
        .strip_suffix(word)
        .is_some_and(|rest| rest.chars().next_back().is_none_or(|c| !is_ident_char(c)))
}

/// True when a command-introducing verb (see [`COMMAND_VERBS`]) is the last
/// word of `preceding_text` — the reason string's own text up to, but not
/// including, the backtick that opens the span under test.
fn preceded_by_command_verb(preceding_text: &str) -> bool {
    let trimmed = preceding_text.trim_end_matches(' ');
    COMMAND_VERBS
        .iter()
        .any(|verb| ends_with_word(trimmed, verb))
}

/// True when a backtick span, found inside a `reason` string, names a
/// `change-harness` command rather than a bare flag, a `field: value`
/// mention, a bare interpolation placeholder, or a bare word describing a
/// state or outcome. `preceding_text` is the span's own reason string, up to
/// (not including) its opening backtick — used only to check for a
/// command-introducing verb. See the module doc comment for the full
/// justification of each condition below.
fn names_a_command(span: &str, preceding_text: &str) -> bool {
    let first_token = span.split_whitespace().next().unwrap_or("");
    let shape_ok = !first_token.is_empty()
        && !first_token.starts_with('-')
        && !first_token.starts_with('{')
        && !span.contains(':');

    shape_ok && (span.split_whitespace().count() >= 2 || preceded_by_command_verb(preceding_text))
}

#[test]
fn every_command_named_in_a_reason_field_really_exists() {
    let files = source_files();

    // A file-walk that silently found nothing would let this test pass
    // vacuously regardless of what `src/` contains — worse than no test.
    assert!(
        !files.is_empty(),
        "found zero .rs files under src/ (excluding error.rs); the walk in `source_files` no \
         longer matches the repository layout"
    );

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut all_strings = Vec::new();
    for file in &files {
        let absolute = manifest_dir.join(file);
        let source = std::fs::read_to_string(&absolute)
            .unwrap_or_else(|e| panic!("{} should be readable: {e}", absolute.display()));
        all_strings.extend(reason_strings(file, &source));
    }

    assert!(
        !all_strings.is_empty(),
        "found zero `reason:` string literals under src/ (excluding error.rs); the extraction \
         rule documented at the top of this file no longer matches how HarnessError::Control and \
         HarnessError::ControlWithRecovery are actually constructed"
    );

    let mut checked = 0usize;
    for entry in &all_strings {
        for (span_start, span) in backtick_spans(&entry.text) {
            let preceding = &entry.text[..span_start - 1];
            if !names_a_command(span, preceding) {
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
                "reason text in {}:{} names `{span}`, which is not a real command shape (exit \
                 {:?}):\nfull string: {:?}\nstdout: {}\nstderr: {}",
                entry.file.display(),
                entry.line,
                output.status.code(),
                entry.text,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }

    // Same vacuity guard as above, one level down: even with reason strings
    // found, a `names_a_command` that silently excluded everything would
    // leave the loop above a no-op that never runs the binary at all.
    assert!(
        checked > 0,
        "zero reason strings named a command by the rule this test uses; either every reason \
         string really is command-free, which would make this test unable to ever fail, or the \
         extraction rule stopped matching text that is actually there"
    );
}

/// A bare interpolation placeholder, wrapped in backticks purely for
/// display (an id, a branch name, a status), is not a command — see the
/// module doc comment's `` `{}` `` example from `control/lock.rs`.
#[test]
fn bare_interpolation_placeholder_is_not_a_command() {
    assert!(!names_a_command("{}", "during "));
    assert!(!names_a_command("{card_id}", "card "));
}

/// A bare flag, mentioned without the subcommand it belongs to, is not a
/// command — mirrors `recovery_text.rs`'s documented `` `--help` `` /
/// `` `--invariant-holds` `` exclusion for `error.rs`.
#[test]
fn bare_flag_is_not_a_command() {
    assert!(!names_a_command("--final", "cannot use "));
    assert!(!names_a_command("--mode individual", "cannot use "));
}

/// A `field: value` mention is not a command — the same exclusion
/// `recovery_text.rs` documents for `` `human_reviewer: true` ``.
#[test]
fn field_value_mention_is_not_a_command() {
    assert!(!names_a_command("human_reviewer: true", "set "));
    assert!(!names_a_command("reason_category: {}", "declares "));
}

/// A single bare word describing a state or outcome, with no
/// command-introducing verb before it, is not a command — `` `failed` ``
/// and `` `abandoned` `` from `gate.rs`'s `--outcome must be ...` refusal.
#[test]
fn bare_single_word_without_a_verb_is_not_a_command() {
    assert!(!names_a_command("failed", "--outcome must be "));
    assert!(!names_a_command("abandoned", "must be `failed` or "));
    assert!(!names_a_command("continue", "resolution "));
}

/// A single bare word IS a command when one of the recognized verbs
/// introduces it — `` `audit` `` after "run", the one live example.
#[test]
fn single_word_command_after_a_recognized_verb_is_a_command() {
    assert!(names_a_command("audit", "problems? run "));
    assert!(names_a_command("audit", "problems? Run "));
}

/// A multi-word span is a command even with no introducing verb at all,
/// when it is the grammatical subject of its own clause — mirrors
/// `disposition.rs`'s `` `disposition abandon` only ends an active
/// escalation ``.
#[test]
fn multi_word_span_is_a_command_even_without_a_verb() {
    assert!(names_a_command("disposition abandon", "...; "));
    assert!(names_a_command(
        "gate reserve --card-id {card_id} --gate-id {gate_id}",
        "one live validation reservation; run "
    ));
}

/// [`reason_strings`] anchors on `reason:` (struct-literal field position)
/// and deliberately does not follow `let reason = format!(...)` — see the
/// module doc comment's "This one anchor is deliberately narrower..."
/// paragraph. This test pins that boundary so a future edit cannot widen or
/// narrow it without this test noticing.
#[test]
fn reason_field_literal_is_found_but_a_let_binding_is_not() {
    let source = r#"
fn refuse() -> Result<(), HarnessError> {
    let reason = format!("run `disposition rebaseline` first");
    Err(HarnessError::Control {
        reason: format!("run `work start` first"),
        code: ErrorCode::PreconditionNotFound,
    })
}
"#;
    let found = reason_strings(Path::new("synthetic.rs"), source);
    let texts: Vec<&str> = found.iter().map(|entry| entry.text.as_str()).collect();
    assert_eq!(texts, vec!["run `work start` first"]);
}

/// Test-module fixtures are excluded: [`reason_strings`] only scans the
/// portion of a file before its first `#[cfg(test)]` marker, the same
/// discipline `recovery_text.rs` documents for `error.rs`.
#[test]
fn cfg_test_module_content_is_excluded() {
    let source = r#"
fn refuse() -> Result<(), HarnessError> {
    Err(HarnessError::Control {
        reason: format!("run `work start` first"),
        code: ErrorCode::PreconditionNotFound,
    })
}

#[cfg(test)]
mod tests {
    fn a_fixture() -> HarnessError {
        HarnessError::Control {
            reason: format!("run `card create` first"),
            code: ErrorCode::PreconditionNotFound,
        }
    }
}
"#;
    let found = reason_strings(Path::new("synthetic.rs"), source);
    let texts: Vec<&str> = found.iter().map(|entry| entry.text.as_str()).collect();
    assert_eq!(texts, vec!["run `work start` first"]);
}

/// `error.rs` is excluded from the scan — its recovery text belongs to
/// `recovery_text.rs`, per this card's instruction not to change or
/// generalize that file's rule (§6). Pinned directly rather than left to
/// the main test to notice indirectly.
#[test]
fn source_files_excludes_error_rs_but_not_ordinary_source() {
    let files = source_files();
    assert!(
        !files
            .iter()
            .any(|path| path.file_name().and_then(|n| n.to_str()) == Some("error.rs")),
        "source_files() must exclude error.rs"
    );
    assert!(
        files.iter().any(|path| path.ends_with("commands/gate.rs")),
        "source_files() should still include ordinary source files"
    );
}
