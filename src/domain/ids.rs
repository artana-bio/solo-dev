//! Validated identifier types.
//!
//! Every authoritative record names other records by identifier, so a malformed
//! identifier must be rejected at the boundary rather than stored and
//! discovered later. Each type parses from a string, rejects anything outside
//! its documented shape, and renders back to exactly the text it parsed.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{ErrorCode, HarnessError};

/// Longest identifier this crate accepts.
///
/// Identifiers reach the filesystem as ref and path components, so an unbounded
/// identifier is a denial-of-service and a path-length hazard.
const MAX_ID_LEN: usize = 64;

/// Parses the `<PREFIX>-<DIGITS>` shape shared by most record identifiers.
fn parse_prefixed(value: &str, prefix: &str, min_digits: usize) -> Result<String, HarnessError> {
    if value.len() > MAX_ID_LEN {
        return Err(HarnessError::invalid_id(
            value,
            format!("identifier exceeds {MAX_ID_LEN} characters"),
        ));
    }
    let Some(digits) = value.strip_prefix(prefix) else {
        return Err(HarnessError::invalid_id(
            value,
            format!("expected prefix `{prefix}`"),
        ));
    };
    if digits.len() < min_digits {
        return Err(HarnessError::invalid_id(
            value,
            format!("expected at least {min_digits} digits after `{prefix}`"),
        ));
    }
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(HarnessError::invalid_id(
            value,
            format!("expected only ASCII digits after `{prefix}`"),
        ));
    }
    Ok(value.to_owned())
}

/// Declares one identifier type with a fixed prefix and minimum digit count.
macro_rules! prefixed_id {
    ($name:ident, $prefix:literal, $min_digits:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// The identifier's canonical text form.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// The prefix every identifier of this type carries.
            #[must_use]
            pub const fn prefix() -> &'static str {
                $prefix
            }
        }

        impl FromStr for $name {
            type Err = HarnessError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                parse_prefixed(value, $prefix, $min_digits).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                raw.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

prefixed_id!(
    CycleId,
    "C-",
    3,
    "Identifies one bounded integration period."
);
prefixed_id!(
    CardId,
    "F-",
    3,
    "Identifies one independently reviewable outcome."
);
prefixed_id!(
    LeaseId,
    "L-",
    6,
    "Identifies one exclusive card-to-actor assignment."
);
prefixed_id!(ReceiptId, "R-", 6, "Identifies one named-gate run.");
prefixed_id!(
    ValidationReservationId,
    "VR-",
    6,
    "Identifies one durable reservation for an expensive validation run."
);
prefixed_id!(ReviewId, "RV-", 6, "Identifies one review record.");
prefixed_id!(
    IntegrationId,
    "INT-",
    3,
    "Identifies one deterministic combination of approved candidates."
);
prefixed_id!(
    AcceptanceId,
    "ACC-",
    6,
    "Identifies one acceptance decision over a landing commit."
);
prefixed_id!(EventId, "E-", 6, "Identifies one authoritative transition.");
prefixed_id!(
    OperationId,
    "OP-",
    6,
    "Identifies one journaled mutating operation."
);

/// Identifies one configured project.
///
/// Project identifiers are operator-authored rather than allocated, so they use
/// a restricted slug instead of a numeric suffix. The restriction exists because
/// this value becomes a directory and ref component.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProjectId(String);

impl ProjectId {
    /// The identifier's canonical text form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ProjectId {
    type Err = HarnessError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(HarnessError::invalid_id(value, "identifier is empty"));
        }
        if value.len() > MAX_ID_LEN {
            return Err(HarnessError::invalid_id(
                value,
                format!("identifier exceeds {MAX_ID_LEN} characters"),
            ));
        }
        let mut bytes = value.bytes();
        let first = bytes.next().expect("non-empty checked above");
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return Err(HarnessError::invalid_id(
                value,
                "must begin with a lowercase ASCII letter or digit",
            ));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(HarnessError::invalid_id(
                value,
                "may contain only lowercase ASCII letters, digits, and `-`",
            ));
        }
        if value.ends_with('-') {
            return Err(HarnessError::invalid_id(value, "must not end with `-`"));
        }
        if value.contains("--") {
            return Err(HarnessError::invalid_id(
                value,
                "must not contain consecutive `-`",
            ));
        }
        Ok(Self(value.to_owned()))
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for ProjectId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ProjectId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// Confirms an identifier rejection carries the stable invalid-identifier code.
#[must_use]
pub fn is_invalid_id(error: &HarnessError) -> bool {
    error.code() == ErrorCode::UsageInvalidId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_documented_identifier_shapes() {
        assert_eq!("F-123".parse::<CardId>().unwrap().to_string(), "F-123");
        assert_eq!("C-001".parse::<CycleId>().unwrap().to_string(), "C-001");
        assert_eq!(
            "L-000123".parse::<LeaseId>().unwrap().to_string(),
            "L-000123"
        );
        assert_eq!(
            "OP-000123".parse::<OperationId>().unwrap().to_string(),
            "OP-000123"
        );
        assert_eq!(
            "INT-001".parse::<IntegrationId>().unwrap().to_string(),
            "INT-001"
        );
        assert_eq!(
            "RV-000001".parse::<ReviewId>().unwrap().to_string(),
            "RV-000001"
        );
        assert_eq!(
            "ACC-000001".parse::<AcceptanceId>().unwrap().to_string(),
            "ACC-000001"
        );
    }

    #[test]
    fn rejects_wrong_prefix_and_short_or_non_numeric_suffix() {
        for bad in ["X-123", "F123", "F-12", "F-12a", "F-", ""] {
            let error = bad.parse::<CardId>().expect_err("must reject");
            assert!(is_invalid_id(&error), "{bad} produced {error:?}");
        }
    }

    #[test]
    fn rejects_overlong_identifiers() {
        let long = format!("F-{}", "1".repeat(MAX_ID_LEN));
        let error = long.parse::<CardId>().expect_err("must reject");
        assert!(is_invalid_id(&error));
    }

    #[test]
    fn project_id_accepts_slugs_and_rejects_unsafe_shapes() {
        assert_eq!("example".parse::<ProjectId>().unwrap().as_str(), "example");
        assert_eq!("a1-b2".parse::<ProjectId>().unwrap().as_str(), "a1-b2");
        for bad in [
            "",
            "-lead",
            "trail-",
            "Upper",
            "has space",
            "has/slash",
            "has..dots",
            "double--dash",
        ] {
            let error = bad.parse::<ProjectId>().expect_err("must reject");
            assert!(is_invalid_id(&error), "{bad} produced {error:?}");
        }
    }

    #[test]
    fn identifiers_round_trip_through_json() {
        let card: CardId = "F-4242".parse().unwrap();
        let encoded = serde_json::to_string(&card).unwrap();
        assert_eq!(encoded, "\"F-4242\"");
        let decoded: CardId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, card);
    }

    #[test]
    fn deserialization_rejects_malformed_identifiers() {
        let error = serde_json::from_str::<CardId>("\"nope\"").expect_err("must reject");
        assert!(error.to_string().contains("nope"));
    }
}
