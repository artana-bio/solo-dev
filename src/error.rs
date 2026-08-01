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
    /// The gate runner could not execute or supervise a process.
    GateRunnerError,
    /// A named gate failed, timed out, or was signalled.
    GateFailed,
    /// Required gate evidence is missing or no longer applies.
    GateEvidenceStale,
    /// A handoff declaration is missing a required field.
    PolicyIncompleteHandoff,
    /// The branch was rewritten between delivery and handoff creation.
    PolicyDeliveredShaMismatch,
    /// The reviewer is the feature actor.
    PolicySelfReview,
    /// A review approved while findings remain open.
    PolicyOpenFindings,
    /// A review is missing a required field.
    PolicyIncompleteReview,
    /// The handoff under review no longer describes the branch.
    PolicyStaleHandoff,
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
    pub const ALL: [Self; 79] = [
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
        Self::PolicyOwnershipOverlap,
        Self::PolicyContractOverlap,
        Self::PolicyResourceConflict,
        Self::PolicyDependencyUnsatisfied,
        Self::PolicyDependencyCycle,
        Self::PolicyNotIntegrable,
        Self::PolicyAtomicGroupSplit,
        Self::PolicyIntegrationOpen,
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
        Self::GateRunnerError,
        Self::GateFailed,
        Self::GateEvidenceStale,
        Self::PolicyIncompleteHandoff,
        Self::PolicyDeliveredShaMismatch,
        Self::PolicySelfReview,
        Self::PolicyOpenFindings,
        Self::PolicyIncompleteReview,
        Self::PolicyStaleHandoff,
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
            | Self::PolicyOwnershipOverlap
            | Self::PolicyContractOverlap
            | Self::PolicyResourceConflict
            | Self::PolicyDependencyUnsatisfied
            | Self::PolicyDependencyCycle
            | Self::PolicyNotIntegrable
            | Self::PolicyAtomicGroupSplit
            | Self::PolicyIntegrationOpen
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
            | Self::PolicyIncompleteHandoff
            | Self::PolicyDeliveredShaMismatch
            | Self::PolicySelfReview
            | Self::PolicyRiskReview
            | Self::PolicyOpenFindings
            | Self::PolicyIncompleteReview
            | Self::PolicyStaleHandoff => ExitCategory::Policy,
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
            Self::PolicyOwnershipOverlap => "OWNERSHIP-OVERLAP",
            Self::PolicyContractOverlap => "CONTRACT-OVERLAP",
            Self::PolicyResourceConflict => "RESOURCE-CONFLICT",
            Self::PolicyDependencyUnsatisfied => "DEPENDENCY-UNSATISFIED",
            Self::PolicyDependencyCycle => "DEPENDENCY-CYCLE",
            Self::PolicyNotIntegrable => "NOT-INTEGRABLE",
            Self::PolicyAtomicGroupSplit => "ATOMIC-GROUP-SPLIT",
            Self::PolicyIntegrationOpen => "INTEGRATION-OPEN",
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
            Self::ConfigControlIncompatible => {
                "Point at the matching control repository, or initialize a new project elsewhere."
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

    const fn policy_recovery(self) -> &'static str {
        match self {
            Self::PolicyBackupNotIndependent
            | Self::PolicyBackupCorrupt
            | Self::PolicyAuditDiscrepancy
            | Self::PolicyInvalidArtifact
            | Self::PolicyArtifactNotOwned => Self::hardening_recovery(self),
            Self::PolicyLockHeld => "Wait for the other command to finish, then retry.",
            Self::PolicyInvalidTransition => {
                "Move through the documented states in order, or abandon the subject."
            }
            Self::PolicyInvalidCycle => {
                "Correct the cycle record so its membership and baseline are coherent."
            }
            Self::PolicyInvalidCard => {
                "Correct the card so it satisfies the activation rules in Section 10.3."
            }
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
            Self::PolicyIntegrationOpen => {
                "Promote, archive, or abandon the open integration before preparing another."
            }
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
            _ => "Resolve the reported policy violation.",
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
            Self::Config { code, .. } | Self::Control { code, .. } => *code,
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
            Self::Control { reason, .. } => serde_json::json!({ "reason": reason }),
            Self::ControlIo { path, .. } => serde_json::json!({ "path": path }),
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
