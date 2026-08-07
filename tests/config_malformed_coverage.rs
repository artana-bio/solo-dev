//! #142 §13.2: a coverage net for `ErrorCode::ConfigMalformed` construction
//! sites, so the next person who adds one does not silently inherit
//! `src/error.rs`'s shared fallback recovery the way every site but seven
//! did for #106's `ControlWithRecovery` mechanism (#142 §2) — the exact
//! mechanism that produced this card.
//!
//! # What "coverage" means here
//!
//! This file does not re-derive the three failure modes or judge whether a
//! site's recovery text is *good*; `tests/config_malformed_recovery.rs` and
//! `tests/recovery_override_text.rs` do that. This file only asks: does
//! every real `ErrorCode::ConfigMalformed` construction in `src/` carry a
//! per-site recovery (`HarnessError::ControlWithRecovery`), or — for the
//! four sites #142 documents as unable to — an explicit, reviewed marker
//! saying so?
//!
//! # The scan
//!
//! For every `.rs` file under `src/`, [`configmalformed_sites`] finds every
//! line containing the literal `ErrorCode::ConfigMalformed`, excluding:
//!
//! - anything after the file's first `#[cfg(test)]` (this codebase's
//!   established convention for excluding test modules from a source scan —
//!   `tests/recovery_override_text.rs` and `tests/reason_text.rs` both do
//!   the same split for the same reason: a fabricated error in a unit test
//!   is not a construction site an operator can reach)
//! - a line whose trimmed text starts with `//` (a doc comment or a plain
//!   comment *mentioning* the code, not constructing it —
//!   `disposition.rs`'s doc comment on `require_rebaseline`, which names
//!   `ErrorCode::ConfigMalformed` in an intralink rather than constructing
//!   it, is exactly this shape)
//!
//! For each remaining occurrence, [`enclosing_variant`] finds the nearest
//! preceding occurrence of `HarnessError::Control {`,
//! `HarnessError::ControlWithRecovery {`, or `HarnessError::Config {` in the
//! same file and reports which one it is — the struct literal this
//! `ErrorCode::ConfigMalformed` is a field of. This is a byte-offset
//! nearest-match, not a parser, so it assumes (true of every site in this
//! codebase today, verified by hand while writing this file) that a
//! `ConfigMalformed` construction is not separated from its own opening
//! brace by a second, unrelated `HarnessError::` struct literal — see "What
//! this cannot catch" below for what that assumption costs.
//!
//! Searching for the literal text `HarnessError::Control {` (with the
//! space) rather than just `HarnessError::Control` is deliberate: it cannot
//! match `HarnessError::ControlIo {` or `HarnessError::ControlWithRecovery {`,
//! both of which have a different character immediately after `Control`, so
//! the three-way search is unambiguous with no extra exclusion needed.
//!
//! # The rule
//!
//! - `HarnessError::ControlWithRecovery {` → covered; this site carries its
//!   own recovery.
//! - `HarnessError::Control {` or `HarnessError::Config {` → covered only if
//!   the *same source line* also contains the literal marker
//!   `#142-fallback-ok`. #142's evidence report names all four sites that
//!   carry this marker today and why each cannot be converted: two
//!   (`src/commands/gate.rs`) are not file-read or parse sites at all — a
//!   `u32` overflow guard and a defensive empty-list check, deep inside
//!   mutation execution, that do not fit #142 §3's taxonomy; two
//!   (`src/commands/project.rs`, `src/config/mod.rs`) are
//!   `HarnessError::Config` constructions, a variant with no per-site
//!   recovery mechanism today. A fifth marked site would still pass this
//!   test — the marker is a place to write *why*, reviewed at the point it
//!   is added, not a budget this file enforces a count against — but a
//!   *new*, unmarked plain `Control`/`Config` site fails it immediately,
//!   naming itself.
//! - No enclosing variant found at all → fails loudly, naming the site,
//!   rather than silently passing: this would mean the heuristic above no
//!   longer matches how sites are written, not that the site is fine.
//!
//! # What this cannot catch
//!
//! - **A `ConfigMalformed` construction that does not go through
//!   `HarnessError::Control`, `HarnessError::ControlWithRecovery`, or
//!   `HarnessError::Config` at all** — some future fourth variant, or a
//!   helper that builds one of these three but is itself more than one
//!   `HarnessError::` construction away from its `ErrorCode::ConfigMalformed`
//!   field (defeating the nearest-preceding-opener search). No site in this
//!   codebase is shaped this way today (confirmed by hand against every one
//!   of the 30 real construction sites #142 found), so this is a documented
//!   gap, not a known false negative.
//! - **A site that adds the `#142-fallback-ok` marker without a real
//!   reason**, ambitiously copy-pasted rather than earned. This file checks
//!   the marker's *presence*, not the comment above it — the same
//!   limitation `tests/recovery_override_text.rs`'s own module doc names for
//!   its `_RECOVERY` naming convention: a rule that scans for a shape can be
//!   satisfied by imitating the shape. A reviewer reading the marked site's
//!   comment is what catches that, same as today.
//! - **A `ConfigMalformed` site added to a file outside `src/`** — none
//!   exist; `error.rs`'s registry itself only ever writes `Self::ConfigMalformed`
//!   (no `ErrorCode::` prefix), so it does not appear in this scan at all,
//!   correctly, since the registry is not a construction site.

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

/// The three struct-literal openers a `ConfigMalformed` construction can
/// live inside. Order does not matter; [`enclosing_variant`] finds whichever
/// is textually nearest.
const OPENERS: [&str; 3] = [
    "HarnessError::Control {",
    "HarnessError::ControlWithRecovery {",
    "HarnessError::Config {",
];

/// One real `ErrorCode::ConfigMalformed` construction found in source.
#[derive(Debug)]
struct Site {
    file: PathBuf,
    line: usize,
    text: String,
}

/// Every real (non-test, non-comment) `ErrorCode::ConfigMalformed` mention
/// in `source`, with its 1-based line number and the line's own text.
fn configmalformed_sites(file: &Path, source: &str) -> Vec<Site> {
    let scanned = source
        .split("#[cfg(test)]")
        .next()
        .expect("str::split always yields at least one piece");

    let mut out = Vec::new();
    for (idx, line) in scanned.lines().enumerate() {
        if !line.contains("ErrorCode::ConfigMalformed") {
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
/// then at all.
///
/// The search window ends at the *end* of `site.line`, not its start: a
/// short single-line construction (`HarnessError::Control { reason: ...,
/// code: ErrorCode::ConfigMalformed }`, all on one line — every synthetic
/// test in this file uses that shape for brevity) has its own opener on the
/// same line as the `ErrorCode::ConfigMalformed` text, earlier in that
/// line's byte range but after that line's *start* offset. Ending the
/// window at the line's start would exclude it and find the wrong,
/// farther-back opener instead; found by this file's own
/// `enclosing_variant_distinguishes_all_three_shapes` test failing until
/// this was fixed. Every real multi-line site in `src/` still resolves
/// identically either way, since their own opener is always on an earlier
/// line.
fn enclosing_variant(full_source: &str, site: &Site) -> Option<&'static str> {
    // Re-find this exact line's byte offset in the untruncated source (the
    // scan above works on the pre-`#[cfg(test)]` slice, but byte offsets
    // into that slice are also valid offsets into the full source, since it
    // is a prefix).
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

const FALLBACK_OK_MARKER: &str = "#142-fallback-ok";

#[test]
fn every_configmalformed_construction_carries_its_own_recovery_or_a_reviewed_marker() {
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
        all_sites.extend(configmalformed_sites(file, &source));
        sources.insert(file.clone(), source);
    }

    // #142's own re-measurement: exactly 30 real construction sites at the
    // base this card started from (`src/config/mod.rs` 1, `commands/project.rs`
    // 5, `commands/gate.rs` 13, `commands/review.rs` 2, `commands/handoff.rs`
    // 2, `commands/audit.rs` 3, `commands/card.rs` 1, `domain/card.rs` 1,
    // `commands/disposition.rs` 2). Asserted with room to grow rather than
    // pinned exactly, so a future card adding a legitimate new site (with
    // its own recovery, covered below) does not have to edit this number —
    // only a collapse in the scan itself, which zero would signal, should
    // fail this line.
    assert!(
        all_sites.len() >= 30,
        "found {} real ErrorCode::ConfigMalformed sites, expected at least the 30 #142 \
         catalogued; the scan may no longer be matching how sites are written",
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
         ErrorCode::ConfigMalformed sites (the nearest-preceding-opener heuristic in \
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
        "these ErrorCode::ConfigMalformed sites construct a plain HarnessError::Control or \
         HarnessError::Config — inheriting src/error.rs's shared fallback recovery — without a \
         `{FALLBACK_OK_MARKER}` marker explaining why. Either give the site its own \
         `HarnessError::ControlWithRecovery` with per-site `recovery:` text (see \
         `tests/config_malformed_recovery.rs` for the three-mode pattern), or, if it truly \
         cannot carry one, add a reviewed comment ending in the literal marker \
         `{FALLBACK_OK_MARKER}` on the same line, the way #142's four documented exceptions do:\n{}",
        uncovered
            .iter()
            .map(|s| format!("  {}:{}: {}", s.file.display(), s.line, s.text.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// [`configmalformed_sites`] finds a real construction and skips a comment
/// mentioning the code, on synthetic text mirroring what
/// `disposition.rs:756`'s doc comment and a real construction both look
/// like.
#[test]
fn configmalformed_sites_skips_comments_but_keeps_constructions() {
    let source = r"
/// Returns [`ErrorCode::ConfigMalformed`] when the new policy cannot be read.
fn read_new_policy() {
    let raw = fs::read_to_string(path).map_err(|source| HarnessError::Control {
        reason: cannot_read_message(source),
        code: ErrorCode::ConfigMalformed,
    })?;
}
";
    let sites = configmalformed_sites(Path::new("synthetic.rs"), source);
    assert_eq!(
        sites.len(),
        1,
        "expected exactly the construction, not the doc comment: {sites:?}"
    );
    assert!(sites[0].text.contains("code: ErrorCode::ConfigMalformed"));
}

/// [`configmalformed_sites`] excludes everything after the file's first
/// `#[cfg(test)]`, mirroring `tests/recovery_override_text.rs`'s identical
/// split.
#[test]
fn configmalformed_sites_excludes_test_modules() {
    let source = r"
fn real_site() -> Result<(), HarnessError> {
    Err(HarnessError::Control { reason: String::new(), code: ErrorCode::ConfigMalformed })
}

#[cfg(test)]
mod tests {
    #[test]
    fn fabricated() {
        let error = HarnessError::Control { reason: String::new(), code: ErrorCode::ConfigMalformed };
        assert_eq!(error.code(), ErrorCode::ConfigMalformed);
    }
}
";
    let sites = configmalformed_sites(Path::new("synthetic.rs"), source);
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
    HarnessError::Control { reason: String::new(), code: ErrorCode::ConfigMalformed }
}
fn b() {
    HarnessError::ControlWithRecovery { reason: String::new(), code: ErrorCode::ConfigMalformed, recovery: "x" }
}
fn c() {
    HarnessError::Config { field: String::new(), reason: String::new(), code: ErrorCode::ConfigMalformed }
}
"#;
    let sites = configmalformed_sites(Path::new("synthetic.rs"), source);
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
