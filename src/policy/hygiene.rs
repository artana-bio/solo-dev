//! Record hygiene: keeping credentials out of durable control records.
//!
//! Cards, handoffs, reviews, gate definitions, and receipts are committed to
//! the control repository, and Git history *is* the integrity chain (D-011).
//! Hand-editing control is forbidden, so a credential that reaches a control
//! commit cannot be excised without breaking the property the harness exists
//! to provide. Every other failure in this codebase produces a recoverable
//! state; this one produces an immutable one, which is why the check runs
//! before the write rather than as an audit afterwards.
//!
//! What this is not: complete secret discovery. The scanner recognizes token
//! shapes that announce themselves — issuer prefixes, PEM headers, credentials
//! embedded in URLs. A bare password, an internal token with no distinguishing
//! prefix, and a base64 blob that could be anything all pass through it. That
//! limit is deliberate rather than a gap left for later: the control
//! repository is full of SHA-256 digests, and a scanner that flagged
//! high-entropy strings generically would refuse ordinary evidence constantly
//! until someone turned it off. A control nobody leaves on protects nothing.

use crate::error::{ErrorCode, HarnessError};

/// A credential shape the scanner recognizes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretKind {
    /// A GitHub personal access, OAuth, user, server, or refresh token.
    GitHubToken,
    /// A GitLab personal access token.
    GitLabToken,
    /// A Slack bot, app, user, or refresh token.
    SlackToken,
    /// An AWS access key identifier.
    AwsAccessKeyId,
    /// An npm access token.
    NpmToken,
    /// A vendor API key issued with an `sk-` prefix.
    ApiKey,
    /// A PEM-encoded private key block.
    PrivateKey,
    /// A password embedded in a URL's userinfo.
    UrlPassword,
}

impl SecretKind {
    /// Names the shape for a refusal message.
    ///
    /// Only the shape is ever named. The matched text is not returned to any
    /// caller that formats an error, because a refusal that quotes the value
    /// re-leaks it into terminal scrollback and CI logs — the failure the
    /// refusal exists to prevent, one layer out.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GitHubToken => "a GitHub token",
            Self::GitLabToken => "a GitLab token",
            Self::SlackToken => "a Slack token",
            Self::AwsAccessKeyId => "an AWS access key id",
            Self::NpmToken => "an npm token",
            Self::ApiKey => "an API key",
            Self::PrivateKey => "a PEM private key",
            Self::UrlPassword => "a password in a URL",
        }
    }
}

/// Where a credential shape was found, as byte offsets into the scanned text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Match {
    /// Byte offset of the first character.
    pub start: usize,
    /// Byte offset one past the last character.
    pub end: usize,
    /// Which shape matched.
    pub kind: SecretKind,
}

/// The placeholder that replaces a matched value.
const PLACEHOLDER_OPEN: &str = "[redacted:";

/// One issuer-prefixed token shape.
struct Prefixed {
    /// The literal that introduces the token.
    prefix: &'static str,
    /// How many trailing characters must follow for this to be a token rather
    /// than the prefix appearing in prose.
    min_tail: usize,
    /// What the match is called.
    kind: SecretKind,
}

/// Issuer prefixes, longest first so `github_pat_` wins over any shorter
/// overlap and the scan stays deterministic regardless of table order.
const PREFIXED: [Prefixed; 13] = [
    Prefixed {
        prefix: "github_pat_",
        min_tail: 22,
        kind: SecretKind::GitHubToken,
    },
    Prefixed {
        prefix: "glpat-",
        min_tail: 20,
        kind: SecretKind::GitLabToken,
    },
    Prefixed {
        prefix: "ghp_",
        min_tail: 36,
        kind: SecretKind::GitHubToken,
    },
    Prefixed {
        prefix: "gho_",
        min_tail: 36,
        kind: SecretKind::GitHubToken,
    },
    Prefixed {
        prefix: "ghu_",
        min_tail: 36,
        kind: SecretKind::GitHubToken,
    },
    Prefixed {
        prefix: "ghs_",
        min_tail: 36,
        kind: SecretKind::GitHubToken,
    },
    Prefixed {
        prefix: "ghr_",
        min_tail: 36,
        kind: SecretKind::GitHubToken,
    },
    Prefixed {
        prefix: "xoxb-",
        min_tail: 10,
        kind: SecretKind::SlackToken,
    },
    Prefixed {
        prefix: "xoxp-",
        min_tail: 10,
        kind: SecretKind::SlackToken,
    },
    Prefixed {
        prefix: "xoxa-",
        min_tail: 10,
        kind: SecretKind::SlackToken,
    },
    Prefixed {
        prefix: "xoxs-",
        min_tail: 10,
        kind: SecretKind::SlackToken,
    },
    Prefixed {
        prefix: "npm_",
        min_tail: 36,
        kind: SecretKind::NpmToken,
    },
    // Long enough that `sk-` in prose cannot reach it.
    Prefixed {
        prefix: "sk-",
        min_tail: 20,
        kind: SecretKind::ApiKey,
    },
];

/// Characters that continue a token once its prefix has matched.
fn is_token_char(value: char) -> bool {
    value.is_ascii_alphanumeric() || value == '_' || value == '-'
}

/// Length of the token starting at `rest`, when one of the prefixes matches.
fn prefixed_at(rest: &str) -> Option<(SecretKind, usize)> {
    for candidate in &PREFIXED {
        let Some(tail) = rest.strip_prefix(candidate.prefix) else {
            continue;
        };
        let length: usize = tail
            .chars()
            .take_while(|value| is_token_char(*value))
            .map(char::len_utf8)
            .sum();
        if length >= candidate.min_tail {
            return Some((candidate.kind, candidate.prefix.len() + length));
        }
    }
    None
}

/// Length of an AWS access key identifier starting at `rest`.
///
/// Separate from [`PREFIXED`] because the identifier is a fixed twenty
/// characters of uppercase and digits; measuring it as a run would swallow an
/// adjacent word and report the wrong span to the redactor.
fn aws_key_at(rest: &str) -> Option<(SecretKind, usize)> {
    if !(rest.starts_with("AKIA") || rest.starts_with("ASIA")) {
        return None;
    }
    let body: Vec<char> = rest.chars().take(20).collect();
    if body.len() == 20
        && body[4..]
            .iter()
            .all(|value| value.is_ascii_uppercase() || value.is_ascii_digit())
    {
        // Twenty-one significant characters would be a longer identifier this
        // does not recognize; refusing to guess keeps the span exact.
        let next = rest.chars().nth(20);
        if !next.is_some_and(is_token_char) {
            return Some((SecretKind::AwsAccessKeyId, 20));
        }
    }
    None
}

/// Length of a PEM private key block starting at `rest`.
///
/// The whole block is one match. Redacting only the header would leave the key
/// material behind, which is the opposite of the point.
fn private_key_at(rest: &str) -> Option<(SecretKind, usize)> {
    if !rest.starts_with("-----BEGIN") {
        return None;
    }
    let header_end = rest.find("-----\n").or_else(|| rest.find("-----\r\n"))?;
    let header = &rest[..header_end];
    if !header.contains("PRIVATE KEY") {
        return None;
    }
    // An unterminated block still gets redacted to the end of the text: a
    // truncated key is not a safe key.
    let end = rest
        .find("-----END")
        .and_then(|start| rest[start..].find("-----\n").map(|to| start + to + 5))
        .unwrap_or(rest.len());
    Some((SecretKind::PrivateKey, end))
}

/// Length of a URL userinfo carrying a password, starting at `rest`.
fn url_password_at(rest: &str) -> Option<(SecretKind, usize)> {
    let after_scheme = rest.strip_prefix("://")?;
    let mut length = 0usize;
    let mut has_colon = false;
    for value in after_scheme.chars() {
        match value {
            '@' if has_colon && length > 0 => {
                return Some((SecretKind::UrlPassword, 3 + length + 1));
            }
            // A bare `user@host` carries no password, and any path or query
            // character means the userinfo ended without one.
            '@' | '/' | '?' | '#' | ' ' | '\t' | '\n' | '\r' => return None,
            ':' => {
                has_colon = true;
                length += 1;
            }
            other => length += other.len_utf8(),
        }
    }
    None
}

/// Every credential shape in `text`, in the order they appear.
///
/// Matches never overlap: the scan resumes after each one, so a redaction
/// built from these spans cannot corrupt the surrounding text.
#[must_use]
pub fn find_all(text: &str) -> Vec<Match> {
    let mut found = Vec::new();
    let mut index = 0usize;
    while index < text.len() {
        if !text.is_char_boundary(index) {
            index += 1;
            continue;
        }
        let rest = &text[index..];
        let hit = prefixed_at(rest)
            .or_else(|| aws_key_at(rest))
            .or_else(|| private_key_at(rest))
            .or_else(|| url_password_at(rest));
        if let Some((kind, length)) = hit {
            found.push(Match {
                start: index,
                end: index + length,
                kind,
            });
            index += length;
        } else {
            index += 1;
        }
    }
    found
}

/// The first credential shape in `text`, if any.
#[must_use]
pub fn first(text: &str) -> Option<SecretKind> {
    find_all(text).first().map(|hit| hit.kind)
}

/// Replaces every recognized credential with a placeholder naming its shape.
///
/// Used where failing closed is worse than disclosing that something was
/// removed: error envelopes and structured output are generated rather than
/// authored, and a command that refused to render its own error would leave
/// the caller with nothing to act on.
#[must_use]
pub fn redact(text: &str) -> String {
    let matches = find_all(text);
    if matches.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for hit in matches {
        out.push_str(&text[cursor..hit.start]);
        out.push_str(PLACEHOLDER_OPEN);
        out.push_str(match hit.kind {
            SecretKind::GitHubToken => "github-token",
            SecretKind::GitLabToken => "gitlab-token",
            SecretKind::SlackToken => "slack-token",
            SecretKind::AwsAccessKeyId => "aws-access-key-id",
            SecretKind::NpmToken => "npm-token",
            SecretKind::ApiKey => "api-key",
            SecretKind::PrivateKey => "private-key",
            SecretKind::UrlPassword => "url-password",
        });
        out.push(']');
        cursor = hit.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Redacts every string inside a JSON value, in place.
///
/// Object keys are left alone: a key is a field name the caller chose, and
/// rewriting it would change the shape of a document a program is parsing.
pub fn redact_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) if !find_all(text).is_empty() => {
            *text = redact(text);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_json(item);
            }
        }
        serde_json::Value::Object(fields) => {
            for (_, field) in fields.iter_mut() {
                redact_json(field);
            }
        }
        _ => {}
    }
}

/// Refuses `text` when it carries a recognized credential.
///
/// `field` names where the value was found — a JSON-ish path such as
/// `handoff.implementation_decisions[1]` — so an author can find it without
/// the message repeating what it refused.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicySensitiveValue`] naming the field and the shape.
pub fn refuse_secret(field: &str, text: &str) -> Result<(), HarnessError> {
    if let Some(kind) = first(text) {
        return Err(HarnessError::Control {
            reason: format!(
                "`{field}` contains what looks like {}; a control record is committed history and cannot be redacted afterwards. Remove the value and rotate it — it has been on this machine in plaintext",
                kind.label()
            ),
            code: ErrorCode::PolicySensitiveValue,
        });
    }
    Ok(())
}

/// Refuses every field in `fields`, which pairs a path with its text.
///
/// # Errors
///
/// Returns the first violation, so the order callers pass fields in is the
/// order an author fixes them.
pub fn refuse_secrets<'a>(
    fields: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), HarnessError> {
    for (field, text) in fields {
        refuse_secret(field, text)?;
    }
    Ok(())
}

/// Environment variable names whose *value* is a credential by definition.
///
/// A gate's `environment.set` map holds literal values and is committed with
/// the definition, which makes it the one field in the whole schema whose
/// purpose is to carry something a process needs at runtime. `ALWAYS_DENIED`
/// in [`crate::domain::gate`] covers eight exact names; this catches the
/// project-specific ones by shape, and the remedy is always the same — name it
/// in `allow` and let the host supply it.
const CREDENTIAL_NAME_MARKERS: [&str; 7] = [
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "PRIVATE_KEY",
    "API_KEY",
];

/// Whether an environment variable name announces that its value is a secret.
#[must_use]
pub fn is_credential_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    CREDENTIAL_NAME_MARKERS
        .iter()
        .any(|marker| upper.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issuer_prefixed_tokens_are_recognized() {
        for (text, kind) in [
            (
                "ghp_0123456789abcdef0123456789abcdef0123",
                SecretKind::GitHubToken,
            ),
            (
                "github_pat_11ABCDEFG0abcdefghijklmnop",
                SecretKind::GitHubToken,
            ),
            ("glpat-ABCDEFGHIJKLMNOPQRSTUV", SecretKind::GitLabToken),
            ("xoxb-1234567890-abcdefghij", SecretKind::SlackToken),
            (
                "npm_abcdefghijklmnopqrstuvwxyz0123456789",
                SecretKind::NpmToken,
            ),
            ("sk-ant-api03-abcdefghijklmnopqrst", SecretKind::ApiKey),
            ("AKIAIOSFODNN7EXAMPLE", SecretKind::AwsAccessKeyId),
        ] {
            assert_eq!(first(text), Some(kind), "{text} should be {kind:?}");
            assert_eq!(
                first(&format!("the value is {text} and that is all")),
                Some(kind),
                "and it is still found in surrounding prose"
            );
        }
    }

    #[test]
    fn ordinary_evidence_is_not_flagged() {
        // The control repository is mostly digests, SHAs, and paths. A scanner
        // that trips on these would be turned off within a day, so the cases
        // it must stay quiet about are pinned as tightly as the ones it
        // catches.
        for benign in [
            "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "016578a1bb3fc8d4729d18f0000000000000000",
            "src/policy/hygiene.rs:214",
            "the sk- prefix identifies a vendor key",
            "run `cargo test` in the worktree",
            "AKIA is the prefix AWS uses",
            "https://github.com/artana-bio/solo-dev.git",
            "postgres://localhost:5432/db",
        ] {
            assert_eq!(first(benign), None, "`{benign}` must not be flagged");
        }
    }

    #[test]
    fn a_private_key_block_is_matched_whole() {
        let pem =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\n";
        let text = format!("before\n{pem}after");
        assert_eq!(first(&text), Some(SecretKind::PrivateKey));

        let redacted = redact(&text);
        assert!(redacted.starts_with("before\n"), "{redacted}");
        assert!(redacted.ends_with("after"), "{redacted}");
        assert!(
            !redacted.contains("MIIEowIBAAKCAQEA"),
            "the key material must not survive: {redacted}"
        );
    }

    #[test]
    fn an_unterminated_private_key_is_still_removed() {
        let text = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEA";
        assert_eq!(first(text), Some(SecretKind::PrivateKey));
        assert!(!redact(text).contains("b3BlbnNzaC1rZXktdjEA"));
    }

    #[test]
    fn a_password_in_a_url_is_recognized_but_a_port_is_not() {
        assert_eq!(
            first("https://deploy:hunter2@internal.example/repo.git"),
            Some(SecretKind::UrlPassword)
        );
        assert_eq!(first("https://internal.example:8443/repo.git"), None);
        assert_eq!(first("ssh://git@github.com/o/r.git"), None);

        let redacted = redact("clone https://deploy:hunter2@internal.example/repo.git now");
        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(redacted.contains("internal.example/repo.git"), "{redacted}");
    }

    #[test]
    fn redaction_preserves_everything_around_the_value() {
        let text = "first ghp_0123456789abcdef0123456789abcdef0123 then AKIAIOSFODNN7EXAMPLE end";
        let redacted = redact(text);
        assert_eq!(
            redacted,
            "first [redacted:github-token] then [redacted:aws-access-key-id] end"
        );
    }

    #[test]
    fn redaction_is_deterministic_and_idempotent() {
        let text = "token ghp_0123456789abcdef0123456789abcdef0123 here";
        let once = redact(text);
        assert_eq!(once, redact(text), "the same input always redacts the same");
        assert_eq!(
            once,
            redact(&once),
            "a redacted record has nothing left to remove"
        );
    }

    #[test]
    fn a_refusal_names_the_field_and_the_shape_but_never_the_value() {
        let secret = "ghp_0123456789abcdef0123456789abcdef0123";
        let error = refuse_secret("handoff.assumptions[0]", secret).unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("handoff.assumptions[0]"), "{rendered}");
        assert!(rendered.contains("a GitHub token"), "{rendered}");
        assert!(
            !rendered.contains(secret),
            "the refusal must not repeat the value it refused: {rendered}"
        );
        assert_eq!(error.code(), ErrorCode::PolicySensitiveValue);
    }

    #[test]
    fn clean_text_passes() {
        assert!(refuse_secret("card.goal", "Make handoff refuse a dirty tree").is_ok());
        assert!(
            refuse_secrets([
                ("card.goal", "ordinary prose"),
                ("card.rollback_strategy", "revert the landing commit"),
            ])
            .is_ok()
        );
    }

    #[test]
    fn credential_shaped_variable_names_are_recognized() {
        for name in ["NPM_TOKEN", "my_api_key", "DEPLOY_SECRET", "db_password"] {
            assert!(is_credential_name(name), "{name}");
        }
        for name in ["PATH", "HOME", "CARGO_TERM_COLOR", "RUST_BACKTRACE"] {
            assert!(!is_credential_name(name), "{name}");
        }
    }
}
