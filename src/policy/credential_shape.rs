//! Recognition for the one credential-shaped class currently governed.
//!
//! This is intentionally one narrow shape check, not a general secret scanner.

const GITHUB_TOKEN_PREFIX: &[u8] = b"ghp_";
const GITHUB_TOKEN_TAIL_LENGTH: usize = 36;

/// Returns whether `contents` contains a standalone `ghp_` value with the
/// governed 36-character ASCII-alphanumeric tail.
///
/// The surrounding bytes are deliberately not interpreted as JSON or any
/// other format. Token-character boundaries keep a larger identifier that
/// merely contains this shape from becoming the governed staged-content class.
#[must_use]
pub fn contains_standalone_github_token_shape(contents: &[u8]) -> bool {
    let candidate_length = GITHUB_TOKEN_PREFIX.len() + GITHUB_TOKEN_TAIL_LENGTH;
    contents
        .windows(candidate_length)
        .enumerate()
        .any(|(start, window)| {
            if !window.starts_with(GITHUB_TOKEN_PREFIX)
                || !window[GITHUB_TOKEN_PREFIX.len()..]
                    .iter()
                    .all(u8::is_ascii_alphanumeric)
            {
                return false;
            }
            let before = start.checked_sub(1).and_then(|index| contents.get(index));
            let after = contents.get(start + candidate_length);
            !before.is_some_and(|byte| is_token_character(*byte))
                && !after.is_some_and(|byte| is_token_character(*byte))
        })
}

/// Returns whether a path contains the governed token shape delimited by
/// underscores. The token-shape predicate remains the single credential
/// classifier; underscores only provide the path boundary needed before mode
/// dispatch.
#[must_use]
pub(crate) fn contains_underscore_delimited_github_token_shape(path: &[u8]) -> bool {
    let candidate_length = GITHUB_TOKEN_PREFIX.len() + GITHUB_TOKEN_TAIL_LENGTH;
    path.windows(candidate_length + 2).any(|window| {
        window[0] == b'_'
            && window[candidate_length + 1] == b'_'
            && contains_standalone_github_token_shape(&window[1..=candidate_length])
    })
}

const fn is_token_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::contains_standalone_github_token_shape;

    const VALID_TAIL: &str = "abcdefghijklmnopqrstuvwxyz0123456789";

    #[test]
    fn recognizes_another_valid_standalone_tail() {
        let value = format!("ghp_{VALID_TAIL}");
        assert!(contains_standalone_github_token_shape(
            format!(r#"{{"note":"{value}"}}"#).as_bytes()
        ));
    }

    #[test]
    fn rejects_short_long_and_non_alphanumeric_tails() {
        assert!(!contains_standalone_github_token_shape(
            format!("ghp_{}", &VALID_TAIL[..35]).as_bytes()
        ));
        assert!(!contains_standalone_github_token_shape(
            format!("ghp_{VALID_TAIL}x").as_bytes()
        ));
        assert!(!contains_standalone_github_token_shape(
            b"ghp_abcdefghijklmnopqrstuvwxyz012345678-"
        ));
    }

    #[test]
    fn rejects_other_prefixes_and_embedded_identifiers() {
        let value = format!("ghp_{VALID_TAIL}");
        assert!(!contains_standalone_github_token_shape(
            format!("gho_{VALID_TAIL}").as_bytes()
        ));
        assert!(!contains_standalone_github_token_shape(
            format!("before{value}after").as_bytes()
        ));
    }
}
