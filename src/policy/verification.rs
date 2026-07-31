//! Candidate verification: does this exact commit stay inside its card?
//!
//! Section 13.3 requires verification to compare Git objects, not the feature
//! worktree's cached card. That is the whole point: the actor can edit anything
//! inside their worktree, including a copy of the card, so a verifier that read
//! the worktree's copy would be asking the accused to describe the crime.
//!
//! Everything here is a pure function over already-collected facts, so the same
//! candidate verifies identically from any clean clone.

use serde::{Deserialize, Serialize};

use crate::{
    domain::card::CardRecord,
    git::diff::{ChangeKind, ChangedPath, DiffSummary},
};

use super::paths::{CaseSensitivity, Scope};

/// Schema identifier for a verification report.
pub const VERIFICATION_SCHEMA: &str = "harness.verification/v1";

/// Paths no card may write, whatever its declared scope.
///
/// These are the harness's own footprint inside a candidate worktree. A card
/// that could rewrite them could rewrite the record of what it is allowed to
/// do.
const PROTECTED_PREFIXES: [&str; 2] = [".agent/", ".git/"];

/// Files that change what the build resolves, so a silent edit is a supply
/// chain change rather than a code change.
const DEPENDENCY_MANIFESTS: [&str; 10] = [
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.mod",
    "go.sum",
    "requirements.txt",
    "pyproject.toml",
];

/// True when a repository-relative path names a recognized dependency manifest.
///
/// Monorepos commonly keep manifests below the repository root. Matching the
/// final path segment preserves the explicit supported set while applying the
/// same declaration rule at every depth.
fn is_dependency_manifest(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|name| DEPENDENCY_MANIFESTS.contains(&name))
}

/// How serious a finding is.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The candidate may not hand off.
    Blocking,
    /// Recorded for the reviewer, but not itself disqualifying.
    Advisory,
}

/// One thing verification noticed.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct Finding {
    /// Stable machine-readable kind, such as `out-of-scope`.
    pub kind: String,
    /// Whether it blocks handoff.
    pub severity: Severity,
    /// The path it concerns, when it has one.
    pub path: Option<String>,
    /// What is wrong, in one sentence.
    pub detail: String,
}

impl Finding {
    fn blocking(kind: &str, path: Option<String>, detail: String) -> Self {
        Self {
            kind: kind.to_owned(),
            severity: Severity::Blocking,
            path,
            detail,
        }
    }

    fn advisory(kind: &str, path: Option<String>, detail: String) -> Self {
        Self {
            kind: kind.to_owned(),
            severity: Severity::Advisory,
            path,
            detail,
        }
    }
}

/// Facts about a candidate, gathered from Git before any policy runs.
#[derive(Clone, Debug)]
pub struct CandidateFacts {
    /// The commit the card declares as its base.
    pub declared_base: String,
    /// The commit the branch actually starts from, per merge-base.
    pub actual_base: String,
    /// The exact candidate commit.
    pub candidate_sha: String,
    /// Every change between base and candidate.
    pub diff: DiffSummary,
    /// Subjects of the commits the candidate introduces.
    pub commit_subjects: Vec<String>,
    /// Whether the worktree had uncommitted or untracked content.
    pub worktree_clean: bool,
    /// Paths reported as dirty, when it was not clean.
    pub dirty_paths: Vec<String>,
}

/// The structured result of verifying one candidate.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VerificationReport {
    /// Always [`VERIFICATION_SCHEMA`].
    pub schema: String,
    /// The card verified against.
    pub card_id: String,
    /// The card revision in force.
    pub card_revision: u32,
    /// The card digest the verdict is bound to.
    pub card_digest: String,
    /// The base the card declares.
    pub base_sha: String,
    /// The exact candidate commit.
    pub candidate_sha: String,
    /// Every path the candidate touched, including rename sources.
    pub changed_paths: Vec<ChangedPath>,
    /// Everything verification noticed.
    pub findings: Vec<Finding>,
    /// True when nothing blocking was found.
    pub passed: bool,
}

impl VerificationReport {
    /// Findings that prevent handoff.
    #[must_use]
    pub fn blocking(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Blocking)
            .collect()
    }
}

/// Verifies a candidate against its authoritative card.
///
/// The card comes from control state, never from the worktree.
#[must_use]
pub fn verify(card: &CardRecord, card_digest: &str, facts: &CandidateFacts) -> VerificationReport {
    let mut findings = Vec::new();
    let case = CaseSensitivity::host();
    let scope = Scope::new(&card.write_scope.include, &card.write_scope.exclude);

    if facts.actual_base != facts.declared_base {
        findings.push(Finding::blocking(
            "base-ancestry-mismatch",
            None,
            format!(
                "the candidate branches from {} but the card declares base {}",
                facts.actual_base, facts.declared_base
            ),
        ));
    }

    if !facts.worktree_clean {
        findings.push(Finding::blocking(
            "dirty-worktree",
            None,
            format!(
                "the worktree has uncommitted or untracked content: {}",
                facts.dirty_paths.join(", ")
            ),
        ));
    }

    for change in &facts.diff.paths {
        findings.extend(check_change(change, &scope, case, card));
    }

    if facts.diff.touches_gitmodules() {
        findings.push(Finding::blocking(
            "gitmodules-change",
            Some(".gitmodules".to_owned()),
            "changing submodule configuration alters what the build resolves and is never in scope for an ordinary card".to_owned(),
        ));
    }

    for subject in &facts.commit_subjects {
        if subject.trim().is_empty() {
            findings.push(Finding::blocking(
                "empty-commit-message",
                None,
                "a commit with no subject cannot be reviewed or attributed".to_owned(),
            ));
        }
    }

    let passed = findings
        .iter()
        .all(|finding| finding.severity != Severity::Blocking);

    VerificationReport {
        schema: VERIFICATION_SCHEMA.to_owned(),
        card_id: card.card_id.to_string(),
        card_revision: card.revision,
        card_digest: card_digest.to_owned(),
        base_sha: facts.declared_base.clone(),
        candidate_sha: facts.candidate_sha.clone(),
        changed_paths: facts.diff.paths.clone(),
        findings,
        passed,
    }
}

/// Checks one changed path against the card.
fn check_change(
    change: &ChangedPath,
    scope: &Scope<'_>,
    case: CaseSensitivity,
    card: &CardRecord,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    // Both sides of a rename are checked. A rename that moves a file out of one
    // card's territory into another's is two ownership events, not one, and
    // checking only the destination would miss the half that removes code.
    for path in change.touched_paths() {
        findings.extend(check_path(path, scope, case, card));
    }
    findings.extend(check_entry_kind(change));
    findings
}

/// Checks one path, on either side of a rename, against the card's claims.
fn check_path(
    path: &str,
    scope: &Scope<'_>,
    case: CaseSensitivity,
    card: &CardRecord,
) -> Vec<Finding> {
    if PROTECTED_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
    {
        return vec![Finding::blocking(
            "protected-path",
            Some(path.to_owned()),
            format!("`{path}` belongs to the harness and is never writable by a card"),
        )];
    }

    let mut findings = Vec::new();

    if !scope.allows(path) {
        let excluded = card
            .write_scope
            .exclude
            .iter()
            .any(|pattern| super::paths::matches(pattern, path, case));
        findings.push(Finding::blocking(
            if excluded {
                "excluded-path"
            } else {
                "out-of-scope"
            },
            Some(path.to_owned()),
            if excluded {
                format!("`{path}` is explicitly excluded by the card's write scope")
            } else {
                format!("`{path}` is outside the card's write scope")
            },
        ));
    }

    if is_dependency_manifest(path)
        && !card
            .write_scope
            .include
            .iter()
            .any(|pattern| pattern == path)
    {
        findings.push(Finding::blocking(
            "undeclared-dependency-manifest",
            Some(path.to_owned()),
            format!(
                "`{path}` changes what the build resolves and must be named explicitly in the card's write scope"
            ),
        ));
    }

    // Whether a generated path may be committed is a matter of class, not of
    // taste. Transient output belongs to nobody and shared output belongs to
    // integration, so a card committing either is claiming something it does
    // not own — which is the whole reason the classes exist.
    if let Some(artifact) = card
        .generated_artifacts
        .iter()
        .find(|artifact| super::paths::matches(&artifact.path, path, case))
    {
        if artifact.class.card_may_commit() {
            findings.push(Finding::advisory(
                "generated-artifact",
                Some(path.to_owned()),
                format!(
                    "`{path}` is a declared `{}` artifact",
                    artifact.class.name()
                ),
            ));
        } else {
            findings.push(Finding::blocking(
                "generated-artifact-not-owned",
                Some(path.to_owned()),
                match artifact.class {
                    crate::domain::artifact::ArtifactClass::Transient => format!(
                        "`{path}` is declared transient and must never be committed"
                    ),
                    _ => format!(
                        "`{path}` is declared shared; integration generates it, so no card may commit it"
                    ),
                },
            ));
        }
    }

    findings
}

/// Checks what kind of entry a change produced, independently of its path.
fn check_entry_kind(change: &ChangedPath) -> Vec<Finding> {
    let mut findings = Vec::new();

    if change.involves_symlink() {
        findings.push(Finding::blocking(
            "symlink-change",
            Some(change.path.clone()),
            format!(
                "`{}` adds or changes a symbolic link, which can redirect reads outside the repository",
                change.path
            ),
        ));
    }

    if change.involves_submodule() {
        findings.push(Finding::blocking(
            "submodule-change",
            Some(change.path.clone()),
            format!(
                "`{}` adds or changes a submodule entry, which imports code the card did not declare",
                change.path
            ),
        ));
    }

    if change.changes_executable_bit() {
        findings.push(Finding::advisory(
            "executable-bit-change",
            Some(change.path.clone()),
            format!(
                "`{}` changes the executable bit from {} to {}",
                change.path, change.old_mode, change.new_mode
            ),
        ));
    } else if change.changes_mode() {
        findings.push(Finding::advisory(
            "mode-change",
            Some(change.path.clone()),
            format!(
                "`{}` changes file mode from {} to {}",
                change.path, change.old_mode, change.new_mode
            ),
        ));
    }

    if change.kind == ChangeKind::Renamed
        && let Some(previous) = &change.previous_path
    {
        // Reported even when both sides are in scope, because a reviewer
        // reading a diff can otherwise miss that a file moved rather than being
        // rewritten.
        findings.push(Finding::advisory(
            "rename",
            Some(change.path.clone()),
            format!("`{previous}` was renamed to `{}`", change.path),
        ));
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::artifact::{ArtifactClass, GeneratedArtifact};
    use crate::domain::{
        card::{Acceptance, CardDraft, CardRecord, NamedGates, Risk, WriteScope},
        clock::{Clock as _, FixedClock},
    };

    fn card_with(
        include: &[&str],
        exclude: &[&str],
        generated: &[GeneratedArtifact],
    ) -> CardRecord {
        let draft = CardDraft {
            card_id: "F-001".parse().unwrap(),
            cycle_id: "C-001".parse().unwrap(),
            title: "T".to_owned(),
            goal: "G".to_owned(),
            non_goals: vec![],
            risk: Risk::Low,
            change_kind: "feature".to_owned(),
            base_sha: "a".repeat(40),
            write_scope: WriteScope {
                include: include.iter().map(|s| (*s).to_owned()).collect(),
                exclude: exclude.iter().map(|s| (*s).to_owned()).collect(),
            },
            contract_reads: vec![],
            contract_changes: vec![],
            depends_on: vec![],
            exclusive_resources: vec![],
            named_gates: NamedGates {
                feature: vec!["gate.unit".to_owned()],
                review: vec![],
                integration: vec![],
            },
            acceptance: Acceptance {
                behaviors: vec!["works".to_owned()],
                regressions: vec![],
            },
            generated_artifacts: generated.to_vec(),
            review_policy: "independent".to_owned(),
            rollback_strategy: "revert".to_owned(),
        };
        let stamp = FixedClock::at_unix_seconds(1_785_196_800).unwrap().now();
        CardRecord::activate(&draft, 1, "alvaro", stamp).unwrap()
    }

    /// A declaration of the given class, with whatever that class requires.
    fn artifact(path: &str, class: ArtifactClass) -> GeneratedArtifact {
        GeneratedArtifact {
            path: path.to_owned(),
            class,
            generator: matches!(class, ArtifactClass::PerCard | ArtifactClass::Shared)
                .then(|| "gate.generate".to_owned()),
            sources: if class == ArtifactClass::PerCard {
                vec!["src/**".to_owned()]
            } else {
                Vec::new()
            },
            identifier: (class == ArtifactClass::Serialized).then(|| "0007".to_owned()),
        }
    }

    fn facts(raw_diff: &str) -> CandidateFacts {
        CandidateFacts {
            declared_base: "a".repeat(40),
            actual_base: "a".repeat(40),
            candidate_sha: "b".repeat(40),
            diff: crate::git::diff::parse_raw_z(raw_diff).unwrap(),
            commit_subjects: vec!["feat: do the thing".to_owned()],
            worktree_clean: true,
            dirty_paths: vec![],
        }
    }

    fn kinds(report: &VerificationReport) -> Vec<&str> {
        report
            .findings
            .iter()
            .map(|finding| finding.kind.as_str())
            .collect()
    }

    #[test]
    fn an_in_scope_change_passes() {
        let card = card_with(&["src/**"], &[], &[]);
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100644 a b M\0src/a.rs\0"),
        );
        assert!(report.passed, "{:?}", report.findings);
        assert!(report.blocking().is_empty());
    }

    #[test]
    fn scenario_13_an_out_of_scope_modification_blocks() {
        let card = card_with(&["src/**"], &[], &[]);
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100644 a b M\0docs/readme.md\0"),
        );
        assert!(!report.passed);
        assert!(kinds(&report).contains(&"out-of-scope"));
    }

    #[test]
    fn scenario_14_an_excluded_path_blocks_and_is_named_distinctly() {
        let card = card_with(&["src/**"], &["src/generated/**"], &[]);
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100644 a b M\0src/generated/api.rs\0"),
        );
        assert!(!report.passed);
        assert!(
            kinds(&report).contains(&"excluded-path"),
            "an exclusion is a different operator fix than an out-of-scope path: {:?}",
            kinds(&report)
        );
    }

    #[test]
    fn scenario_15_a_rename_across_an_ownership_boundary_blocks() {
        let card = card_with(&["src/**"], &[], &[]);
        // The destination is in scope; the source is not.
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100644 a b R100\0docs/old.md\0src/new.rs\0"),
        );
        assert!(
            !report.passed,
            "a rename must be checked on both sides: {:?}",
            report.findings
        );
        let out_of_scope: Vec<_> = report
            .findings
            .iter()
            .filter(|finding| finding.kind == "out-of-scope")
            .collect();
        assert_eq!(out_of_scope.len(), 1);
        assert_eq!(out_of_scope[0].path.as_deref(), Some("docs/old.md"));
    }

    #[test]
    fn scenario_16_a_deletion_outside_scope_blocks() {
        let card = card_with(&["src/**"], &[], &[]);
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 000000 a 0 D\0docs/gone.md\0"),
        );
        assert!(!report.passed);
        assert!(kinds(&report).contains(&"out-of-scope"));
    }

    #[test]
    fn scenario_17_an_executable_bit_change_is_reported() {
        let card = card_with(&["src/**"], &[], &[]);
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100755 a b M\0src/run.sh\0"),
        );
        assert!(kinds(&report).contains(&"executable-bit-change"));
        assert!(
            report.passed,
            "an in-scope mode change is advisory, not blocking"
        );
    }

    #[test]
    fn scenario_18_a_symlink_addition_blocks() {
        let card = card_with(&["src/**"], &[], &[]);
        let report = verify(
            &card,
            "sha256:x",
            &facts(":000000 120000 0 b A\0src/link\0"),
        );
        assert!(!report.passed);
        assert!(kinds(&report).contains(&"symlink-change"));
    }

    #[test]
    fn scenario_19_a_gitmodules_change_blocks() {
        let card = card_with(&["**"], &[], &[]);
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100644 a b M\0.gitmodules\0"),
        );
        assert!(!report.passed);
        assert!(kinds(&report).contains(&"gitmodules-change"));
    }

    #[test]
    fn a_submodule_entry_blocks() {
        let card = card_with(&["**"], &[], &[]);
        let report = verify(
            &card,
            "sha256:x",
            &facts(":000000 160000 0 b A\0vendor/dep\0"),
        );
        assert!(!report.passed);
        assert!(kinds(&report).contains(&"submodule-change"));
    }

    #[test]
    fn a_protected_path_blocks_even_when_the_scope_would_allow_it() {
        let card = card_with(&["**"], &[], &[]);
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100644 a b M\0.agent/project.json\0"),
        );
        assert!(!report.passed);
        assert!(kinds(&report).contains(&"protected-path"));
    }

    #[test]
    fn an_undeclared_dependency_manifest_blocks() {
        // Keep this expectation independent from DEPENDENCY_MANIFESTS. If the
        // production list loses an ecosystem, iterating the production list
        // here would silently shrink the test with it.
        let expected = [
            "Cargo.toml",
            "Cargo.lock",
            "package.json",
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
            "go.mod",
            "go.sum",
            "requirements.txt",
            "pyproject.toml",
        ];
        let card = card_with(&["**"], &[], &[]);

        for manifest in expected {
            for path in [manifest.to_owned(), format!("nested/{manifest}")] {
                let raw_diff = format!(":100644 100644 a b M\0{path}\0");
                let report = verify(&card, "sha256:x", &facts(&raw_diff));
                let blocking: Vec<_> = report
                    .blocking()
                    .iter()
                    .map(|finding| (finding.kind.as_str(), finding.path.as_deref()))
                    .collect();
                assert_eq!(
                    blocking,
                    vec![("undeclared-dependency-manifest", Some(path.as_str()))],
                    "`**` admits {path}, so only the explicit dependency-manifest rule may block it"
                );
            }
        }
    }

    #[test]
    fn an_explicitly_declared_manifest_is_allowed() {
        for path in ["Cargo.lock", "nested/Cargo.lock"] {
            let card = card_with(&[path], &[], &[]);
            let raw_diff = format!(":100644 100644 a b M\0{path}\0");
            let report = verify(&card, "sha256:x", &facts(&raw_diff));
            assert!(
                report.passed,
                "naming {path} explicitly is how a card takes responsibility for it: {:?}",
                report.findings
            );
        }
    }

    #[test]
    fn a_base_ancestry_mismatch_blocks() {
        let card = card_with(&["src/**"], &[], &[]);
        let mut mismatched = facts(":100644 100644 a b M\0src/a.rs\0");
        mismatched.actual_base = "c".repeat(40);
        let report = verify(&card, "sha256:x", &mismatched);
        assert!(!report.passed);
        assert!(kinds(&report).contains(&"base-ancestry-mismatch"));
    }

    #[test]
    fn a_dirty_worktree_blocks_and_names_the_paths() {
        let card = card_with(&["src/**"], &[], &[]);
        let mut dirty = facts(":100644 100644 a b M\0src/a.rs\0");
        dirty.worktree_clean = false;
        dirty.dirty_paths = vec!["src/scratch.rs".to_owned()];
        let report = verify(&card, "sha256:x", &dirty);
        assert!(!report.passed);
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.kind == "dirty-worktree")
            .unwrap();
        assert!(finding.detail.contains("src/scratch.rs"));
    }

    #[test]
    fn an_empty_commit_subject_blocks() {
        let card = card_with(&["src/**"], &[], &[]);
        let mut blank = facts(":100644 100644 a b M\0src/a.rs\0");
        blank.commit_subjects = vec!["   ".to_owned()];
        let report = verify(&card, "sha256:x", &blank);
        assert!(!report.passed);
        assert!(kinds(&report).contains(&"empty-commit-message"));
    }

    #[test]
    fn committing_an_artifact_the_card_owns_is_advisory() {
        let card = card_with(
            &["src/**"],
            &[],
            &[artifact("src/generated.rs", ArtifactClass::PerCard)],
        );
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100644 a b M\0src/generated.rs\0"),
        );
        assert!(report.passed, "a card may commit what it generates");
        assert!(kinds(&report).contains(&"generated-artifact"));
    }

    #[test]
    fn committing_a_transient_artifact_blocks() {
        let card = card_with(
            &["src/**"],
            &[],
            &[artifact("src/generated.rs", ArtifactClass::Transient)],
        );
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100644 a b M\0src/generated.rs\0"),
        );
        assert!(!report.passed, "transient output belongs to nobody");
        assert!(kinds(&report).contains(&"generated-artifact-not-owned"));
    }

    #[test]
    fn committing_a_shared_artifact_blocks() {
        // The shared path is carved out of the scope, which is now the only
        // legal way to declare one: a card whose include covers the artifact is
        // refused at activation. This fixture needs a legal card to exercise
        // the verify-side rule at all — it never asserted the comparison, it
        // just used a shape the hole permitted.
        let card = card_with(
            &["src/**"],
            &["src/generated.rs"],
            &[artifact("src/generated.rs", ArtifactClass::Shared)],
        );
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100644 a b M\0src/generated.rs\0"),
        );
        assert!(!report.passed, "integration owns a shared artifact");
        let detail = report
            .findings
            .iter()
            .find(|finding| finding.kind == "generated-artifact-not-owned")
            .expect("a blocking finding");
        assert!(
            detail.detail.contains("integration generates it"),
            "the reason must say who owns it: {}",
            detail.detail
        );
    }

    #[test]
    fn a_rename_within_scope_is_reported_but_does_not_block() {
        let card = card_with(&["src/**"], &[], &[]);
        let report = verify(
            &card,
            "sha256:x",
            &facts(":100644 100644 a b R097\0src/old.rs\0src/new.rs\0"),
        );
        assert!(report.passed, "{:?}", report.findings);
        assert!(kinds(&report).contains(&"rename"));
    }

    #[test]
    fn verification_is_a_pure_function_of_its_inputs() {
        // The same facts must verify identically every time, which is what
        // makes "identical results from another clean clone" achievable.
        let card = card_with(&["src/**"], &[], &[]);
        let input = facts(":100644 100644 a b M\0docs/readme.md\0");
        let first = verify(&card, "sha256:x", &input);
        let second = verify(&card, "sha256:x", &input);
        assert_eq!(first.findings, second.findings);
        assert_eq!(first.passed, second.passed);
    }

    #[test]
    fn the_report_binds_the_verdict_to_the_exact_card_and_candidate() {
        let card = card_with(&["src/**"], &[], &[]);
        let report = verify(
            &card,
            "sha256:abc",
            &facts(":100644 100644 a b M\0src/a.rs\0"),
        );
        assert_eq!(report.card_id, "F-001");
        assert_eq!(report.card_revision, 1);
        assert_eq!(report.card_digest, "sha256:abc");
        assert_eq!(report.candidate_sha, "b".repeat(40));
    }
}
