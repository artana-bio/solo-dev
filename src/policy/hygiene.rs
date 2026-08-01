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

use std::fmt::Write as _;

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

/// The label of a PEM armour line, when `line` is one.
///
/// A whole line, trimmed, that opens and closes with five dashes. Nothing is
/// searched across a line boundary, which is the point: three review rounds
/// produced four defects in this detector — a CRLF key with no findable end,
/// a mismatched footer ending the block early, a footer accepted because it
/// merely *contained* the label, and closing dashes found on some later line
/// so that ordinary prose became a key. Every one of them was a substring
/// search wandering past the end of its line. Parsing line-wise removes the
/// class rather than the four instances.
fn armour_label<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let trimmed = line.trim();
    let body = trimmed.strip_prefix("-----")?.strip_suffix("-----")?;
    let label = body.trim().strip_prefix(keyword)?.trim();
    Some(label)
}

/// Length of a PEM private key block starting at `rest`.
///
/// The whole block is one match. Redacting only the header would leave the key
/// material behind, which is the opposite of the point.
fn private_key_at(rest: &str) -> Option<(SecretKind, usize)> {
    if !rest.starts_with("-----BEGIN") {
        return None;
    }
    // Line-wise from here. The header must be one complete armour line, so a
    // `-----BEGIN` that never closes on its own line is ordinary prose rather
    // than a key whose redaction swallows the rest of the document.
    let mut lines = rest.split_inclusive('\n');
    let header = lines.next()?;
    let label = armour_label(header, "BEGIN")?;
    if !label.contains("PRIVATE KEY") {
        return None;
    }

    // The footer closes *this* block: its label must equal the header's, not
    // merely contain it. `END CERTIFICATE RSA PRIVATE KEY` contains
    // `RSA PRIVATE KEY` and closed the block early, leaving the material after
    // it in the clear — a partial redaction that reads as a complete one.
    let mut consumed = header.len();
    for line in lines {
        consumed += line.len();
        if armour_label(line, "END").is_some_and(|closing| closing == label) {
            return Some((SecretKind::PrivateKey, consumed));
        }
    }

    // Unterminated, or closed only by a footer for something else: redact to
    // the end of the text. A truncated key is not a safe key.
    Some((SecretKind::PrivateKey, rest.len()))
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

/// Refuses a document about to become part of control history.
///
/// This is the guarantee; the per-record `validate` calls are a courtesy. Two
/// review rounds established that enumerating the fields to scan does not
/// converge: the first missed `contract_reads` and `contract_changes`, the
/// rebuild missed `change_kind`, `review_policy`, and a draft's gate names,
/// and the round after that found cycle objectives, release invariants, and
/// integration-review residual risks still open. Each was the same mistake —
/// a list written from memory of a schema — and each was found by someone
/// probing rather than by the list.
///
/// So the check moved to the one place every durable record passes through.
/// A field added to any schema is covered the day it is added, by nobody
/// remembering anything.
///
/// `relative` names the document; the reported path is the JSON pointer within
/// it, so a refusal still says exactly where to look. Content that is not JSON
/// is scanned as text — the location is coarser, and refusing is what matters.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicySensitiveValue`] naming the document and field.
pub fn refuse_secrets_in_document(relative: &str, contents: &str) -> Result<(), HarnessError> {
    // Above this, parse into a tree and walk it is the wrong shape of work:
    // a 100 MiB document took thirteen seconds, on a check that every control
    // write in the lifecycle passes through. No durable record here is
    // remotely this size, so the cap costs nothing real and bounds the worst
    // case. The scan still happens — the fallback is a single linear pass over
    // the text, which loses the field path and keeps the guarantee.
    const PARSE_LIMIT_BYTES: usize = 4 * 1024 * 1024;

    if contents.len() > PARSE_LIMIT_BYTES {
        return refuse_secret(relative, contents);
    }
    match serde_json::from_str::<serde_json::Value>(contents) {
        Ok(value) => {
            let mut at = String::new();
            if let Some((path, kind)) = first_in_json(&value, &mut at) {
                return Err(sensitive(&format!("{relative}:{path}"), kind));
            }
            Ok(())
        }
        Err(_) => refuse_secret(relative, contents),
    }
}

/// The first credential in a JSON document, with its path.
fn first_in_json(value: &serde_json::Value, at: &mut String) -> Option<(String, SecretKind)> {
    match value {
        serde_json::Value::String(text) => first(text).map(|kind| (at.clone(), kind)),
        serde_json::Value::Array(items) => items.iter().enumerate().find_map(|(index, item)| {
            let restore = at.len();
            write!(at, "[{index}]").expect("writing to a String cannot fail");
            let found = first_in_json(item, at);
            at.truncate(restore);
            found
        }),
        serde_json::Value::Object(fields) => fields.iter().find_map(|(name, field)| {
            let restore = at.len();
            if !at.is_empty() {
                at.push('.');
            }
            at.push_str(name);
            // The key too, not only the value. A gate whose `environment.set`
            // used a credential as the *variable name* and something ordinary
            // as the value was accepted and committed: the walk visited values
            // and a key is just as much author-supplied text.
            let found = first(name)
                .map(|kind| (at.clone(), kind))
                .or_else(|| first_in_json(field, at));
            at.truncate(restore);
            found
        }),
        _ => None,
    }
}

/// The refusal every hygiene check raises.
///
/// The field path is redacted too. That looks paranoid until a credential is
/// used as a JSON *key*, at which point the path naming where the value sits
/// is the value — and the refusal quoting it becomes the leak it exists to
/// prevent. Found by the test for key scanning, one line after adding it.
fn sensitive(field: &str, kind: SecretKind) -> HarnessError {
    HarnessError::Control {
        reason: format!(
            "`{}` contains what looks like {}; a control record is committed history and cannot be redacted afterwards. Remove the value and rotate it — it has been on this machine in plaintext",
            redact(field),
            kind.label()
        ),
        code: ErrorCode::PolicySensitiveValue,
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
    match first(text) {
        Some(kind) => Err(sensitive(field, kind)),
        None => Ok(()),
    }
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

/// Splits a variable name into upper-cased word parts.
///
/// `NPM_TOKEN` and `npmToken` both become `[NPM, TOKEN]`, so naming style does
/// not decide whether a credential is recognized.
fn name_words(name: &str) -> Vec<String> {
    let characters: Vec<char> = name.chars().collect();
    let mut words = Vec::new();
    let mut current = String::new();
    for (index, value) in characters.iter().enumerate() {
        if !value.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        let previous = index.checked_sub(1).map(|at| characters[at]);
        // Two boundaries, not one. `npmToken` breaks at the lower-to-upper
        // step; `myAPIKey` breaks between the run of capitals and the word
        // that follows it, which the first rule alone reads as one long word —
        // so `APIKey` was accepted as an ordinary name until a reviewer tried
        // it.
        let after_lower =
            previous.is_some_and(|value| value.is_ascii_lowercase()) && value.is_ascii_uppercase();
        let starts_a_word = previous.is_some_and(|value| value.is_ascii_uppercase())
            && value.is_ascii_uppercase()
            && characters
                .get(index + 1)
                .is_some_and(char::is_ascii_lowercase);
        if (after_lower || starts_a_word) && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.push(value.to_ascii_uppercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Whether an environment variable name announces that its value is a secret.
///
/// Matched on whole words rather than as a substring. Substring matching read
/// `TOKENIZER` as a credential and refused an ordinary variable, and a check
/// that fires on innocuous names is a check somebody turns off.
#[must_use]
pub fn is_credential_name(name: &str) -> bool {
    let words = name_words(name);
    CREDENTIAL_NAME_MARKERS.iter().any(|marker| {
        let marker: Vec<&str> = marker.split('_').collect();
        words
            .windows(marker.len())
            .any(|window| window.iter().zip(&marker).all(|(word, part)| word == part))
    })
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
    fn a_private_key_with_crlf_endings_does_not_swallow_the_text_after_it() {
        // Regression, RV-000036. The end marker was searched for as `-----\n`,
        // which a CRLF key never contains, so the span fell back to the end of
        // input and redaction deleted every character after the block — losing
        // ordinary text while reporting success. Worse than missing a secret:
        // it destroys the surrounding record.
        let text = "before\r\n-----BEGIN RSA PRIVATE KEY-----\r\nMIIEowIBAAKCAQEA\r\n-----END RSA PRIVATE KEY-----\r\nafter";
        assert_eq!(first(text), Some(SecretKind::PrivateKey));

        let redacted = redact(text);
        assert!(
            redacted.contains("after"),
            "the text following the block must survive: {redacted:?}"
        );
        assert!(redacted.starts_with("before\r\n"), "{redacted:?}");
        assert!(
            !redacted.contains("MIIEowIBAAKCAQEA"),
            "and the key material must not: {redacted:?}"
        );
    }

    #[test]
    fn a_mismatched_footer_does_not_end_the_block_early() {
        // Regression, second review round. Taking the first `-----END` stopped
        // at a footer belonging to something else and left the key material
        // after it in the clear — a partial redaction that reads as a complete
        // one, which is worse than none.
        let text = concat!(
            "-----BEGIN RSA PRIVATE KEY-----\n",
            "FIRSTKEYMATERIAL\n",
            "-----END CERTIFICATE-----\n",
            "SECONDKEYMATERIAL\n",
            "-----END RSA PRIVATE KEY-----\n",
            "after"
        );
        let redacted = redact(text);
        assert!(
            !redacted.contains("FIRSTKEYMATERIAL") && !redacted.contains("SECONDKEYMATERIAL"),
            "the block runs to its own footer: {redacted:?}"
        );
        assert!(redacted.ends_with("after"), "{redacted:?}");
    }

    #[test]
    fn a_footer_whose_label_merely_contains_the_headers_does_not_close_the_block() {
        // Third review round. `contains` accepted
        // `END CERTIFICATE RSA PRIVATE KEY` as the footer for an
        // `RSA PRIVATE KEY` header, ending the block early and leaving the
        // material after it in the clear. The labels must be equal.
        let text = concat!(
            "-----BEGIN RSA PRIVATE KEY-----\n",
            "FIRSTMATERIAL\n",
            "-----END CERTIFICATE RSA PRIVATE KEY-----\n",
            "SECONDMATERIAL\n",
            "-----END RSA PRIVATE KEY-----\n",
            "after"
        );
        let redacted = redact(text);
        assert!(
            !redacted.contains("FIRSTMATERIAL") && !redacted.contains("SECONDMATERIAL"),
            "neither block's material may survive: {redacted:?}"
        );
        assert!(redacted.ends_with("after"), "{redacted:?}");
    }

    #[test]
    fn a_begin_line_that_closes_on_a_later_line_is_not_a_header() {
        // Third review round. Closing dashes were searched across following
        // lines, so ordinary prose beginning with the marker could pick up a
        // `-----` from anywhere below and redact everything between. Parsing
        // is line-wise now, so a header is a header or it is nothing.
        let text = "-----BEGIN of a sentence about a PRIVATE KEY\nand a later line -----\nkept";
        assert_eq!(first(text), None, "not an armour line");
        assert_eq!(redact(text), text, "and nothing is removed");
    }

    #[test]
    fn a_credential_used_as_a_json_key_is_found() {
        // Third review round. A gate whose `environment.set` used the token as
        // the *variable name* and something ordinary as the value was accepted
        // and committed: the walk visited values only, and a key is just as
        // much author-supplied text.
        let document = serde_json::json!({
            "environment": { "set": { "ghp_0123456789abcdef0123456789abcdef0123": "value" } }
        })
        .to_string();

        let error = refuse_secrets_in_document("gates/gate.x.json", &document).unwrap_err();
        assert_eq!(error.code(), ErrorCode::PolicySensitiveValue);
        assert!(!error.to_string().contains("ghp_0123"), "and never echoed");
    }

    #[test]
    fn an_oversized_document_is_still_scanned_without_being_parsed() {
        // Third review round measured thirteen seconds for a 100 MiB document
        // on a check every control write passes through. Past the cap the scan
        // is one linear pass instead of a parse and a tree walk: the field path
        // is lost, the guarantee is not.
        let mut huge = String::with_capacity(5 * 1024 * 1024);
        huge.push_str("{\"padding\":\"");
        while huge.len() < 5 * 1024 * 1024 {
            huge.push('x');
        }
        huge.push_str("ghp_0123456789abcdef0123456789abcdef0123\"}");

        let error = refuse_secrets_in_document("cards/F-001.json", &huge).unwrap_err();
        assert_eq!(error.code(), ErrorCode::PolicySensitiveValue);
    }

    #[test]
    fn a_begin_line_with_no_closing_dashes_is_not_a_header() {
        // Without the armour-line check the header test ran against the whole
        // remaining input, so any later occurrence of the words satisfied it
        // and unrelated text was swallowed as a key.
        let text = "-----BEGIN and then ordinary prose mentioning a PRIVATE KEY somewhere later";
        assert_eq!(first(text), None, "not a PEM header");
        assert_eq!(redact(text), text, "and nothing is removed");
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
        for name in [
            "NPM_TOKEN",
            "my_api_key",
            "DEPLOY_SECRET",
            "db_password",
            "npmToken",
            "SERVICE-CREDENTIAL",
        ] {
            assert!(is_credential_name(name), "{name}");
        }
        for name in ["PATH", "HOME", "CARGO_TERM_COLOR", "RUST_BACKTRACE"] {
            assert!(!is_credential_name(name), "{name}");
        }
    }

    #[test]
    fn an_acronym_run_before_a_word_is_still_split() {
        // Regression, second review round. Whole-word matching fixed
        // `TOKENIZER` and introduced the mirror defect: `myAPIKey` has no
        // lower-to-upper step between `API` and `Key`, so it read as one word
        // and passed. A run of capitals ends where the next word begins.
        for name in ["myAPIKey", "APIKey", "AWSSecretValue", "myDBPassword"] {
            assert!(is_credential_name(name), "{name} must be recognized");
        }
        assert_eq!(name_words("myAPIKey"), vec!["MY", "API", "KEY"]);
        assert_eq!(name_words("APIKey"), vec!["API", "KEY"]);
        assert_eq!(name_words("NPM_TOKEN"), vec!["NPM", "TOKEN"]);
        assert_eq!(
            name_words("TOKENIZER"),
            vec!["TOKENIZER"],
            "an unbroken run stays one word"
        );
    }

    #[test]
    fn a_document_is_scanned_wherever_the_credential_sits_in_it() {
        // The boundary check is the guarantee: it does not know or care which
        // schema it is looking at, so a field added to any record is covered
        // the day it is added.
        let document = serde_json::json!({
            "schema": "harness.cycle/v1",
            "objective": "ordinary text",
            "release_invariants": ["fine", "uses ghp_0123456789abcdef0123456789abcdef0123"],
        })
        .to_string();

        let error = refuse_secrets_in_document("cycles/C-001.json", &document).unwrap_err();
        let rendered = error.to_string();
        assert!(
            rendered.contains("cycles/C-001.json:release_invariants[1]"),
            "the refusal locates it inside the document: {rendered}"
        );
        assert!(!rendered.contains("ghp_0123"), "and does not echo it");

        assert!(
            refuse_secrets_in_document("cycles/C-002.json", r#"{"objective":"clean"}"#).is_ok()
        );
        // Content that is not JSON is still scanned, coarsely.
        assert!(
            refuse_secrets_in_document("notes.txt", "ghp_0123456789abcdef0123456789abcdef0123")
                .is_err()
        );
    }

    #[test]
    fn a_marker_word_inside_a_longer_word_is_not_a_credential() {
        // Regression, RV-000036. Substring matching read `TOKENIZER` as a
        // credential and refused an ordinary variable. A check that fires on
        // innocuous names is a check somebody switches off, which costs more
        // than the case it was meant to catch.
        for benign in [
            "TOKENIZER",
            "TOKENIZER_VERSION",
            "SECRETARY_EMAIL",
            "PASSWORDLESS_MODE",
            "CREDENTIALING_URL",
        ] {
            assert!(!is_credential_name(benign), "{benign} must be allowed");
        }
        assert!(
            is_credential_name("TOKENIZER_API_KEY"),
            "a real marker beside the innocent word is still caught"
        );
    }
}
