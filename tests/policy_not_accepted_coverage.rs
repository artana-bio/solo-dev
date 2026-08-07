//! #179: a coverage net for `ErrorCode::PolicyNotAccepted` construction
//! sites, mirroring `tests/config_malformed_coverage.rs` (#142 §13.2) for a
//! different code — so the next person who adds a `PolicyNotAccepted` site
//! does not silently inherit `src/error.rs`'s shared fallback recovery the
//! way 29 of the 29 real sites (and, before #106, every site of every code)
//! once did.
//!
//! # Extending `config_malformed_coverage.rs` versus a sibling file
//!
//! #179 §9 asks this to be established and argued. This file is a sibling,
//! not an extension, for the same reason `tests/recovery_override_text.rs`
//! gives for being a third sibling to `tests/reason_text.rs` and
//! `tests/recovery_text.rs` rather than folding into either: extending
//! `config_malformed_coverage.rs` would mean either parameterizing its
//! single-purpose constants and module doc (`FALLBACK_OK_MARKER`, the
//! `OPENERS` list, the "30 sites #142 catalogued" narration, all written to
//! tell one code's own story) over a second code, muddying a file that is
//! currently a clean, self-contained account of one incident, or bolting a
//! second near-identical scan onto the same file behind the same doc
//! comment — the exact "generalizing it into something that serves
//! neither" outcome `recovery_override_text.rs`'s own module doc warns
//! against. A sibling file keeps `config_malformed_coverage.rs` byte-for-
//! byte untouched (confirmed: this card's diff does not touch it) and
//! gives `PolicyNotAccepted` its own scan, shaped for it. The scan
//! mechanics below — `rust_files_under`, `source_files`, the `Site`
//! struct, `enclosing_variant`'s nearest-preceding-opener search — are
//! copied with only the target code and marker text changed, the same
//! "duplication, not a shared module" choice `recovery_override_text.rs`
//! makes explicit for its own trio of sibling files.
//!
//! # What "coverage" means here
//!
//! Same contract as `config_malformed_coverage.rs`: this file does not
//! re-derive the five situations #179's evidence report groups sites into,
//! or judge whether a site's recovery text is *good* — `tests/
//! policy_not_accepted_recovery.rs` and `tests/recovery_override_text.rs`
//! do that. This file only asks: does every real
//! `ErrorCode::PolicyNotAccepted` construction in `src/` carry a per-site
//! recovery (`HarnessError::ControlWithRecovery`), or — for a site that
//! genuinely cannot — an explicit, reviewed marker saying so?
//!
//! # The scan
//!
//! Identical exclusions to `config_malformed_coverage.rs`: anything after
//! the file's first `#[cfg(test)]`, and a line whose trimmed text starts
//! with `//` (a doc comment or plain comment *mentioning* the code, not
//! constructing it — `disposition.rs` alone has six `///` intralinks to
//! `` [`ErrorCode::PolicyNotAccepted`] `` on its six `require_*` functions,
//! plus one plain `//` comment cross-referencing the code in
//! `require_renewable`'s check 4; none of the seven is a construction).
//!
//! # The rule
//!
//! - `HarnessError::ControlWithRecovery {` → covered; this site carries
//!   its own recovery.
//! - `HarnessError::Control {` or `HarnessError::Config {` → covered only
//!   if the same source line also contains the literal marker
//!   `#179-fallback-ok`. #179's evidence report found no site in this
//!   codebase today that needs this marker — every real
//!   `PolicyNotAccepted` construction turned out to be a reachable
//!   `HarnessError::Control` convertible to `ControlWithRecovery` — so no
//!   test here exercises the marker against a real, present site the way
//!   `config_malformed_coverage.rs` could point at #142's four. The
//!   synthetic tests below (copied from that file' own synthetic tests)
//!   still prove the mechanism itself works: a marked plain `Control` or
//!   `Config` site passes, an unmarked one fails naming itself. A fifth
//!   site earning the marker in the future would still pass this test —
//!   the marker is a place to write *why*, reviewed at the point it is
//!   added, not a budget this file enforces a count against.
//! - No enclosing variant found at all → fails loudly, naming the site.
//!
//! # What this cannot catch
//!
//! Identical, code-for-code, to what `config_malformed_coverage.rs`'s own
//! module doc names for its scan: a `PolicyNotAccepted` construction that
//! does not go through `HarnessError::Control`, `ControlWithRecovery`, or
//! `Config` at all; the `#179-fallback-ok` marker added without a real
//! reason (this file checks the marker's presence, not the comment above
//! it — a human reviewer catches that, same as today); and a
//! `PolicyNotAccepted` site added to a file outside `src/`.

use std::path::{Path, PathBuf};

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

/// The three struct-literal openers a `PolicyNotAccepted` construction can
/// live inside. Order does not matter; [`enclosing_variant`] finds whichever
/// is textually nearest.
const OPENERS: [&str; 3] = [
    "HarnessError::Control {",
    "HarnessError::ControlWithRecovery {",
    "HarnessError::Config {",
];

/// One real `ErrorCode::PolicyNotAccepted` construction found in source.
#[derive(Debug)]
struct Site {
    file: PathBuf,
    line: usize,
    text: String,
}

/// Every real (non-test, non-comment) `ErrorCode::PolicyNotAccepted`
/// mention in `source`, with its 1-based line number and the line's own
/// text.
fn policynotaccepted_sites(file: &Path, source: &str) -> Vec<Site> {
    let scanned = source
        .split("#[cfg(test)]")
        .next()
        .expect("str::split always yields at least one piece");

    let mut out = Vec::new();
    for (idx, line) in scanned.lines().enumerate() {
        if !line.contains("ErrorCode::PolicyNotAccepted") {
            continue;
        }
        if line.trim_start().starts_with("//") {
            continue;
        }
        out.push(Site {
            file: file.to_path_buf(),
            line: idx + 1,
            text: line.to_string(),
        });
    }
    out
}

/// The struct-literal opener nearest to (and no later than) `site`'s own
/// line within `full_source`, or `None` if none of [`OPENERS`] appears by
/// then at all. See `config_malformed_coverage.rs`'s identical function for
/// why the search window ends at the *end* of `site.line`, not its start.
fn enclosing_variant(full_source: &str, site: &Site) -> Option<&'static str> {
    let start_of_line: usize = full_source
        .lines()
        .take(site.line - 1)
        .map(|l| l.len() + 1)
        .sum();
    let window_end = start_of_line + site.text.len();

    let mut best: Option<(&'static str, usize)> = None;
    for opener in OPENERS {
        if let Some(pos) = full_source[..window_end].rfind(opener)
            && best.is_none_or(|(_, best_pos)| pos > best_pos)
        {
            best = Some((opener, pos));
        }
    }
    best.map(|(opener, _)| opener)
}

const FALLBACK_OK_MARKER: &str = "#179-fallback-ok";

#[test]
fn every_policynotaccepted_construction_carries_its_own_recovery_or_a_reviewed_marker() {
    let files = source_files();
    assert!(
        !files.is_empty(),
        "found zero .rs files under src/; the walk in `source_files` no longer matches the \
         repository layout"
    );

    let mut all_sites = Vec::new();
    let mut sources = std::collections::HashMap::new();
    for file in &files {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let absolute = manifest_dir.join(file);
        let source = std::fs::read_to_string(&absolute)
            .unwrap_or_else(|e| panic!("{} should be readable: {e}", absolute.display()));
        all_sites.extend(policynotaccepted_sites(file, &source));
        sources.insert(file.clone(), source);
    }

    // #179's own re-measurement: exactly 29 real construction sites at the
    // base this card started from (`src/commands/acceptance.rs` 8,
    // `src/commands/disposition.rs` 12, `src/commands/integration.rs` 9) —
    // not the 36 the card's own §2 counted, which turned out to be raw
    // grep hits for the literal text `ErrorCode::PolicyNotAccepted`
    // (including six `///` intralinks and one `//` comment in
    // `disposition.rs` alone that mention the code without constructing
    // it). Asserted with room to grow rather than pinned exactly, so a
    // future card adding a legitimate new site (with its own recovery,
    // covered below) does not have to edit this number — only a collapse
    // in the scan itself, which zero would signal, should fail this line.
    assert!(
        all_sites.len() >= 29,
        "found {} real ErrorCode::PolicyNotAccepted sites, expected at least the 29 #179 \
         re-measured; the scan may no longer be matching how sites are written",
        all_sites.len()
    );

    let mut uncovered = Vec::new();
    let mut unrecognized = Vec::new();
    for site in &all_sites {
        let source = &sources[&site.file];
        match enclosing_variant(source, site) {
            Some("HarnessError::ControlWithRecovery {") => {}
            Some("HarnessError::Control {" | "HarnessError::Config {") => {
                if !site.text.contains(FALLBACK_OK_MARKER) {
                    uncovered.push(site);
                }
            }
            _ => unrecognized.push(site),
        }
    }

    assert!(
        unrecognized.is_empty(),
        "could not determine the enclosing HarnessError variant for these \
         ErrorCode::PolicyNotAccepted sites (the nearest-preceding-opener heuristic in \
         `enclosing_variant` no longer matches how they are written — see this file's own \
         module doc, \"What this cannot catch\"):\n{}",
        unrecognized
            .iter()
            .map(|s| format!("  {}:{}: {}", s.file.display(), s.line, s.text.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );

    assert!(
        uncovered.is_empty(),
        "these ErrorCode::PolicyNotAccepted sites construct a plain HarnessError::Control or \
         HarnessError::Config — inheriting src/error.rs's shared fallback recovery — without a \
         `{FALLBACK_OK_MARKER}` marker explaining why. Either give the site its own \
         `HarnessError::ControlWithRecovery` with per-site `recovery:` text (see \
         `src/commands/acceptance.rs`'s five `*_RECOVERY` constants for the situations already \
         established, and reuse one if it fits), or, if it truly cannot carry one, add a \
         reviewed comment ending in the literal marker `{FALLBACK_OK_MARKER}` on the same line, \
         the way #142's four documented `ConfigMalformed` exceptions do for their own code:\n{}",
        uncovered
            .iter()
            .map(|s| format!("  {}:{}: {}", s.file.display(), s.line, s.text.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// [`policynotaccepted_sites`] finds a real construction and skips a
/// comment mentioning the code — synthetic text mirroring
/// `disposition.rs`'s own `///` intralinks (six of them, one per
/// `require_*` function) and `require_renewable`'s plain `//` comment,
/// none of which construct anything.
#[test]
fn policynotaccepted_sites_skips_comments_but_keeps_constructions() {
    let source = r"
/// Returns [`ErrorCode::PolicyNotAccepted`] when no final-authorization
/// policy is configured, or when the acting actor is not in its
/// authorized set.
fn require_renewable() {
    // a missing policy and an actor absent from its configured set both
    // refuse with `ErrorCode::PolicyNotAccepted`, and membership is
    // decided by `FinalAuthorizationPolicy::authorizes`.
    let authorization = config.final_authorization_policy.as_ref().ok_or_else(|| HarnessError::Control {
        reason: cannot_authorize_message(),
        code: ErrorCode::PolicyNotAccepted,
    })?;
}
";
    let sites = policynotaccepted_sites(Path::new("synthetic.rs"), source);
    assert_eq!(
        sites.len(),
        1,
        "expected exactly the construction, not the three doc-comment lines or the plain \
         comment: {sites:?}"
    );
    assert!(sites[0].text.contains("code: ErrorCode::PolicyNotAccepted"));
}

/// [`policynotaccepted_sites`] excludes everything after the file's first
/// `#[cfg(test)]`, mirroring `config_malformed_coverage.rs`'s identical
/// split.
#[test]
fn policynotaccepted_sites_excludes_test_modules() {
    let source = r"
fn real_site() -> Result<(), HarnessError> {
    Err(HarnessError::Control { reason: String::new(), code: ErrorCode::PolicyNotAccepted })
}

#[cfg(test)]
mod tests {
    #[test]
    fn fabricated() {
        let error = HarnessError::Control { reason: String::new(), code: ErrorCode::PolicyNotAccepted };
        assert_eq!(error.code(), ErrorCode::PolicyNotAccepted);
    }
}
";
    let sites = policynotaccepted_sites(Path::new("synthetic.rs"), source);
    assert_eq!(
        sites.len(),
        1,
        "expected only the real site, none of the two test-module mentions: {sites:?}"
    );
}

/// [`enclosing_variant`] tells a plain `Control` site from a
/// `ControlWithRecovery` site from a `Config` site, on synthetic text
/// shaped like three real sites side by side.
#[test]
fn enclosing_variant_distinguishes_all_three_shapes() {
    let source = r#"
fn a() {
    HarnessError::Control { reason: String::new(), code: ErrorCode::PolicyNotAccepted }
}
fn b() {
    HarnessError::ControlWithRecovery { reason: String::new(), code: ErrorCode::PolicyNotAccepted, recovery: "x" }
}
fn c() {
    HarnessError::Config { field: String::new(), reason: String::new(), code: ErrorCode::PolicyNotAccepted }
}
"#;
    let sites = policynotaccepted_sites(Path::new("synthetic.rs"), source);
    assert_eq!(sites.len(), 3, "{sites:?}");
    let variants: Vec<Option<&str>> = sites
        .iter()
        .map(|site| enclosing_variant(source, site))
        .collect();
    assert_eq!(
        variants,
        vec![
            Some("HarnessError::Control {"),
            Some("HarnessError::ControlWithRecovery {"),
            Some("HarnessError::Config {"),
        ]
    );
}

/// End-to-end proof of the marker mechanism itself, since #179 found no
/// real site needing it (unlike #142, which could point this same test at
/// four real, present sites). A marked plain `Control` site passes; an
/// unmarked one fails, naming itself.
#[test]
fn a_marked_plain_control_site_passes_but_an_unmarked_one_fails() {
    let marked = r"
fn a() -> Result<(), HarnessError> {
    Err(HarnessError::Control { reason: String::new(), code: ErrorCode::PolicyNotAccepted }) // #179-fallback-ok: synthetic proof of the marker mechanism
}
";
    let unmarked = r"
fn a() -> Result<(), HarnessError> {
    Err(HarnessError::Control { reason: String::new(), code: ErrorCode::PolicyNotAccepted })
}
";

    let marked_sites = policynotaccepted_sites(Path::new("synthetic.rs"), marked);
    assert_eq!(marked_sites.len(), 1, "{marked_sites:?}");
    assert!(marked_sites[0].text.contains(FALLBACK_OK_MARKER));

    let unmarked_sites = policynotaccepted_sites(Path::new("synthetic.rs"), unmarked);
    assert_eq!(unmarked_sites.len(), 1, "{unmarked_sites:?}");
    assert!(!unmarked_sites[0].text.contains(FALLBACK_OK_MARKER));
}
