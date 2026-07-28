//! NUL-safe diff parsing.
//!
//! Scope enforcement decides whether a candidate may hand off, so the parser
//! must be exact. It reads `--raw -z`, not `--name-status`, because only the
//! raw format carries file modes, and modes are how a symlink, a submodule
//! entry, or a new executable bit becomes visible.
//!
//! NUL delimiting matters because a path may legally contain a newline, a
//! quote, or arbitrary Unicode. A line-oriented parser would mis-split those.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::HarnessError;

use super::command::{GitScope, run_ok};

/// Git's mode for an absent side of a change.
const MODE_ABSENT: &str = "000000";
/// Git's mode for a symbolic link.
const MODE_SYMLINK: &str = "120000";
/// Git's mode for a gitlink, meaning a submodule entry.
const MODE_GITLINK: &str = "160000";
/// Git's mode for an executable regular file.
const MODE_EXECUTABLE: &str = "100755";

/// How one path changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// The path was added.
    Added,
    /// The path's content changed.
    Modified,
    /// The path was deleted.
    Deleted,
    /// The path was renamed from another path.
    Renamed,
    /// The path was copied from another path.
    Copied,
    /// The path's type changed, such as file to symlink.
    TypeChanged,
    /// The path is unmerged.
    Unmerged,
    /// Git reported a status this parser does not model.
    Unknown,
}

impl ChangeKind {
    /// Maps Git's raw status letter.
    #[must_use]
    fn from_status(status: &str) -> Self {
        match status.chars().next() {
            Some('A') => Self::Added,
            Some('M') => Self::Modified,
            Some('D') => Self::Deleted,
            Some('R') => Self::Renamed,
            Some('C') => Self::Copied,
            Some('T') => Self::TypeChanged,
            Some('U') => Self::Unmerged,
            _ => Self::Unknown,
        }
    }
}

/// One changed path, with enough detail to enforce scope and safety policy.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ChangedPath {
    /// How the path changed.
    pub kind: ChangeKind,
    /// The path after the change.
    pub path: String,
    /// The path before a rename or copy.
    pub previous_path: Option<String>,
    /// Mode before the change.
    pub old_mode: String,
    /// Mode after the change.
    pub new_mode: String,
    /// Similarity score for a rename or copy, when Git reported one.
    pub score: Option<u32>,
}

impl ChangedPath {
    /// Every path this change touches, meaning both sides of a rename.
    #[must_use]
    pub fn touched_paths(&self) -> Vec<&str> {
        let mut paths = vec![self.path.as_str()];
        paths.extend(self.previous_path.as_deref());
        paths
    }

    /// True when either side of the change is a symbolic link.
    #[must_use]
    pub fn involves_symlink(&self) -> bool {
        self.old_mode == MODE_SYMLINK || self.new_mode == MODE_SYMLINK
    }

    /// True when either side of the change is a submodule entry.
    #[must_use]
    pub fn involves_submodule(&self) -> bool {
        self.old_mode == MODE_GITLINK || self.new_mode == MODE_GITLINK
    }

    /// True when the executable bit changed on an existing file.
    #[must_use]
    pub fn changes_executable_bit(&self) -> bool {
        self.old_mode != MODE_ABSENT
            && self.new_mode != MODE_ABSENT
            && self.old_mode != self.new_mode
            && (self.old_mode == MODE_EXECUTABLE || self.new_mode == MODE_EXECUTABLE)
    }

    /// True when the entry's mode changed at all between two present sides.
    #[must_use]
    pub fn changes_mode(&self) -> bool {
        self.old_mode != MODE_ABSENT
            && self.new_mode != MODE_ABSENT
            && self.old_mode != self.new_mode
    }
}

/// The complete set of changes between two commits.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct DiffSummary {
    /// Every changed path.
    pub paths: Vec<ChangedPath>,
}

impl DiffSummary {
    /// Every distinct path touched, including rename sources, sorted.
    #[must_use]
    pub fn touched_paths(&self) -> BTreeSet<&str> {
        self.paths
            .iter()
            .flat_map(ChangedPath::touched_paths)
            .collect()
    }

    /// True when any change touches a `.gitmodules` file.
    #[must_use]
    pub fn touches_gitmodules(&self) -> bool {
        self.touched_paths()
            .iter()
            .any(|path| *path == ".gitmodules" || path.ends_with("/.gitmodules"))
    }

    /// Changes involving a symbolic link on either side.
    #[must_use]
    pub fn symlink_changes(&self) -> Vec<&ChangedPath> {
        self.paths
            .iter()
            .filter(|change| change.involves_symlink())
            .collect()
    }

    /// Changes involving a submodule entry on either side.
    #[must_use]
    pub fn submodule_changes(&self) -> Vec<&ChangedPath> {
        self.paths
            .iter()
            .filter(|change| change.involves_submodule())
            .collect()
    }
}

/// Computes the change set between two commits.
///
/// Rename detection is on because a rename that crosses an ownership boundary
/// must be visible as both a source and a destination, not as an unrelated
/// add-and-delete pair.
///
/// # Errors
///
/// Returns an error when Git cannot be executed or either revision is unknown.
pub fn diff_commits(scope: &GitScope, base: &str, head: &str) -> Result<DiffSummary, HarnessError> {
    let output = run_ok(
        scope,
        [
            "diff",
            "--raw",
            "-z",
            "--find-renames",
            "--find-copies",
            base,
            head,
        ],
    )?;
    parse_raw_z(&output.stdout)
}

/// Parses `git diff --raw -z` output.
///
/// Each record is `:<old> <new> <oldsha> <newsha> <status>` followed by one
/// NUL-terminated path, or two for a rename or copy.
///
/// # Errors
///
/// Returns an error when a record is structurally malformed.
pub fn parse_raw_z(raw: &str) -> Result<DiffSummary, HarnessError> {
    let mut fields = raw.split('\0').filter(|field| !field.is_empty());
    let mut paths = Vec::new();

    while let Some(header) = fields.next() {
        let header = header.strip_prefix(':').ok_or_else(|| {
            HarnessError::GitCommand(format!("malformed diff record header: {header}"))
        })?;
        let parts: Vec<&str> = header.split_whitespace().collect();
        let [old_mode, new_mode, _old_oid, _new_oid, status] = parts.as_slice() else {
            return Err(HarnessError::GitCommand(format!(
                "malformed diff record header: :{header}"
            )));
        };

        let kind = ChangeKind::from_status(status);
        let score = status[1..].parse::<u32>().ok();
        let takes_two_paths = matches!(kind, ChangeKind::Renamed | ChangeKind::Copied);

        let first = fields.next().ok_or_else(|| {
            HarnessError::GitCommand("diff record is missing its path".to_owned())
        })?;
        let (previous_path, path) = if takes_two_paths {
            let second = fields.next().ok_or_else(|| {
                HarnessError::GitCommand("rename record is missing its destination".to_owned())
            })?;
            (Some(first.to_owned()), second.to_owned())
        } else {
            (None, first.to_owned())
        };

        paths.push(ChangedPath {
            kind,
            path,
            previous_path,
            old_mode: (*old_mode).to_owned(),
            new_mode: (*new_mode).to_owned(),
            score,
        });
    }

    Ok(DiffSummary { paths })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_simple_modification() {
        let raw = ":100644 100644 aaa bbb M\0src/lib.rs\0";
        let summary = parse_raw_z(raw).unwrap();
        assert_eq!(summary.paths.len(), 1);
        assert_eq!(summary.paths[0].kind, ChangeKind::Modified);
        assert_eq!(summary.paths[0].path, "src/lib.rs");
        assert!(summary.paths[0].previous_path.is_none());
    }

    #[test]
    fn parses_a_rename_and_keeps_both_sides() {
        let raw = ":100644 100644 aaa bbb R096\0old/name.rs\0new/name.rs\0";
        let summary = parse_raw_z(raw).unwrap();
        let change = &summary.paths[0];
        assert_eq!(change.kind, ChangeKind::Renamed);
        assert_eq!(change.previous_path.as_deref(), Some("old/name.rs"));
        assert_eq!(change.path, "new/name.rs");
        assert_eq!(change.score, Some(96));
        assert_eq!(
            summary.touched_paths().into_iter().collect::<Vec<_>>(),
            vec!["new/name.rs", "old/name.rs"]
        );
    }

    #[test]
    fn parses_paths_containing_newlines_spaces_and_unicode() {
        let raw = ":100644 100644 aaa bbb M\0weird/na me\nwith\nnewlines.rs\0\
                   :100644 100644 ccc ddd M\0docs/ünïcodé — dash.md\0";
        let summary = parse_raw_z(raw).unwrap();
        assert_eq!(summary.paths.len(), 2);
        assert_eq!(summary.paths[0].path, "weird/na me\nwith\nnewlines.rs");
        assert_eq!(summary.paths[1].path, "docs/ünïcodé — dash.md");
    }

    #[test]
    fn detects_additions_and_deletions_by_absent_mode() {
        let raw = ":000000 100644 0000 bbb A\0added.rs\0:100644 000000 aaa 0000 D\0gone.rs\0";
        let summary = parse_raw_z(raw).unwrap();
        assert_eq!(summary.paths[0].kind, ChangeKind::Added);
        assert_eq!(summary.paths[0].old_mode, MODE_ABSENT);
        assert_eq!(summary.paths[1].kind, ChangeKind::Deleted);
        assert_eq!(summary.paths[1].new_mode, MODE_ABSENT);
    }

    #[test]
    fn detects_symlinks_submodules_and_mode_flips() {
        let raw = ":000000 120000 0000 bbb A\0link\0\
                   :000000 160000 0000 ccc A\0vendor/dep\0\
                   :100644 100755 aaa bbb M\0script.sh\0";
        let summary = parse_raw_z(raw).unwrap();

        assert!(summary.paths[0].involves_symlink());
        assert_eq!(summary.symlink_changes().len(), 1);

        assert!(summary.paths[1].involves_submodule());
        assert_eq!(summary.submodule_changes().len(), 1);

        assert!(summary.paths[2].changes_executable_bit());
        assert!(summary.paths[2].changes_mode());
    }

    #[test]
    fn an_addition_is_not_reported_as_a_mode_change() {
        let raw = ":000000 100755 0000 bbb A\0new-script.sh\0";
        let summary = parse_raw_z(raw).unwrap();
        assert!(!summary.paths[0].changes_executable_bit());
        assert!(!summary.paths[0].changes_mode());
    }

    #[test]
    fn detects_gitmodules_changes_at_any_depth() {
        let root = parse_raw_z(":100644 100644 aaa bbb M\0.gitmodules\0").unwrap();
        assert!(root.touches_gitmodules());
        let nested = parse_raw_z(":100644 100644 aaa bbb M\0sub/.gitmodules\0").unwrap();
        assert!(nested.touches_gitmodules());
        let unrelated = parse_raw_z(":100644 100644 aaa bbb M\0notgitmodules\0").unwrap();
        assert!(!unrelated.touches_gitmodules());
    }

    #[test]
    fn detects_a_type_change() {
        let raw = ":100644 120000 aaa bbb T\0was-a-file\0";
        let summary = parse_raw_z(raw).unwrap();
        assert_eq!(summary.paths[0].kind, ChangeKind::TypeChanged);
        assert!(summary.paths[0].involves_symlink());
    }

    #[test]
    fn empty_output_is_an_empty_summary() {
        assert!(parse_raw_z("").unwrap().paths.is_empty());
        assert!(parse_raw_z("\0\0").unwrap().paths.is_empty());
    }

    #[test]
    fn rejects_malformed_records() {
        assert!(parse_raw_z("no-colon-prefix\0path\0").is_err());
        assert!(parse_raw_z(":100644 100644 aaa M\0path\0").is_err());
        assert!(parse_raw_z(":100644 100644 aaa bbb M\0").is_err());
        assert!(parse_raw_z(":100644 100644 aaa bbb R100\0only-source\0").is_err());
    }
}
