//! Error types and the machine-readable error-code registry.
//!
//! Every failure carries a stable code that scripts and agents may match on.
//! Codes are append-only: a code's meaning must never change once released.

use std::{io, path::PathBuf};

use thiserror::Error;

use crate::cli::exit::ExitCategory;

/// A stable, machine-readable error code.
///
/// The rendered form is `CH-<CATEGORY>-<DETAIL>`. Categories come from
/// [`ExitCategory`], so a code always implies its exit status.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ErrorCode {
    /// An identifier did not match its documented shape.
    UsageInvalidId,
    /// A digest did not match `sha256:<64 lowercase hex>`.
    UsageInvalidDigest,
    /// A timestamp was outside the supported range.
    UsageInvalidTimestamp,
    /// Two options were combined in an unsupported way.
    UsageConflictingOptions,
    /// The requested workspace path does not exist.
    PreconditionWorkspaceMissing,
    /// The requested workspace path could not be read.
    PreconditionWorkspaceAccess,
    /// A configuration file could not be read or parsed.
    ConfigMalformed,
    /// A configuration document carried a field the schema does not define.
    ConfigUnknownField,
    /// A configured path was not absolute.
    ConfigPathNotAbsolute,
    /// A configured path did not exist.
    ConfigPathMissing,
    /// Two configured paths resolved to the same location.
    ConfigPathAlias,
    /// A control or authority path was nested inside a candidate worktree.
    ConfigPathNested,
    /// The configured candidate path is not a Git repository.
    ConfigNotRepository,
    /// The configured protected branch does not resolve to one commit.
    ConfigProtectedBranch,
    /// The installed Git is older than the configured minimum.
    ConfigGitVersion,
    /// The host operating system is not in the configured support list.
    ConfigUnsupportedHost,
    /// A configured value was structurally valid but not usable.
    ConfigInvalidValue,
    /// Git could not be executed.
    ExternalGitUnavailable,
    /// A Git command failed.
    ExternalGitCommand,
    /// A value could not be encoded for output.
    InternalEncoding,
}

impl ErrorCode {
    /// Every registered code, for exhaustive testing and documentation.
    pub const ALL: [Self; 20] = [
        Self::UsageInvalidId,
        Self::UsageInvalidDigest,
        Self::UsageInvalidTimestamp,
        Self::UsageConflictingOptions,
        Self::PreconditionWorkspaceMissing,
        Self::PreconditionWorkspaceAccess,
        Self::ConfigMalformed,
        Self::ConfigUnknownField,
        Self::ConfigPathNotAbsolute,
        Self::ConfigPathMissing,
        Self::ConfigPathAlias,
        Self::ConfigPathNested,
        Self::ConfigNotRepository,
        Self::ConfigProtectedBranch,
        Self::ConfigGitVersion,
        Self::ConfigUnsupportedHost,
        Self::ConfigInvalidValue,
        Self::ExternalGitUnavailable,
        Self::ExternalGitCommand,
        Self::InternalEncoding,
    ];

    /// The exit category this code belongs to.
    #[must_use]
    pub const fn category(self) -> ExitCategory {
        match self {
            Self::UsageInvalidId
            | Self::UsageInvalidDigest
            | Self::UsageInvalidTimestamp
            | Self::UsageConflictingOptions => ExitCategory::Usage,
            Self::PreconditionWorkspaceMissing | Self::PreconditionWorkspaceAccess => {
                ExitCategory::Precondition
            }
            Self::ConfigMalformed
            | Self::ConfigUnknownField
            | Self::ConfigPathNotAbsolute
            | Self::ConfigPathMissing
            | Self::ConfigPathAlias
            | Self::ConfigPathNested
            | Self::ConfigNotRepository
            | Self::ConfigProtectedBranch
            | Self::ConfigGitVersion
            | Self::ConfigUnsupportedHost
            | Self::ConfigInvalidValue => ExitCategory::Configuration,
            Self::ExternalGitUnavailable | Self::ExternalGitCommand => ExitCategory::ExternalTool,
            Self::InternalEncoding => ExitCategory::Internal,
        }
    }

    /// The part of the code following the category segment.
    #[must_use]
    const fn detail(self) -> &'static str {
        match self {
            Self::UsageInvalidId => "INVALID-ID",
            Self::UsageInvalidDigest => "INVALID-DIGEST",
            Self::UsageInvalidTimestamp => "INVALID-TIMESTAMP",
            Self::UsageConflictingOptions => "CONFLICTING-OPTIONS",
            Self::PreconditionWorkspaceMissing => "WORKSPACE-MISSING",
            Self::PreconditionWorkspaceAccess => "WORKSPACE-ACCESS",
            Self::ConfigMalformed => "MALFORMED",
            Self::ConfigUnknownField => "UNKNOWN-FIELD",
            Self::ConfigPathNotAbsolute => "PATH-NOT-ABSOLUTE",
            Self::ConfigPathMissing => "PATH-MISSING",
            Self::ConfigPathAlias => "PATH-ALIAS",
            Self::ConfigPathNested => "PATH-NESTED",
            Self::ConfigNotRepository => "NOT-REPOSITORY",
            Self::ConfigProtectedBranch => "PROTECTED-BRANCH",
            Self::ConfigGitVersion => "GIT-VERSION",
            Self::ConfigUnsupportedHost => "UNSUPPORTED-HOST",
            Self::ConfigInvalidValue => "INVALID-VALUE",
            Self::ExternalGitUnavailable => "GIT-UNAVAILABLE",
            Self::ExternalGitCommand => "GIT-COMMAND",
            Self::InternalEncoding => "ENCODING",
        }
    }

    /// The full rendered code, such as `CH-USAGE-INVALID-ID`.
    #[must_use]
    pub fn as_string(self) -> String {
        format!("CH-{}-{}", self.category().name(), self.detail())
    }

    /// Operator guidance for recovering from this failure.
    #[must_use]
    pub const fn recovery(self) -> &'static str {
        match self {
            Self::UsageInvalidId => {
                "Supply an identifier matching its documented prefix and shape."
            }
            Self::UsageInvalidDigest => "Supply a digest of the form sha256:<64 lowercase hex>.",
            Self::UsageInvalidTimestamp => "Supply an RFC 3339 UTC timestamp.",
            Self::UsageConflictingOptions => "Remove one of the conflicting options.",
            Self::PreconditionWorkspaceMissing => "Create the path or pass an existing one.",
            Self::PreconditionWorkspaceAccess => "Check filesystem permissions on the path.",
            Self::ConfigMalformed => "Correct the JSON syntax of the project file.",
            Self::ConfigUnknownField => {
                "Remove the field; the schema rejects fields it does not define."
            }
            Self::ConfigPathNotAbsolute => "Replace the value with an absolute, normalized path.",
            Self::ConfigPathMissing => "Create the path or correct the configured value.",
            Self::ConfigPathAlias => {
                "Give each role a distinct location; they must not resolve to the same path."
            }
            Self::ConfigPathNested => {
                "Move the control or authority repository outside every candidate worktree."
            }
            Self::ConfigNotRepository => {
                "Point the repository field at an existing Git repository."
            }
            Self::ConfigProtectedBranch => "Create the protected branch or correct its name.",
            Self::ConfigGitVersion => "Upgrade Git to at least the configured minimum version.",
            Self::ConfigUnsupportedHost => {
                "Run on a supported host, or add this host to the support list deliberately."
            }
            Self::ConfigInvalidValue => "Correct the reported field to a value the schema accepts.",
            Self::ExternalGitUnavailable => "Install Git and ensure it is on PATH.",
            Self::ExternalGitCommand => "Inspect the reported Git diagnostic and retry.",
            Self::InternalEncoding => {
                "Report this as a defect; it indicates a harness invariant violation."
            }
        }
    }
}

/// Any failure the harness reports.
#[derive(Debug, Error)]
pub enum HarnessError {
    /// An identifier did not match its documented shape.
    #[error("invalid identifier `{value}`: {reason}")]
    InvalidId {
        /// The rejected text.
        value: String,
        /// Why it was rejected.
        reason: String,
    },

    /// A digest did not match its documented shape.
    #[error("invalid digest `{value}`: {reason}")]
    InvalidDigest {
        /// The rejected text.
        value: String,
        /// Why it was rejected.
        reason: String,
    },

    /// A timestamp was outside the supported range.
    #[error("invalid timestamp `{value}`: {source}")]
    InvalidTimestamp {
        /// The rejected text.
        value: String,
        /// The underlying range error.
        #[source]
        source: time::error::ComponentRange,
    },

    /// Two options were combined in an unsupported way.
    #[error("conflicting options: {0}")]
    ConflictingOptions(String),

    /// A configuration field was missing, malformed, or unusable.
    ///
    /// The field name travels with the error so both text and JSON diagnostics
    /// can name the exact offending value rather than reporting that
    /// configuration is invalid.
    #[error("configuration field `{field}`: {reason}")]
    Config {
        /// Dotted path to the offending field.
        field: String,
        /// Why the value was rejected.
        reason: String,
        /// The stable code for this class of configuration failure.
        code: ErrorCode,
    },

    /// The requested workspace path does not exist.
    #[error("workspace does not exist: {0}")]
    WorkspaceNotFound(PathBuf),

    /// The requested workspace path could not be read.
    #[error("cannot access workspace {path}: {source}")]
    WorkspaceAccess {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// Git could not be executed.
    #[error("failed to execute Git: {0}")]
    GitUnavailable(#[source] io::Error),

    /// A Git command failed.
    #[error("Git command failed: {0}")]
    GitCommand(String),

    /// A value could not be encoded for output.
    #[error("failed to encode report: {0}")]
    ReportEncoding(#[from] serde_json::Error),
}

impl HarnessError {
    /// Builds an invalid-identifier error.
    pub fn invalid_id(value: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidId {
            value: value.into(),
            reason: reason.into(),
        }
    }

    /// Builds an invalid-digest error.
    pub fn invalid_digest(value: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidDigest {
            value: value.into(),
            reason: reason.into(),
        }
    }

    /// Builds an invalid-timestamp error.
    pub fn invalid_timestamp(
        value: impl Into<String>,
        source: time::error::ComponentRange,
    ) -> Self {
        Self::InvalidTimestamp {
            value: value.into(),
            source,
        }
    }

    /// The stable code for this failure.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidId { .. } => ErrorCode::UsageInvalidId,
            Self::InvalidDigest { .. } => ErrorCode::UsageInvalidDigest,
            Self::InvalidTimestamp { .. } => ErrorCode::UsageInvalidTimestamp,
            Self::ConflictingOptions(_) => ErrorCode::UsageConflictingOptions,
            Self::Config { code, .. } => *code,
            Self::WorkspaceNotFound(_) => ErrorCode::PreconditionWorkspaceMissing,
            Self::WorkspaceAccess { .. } => ErrorCode::PreconditionWorkspaceAccess,
            Self::GitUnavailable(_) => ErrorCode::ExternalGitUnavailable,
            Self::GitCommand(_) => ErrorCode::ExternalGitCommand,
            Self::ReportEncoding(_) => ErrorCode::InternalEncoding,
        }
    }

    /// The exit category for this failure.
    #[must_use]
    pub const fn category(&self) -> ExitCategory {
        self.code().category()
    }

    /// Structured detail fields for the JSON error envelope.
    #[must_use]
    pub fn details(&self) -> serde_json::Value {
        match self {
            Self::InvalidId { value, reason } | Self::InvalidDigest { value, reason } => {
                serde_json::json!({ "value": value, "reason": reason })
            }
            Self::InvalidTimestamp { value, .. } => serde_json::json!({ "value": value }),
            Self::ConflictingOptions(detail) => serde_json::json!({ "detail": detail }),
            Self::Config { field, reason, .. } => {
                serde_json::json!({ "field": field, "reason": reason })
            }
            Self::WorkspaceNotFound(path) | Self::WorkspaceAccess { path, .. } => {
                serde_json::json!({ "path": path })
            }
            Self::GitUnavailable(_) | Self::GitCommand(_) | Self::ReportEncoding(_) => {
                serde_json::json!({})
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_code_renders_with_its_category_prefix() {
        for code in ErrorCode::ALL {
            let rendered = code.as_string();
            assert!(
                rendered.starts_with(&format!("CH-{}-", code.category().name())),
                "{rendered} does not carry its category"
            );
        }
    }

    #[test]
    fn codes_are_unique() {
        let unique: HashSet<_> = ErrorCode::ALL.iter().map(|code| code.as_string()).collect();
        assert_eq!(unique.len(), ErrorCode::ALL.len());
    }

    #[test]
    fn every_code_offers_recovery_guidance() {
        for code in ErrorCode::ALL {
            assert!(!code.recovery().is_empty(), "{code:?} lacks guidance");
            assert!(
                code.recovery().ends_with('.'),
                "{code:?} guidance should be a sentence"
            );
        }
    }

    #[test]
    fn no_code_maps_to_the_success_category() {
        for code in ErrorCode::ALL {
            assert_ne!(code.category(), ExitCategory::Success, "{code:?}");
        }
    }

    #[test]
    fn errors_report_their_documented_codes_and_categories() {
        let cases: Vec<(HarnessError, ErrorCode, ExitCategory)> = vec![
            (
                HarnessError::invalid_id("x", "bad"),
                ErrorCode::UsageInvalidId,
                ExitCategory::Usage,
            ),
            (
                HarnessError::invalid_digest("x", "bad"),
                ErrorCode::UsageInvalidDigest,
                ExitCategory::Usage,
            ),
            (
                HarnessError::ConflictingOptions("a and b".into()),
                ErrorCode::UsageConflictingOptions,
                ExitCategory::Usage,
            ),
            (
                HarnessError::WorkspaceNotFound(PathBuf::from("/nope")),
                ErrorCode::PreconditionWorkspaceMissing,
                ExitCategory::Precondition,
            ),
            (
                HarnessError::GitCommand("boom".into()),
                ErrorCode::ExternalGitCommand,
                ExitCategory::ExternalTool,
            ),
        ];
        for (error, code, category) in cases {
            assert_eq!(error.code(), code, "{error}");
            assert_eq!(error.category(), category, "{error}");
        }
    }

    #[test]
    fn details_carry_the_rejected_value() {
        let error = HarnessError::invalid_id("F-1", "too short");
        assert_eq!(error.details()["value"], "F-1");
        assert_eq!(error.details()["reason"], "too short");
    }
}
