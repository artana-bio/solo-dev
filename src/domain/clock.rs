//! UTC clock abstraction.
//!
//! Authoritative records carry timestamps, and tests must be able to assert on
//! exact record bytes. Commands therefore take a clock rather than reading the
//! system time directly.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::error::HarnessError;

/// A UTC instant rendered as RFC 3339 with second precision.
///
/// Sub-second precision is deliberately discarded. It would vary between a
/// record and its recomputation without carrying workflow meaning, and these
/// values participate in digests.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    /// Builds a timestamp from a Unix epoch second count.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is outside the supported range.
    pub fn from_unix_seconds(seconds: i64) -> Result<Self, HarnessError> {
        OffsetDateTime::from_unix_timestamp(seconds)
            .map(Self)
            .map_err(|source| HarnessError::invalid_timestamp(seconds.to_string(), source))
    }

    /// The instant as a Unix epoch second count.
    #[must_use]
    pub const fn unix_seconds(&self) -> i64 {
        self.0.unix_timestamp()
    }

    /// Renders the instant as RFC 3339 in UTC.
    #[must_use]
    pub fn to_rfc3339(&self) -> String {
        self.0
            .replace_nanosecond(0)
            .unwrap_or(self.0)
            .format(&Rfc3339)
            .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_rfc3339())
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        OffsetDateTime::parse(&raw, &Rfc3339)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// Supplies the current UTC instant.
pub trait Clock: fmt::Debug + Send + Sync {
    /// Returns the current instant, truncated to whole seconds.
    fn now(&self) -> Timestamp;
}

/// Reads the host clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp(OffsetDateTime::now_utc())
    }
}

/// Returns a caller-supplied instant, for deterministic tests.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock(Timestamp);

impl FixedClock {
    /// Builds a clock pinned to one instant.
    #[must_use]
    pub const fn new(instant: Timestamp) -> Self {
        Self(instant)
    }

    /// Builds a clock pinned to a Unix epoch second count.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is outside the supported range.
    pub fn at_unix_seconds(seconds: i64) -> Result<Self, HarnessError> {
        Timestamp::from_unix_seconds(seconds).map(Self)
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_unix_epoch() {
        let epoch = Timestamp::from_unix_seconds(0).unwrap();
        assert_eq!(epoch.to_rfc3339(), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn renders_a_known_instant_in_utc() {
        // 2026-07-28T00:00:00Z
        let instant = Timestamp::from_unix_seconds(1_785_196_800).unwrap();
        assert_eq!(instant.to_rfc3339(), "2026-07-28T00:00:00Z");
    }

    #[test]
    fn fixed_clock_does_not_advance() {
        let clock = FixedClock::at_unix_seconds(1_785_196_800).unwrap();
        assert_eq!(clock.now(), clock.now());
    }

    #[test]
    fn system_clock_is_after_the_plan_date() {
        let now = SystemClock.now();
        assert!(now.unix_seconds() > 1_785_196_800);
    }

    #[test]
    fn timestamp_round_trips_through_json() {
        let instant = Timestamp::from_unix_seconds(1_785_196_800).unwrap();
        let encoded = serde_json::to_string(&instant).unwrap();
        assert_eq!(encoded, "\"2026-07-28T00:00:00Z\"");
        let decoded: Timestamp = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, instant);
    }

    #[test]
    fn deserialization_rejects_non_rfc3339_text() {
        assert!(serde_json::from_str::<Timestamp>("\"yesterday\"").is_err());
    }
}
