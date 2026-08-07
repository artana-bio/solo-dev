//! #142 review round 2: six of the per-site recovery constants added in
//! `ca54c60` (`src/commands/{gate,audit,handoff}.rs`, `src/domain/card.rs`)
//! say "There is no generated example for a <kind> document" for a document
//! kind with no `example` subcommand today. That is a claim of *absence*,
//! and absence is exactly the kind of claim that silently rots: #143 added
//! `project example-final-authorization` three commits before this card
//! landed, and nothing would stop a future card adding
//! `gate example-mutation-campaign` from leaving one of these six sentences
//! lying to an operator, the same way `ErrorCode::convergence_recovery`
//! told operators a shipped command "is not part of this release" for two
//! releases (`tests/recovery_text.rs`'s own motivating incident) because
//! nothing checked it against the built binary.
//!
//! This file is that check, for the absence claim specifically — nothing
//! wider. It does not verify any other sentence in any recovery text;
//! `tests/recovery_override_text.rs` and `tests/recovery_text.rs` already
//! verify every *positive* command reference the same way this file
//! verifies the *negative* one.
//!
//! # The three pieces
//!
//! 1. **Enumerate `example` commands from the CLI itself.** [`walk`] drives
//!    `--help` recursively from the root, exactly the way an operator would
//!    discover the command tree, rather than hard-coding the four commands
//!    #142 found by hand. Confirmed live while writing this file: 94
//!    `--help` invocations, 4 leaves named `example` or `example-*`.
//! 2. **Extract each one's document kind from its own about text**, via the
//!    two templates every `example` command's doc comment happens to use
//!    today (`Print a complete, valid <kind> example.` — `project`,
//!    `gate` — and `...document.` — `review`). An about text matching
//!    neither template fails the test loudly rather than being silently
//!    skipped — see [`extract_kind_from_about`].
//! 3. **Extract every "no generated example" claim from `src/`** via the
//!    literal anchor `no generated example for a`/`an`, and assert none of
//!    the claimed kinds, normalized, matches a kind the CLI now covers.
//!
//! # Why this anchor
//!
//! The claim's prose is uniform across all six constants:
//! `"There is no generated example for a <kind> document; ..."`. The
//! anchor is the fixed part of that sentence, not the whole recovery text —
//! narrower than scanning for the word "example" anywhere (which would
//! also match the *positive* `` `project example` `` references) and wide
//! enough to survive the varying `<kind>` and the trailing clause.
//!
//! # Normalization
//!
//! [`normalize`] lowercases and treats `-` as a word boundary, so
//! `convergence-policy` (an about text's phrasing) and `convergence policy`
//! (a recovery constant's phrasing) — the exact mismatch #142's own site
//! table already had to work around by eye — compare equal. It does not
//! attempt any richer matching (stemming, synonyms, word-subset overlap):
//! the six kinds in `src/` today share no words with the four kinds that
//! have examples, so exact-after-normalization is sufficient, and a looser
//! rule would risk false positives this file has no way to unit-test
//! itself out of. If a future kind's phrasing drifts enough to defeat this
//! (say, an about text reading "Print a complete, valid mutation-campaign
//! example for gate mutate" — extra trailing words `extract_kind_from_about`
//! does not expect), that about text fails extraction and this test fails
//! loudly asking a human to look, rather than silently comparing the wrong
//! strings.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------
// Source walk — same shape as `tests/config_malformed_coverage.rs` and
// `tests/recovery_override_text.rs`; duplicated rather than shared, per
// this trio's established convention (see `recovery_override_text.rs`'s
// own module doc for why).
// ---------------------------------------------------------------------

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

/// Every `.rs` file under `src/`, relative to the repository root, in a
/// stable order.
fn source_files() -> Vec<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut absolute = Vec::new();
    rust_files_under(&manifest_dir.join("src"), &mut absolute);
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

// ---------------------------------------------------------------------
// CLI walk: discovers every `example`/`example-*` leaf command by driving
// `--help` recursively, the way an operator would.
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

/// Every subcommand name listed under a `--help` page's `Commands:`
/// section, excluding clap's auto-generated `help`. Empty for a leaf
/// command (one with only an `Options:` section).
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

/// The first non-blank line of a `--help` page — clap's rendering of the
/// command's own `about` text.
fn about_text(help: &str) -> String {
    help.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// Recursively walks the CLI's subcommand tree from `path`, incrementing
/// `visited` once per `--help` page fetched and appending `(full path,
/// about text)` to `examples` for every leaf whose last path segment is
/// `example` or starts with `example-`.
fn walk(path: &[String], visited: &mut usize, examples: &mut Vec<(Vec<String>, String)>) {
    let help = help_text(path);
    *visited += 1;
    let children = child_command_names(&help);
    if children.is_empty() {
        if let Some(last) = path.last()
            && (last == "example" || last.starts_with("example-"))
        {
            examples.push((path.to_vec(), about_text(&help)));
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
// Extraction and comparison.
// ---------------------------------------------------------------------

const ABOUT_PREFIX: &str = "Print a complete, valid ";
const ABOUT_SUFFIXES: [&str; 2] = [" example", " document"];

/// Pulls the document-kind phrase out of an `example` command's about
/// text, matching either observed template. Returns `None` — deliberately,
/// so the caller fails loudly rather than silently skipping — when neither
/// template matches.
fn extract_kind_from_about(about: &str) -> Option<String> {
    let trimmed = about.trim().trim_end_matches('.');
    let after_prefix = trimmed.strip_prefix(ABOUT_PREFIX)?;
    for suffix in ABOUT_SUFFIXES {
        if let Some(kind) = after_prefix.strip_suffix(suffix) {
            return Some(kind.to_owned());
        }
    }
    None
}

/// Lowercases `kind` and treats `-` as a word boundary, so
/// `convergence-policy` and `convergence policy` compare equal. See this
/// file's module doc, "Normalization", for why nothing richer is used.
fn normalize(kind: &str) -> String {
    kind.to_lowercase()
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// One "no generated example" claim found in source.
#[derive(Debug)]
struct AbsentClaim {
    file: PathBuf,
    line: usize,
    /// The raw, un-normalized document-kind phrase the claim names.
    kind: String,
}

const ANCHOR_A: &str = "no generated example for a ";
const ANCHOR_AN: &str = "no generated example for an ";
const CLAIM_SUFFIX: &str = " document";

/// Every "no generated example for a(n) `<kind>` document" claim in
/// `source`, skipping comment lines and anything after the file's first
/// `#[cfg(test)]` — the same test-module exclusion
/// `tests/config_malformed_coverage.rs` and `tests/recovery_override_text.rs`
/// both use, and for the same reason: a claim inside a synthetic test
/// fixture is not operator-facing text.
fn claimed_absent_kinds(file: &Path, source: &str) -> Vec<AbsentClaim> {
    let scanned = source
        .split("#[cfg(test)]")
        .next()
        .expect("str::split always yields at least one piece");

    let mut out = Vec::new();
    for (idx, line) in scanned.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for anchor in [ANCHOR_A, ANCHOR_AN] {
            if let Some(pos) = line.find(anchor) {
                let after = &line[pos + anchor.len()..];
                if let Some(end) = after.find(CLAIM_SUFFIX) {
                    out.push(AbsentClaim {
                        file: file.to_path_buf(),
                        line: idx + 1,
                        kind: after[..end].to_owned(),
                    });
                }
            }
        }
    }
    out
}

#[test]
fn no_recovery_claims_a_missing_example_that_the_cli_now_has() {
    // 1. Walk the CLI from the root, exactly as an operator would explore it.
    let mut visited = 0usize;
    let mut examples = Vec::new();
    walk(&[], &mut visited, &mut examples);
    assert!(
        visited > 20,
        "walked only {visited} --help pages; #142 found 94 on the base this card started \
         from — the walk is very likely broken (child_command_names no longer parsing the \
         `Commands:` section), not the CLI having shrunk this much"
    );
    assert!(
        !examples.is_empty(),
        "found zero `example`/`example-*` leaf commands after walking {visited} --help pages; \
         either every example command was renamed away from that convention, or the walk is \
         broken"
    );

    // 2. Extract each one's document kind, failing loudly on an
    //    unrecognized about-text template rather than silently ignoring it.
    let mut has_example: HashSet<String> = HashSet::new();
    for (path, about) in &examples {
        let kind = extract_kind_from_about(about).unwrap_or_else(|| {
            panic!(
                "example command `{}` has an about text that matches neither known template \
                 (\"Print a complete, valid <kind> example.\" or \"...<kind> document.\"): \
                 {about:?}. Update extract_kind_from_about for the new phrasing, and separately \
                 check by hand whether any \"no generated example\" recovery claim in src/ \
                 now needs to name this command.",
                path.join(" ")
            )
        });
        has_example.insert(normalize(&kind));
    }

    // 3. Collect every absence claim in src/.
    let files = source_files();
    assert!(!files.is_empty(), "found zero .rs files under src/");
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut claims = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(manifest_dir.join(file))
            .unwrap_or_else(|e| panic!("{} should be readable: {e}", file.display()));
        claims.extend(claimed_absent_kinds(file, &source));
    }
    assert!(
        !claims.is_empty(),
        "found zero \"no generated example\" claims in src/; either every ConfigMalformed \
         document kind now has an example (update this test's premise, it is proving nothing) \
         or the anchor text `{ANCHOR_A}` / `{ANCHOR_AN}` no longer matches how the claim is \
         written"
    );

    // 4. Cross-reference: no claim may name a kind the CLI now covers.
    let stale: Vec<&AbsentClaim> = claims
        .iter()
        .filter(|claim| has_example.contains(&normalize(&claim.kind)))
        .collect();

    assert!(
        stale.is_empty(),
        "these recovery constants claim no generated example exists for their document kind, \
         but the CLI's --help tree now has one (found via the recursive walk above) — the \
         absence claim has rotted and is actively misleading an operator away from a command \
         that would help them:\n{}",
        stale
            .iter()
            .map(|c| format!(
                "  {}:{}: claims \"{}\" has no generated example",
                c.file.display(),
                c.line,
                c.kind
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn extract_kind_from_about_handles_both_observed_templates() {
    assert_eq!(
        extract_kind_from_about("Print a complete, valid convergence-policy example."),
        Some("convergence-policy".to_owned())
    );
    assert_eq!(
        extract_kind_from_about("Print a complete, valid verdict document."),
        Some("verdict".to_owned())
    );
}

#[test]
fn extract_kind_from_about_returns_none_for_an_unrecognized_template() {
    assert_eq!(
        extract_kind_from_about("Print a complete, valid gate definition example for gate mutate."),
        None,
        "trailing words after the kind must not be silently absorbed into it"
    );
    assert_eq!(extract_kind_from_about("Register a new gate."), None);
}

#[test]
fn normalize_treats_hyphens_as_spaces_and_is_case_insensitive() {
    assert_eq!(
        normalize("final-authorization-policy"),
        normalize("Final Authorization Policy")
    );
    assert_eq!(
        normalize("final-authorization-policy"),
        "final authorization policy"
    );
}

#[test]
fn claimed_absent_kinds_finds_the_anchor_and_skips_comments_and_tests() {
    let source = r#"
const REAL_RECOVERY: &str = "This document is valid JSON but does not match the schema; the message above names the missing or invalid field. There is no generated example for a mutation campaign document; re-check the field the message names.";

// A comment mentioning: There is no generated example for a decoy document, ignored.

const AN_RECOVERY: &str = "There is no generated example for an integration compatibility request document; re-check the field.";

#[cfg(test)]
mod tests {
    const FIXTURE: &str = "There is no generated example for a test-only document; ignored.";
}
"#;
    let found = claimed_absent_kinds(Path::new("synthetic.rs"), source);
    let kinds: Vec<&str> = found.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["mutation campaign", "integration compatibility request"],
        "expected the two real constants only, not the comment or the test-module fixture"
    );
}
