//! Declared-actor comparison and the role-separation policy.
//!
//! Every actor in this harness is a string somebody typed. D-013 accepts that
//! deliberately: same-user local operation is not a security boundary, and
//! pretending a `--actor` flag proves identity would be worse than admitting
//! it does not. What these rules catch is the mistake, not the adversary.
//!
//! That is still worth having. The lifecycle's whole value is that the party
//! who wrote a change is not the party who blesses it, and until now the
//! harness checked that at the two review steps and nowhere else — including
//! at acceptance, the one step that authorizes moving the protected branch.
//!
//! # The separation policy
//!
//! Two rules, and one deliberate non-rule:
//!
//! 1. **Authorization is not self-granted.** The acceptance owner must differ
//!    from every implementer whose card the integration carries. Accepting
//!    your own work is the case acceptance exists to prevent.
//! 2. **Execution is not performed by the author.** The promoter must differ
//!    from those same implementers, for the same reason one step later.
//! 3. **The authorizer may execute their own decision.** The promoter is
//!    allowed to be the acceptance owner, and this is not an oversight.
//!    Section 15.1's operating model is one human and many agent sessions:
//!    the human accepts, and the human runs the promote. Requiring a fourth
//!    distinct party there would make the documented model impossible to
//!    follow, which is how a control gets worked around rather than kept.
//!
//! Identity is declared, so all three are refusals about *declarations*. An
//! implementer who accepts their own integration under a different name is
//! not stopped by this and is not meant to be; Q-004 is where that becomes
//! answerable.

use crate::error::{ErrorCode, HarnessError};

/// The comparable form of a declared actor identifier.
///
/// Trimmed and lowercased, because `reviewer-b`, `reviewer-b `, and
/// `Reviewer-B` are one person every time they differ, and a separation check
/// that a trailing space defeats is not a check. Only the comparison is
/// normalized — records keep the label exactly as it was declared, so an
/// auditor reads what was typed rather than what was matched.
///
/// This is *simple lowercase mapping*, not case folding, and on its own it is
/// not sufficient; see [`same`].
#[must_use]
pub fn normalize(actor: &str) -> String {
    actor.trim().to_lowercase()
}

/// Whether two declared identifiers name the same actor.
///
/// Lowercase **or** uppercase agreement, not lowercase alone. Rust's
/// `to_lowercase` is Unicode simple lowercase mapping, which is not case
/// folding: an identifier containing the German sharp s lowercases to itself
/// while its uppercase spelling lowercases to a double s, so two spellings of
/// one name compared unequal. A reviewer drove a full lifecycle with such an
/// identifier and recorded both an acceptance and a promotion under the other
/// spelling — the guarantee this module exists to provide, defeated by a case
/// variant, and introduced by the normalization added to fix a *different*
/// case-sensitivity defect.
///
/// Uppercase catches that pair where lowercase does not, and the reverse holds
/// for other scripts, so both are compared. Proper case folding needs a
/// Unicode table this crate does not carry; agreeing on either mapping is a
/// deliberate over-approximation, and over-matching is the safe direction. A
/// false match refuses two genuinely distinct people and is fixed by choosing
/// a more distinct identifier; a false mismatch lets an author bless their own
/// work.
#[must_use]
pub fn same(left: &str, right: &str) -> bool {
    let (left, right) = (left.trim(), right.trim());
    left.to_lowercase() == right.to_lowercase() || left.to_uppercase() == right.to_uppercase()
}

/// Refuses when the actor taking `role` also produced one of the changes.
///
/// `implementers` is every feature actor whose card the integration carries;
/// duplicates are harmless. `subject` names what is being acted on, so the
/// refusal says which integration rather than only which person.
///
/// An unnamed actor is refused outright. `check_independence` already refuses
/// a review that names no reviewer, and the step that authorizes moving the
/// protected branch held itself to a lower standard than the one before it —
/// an acceptance could be recorded, and a promotion run, under an empty owner.
/// That is not a bypass beyond what D-013 discloses, since an empty string is
/// simply a second declared name, but a record authorizing a published commit
/// should not be able to name nobody.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicySameActor`] naming the role, the actor, and the
/// card they wrote, or [`ErrorCode::PolicyIncompleteReview`] when the actor is
/// unnamed.
pub fn refuse_author_acting_as<'a>(
    role: &str,
    actor: &str,
    subject: &str,
    implementers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), HarnessError> {
    if actor.trim().is_empty() {
        return Err(HarnessError::Control {
            reason: format!("the {role} for {subject} must be named"),
            code: ErrorCode::PolicyIncompleteReview,
        });
    }
    for (card_id, implementer) in implementers {
        if same(actor, implementer) {
            return Err(HarnessError::Control {
                reason: format!(
                    "`{actor}` implemented {card_id} and cannot also be the {role} for {subject}; the party who wrote a change does not get to bless it. Identity here is declared rather than proven, so this refuses the obvious case only"
                ),
                code: ErrorCode::PolicySameActor,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_ignores_surrounding_space_and_case() {
        assert!(same("reviewer-b", "reviewer-b"));
        assert!(same("reviewer-b ", "reviewer-b"));
        assert!(same(" Reviewer-B", "reviewer-b\t"));
        assert!(!same("reviewer-b", "reviewer-c"));
        assert!(
            !same("reviewer b", "reviewer-b"),
            "normalization tidies whitespace at the edges, it does not rewrite the identifier"
        );
    }

    #[test]
    fn an_author_cannot_take_a_blessing_role_over_their_own_card() {
        let error = refuse_author_acting_as(
            "acceptance owner",
            " Implementer-A ",
            "INT-001",
            [("F-001", "implementer-a"), ("F-002", "implementer-b")],
        )
        .unwrap_err();

        assert_eq!(error.code(), ErrorCode::PolicySameActor);
        let rendered = error.to_string();
        assert!(rendered.contains("F-001"), "names the card: {rendered}");
        assert!(
            rendered.contains("acceptance owner"),
            "names the role: {rendered}"
        );
    }

    #[test]
    fn a_non_ascii_case_variant_is_the_same_actor() {
        // Regression, RV-000038, the one exploitable finding. `to_lowercase`
        // is simple lowercase mapping, not case folding: the German sharp s
        // lowercases to itself while its uppercase spelling lowercases to a
        // double s, so two spellings of one name compared unequal and a
        // reviewer recorded both an acceptance and a promotion under the other
        // one. Comparing both mappings closes it; over-matching is the safe
        // direction for a separation check.
        assert!(same("Stra\u{df}e", "STRASSE"));
        assert!(same("STRASSE", "Stra\u{df}e"));
        assert!(same("  Stra\u{df}e\t", "strasse"));
        assert!(
            refuse_author_acting_as(
                "acceptance owner",
                "STRASSE",
                "INT-001",
                [("F-001", "Stra\u{df}e")],
            )
            .is_err(),
            "the case variant must not reach the authorizing step"
        );
        // Genuinely different names still differ.
        assert!(!same("Stra\u{df}e", "Strasser"));
    }

    #[test]
    fn an_unnamed_actor_is_refused() {
        // `check_independence` refuses a review that names no reviewer; the
        // step that authorizes moving the protected branch did not, so an
        // acceptance could be recorded and a promotion run under an empty
        // owner.
        for blank in ["", "   ", "\t"] {
            let error =
                refuse_author_acting_as("acceptance owner", blank, "INT-001", [("F-001", "a")])
                    .unwrap_err();
            assert_eq!(error.code(), ErrorCode::PolicyIncompleteReview, "{blank:?}");
        }
    }

    #[test]
    fn an_unrelated_actor_passes() {
        assert!(
            refuse_author_acting_as(
                "promoter",
                "alvaro",
                "INT-001",
                [("F-001", "implementer-a"), ("F-002", "codex")],
            )
            .is_ok()
        );
    }

    #[test]
    fn an_integration_with_no_members_cannot_refuse_anyone() {
        // Vacuous rather than an error: whether an integration may be empty is
        // a question for the integration model, not for this policy.
        assert!(refuse_author_acting_as("promoter", "anyone", "INT-001", []).is_ok());
    }
}
