//! The index of what was archived, and where.
//!
//! Section 9's archive refs live in the candidate repository, which the harness
//! does not own exclusively. This record is the control repository's own
//! account of what should be there, so a missing or moved ref is detectable
//! rather than merely absent.

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        clock::Timestamp,
        digest::{CANONICAL_ALGORITHM, Digest},
        ids::{CycleId, IntegrationId},
    },
    error::HarnessError,
    git::archive::ArchivedRef,
};

/// Schema identifier for an archive record.
pub const ARCHIVE_SCHEMA: &str = "harness.archive/v1";

/// Directory holding archive records, relative to the control repository.
pub const ARCHIVE_DIR: &str = "archives";

/// What one promoted integration archived.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArchiveRecord {
    /// Always [`ARCHIVE_SCHEMA`].
    pub schema: String,
    /// The integration archived.
    pub integration_id: IntegrationId,
    /// The cycle it belongs to.
    pub cycle_id: CycleId,
    /// Every ref created, with the commit it must resolve to.
    pub refs: Vec<ArchivedRef>,
    /// Who archived it. Declared, not proven; see D-013.
    pub archived_by: String,
    /// When it was archived.
    pub archived_at: Timestamp,
    /// The canonicalization algorithm its digest was computed under.
    pub canonical_algorithm: String,
}

impl ArchiveRecord {
    /// Relative path of an archive record inside the control repository.
    #[must_use]
    pub fn relative_path(integration_id: &IntegrationId) -> String {
        format!("{ARCHIVE_DIR}/{integration_id}.json")
    }

    /// The record's canonical digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the record cannot be serialized.
    pub fn digest(&self) -> Result<Digest, HarnessError> {
        Digest::of_canonical(self)
    }

    /// The canonicalization algorithm archives are digested under.
    #[must_use]
    pub const fn canonical_algorithm() -> &'static str {
        CANONICAL_ALGORITHM
    }
}
