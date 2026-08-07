//! Confirms `README.md` — the walkthrough a brand-new operator actually
//! reads, as opposed to `SKILL.md`'s agent-oriented guide — never documents
//! a `change-harness` invocation that is not a real command shape, and never
//! documents a `gate run` that is not provably reserved.
//!
//! `tests/skill_guide.rs` and `tests/skill_guide_execution.rs` both open
//! exactly one file: `concat!(env!("CARGO_MANIFEST_DIR"), "/SKILL.md")`.
//! Nothing in this repository read `README.md`, so when `SKILL.md` was
//! corrected (#109) to reserve a gate before running it, `README.md:272`
//! kept telling an operator to run:
//!
//! ```text
//! change-harness gate run --card-id F-001 --gate-id gate.unit
//! ```
//!
//! with no preceding `gate reserve` and no `--reservation-id` — refused
//! since #105, and the exact wall the #105-#119 wave exists to name. This
//! file is `README.md`'s equivalent of the two files above.
//!
//! # The trap: `--help` alone cannot see this defect
//!
//! Verified against the built binary:
//!
//! ```text
//! $ change-harness gate run --card-id F-001 --gate-id gate.unit --help
//! EXIT: 0
//! ```
//!
//! Every token on that line is a real subcommand and a real flag —
//! `--reservation-id` is `Option<String>` to `clap` (see `RunArgs` in
//! `src/commands/gate.rs`), so its absence is not a parse error. The refusal
//! (`live_reservation_for_run`, same file) fires only on a real execution,
//! after argument parsing already succeeded. A test that only appends
//! `--help` to every documented line — the mechanism `tests/skill_guide.rs`
//! uses — would go green over a document that walks an operator into a
//! wall, exactly as `tests/skill_guide_execution.rs`'s own module doc
//! explains for the identical trap in `SKILL.md`. So this file checks two
//! properties, by two different mechanisms:
//!
//! - **Shape** (`readme_commands_are_real`): every `change-harness` line
//!   anywhere in `README.md` names a real subcommand and real flags. The
//!   `--help`-append mechanism from `tests/skill_guide.rs` is exactly right
//!   for this and is reused here (helper functions copied, not imported —
//!   an integration test binary cannot import another one's non-`pub`
//!   items, only shared code under `tests/support/`; `tests/skill_guide_execution.rs`
//!   gives the identical reason for keeping its own copy of
//!   `split_invocation` rather than promoting it there for one extra
//!   caller). This check alone is the one the trap above defeats.
//! - **Sequence** (`readme_never_documents_an_unreserved_gate_run`): a
//!   documented `gate run` must be preceded by an unconsumed `gate reserve`
//!   naming the same `--card-id` and `--gate-id`, and must itself carry
//!   `--reservation-id`. This is a **structural rule over the parsed
//!   invocations**, not real execution.
//!
//! # Scope: the whole document, not one section
//!
//! This file originally scoped both checks to `README.md`'s
//! "## Operator workflow" section — the walkthrough that actually contains
//! `README.md:272`. A reviewer found the gap that leaves: they documented a
//! second, unreserved `gate run` under a *different* heading,
//! "## Recovering from an interruption" —
//!
//! ```text
//! ## Recovering from an interruption
//! Re-run the gate once the reservation is settled:
//! ```bash
//! change-harness gate run --card-id F-001 --gate-id gate.unit
//! ```
//! ```
//!
//! — and the suite stayed green, because nothing was scanning that heading
//! at all. The property this card actually exists for is "`README.md` does
//! not document an unreserved `gate run`", full stop, not "...inside one
//! section" — so both [`all_invocations`]-based checks below now scan the
//! entire document. `--card-id`/`--gate-id` matching is unaffected: `gate
//! run`/`gate reserve` never appear outside "## Operator workflow" today, so
//! nothing about *today's* README changes shape; what changes is that a
//! *future* unreserved `gate run` anywhere else in the file is no longer
//! invisible.
//!
//! Widening the scan alone is not sufficient, and was the second thing this
//! review found: `SKILL.md` states the semantics this rule is supposed to
//! enforce — "a reservation authorizes one expensive validation attempt, not
//! standing permission" (`SKILL.md:295-296`). A naive rule that just
//! remembers "some `gate reserve` for this card and gate was seen somewhere
//! earlier" would treat the *already-correctly-reserved-and-run* pair in
//! "## Operator workflow" as permanently satisfying *any* later `gate run`
//! reusing the same illustrative `F-001`/`gate.unit` ids anywhere else in
//! the file — including the reviewer's example above, which is exactly the
//! defect it is meant to demonstrate. [`sequence_violations`] instead treats
//! each `gate reserve` as good for exactly one later `gate run`: a per-pair
//! *available count*, incremented by `gate reserve` and decremented by the
//! next `gate run` that claims one. See [`sequence_violations`]'s own doc
//! comment.
//!
//! # Why a structural rule instead of executing the sequence
//!
//! `tests/skill_guide_execution.rs` executes `SKILL.md`'s reserve/run pair
//! for real, and says exactly why it is scoped to that one `### ` step:
//! every other fenced block in its section either names a file whose
//! content the guide never shows (`gate.test.json`, `verdict.yaml`) or
//! embeds a placeholder that cannot be run literally.
//!
//! `README.md`'s "## Operator workflow" section — the only place `gate run`
//! or `gate reserve` are documented today — has the same property, more
//! severely. Counted directly against the section's 22 documented
//! `change-harness` lines: 20 either reference a file whose content is
//! never shown in the guide (`gate.unit.yaml`, `F-001.yaml`, `decl.yaml`,
//! `verdict.yaml`) or write a literal, never-assigned shell variable,
//! `$CONTROL`, straight into the argument list — `--control $CONTROL` is
//! not runnable as written by any mechanism short of a real shell with that
//! variable exported, which nothing in this document does (the "Quick
//! install" section exports `CHANGE_HARNESS_CONTROL`, a different name,
//! once, for a different purpose). The remaining 2 lines — both of step 4,
//! `work start` followed by `gate run` — are fully concrete: no unshown
//! file, no `$CONTROL` (both commands rely on `clap`'s `env =
//! CHANGE_HARNESS_CONTROL` fallback and `--actor`'s `"operator"` default;
//! confirmed by reading `CommonArgs` in `src/commands/work.rs` and
//! `src/commands/gate.rs`). Step 4 is the one step execution could cover —
//! and it is exactly the step already broken, so an execution-based test
//! here would duplicate what the structural rule below already catches, at
//! the cost of standing up a full project fixture. This reasoning is about
//! the `gate run`/`gate reserve` *mechanism*, not the scan's scope: widening
//! the scan (above) does not change which lines could ever be a `gate run`
//! or `gate reserve` today, only whether a *future* one outside this section
//! would be seen.
//!
//! Six more `change-harness` lines exist outside "## Operator workflow"
//! (`doctor --workspace .`; `project init` with five `\`-continued flags;
//! `backup create`/`backup verify`/`project status`/`project recover`, each
//! with `--control $CONTROL`). None is a `gate run` or `gate reserve`, so
//! none affects the sequence argument above. Only `doctor --workspace .` is
//! itself fully concrete (`.` always exists; the rest carry `$CONTROL` or
//! illustrative, non-real paths like `/path/to/repo`) — confirmed by running
//! all six against the built binary while writing this file.
//!
//! # What the sequence rule misses
//!
//! [`sequence_violations`] is honest about its blind spots rather than
//! pretending to be complete, in the spirit of `tests/recovery_text.rs`'s
//! own "What this rule would miss" section:
//!
//! - It does not check that `gate reserve` and `gate run` share the same
//!   `--actor`. `tests/skill_guide_execution.rs` catches exactly that
//!   mismatch for `SKILL.md` by executing both lines for real. `README.md`
//!   never writes an explicit `--actor` on any `gate`/`work`/`card` line
//!   (every one of them relies on the CLI's own `"operator"` default), so
//!   there is nothing textual here to compare — but if a future edit added a
//!   mismatched `--actor` to two of these lines, this rule would not notice.
//! - It does not confirm the literal `--reservation-id` *value* written in
//!   the doc is what a real `gate reserve` call would actually return. It
//!   only checks that the flag is present and that an *unconsumed*
//!   `gate reserve` naming the same card and gate appears earlier in the
//!   document. Proving the value itself is correct would require real
//!   execution (as `tests/skill_guide_execution.rs` does for `SKILL.md`'s
//!   hardcoded `VR-000001`, lines 243-260 of that file) — deliberately not
//!   done here, for the reason given above. README's fix reuses `VR-000001`
//!   because `next_reservation_id` (`src/commands/gate.rs`) hands out the
//!   highest existing `VR-NNNNNN` across the whole control repository plus
//!   one, defaulting to zero when none exist — so the first `gate reserve`
//!   call in a fresh project's first-ever pass through the document
//!   receives exactly `VR-000001`, the same fact `SKILL.md`'s own example
//!   already relies on. That is a source-reading argument, not a test in
//!   this file: if a future edit adds an earlier `gate reserve` call
//!   anywhere before README's own step 4, or `next_reservation_id`'s
//!   allocation ever becomes scoped per card/gate instead of
//!   repository-wide, `VR-000001` would go stale and nothing here would
//!   catch it.
//! - It matches on `--card-id`/`--gate-id` token equality and consumption
//!   order only. A `gate reserve` that appears earlier in the text but whose
//!   reservation would actually have expired or been abandoned before the
//!   matching `gate run` line runs still counts as available here — this
//!   rule checks the document's own internal consistency (does it *say* to
//!   reserve, once, before each run that claims it) not whether the
//!   described sequence would actually succeed against a live control
//!   repository moment to moment.
//!
//! # The vacuity guard (`tests/skill_guide.rs:129`'s pattern, once per property)
//!
//! A parser that silently matched zero commands would make the shape check
//! pass on a document with nothing left to check; a sequence rule that finds
//! zero `gate run` invocations would make the sequence check pass the same
//! way. Both tests below assert their own precondition is non-empty before
//! asserting anything about its content, exactly as `tests/skill_guide.rs`
//! does at the line cited above — now against the whole document, matching
//! the scope above.
//!
//! The two guards are no longer symmetric under every mutation, and that is
//! disclosed rather than hidden: deleting only "## Operator workflow"'s
//! fenced blocks empties the *sequence* property's whole search space
//! (`gate run`/`gate reserve` exist nowhere else in the document today), so
//! its guard still fires. The *shape* property's search space is the whole
//! document, which still has six real, unrelated invocations left standing
//! elsewhere — so the shape check correctly finds real commands and passes,
//! rather than firing a guard that would be checking a false premise (that
//! nothing was left to check, when six real lines were). At least one guard
//! catching a defective document is what the vacuity requirement is for;
//! see the mutation table in this card's report for the exact test names
//! each mutation now produces.

use std::collections::HashMap;
use std::process::Command;

/// Reads `README.md` from the repository root.
fn readme_md() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/README.md");
    std::fs::read_to_string(path).expect("README.md should be readable at the repository root")
}

/// One `change-harness ...` invocation parsed from a fenced `bash` block,
/// paired with the 1-based line number in the source text where it begins —
/// kept so a failure can name an exact `README.md:NNN` line, which
/// `tests/skill_guide.rs`'s `Vec<Vec<String>>` return type does not carry.
#[derive(Debug)]
struct Invocation {
    line: usize,
    tokens: Vec<String>,
}

impl Invocation {
    /// Reconstructs the invocation as space-joined text for messages. Lossy
    /// on purpose, same as the precedent: a quoted `--objective "First
    /// slice"` value loses its quotes when rejoined. Fine for a diagnostic,
    /// never compared against.
    fn text(&self) -> String {
        self.tokens.join(" ")
    }

    /// The two tokens naming the subcommand path, e.g. `(Some("gate"),
    /// Some("run"))`. `tokens[0]` is always the literal `"change-harness"`.
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
/// `--objective "First slice"` value survives as one token.
///
/// Copied from `tests/skill_guide.rs` / `tests/skill_guide_execution.rs`
/// (identical in both). Not imported: a Rust integration test file is its
/// own crate, so it can only reach another test file's items through a
/// shared module under `tests/support/`, and `tests/skill_guide_execution.rs`
/// already made the call that a ~15-line helper is simpler kept in sync by
/// hand than promoted there for one more caller. This file is that one more
/// caller, once removed.
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
/// present. Copied from `tests/skill_guide_execution.rs`'s helper of the
/// same name and behavior, for the same reason `split_invocation` above is
/// copied rather than shared.
fn find_flag_value<'a>(tokens: &'a [String], flag: &str) -> Option<&'a str> {
    tokens
        .windows(2)
        .find_map(|pair| (pair[0] == flag).then_some(pair[1].as_str()))
}

/// Parses every `change-harness ...` invocation out of fenced `bash` blocks
/// anywhere in `lines`, in source order, each paired with its 1-based line
/// number within `lines`. No heading or section scoping: see the module
/// doc's "Scope" section for why this file scans the whole document.
///
/// Differs from `tests/skill_guide.rs` and `tests/skill_guide_execution.rs`'s
/// same-named helper in one way, required by what `README.md` actually
/// contains rather than by preference: it joins a trailing `\`
/// continuation with the line that follows (and chains across more than one
/// continuation line, as `README.md`'s "## Three repositories" section's
/// five-line `project init` uses), instead of treating each continuation
/// line as a separate non-matching line. The precedent's own module doc
/// says plainly "no `\` continuations", and verified against the built
/// binary, a naive line-by-line parse of a `\`-continued command with
/// `--help` appended exits 2 (`error: unexpected argument '\' found`)
/// *before* `--help` gets a chance to fire. Without joining, the shape check
/// below would fail on a today-correct `README.md` for a reason that has
/// nothing to do with card #144's defect.
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
                    tokens: split_invocation(&acc),
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
                    tokens: split_invocation(trimmed),
                }),
            }
        }
    }

    // Defensive only: every continuation in the real document resolves
    // before its fenced block closes (confirmed by reading `README.md`
    // while writing this file). A synthetic test fixture that left a `\`
    // dangling would otherwise silently lose that invocation instead of
    // surfacing it.
    if let Some((start_line, acc)) = pending {
        invocations.push(Invocation {
            line: start_line,
            tokens: split_invocation(&acc),
        });
    }

    invocations
}

/// Parses every `change-harness ...` invocation anywhere in `readme` — every
/// fenced `bash` block in the whole document, regardless of which heading it
/// falls under. See the module doc's "Scope" section for why.
fn all_invocations(readme: &str) -> Vec<Invocation> {
    let lines: Vec<&str> = readme.lines().collect();
    parse_invocations(&lines)
}

/// The sequence property, checked across the whole document passed as
/// `invocations`: every `gate run` must be preceded by an *unconsumed*
/// `gate reserve` naming the same `--card-id` and `--gate-id`, and must
/// itself carry `--reservation-id`.
///
/// "Unconsumed" is the load-bearing word, not a simplification. `SKILL.md`
/// states the semantics this rule enforces: "a reservation authorizes one
/// expensive validation attempt, not standing permission" (`SKILL.md:295-296`).
/// A `HashSet`-based "has *some* earlier reserve for this pair ever been
/// seen" rule would let one correctly-reserved-and-run pair in
/// "## Operator workflow" silently authorize *every* later `gate run` reusing
/// the same illustrative `F-001`/`gate.unit` ids anywhere else in the
/// document — which is exactly the shape of the defect a reviewer found (see
/// the module doc). This function instead tracks, per `(card_id, gate_id)`,
/// how many reservations are currently *available*: incremented by
/// `gate reserve`, decremented by the next `gate run` that claims one. A
/// `gate reserve`/`gate run` pair can satisfy at most one `gate run`; a
/// second reserve for the same card and gate authorizes exactly one more.
///
/// Returns one human-readable violation per failing `gate run`; an empty vec
/// means the property holds for every documented `gate run` in
/// `invocations`. See this file's module doc for what this rule still
/// cannot see.
fn sequence_violations(invocations: &[Invocation]) -> Vec<String> {
    let mut available: HashMap<(String, String), u32> = HashMap::new();
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
                    "README.md:{}: `{}` does not carry `--reservation-id`",
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
                            "README.md:{}: `{}` is not preceded by an unconsumed `gate reserve \
                             --card-id {card_id} --gate-id {gate_id}` earlier in README.md",
                            invocation.line,
                            invocation.text(),
                        ));
                    }
                }
                _ => violations.push(format!(
                    "README.md:{}: `{}` is missing --card-id or --gate-id, so this rule cannot confirm \
                     it was ever reserved",
                    invocation.line,
                    invocation.text(),
                )),
            }
        }
    }

    violations
}

/// Every `change-harness` line anywhere in `README.md` is a real subcommand
/// with real flags — the **shape** property, checked document-wide. Catches
/// a renamed flag, a misspelled subcommand, or an invented option, by the
/// identical mechanism `tests/skill_guide.rs` uses on `SKILL.md`. Cannot
/// catch the sequence defect this card exists for; see the module doc.
#[test]
fn readme_commands_are_real() {
    let readme = readme_md();
    let invocations = all_invocations(&readme);

    // Vacuity guard: a parser that silently matched nothing would let this
    // test pass no matter what the document said.
    assert!(
        !invocations.is_empty(),
        "found zero `change-harness` invocations anywhere in README.md; either the document lost its \
         commands or the fenced-`bash`-block convention this file expects was not followed"
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
            "README.md:{}: `{}` is not a real command shape (exit {:?}):\nstdout: {}\nstderr: {}",
            invocation.line,
            invocation.text(),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

/// `README.md` never documents a `gate run` that is not provably reserved —
/// the **sequence** property, checked document-wide, and the one actually
/// broken at this card's base commit (`README.md:272`). `--help` cannot see
/// this defect (see the module doc); this test uses [`sequence_violations`]
/// instead, a structural rule over the parsed invocations, without executing
/// any of them.
#[test]
fn readme_never_documents_an_unreserved_gate_run() {
    let readme = readme_md();
    let invocations = all_invocations(&readme);

    // Vacuity guard, the sequence-property twin of the shape check's guard
    // above: a rule that finds zero `gate run` invocations to examine would
    // report zero violations no matter what the document said.
    let gate_run_count = invocations
        .iter()
        .filter(|invocation| invocation.subcommand() == (Some("gate"), Some("run")))
        .count();
    assert!(
        gate_run_count > 0,
        "found zero `gate run` invocations anywhere in README.md; a sequence rule with nothing to \
         check would pass vacuously no matter what the document said"
    );

    let violations = sequence_violations(&invocations);
    assert!(
        violations.is_empty(),
        "README.md documents at least one `gate run` that is not provably reserved:\n{}",
        violations.join("\n")
    );
}

/// [`sequence_violations`] itself is correct, decoupled from whatever
/// `README.md` currently says — mirrors why `tests/skill_guide_execution.rs`
/// has its own `a_correctly_matched_reserve_and_run_pair_is_accepted`.
/// Without this, a rule that rejected *everything* would still make the
/// test above "detect" card #144's defect, for a reason that has nothing to
/// do with actually checking reserve-before-run. Exercises the rule
/// directly against synthetic fenced blocks — pure parsing and hash-map
/// bookkeeping, no process spawned — covering the shapes the module doc
/// promises the rule can and cannot see: a correctly matched pair, a run
/// missing `--reservation-id`, a run with no preceding reserve at all, a run
/// whose only preceding reserve names a different gate, a reservation
/// already spent by an earlier run, and two independently reserved runs for
/// the same card and gate (proving the consumption rule is "one reserve, one
/// run, repeatable", not "at most once ever").
#[test]
fn gate_run_reservation_rule_is_not_a_rubber_stamp() {
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
    assert_eq!(
        correctly_matched.len(),
        2,
        "fixture should parse to 2 invocations"
    );
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
    assert_eq!(
        violations.len(),
        1,
        "a `gate run` missing `--reservation-id` should be exactly one violation: {violations:?}"
    );
    assert!(
        violations[0].contains("--reservation-id"),
        "the violation should name the missing flag: {violations:?}"
    );

    let no_preceding_reserve = invocations_from(
        "```bash\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000001\n\
         ```\n",
    );
    let violations = sequence_violations(&no_preceding_reserve);
    assert_eq!(
        violations.len(),
        1,
        "a `gate run` with no preceding `gate reserve` at all should be exactly one violation: {violations:?}"
    );
    assert!(
        violations[0].contains("not preceded by"),
        "the violation should say it found no preceding reserve: {violations:?}"
    );

    let mismatched_gate = invocations_from(
        "```bash\n\
         change-harness gate reserve --card-id F-900 --gate-id gate.other\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000001\n\
         ```\n",
    );
    let violations = sequence_violations(&mismatched_gate);
    assert_eq!(
        violations.len(),
        1,
        "a reserve for a different gate must not satisfy this run: {violations:?}"
    );
    assert!(
        violations[0].contains("not preceded by"),
        "the violation should say it found no matching preceding reserve: {violations:?}"
    );

    // The consumption case: a single reservation authorizes exactly one
    // `gate run`. A second, later `gate run` for the same card and gate is
    // not covered by a reservation an earlier run already spent — this is
    // the exact shape of the defect the reviewer's mutation demonstrated:
    // README's own "## Operator workflow" section correctly reserves and
    // runs `gate.unit` for `F-001`; a second, unrelated `gate run` reusing
    // those same illustrative ids later in the document, with no fresh
    // reserve of its own, must still be flagged.
    let reservation_consumed_by_first_run = invocations_from(
        "```bash\n\
         change-harness gate reserve --card-id F-900 --gate-id gate.check\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000001\n\
         change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000002\n\
         ```\n",
    );
    let violations = sequence_violations(&reservation_consumed_by_first_run);
    assert_eq!(
        violations.len(),
        1,
        "only the second, unreserved run should be flagged - the one reservation was already spent by \
         the first: {violations:?}"
    );
    assert!(
        violations[0].contains("VR-000002") && violations[0].contains("not preceded by"),
        "the violation should be about the second run specifically: {violations:?}"
    );

    // The repeatability case, so the consumption rule above is not
    // over-corrected into "at most once ever": two independently reserved
    // runs for the same card and gate must both be accepted.
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
