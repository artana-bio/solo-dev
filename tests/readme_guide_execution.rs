//! Executes `README.md`'s documented review step for real, instead of only
//! appending `--help` to it — the identical mechanism
//! `tests/skill_guide_execution.rs` uses for `SKILL.md`'s gate-reserve/run
//! pair, and for the identical reason: `tests/readme_guide.rs` proves every
//! command *shape* in the document is real, but `clap` validates syntax and
//! lets `--help` short-circuit before any flag value is ever acted on, so
//! two individually well-formed, real-shaped lines can still contradict the
//! state the harness actually enforces once run for real. #120's own
//! defect — `review record --actor` accepted and never read — was exactly
//! this class: `--help` cannot see it, because `--actor` is a real, valid
//! flag on `record` either way.
//!
//! # What this reproduces
//!
//! `README.md`'s `## Operator workflow` documents, at what was line 364
//! before #120's repair:
//!
//! ```text
//! change-harness review begin  --control $CONTROL --card-id F-001
//! change-harness review record --control $CONTROL --card-id F-001 --verdict verdict.yaml
//! ```
//!
//! README never shows `verdict.yaml`'s content — like `decl.yaml`,
//! `F-001.yaml`, and `gate.unit.yaml` elsewhere in the same section (see
//! `tests/readme_guide.rs`'s module doc, "Why a structural rule instead of
//! executing the sequence"), it is one of the files this section assumes
//! rather than spells out. The one sanctioned source of a valid verdict
//! document anywhere in this codebase is `review example` (#108), so this
//! file builds `verdict.yaml` the same way an operator actually would: by
//! running `review example` for real and using its exact stdout, never a
//! hand-written approximation — matching `tests/review_example.rs`'s own
//! discipline.
//!
//! Before #120's repair, the documented `review record` line carried no
//! `--actor` at all. `--actor` defaults to `operator`, which disagreed with
//! the example verdict's declared `reviewer_actor_id: reviewer-example` —
//! refused, `CH-POLICY-INCOMPLETE-REVIEW`, for a reader who typed exactly
//! what the document said, on a command the document claimed would work.
//! The repair added `--actor reviewer-example`, matching `SKILL.md:368`'s
//! form (`--verdict verdict.yaml --actor <name>`) with the one name that is
//! actually correct here: the reviewer `review example` itself declares.
//!
//! # `$CONTROL`
//!
//! README's own text carries a literal, never-defined `$CONTROL` — the
//! "Quick install" section exports `CHANGE_HARNESS_CONTROL`, a different
//! name, for a different purpose (see `tests/readme_guide.rs`'s module
//! doc). `tests/skill_guide_execution.rs` avoids this entirely by choosing
//! a `SKILL.md` step that never spells `--control` at all, relying instead
//! on `CommonArgs`'s own `env = CHANGE_HARNESS_CONTROL` fallback. That
//! option does not exist here: both documented lines write a literal
//! `--control $CONTROL` token, and dropping it would not be running what
//! the document says. This file makes the one substitution a real shell
//! would make — the literal token `"$CONTROL"` is replaced with the
//! fixture's real control path, and nothing else about either line is
//! touched. Building a general mechanism to resolve `$CONTROL` for the
//! whole document is out of proportion to this one step, and was already
//! declined, for the whole file, by `tests/readme_guide.rs`'s own module
//! doc (`--control $CONTROL` is named there as "not runnable as written by
//! any mechanism short of a real shell with that variable exported"); this
//! is a narrower, disclosed, one-off adaptation for one documented pair,
//! not a reopening of that decision.
//!
//! Likewise `verdict.yaml` is a bare relative path in the documented line;
//! this file runs both documented commands with the fixture's root as the
//! child process's working directory and writes `verdict.yaml` there, so
//! the relative path resolves exactly as it would for an operator sitting
//! in the directory holding their other working files (`decl.yaml`,
//! `F-001.yaml`, ...).
//!
//! # Scope
//!
//! `README.md` is not in `#120`'s original file scope; this file exists
//! because the repair that followed found `README.md:364` itself refused —
//! the same class #144 was filed for, in the same document
//! (`README.md:272` documented a `gate run` refused since #105, and #144's
//! own contract proved a `--help`-only shape check cannot see that class).
//! `tests/readme_guide.rs` passing is not evidence against that class; it
//! is the exact thing #144 already showed it cannot be.

mod support;

use std::{fs, process::Command};

use support::Workspace;

/// Reads `README.md` from the repository root.
fn readme_md() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/README.md");
    std::fs::read_to_string(path).expect("README.md should be readable at the repository root")
}

/// Splits one line into argv-style tokens, honoring `"..."` spans so a
/// quoted value survives as one token.
///
/// Copied from `tests/readme_guide.rs` / `tests/skill_guide_execution.rs`
/// (identical in both): an integration test binary cannot import another
/// one's non-`pub` items, only shared code under `tests/support/`, and both
/// existing files already made the call that this ~15-line helper is
/// simpler kept in sync by hand than promoted there for one more caller.
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

/// Every `change-harness review begin` or `change-harness review record`
/// line, anywhere in `README.md`, in source order — the whole document, not
/// one section, matching `tests/readme_guide.rs`'s own scope argument: a
/// defect outside "## Operator workflow" would be exactly as invisible to a
/// reader as one inside it.
fn review_step_invocations(readme: &str) -> Vec<Vec<String>> {
    let mut invocations = Vec::new();
    let mut in_bash_block = false;
    for line in readme.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```bash") {
            in_bash_block = true;
            continue;
        }
        if in_bash_block && trimmed.starts_with("```") {
            in_bash_block = false;
            continue;
        }
        if in_bash_block
            && (trimmed.starts_with("change-harness review begin")
                || trimmed.starts_with("change-harness review record"))
        {
            invocations.push(split_invocation(trimmed));
        }
    }
    invocations
}

/// Runs a parsed invocation for real, substituting the one placeholder
/// README never defines (`$CONTROL`) for the fixture's real control path,
/// and nothing else — see this file's module doc. `--verdict verdict.yaml`
/// is a bare relative path in the document, so the child process's working
/// directory is set to `root`, where `verdict.yaml` is written.
fn run_documented_line(
    root: &std::path::Path,
    control: &std::path::Path,
    tokens: &[String],
) -> std::process::Output {
    let args: Vec<String> = tokens[1..]
        .iter()
        .map(|token| {
            if token == "$CONTROL" {
                control.display().to_string()
            } else {
                token.clone()
            }
        })
        .collect();
    Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(&args)
        .current_dir(root)
        .output()
        .expect("the CLI binary should start")
}

/// Builds the project state README's own earlier steps establish: gate
/// `gate.unit` registered (`Workspace::initialized` — step 1), cycle
/// `C-001` activated (step 2), card `F-001` activated (step 3), `operator`
/// holding an allocated worktree with a real commit and a passed gate (step
/// 4), and a handoff naming that exact commit (step 5) — the state an
/// operator who had actually done steps 1-5 would be standing in before
/// typing step 6's two lines. None of this is parsed *from* README's text;
/// see `tests/skill_guide_execution.rs`'s identical note on its own
/// `fixture_ready_for_the_narrow_claim`.
fn fixture_ready_for_review() -> Workspace {
    let workspace = Workspace::initialized();
    workspace.cycle(&[
        "create",
        "--cycle-id",
        "C-001",
        "--objective",
        "First slice",
    ]);
    workspace.cycle(&["activate", "--cycle-id", "C-001"]);
    workspace.activate_card("F-001", &["src/**"]);
    workspace.work(&["start", "--card-id", "F-001"]);

    let worktree = workspace.worktrees.join("F-001");
    let file = worktree.join("src/thing.rs");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "// thing\n").unwrap();
    support::git(&worktree, &["add", "-A"]);
    support::git(&worktree, &["commit", "-q", "-m", "feat: thing"]);
    workspace.gate(&["run", "--card-id", "F-001", "--gate-id", "gate.unit"]);

    let head = support::capture(&worktree, &["rev-parse", "HEAD"]);
    let declaration = workspace.root.join("decl.yaml");
    fs::write(
        &declaration,
        format!(
            "delivered_sha: {head}\nbehavior_delivered: adds thing.rs\nimplementation_decisions: [minimal]\nassumptions: []\nknown_limitations: []\nresidual_risks: []\nrollback_notes: revert\n"
        ),
    )
    .unwrap();
    workspace.handoff(&[
        "create",
        "--card-id",
        "F-001",
        "--declaration",
        &declaration.display().to_string(),
    ]);

    workspace
}

/// The documented review step — `review begin` then `review record`,
/// exactly as `README.md` writes them today — runs for real against a live
/// fixture, and the recorded review carries the exact attribution the
/// documented `--actor` names.
///
/// Fails, and names exactly why, if: README's `review record` line drops
/// `--actor` (or names one that disagrees with the verdict document's own
/// `reviewer_actor_id`) — `require_actor_agreement` refuses, unconditionally,
/// on the real CLI's own exit code, before this test's own assertions past
/// that point ever run; the two lines are reordered; or `review example`'s
/// emitted `reviewer_actor_id` ever stops being `reviewer-example`, the
/// value README's own `--actor` now names.
#[test]
fn documented_review_step_executes_and_records_the_documented_actor() {
    let readme = readme_md();
    let invocations = review_step_invocations(&readme);

    // A parser that silently matched nothing (a moved heading, a fence
    // typo, both lines deleted) would let this test pass vacuously no
    // matter what the document said — the same guard `tests/skill_guide.rs`
    // and `tests/skill_guide_execution.rs` both use for the identical
    // reason.
    assert_eq!(
        invocations.len(),
        2,
        "expected exactly the documented `review begin` then `review record` pair anywhere in \
         README.md, found {invocations:?}"
    );
    let begin = &invocations[0];
    let record = &invocations[1];
    assert_eq!(
        (begin[1].as_str(), begin[2].as_str()),
        ("review", "begin"),
        "the first documented line should be `review begin`: {begin:?}"
    );
    assert_eq!(
        (record[1].as_str(), record[2].as_str()),
        ("review", "record"),
        "the second documented line should be `review record`: {record:?}"
    );

    let workspace = fixture_ready_for_review();

    // The one sanctioned source of a valid verdict document: `review
    // example`'s real stdout, never a hand-written approximation — #108,
    // and `tests/review_example.rs`'s own discipline.
    let example = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(["review", "example"])
        .output()
        .expect("the CLI binary should start");
    assert!(
        example.status.success(),
        "`review example` failed (exit {:?}): {}",
        example.status.code(),
        String::from_utf8_lossy(&example.stderr)
    );
    fs::write(workspace.root.join("verdict.yaml"), &example.stdout).unwrap();

    let begin_output = run_documented_line(&workspace.root, &workspace.control, begin);
    assert!(
        begin_output.status.success(),
        "documented `{}` failed (exit {:?}):\nstdout: {}\nstderr: {}",
        begin.join(" "),
        begin_output.status.code(),
        String::from_utf8_lossy(&begin_output.stdout),
        String::from_utf8_lossy(&begin_output.stderr),
    );

    let record_output = run_documented_line(&workspace.root, &workspace.control, record);
    assert!(
        record_output.status.success(),
        "documented `{}` failed (exit {:?}):\nstdout: {}\nstderr: {}",
        record.join(" "),
        record_output.status.code(),
        String::from_utf8_lossy(&record_output.stdout),
        String::from_utf8_lossy(&record_output.stderr),
    );

    // Not merely "some command succeeded": the recorded review must carry
    // the exact attribution the documented `--actor` declared — the same
    // distinction `tests/review_example.rs` draws between a command
    // returning success and the example's own content actually landing.
    let inspected = workspace.review_json(&["inspect", "--card-id", "F-001"]);
    let reviews = inspected["data"]["reviews"]
        .as_array()
        .expect("the review history");
    assert_eq!(reviews.len(), 1, "the documented review must be recorded");
    assert_eq!(reviews[0]["decision"], "approved");
    assert_eq!(
        reviews[0]["reviewer_actor_id"], "reviewer-example",
        "the recorded review must carry the actor the documented --actor names, not a default"
    );
}
