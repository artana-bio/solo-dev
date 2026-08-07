//! #153: #151 added routing so an agent reading `SKILL.md` reaches for a
//! generator instead of hand-writing a document — `gate example`, `review
//! example`, and `project example` are each named inside a fenced ```bash```
//! block there. `project example-final-authorization` shipped in #143, in
//! parallel with #151, and #151's author had no way to know it existed: it
//! is named nowhere in `SKILL.md`. It is the generator for the document
//! that produced the only unnavigable refusal in the second independent
//! cold-start run — an operator who could not learn `authorization_unit`
//! accepts exactly `"sealed_cycle"` from the message, from `SKILL.md`, or
//! from `project example`, and resolved it by running `strings` on the
//! binary.
//!
//! `tests/skill_guide.rs` verifies that commands `SKILL.md` names are real.
//! Nothing verified the converse — that a generator the CLI ships is named
//! where an operator would look. This file is that check: every
//! `example`/`example-*` leaf command the CLI's `--help` tree exposes today
//! must be named in `SKILL.md`, or this test fails, naming exactly which one
//! is missing.
//!
//! # What "named in `SKILL.md`" means here
//!
//! Three bars were available, in increasing strength: (1) the bare command
//! string appearing anywhere in the file, which would pass on a command
//! mentioned only in a footnote; (2) the string appearing inside a fenced
//! ```bash``` block, which is how #151 wrote the other three routes; (3)
//! appearing specifically within the section that documents that command's
//! document kind, which is the strongest but requires a maintained
//! doc-kind-to-heading mapping.
//!
//! This file uses bar (2): a leaf command with path segments `[s1, ..., sn]`
//! is routed when some line inside a fenced ```bash``` block anywhere in
//! `SKILL.md`, split on whitespace, opens with `change-harness s1 ... sn`.
//! [`is_routed`] compares token-by-token rather than doing a raw substring
//! search, so a command name that is a prefix of another's (`project
//! example` versus `project example-final-authorization`) cannot cross-match
//! in either direction, and a mention buried inside another command's own
//! quoted argument text does not count. See
//! `is_routed_does_not_match_a_mention_inside_another_commands_argument`
//! below for exactly that case.
//!
//! # What this rule does not check
//!
//! - **Which section carries the routing.** Bar (2) does not ask whether the
//!   command sits in the section that actually explains its document kind —
//!   only that it appears in *some* fenced block *somewhere*. A future edit
//!   that moved a routing line into an unrelated section would keep passing
//!   this test. This is a deliberate choice, not an oversight: enforcing bar
//!   (3) instead means asserting a document-kind-to-heading table that this
//!   file would then own, and that nobody is forced to update when a new
//!   document kind or a renamed section appears — the same staleness shape
//!   #153 exists to close, only moved one level up rather than eliminated.
//!   `tests/skill_guide.rs` and this file both leave "is this well-organized"
//!   to review, and check only "is it real" (that file) or "is it present"
//!   (this one).
//! - **Whether the surrounding prose is accurate or complete.** A command
//!   string alone inside a fence says nothing about whether the guide text
//!   around it is honest — in particular, whether it warns that `project
//!   example-final-authorization`'s `authorizer_actor_ids` are placeholder
//!   slot names that must be edited before the installed document
//!   authorizes anything real. That is an editorial judgment, satisfied here
//!   only by inspection when the routing text is written, not by assertion.
//! - **Whether the command actually runs**, the way `tests/skill_guide.rs`
//!   checks every invocation in its `COVERED_SECTIONS` by running it with
//!   `--help` appended. This file only checks presence of the text. A typo
//!   in a routed command string is caught here only indirectly — because the
//!   *correctly spelled* string is then absent, so this test fails — not
//!   because the *corrupted* spelling was itself flagged as an invalid CLI
//!   invocation; that is `tests/skill_guide.rs`'s job, and only for a
//!   corrupted string that happens to land inside one of its
//!   `COVERED_SECTIONS`.
//!
//! # The CLI walk
//!
//! [`walk`], [`child_command_names`], and [`help_text`] are the same shape as
//! `tests/config_malformed_example_claims.rs`'s functions of the same names
//! — copied and simplified for this file's narrower need (every leaf's path
//! segments, not its about text), per that trio's own established
//! convention of duplicating this logic across files rather than sharing it
//! (see that file's own module doc for why).
//!
//! # Guarding against vacuity
//!
//! A walk that silently visited nothing, or found no `example` leaves, would
//! make the routing assertion below pass no matter what `SKILL.md` said.
//! [`MIN_PAGES_WALKED`] and [`MIN_EXAMPLE_LEAVES_FOUND`] guard both counts;
//! see their doc comments for exactly how each number was chosen.

use std::process::Command;

// ---------------------------------------------------------------------
// Vacuity floors.
// ---------------------------------------------------------------------

/// Floor on `--help` pages visited by [`walk`]. A fresh walk on this card's
/// base (`be22975`) measured 94. `tests/config_malformed_example_claims.rs`
/// measured the same 94 on the commit it started from and chose 20 as its
/// own floor for the identical CLI; this file reuses that number for the
/// same reason: comfortably below 94, so trimming some commands over time
/// will not trip it, and comfortably above what a broken walk produces —
/// even a walk that visited the root and every one of the CLI's 15
/// top-level pages without recursing any further (the shape of this card's
/// required "enumerates only the top level" mutation) stops at 16.
const MIN_PAGES_WALKED: usize = 20;

/// Floor on `example`/`example-*` leaves [`walk`] finds. A fresh walk found
/// exactly 4 today (`gate example`, `project example`, `project
/// example-final-authorization`, `review example`); this floor matches that
/// count exactly rather than settling for "at least one", so a walk that
/// silently lost track of one of today's four commands is caught, not only
/// a walk that finds none. It is still a floor, not an exact-equality
/// assertion, so a fifth `example` command added later does not fail this
/// test on the day it ships.
const MIN_EXAMPLE_LEAVES_FOUND: usize = 4;

// ---------------------------------------------------------------------
// CLI walk: discovers every `example`/`example-*` leaf command by driving
// `--help` recursively, the way an operator would. Same shape as
// `tests/config_malformed_example_claims.rs`'s functions of the same names;
// duplicated per that file's own documented convention rather than shared.
// ---------------------------------------------------------------------

/// Runs `change-harness <path> --help` and returns its stdout.
fn help_text(path: &[String]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_change-harness"))
        .args(path)
        .arg("--help")
        .output()
        .expect("the CLI binary should start");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Every subcommand name listed under a `--help` page's `Commands:` section,
/// excluding clap's auto-generated `help`. Empty for a leaf command (one
/// with only an `Options:` section).
fn child_command_names(help: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        if line.starts_with("Commands:") {
            in_commands = true;
            continue;
        }
        if !in_commands {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with("  ") {
            break;
        }
        if let Some(name) = line.split_whitespace().next()
            && name != "help"
        {
            out.push(name.to_owned());
        }
    }
    out
}

/// Recursively walks the CLI's subcommand tree from `path`, incrementing
/// `visited` once per `--help` page fetched and appending the full path to
/// `examples` for every leaf whose last segment is `example` or starts with
/// `example-`.
fn walk(path: &[String], visited: &mut usize, examples: &mut Vec<Vec<String>>) {
    let help = help_text(path);
    *visited += 1;
    let children = child_command_names(&help);
    if children.is_empty() {
        if let Some(last) = path.last()
            && (last == "example" || last.starts_with("example-"))
        {
            examples.push(path.to_vec());
        }
        return;
    }
    for name in children {
        let mut child = path.to_vec();
        child.push(name);
        walk(&child, visited, examples);
    }
}

// ---------------------------------------------------------------------
// SKILL.md scan.
// ---------------------------------------------------------------------

/// Reads `SKILL.md` from the repository root.
fn skill_md() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/SKILL.md");
    std::fs::read_to_string(path).expect("SKILL.md should be readable at the repository root")
}

/// Every line inside a fenced ```bash``` block in `skill_md`, trimmed, in
/// source order, across every such block in the file — not scoped to any
/// particular section, unlike `tests/skill_guide.rs`'s `COVERED_SECTIONS`.
/// See the module doc, "What this rule does not check", for why no section
/// scoping is applied here.
fn bash_block_lines(skill_md: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut in_block = false;
    for line in skill_md.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```bash") {
            in_block = true;
            continue;
        }
        if in_block && trimmed.starts_with("```") {
            in_block = false;
            continue;
        }
        if in_block {
            lines.push(trimmed);
        }
    }
    lines
}

/// True when `path` is named in `bash_lines` by this file's bar (see the
/// module doc, "What 'named in `SKILL.md`' means here"): some line, split on
/// whitespace, opens with the literal `change-harness` followed by every
/// segment of `path`, in order. A token-by-token comparison rather than a
/// substring search, so `project example` cannot cross-match `project
/// example-final-authorization` in either direction, and a mention inside
/// another command's quoted argument text does not count.
fn is_routed(bash_lines: &[&str], path: &[String]) -> bool {
    bash_lines.iter().any(|line| {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 1 + path.len() || tokens[0] != "change-harness" {
            return false;
        }
        tokens[1..=path.len()]
            .iter()
            .zip(path.iter())
            .all(|(token, segment)| *token == segment.as_str())
    })
}

// ---------------------------------------------------------------------
// The guard.
// ---------------------------------------------------------------------

#[test]
fn every_example_family_command_is_routed_from_skill_md() {
    // 1. Walk the CLI from the root, exactly as an operator would explore
    //    it, never from a hard-coded list — see #153 §6 and this file's
    //    module doc.
    let mut visited = 0usize;
    let mut examples: Vec<Vec<String>> = Vec::new();
    walk(&[], &mut visited, &mut examples);

    // 2. Vacuity guards: a broken walk must fail loudly rather than pass
    //    with nothing to check.
    assert!(
        visited > MIN_PAGES_WALKED,
        "walked only {visited} --help pages (floor {MIN_PAGES_WALKED}); a fresh walk on this \
         card's base measured 94 — the walk is very likely broken (child_command_names no \
         longer parsing the `Commands:` section, or the recursive call into child commands \
         removed), not the CLI having shrunk this much"
    );
    assert!(
        examples.len() >= MIN_EXAMPLE_LEAVES_FOUND,
        "found only {} `example`/`example-*` leaf command(s) after walking {visited} --help \
         pages (floor {MIN_EXAMPLE_LEAVES_FOUND}); a fresh walk on this card's base found \
         exactly 4 (`gate example`, `project example`, `project example-final-authorization`, \
         `review example`) — either one was renamed away from that convention, or the walk is \
         broken",
        examples.len()
    );

    // 3. Every example-family leaf the CLI ships must be named in SKILL.md.
    let skill_md = skill_md();
    let bash_lines = bash_block_lines(&skill_md);

    let mut unrouted = Vec::new();
    for path in &examples {
        if !is_routed(&bash_lines, path) {
            unrouted.push(format!("change-harness {}", path.join(" ")));
        }
    }

    assert!(
        unrouted.is_empty(),
        "these example-family commands the CLI ships are not named in any fenced ```bash``` \
         block in SKILL.md, so an operator reading the guide has no route to the generator that \
         would have produced their document instead of hand-writing it (see #153): {}",
        unrouted.join(", ")
    );
}

// ---------------------------------------------------------------------
// Unit tests for the pure SKILL.md-scanning logic this file adds.
// `walk` / `child_command_names` / `help_text` are exercised against the
// real binary by the integration test above, matching
// `tests/config_malformed_example_claims.rs`'s own convention of not
// separately unit-testing its CLI-driving functions.
// ---------------------------------------------------------------------

#[test]
fn bash_block_lines_collects_only_lines_inside_bash_fences() {
    let md = "\
prose before

```bash
change-harness gate example
change-harness gate register --definition gate.test.json
```

more prose, and a non-bash fence:

```yaml
not_bash: true
```

```bash
change-harness review example
```
";
    let lines = bash_block_lines(md);
    assert_eq!(
        lines,
        vec![
            "change-harness gate example",
            "change-harness gate register --definition gate.test.json",
            "change-harness review example",
        ]
    );
}

#[test]
fn is_routed_matches_token_by_token_not_by_substring() {
    let lines = vec!["change-harness project example"];
    let path = vec!["project".to_owned(), "example".to_owned()];
    assert!(is_routed(&lines, &path));

    let longer_sibling_path = vec![
        "project".to_owned(),
        "example-final-authorization".to_owned(),
    ];
    assert!(
        !is_routed(&lines, &longer_sibling_path),
        "`change-harness project example` must not satisfy its longer sibling command"
    );
}

#[test]
fn is_routed_does_not_match_a_mention_inside_another_commands_argument() {
    let lines = vec![
        "change-harness disposition abandon --card-id F-001 --actor coordinator --rationale \"see change-harness project example-final-authorization for details\"",
    ];
    let path = vec![
        "project".to_owned(),
        "example-final-authorization".to_owned(),
    ];
    assert!(
        !is_routed(&lines, &path),
        "a mention buried inside another command's argument text must not count as routing"
    );
}

#[test]
fn is_routed_is_false_when_the_command_is_absent() {
    let lines = vec!["change-harness gate example"];
    let path = vec![
        "project".to_owned(),
        "example-final-authorization".to_owned(),
    ];
    assert!(!is_routed(&lines, &path));
}
