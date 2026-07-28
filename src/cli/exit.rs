//! Exit-code categories.
//!
//! Section 12.2 fixes these numbers. Callers script against them, so a category
//! may be added but an existing number must never be reused for a different
//! meaning.

use std::process::ExitCode;

/// The documented exit-code categories.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExitCategory {
    /// Command completed and postconditions passed.
    Success,
    /// Invalid arguments or an unsupported option combination.
    Usage,
    /// Missing, invalid, or incompatible project or control configuration.
    Configuration,
    /// A repository, worktree, branch, state, or cleanliness check failed.
    Precondition,
    /// Ownership, dependency, identity-separation, or transition violation.
    Policy,
    /// Merge conflict or a stale expected SHA.
    Conflict,
    /// A named gate failed, timed out, or produced invalid evidence.
    Gate,
    /// Git or another required executable failed unexpectedly.
    ExternalTool,
    /// A mutation partially completed and requires `recover`.
    RecoveryRequired,
    /// A harness invariant was violated.
    Internal,
}

impl ExitCategory {
    /// Every category, for exhaustive testing and documentation.
    pub const ALL: [Self; 10] = [
        Self::Success,
        Self::Usage,
        Self::Configuration,
        Self::Precondition,
        Self::Policy,
        Self::Conflict,
        Self::Gate,
        Self::ExternalTool,
        Self::RecoveryRequired,
        Self::Internal,
    ];

    /// The process exit code for this category.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Usage => 2,
            Self::Configuration => 3,
            Self::Precondition => 4,
            Self::Policy => 5,
            Self::Conflict => 6,
            Self::Gate => 7,
            Self::ExternalTool => 8,
            Self::RecoveryRequired => 9,
            Self::Internal => 10,
        }
    }

    /// The category's stable name, used inside error codes.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Usage => "USAGE",
            Self::Configuration => "CONFIG",
            Self::Precondition => "PRECONDITION",
            Self::Policy => "POLICY",
            Self::Conflict => "CONFLICT",
            Self::Gate => "GATE",
            Self::ExternalTool => "EXTERNAL",
            Self::RecoveryRequired => "RECOVERY",
            Self::Internal => "INTERNAL",
        }
    }
}

impl From<ExitCategory> for ExitCode {
    fn from(category: ExitCategory) -> Self {
        Self::from(category.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_category_maps_to_its_documented_code() {
        assert_eq!(ExitCategory::Success.code(), 0);
        assert_eq!(ExitCategory::Usage.code(), 2);
        assert_eq!(ExitCategory::Configuration.code(), 3);
        assert_eq!(ExitCategory::Precondition.code(), 4);
        assert_eq!(ExitCategory::Policy.code(), 5);
        assert_eq!(ExitCategory::Conflict.code(), 6);
        assert_eq!(ExitCategory::Gate.code(), 7);
        assert_eq!(ExitCategory::ExternalTool.code(), 8);
        assert_eq!(ExitCategory::RecoveryRequired.code(), 9);
        assert_eq!(ExitCategory::Internal.code(), 10);
    }

    #[test]
    fn all_lists_every_category_exactly_once() {
        let unique: HashSet<_> = ExitCategory::ALL.iter().collect();
        assert_eq!(unique.len(), ExitCategory::ALL.len());
    }

    #[test]
    fn codes_are_unique_and_never_use_one() {
        let mut seen = HashSet::new();
        for category in ExitCategory::ALL {
            assert!(seen.insert(category.code()), "duplicate {category:?}");
            assert_ne!(
                category.code(),
                1,
                "exit 1 is reserved so an uncategorized failure is distinguishable"
            );
        }
    }

    #[test]
    fn names_are_unique_and_uppercase() {
        let mut seen = HashSet::new();
        for category in ExitCategory::ALL {
            assert!(seen.insert(category.name()), "duplicate {category:?}");
            assert!(
                category
                    .name()
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase()),
                "{category:?} name must be uppercase ASCII"
            );
        }
    }
}
