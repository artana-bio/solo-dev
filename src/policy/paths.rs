//! Path-pattern matching for write scopes.
//!
//! Written here rather than pulled in, because this is a policy boundary: what
//! a pattern means decides what a card is allowed to change, and that semantics
//! must be specified and tested exactly rather than inherited.
//!
//! Grammar, deliberately small:
//!
//! - `*` matches any run of characters within one segment, never `/`;
//! - `**` matches zero or more whole segments;
//! - everything else is literal.

/// How the host filesystem compares names.
///
/// macOS is case-insensitive by default, so `SRC/a.rs` and `src/a.rs` are the
/// same file there and two cards claiming them must be treated as overlapping.
/// Treating them as distinct on such a host would let two actors write the same
/// file believing they owned different ones.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaseSensitivity {
    /// Names differing only in case are the same path.
    Insensitive,
    /// Names differing in case are distinct paths.
    Sensitive,
}

impl CaseSensitivity {
    /// The behavior of the host this binary is running on.
    #[must_use]
    pub const fn host() -> Self {
        if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
            Self::Insensitive
        } else {
            Self::Sensitive
        }
    }

    /// Normalizes a name for comparison.
    #[must_use]
    pub fn normalize(self, value: &str) -> String {
        match self {
            Self::Insensitive => value.to_lowercase(),
            Self::Sensitive => value.to_owned(),
        }
    }
}

/// Splits a pattern or path into segments, discarding empty ones.
fn segments(value: &str, case: CaseSensitivity) -> Vec<String> {
    value
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .map(|segment| case.normalize(segment))
        .collect()
}

/// Matches one segment against one non-`**` pattern segment.
fn segment_matches(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == name;
    }
    // Anchor the literal runs between `*`s in order.
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut cursor = 0usize;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            if !name[cursor..].starts_with(part) {
                return false;
            }
            cursor += part.len();
            continue;
        }
        let Some(found) = name[cursor..].find(part) else {
            return false;
        };
        cursor += found + part.len();
    }
    // A pattern not ending in `*` must consume the whole name.
    if let Some(last) = parts.last()
        && !last.is_empty()
    {
        return name.ends_with(last) && cursor == name.len();
    }
    true
}

/// True when a pattern matches a repository-relative path.
#[must_use]
pub fn matches(pattern: &str, path: &str, case: CaseSensitivity) -> bool {
    match_segments(
        &segments(pattern, case),
        &segments(path, case),
        MatchMode::Path,
    )
}

/// Whether the right side is a concrete path or another pattern.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MatchMode {
    /// The right side is a literal path.
    Path,
    /// The right side is another pattern; wildcards on either side may match.
    Pattern,
}

/// Recursive segment matcher shared by path matching and pattern intersection.
fn match_segments(pattern: &[String], other: &[String], mode: MatchMode) -> bool {
    match (pattern.first(), other.first()) {
        (None, None) => true,
        // A trailing `**` matches the empty remainder, so `src/**` covers `src`.
        (Some(head), None) if head == "**" => match_segments(&pattern[1..], other, mode),
        (None, Some(head)) if mode == MatchMode::Pattern && head == "**" => {
            match_segments(pattern, &other[1..], mode)
        }
        (None, Some(_)) | (Some(_), None) => false,
        (Some(left), Some(right)) => {
            if left == "**" {
                // Consume zero segments, or one and retry.
                return match_segments(&pattern[1..], other, mode)
                    || match_segments(pattern, &other[1..], mode);
            }
            if mode == MatchMode::Pattern && right == "**" {
                return match_segments(pattern, &other[1..], mode)
                    || match_segments(&pattern[1..], other, mode);
            }
            let heads_match = if mode == MatchMode::Pattern {
                // Either side may hold a wildcard, so try both directions.
                segment_matches(left, right) || segment_matches(right, left)
            } else {
                segment_matches(left, right)
            };
            heads_match && match_segments(&pattern[1..], &other[1..], mode)
        }
    }
}

/// True when some path could match both patterns.
///
/// Deliberately an over-approximation in ambiguous cases: rejecting two cards
/// that might overlap costs a coordinator one conversation, while admitting two
/// that do overlap costs a silent lost write.
#[must_use]
pub fn patterns_intersect(left: &str, right: &str, case: CaseSensitivity) -> bool {
    match_segments(
        &segments(left, case),
        &segments(right, case),
        MatchMode::Pattern,
    )
}

/// A resolved include/exclude scope.
#[derive(Clone, Debug)]
pub struct Scope<'a> {
    include: &'a [String],
    exclude: &'a [String],
    case: CaseSensitivity,
}

impl<'a> Scope<'a> {
    /// Binds a scope to the host's case behavior.
    #[must_use]
    pub fn new(include: &'a [String], exclude: &'a [String]) -> Self {
        Self {
            include,
            exclude,
            case: CaseSensitivity::host(),
        }
    }

    /// Binds a scope with an explicit case behavior, for tests.
    #[must_use]
    pub const fn with_case(
        include: &'a [String],
        exclude: &'a [String],
        case: CaseSensitivity,
    ) -> Self {
        Self {
            include,
            exclude,
            case,
        }
    }

    /// True when the scope permits writing this path.
    ///
    /// Deny by default: a path outside every include is refused, and an exclude
    /// overrides any include that would have admitted it.
    #[must_use]
    pub fn allows(&self, path: &str) -> bool {
        if self
            .exclude
            .iter()
            .any(|pattern| matches(pattern, path, self.case))
        {
            return false;
        }
        self.include
            .iter()
            .any(|pattern| matches(pattern, path, self.case))
    }

    /// Every include pattern that is not fully cancelled by an exclude.
    ///
    /// Used for overlap: two cards do not conflict over a region both exclude.
    #[must_use]
    pub fn effective_includes(&self) -> Vec<&'a String> {
        self.include
            .iter()
            .filter(|pattern| {
                !self
                    .exclude
                    .iter()
                    .any(|excluded| excluded == *pattern || excluded.as_str() == "**")
            })
            .collect()
    }

    /// True when this scope could contend with another for some path.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> Option<(String, String)> {
        for left in self.effective_includes() {
            for right in other.effective_includes() {
                if patterns_intersect(left, right, self.case) {
                    return Some((left.clone(), right.clone()));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENSITIVE: CaseSensitivity = CaseSensitivity::Sensitive;
    const INSENSITIVE: CaseSensitivity = CaseSensitivity::Insensitive;

    #[test]
    fn literal_patterns_match_exactly() {
        assert!(matches("src/a.rs", "src/a.rs", SENSITIVE));
        assert!(!matches("src/a.rs", "src/b.rs", SENSITIVE));
        assert!(!matches("src/a.rs", "src/nested/a.rs", SENSITIVE));
    }

    #[test]
    fn single_star_stays_within_one_segment() {
        assert!(matches("src/*.rs", "src/a.rs", SENSITIVE));
        assert!(!matches("src/*.rs", "src/nested/a.rs", SENSITIVE));
        assert!(matches("src/*", "src/anything", SENSITIVE));
        assert!(!matches("src/*", "src/a/b", SENSITIVE));
    }

    #[test]
    fn double_star_spans_segments_including_none() {
        assert!(matches("src/**", "src/a.rs", SENSITIVE));
        assert!(matches("src/**", "src/deeply/nested/a.rs", SENSITIVE));
        assert!(
            matches("src/**", "src", SENSITIVE),
            "`**` may match nothing"
        );
        assert!(!matches("src/**", "other/a.rs", SENSITIVE));
        assert!(matches("**", "anything/at/all", SENSITIVE));
    }

    #[test]
    fn stars_inside_a_segment_anchor_correctly() {
        assert!(matches("src/test_*.rs", "src/test_a.rs", SENSITIVE));
        assert!(!matches("src/test_*.rs", "src/other.rs", SENSITIVE));
        assert!(matches("src/*_test.rs", "src/a_test.rs", SENSITIVE));
        assert!(!matches("src/*_test.rs", "src/a_test.rsx", SENSITIVE));
        assert!(matches("*.md", "README.md", SENSITIVE));
    }

    #[test]
    fn case_sensitivity_follows_the_configured_host() {
        assert!(!matches("src/A.rs", "src/a.rs", SENSITIVE));
        assert!(
            matches("src/A.rs", "src/a.rs", INSENSITIVE),
            "on a case-insensitive host these are the same file"
        );
    }

    #[test]
    fn deny_by_default_refuses_anything_outside_include() {
        let include = vec!["src/**".to_owned()];
        let exclude = vec![];
        let scope = Scope::with_case(&include, &exclude, SENSITIVE);
        assert!(scope.allows("src/a.rs"));
        assert!(!scope.allows("tests/a.rs"));
        assert!(!scope.allows("README.md"));
    }

    #[test]
    fn excludes_override_includes() {
        let include = vec!["src/**".to_owned()];
        let exclude = vec!["src/generated/**".to_owned()];
        let scope = Scope::with_case(&include, &exclude, SENSITIVE);
        assert!(scope.allows("src/a.rs"));
        assert!(
            !scope.allows("src/generated/b.rs"),
            "an exclude must win over an include that would admit the path"
        );
    }

    #[test]
    fn an_empty_scope_allows_nothing() {
        let include = vec![];
        let exclude = vec![];
        let scope = Scope::with_case(&include, &exclude, SENSITIVE);
        assert!(!scope.allows("anything"));
    }

    #[test]
    fn identical_patterns_intersect() {
        assert!(patterns_intersect("src/a.rs", "src/a.rs", SENSITIVE));
    }

    #[test]
    fn disjoint_literals_do_not_intersect() {
        assert!(!patterns_intersect("src/a.rs", "src/b.rs", SENSITIVE));
        assert!(!patterns_intersect("src/**", "tests/**", SENSITIVE));
    }

    #[test]
    fn a_wildcard_intersects_a_literal_it_could_match() {
        assert!(patterns_intersect("src/**", "src/a.rs", SENSITIVE));
        assert!(patterns_intersect("src/a.rs", "src/**", SENSITIVE));
        assert!(patterns_intersect("src/*.rs", "src/a.rs", SENSITIVE));
        assert!(!patterns_intersect("src/*.rs", "src/a.txt", SENSITIVE));
    }

    #[test]
    fn nested_wildcards_intersect() {
        assert!(patterns_intersect("src/**", "src/nested/**", SENSITIVE));
        assert!(patterns_intersect("**", "anything/here", SENSITIVE));
    }

    #[test]
    fn case_insensitive_hosts_treat_differing_case_as_overlap() {
        assert!(
            patterns_intersect("SRC/a.rs", "src/a.rs", INSENSITIVE),
            "two cards claiming the same file on a case-insensitive host overlap"
        );
        assert!(!patterns_intersect("SRC/a.rs", "src/a.rs", SENSITIVE));
    }

    #[test]
    fn overlap_reports_the_offending_pattern_pair() {
        let left_include = vec!["src/**".to_owned()];
        let right_include = vec!["docs/**".to_owned(), "src/shared.rs".to_owned()];
        let empty = vec![];
        let left = Scope::with_case(&left_include, &empty, SENSITIVE);
        let right = Scope::with_case(&right_include, &empty, SENSITIVE);

        let (a, b) = left.overlaps(&right).expect("these overlap");
        assert_eq!(a, "src/**");
        assert_eq!(b, "src/shared.rs");
    }

    #[test]
    fn disjoint_scopes_do_not_overlap() {
        let left_include = vec!["src/temperature.rs".to_owned()];
        let right_include = vec!["src/currency.rs".to_owned()];
        let empty = vec![];
        let left = Scope::with_case(&left_include, &empty, SENSITIVE);
        let right = Scope::with_case(&right_include, &empty, SENSITIVE);
        assert!(left.overlaps(&right).is_none());
    }

    #[test]
    fn leading_and_duplicate_separators_are_normalized() {
        assert!(matches("src//a.rs", "src/a.rs", SENSITIVE));
        assert!(matches("./src/a.rs", "src/a.rs", SENSITIVE));
    }
}
