//! Non-destructive merge preflight via `git merge-tree --write-tree`.
//!
//! Section 13.4 permits this only once the minimum Git version and the
//! command's output and exit behavior are covered by tests, which is what the
//! test module below does against a real repository. There is deliberately no
//! fallback: `MINIMUM_GIT_VERSION` exists for this command, project validation
//! refuses an older Git, and silently switching to a worktree-based merge would
//! mean the preflight and the real merge could disagree.
//!
//! The value of `merge-tree` here is that it answers "would this merge" without
//! touching a working tree, an index, or a ref. Nothing it does can be left
//! half-finished.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{
    error::{ErrorCode, HarnessError},
    git::command::{GitScope, run},
};

/// What kind of disagreement Git reported.
///
/// The split that matters is [`ConflictKind::class`]: a content conflict is a
/// human editing job inside one file, while a structural conflict means the two
/// sides disagree about whether a path should exist at all. Reporting both as
/// "conflict" would leave an actor guessing which one they have.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Both sides changed the same region of a file.
    Content,
    /// One side changed a file the other deleted.
    ModifyDelete,
    /// Both sides renamed the same file differently, or onto each other.
    RenameRename,
    /// One side renamed a file the other deleted.
    RenameDelete,
    /// One side put a directory where the other put a file.
    DirectoryFile,
    /// The sides disagree about a path's object type.
    DistinctTypes,
    /// Both sides changed a file Git cannot merge textually.
    ///
    /// Structural, not textual, and the distinction is not cosmetic: Git writes
    /// "our" side verbatim into the conflicted tree with no conflict markers, so
    /// telling an actor to resolve it by editing markers sends them looking for
    /// something that is not there.
    Binary,
    /// Anything Git reports that this list does not name.
    Other,
}

/// Whether a conflict is resolved by editing content or by deciding structure.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictClass {
    /// Resolved by editing the file's contents.
    Textual,
    /// Resolved by deciding what should exist, not what it should contain.
    Structural,
}

impl ConflictKind {
    /// Classifies Git's conflict token.
    ///
    /// Unrecognized tokens become [`ConflictKind::Other`] rather than being
    /// dropped. A conflict this code cannot name is still a conflict, and
    /// discarding it would turn an unknown failure into a clean preflight.
    #[must_use]
    pub fn parse(token: &str) -> Self {
        match token {
            "CONFLICT (contents)" => Self::Content,
            "CONFLICT (modify/delete)" => Self::ModifyDelete,
            "CONFLICT (rename/rename)" => Self::RenameRename,
            "CONFLICT (rename/delete)" => Self::RenameDelete,
            "CONFLICT (file/directory)" => Self::DirectoryFile,
            // `distinct modes`, not `distinct types`. The phrase "distinct
            // types" appears only inside Git's human message, so the old arm
            // was unreachable and every file-versus-symlink change was reported
            // as `other`. Likewise `CONFLICT (directory/file)` is a token no
            // supported Git emits, in either merge direction.
            "CONFLICT (distinct modes)" => Self::DistinctTypes,
            "CONFLICT (binary)" => Self::Binary,
            _ => Self::Other,
        }
    }

    /// How this conflict has to be resolved.
    #[must_use]
    pub const fn class(self) -> ConflictClass {
        match self {
            Self::Content => ConflictClass::Textual,
            // `Other` counts as structural because an unclassified conflict
            // must not be presented as the milder, well-understood case.
            Self::ModifyDelete
            | Self::RenameRename
            | Self::RenameDelete
            | Self::DirectoryFile
            | Self::DistinctTypes
            | Self::Binary
            | Self::Other => ConflictClass::Structural,
        }
    }

    /// Its stable serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::ModifyDelete => "modify_delete",
            Self::RenameRename => "rename_rename",
            Self::RenameDelete => "rename_delete",
            Self::DirectoryFile => "directory_file",
            Self::DistinctTypes => "distinct_types",
            Self::Binary => "binary",
            Self::Other => "other",
        }
    }
}

/// One reported conflict.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Conflict {
    /// The paths Git named. Rename conflicts name more than one.
    pub paths: Vec<String>,
    /// What kind of conflict it is.
    pub kind: ConflictKind,
    /// Git's own description, kept verbatim.
    pub detail: String,
}

/// The result of merging two commits without touching anything.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MergePreview {
    /// The tree the merge would produce.
    ///
    /// Git writes a tree even for a conflicted merge, with conflict markers in
    /// the conflicting files. That tree is never used as a landing tree; it is
    /// recorded only so a conflicted preflight is reproducible.
    pub tree: String,
    /// Every conflict Git reported, in the order it reported them.
    pub conflicts: Vec<Conflict>,
}

impl MergePreview {
    /// True when the merge would apply without conflict.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// Conflicts of the given class.
    pub fn of_class(&self, class: ConflictClass) -> impl Iterator<Item = &Conflict> {
        self.conflicts
            .iter()
            .filter(move |conflict| conflict.kind.class() == class)
    }
}

/// Merges two commits in memory, changing nothing.
///
/// # Errors
///
/// Returns an external-tool error when Git fails for any reason other than
/// reporting conflicts, which includes refusing unrelated histories.
pub fn merge_tree(
    repository: &Path,
    ours: &str,
    theirs: &str,
) -> Result<MergePreview, HarnessError> {
    let scope = GitScope::work_tree(repository);
    let output = run(
        &scope,
        [
            "merge-tree",
            "--write-tree",
            "--name-only",
            "-z",
            ours,
            theirs,
        ],
    )?;

    // Exit 0 is a clean merge and exit 1 is a conflicted one; both produced a
    // tree. Anything else means Git did not perform the merge at all.
    if output.code != Some(0) && output.code != Some(1) {
        return Err(HarnessError::Control {
            reason: format!(
                "git merge-tree could not merge {ours} into {theirs}: {}",
                output.diagnostic()
            ),
            code: ErrorCode::ExternalGitCommand,
        });
    }

    parse_preview(&output.stdout)
}

/// Parses the `-z` output of `merge-tree --write-tree --name-only`.
///
/// The format is two NUL-NUL-separated sections. The first holds the tree OID
/// followed by the NUL-separated names of conflicted paths; the second is an
/// informational block. Only the OID and the informational block are used —
/// the name list says which paths conflicted but not how, which is the part
/// that decides whether an actor edits a file or decides its fate.
fn parse_preview(stdout: &str) -> Result<MergePreview, HarnessError> {
    let mut sections = stdout.split("\0\0");
    let tree = sections
        .next()
        .and_then(|section| section.split('\0').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| HarnessError::Control {
            reason: "git merge-tree produced no tree".to_owned(),
            code: ErrorCode::ExternalGitCommand,
        })?
        .to_owned();

    let Some(information) = sections.next() else {
        return Ok(MergePreview {
            tree,
            conflicts: Vec::new(),
        });
    };

    // Each record is: path count, that many paths, a type token, a message.
    let mut fields = information.split('\0').filter(|field| !field.is_empty());
    let mut conflicts = Vec::new();
    while let Some(count) = fields.next() {
        let Ok(count) = count.parse::<usize>() else {
            break;
        };
        let paths: Vec<String> = fields.by_ref().take(count).map(ToOwned::to_owned).collect();
        let (Some(token), Some(detail)) = (fields.next(), fields.next()) else {
            break;
        };
        // `Auto-merging` records a file Git merged successfully and is not a
        // conflict; only the CONFLICT records are.
        if !token.starts_with("CONFLICT") {
            continue;
        }
        conflicts.push(Conflict {
            paths,
            kind: ConflictKind::parse(token),
            detail: detail.trim().to_owned(),
        });
    }

    // Git reports a binary conflict twice for the same path: `CONFLICT (binary)`
    // and then `CONFLICT (contents)`. The second is a lie about what is in the
    // tree — Git wrote one side verbatim, with no conflict markers — so keeping
    // it both doubles the count and tells an actor to resolve markers that do
    // not exist. The `binary` record is the one kept, because its detail names
    // both sides, which is what someone choosing a side needs.
    //
    // Matched on the exact path vector rather than membership: every binary form
    // observed (real binary content, binary add/add, and a text file marked
    // binary in .gitattributes) carries the same single-path vector in both
    // records, so equality is as loose as the evidence supports.
    let binary: std::collections::HashSet<Vec<String>> = conflicts
        .iter()
        .filter(|conflict| conflict.kind == ConflictKind::Binary)
        .map(|conflict| conflict.paths.clone())
        .collect();
    conflicts.retain(|conflict| {
        conflict.kind != ConflictKind::Content || !binary.contains(&conflict.paths)
    });

    Ok(MergePreview { tree, conflicts })
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, process::Command};

    use tempfile::TempDir;

    use super::*;

    struct Fixture {
        _temp: TempDir,
        path: PathBuf,
    }

    impl Fixture {
        /// A repository with a base commit on `main`.
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("temp dir");
            let path = temp.path().to_path_buf();
            git(&path, &["init", "-q", "-b", "main", "."]);
            git(&path, &["config", "user.email", "f@local.invalid"]);
            git(&path, &["config", "user.name", "Fixture"]);
            fs::write(path.join("f.txt"), "line1\nline2\nline3\n").unwrap();
            git(&path, &["add", "-A"]);
            git(&path, &["commit", "-q", "-m", "base"]);
            Self { _temp: temp, path }
        }

        /// Branches from `main` and applies one change.
        fn branch(&self, name: &str, apply: impl FnOnce(&PathBuf)) {
            git(&self.path, &["checkout", "-q", "main"]);
            git(&self.path, &["checkout", "-q", "-b", name]);
            apply(&self.path);
            git(&self.path, &["add", "-A"]);
            git(&self.path, &["commit", "-q", "-m", name]);
        }
    }

    fn git(repo: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn a_clean_merge_reports_a_tree_and_no_conflicts() {
        let fixture = Fixture::new();
        fixture.branch("a", |path| {
            fs::write(path.join("f.txt"), "A1\nline2\nline3\n").unwrap();
        });
        fixture.branch("c", |path| {
            fs::write(path.join("f.txt"), "line1\nline2\nC3\n").unwrap();
        });

        let preview = merge_tree(&fixture.path, "a", "c").expect("a preview");
        assert!(preview.is_clean(), "unexpected: {preview:?}");
        assert_eq!(preview.tree.len(), 40, "a tree OID: {}", preview.tree);
    }

    #[test]
    fn overlapping_edits_are_a_textual_conflict() {
        let fixture = Fixture::new();
        fixture.branch("a", |path| {
            fs::write(path.join("f.txt"), "A1\nline2\nline3\n").unwrap();
        });
        fixture.branch("b", |path| {
            fs::write(path.join("f.txt"), "B1\nline2\nline3\n").unwrap();
        });

        let preview = merge_tree(&fixture.path, "a", "b").expect("a preview");
        assert!(!preview.is_clean());
        assert_eq!(preview.conflicts.len(), 1, "unexpected: {preview:?}");
        let conflict = &preview.conflicts[0];
        assert_eq!(conflict.kind, ConflictKind::Content);
        assert_eq!(conflict.kind.class(), ConflictClass::Textual);
        assert_eq!(conflict.paths, ["f.txt"]);
        assert!(
            conflict.detail.contains("f.txt"),
            "Git's own wording is kept: {conflict:?}"
        );
    }

    #[test]
    fn a_modified_and_deleted_file_is_a_structural_conflict() {
        let fixture = Fixture::new();
        fixture.branch("a", |path| {
            fs::write(path.join("f.txt"), "A1\nline2\nline3\n").unwrap();
        });
        fixture.branch("d", |path| {
            fs::remove_file(path.join("f.txt")).unwrap();
        });

        let preview = merge_tree(&fixture.path, "a", "d").expect("a preview");
        assert_eq!(preview.conflicts.len(), 1, "unexpected: {preview:?}");
        assert_eq!(preview.conflicts[0].kind, ConflictKind::ModifyDelete);
        assert_eq!(
            preview.conflicts[0].kind.class(),
            ConflictClass::Structural,
            "a deleted file is not resolved by editing it"
        );
        assert_eq!(preview.of_class(ConflictClass::Textual).count(), 0);
        assert_eq!(preview.of_class(ConflictClass::Structural).count(), 1);
    }

    #[test]
    fn two_sides_adding_the_same_path_conflict() {
        let fixture = Fixture::new();
        fixture.branch("e1", |path| {
            fs::write(path.join("new.txt"), "x\n").unwrap();
        });
        fixture.branch("e2", |path| {
            fs::write(path.join("new.txt"), "y\n").unwrap();
        });

        let preview = merge_tree(&fixture.path, "e1", "e2").expect("a preview");
        assert_eq!(preview.conflicts.len(), 1, "unexpected: {preview:?}");
        assert_eq!(preview.conflicts[0].paths, ["new.txt"]);
    }

    #[test]
    fn successfully_merged_files_are_not_reported_as_conflicts() {
        // Git emits an `Auto-merging` record for every file it merged, in the
        // same block as the conflicts. Treating those as conflicts would make
        // any non-trivial merge look broken.
        let fixture = Fixture::new();
        fixture.branch("a", |path| {
            fs::write(path.join("f.txt"), "A1\nline2\nline3\n").unwrap();
            fs::write(path.join("g.txt"), "g\n").unwrap();
        });
        fixture.branch("b", |path| {
            fs::write(path.join("f.txt"), "line1\nline2\nB3\n").unwrap();
        });

        let preview = merge_tree(&fixture.path, "a", "b").expect("a preview");
        assert!(
            preview.is_clean(),
            "auto-merged files must not read as conflicts: {preview:?}"
        );
    }

    #[test]
    fn unrelated_histories_are_an_error_not_a_clean_merge() {
        let fixture = Fixture::new();
        let other = tempfile::tempdir().expect("temp dir");
        git(other.path(), &["init", "-q", "-b", "main", "."]);
        git(other.path(), &["config", "user.email", "f@local.invalid"]);
        git(other.path(), &["config", "user.name", "Fixture"]);
        fs::write(other.path().join("z.txt"), "z\n").unwrap();
        git(other.path(), &["add", "-A"]);
        git(other.path(), &["commit", "-q", "-m", "unrelated"]);

        git(
            &fixture.path,
            &[
                "remote",
                "add",
                "other",
                &other.path().display().to_string(),
            ],
        );
        git(&fixture.path, &["fetch", "-q", "other"]);

        // Git exits 128 here. Reading that as "no conflicts" would let an
        // unrelated tree through the preflight.
        let error = merge_tree(&fixture.path, "main", "other/main")
            .expect_err("unrelated histories should fail");
        assert!(
            error.to_string().contains("unrelated histories"),
            "Git's diagnostic must survive: {error}"
        );
    }

    /// Adds a committed binary file to `main` before any branching.
    fn with_binary(fixture: &Fixture) {
        git(&fixture.path, &["checkout", "-q", "main"]);
        fs::write(fixture.path.join("b.bin"), b"\x00\x01\x02base\x00").unwrap();
        git(&fixture.path, &["add", "-A"]);
        git(&fixture.path, &["commit", "-q", "-m", "bin base"]);
    }

    #[test]
    fn a_binary_conflict_is_reported_once_and_is_not_textual() {
        // Git reports a binary conflict three times — `CONFLICT (binary)`,
        // `Auto-merging`, `CONFLICT (contents)` — and the parser kept two of
        // them, so one path became two conflicts and one of those was labelled
        // textual. There are no conflict markers in the tree to edit: Git wrote
        // "our" side verbatim.
        let fixture = Fixture::new();
        with_binary(&fixture);
        fixture.branch("ba", |path| {
            fs::write(path.join("b.bin"), b"\x00\x01AAAA\x00").unwrap();
        });
        fixture.branch("bb", |path| {
            fs::write(path.join("b.bin"), b"\x00\x01BBBB\x00").unwrap();
        });

        let preview = merge_tree(&fixture.path, "ba", "bb").expect("a preview");
        assert_eq!(preview.conflicts.len(), 1, "unexpected: {preview:?}");
        assert_eq!(preview.conflicts[0].kind, ConflictKind::Binary);
        assert_eq!(preview.of_class(ConflictClass::Textual).count(), 0);
        assert_eq!(preview.of_class(ConflictClass::Structural).count(), 1);
    }

    #[test]
    fn a_text_conflict_beside_a_binary_one_survives() {
        // The guard against collapsing too much. Deduplicating by path
        // membership rather than by the exact path vector, or dropping every
        // `content` record once any binary conflict exists, would silently eat a
        // real textual conflict — and textual conflict reporting is what this
        // command is for.
        let fixture = Fixture::new();
        with_binary(&fixture);
        fixture.branch("ma", |path| {
            fs::write(path.join("b.bin"), b"\x00\x01AAAA\x00").unwrap();
            fs::write(path.join("f.txt"), "line1\nMINE\nline3\n").unwrap();
        });
        fixture.branch("mb", |path| {
            fs::write(path.join("b.bin"), b"\x00\x01BBBB\x00").unwrap();
            fs::write(path.join("f.txt"), "line1\nTHEIRS\nline3\n").unwrap();
        });

        let preview = merge_tree(&fixture.path, "ma", "mb").expect("a preview");
        assert_eq!(preview.conflicts.len(), 2, "unexpected: {preview:?}");
        assert_eq!(preview.of_class(ConflictClass::Textual).count(), 1);
        assert_eq!(
            preview
                .of_class(ConflictClass::Textual)
                .next()
                .expect("a textual conflict")
                .paths,
            ["f.txt"]
        );
        assert_eq!(preview.of_class(ConflictClass::Structural).count(), 1);
    }

    #[test]
    fn a_type_change_is_named_not_lumped_into_other() {
        // Git's token is `CONFLICT (distinct modes)`; the table looked for
        // `distinct types`, which appears only in the human message. So the
        // `DistinctTypes` variant was unreachable and every type change was
        // reported as `other`.
        let fixture = Fixture::new();
        fixture.branch("ta", |path| {
            fs::write(path.join("t"), "a regular file\n").unwrap();
        });
        fixture.branch("tb", |path| {
            std::os::unix::fs::symlink("f.txt", path.join("t")).unwrap();
        });

        let preview = merge_tree(&fixture.path, "ta", "tb").expect("a preview");
        assert_eq!(preview.conflicts.len(), 1, "unexpected: {preview:?}");
        assert_eq!(preview.conflicts[0].kind, ConflictKind::DistinctTypes);
        assert_eq!(preview.conflicts[0].kind.class(), ConflictClass::Structural);
    }

    #[test]
    fn a_file_against_a_directory_is_named() {
        let fixture = Fixture::new();
        fixture.branch("fa", |path| {
            fs::write(path.join("thing"), "a file\n").unwrap();
        });
        fixture.branch("fb", |path| {
            fs::create_dir_all(path.join("thing")).unwrap();
            fs::write(path.join("thing/inner.txt"), "inside\n").unwrap();
        });

        let preview = merge_tree(&fixture.path, "fa", "fb").expect("a preview");
        assert_eq!(preview.conflicts.len(), 1, "unexpected: {preview:?}");
        assert_eq!(preview.conflicts[0].kind, ConflictKind::DirectoryFile);
    }

    #[test]
    fn an_unknown_conflict_token_is_kept_and_treated_as_structural() {
        // Guards the parser's default. A conflict this code cannot name must
        // not vanish, and must not be reported as the milder textual case.
        //
        // The token must be one Git cannot emit. This used `CONFLICT
        // (submodule)`, which is a real Git 2.50 token, so the test was pinning
        // the table's gap open rather than guarding its default — and it would
        // have silently become a submodule-classification test the moment
        // anyone extended the table.
        let kind = ConflictKind::parse("CONFLICT (wormhole)");
        assert_eq!(kind, ConflictKind::Other);
        assert_eq!(kind.class(), ConflictClass::Structural);
    }
}
