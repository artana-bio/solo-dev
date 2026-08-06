//! Confirms every command reference written into a per-site `recovery:
//! &'static str` override — the mechanism #106 added specifically so a
//! refusal that "already knows its own fix" can say so without disturbing
//! `error.rs`'s shared table (see `HarnessError::ControlWithRecovery`'s own
//! doc comment) — actually exists on the CLI.
//!
//! # Why this surface needed its own card
//!
//! `tests/recovery_text.rs` reads `error.rs`'s shared table.
//! `tests/reason_text.rs` reads `reason:` fields. Neither reads a per-site
//! `recovery:` override, because neither was built to: `recovery_text.rs`
//! opens exactly one file (`error.rs`); `reason_text.rs` anchors on the
//! literal token `reason:`, which a `recovery:` override never contains.
//! This was found, not guessed: `reason_text.rs`'s own module doc names the
//! gap directly, and it was confirmed live by corrupting `` `gate settle` ``
//! inside two of these overrides and running both existing tests —
//! `cargo test --test reason_text --test recovery_text` — which passed,
//! 10/10 and 1/1, against text that was wrong.
//!
//! This matters more than an ordinary blind spot because of what #106 did:
//! it created this override mechanism specifically so *more* sites would
//! carry their own recovery text instead of falling back to `error.rs`'s
//! generic table, and this wave has already put `gate settle`, `gate
//! abandon`, `gate run`, and `integration prepare` there. It is the newest
//! and least-guarded operator-facing text in the codebase — the exact rot
//! this contract exists to prevent, sitting in the code #106 wrote to fix
//! the problem this contract's own motivating incident (#105) came from.
//!
//! # Why a third sibling file, not an extension of either existing one
//!
//! Folding this into `recovery_text.rs` would mean either changing its
//! existing, single-purpose, already-reviewed rule for `error.rs` (which
//! the original card for this contract was explicitly told not to do), or
//! bolting a second, structurally different scan onto the same file behind
//! the same module doc — exactly the "generalizing it into something that
//! serves neither" outcome that card's own instructions warned against.
//! Folding it into `reason_text.rs` is worse: that file's name, its own
//! extensive module doc, and its nine dedicated boundary tests all say
//! `reason:` fields, specifically, and this text is never written at a
//! `reason:` position — it would not even be found by that file's anchor.
//! A third file keeps every existing test byte-for-byte untouched (verified
//! below) and gives a third, syntactically distinct surface a home shaped
//! for it, the same argument `reason_text.rs` itself already made for being
//! a sibling rather than an extension.
//!
//! # The shapes this file actually found (verified, not assumed)
//!
//! Grepping for `recovery:` across `src/` (excluding `error.rs`, and
//! excluding `cli/output.rs`'s generic envelope-rendering field, which just
//! forwards whatever `HarnessError::recovery()` already returns) turns up
//! exactly two shapes that write a literal:
//!
//! 1. **Three named constants**, all in `gate.rs`:
//!    `ABANDON_LIVE_RESERVATION_RECOVERY`, `RESERVATION_ALREADY_ACQUIRED_RECOVERY`,
//!    `FINAL_INTEGRATION_GATE_RECOVERY` — each `const NAME: &str = "...";`,
//!    each assigned once via `recovery: NAME,`. (Not "three sites, four
//!    declarations" — that earlier count was a grep guess, offered as one,
//!    and it was wrong in the same direction this whole contract keeps
//!    finding: an undercount of declarations relative to sites, when here
//!    they are 1:1.)
//! 2. **Two inline literals**, passed as the third, positional argument to
//!    `status_gate_refusal` (`integration.rs`, `acceptance.rs`) — never
//!    bound to a name at all.
//!
//! A third candidate, `domain/integration.rs`'s `transition_recovery`, is
//! deliberately **not** scanned — see the next section for why that is a
//! finding in its own right, not an oversight.
//!
//! # `transition_recovery`: excluded on purpose, not missed
//!
//! `transition_recovery`'s own doc comment states the design outright:
//! "Recovery guidance for a refused transition: the states this one may
//! still reach, **not a command**." The paragraph below it explains why —
//! the function guards every integration transition, not one command's
//! precondition, so it cannot hardcode any single command's name without
//! smuggling CLI vocabulary into a domain type that is deliberately kept
//! ignorant of it. All nine of its match arms, read in full, obey this:
//! every backtick span across all nine is a state name (`` `draft` ``,
//! `` `prepared` ``, `` `blocked` ``, …), never a command — confirmed by
//! hand, not inferred from the doc comment alone. That is almost certainly
//! where this contract's own §5 drew its `` `prepared` `` example from.
//!
//! Scanning it would need a third extraction mechanism — brace-depth
//! bounding a function body to pull nine match-arm literals out, rather
//! than a single literal after a name or a call — for a surface the
//! codebase's own architecture guarantees will never contain what this
//! file checks for. That is speculative complexity in exchange for zero
//! live protection: if this boundary is ever deliberately relaxed, the
//! commit that relaxes it should add coverage here alongside it, not have
//! this file guess ahead of a design decision that has not been made.
//!
//! # Does `names_a_command` need adjusting for this text?
//!
//! Asked directly, answered by testing rather than by inspection: **no**,
//! the classification function itself — reused byte-for-byte from
//! `tests/reason_text.rs`, not touched — sorts every span in all five real
//! recovery texts correctly with no change:
//!
//! - `` `gate abandon` `` opens `ABANDON_LIVE_RESERVATION_RECOVERY`'s text
//!   as the literal first four characters of the string — no preceding
//!   text at all, let alone a `run`/`use`/`try` verb — and is still kept,
//!   because it has two tokens. The verb branch was never exercised here;
//!   the token-count branch alone was enough.
//! - `` `gate run` `` in `FINAL_INTEGRATION_GATE_RECOVERY` is introduced by
//!   "through", a preposition outside the four-verb list — kept for the
//!   same reason, two tokens.
//! - `` `--reservation-id` ``, `` `--cycle-id` ``, `` `--actor-id` `` are
//!   correctly excluded as bare flags, exactly as `` `--resume` `` already
//!   is in `reason_text.rs`'s own population.
//!
//! What *did* need something new is not classification but **collection**.
//! `reason` text's one flag-bearing example bundles command and flags in a
//! single span — `` `gate reserve --card-id {card_id} --gate-id {gate_id}` ``
//! — so seeing the flag was free once the span was kept. Recovery prose
//! does not write that way: "Run `gate settle` with `--reservation-id`
//! (the id named above) and `--outcome abandoned` to end it now." backticks
//! the command and each flag **separately**. A flag in its own span fails
//! `names_a_command` by design (the same bare-flag exclusion
//! `recovery_text.rs` documents for `` `--help` ``) — which is exactly why
//! corrupting `--reservation-id` alone, unpaired, would still slip past
//! completely unseen, mutation or not. [`runnable_invocations`] is the
//! fix: it tracks the most recently *kept* command span while walking a
//! text's spans in order, and folds a following bare-flag span into that
//! command before running it — the flag is validated in the CLI context it
//! reads as belonging to, not as an orphaned token.
//!
//! A single bare flag with no value in its own span (`` `--reservation-id` ``)
//! is rewritten `--reservation-id=x` before running — confirmed necessary,
//! not assumed: `gate settle --reservation-id --help` fails with a "value
//! is required" error naming the flag, regardless of whether the flag
//! itself is even spelled correctly, because `clap` will not consume a
//! token starting with `-` (here, `--help`) as an ordinary value. The `=`
//! form sidesteps that ambiguity entirely. A flag whose span already
//! carries its value (`` `--outcome abandoned` ``) is used as written.
//!
//! # What this still misses
//!
//! - **`transition_recovery`**, on purpose — see above.
//! - **A future helper with `status_gate_refusal`'s shape but a different
//!   name.** The anchor is that literal function name; a second such
//!   helper would need a second anchor added here.
//! - **A future `*_RECOVERY` constant with a name that does not end in
//!   `_RECOVERY`.** All three that exist today do; nothing enforces that a
//!   fifth one must.
//! - **A flag backticked with no command anywhere earlier in the same
//!   text.** [`runnable_invocations`] only pairs forward from a command
//!   already seen; a flag mentioned before any command, or in a text with
//!   no command at all, has nothing to pair with and is not run. No
//!   current text does this.
//! - Every limitation `reason_text.rs` already documents for
//!   `names_a_command` itself (bare interpolation placeholders, `field:
//!   value` mentions, a single-word command introduced by an unlisted
//!   verb) applies here unchanged, since the function is unchanged.
//!
//! # Required mutations (this card's evidence, not part of the suite)
//!
//! 1. Corrupt a subcommand inside a named constant —
//!    `` `gate settle` `` → `` `gate settl` `` — fails, naming the site.
//! 2. Corrupt a flag inside one — `--reservation-id` → `--reservation-idd`
//!    — fails via [`runnable_invocations`]'s pairing, naming the site. This
//!    is the one that matters: it is the shape that survived both existing
//!    tests when tried by hand.
//!
//! Both run by hand, reverted, `git status --short` clean before and
//! after — see this card's evidence report, not a test in this file (a
//! test that mutates `src/` to prove itself would violate the frozen "no
//! source files" scope this whole contract runs under).

use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------
// Duplicated verbatim from `tests/reason_text.rs`: the classification
// rule (`names_a_command` and everything it depends on) and the source
// file walk. See that file's own module doc for why duplication, not a
// shared module, is the deliberate choice for this trio of sibling files.
// Not one line here differs from its counterpart in `reason_text.rs`.
// ---------------------------------------------------------------------

/// True for a Rust identifier character.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Recursively collects every `.rs` file under `dir`.
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

/// Every `.rs` file under `src/`, excluding `error.rs` (`recovery_text.rs`
/// owns that file's recovery text), relative to the repository root and in
/// a stable order.
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

/// Finds the end of a Rust string literal's content, given the text
/// immediately after its opening quote.
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

/// Extracts every backtick-delimited span from `text`, paired with the
/// byte offset where the span's own content begins.
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

/// The verbs this codebase actually uses to introduce a single bare
/// command word.
const COMMAND_VERBS: &[&str] = &["run", "Run", "use", "try"];

/// True when `haystack` ends with the standalone word `word`.
fn ends_with_word(haystack: &str, word: &str) -> bool {
    haystack
        .strip_suffix(word)
        .is_some_and(|rest| rest.chars().next_back().is_none_or(|c| !is_ident_char(c)))
}

/// True when a command-introducing verb is the last word of
/// `preceding_text`.
fn preceded_by_command_verb(preceding_text: &str) -> bool {
    let trimmed = preceding_text.trim_end_matches(' ');
    COMMAND_VERBS
        .iter()
        .any(|verb| ends_with_word(trimmed, verb))
}

/// True when a backtick span names a `change-harness` command. See
/// `tests/reason_text.rs` for the full justification of each condition;
/// unchanged here.
fn names_a_command(span: &str, preceding_text: &str) -> bool {
    let first_token = span.split_whitespace().next().unwrap_or("");
    let shape_ok = !first_token.is_empty()
        && !first_token.starts_with('-')
        && !first_token.starts_with('{')
        && !span.contains(':');

    shape_ok && (span.split_whitespace().count() >= 2 || preceded_by_command_verb(preceding_text))
}

// ---------------------------------------------------------------------
// New: extraction for the `recovery:` override shapes. Nothing below
// this point exists in `reason_text.rs`.
// ---------------------------------------------------------------------

/// One recovery-override string literal found in source.
struct RecoveryString {
    /// Path to the file this literal was read from, relative to the
    /// repository root.
    file: PathBuf,
    /// 1-based line of the anchor that found this literal — the `const`
    /// declaration's line, or the `status_gate_refusal(` call's line. Not
    /// necessarily the literal's own line when the two differ (a call
    /// wrapped across several lines reports the line the call opens on).
    line: usize,
    /// The literal's content, exactly as written.
    text: String,
}

/// The suffix every current per-site recovery constant's declaration
/// shares: a name ending in `_RECOVERY`, typed `&str`, assigned a literal
/// directly. See the module doc comment for what a differently-named
/// constant would mean for this anchor.
const RECOVERY_CONST_SUFFIX: &str = "_RECOVERY: &str = \"";

/// Extracts every `const <NAME>_RECOVERY: &str = "...";` literal from
/// `source`.
fn named_recovery_constants(file: &Path, source: &str) -> Vec<RecoveryString> {
    let scanned = source
        .split("#[cfg(test)]")
        .next()
        .expect("str::split always yields at least one piece");

    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = scanned[offset..].find(RECOVERY_CONST_SUFFIX) {
        let abs = offset + rel;
        let quote_at = abs + RECOVERY_CONST_SUFFIX.len();
        if let Some(text) = closing_quote(&scanned[quote_at..]) {
            let line = scanned[..abs].matches('\n').count() + 1;
            out.push(RecoveryString {
                file: file.to_path_buf(),
                line,
                text: text.to_string(),
            });
        }
        offset = quote_at;
    }
    out
}

/// Returns the text strictly between the `(` immediately after
/// `anchor_end` and its matching `)`, tracking nested parens and skipping
/// over string-literal content byte-for-byte (so a literal that happened
/// to contain `(` or `)` could not desynchronize the depth count — none
/// currently does, but this does not assume that stays true). Returns
/// `None` if `anchor_end` is not immediately followed by `(`, or the
/// matching `)` is never found.
fn call_arguments(source: &str, anchor_end: usize) -> Option<&str> {
    let rest = &source[anchor_end..];
    if !rest.starts_with('(') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut depth = 1i32;
    let mut i = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let content = closing_quote(&rest[i + 1..])?;
                i += 1 + content.len() + 1;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[1..i]);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// The function name [`status_gate_refusal_arguments`] anchors on.
/// Matches the function's own definition as well as both call sites — the
/// definition's parameter list contains no string literal, so the "only
/// keep it if a quote turns up inside the parens" check below silently
/// excludes it without needing to tell the two apart syntactically.
const STATUS_GATE_REFUSAL: &str = "status_gate_refusal";

/// Extracts the literal third argument from every `status_gate_refusal(
/// reason, record, "...")` call in `source`.
fn status_gate_refusal_arguments(file: &Path, source: &str) -> Vec<RecoveryString> {
    let scanned = source
        .split("#[cfg(test)]")
        .next()
        .expect("str::split always yields at least one piece");

    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = scanned[offset..].find(STATUS_GATE_REFUSAL) {
        let abs = offset + rel;
        let after_name = abs + STATUS_GATE_REFUSAL.len();
        if let Some(args) = call_arguments(scanned, after_name)
            && let Some(quote_rel) = args.find('"')
            && let Some(text) = closing_quote(&args[quote_rel + 1..])
        {
            let line = scanned[..abs].matches('\n').count() + 1;
            out.push(RecoveryString {
                file: file.to_path_buf(),
                line,
                text: text.to_string(),
            });
        }
        offset = after_name;
    }
    out
}

/// Every per-site `recovery:` override literal found in `source`, across
/// both shapes this file scans. See the module doc comment for why
/// `transition_recovery` is a third, deliberately unscanned shape.
fn recovery_override_strings(file: &Path, source: &str) -> Vec<RecoveryString> {
    let mut out = named_recovery_constants(file, source);
    out.extend(status_gate_refusal_arguments(file, source));
    out
}

/// True when `span` looks like a flag mention on its own — the shape
/// [`names_a_command`] already excludes, and the shape
/// [`runnable_invocations`] tries to pair with a preceding command instead
/// of leaving unchecked.
fn looks_like_a_flag(span: &str) -> bool {
    let first_token = span.split_whitespace().next().unwrap_or("");
    !first_token.is_empty() && first_token.starts_with('-') && !span.contains(':')
}

/// Every runnable invocation implied by `text`'s backtick spans, in the
/// order the spans appear: each kept command span on its own, plus each
/// bare-flag span folded into the most recently kept command span before
/// it in the same text. See the module doc comment's "Does
/// `names_a_command` need adjusting" section for why this pairing exists
/// and why a lone bare flag is rewritten `--flag=x` before use.
fn runnable_invocations(text: &str) -> Vec<Vec<String>> {
    let mut invocations = Vec::new();
    let mut current_command: Option<Vec<&str>> = None;

    for (span_start, span) in backtick_spans(text) {
        let preceding = &text[..span_start - 1];

        if names_a_command(span, preceding) {
            let tokens: Vec<&str> = span.split_whitespace().collect();
            invocations.push(tokens.iter().map(|t| (*t).to_string()).collect());
            current_command = Some(tokens);
            continue;
        }

        if looks_like_a_flag(span)
            && let Some(command) = &current_command
        {
            let mut args: Vec<String> = command.iter().map(|t| (*t).to_string()).collect();
            let flag_tokens: Vec<&str> = span.split_whitespace().collect();
            if let [only] = flag_tokens.as_slice() {
                args.push(format!("{only}=x"));
            } else {
                args.extend(flag_tokens.iter().map(|t| (*t).to_string()));
            }
            invocations.push(args);
        }
    }

    invocations
}

#[test]
fn every_command_named_in_a_recovery_override_really_exists() {
    let files = source_files();
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
        all_strings.extend(recovery_override_strings(file, &source));
    }

    assert!(
        !all_strings.is_empty(),
        "found zero per-site recovery-override string literals under src/; either every override \
         was removed, or the extraction rules documented at the top of this file no longer match \
         how they are written (a renamed helper, a constant no longer ending in `_RECOVERY`, ...)"
    );

    let mut checked = 0usize;
    for entry in &all_strings {
        for args in runnable_invocations(&entry.text) {
            checked += 1;

            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
                .args(&arg_refs)
                .arg("--help")
                .output()
                .expect("the CLI binary should start");

            assert!(
                output.status.success(),
                "recovery override text in {}:{} runs as `{}`, which is not a real command shape \
                 (exit {:?}):\nfull string: {:?}\nstdout: {}\nstderr: {}",
                entry.file.display(),
                entry.line,
                args.join(" "),
                output.status.code(),
                entry.text,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }

    assert!(
        checked > 0,
        "zero recovery-override strings produced a runnable invocation; either every override \
         really is command-free, which would make this test unable to ever fail, or the \
         extraction/pairing rules stopped matching text that is actually there"
    );
}

/// [`named_recovery_constants`] finds a `const ..._RECOVERY: &str = "...";`
/// literal and reports it under the constant's own declaration line.
#[test]
fn named_recovery_constants_finds_the_declared_literal() {
    let source = r#"
const SOME_THING_RECOVERY: &str = "Run `gate settle` with `--reservation-id`.";
"#;
    let found = named_recovery_constants(Path::new("synthetic.rs"), source);
    let texts: Vec<&str> = found.iter().map(|entry| entry.text.as_str()).collect();
    assert_eq!(texts, vec!["Run `gate settle` with `--reservation-id`."]);
}

/// [`status_gate_refusal_arguments`] finds a call site's literal third
/// argument, and does not mistake the function's own definition (whose
/// parameter list has no string literal) for one.
#[test]
fn status_gate_refusal_arguments_skips_the_definition() {
    let source = r#"
pub(crate) fn status_gate_refusal(
    reason: String,
    record: &IntegrationRecord,
    unverified_recovery: &'static str,
) -> HarnessError {
    HarnessError::ControlWithRecovery { reason, code: ErrorCode::PolicyInvalidTransition, recovery: unverified_recovery }
}

fn caller() {
    status_gate_refusal(reason, record, "Run `integration verify` before promotion can be authorized.");
}
"#;
    let found = status_gate_refusal_arguments(Path::new("synthetic.rs"), source);
    let texts: Vec<&str> = found.iter().map(|entry| entry.text.as_str()).collect();
    assert_eq!(
        texts,
        vec!["Run `integration verify` before promotion can be authorized."]
    );
}

/// A bare flag span pairs with the most recently kept command span before
/// it, and a value-less flag is rewritten `--flag=x`.
#[test]
fn runnable_invocations_pairs_a_bare_flag_with_the_preceding_command() {
    let text = "`gate abandon` only ends an expired reservation. Run `gate settle` with `--reservation-id` \
         and `--outcome abandoned` to end it now.";
    let invocations = runnable_invocations(text);
    let joined: Vec<String> = invocations.iter().map(|args| args.join(" ")).collect();
    assert_eq!(
        joined,
        vec![
            "gate abandon",
            "gate settle",
            "gate settle --reservation-id=x",
            "gate settle --outcome abandoned",
        ]
    );
}

/// A bare flag with no command mentioned earlier in the same text has
/// nothing to pair with and produces no invocation — documented as a
/// limitation in the module doc comment, pinned here so it cannot regress
/// into a silent false pass.
#[test]
fn runnable_invocations_does_not_pair_a_flag_with_no_preceding_command() {
    let text = "Provide `--reservation-id` before retrying.";
    let invocations = runnable_invocations(text);
    assert!(
        invocations.is_empty(),
        "expected no invocations (no command precedes the flag), got {invocations:?}"
    );
}
