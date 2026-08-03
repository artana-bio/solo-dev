//! Recognition for the one credential-shaped class currently governed.
//!
//! This is intentionally one narrow shape check, not a general secret scanner.

const GITHUB_TOKEN_PREFIX: &[u8] = b"ghp_";
const GITHUB_TOKEN_TAIL_LENGTH: usize = 36;
const ANTHROPIC_API_KEY_PREFIX: &[u8] = b"sk-ant-api03-";
const ANTHROPIC_API_KEY_MIN_TAIL_LENGTH: usize = 20;
const RSA_PRIVATE_KEY_PEM_HEADER: &str = "-----BEGIN RSA PRIVATE KEY-----";

/// Returns whether `contents` contains a token beginning with the governed
/// Anthropic API-key prefix and at least 20 ASCII token characters.
#[must_use]
pub(crate) fn contains_anthropic_api_key_shape(contents: &[u8]) -> bool {
    let mut search_from = 0;
    while let Some(relative_start) = contents[search_from..]
        .windows(ANTHROPIC_API_KEY_PREFIX.len())
        .position(|window| window == ANTHROPIC_API_KEY_PREFIX)
    {
        let start = search_from + relative_start;
        let before = start.checked_sub(1).and_then(|index| contents.get(index));
        let tail_start = start + ANTHROPIC_API_KEY_PREFIX.len();
        if before.is_some_and(|byte| is_anthropic_token_character(*byte)) {
            search_from = tail_start;
            continue;
        }
        let tail_length = contents[tail_start..]
            .iter()
            .take_while(|byte| is_anthropic_token_character(**byte))
            .count();
        if tail_length >= ANTHROPIC_API_KEY_MIN_TAIL_LENGTH {
            return true;
        }
        search_from = tail_start + tail_length;
    }
    false
}

/// Returns whether `contents` contains the exact RSA private-key PEM header as
/// its own trimmed line. Inline prose that merely quotes the header is not the
/// governed credential shape.
#[must_use]
pub(crate) fn contains_rsa_private_key_pem_header_line(contents: &str) -> bool {
    contents
        .lines()
        .any(|line| line.trim() == RSA_PRIVATE_KEY_PEM_HEADER)
}

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

const fn is_anthropic_token_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
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
