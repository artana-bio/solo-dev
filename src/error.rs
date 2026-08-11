//! Error types and the machine-readable error-code registry.
//!
//! Every failure carries a stable code that scripts and agents may match on.
//! Codes are append-only: a code's meaning must never change once released.

use std::{io, path::PathBuf};

use thiserror::Error;

use crate::cli::exit::ExitCategory;

const CYCLE_BASELINE_MISMATCH_RECOVERY: &str =
    "Set the card base to the cycle's frozen baseline, or create a new cycle for a different base.";

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
    /// The command line could not be parsed.
    UsageInvalidArguments,
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
    /// The candidate repository is mid-operation or holds uncommitted changes.
    ConfigCandidateUnsettled,
    /// The configured protected branch does not resolve to one commit.
    ConfigProtectedBranch,
    /// The installed Git is older than the configured minimum.
    ConfigGitVersion,
    /// The host operating system is not in the configured support list.
    ConfigUnsupportedHost,
    /// A configured value was structurally valid but not usable.
    ConfigInvalidValue,
    /// A gate definition violates a registry rule.
    ConfigInvalidGate,
    /// A card names a gate the registry does not define.
    ConfigUnknownGate,
    /// An existing control repository does not match the supplied configuration.
    ConfigControlIncompatible,
    /// The authority path is not a usable bare repository.
    ConfigAuthorityIncompatible,
    /// Another process holds the project mutation lock.
    PolicyLockHeld,
    /// A state transition outside the documented state machine was requested.
    PolicyInvalidTransition,
    /// A cycle record is internally inconsistent.
    PolicyInvalidCycle,
    /// A card record is internally inconsistent or violates a card rule.
    PolicyInvalidCard,
    /// A card declares a base other than its cycle's frozen baseline.
    PolicyCycleBaselineMismatch,
    /// Two active cards claim overlapping write scopes.
    PolicyOwnershipOverlap,
    /// Two active cards change the same contract domain.
    PolicyContractOverlap,
    /// Two active cards reserve the same exclusive resource.
    PolicyResourceConflict,
    /// A declared dependency does not exist.
    PolicyDependencyUnsatisfied,
    /// Declared dependencies form a cycle.
    PolicyDependencyCycle,
    /// A card is not in a state that permits integration.
    PolicyNotIntegrable,
    /// An integration would land only part of an atomic group.
    PolicyAtomicGroupSplit,
    /// The cycle already has an outstanding integration.
    PolicyIntegrationOpen,
    /// A final integration was requested before cycle membership was sealed.
    PolicyCycleNotSealed,
    /// A sealed cycle member was neither integrable nor explicitly abandoned.
    PolicyFinalCycleIncomplete,
    /// A decision packet was requested for an ordinary rather than final integration.
    PolicyDecisionPacketFinalOnly,
    /// A reviewed final integration has an unresolved exception decision.
    PolicyExceptionPending,
    /// The project uses a capability no shipped work package implements yet.
    PolicyUnsupportedUntilWp540,
    /// A declared cycle invariant was neither confirmed nor refused.
    PolicyInvariantUnaddressed,
    /// One actor attempted two roles that must be held by different actors.
    PolicySameActor,
    /// Promotion was attempted without an acceptance that authorizes it.
    PolicyNotAccepted,
    /// The landing commit was not covered by the recorded verification.
    PolicyNotVerified,
    /// The landing commit does not have the shape promotion requires.
    PolicyLandingMismatch,
    /// An archived ref no longer resolves to the commit it recorded.
    PolicyArchiveBroken,
    /// The named record does not exist.
    PreconditionNotFound,
    /// A card's declared base commit does not exist in the repository.
    PreconditionBaseMissing,
    /// The target branch already exists.
    PreconditionBranchExists,
    /// The target worktree path already exists.
    PreconditionWorktreeExists,
    /// A worktree holds uncommitted work and cannot be removed.
    PreconditionWorktreeDirty,
    /// A branch holds commits that are not merged elsewhere.
    PreconditionUnmergedWork,
    /// The local protected worktree is not at the commit promotion expects.
    PreconditionLocalMainStale,
    /// Git refused to begin a merge, so no content was ever combined.
    ///
    /// Distinct from [`Self::ConflictMergeFailed`] on purpose. A conflict means
    /// two changes disagree and the answer is a fix card; this means Git never
    /// got as far as comparing them, and the answer is somewhere in the
    /// worktree or the repository's configuration. Reporting the second as the
    /// first sent operators to `integration preflight`, which reports clean
    /// because in-memory merging is unaffected by whatever stopped this one.
    PreconditionMergeRefused,
    /// A path initialization would adopt already holds content nobody checked.
    PreconditionOccupiedPath,
    /// The control repository could not be read or written by this process.
    PreconditionControlAccess,
    /// The card's risk level requires a human reviewer and none was declared.
    PolicyRiskReview,
    /// A card already holds an active lease.
    PolicyLeaseHeld,
    /// The project lock was left behind by a process that has exited.
    PolicyStaleLock,
    /// The project lock exists but its disposition cannot be established.
    PolicyLockAmbiguous,
    /// A backup destination shares a device with what it protects.
    PolicyBackupNotIndependent,
    /// A backup failed verification.
    PolicyBackupCorrupt,
    /// An audit found the record disagreeing with the objects it names.
    PolicyAuditDiscrepancy,
    /// A generated-artifact declaration is invalid or contradicts another.
    PolicyInvalidArtifact,
    /// A candidate committed a path its class forbids it from committing.
    PolicyArtifactNotOwned,
    /// A worktree locator disagrees with authoritative control state.
    PolicyLocatorMismatch,
    /// A candidate changed paths outside its card's declared scope.
    PolicyCandidateOutOfScope,
    /// A staged control entry is not valid control-record text.
    PolicyControlEncoding,
    /// A staged JSON control entry could not be completely inspected.
    PolicyControlJsonInspection,
    /// A staged control entry contains the governed sensitive-value shape.
    PolicySensitiveValue,
    /// The gate runner could not execute or supervise a process.
    GateRunnerError,
    /// A named gate failed, timed out, or was signalled.
    GateFailed,
    /// Required gate evidence is missing or no longer applies.
    GateEvidenceStale,
    /// A handoff declaration is missing a required field, or names one a
    /// value it cannot hold.
    ///
    /// The second half covers a declared gate failure: a `gate_id` that is
    /// not among the card's feature gates, a `reason_category`
    /// `AttemptKind::GateFailure` does not admit, or the same gate declared
    /// twice. Grouped with the missing-field case rather than given a new
    /// code, for the same reason `PolicyIncompleteReview` groups its own
    /// second half: both are a declaration that cannot be acted on until the
    /// actor supplies something the schema already had a place for.
    PolicyIncompleteHandoff,
    /// The branch was rewritten between delivery and handoff creation.
    PolicyDeliveredShaMismatch,
    /// The reviewer is the feature actor.
    PolicySelfReview,
    /// A review approved while findings remain open.
    PolicyOpenFindings,
    /// A review is missing a required field, or names one a value it cannot
    /// hold.
    ///
    /// The second half covers a declared `reason_category` an attempt kind
    /// does not admit — `scope_change` on a `changes_requested` verdict, for
    /// instance, which is a material scope revision wearing a review return's
    /// clothes. Grouped with the missing-field case rather than given a new
    /// code: both are a review that cannot be acted on until the reviewer
    /// supplies something the schema already had a place for.
    PolicyIncompleteReview,
    /// The handoff under review no longer describes the branch.
    PolicyStaleHandoff,
    /// A verdict was recorded against a handoff no review round ever opened.
    PolicyReviewNotBegun,
    /// A card's registered convergence budget is spent, and no authorized
    /// disposition has released it.
    PolicyConvergenceEscalated,
    /// A lesson record is malformed or violates the lesson schema.
    PolicyLessonInvalid,
    /// A required lesson has no matching gate or review evidence.
    PolicyLessonEvidenceMissing,
    /// A packet or record is bound to a lesson manifest that no longer applies.
    PolicyLessonManifestStale,
    /// A lesson proposal or retirement lacks an authorized disposition.
    PolicyLessonDisposition,
    /// A previous mutation did not complete and must be recovered first.
    RecoveryIncomplete,
    /// Control state is internally inconsistent.
    InternalControlCorrupt,
    /// The control repository moved under a command that expected a fixed head.
    ConflictControlHeadMoved,
    /// A candidate could not be merged into the integration.
    ConflictMergeFailed,
    /// Git could not be executed.
    ExternalGitUnavailable,
    /// A Git command failed.
    ExternalGitCommand,
    /// A value could not be encoded for output.
    InternalEncoding,
}

/// Classifies a filesystem failure as the environment's or the harness's.
///
/// Every `ControlIo` failure used to become `InternalControlCorrupt` — exit 10,
/// the category reserved for "the harness is broken, file a bug". A read-only
/// filesystem, a permissions mistake, a full disk: all environment problems the
/// operator can fix, all reported as harness defects, which sends them to
/// exactly the wrong place and makes exit 10 mean nothing.
///
/// Only the unambiguous cases are reclassified. `NotFound` stays internal on
/// purpose: a control file missing when the state says it exists is corruption,
/// and there is no way here to tell that from someone deleting the directory.
fn classify_io(source: &std::io::Error) -> ErrorCode {
    match source.kind() {
        std::io::ErrorKind::PermissionDenied
        | std::io::ErrorKind::ReadOnlyFilesystem
        | std::io::ErrorKind::StorageFull
        | std::io::ErrorKind::QuotaExceeded => ErrorCode::PreconditionControlAccess,
        _ => ErrorCode::InternalControlCorrupt,
    }
}

impl ErrorCode {
    /// Every registered code, for exhaustive testing and documentation.
    pub const ALL: [Self; 93] = [
        Self::UsageInvalidId,
        Self::UsageInvalidDigest,
        Self::UsageInvalidTimestamp,
        Self::UsageConflictingOptions,
        Self::UsageInvalidArguments,
        Self::PreconditionWorkspaceMissing,
        Self::PreconditionWorkspaceAccess,
        Self::ConfigMalformed,
        Self::ConfigUnknownField,
        Self::ConfigPathNotAbsolute,
        Self::ConfigPathMissing,
        Self::ConfigPathAlias,
        Self::ConfigPathNested,
        Self::ConfigNotRepository,
        Self::ConfigCandidateUnsettled,
        Self::ConfigProtectedBranch,
        Self::ConfigGitVersion,
        Self::ConfigUnsupportedHost,
        Self::ConfigInvalidValue,
        Self::ConfigInvalidGate,
        Self::ConfigUnknownGate,
        Self::ConfigControlIncompatible,
        Self::ConfigAuthorityIncompatible,
        Self::PolicyLockHeld,
        Self::PolicyInvalidTransition,
        Self::PolicyInvalidCycle,
        Self::PolicyInvalidCard,
        Self::PolicyCycleBaselineMismatch,
        Self::PolicyOwnershipOverlap,
        Self::PolicyContractOverlap,
        Self::PolicyResourceConflict,
        Self::PolicyDependencyUnsatisfied,
        Self::PolicyDependencyCycle,
        Self::PolicyNotIntegrable,
        Self::PolicyAtomicGroupSplit,
        Self::PolicyIntegrationOpen,
        Self::PolicyCycleNotSealed,
        Self::PolicyFinalCycleIncomplete,
        Self::PolicyDecisionPacketFinalOnly,
        Self::PolicyExceptionPending,
        Self::PolicyUnsupportedUntilWp540,
        Self::PolicyInvariantUnaddressed,
        Self::PolicySameActor,
        Self::PolicyNotAccepted,
        Self::PolicyNotVerified,
        Self::PolicyLandingMismatch,
        Self::PolicyArchiveBroken,
        Self::PreconditionNotFound,
        Self::PreconditionBaseMissing,
        Self::PreconditionBranchExists,
        Self::PreconditionWorktreeExists,
        Self::PreconditionWorktreeDirty,
        Self::PreconditionUnmergedWork,
        Self::PreconditionLocalMainStale,
        Self::PreconditionMergeRefused,
        Self::PreconditionOccupiedPath,
        Self::PreconditionControlAccess,
        Self::PolicyRiskReview,
        Self::PolicyLeaseHeld,
        Self::PolicyStaleLock,
        Self::PolicyLockAmbiguous,
        Self::PolicyBackupNotIndependent,
        Self::PolicyBackupCorrupt,
        Self::PolicyAuditDiscrepancy,
        Self::PolicyInvalidArtifact,
        Self::PolicyArtifactNotOwned,
        Self::PolicyLocatorMismatch,
        Self::PolicyCandidateOutOfScope,
        Self::PolicyControlEncoding,
        Self::PolicyControlJsonInspection,
        Self::PolicySensitiveValue,
        Self::GateRunnerError,
        Self::GateFailed,
        Self::GateEvidenceStale,
        Self::PolicyIncompleteHandoff,
        Self::PolicyDeliveredShaMismatch,
        Self::PolicySelfReview,
        Self::PolicyOpenFindings,
        Self::PolicyIncompleteReview,
        Self::PolicyStaleHandoff,
        Self::PolicyReviewNotBegun,
        Self::PolicyConvergenceEscalated,
        Self::PolicyLessonInvalid,
        Self::PolicyLessonEvidenceMissing,
        Self::PolicyLessonManifestStale,
        Self::PolicyLessonDisposition,
        Self::RecoveryIncomplete,
        Self::InternalControlCorrupt,
        Self::ConflictControlHeadMoved,
        Self::ConflictMergeFailed,
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
            | Self::UsageConflictingOptions
            | Self::UsageInvalidArguments => ExitCategory::Usage,
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
            | Self::ConfigCandidateUnsettled
            | Self::ConfigProtectedBranch
            | Self::ConfigGitVersion
            | Self::ConfigUnsupportedHost
            | Self::ConfigInvalidValue
            | Self::ConfigInvalidGate
            | Self::ConfigUnknownGate
            | Self::ConfigControlIncompatible
            | Self::ConfigAuthorityIncompatible => ExitCategory::Configuration,
            Self::PolicyLockHeld
            | Self::PolicyInvalidTransition
            | Self::PolicyInvalidCycle
            | Self::PolicyInvalidCard
            | Self::PolicyCycleBaselineMismatch
            | Self::PolicyOwnershipOverlap
            | Self::PolicyContractOverlap
            | Self::PolicyResourceConflict
            | Self::PolicyDependencyUnsatisfied
            | Self::PolicyDependencyCycle
            | Self::PolicyNotIntegrable
            | Self::PolicyAtomicGroupSplit
            | Self::PolicyIntegrationOpen
            | Self::PolicyCycleNotSealed
            | Self::PolicyFinalCycleIncomplete
            | Self::PolicyDecisionPacketFinalOnly
            | Self::PolicyExceptionPending
            | Self::PolicyUnsupportedUntilWp540
            | Self::PolicyInvariantUnaddressed
            | Self::PolicySameActor
            | Self::PolicyNotAccepted
            | Self::PolicyNotVerified
            | Self::PolicyLandingMismatch
            | Self::PolicyArchiveBroken
            | Self::PolicyLeaseHeld
            | Self::PolicyStaleLock
            | Self::PolicyLockAmbiguous
            | Self::PolicyBackupNotIndependent
            | Self::PolicyBackupCorrupt
            | Self::PolicyAuditDiscrepancy
            | Self::PolicyInvalidArtifact
            | Self::PolicyArtifactNotOwned
            | Self::PolicyLocatorMismatch
            | Self::PolicyCandidateOutOfScope
            | Self::PolicyControlEncoding
            | Self::PolicyControlJsonInspection
            | Self::PolicySensitiveValue
            | Self::PolicyIncompleteHandoff
            | Self::PolicyDeliveredShaMismatch
            | Self::PolicySelfReview
            | Self::PolicyRiskReview
            | Self::PolicyOpenFindings
            | Self::PolicyIncompleteReview
            | Self::PolicyStaleHandoff
            | Self::PolicyReviewNotBegun
            | Self::PolicyConvergenceEscalated
            | Self::PolicyLessonInvalid
            | Self::PolicyLessonEvidenceMissing
            | Self::PolicyLessonManifestStale
            | Self::PolicyLessonDisposition => ExitCategory::Policy,
            Self::GateRunnerError | Self::GateFailed | Self::GateEvidenceStale => {
                ExitCategory::Gate
            }
            Self::PreconditionNotFound
            | Self::PreconditionBaseMissing
            | Self::PreconditionBranchExists
            | Self::PreconditionWorktreeExists
            | Self::PreconditionOccupiedPath
            | Self::PreconditionControlAccess
            | Self::PreconditionWorktreeDirty
            | Self::PreconditionUnmergedWork
            | Self::PreconditionLocalMainStale
            | Self::PreconditionMergeRefused => ExitCategory::Precondition,
            Self::RecoveryIncomplete => ExitCategory::RecoveryRequired,
            Self::ConflictControlHeadMoved | Self::ConflictMergeFailed => ExitCategory::Conflict,
            Self::ExternalGitUnavailable | Self::ExternalGitCommand => ExitCategory::ExternalTool,
            Self::InternalControlCorrupt | Self::InternalEncoding => ExitCategory::Internal,
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
            Self::UsageInvalidArguments => "INVALID-ARGUMENTS",
            Self::PreconditionWorkspaceMissing => "WORKSPACE-MISSING",
            Self::PreconditionWorkspaceAccess => "WORKSPACE-ACCESS",
            Self::ConfigMalformed => "MALFORMED",
            Self::ConfigUnknownField => "UNKNOWN-FIELD",
            Self::ConfigPathNotAbsolute => "PATH-NOT-ABSOLUTE",
            Self::ConfigPathMissing => "PATH-MISSING",
            Self::ConfigPathAlias => "PATH-ALIAS",
            Self::ConfigPathNested => "PATH-NESTED",
            Self::ConfigNotRepository => "NOT-REPOSITORY",
            Self::ConfigCandidateUnsettled => "CANDIDATE-UNSETTLED",
            Self::ConfigProtectedBranch => "PROTECTED-BRANCH",
            Self::ConfigGitVersion => "GIT-VERSION",
            Self::ConfigUnsupportedHost => "UNSUPPORTED-HOST",
            Self::ConfigInvalidValue => "INVALID-VALUE",
            Self::ConfigInvalidGate => "INVALID-GATE",
            Self::ConfigUnknownGate => "UNKNOWN-GATE",
            Self::ConfigControlIncompatible => "CONTROL-INCOMPATIBLE",
            Self::ConfigAuthorityIncompatible => "AUTHORITY-INCOMPATIBLE",
            Self::PolicyLockHeld => "LOCK-HELD",
            Self::PolicyInvalidTransition => "INVALID-TRANSITION",
            Self::PolicyInvalidCycle => "INVALID-CYCLE",
            Self::PolicyInvalidCard => "INVALID-CARD",
            Self::PolicyCycleBaselineMismatch => "CYCLE-BASELINE-MISMATCH",
            Self::PolicyOwnershipOverlap => "OWNERSHIP-OVERLAP",
            Self::PolicyContractOverlap => "CONTRACT-OVERLAP",
            Self::PolicyResourceConflict => "RESOURCE-CONFLICT",
            Self::PolicyDependencyUnsatisfied => "DEPENDENCY-UNSATISFIED",
            Self::PolicyDependencyCycle => "DEPENDENCY-CYCLE",
            Self::PolicyNotIntegrable => "NOT-INTEGRABLE",
            Self::PolicyAtomicGroupSplit => "ATOMIC-GROUP-SPLIT",
            Self::PolicyIntegrationOpen => "INTEGRATION-OPEN",
            Self::PolicyCycleNotSealed => "CYCLE-NOT-SEALED",
            Self::PolicyFinalCycleIncomplete => "FINAL-CYCLE-INCOMPLETE",
            Self::PolicyDecisionPacketFinalOnly => "DECISION-PACKET-FINAL-ONLY",
            Self::PolicyExceptionPending => "EXCEPTION-PENDING",
            Self::PolicyUnsupportedUntilWp540 => "UNSUPPORTED-UNTIL-WP-540",
            Self::PolicyInvariantUnaddressed => "INVARIANT-UNADDRESSED",
            Self::PolicySameActor => "SAME-ACTOR",
            Self::PolicyNotAccepted => "NOT-ACCEPTED",
            Self::PolicyNotVerified => "NOT-VERIFIED",
            Self::PolicyLandingMismatch => "LANDING-MISMATCH",
            Self::PolicyArchiveBroken => "ARCHIVE-BROKEN",
            Self::PreconditionNotFound => "NOT-FOUND",
            Self::PreconditionBaseMissing => "BASE-MISSING",
            Self::PreconditionBranchExists => "BRANCH-EXISTS",
            Self::PreconditionWorktreeExists => "WORKTREE-EXISTS",
            Self::PreconditionOccupiedPath => "OCCUPIED-PATH",
            Self::PreconditionControlAccess => "CONTROL-ACCESS",
            Self::PreconditionWorktreeDirty => "WORKTREE-DIRTY",
            Self::PreconditionUnmergedWork => "UNMERGED-WORK",
            Self::PreconditionLocalMainStale => "LOCAL-MAIN-STALE",
            Self::PreconditionMergeRefused => "MERGE-REFUSED",
            Self::PolicyLeaseHeld => "LEASE-HELD",
            Self::PolicyStaleLock => "STALE-LOCK",
            Self::PolicyLockAmbiguous => "LOCK-AMBIGUOUS",
            Self::PolicyBackupNotIndependent => "BACKUP-NOT-INDEPENDENT",
            Self::PolicyBackupCorrupt => "BACKUP-CORRUPT",
            Self::PolicyAuditDiscrepancy => "AUDIT-DISCREPANCY",
            Self::PolicyInvalidArtifact => "INVALID-ARTIFACT",
            Self::PolicyArtifactNotOwned => "ARTIFACT-NOT-OWNED",
            Self::PolicyLocatorMismatch => "LOCATOR-MISMATCH",
            Self::PolicyCandidateOutOfScope => "CANDIDATE-OUT-OF-SCOPE",
            Self::PolicyControlEncoding => "CONTROL-ENCODING",
            Self::PolicyControlJsonInspection => "CONTROL-JSON-INSPECTION",
            Self::PolicySensitiveValue => "SENSITIVE-VALUE",
            Self::GateRunnerError => "RUNNER-ERROR",
            Self::GateFailed => "FAILED",
            Self::GateEvidenceStale => "EVIDENCE-STALE",
            Self::PolicyIncompleteHandoff => "INCOMPLETE-HANDOFF",
            Self::PolicyDeliveredShaMismatch => "DELIVERED-SHA-MISMATCH",
            Self::PolicySelfReview => "SELF-REVIEW",
            Self::PolicyRiskReview => "RISK-REVIEW",
            Self::PolicyOpenFindings => "OPEN-FINDINGS",
            Self::PolicyIncompleteReview => "INCOMPLETE-REVIEW",
            Self::PolicyStaleHandoff => "STALE-HANDOFF",
            Self::PolicyReviewNotBegun => "REVIEW-NOT-BEGUN",
            Self::PolicyConvergenceEscalated => "CONVERGENCE-ESCALATED",
            Self::PolicyLessonInvalid => "LESSON-INVALID",
            Self::PolicyLessonEvidenceMissing => "LESSON-EVIDENCE-MISSING",
            Self::PolicyLessonManifestStale => "LESSON-MANIFEST-STALE",
            Self::PolicyLessonDisposition => "LESSON-DISPOSITION",
            Self::RecoveryIncomplete => "INCOMPLETE-OPERATION",
            Self::InternalControlCorrupt => "CONTROL-CORRUPT",
            Self::ConflictControlHeadMoved => "CONTROL-HEAD-MOVED",
            Self::ConflictMergeFailed => "MERGE-FAILED",
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
    ///
    /// Split by category rather than kept as one match: the registry grows with
    /// every work package, and a single arm list would keep outgrowing any
    /// reasonable function length.
    #[must_use]
    pub const fn recovery(self) -> &'static str {
        if matches!(
            self,
            Self::PolicyControlEncoding
                | Self::PolicyControlJsonInspection
                | Self::PolicySensitiveValue
        ) {
            return self.record_recovery();
        }
        if matches!(self, Self::PolicyConvergenceEscalated) {
            return Self::convergence_recovery();
        }
        match self.category() {
            ExitCategory::Usage => self.usage_recovery(),
            ExitCategory::Configuration => self.config_recovery(),
            ExitCategory::Policy => self.policy_recovery(),
            ExitCategory::Precondition => self.precondition_recovery(),
            ExitCategory::Gate => self.gate_recovery(),
            _ => self.other_recovery(),
        }
    }

    /// Guidance for usage failures.
    const fn usage_recovery(self) -> &'static str {
        match self {
            Self::UsageInvalidId => {
                "Supply an identifier matching its documented prefix and shape."
            }
            Self::UsageInvalidDigest => "Supply a digest of the form sha256:<64 lowercase hex>.",
            Self::UsageInvalidTimestamp => "Supply an RFC 3339 UTC timestamp.",
            Self::UsageConflictingOptions => "Remove one of the conflicting options.",
            Self::UsageInvalidArguments => {
                "Correct the command line; run the command with `--help` for its arguments."
            }
            _ => "Correct the command invocation.",
        }
    }

    /// Guidance for configuration failures.
    const fn config_recovery(self) -> &'static str {
        match self {
            Self::ConfigMalformed => "Correct the JSON or YAML syntax of the document.",
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
            Self::ConfigInvalidGate => {
                "Correct the gate definition; argv is an array and commands are never shell strings."
            }
            Self::ConfigUnknownGate => {
                "Register the gate before naming it in a card, or correct the gate name."
            }
            // Deliberately points at this error's own message instead of
            // naming a remedy, for the same reason
            // `PreconditionMergeRefused` does. The ways control state can
            // disagree with what a command is installing form an open set —
            // no control repository at the path, one already bound to a
            // different configuration, convergence facts recorded under a
            // policy digest the install would orphan — and each needs a
            // different thing done about it. The previous text
            // ("Point at the matching control repository, or initialize a
            // new project elsewhere") described the first two and so sent
            // the operator to the wrong place for the third, which is
            // resolved by `disposition rebaseline` and not by changing which
            // repository they point at. Every site's own message names what
            // mismatched and what to do about it; the guidance this text
            // used to carry for `reinitialize` now lives in that site's
            // message, which has the context to state it.
            Self::ConfigControlIncompatible => {
                "Read the mismatch this error reports: it names what the control repository already holds and what this command would install over it."
            }
            Self::ConfigAuthorityIncompatible => {
                "Point at an empty directory or an existing bare repository; an authority must have no working tree."
            }
            _ => "Correct the reported field to a value the schema accepts.",
        }
    }

    /// Guidance for policy failures.
    /// Guidance for the operational-hardening policy codes.
    ///
    /// Split out because `policy_recovery` outgrew its line budget as the
    /// hardening packages landed, the same way `recovery` was split earlier.
    const fn hardening_recovery(self) -> &'static str {
        match self {
            Self::PolicyBackupNotIndependent => {
                "Choose a destination on a different device; a copy beside the original protects against almost nothing."
            }
            Self::PolicyBackupCorrupt => {
                "Recreate the backup and verify it again; the stored copy cannot be trusted."
            }
            Self::PolicyAuditDiscrepancy => {
                "Read the reported discrepancies; each names what a record claims and what was found."
            }
            Self::PolicyInvalidArtifact => {
                "Give each generated path exactly one class, with what that class requires."
            }
            Self::PolicyArtifactNotOwned => {
                "Remove the path from the candidate; its class gives ownership to integration or to nobody."
            }
            _ => "Report this as a defect; a hardening code reached the wrong recovery table.",
        }
    }

    const fn record_recovery(self) -> &'static str {
        match self {
            Self::PolicyControlEncoding => {
                "Rewrite the named control entry as UTF-8 text without embedded NUL bytes, then retry."
            }
            Self::PolicyControlJsonInspection => {
                "Rewrite the named control entry as complete JSON no larger than 4 MiB, then retry."
            }
            Self::PolicySensitiveValue => {
                "Remove the sensitive value from the named control entry, then retry."
            }
            _ => "Report this as a defect; a record-hygiene code reached the wrong recovery table.",
        }
    }

    /// Guidance for a card whose convergence budget is spent.
    ///
    /// Split out for the same reason [`Self::record_recovery`] is: it is
    /// dispatched to directly from `recovery`, ahead of `policy_recovery`'s
    /// per-category match, so its longer remedy text does not push that
    /// function over its line budget the way it did before this split.
    ///
    /// Names every flag `disposition renew` requires, not just the
    /// command. Naming only the command left an operator one lookup short
    /// of a working invocation — they still had to go find out it needs
    /// `--card-id`, `--dimension`, and `--rationale` — the same "go read
    /// something else first" shape the original defect had, just smaller.
    ///
    /// The three flags are named as bare, individually backticked mentions
    /// rather than folded into one fully worked invocation with example
    /// values. `--dimension` takes a closed `clap` value enum, so writing
    /// a placeholder in its place, e.g. `` `--dimension <dimension>` ``,
    /// fails `tests/recovery_text.rs`: that test appends `--help` to every
    /// checked span, but `clap` rejects an invalid enum value while
    /// parsing the span itself, before parsing ever reaches the appended
    /// `--help`. A bare flag mention is exactly the shape that test's own
    /// `names_a_command` rule already excludes from the check — the same
    /// shape it documents for `` `--invariant-holds` `` — which is what
    /// makes it safe here.
    const fn convergence_recovery() -> &'static str {
        "This card's convergence budget is spent; it requires an authorized disposition before it can be delivered or reviewed again. Run `disposition renew` with `--card-id`, `--dimension`, and `--rationale` to grant the exhausted dimension its configured limit again, or record a different disposition if renewal is not the right response."
    }

    #[allow(clippy::too_many_lines)]
    const fn policy_recovery(self) -> &'static str {
        match self {
            Self::PolicyBackupNotIndependent
            | Self::PolicyBackupCorrupt
            | Self::PolicyAuditDiscrepancy
            | Self::PolicyInvalidArtifact
            | Self::PolicyArtifactNotOwned => Self::hardening_recovery(self),
            Self::PolicyIntegrationOpen
            | Self::PolicyCycleNotSealed
            | Self::PolicyFinalCycleIncomplete
            | Self::PolicyDecisionPacketFinalOnly => Self::integration_recovery(self),
            Self::PolicyLockHeld => "Wait for the other command to finish, then retry.",
            Self::PolicyInvalidTransition => {
                "Move through the documented states in order, or abandon the subject."
            }
            Self::PolicyInvalidCycle => {
                "Correct the cycle record so its membership and baseline are coherent."
            }
            Self::PolicyInvalidCard => {
                "Correct the card as this error states; use `card revise` if the card is already activated."
            }
            Self::PolicyLessonInvalid => {
                "Correct the lesson definition; it must name explicit selectors, provenance, and valid obligations."
            }
            Self::PolicyLessonEvidenceMissing => {
                "Read the generated lesson packet, add the required gate or review disposition, then recreate the handoff."
            }
            Self::PolicyLessonManifestStale => {
                "Regenerate the packet for the current card and active lesson revisions before continuing."
            }
            Self::PolicyLessonDisposition => {
                "Have an authorized operator activate, retire, or supersede the lesson before relying on it."
            }
            Self::PolicyCycleBaselineMismatch => CYCLE_BASELINE_MISMATCH_RECOVERY,
            Self::PolicyOwnershipOverlap => {
                "Narrow one card's write scope, or serialize the two cards."
            }
            Self::PolicyContractOverlap => {
                "Serialize the cards, or split the contract change into one owning card."
            }
            Self::PolicyResourceConflict => {
                "Assign a distinct resource, or wait for the holding card to close."
            }
            Self::PolicyDependencyUnsatisfied => {
                "Declare the dependency in this cycle, or remove the reference."
            }
            Self::PolicyDependencyCycle => {
                "Break the reported cycle by removing one dependency edge."
            }
            Self::PolicyNotIntegrable => {
                "Run `integration ready` to see why, then re-review or re-hand-off the card."
            }
            Self::PolicyAtomicGroupSplit => "Select the whole atomic group, or none of it.",
            Self::PolicyUnsupportedUntilWp540 => {
                "Remove the shared generated artifacts from the cards, or wait for WP-540."
            }
            Self::PolicyInvariantUnaddressed => {
                "Confirm each declared cycle invariant with `--invariant-holds`, or block the integration."
            }
            Self::PolicySameActor => {
                "Have a different actor perform this step; the two roles must be held separately."
            }
            Self::PolicyNotAccepted => {
                "Record an acceptance for this exact landing commit before promoting."
            }
            Self::PolicyNotVerified => {
                "Re-run `integration verify`; the recorded verification covers a different commit."
            }
            Self::PolicyLandingMismatch => {
                "Rebuild the landing commit; it no longer matches the plan it was built from."
            }
            Self::PolicyArchiveBroken => {
                "Restore or recreate the archived refs with `archive create` before cleaning up."
            }
            Self::PolicyLeaseHeld => "Resume the existing lease, or release it explicitly.",
            Self::PolicyStaleLock => {
                "Run `project recover --resume` to clear the lock its dead holder left behind."
            }
            Self::PolicyLockAmbiguous => {
                "Confirm no harness command is running, then remove the lock file deliberately."
            }
            Self::PolicyLocatorMismatch => {
                "The worktree link disagrees with control state; re-allocate rather than trusting it."
            }
            Self::PolicyCandidateOutOfScope => {
                "Revert the out-of-scope changes, or revise the card to declare them."
            }
            Self::PolicyIncompleteHandoff => {
                "Supply every declaration field; an empty list is a claim, an absent one is not."
            }
            Self::PolicyDeliveredShaMismatch => {
                "The branch moved after delivery. Re-verify the candidate and hand off the commit that is actually there."
            }
            Self::PolicyRiskReview => {
                "Have a human review this card and record `human_reviewer: true` on the verdict, or lower the card's risk if that was set wrongly."
            }
            Self::PolicySelfReview => {
                "Have a different actor review this candidate in a fresh context."
            }
            Self::PolicyOpenFindings => {
                "Disposition each finding as resolved, accepted risk, or out of scope before approving."
            }
            Self::PolicyIncompleteReview => {
                "Supply the missing review field; a review that omits it cannot be acted on."
            }
            Self::PolicyStaleHandoff => {
                "Create a fresh handoff for the current candidate before reviewing it."
            }
            Self::PolicyReviewNotBegun => {
                "Run `review begin` for this handoff before recording a verdict against it."
            }
            _ => "Resolve the reported policy violation.",
        }
    }

    const fn integration_recovery(self) -> &'static str {
        match self {
            Self::PolicyIntegrationOpen => {
                "Promote, archive, or abandon the open integration before preparing another."
            }
            Self::PolicyCycleNotSealed => "Seal the cycle before preparing its final integration.",
            Self::PolicyFinalCycleIncomplete => {
                "Approve the named card with current evidence, or abandon it explicitly before preparing the final integration."
            }
            Self::PolicyDecisionPacketFinalOnly => {
                "Prepare the sealed cycle with `integration prepare --final` before requesting a decision packet."
            }
            _ => "Report this as a defect; an integration code reached the wrong recovery table.",
        }
    }

    /// Guidance for precondition failures.
    const fn precondition_recovery(self) -> &'static str {
        match self {
            Self::PreconditionWorkspaceMissing => "Create the path or pass an existing one.",
            Self::PreconditionWorkspaceAccess => "Check filesystem permissions on the path.",
            Self::PreconditionNotFound => "Create the named record, or correct the identifier.",
            Self::PreconditionBaseMissing => {
                "Fetch the base commit into this repository, or revise the card's base."
            }
            Self::PreconditionBranchExists => {
                "Remove or rename the existing branch, or resume the existing allocation."
            }
            Self::PreconditionWorktreeExists => {
                "Remove the existing directory, or resume the existing allocation."
            }
            Self::PreconditionWorktreeDirty => {
                "Commit or discard the uncommitted work, then retry."
            }
            // Deliberately points at Git's own words rather than naming a
            // cause. The set of things that stop a merge before it starts is
            // open — untracked files a gate left behind, unrelated histories,
            // a merge driver that will not run — and a remediation that names
            // one of them sends the operator to the wrong place for the rest.
            // Naming `integration preflight` would be worse still: it merges
            // in memory and reports clean while this fails.
            Self::PreconditionMergeRefused => {
                "Read the Git diagnostic in this error: the merge was refused before any content was combined, so this is the integration worktree's state or the repository's configuration, not a conflict."
            }
            Self::PreconditionUnmergedWork => {
                "Land or archive the branch's commits before deleting it."
            }
            Self::PreconditionOccupiedPath => {
                "Point the flag at an empty or new directory, or move the existing contents aside."
            }
            Self::PreconditionControlAccess => {
                "Check permissions and free space on the control repository, then retry."
            }
            _ => "Satisfy the reported precondition, then retry.",
        }
    }

    /// Guidance for gate failures.
    const fn gate_recovery(self) -> &'static str {
        match self {
            Self::GateRunnerError => {
                "Check the gate's executable exists and is runnable in the evaluation worktree."
            }
            Self::GateFailed => "Fix the failure the gate reported, then rerun it.",
            Self::GateEvidenceStale => {
                "Rerun the gate against the current candidate; the recorded run no longer applies."
            }
            _ => "Resolve the reported gate failure.",
        }
    }

    /// Guidance for the remaining categories.
    const fn other_recovery(self) -> &'static str {
        match self {
            Self::RecoveryIncomplete => {
                "Run `project recover` to resume or diagnose the interrupted operation."
            }
            Self::ConflictControlHeadMoved => {
                "Reload the project and retry; another writer advanced control state."
            }
            Self::ConflictMergeFailed => {
                "Run `integration preflight` for the conflict detail, then resolve it in an integration fix card."
            }
            Self::ExternalGitUnavailable => "Install Git and ensure it is on PATH.",
            Self::ExternalGitCommand => "Inspect the reported Git diagnostic and retry.",
            Self::InternalControlCorrupt => {
                "Inspect the control repository history; this indicates a harness defect or external edit."
            }
            _ => "Report this as a defect; it indicates a harness invariant violation.",
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

    /// A control-state operation failed for a reason with a stable code.
    #[error("control state: {reason}")]
    Control {
        /// What went wrong.
        reason: String,
        /// The stable code for this class of failure.
        code: ErrorCode,
    },

    /// A control-state operation failed for a reason with a stable code,
    /// where this exact site already knows a better fix than the code's
    /// shared table entry.
    ///
    /// A sibling of [`Self::Control`] rather than a new field on it:
    /// `Control` has hundreds of construction sites that supply no
    /// override, and adding a field there would force every one of them to
    /// spell out a `None` for no behavior change. This variant exists so a
    /// refusal that already knows its own fix can say so without disturbing
    /// any of them. See #106.
    #[error("control state: {reason}")]
    ControlWithRecovery {
        /// What went wrong.
        reason: String,
        /// The stable code for this class of failure.
        code: ErrorCode,
        /// This site's own recovery guidance, used in place of the code's
        /// default table entry.
        recovery: &'static str,
    },

    /// A control-state failure carrying a structured report for automation.
    #[error("control state: {reason}")]
    ControlWithDetails {
        /// What went wrong.
        reason: String,
        /// The stable code for this class of failure.
        code: ErrorCode,
        /// Structured evidence produced before the command failed closed.
        details: serde_json::Value,
    },

    /// A pure policy refusal discovered after a transaction journal opened.
    /// The transaction must discard its provisional journal rather than
    /// turning a TOCTOU policy rejection into durable operation history.
    #[error("{0}")]
    NoPersist(#[source] Box<HarnessError>),

    /// A control-state file could not be read or written.
    #[error("cannot access control state at {path}: {source}")]
    ControlIo {
        /// The path that could not be accessed.
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
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidId { .. } => ErrorCode::UsageInvalidId,
            Self::InvalidDigest { .. } => ErrorCode::UsageInvalidDigest,
            Self::InvalidTimestamp { .. } => ErrorCode::UsageInvalidTimestamp,
            Self::ConflictingOptions(_) => ErrorCode::UsageConflictingOptions,
            Self::Config { code, .. }
            | Self::Control { code, .. }
            | Self::ControlWithRecovery { code, .. }
            | Self::ControlWithDetails { code, .. } => *code,
            Self::NoPersist(error) => error.code(),
            Self::ControlIo { source, .. } => classify_io(source),
            Self::WorkspaceNotFound(_) => ErrorCode::PreconditionWorkspaceMissing,
            Self::WorkspaceAccess { .. } => ErrorCode::PreconditionWorkspaceAccess,
            Self::GitUnavailable(_) => ErrorCode::ExternalGitUnavailable,
            Self::GitCommand(_) => ErrorCode::ExternalGitCommand,
            Self::ReportEncoding(_) => ErrorCode::InternalEncoding,
        }
    }

    /// The exit category for this failure.
    #[must_use]
    pub fn category(&self) -> ExitCategory {
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
            Self::Control { reason, .. } | Self::ControlWithRecovery { reason, .. } => {
                serde_json::json!({ "reason": reason })
            }
            Self::ControlWithDetails { details, .. } => details.clone(),
            Self::NoPersist(error) => error.details(),
            Self::ControlIo { path, .. } => serde_json::json!({ "path": path }),
            Self::WorkspaceNotFound(path) | Self::WorkspaceAccess { path, .. } => {
                serde_json::json!({ "path": path })
            }
            Self::GitUnavailable(_) | Self::GitCommand(_) | Self::ReportEncoding(_) => {
                serde_json::json!({})
            }
        }
    }

    /// Operator guidance for recovering from this failure.
    ///
    /// Every variant except [`Self::ControlWithRecovery`] has no better
    /// guidance than its [`ErrorCode`]'s shared table entry, reached through
    /// [`Self::code`]. [`Self::ControlWithRecovery`] carries a fix specific
    /// to its one construction site and returns that instead. See #106.
    #[must_use]
    pub fn recovery(&self) -> &'static str {
        match self {
            Self::ControlWithRecovery { recovery, .. } => recovery,
            _ => self.code().recovery(),
        }
    }

    /// Marks a pure policy refusal as non-persisting when it occurs inside a
    /// transaction after the journal has opened.
    #[must_use]
    pub fn no_persist(error: Self) -> Self {
        Self::NoPersist(Box::new(error))
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
    fn the_card_recovery_names_no_specification_section() {
        // `PolicyInvalidCard` used to send operators to "the activation rules
        // in Section 10.3" — a specification `SKILL.md` never states and no
        // adopting operator holds. This pins the defect closed: whatever the
        // recovery says, it must not point at a section number again.
        assert!(
            !ErrorCode::PolicyInvalidCard.recovery().contains("Section"),
            "PolicyInvalidCard recovery names a specification section the operator \
             cannot read; got: {}",
            ErrorCode::PolicyInvalidCard.recovery()
        );
    }

    #[test]
    fn the_card_recovery_is_actionable() {
        // `PolicyInvalidCard` covers construction sites in `domain/card.rs`,
        // `commands/card.rs`, and `policy/progressive_validation.rs` whose
        // reasons are heterogeneous by design: draft-shape defects (a
        // missing title, an incomplete proof map), immutable-card
        // collisions (creating or activating a card that already exists),
        // risk-driven proof and gate-stage requirements, and revision
        // identity checks. No single sentence can restate all of those
        // without becoming a paragraph, so this recovery does not try —
        // instead it points the operator at the per-site `reason` text
        // (already specific; that was never the defect) and separately
        // names `card revise`, the command that resolves the largest
        // single subclass: every refusal caused by a card that already
        // exists or is already activated.
        //
        // Asserting the command is actually named, rather than only that
        // the string is non-empty, is what lets this test fail: a
        // regression to generic filler such as "Correct the card." reads
        // as guidance but resolves nothing, and would still pass a
        // non-empty check.
        let recovery = ErrorCode::PolicyInvalidCard.recovery();
        assert!(
            recovery.contains("`card revise`"),
            "PolicyInvalidCard recovery should name `card revise`, the command that \
             resolves the largest subclass of refusals (a card that already exists or \
             is already activated); got: {recovery}"
        );
    }

    #[test]
    fn the_card_recovery_conditions_card_revise_on_activation() {
        // The test above only checks that `card revise` is named
        // *somewhere*; it passes just as well for an unconditional "use
        // `card revise`" as for the real text. Unconditional is wrong:
        // it is the exact defect §6 argues this recovery avoids for a
        // draft nothing has activated yet, and for the one internal
        // collision guard reachable with no activated state to revise
        // (both send the operator toward `card revise`, which would
        // either do nothing useful or bounce them into `card activate`'s
        // own "not activated" refusal). This test pins that the
        // conditional is present *and attached to the clause that names
        // the command*, not merely present somewhere in the sentence.
        //
        // Method: split on `;`, the punctuation this sentence (and its
        // neighbour `PolicyLandingMismatch`'s recovery) already uses to
        // separate its two clauses. Find the clause naming `card revise`,
        // and require that *same* clause to also carry the words "if"
        // and "activated". Dropping the conditional entirely fails this,
        // because that clause then has neither word. Keeping a
        // conditional but attaching it to the *other* clause — "Correct
        // the card ... if the card is already activated; use `card
        // revise`." — fails the same way: the clause naming `card
        // revise` still has neither word, because the condition landed
        // on "correct the card" instead.
        //
        // What this does not catch, named rather than implied: a
        // rewording that keeps "if" and "activated" in the clause naming
        // `card revise` but inverts what they mean (a double negative, an
        // "unless", a condition that no longer actually means "this
        // named card is activated"); one that restructures the sentence
        // without `;` as its clause separator; or one that stops
        // backtick-quoting `card revise` (that one fails loudly instead —
        // `find` returns `None` and the lookup panics — rather than
        // silently passing, which is the safe direction for an
        // unanticipated rewording, but is still a failure for a reason
        // other than the one this test is named for). Each of those
        // needs a human rereading the sentence, not a substring check.
        let recovery = ErrorCode::PolicyInvalidCard.recovery();
        let clause_naming_card_revise = recovery
            .split(';')
            .find(|clause| clause.contains("`card revise`"))
            .unwrap_or_else(|| {
                panic!("no clause in PolicyInvalidCard recovery names `card revise`: {recovery:?}")
            });
        assert!(
            clause_naming_card_revise.contains("if")
                && clause_naming_card_revise.contains("activated"),
            "the clause naming `card revise` must itself condition it on activation state \
             rather than presenting it as the unconditional remedy, and the condition must \
             not have landed in a different clause; clause naming `card revise`: \
             {clause_naming_card_revise:?} (full recovery: {recovery:?})"
        );
    }

    #[test]
    fn the_control_incompatible_recovery_prescribes_no_single_cause() {
        // `ConfigControlIncompatible` is shared by three sites whose
        // remedies have nothing in common: `ControlRepository::open` (no
        // control repository at the path), `project.rs::reinitialize` (one
        // already bound to a different configuration), and
        // `project.rs::refuse_orphaning_facts` (convergence facts recorded
        // under a policy digest the install would orphan). This recovery
        // used to read "Point at the matching control repository, or
        // initialize a new project elsewhere." — correct for the first two
        // and actively misleading for the third, where the resolution is
        // `disposition rebaseline` and no amount of pointing at a different
        // repository helps.
        //
        // Unlike `PolicyInvalidCard` above, there is no dominant subclass to
        // name a command for: each of the three wants a different one, or
        // none. So this recovery defers to the per-site `reason` text, the
        // way `PreconditionMergeRefused` does for an equally open set. The
        // two assertions are deliberately opposite in sign — the first
        // fails if the old cause-specific advice returns, the second fails
        // if it is replaced by generic filler ("Correct the configuration.")
        // that resolves nothing while still reading as guidance and still
        // passing `every_code_offers_recovery_guidance`'s non-empty check.
        let recovery = ErrorCode::ConfigControlIncompatible.recovery();
        assert!(
            !recovery.contains("control repository, or initialize"),
            "the recovery prescribes one cause's remedy again, which misdirects the sites \
             it does not fit; got: {recovery}"
        );
        assert!(
            recovery.contains("Read the mismatch this error reports"),
            "the recovery must send the operator to the message, which is the only place \
             that knows which of the three mismatches this is; got: {recovery}"
        );
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
