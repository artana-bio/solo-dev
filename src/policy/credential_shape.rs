//! Recognition for the one credential-shaped fixture currently governed.
//!
//! This is intentionally an exact fixture check, not a general secret scanner.

const GITHUB_TOKEN_FIXTURE: &[u8] = b"ghp_0123456789abcdef0123456789abcdef0123";

/// Returns whether `contents` contains the standalone proof fixture.
///
/// The surrounding bytes are deliberately not interpreted as JSON or any
/// other format. A token-character boundary keeps a larger identifier that
/// merely contains the fixture from becoming this exact staged-content class.
#[must_use]
pub fn contains_standalone_github_token_fixture(contents: &[u8]) -> bool {
    contents
        .windows(GITHUB_TOKEN_FIXTURE.len())
        .enumerate()
        .any(|(start, window)| {
            if window != GITHUB_TOKEN_FIXTURE {
                return false;
            }
            let before = start.checked_sub(1).and_then(|index| contents.get(index));
            let after = contents.get(start + GITHUB_TOKEN_FIXTURE.len());
            !before.is_some_and(|byte| is_token_character(*byte))
                && !after.is_some_and(|byte| is_token_character(*byte))
        })
}

const fn is_token_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::contains_standalone_github_token_fixture;

    const FIXTURE: &str = "ghp_0123456789abcdef0123456789abcdef0123";

    #[test]
    fn recognizes_the_exact_standalone_fixture() {
        assert!(contains_standalone_github_token_fixture(
            format!(r#"{{"note":"{FIXTURE}"}}"#).as_bytes()
        ));
    }

    #[test]
    fn ignores_other_prefixes_and_embedded_identifiers() {
        assert!(!contains_standalone_github_token_fixture(
            format!("gho_{FIXTURE}").as_bytes()
        ));
        assert!(!contains_standalone_github_token_fixture(
            format!("before{FIXTURE}").as_bytes()
        ));
        assert!(!contains_standalone_github_token_fixture(
            b"ghp_0123456789abcdef0123456789abcdef0124"
        ));
    }
}
