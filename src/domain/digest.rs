//! SHA-256 digests and the canonical JSON form they are computed over.
//!
//! Section 10.1 requires one documented canonical serialization. `SPIKE-001`
//! finding F-2 additionally requires that a record name the algorithm it was
//! digested under, so a holder of the record can recompute the digest without
//! consulting this source.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use crate::error::HarnessError;

/// Identifies the canonical serialization used before hashing.
///
/// Recorded inside authoritative records so digests stay reproducible if this
/// algorithm is ever revised.
pub const CANONICAL_ALGORITHM: &str = "harness.canonical-json/v1";

const PREFIX: &str = "sha256:";
const HEX_LEN: usize = 64;

/// A SHA-256 digest rendered as `sha256:<64 lowercase hex characters>`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Digest(String);

impl Digest {
    /// Hashes raw bytes.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(format!("{PREFIX}{:x}", hasher.finalize()))
    }

    /// Hashes a value through the canonical JSON form.
    ///
    /// # Errors
    ///
    /// Returns an error when the value cannot be serialized.
    pub fn of_canonical<T: Serialize>(value: &T) -> Result<Self, HarnessError> {
        Ok(Self::of_bytes(canonical_json(value)?.as_bytes()))
    }

    /// The digest's canonical text form, including the `sha256:` prefix.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Digest {
    type Err = HarnessError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some(hex) = value.strip_prefix(PREFIX) else {
            return Err(HarnessError::invalid_digest(
                value,
                format!("expected prefix `{PREFIX}`"),
            ));
        };
        if hex.len() != HEX_LEN {
            return Err(HarnessError::invalid_digest(
                value,
                format!("expected {HEX_LEN} hex characters, found {}", hex.len()),
            ));
        }
        if !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(HarnessError::invalid_digest(
                value,
                "expected only lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value.to_owned()))
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// Renders a value as canonical JSON: object keys sorted, no insignificant
/// whitespace, and no trailing newline.
///
/// `serde_json`'s preserve-order feature is deliberately not enabled, so its
/// maps sort keys already. Re-parsing into [`serde_json::Value`] makes that
/// ordering apply to every value regardless of how its `Serialize` impl emits
/// fields.
///
/// # Errors
///
/// Returns an error when the value cannot be serialized.
pub fn canonical_json<T: Serialize>(value: &T) -> Result<String, HarnessError> {
    let intermediate = serde_json::to_string(value)?;
    let sorted: serde_json::Value = serde_json::from_str(&intermediate)?;
    Ok(serde_json::to_string(&sorted)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hashes_the_documented_empty_string_vector() {
        // Committed test vector: SHA-256 of the empty byte string.
        assert_eq!(
            Digest::of_bytes(b"").as_str(),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hashes_the_documented_abc_vector() {
        assert_eq!(
            Digest::of_bytes(b"abc").as_str(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn canonical_json_sorts_keys_and_omits_whitespace() {
        let value = json!({"b": 1, "a": {"d": 2, "c": 3}});
        assert_eq!(
            canonical_json(&value).unwrap(),
            r#"{"a":{"c":3,"d":2},"b":1}"#
        );
    }

    #[test]
    fn field_order_does_not_change_the_digest() {
        let one = json!({"alpha": 1, "beta": 2});
        let two = json!({"beta": 2, "alpha": 1});
        assert_eq!(
            Digest::of_canonical(&one).unwrap(),
            Digest::of_canonical(&two).unwrap()
        );
    }

    #[test]
    fn material_change_changes_the_digest() {
        let one = json!({"alpha": 1});
        let two = json!({"alpha": 2});
        assert_ne!(
            Digest::of_canonical(&one).unwrap(),
            Digest::of_canonical(&two).unwrap()
        );
    }

    #[test]
    fn canonical_digest_matches_hashing_the_canonical_text() {
        let value = json!({"z": [1, 2], "a": "x"});
        let text = canonical_json(&value).unwrap();
        assert_eq!(
            Digest::of_canonical(&value).unwrap(),
            Digest::of_bytes(text.as_bytes())
        );
    }

    #[test]
    fn parses_and_rejects_digest_text() {
        let good = format!("sha256:{}", "a".repeat(HEX_LEN));
        assert!(good.parse::<Digest>().is_ok());
        for bad in [
            "abc",
            "sha256:",
            "sha1:0000",
            &format!("sha256:{}", "a".repeat(HEX_LEN - 1)),
            &format!("sha256:{}", "A".repeat(HEX_LEN)),
            &format!("sha256:{}", "g".repeat(HEX_LEN)),
        ] {
            assert!(bad.parse::<Digest>().is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn digest_round_trips_through_json() {
        let digest = Digest::of_bytes(b"round trip");
        let encoded = serde_json::to_string(&digest).unwrap();
        let decoded: Digest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, digest);
    }
}
