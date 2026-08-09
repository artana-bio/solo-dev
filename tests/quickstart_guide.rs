//! Pins `QUICKSTART.md` — the newcomer's hands-on walkthrough, second
//! document opened after `README.md` — against the exact class of defect
//! Contract 206 exists to prevent: a guide that reads correctly, parses as a
//! real command, and still strands the reader at runtime.
//!
//! Contract 206 §2. `README.md:272` documented a `gate run` refused since
//! #105 — for two releases, uncaught, because nothing checked the document
//! against the live binary at all. `tests/readme_guide.rs` and
//! `tests/skill_guide_execution.rs` then proved a second, narrower thing:
//! **`--help` alone is not enough**, because `clap` validates syntax and lets
//! `--help` short-circuit before any flag's value is ever acted on, so a
//! `gate run` missing `--reservation-id` — syntactically perfect, since the
//! flag is `Option<String>` — parses clean and still walks an operator into
//! `CH-POLICY-*` refusal. Verified directly against this repository's own
//! built binary while writing this file, the same way `tests/readme_guide.rs`
//! documents doing it:
//!
//! ```text
//! $ change-harness gate run --card-id F-001 --gate-id gate.unit --help
//! (exit 0)
//! ```
//!
//! So, like `tests/readme_guide.rs`, this file checks two properties, by two
//! different mechanisms, plus one narrow real execution:
//!
//! - **Shape** ([`quickstart_commands_are_real`]) — every `change-harness`
//!   line anywhere in `QUICKSTART.md` is a real subcommand with real flags.
//!   Cheap, catches a rename, and — per the trap above — provably
//!   insufficient alone.
//! - **Sequence** — two independent ordering claims the guide's own prose
//!   singles out as the traps that actually cost a reader time:
//!   - [`quickstart_never_documents_an_unreserved_gate_run`]: `gate reserve`
//!     before `gate run`, the same consumption-tracked rule
//!     `tests/readme_guide.rs` uses for `README.md`, adapted here (see
//!     "What this file's extraction adds" below).
//!   - [`quickstart_registers_enough_gates_before_the_card_names_both_stages`]:
//!     step 2's own words — "A gate can occupy only one validation stage,
//!     and the validation policy requires the final-integration stage to
//!     have at least one gate. So one gate is never enough." — turned into a
//!     structural check: by the time the card's `named_gates` snippet
//!     populates more than one stage, at least that many gates must already
//!     have been registered earlier in the document.
//! - **Execution**, scoped to one step
//!   ([`documented_step_6_gate_sequence_executes_in_the_order_written`]) —
//!   Contract 206 §7's proportionality call; see "Why one step, and which
//!   one" below.
//!
//! # What this file's extraction adds beyond `tests/readme_guide.rs`
//!
//! `README.md`'s "## Operator workflow" never redirects a command's output
//! to a file and never trails a shell comment on a `change-harness` line.
//! `QUICKSTART.md` does both, deliberately — `change-harness gate example >
//! gate.yaml` is exactly how a reader is meant to use the generator commands,
//! and `--actor-id coordinator   # -> INT-001` documents what a reader
//! should expect to see. Both are real shell syntax a reader's own terminal
//! would interpret, and neither is a `change-harness` argument:
//!
//! ```text
//! $ change-harness gate example > gate.yaml --help
//! error: unexpected argument '>' found
//! ```
//!
//! A naive port of `tests/readme_guide.rs`'s tokenizer would append `--help`
//! *after* the `>`/`#` token and fail every such line for a reason that has
//! nothing to do with whether the documented command is real —
//! [`truncate_at_shell_metachar`] stops token collection at the first `>`,
//! `>>`, `<`, `;`, `|`, `&&`, `||`, or `#`-prefixed token, mirroring what a
//! reader's own shell would do before `change-harness` ever sees the rest of
//! the line. Confirmed against the built binary while writing this file: all
//! 29 `change-harness` invocations in today's `QUICKSTART.md` pass `--help`
//! once this truncation is applied, and 8 of the 29 spuriously fail without
//! it — this is a real parsing requirement this document creates, not a
//! defect in the document.
//!
//! Continuation joining (a `\` at end of line) is unchanged from
//! `tests/readme_guide.rs`: `QUICKSTART.md`'s step 1 `project init` and step
//! 6 `gate run` both wrap onto a second line the identical way README's
//! "Three repositories" section does.
//!
//! # Placeholder paths and illustrative ids (Contract 206 §6)
//!
//! `QUICKSTART.md` writes `/abs/path/to/myapp`-shaped placeholder paths in
//! step 1 and reuses illustrative ids (`F-001`, `gate.unit`, `VR-000001`,
//! `C-001`, `INT-001`, ...) throughout, the same convention `README.md` and
//! `SKILL.md` both use. Decision: **no substitution and no exemption list.**
//! Both mechanisms this file uses are structurally blind to whether a value
//! is realistic, by construction rather than by an added carve-out:
//! `--help` (Shape) never dereferences a flag's value at all — clap resolves
//! `--help` before any argument is opened, stat'd, or looked up — and the
//! Sequence rules below compare flag values only by **token equality across
//! lines** (does this `--gate-id` match that one), never against a
//! filesystem or a live control repository. The main Shape test running
//! clean against today's real, placeholder-laden text is the live proof of
//! this: if either mechanism secretly cared about realism, that test would
//! already be failing.
//!
//! The failure mode this leaves, made loud rather than silent: a command
//! whose *only* defect is a wrong-but-plausible value — a path that does not
//! exist, an id that does not match what the real product would actually
//! allocate — is invisible to both Shape and Sequence. Only real execution
//! catches that, and this file only executes one step (next section). This
//! is the same trade `tests/readme_guide.rs` makes and names in its own
//! "What the sequence rule misses" section.
//!
//! # Why one step, and which one (Contract 206 §7)
//!
//! The strongest version of this card would drive `QUICKSTART.md`'s entire
//! arc, adopt through `Land INT-001`, for real. Judged disproportionate, for
//! reasons specific to *this* guide, not a restatement of
//! `tests/readme_guide_execution.rs`'s:
//!
//! - The general mechanism — an empty project reaching a promoted, archived
//!   commit using only harness commands — is already proven twice by
//!   `tests/lifecycle.rs`, which `README.md`'s own "Operator workflow"
//!   section cites for the identical claim. A second full-arc fixture here
//!   would re-spend a comparable amount of wall clock (the suite already
//!   runs 15+ minutes under load) proving the general mechanism a third
//!   time, not proving anything new about `QUICKSTART.md`'s specific prose.
//! - Several steps require content `QUICKSTART.md` never shows verbatim —
//!   step 4's full `draft.yaml` (only a `named_gates` fragment is shown),
//!   step 7's full `decl.yaml` (only field-by-field instructions) — the same
//!   "unshown file" obstacle `tests/readme_guide.rs`'s own module doc
//!   catalogues for `README.md`'s equivalent steps. Fabricating that content
//!   to reach further would mean asserting the fixture's own invention
//!   executes cleanly, not that the guide's text does.
//!
//! What **is** fully concrete, with no unshown file and no unresolved
//! variable, is step 6: `gate reserve` then `gate run`, the exact pairing
//! `tests/skill_guide_execution.rs` already proves this mechanism for
//! against `SKILL.md`, and — because it is the literal shape of #144/#105 —
//! the single highest-value step to prove for real rather than only
//! structurally. [`documented_step_6_gate_sequence_executes_in_the_order_written`]
//! runs it, unmodified, against a live fixture, the same way
//! `tests/skill_guide_execution.rs` runs `SKILL.md`'s narrow-feature-claim
//! step: no `--help`, no injected flag, `CHANGE_HARNESS_CONTROL` supplied
//! through the environment exactly as step 1's `export` line tells a reader
//! to rely on (neither of step 6's two lines writes `--control` at all).
//!
//! Left unverified by real execution, named rather than silently assumed:
//! step 1 (`project init`, placeholder paths never resolved to a real path
//! and run); step 2's two `gate register` calls beyond what
//! [`quickstart_registers_enough_gates_before_the_card_names_both_stages`]
//! checks structurally (this file never actually pipes a real `gate.yaml`
//! through the documented `sed` line and registers the result); steps 3-5
//! (built directly as fixture state for step 6, the same "not parsed from
//! the guide's own text" approach `tests/skill_guide_execution.rs` documents
//! for its identical steps 1-4); step 7 (unshown `decl.yaml` content); step 8
//! (`verdict.yaml` **is** shown in full, but it pairs with a `farewell`/
//! `greet` example specific to whatever toy project the guide's own author
//! actually ran against — `tests/readme_guide_execution.rs` already proves
//! this exact `review begin`/`review record`/`review example` mechanism
//! works on this CLI, against a different document, so re-proving the
//! identical mechanism a second time here was judged to buy little for a
//! second fixture's cost); step 9 (the integrate-through-promote arc,
//! generically covered by `tests/lifecycle.rs`, and every flag name in it is
//! already re-verified live by the Shape check on every run).
//!
//! # What the Sequence rules miss
//!
//! In the spirit of `tests/recovery_text.rs`'s own disclosed limits:
//!
//! - **Reserve/run** does not compare `--actor` between the two lines the
//!   way `tests/skill_guide_execution.rs` does for `SKILL.md` by executing
//!   both. Unlike `README.md` (which never writes an explicit `--actor` on
//!   these lines at all), `QUICKSTART.md`'s step 6 *does* write a matching
//!   `--actor implementer-a` on both — but this structural rule does not
//!   compare them. That comparison is instead the job of the execution test:
//!   a future edit that desynchronized the two actors would make the real
//!   `gate reserve`/`gate run` pair fail for a policy reason
//!   (`live_reservation_for_run`), which
//!   [`documented_step_6_gate_sequence_executes_in_the_order_written`] would
//!   surface as a failed assertion, not something this structural rule
//!   catches on its own.
//! - It does not confirm the hardcoded `--reservation-id VR-000001` is what
//!   a fresh reservation would really receive — the execution test does that
//!   too, conditionally, the same way `tests/skill_guide_execution.rs` does
//!   for `SKILL.md`'s identical hardcoded id.
//! - It matches on `--card-id`/`--gate-id` token equality and consumption
//!   order only, not on whether a reservation would still be live moment to
//!   moment against a real control repository — identical to the limit
//!   `tests/readme_guide.rs` names for its own rule.
//! - **Two-gates-before-both-stages** counts *distinct `--definition` file
//!   arguments* registered earlier in the document, not distinct `gate_id`
//!   values inside those files. It trusts that step 2's `sed
//!   's/gate_id: gate.unit/gate_id: gate.integration/'` really does produce
//!   a second, different id — it does not read `gate2.yaml`'s content (that
//!   file is never shown in full) to confirm it. A future edit that
//!   registered the same file twice under two names, or whose `sed` pattern
//!   silently failed to rewrite the id, would satisfy this rule's *count*
//!   without the underlying claim being true. Tracing `gate_id` identity
//!   through a shell substitution would mean binding shell semantics to YAML
//!   content across three separate spans of text (the original snippet, the
//!   `sed` pattern, and a file this rule never opens) for a guide section
//!   that already states its own moral in prose; judged not worth the
//!   fragility for what it would additionally prove.
//! - It also does not confirm the registered gate *ids* are the same ids the
//!   `named_gates` snippet names — only that enough distinct registrations
//!   happened by that point in the text. A future edit that changed
//!   `named_gates` to reference `gate.other` while step 2 still registered
//!   `gate.unit`/`gate.integration` would satisfy the count and miss the
//!   mismatch.
//!
//! # The ordering claim this file does not check at all
//!
//! Contract 206 §6 names a third example ordering claim: "`work resume`
//! rather than `work start` after a revise." `QUICKSTART.md` states this in
//! step 5's callout — inline prose, in backticks, not a fenced block:
//! "The fix is `work resume --card-id F-001 --actor implementer-a` ... `work
//! start` will not do it." Deliberately not covered here, and not merely
//! omitted: unlike the two Sequence rules above, this is not a
//! document-internal consistency claim (two things that must agree with each
//! other) but a single, unpaired factual claim about the harness's runtime
//! behavior in a specific state (an active lease, after a revise). The only
//! honest way to check it is *true*, not merely consistently worded, is real
//! execution — hold a lease, revise the card, confirm `work start` refuses
//! and `work resume` succeeds — a second fixture arc distinct from step 6's,
//! and one this file declines for the same proportionality reason § 7's
//! decision above gives. A reader who hit a future edit that quietly
//! flipped this recommendation would be stranded exactly as #144's reader
//! was; that residual risk is named here rather than left to be discovered.
//!
//! # The vacuity guards
//!
//! Each check below asserts its own precondition is non-empty before
//! asserting anything about its content, exactly as `tests/skill_guide.rs`
//! and `tests/readme_guide.rs` both do. Because this file layers Shape,
//! two Sequence rules, and one Execution test over the same document,
//! deleting every fenced block trips more than one guard at once — all four
//! are expected to fail together, not just one; see this card's report for
//! the exact list.

mod support;

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use change_harness::commands::CONTROL_ENV;
use support::Workspace;

// ---------------------------------------------------------------------
// Reading the guide.
// ---------------------------------------------------------------------

/// Reads `QUICKSTART.md` from the repository root.
fn quickstart_md() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/QUICKSTART.md");
    std::fs::read_to_string(path).expect("QUICKSTART.md should be readable at the repository root")
}

// ---------------------------------------------------------------------
// Invocation parsing. Adapted from `tests/readme_guide.rs` (copied, not
// imported: an integration test binary cannot import another one's
// non-`pub` items, only shared code under `tests/support/`, and that file's
// own module doc already makes the call that this ~15-line class of helper
// is simpler kept in sync by hand than promoted there for one more caller).
// One real addition beyond that precedent: `truncate_at_shell_metachar`,
// required by conventions `README.md` never uses; see the module doc,
// "What this file's extraction adds".
// ---------------------------------------------------------------------

/// One `change-harness ...` invocation parsed from a fenced `bash` block,
/// paired with the 1-based line number in the source text where it begins.
#[derive(Debug, Clone)]
struct Invocation {
    line: usize,
    tokens: Vec<String>,
}

impl Invocation {
    fn text(&self) -> String {
        self.tokens.join(" ")
    }

    fn subcommand(&self) -> (Option<&str>, Option<&str>) {
        (
            self.tokens.get(1).map(String::as_str),
            self.tokens.get(2).map(String::as_str),
        )
    }

    fn flag(&self, name: &str) -> Option<&str> {
        find_flag_value(&self.tokens, name)
    }
}

/// Splits one line into argv-style tokens, honoring `"..."` spans so a
/// quoted value survives as one token. Identical to `tests/readme_guide.rs`
/// / `tests/skill_guide.rs` / `tests/skill_guide_execution.rs`'s helper of
/// the same name.
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

/// The value following a `--flag` in a parsed invocation's own tokens, if
/// present. Copied from `tests/readme_guide.rs`'s helper of the same name.
fn find_flag_value<'a>(tokens: &'a [String], flag: &str) -> Option<&'a str> {
    tokens
        .windows(2)
        .find_map(|pair| (pair[0] == flag).then_some(pair[1].as_str()))
}

/// Drops every token from the first shell metacharacter onward: a bare `>`,
/// `>>`, `<`, `;`, `|`, `&&`, `||`, or any token starting with `#`.
/// `QUICKSTART.md` documents `change-harness gate example > gate.yaml` (save
/// generator output) and trailing `# note` comments — both real shell syntax
/// a reader's own terminal interprets, neither a `change-harness` argument.
/// See the module doc, "What this file's extraction adds", for the
/// `--help`-append failure this prevents. A token search rather than a
/// substring cut: `split_invocation` has already resolved `"..."` quoting by
/// the time this runs, so a quoted value that happens to contain `>` (none
/// does today) would survive as one token and not trigger this.
fn truncate_at_shell_metachar(tokens: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(tokens.len());
    for token in tokens {
        if token.starts_with('#')
            || matches!(token.as_str(), ">" | ">>" | "<" | ";" | "|" | "&&" | "||")
        {
            break;
        }
        out.push(token);
    }
    out
}

/// Parses every `change-harness ...` invocation out of fenced `bash` blocks
/// anywhere in `lines`, in source order, each paired with its 1-based line
/// number. Whole-document scope, no heading restriction — matching
/// `tests/readme_guide.rs`'s own "Scope" argument: a defect this file cannot
/// see because it only looked in one section would be exactly as invisible
/// to a reader as one this file never checked at all. Joins a trailing `\`
/// continuation with the line that follows, chaining across more than one
/// continuation line, the same as `tests/readme_guide.rs` (`QUICKSTART.md`'s
/// step 1 `project init` and step 6 `gate run` both use it).
fn parse_invocations(lines: &[&str]) -> Vec<Invocation> {
    let mut invocations = Vec::new();
    let mut in_bash_block = false;
    let mut pending: Option<(usize, String)> = None;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if trimmed.starts_with("```bash") {
            in_bash_block = true;
            continue;
        }
        if in_bash_block && trimmed.starts_with("```") {
            in_bash_block = false;
            continue;
        }
        if !in_bash_block {
            continue;
        }

        if let Some((start_line, mut acc)) = pending.take() {
            acc.push(' ');
            if let Some(stripped) = trimmed.strip_suffix('\\') {
                acc.push_str(stripped.trim_end());
                pending = Some((start_line, acc));
            } else {
                acc.push_str(trimmed);
                invocations.push(Invocation {
                    line: start_line,
                    tokens: truncate_at_shell_metachar(split_invocation(&acc)),
                });
            }
            continue;
        }

        if trimmed.starts_with("change-harness ") {
            let line_no = idx + 1;
            match trimmed.strip_suffix('\\') {
                Some(stripped) => pending = Some((line_no, stripped.trim_end().to_string())),
                None => invocations.push(Invocation {
                    line: line_no,
                    tokens: truncate_at_shell_metachar(split_invocation(trimmed)),
                }),
            }
        }
    }

    // Defensive only: every continuation in the real document resolves
    // before its fenced block closes (confirmed by reading `QUICKSTART.md`
    // while writing this file). A synthetic fixture that left a `\` dangling
    // would otherwise silently lose that invocation instead of surfacing it.
    if let Some((start_line, acc)) = pending {
        invocations.push(Invocation {
            line: start_line,
            tokens: truncate_at_shell_metachar(split_invocation(&acc)),
        });
    }

    invocations
}

/// Parses every `change-harness ...` invocation anywhere in `source`.
fn all_invocations(source: &str) -> Vec<Invocation> {
    let lines: Vec<&str> = source.lines().collect();
    parse_invocations(&lines)
}

// ---------------------------------------------------------------------
// Shape: every documented invocation is a real command.
// ---------------------------------------------------------------------

/// Every `change-harness` line anywhere in `QUICKSTART.md` is a real
/// subcommand with real flags. Catches a renamed flag, a misspelled
/// subcommand, or an invented option. Cannot catch a sequence defect or a
/// wrong-but-plausible value; see the module doc.
#[test]
fn quickstart_commands_are_real() {
    let quickstart = quickstart_md();
    let invocations = all_invocations(&quickstart);

    // Vacuity guard: a parser that silently matched nothing would let this
    // test pass no matter what the document said.
    assert!(
        !invocations.is_empty(),
        "found zero `change-harness` invocations anywhere in QUICKSTART.md; either the document \
         lost its commands or the fenced-`bash`-block convention this file expects was not \
         followed"
    );

    for invocation in &invocations {
        let args = &invocation.tokens[1..];
        let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
            .args(args)
            .arg("--help")
            .output()
            .expect("the CLI binary should start");

        assert!(
            output.status.success(),
            "QUICKSTART.md:{}: `{}` is not a real command shape (exit {:?}):\nstdout: {}\nstderr: {}",
            invocation.line,
            invocation.text(),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

// ---------------------------------------------------------------------
// Sequence 1: `gate reserve` before `gate run`, consumption-tracked.
// Adapted from `tests/readme_guide.rs`'s `sequence_violations`.
// ---------------------------------------------------------------------

/// Every `gate run` must be preceded by an *unconsumed* `gate reserve`
/// naming the same `--card-id` and `--gate-id`, and must itself carry
/// `--reservation-id`. "Unconsumed": `SKILL.md:295-296` states the semantics
/// this rule enforces — "a reservation authorizes one expensive validation
/// attempt, not standing permission" — so this tracks, per `(card_id,
/// gate_id)`, how many reservations are currently available: incremented by
/// `gate reserve`, decremented by the next `gate run` that claims one.
/// Identical algorithm to `tests/readme_guide.rs`'s `sequence_violations`;
/// duplicated rather than shared for the same cross-test-binary reason every
/// other helper in this file is.
fn sequence_violations(invocations: &[Invocation]) -> Vec<String> {
    let mut available: std::collections::HashMap<(String, String), u32> =
        std::collections::HashMap::new();
    let mut violations = Vec::new();

    for invocation in invocations {
        if invocation.subcommand() == (Some("gate"), Some("reserve"))
            && let (Some(card_id), Some(gate_id)) =
                (invocation.flag("--card-id"), invocation.flag("--gate-id"))
        {
            *available
                .entry((card_id.to_string(), gate_id.to_string()))
                .or_insert(0) += 1;
        }

        if invocation.subcommand() == (Some("gate"), Some("run")) {
            if invocation.flag("--reservation-id").is_none() {
                violations.push(format!(
                    "QUICKSTART.md:{}: `{}` does not carry `--reservation-id`",
                    invocation.line,
                    invocation.text(),
                ));
            }

            match (invocation.flag("--card-id"), invocation.flag("--gate-id")) {
                (Some(card_id), Some(gate_id)) => {
                    let remaining = available
                        .entry((card_id.to_string(), gate_id.to_string()))
                        .or_insert(0);
                    if *remaining > 0 {
                        *remaining -= 1;
                    } else {
                        violations.push(format!(
                            "QUICKSTART.md:{}: `{}` is not preceded by an unconsumed `gate \
                             reserve --card-id {card_id} --gate-id {gate_id}` earlier in \
                             QUICKSTART.md",
                            invocation.line,
                            invocation.text(),
                        ));
                    }
                }
                _ => violations.push(format!(
                    "QUICKSTART.md:{}: `{}` is missing --card-id or --gate-id, so this rule \
                     cannot confirm it was ever reserved",
                    invocation.line,
                    invocation.text(),
                )),
            }
        }
    }

    violations
}

/// `QUICKSTART.md` never documents a `gate run` that is not provably
/// reserved. `--help` cannot see this defect (module doc); this uses
/// [`sequence_violations`] instead, a structural rule, without executing
/// anything.
#[test]
fn quickstart_never_documents_an_unreserved_gate_run() {
    let quickstart = quickstart_md();
    let invocations = all_invocations(&quickstart);

    let gate_run_count = invocations
        .iter()
        .filter(|invocation| invocation.subcommand() == (Some("gate"), Some("run")))
        .count();
    assert!(
        gate_run_count > 0,
        "found zero `gate run` invocations anywhere in QUICKSTART.md; a sequence rule with \
         nothing to check would pass vacuously no matter what the document said"
    );

    let violations = sequence_violations(&invocations);
    assert!(
        violations.is_empty(),
        "QUICKSTART.md documents at least one `gate run` that is not provably reserved:\n{}",
        violations.join("\n")
    );
}

/// [`sequence_violations`] itself is correct, decoupled from whatever
/// `QUICKSTART.md` currently says — same discipline as
/// `tests/readme_guide.rs`'s `gate_run_reservation_rule_is_not_a_rubber_stamp`,
/// which this reuses the exact synthetic shapes from (a correctly matched
/// pair, a run missing `--reservation-id`, a run with no preceding reserve,
/// a run whose only preceding reserve names a different gate, a reservation
/// already spent by an earlier run, and two independently reserved runs for
/// the same card and gate).
#[test]
fn quickstart_gate_run_reservation_rule_is_not_a_rubber_stamp() {
    fn invocations_from(bash_block: &str) -> Vec<Invocation> {
        let lines: Vec<&str> = bash_block.lines().collect();
        parse_invocations(&lines)
    }

    let correctly_matched = invocations_from(
        "```bash\n\
         change-harness gate reserve --card-id F-900 --gate-id gate.check\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000001\n\
         ```\n",
    );
    assert_eq!(correctly_matched.len(), 2);
    assert!(
        sequence_violations(&correctly_matched).is_empty(),
        "a genuinely matched reserve/run pair must not be flagged"
    );

    let missing_reservation_id = invocations_from(
        "```bash\n\
         change-harness gate reserve --card-id F-900 --gate-id gate.check\n\
         change-harness gate run --card-id F-900 --gate-id gate.check\n\
         ```\n",
    );
    let violations = sequence_violations(&missing_reservation_id);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("--reservation-id"), "{violations:?}");

    let no_preceding_reserve = invocations_from(
        "```bash\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000001\n\
         ```\n",
    );
    let violations = sequence_violations(&no_preceding_reserve);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("not preceded by"), "{violations:?}");

    let mismatched_gate = invocations_from(
        "```bash\n\
         change-harness gate reserve --card-id F-900 --gate-id gate.other\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000001\n\
         ```\n",
    );
    let violations = sequence_violations(&mismatched_gate);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("not preceded by"), "{violations:?}");

    let reservation_consumed_by_first_run = invocations_from(
        "```bash\n\
         change-harness gate reserve --card-id F-900 --gate-id gate.check\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000001\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000002\n\
         ```\n",
    );
    let violations = sequence_violations(&reservation_consumed_by_first_run);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("VR-000002") && violations[0].contains("not preceded by"),
        "{violations:?}"
    );

    let two_independent_reserve_run_pairs = invocations_from(
        "```bash\n\
         change-harness gate reserve --card-id F-900 --gate-id gate.check\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000001\n\
         change-harness gate reserve --card-id F-900 --gate-id gate.check\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000002\n\
         ```\n",
    );
    assert!(
        sequence_violations(&two_independent_reserve_run_pairs).is_empty(),
        "two independently reserved runs for the same card and gate must both be accepted"
    );
}

// ---------------------------------------------------------------------
// Sequence 2: two distinct gates registered before the card names both
// stages. Step 2's own words, turned into a structural check; see the
// module doc for exactly what this counts and what it misses.
// ---------------------------------------------------------------------

/// One fenced ` ```yaml ` block, with the 1-based line number of its
/// opening fence. Deliberately no heading-tracking, unlike
/// `tests/guide_documents.rs`'s `YamlBlock`: this file only ever needs to
/// find the one block naming `named_gates:`, identified by content, not by
/// which section it falls under.
#[derive(Debug)]
struct YamlBlock {
    line: usize,
    body: String,
}

/// Extracts every fenced ` ```yaml ` block in `source`, in document order.
fn yaml_blocks(source: &str) -> Vec<YamlBlock> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut start_line = 0;
    let mut body_lines: Vec<&str> = Vec::new();

    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if in_block {
            if trimmed == "```" {
                blocks.push(YamlBlock {
                    line: start_line,
                    body: body_lines.join("\n"),
                });
                in_block = false;
                body_lines.clear();
            } else {
                body_lines.push(line);
            }
            continue;
        }
        if trimmed == "```yaml" {
            in_block = true;
            start_line = index + 1;
        }
    }
    assert!(
        !in_block,
        "QUICKSTART.md has an unterminated ```yaml fence starting at line {start_line}"
    );
    blocks
}

/// The three stage keys a `named_gates:` snippet may populate, matching
/// `change_harness::domain::card::NamedGates`'s own field names.
const NAMED_GATE_STAGES: &[&str] = &["feature", "review", "integration"];

/// Which of [`NAMED_GATE_STAGES`] carry at least one gate id in `body` — a
/// `named_gates:` block's fenced YAML text. A stage counts as non-empty when
/// its own line is not the bare `key: []` shape and the very next line is a
/// `- ` list item (at any deeper indentation). These are the only two shapes
/// `QUICKSTART.md`'s own snippet uses (confirmed by reading it in full);
/// see the module doc for what a different YAML shape would do to this
/// (nothing — a key this function does not recognize contributes nothing,
/// never a false positive).
fn nonempty_stages(body: &str) -> Vec<&'static str> {
    let lines: Vec<&str> = body.lines().collect();
    let mut out = Vec::new();
    for &stage in NAMED_GATE_STAGES {
        let inline_empty = format!("{stage}: []");
        let bare = format!("{stage}:");
        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed == inline_empty {
                break;
            }
            if trimmed == bare {
                let has_item = lines
                    .get(idx + 1)
                    .is_some_and(|next| next.trim_start().starts_with("- "));
                if has_item {
                    out.push(stage);
                }
                break;
            }
        }
    }
    out
}

/// Distinct `--definition` file arguments named by a `gate register`
/// invocation strictly before `before_line`, in document order. Distinct by
/// *file argument*, not by the `gate_id` inside that file; see the module
/// doc, "What the Sequence rules miss".
fn distinct_registrations_before(invocations: &[Invocation], before_line: usize) -> Vec<String> {
    let mut seen = Vec::new();
    for invocation in invocations {
        if invocation.line >= before_line {
            break;
        }
        if invocation.subcommand() == (Some("gate"), Some("register"))
            && let Some(definition) = invocation.flag("--definition")
            && !seen.iter().any(|existing| existing == definition)
        {
            seen.push(definition.to_owned());
        }
    }
    seen
}

/// Step 2's own claim, checked structurally: "A gate can occupy only one
/// validation stage, and the validation policy requires the
/// final-integration stage to have at least one gate. So one gate is never
/// enough." By the time the card's `named_gates` snippet populates more than
/// one stage, at least that many distinct gates must already have been
/// registered earlier in the document — otherwise a reader who copied the
/// card as shown would activate it against gates that were never registered
/// for one of the stages it names, the exact trap step 2's prose warns
/// against.
#[test]
fn quickstart_registers_enough_gates_before_the_card_names_both_stages() {
    let quickstart = quickstart_md();
    let invocations = all_invocations(&quickstart);
    let blocks = yaml_blocks(&quickstart);

    // Vacuity guard, this rule's own version of the pattern every other
    // check in this file uses.
    assert!(
        !blocks.is_empty(),
        "found zero fenced ```yaml blocks in QUICKSTART.md; either the document lost its embedded \
         snippets or the ```yaml fence convention this file scans for was not followed"
    );

    let named_gates_block = blocks
        .iter()
        .find(|block| block.body.contains("named_gates:"))
        .unwrap_or_else(|| {
            panic!("QUICKSTART.md should contain a fenced ```yaml block with a `named_gates:` key")
        });

    let populated = nonempty_stages(&named_gates_block.body);
    assert!(
        !populated.is_empty(),
        "the named_gates block at QUICKSTART.md:{} names zero non-empty stages; either the \
         snippet's shape changed or nonempty_stages stopped recognizing it",
        named_gates_block.line
    );

    let registered = distinct_registrations_before(&invocations, named_gates_block.line);
    assert!(
        registered.len() >= populated.len(),
        "QUICKSTART.md:{} names {} populated named_gates stage(s) ({:?}) but only {} distinct \
         gate definition(s) were registered earlier in the document ({:?}) — a card naming a \
         gate in a stage nobody registered by this point is exactly the trap step 2's own text \
         warns about",
        named_gates_block.line,
        populated.len(),
        populated,
        registered.len(),
        registered,
    );
}

/// [`nonempty_stages`], [`distinct_registrations_before`], and [`yaml_blocks`]
/// are correct, decoupled from whatever `QUICKSTART.md` currently says — the
/// same "no false positive" discipline every rule-correctness test in this
/// file and its precedents uses.
#[test]
fn stage_population_and_registration_counting_rules_are_correct_on_synthetic_input() {
    // yaml_blocks: heading-free, but line numbers and body text must still
    // be exact — mirrors `tests/guide_documents.rs`'s
    // `yaml_block_extraction_binds_heading_and_line_from_synthetic_markdown`.
    let synthetic_markdown = "# Title\n\n\
        prose\n\n\
        ```yaml\n\
        a: 1\n\
        ```\n\n\
        more prose\n\n\
        ```yaml\n\
        named_gates:\n\
        \x20\x20feature:\n\
        \x20\x20- gate.unit\n\
        \x20\x20review: []\n\
        \x20\x20integration:\n\
        \x20\x20- gate.integration\n\
        ```\n";
    let blocks = yaml_blocks(synthetic_markdown);
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert_eq!(blocks[0].line, 5);
    assert_eq!(blocks[1].line, 11);
    assert!(blocks[1].body.contains("named_gates:"));

    // nonempty_stages: two populated, one explicitly empty.
    let populated = nonempty_stages(&blocks[1].body);
    assert_eq!(populated, vec!["feature", "integration"]);

    // All three stages explicitly empty.
    let all_empty = "named_gates:\n  feature: []\n  review: []\n  integration: []";
    assert!(nonempty_stages(all_empty).is_empty());

    // A stage present with a list item, and a stage key absent entirely
    // (never a false positive for a shape this function does not recognize).
    let one_populated_one_absent = "named_gates:\n  feature:\n  - gate.unit\n  integration: []";
    assert_eq!(nonempty_stages(one_populated_one_absent), vec!["feature"]);

    // distinct_registrations_before: duplicate --definition values count
    // once; only invocations strictly before the cutoff line count.
    let bash_block = "```bash\n\
        change-harness gate register --definition a.yaml\n\
        change-harness gate register --definition b.yaml\n\
        change-harness gate register --definition a.yaml\n\
        change-harness gate register --definition c.yaml\n\
        ```\n";
    let lines: Vec<&str> = bash_block.lines().collect();
    let invocations = parse_invocations(&lines);
    assert_eq!(invocations.len(), 4, "{invocations:?}");

    let before_everything = distinct_registrations_before(&invocations, 1);
    assert!(
        before_everything.is_empty(),
        "nothing registers before the very first line: {before_everything:?}"
    );

    let before_the_third_line = distinct_registrations_before(&invocations, invocations[2].line);
    assert_eq!(
        before_the_third_line,
        vec!["a.yaml".to_owned(), "b.yaml".to_owned()],
        "only the first two registrations precede line 3, and the repeated a.yaml on line 3 \
         itself must not count: {before_the_third_line:?}"
    );

    let before_past_the_end = distinct_registrations_before(&invocations, usize::MAX);
    assert_eq!(
        before_past_the_end,
        vec![
            "a.yaml".to_owned(),
            "b.yaml".to_owned(),
            "c.yaml".to_owned()
        ],
        "three distinct files across four registrations (one repeated): {before_past_the_end:?}"
    );
}

// ---------------------------------------------------------------------
// Execution: step 6 (`gate reserve` then `gate run`), for real. Mirrors
// `tests/skill_guide_execution.rs`'s `documented_gate_sequence_executes_in_the_order_written`
// exactly; see the module doc, "Why one step, and which one".
// ---------------------------------------------------------------------

/// The exact heading `QUICKSTART.md` uses for the step this file executes.
const STEP_6_HEADING: &str = "## 6. Run the feature gate";

/// The 1-based line number of the line equal to `heading`, and of the next
/// line starting with `## ` (or `usize::MAX` if `heading` is the last
/// section) — used to slice [`all_invocations`]'s output down to one step
/// without a second, section-scoped parser.
fn heading_line_range(lines: &[&str], heading: &str) -> (usize, usize) {
    let start = lines
        .iter()
        .position(|line| line.trim() == heading)
        .unwrap_or_else(|| panic!("QUICKSTART.md should contain the heading {heading:?}"))
        + 1;
    let end = lines[start..]
        .iter()
        .position(|line| line.trim_start().starts_with("## "))
        .map_or(usize::MAX, |offset| start + offset);
    (start, end)
}

/// Runs a parsed `change-harness ...` invocation for real: no `--help`, no
/// injected `--control`. Step 6's two lines rely on `CHANGE_HARNESS_CONTROL`
/// (step 1's `export`), exactly as `tests/skill_guide_execution.rs`'s
/// `run_documented_invocation` does for `SKILL.md`'s identical reliance.
fn run_documented_invocation(control: &Path, tokens: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(&tokens[1..])
        .env(CONTROL_ENV, control)
        .output()
        .expect("the CLI binary should start")
}

/// Builds the project state steps 1-5 establish for one card named
/// `card_id`, gated by one feature gate named `gate_id` (already registered
/// by `Workspace::initialized`) plus the fixture's own integration gate, and
/// leased to `actor` with a real commit sitting in the allocated worktree.
/// Not parsed from `QUICKSTART.md`'s text — steps 1-5 carry placeholders and
/// manual-editing instructions step 6 does not; see the module doc. Mirrors
/// `tests/skill_guide_execution.rs`'s `fixture_ready_for_the_narrow_claim`.
fn fixture_ready_for_a_reserve_and_run(card_id: &str, gate_id: &str, actor: &str) -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "add a farewell function",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates(card_id, &["src/thing.rs"], &[gate_id]);
    workspace.bind_fixture_plan_with_assignment("PLAN-QUICKSTART-001", "parallel", actor);
    workspace.work(&["start", "--card-id", card_id, "--actor", actor]);

    let worktree = workspace.worktrees.join(card_id);
    let file = worktree.join("src/thing.rs");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "// thing\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: thing"]);

    workspace
}

/// `QUICKSTART.md`'s documented step 6 — `gate reserve` then `gate run` —
/// executes in order, for real.
///
/// Fails, and names exactly why, if: the doc's `gate run --actor` differs
/// from its `gate reserve --actor` on the line above (mismatched actor:
/// `live_reservation_for_run` refuses); the doc's `gate run` drops
/// `--reservation-id` (`run_gate` refuses unconditionally); the two lines
/// are reordered or one is deleted (the count/order assertions below fail
/// first, naming what moved, before either command runs); the doc's
/// hardcoded `--reservation-id VR-000001` stops being the id a fresh
/// reservation for this exact card and gate actually receives; or either
/// `--card-id`/`--gate-id` value on the two lines stops matching the other.
#[test]
fn documented_step_6_gate_sequence_executes_in_the_order_written() {
    let quickstart = quickstart_md();
    let lines: Vec<&str> = quickstart.lines().collect();
    let invocations = all_invocations(&quickstart);
    let (start, end) = heading_line_range(&lines, STEP_6_HEADING);

    let step_6: Vec<&Invocation> = invocations
        .iter()
        .filter(|invocation| invocation.line > start && invocation.line < end)
        .collect();

    // Vacuity/shape guard: a parser that silently matched nothing (a moved
    // heading, a fence typo, a deleted line) would let this test pass
    // vacuously, or run the wrong pair, no matter what the document said.
    assert_eq!(
        step_6.len(),
        2,
        "expected exactly the `gate reserve` then `gate run` pair under {STEP_6_HEADING:?}, \
         found {step_6:?}"
    );
    let reserve = step_6[0];
    let run = step_6[1];
    assert_eq!(
        reserve.subcommand(),
        (Some("gate"), Some("reserve")),
        "the first documented line under {STEP_6_HEADING:?} should be `gate reserve`: {reserve:?}"
    );
    assert_eq!(
        run.subcommand(),
        (Some("gate"), Some("run")),
        "the second documented line under {STEP_6_HEADING:?} should be `gate run`: {run:?}"
    );

    let workspace = fixture_ready_for_a_reserve_and_run("F-001", "gate.unit", "implementer-a");

    let reserve_output = run_documented_invocation(&workspace.control, &reserve.tokens);
    assert!(
        reserve_output.status.success(),
        "documented `{}` failed (exit {:?}):\nstdout: {}\nstderr: {}",
        reserve.text(),
        reserve_output.status.code(),
        String::from_utf8_lossy(&reserve_output.stdout),
        String::from_utf8_lossy(&reserve_output.stderr),
    );

    // The doc hardcodes `VR-000001`. Prove that rather than assume it, the
    // same way `tests/skill_guide_execution.rs` does for `SKILL.md`'s
    // identical hardcoded id. Conditional: a documented line that dropped
    // the flag entirely is a different defect, and the unconditional real
    // execution below is what must catch that one.
    if let Some(documented_reservation_id) = run.flag("--reservation-id") {
        let reserve_stdout = String::from_utf8_lossy(&reserve_output.stdout);
        assert!(
            reserve_stdout.contains(&format!(
                "validation reservation {documented_reservation_id}"
            )),
            "the documented `--reservation-id {documented_reservation_id}` does not match what a \
             fresh reservation for this card and gate actually receives; reserve reported: \
             {reserve_stdout}"
        );
    }

    let run_output = run_documented_invocation(&workspace.control, &run.tokens);
    assert!(
        run_output.status.success(),
        "documented `{}` failed (exit {:?}):\nstdout: {}\nstderr: {}",
        run.text(),
        run_output.status.code(),
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr),
    );
    let run_stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(
        run_stdout.contains("verdict: PASS"),
        "documented `{}` exited 0 but did not report a passing verdict: {run_stdout}",
        run.text()
    );

    // Not merely "exit 0 and said PASS": a real, current receipt for exactly
    // this gate landed on the card's record.
    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let feature_gates = status["data"]["feature_gates"]
        .as_array()
        .expect("gate status should report the card's feature gates");
    let unit_gate = feature_gates
        .iter()
        .find(|gate| gate["gate_id"] == "gate.unit")
        .unwrap_or_else(|| panic!("gate.unit should be a feature gate for F-001: {status}"));
    assert_eq!(
        unit_gate["satisfied"], true,
        "the documented `gate run` should have produced a satisfying receipt: {status}"
    );
    assert_eq!(
        unit_gate["runs"], 1,
        "expected exactly one recorded run of gate.unit: {status}"
    );
}

/// The no-false-positive test: a correct sequence, decoupled from
/// `QUICKSTART.md`'s real content, is accepted. Without this, a bug in this
/// file's own fixture wiring — the wrong environment variable name, a typo
/// in [`heading_line_range`] — could make the test above "detect" every
/// required mutation for a reason that has nothing to do with actually
/// executing the pair correctly. Mirrors
/// `tests/skill_guide_execution.rs`'s
/// `a_correctly_matched_reserve_and_run_pair_is_accepted`: a different card,
/// gate, and actor than `QUICKSTART.md` names, run through the identical
/// helpers the test above uses.
#[test]
fn a_correctly_matched_step_6_style_reserve_and_run_pair_is_accepted() {
    let synthetic_lines: Vec<&str> = "\n\
        ```bash\n\
        change-harness gate reserve --card-id F-900 --gate-id gate.check --actor solo-operator\n\
        change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000001 \
        --actor solo-operator\n\
        ```\n"
        .lines()
        .collect();
    let invocations = parse_invocations(&synthetic_lines);
    assert_eq!(
        invocations.len(),
        2,
        "the synthetic fixture text should parse to exactly 2 invocations: {invocations:?}"
    );

    let workspace = Workspace::initialized();
    workspace.register_gate("gate.check", &["true"]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "Synthetic no-false-positive control",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-900", &["src/other.rs"], &["gate.check"]);
    workspace.bind_fixture_plan_with_assignment("PLAN-QUICKSTART-002", "parallel", "solo-operator");
    workspace.work(&["start", "--card-id", "F-900", "--actor", "solo-operator"]);

    let worktree = workspace.worktrees.join("F-900");
    let file = worktree.join("src/other.rs");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "// other\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: other"]);

    for invocation in &invocations {
        let output = run_documented_invocation(&workspace.control, &invocation.tokens);
        assert!(
            output.status.success(),
            "a genuinely correctly-matched `{}` must be accepted (exit {:?}):\nstdout: {}\nstderr: {}",
            invocation.text(),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
