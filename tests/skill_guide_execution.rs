//! Executes `SKILL.md`'s gate-reserve/gate-run sequence for real, instead of
//! only appending `--help` to it.
//!
//! `tests/skill_guide.rs` proves every command *shape* in a covered section
//! is real by parsing it out of the guide and appending `--help`; `clap`
//! validates syntax before `--help` short-circuits execution, so a renamed
//! flag or an invented subcommand fails loudly. It cannot see that two
//! individually well-formed lines *contradict* each other. `SKILL.md`'s
//! "Prove the narrow feature claim first" step reserves a gate as one actor
//! and runs it with that reservation as the same actor:
//!
//! ```text
//! change-harness gate reserve --card-id F-001 --gate-id gate.fmt --actor implementer-a
//! change-harness gate run --card-id F-001 --gate-id gate.fmt --reservation-id VR-000001 --actor implementer-a
//! ```
//!
//! If the second line's `--actor` ever named someone else, `--help` would
//! still accept both lines individually — the policy refusal
//! (`live_reservation_for_run` in `src/commands/gate.rs`, "only reservation
//! holder ... may run ...") only fires on a real execution, after argument
//! parsing has already succeeded. This file runs both lines for real — no
//! `--help`, no injected flags beyond the `CHANGE_HARNESS_CONTROL`
//! environment variable `SKILL.md`'s own "Orient first" section tells an
//! operator to export before typing any of these commands — against a live
//! fixture project, in the order the guide writes them.
//!
//! Scoped to this one `### ` step rather than all of `## Lifecycle`: every
//! other fenced block in that section either names a file whose content the
//! guide never shows in full (`gate.test.json`, `verdict.yaml`) or embeds a
//! placeholder such as `<exact 40-hex cycle baseline>` that cannot be run
//! literally. This step's two lines are the exception — fully concrete, no
//! placeholder — which is what makes literal execution possible at all.
//!
//! Deliberately does not use `Workspace::gate()`: that helper auto-reserves
//! and rewrites `--reservation-id` for any `gate run` missing one (see its
//! doc comment in `tests/support/mod.rs`), which would silently repair
//! exactly the defect this file exists to catch. Every documented line below
//! runs through a real `std::process::Command` instead, completely
//! unmodified from what `command_invocations` parsed out of the guide.

mod support;

use std::process::{Command, Output};

use change_harness::commands::CONTROL_ENV;
use support::Workspace;

/// The exact heading `SKILL.md` uses for the sequence this file checks.
const SUBSECTION_HEADING: &str = "### 5. Prove the narrow feature claim first";

/// Reads `SKILL.md` from the repository root.
fn skill_md() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/SKILL.md");
    std::fs::read_to_string(path).expect("SKILL.md should be readable at the repository root")
}

/// Slices the exact subsection starting at `heading` up to the next `### `
/// or `## ` heading, or end of file.
///
/// Narrower than `tests/skill_guide.rs`'s `section()`, which deliberately
/// spans every `### ` inside one `## ` heading — right for a shape-only
/// `--help` check across a whole section, wrong here: see this file's module
/// doc comment for why only one `### ` step is safe to execute literally.
fn subsection<'a>(skill_md: &'a str, heading: &str) -> &'a str {
    let start = skill_md
        .find(heading)
        .unwrap_or_else(|| panic!("SKILL.md should contain the heading {heading:?}"));
    let after_heading = &skill_md[start + heading.len()..];
    let next_sub = after_heading.find("\n### ");
    let next_top = after_heading.find("\n## ");
    let end = [next_sub, next_top]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(after_heading.len());
    &after_heading[..end]
}

/// Splits one line into argv-style tokens, honoring `"..."` spans so a
/// quoted value survives as one token.
///
/// Identical to `tests/skill_guide.rs`'s helper of the same name: an
/// integration test binary cannot import another one directly, only shared
/// code in `tests/support/mod.rs`, and this ~15-line function is simpler
/// kept in sync by hand than promoted there for one caller outside it.
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
/// in `section`, in source order. See `split_invocation`'s doc comment.
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

/// Runs a parsed `change-harness ...` invocation for real: no `--help`, no
/// injected `--control` or `--output` flag on the command line. `SKILL.md`'s
/// "Orient first" section tells an operator to `export
/// CHANGE_HARNESS_CONTROL` once and then type every later command exactly as
/// written, so the control repository is supplied the same way here: through
/// the environment, never as an extra token this test adds on top of what
/// the guide actually says to type.
fn run_documented_invocation(control: &std::path::Path, tokens: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(&tokens[1..])
        .env(CONTROL_ENV, control)
        .output()
        .expect("the CLI binary should start")
}

/// The value following a `--flag` in a parsed invocation's own tokens, if
/// present. Deliberately `Option`, not a panicking lookup: a documented line
/// that dropped the flag entirely (§10 mutation 2, the missing-flag case) is
/// itself the condition under test, and must reach the real CLI rather than
/// fail inside this helper before execution ever happens — the difference
/// between catching a defect and merely detecting that this file's own
/// bookkeeping could not proceed.
fn find_flag_value<'a>(tokens: &'a [String], flag: &str) -> Option<&'a str> {
    tokens
        .windows(2)
        .find_map(|pair| (pair[0] == flag).then_some(pair[1].as_str()))
}

/// Builds the project state the guide's own prior steps establish: `gate.fmt`
/// registered (step 1), cycle `C-001` activated (step 2), card `F-001`
/// activated naming `gate.fmt` as its only feature gate (step 3), and
/// `implementer-a` holding an allocated worktree with a real commit in it
/// (step 4).
///
/// None of this is parsed *from* `SKILL.md` text — steps 1-4 carry
/// placeholders step 5 does not (see the module doc comment) — it just gets
/// a real project to the state an operator who had actually done steps 1-4
/// would be standing in before typing step 5's two lines.
fn fixture_ready_for_the_narrow_claim() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.register_gate("gate.fmt", &["true"]);
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "One-line objective",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card_with_gates("F-001", &["src/thing.rs"], &["gate.fmt"]);
    workspace.bind_fixture_plan_with_assignment("PLAN-SKILL-001", "parallel", "implementer-a");
    workspace.work(&["start", "--card-id", "F-001", "--actor", "implementer-a"]);

    let worktree = workspace.worktrees.join("F-001");
    let file = worktree.join("src/thing.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "// thing\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: thing"]);

    workspace
}

/// The documented gate sequence executes in order.
///
/// Fails, and names exactly why, if: the doc's `gate run --actor` differs
/// from its `gate reserve --actor` on the line above (mismatched actor:
/// `live_reservation_for_run` refuses to let anyone but the reservation's
/// holder run it — this is §1's acceptance-test mutation); the doc's `gate
/// run` drops `--reservation-id` (missing flag: `run_gate` refuses
/// unconditionally); the two lines are reordered (the order assertions
/// below fail with a message naming which line moved, before either command
/// even runs); or the doc's hardcoded `--reservation-id VR-000001` stops
/// being the id a fresh reservation for this exact card and gate actually
/// receives.
#[test]
fn documented_gate_sequence_executes_in_the_order_written() {
    let skill_md = skill_md();
    let body = subsection(&skill_md, SUBSECTION_HEADING);
    let invocations = command_invocations(body);

    // A parser that silently matched nothing (a moved heading, a fence typo)
    // would let this test pass vacuously no matter what the section said.
    // `tests/skill_guide.rs` guards the identical failure mode for the same
    // reason.
    assert_eq!(
        invocations.len(),
        2,
        "expected exactly the `gate reserve` then `gate run` pair under {SUBSECTION_HEADING:?}, found {invocations:?}"
    );
    let reserve = &invocations[0];
    let run = &invocations[1];
    assert_eq!(
        (reserve[1].as_str(), reserve[2].as_str()),
        ("gate", "reserve"),
        "the first documented line should be `gate reserve`: {reserve:?}"
    );
    assert_eq!(
        (run[1].as_str(), run[2].as_str()),
        ("gate", "run"),
        "the second documented line should be `gate run`: {run:?}"
    );

    let workspace = fixture_ready_for_the_narrow_claim();

    let reserve_output = run_documented_invocation(&workspace.control, reserve);
    assert!(
        reserve_output.status.success(),
        "documented `{}` failed (exit {:?}):\nstdout: {}\nstderr: {}",
        reserve.join(" "),
        reserve_output.status.code(),
        String::from_utf8_lossy(&reserve_output.stdout),
        String::from_utf8_lossy(&reserve_output.stderr),
    );

    // The doc hardcodes `VR-000001` on the strength of it being the id a
    // fresh reservation for this card and gate actually receives. Prove
    // that rather than assume it: when the documented `gate run` line still
    // carries `--reservation-id`, the real reserve output must name the
    // exact id it hardcodes. Conditional on purpose: a documented line that
    // dropped the flag entirely (§10 mutation 2) is a different defect, and
    // the unconditional real execution below is what must catch that one —
    // not a comparison against a value that no longer exists in the text.
    if let Some(documented_reservation_id) = find_flag_value(run, "--reservation-id") {
        let reserve_stdout = String::from_utf8_lossy(&reserve_output.stdout);
        assert!(
            reserve_stdout.contains(&format!(
                "validation reservation {documented_reservation_id}"
            )),
            "the documented `--reservation-id {documented_reservation_id}` does not match what a \
             fresh reservation for this card and gate actually receives; reserve reported: {reserve_stdout}"
        );
    }

    // Run the documented second line exactly as parsed, whatever it is —
    // including, if a future edit reintroduces §10 mutation 2, with no
    // `--reservation-id` at all. `run_gate` (src/commands/gate.rs) refuses
    // that itself, unconditionally, before this test's own bookkeeping ever
    // gets a chance to; this assertion is what must go red for both that
    // mutation and the mismatched-actor one, on the real CLI's own refusal.
    let run_output = run_documented_invocation(&workspace.control, run);
    assert!(
        run_output.status.success(),
        "documented `{}` failed (exit {:?}):\nstdout: {}\nstderr: {}",
        run.join(" "),
        run_output.status.code(),
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr),
    );
    let run_stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(
        run_stdout.contains("verdict: PASS"),
        "documented `{}` exited 0 but did not report a passing verdict: {run_stdout}",
        run.join(" ")
    );

    // Not merely "exit 0 and said PASS": confirm a real, current receipt for
    // exactly this gate landed on the card's record. This is the assertion
    // that would catch the doc naming a `--gate-id` the card's own
    // `named_gates.feature` does not declare — a mismatch `--help` also
    // cannot see, and a different shape of contradiction than the actor
    // mismatch above.
    let status = workspace.gate_json(&["status", "--card-id", "F-001"]);
    let feature_gates = status["data"]["feature_gates"]
        .as_array()
        .expect("gate status should report the card's feature gates");
    let fmt_gate = feature_gates
        .iter()
        .find(|gate| gate["gate_id"] == "gate.fmt")
        .unwrap_or_else(|| panic!("gate.fmt should be a feature gate for F-001: {status}"));
    assert_eq!(
        fmt_gate["satisfied"], true,
        "the documented `gate run` should have produced a satisfying receipt: {status}"
    );
    assert_eq!(
        fmt_gate["runs"], 1,
        "expected exactly one recorded run of gate.fmt: {status}"
    );
}

/// The no-false-positive test: a correct sequence passes.
///
/// Without this, a checker that fails unconditionally — a bug in this
/// file's own fixture wiring, not any real problem with `SKILL.md` — would
/// still make the test above "detect" the mismatched-actor mutation, for a
/// reason that has nothing to do with catching mismatched actors. Genuinely
/// decoupled from `SKILL.md`'s content so it cannot pass merely because
/// today's guide happens to be right: a different card, gate, and actor
/// than the real guide names, run through the identical parsing and
/// execution helpers the test above uses.
///
/// Fails if: `run_documented_invocation` stops passing the control
/// repository through correctly, `command_invocations` stops parsing a
/// well-formed fenced block, or the harness itself starts refusing a
/// reservation and run pair whose actors genuinely agree.
#[test]
fn a_correctly_matched_reserve_and_run_pair_is_accepted() {
    let synthetic = "\n\
        ```bash\n\
        change-harness gate reserve --card-id F-900 --gate-id gate.check --actor solo-operator\n\
        change-harness gate run --card-id F-900 --gate-id gate.check --reservation-id VR-000001 --actor solo-operator\n\
        ```\n";
    let invocations = command_invocations(synthetic);
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
    workspace.bind_fixture_plan_with_assignment("PLAN-SKILL-002", "parallel", "solo-operator");
    workspace.work(&["start", "--card-id", "F-900", "--actor", "solo-operator"]);

    let worktree = workspace.worktrees.join("F-900");
    let file = worktree.join("src/other.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "// other\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: other"]);

    for tokens in &invocations {
        let output = run_documented_invocation(&workspace.control, tokens);
        assert!(
            output.status.success(),
            "a genuinely correctly-matched `{}` must be accepted (exit {:?}):\nstdout: {}\nstderr: {}",
            tokens.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
