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

/// Declared actor/session boundary used by role-separation checks.
/// Values are caller-declared; this type makes the comparison boundary
/// explicit instead of falling back to display labels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActorIdentity<'a> {
    pub actor_kind: &'a str,
    pub actor_id: &'a str,
    pub principal_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
}

impl<'a> ActorIdentity<'a> {
    #[must_use]
    pub const fn new(actor_kind: &'a str, actor_id: &'a str) -> Self {
        Self {
            actor_kind,
            actor_id,
            principal_id: None,
            session_id: None,
        }
    }

    /// Returns whether two declared roles share the same principal/session
    /// boundary. Reuse of either a principal or a session is a collision;
    /// differing sessions cannot make one principal independent.
    #[must_use]
    pub fn same_boundary(&self, other: &Self) -> bool {
        matches!(
            (self.session_id, other.session_id),
            (Some(left), Some(right)) if same(left, right)
        ) || matches!(
            (self.principal_id, other.principal_id),
            (Some(left), Some(right)) if same(left, right)
        ) || same(self.actor_id, other.actor_id)
    }
}

/// The comparable form of a declared actor identifier.
///
/// Whitespace runs collapse to one space and ASCII letters fold to lowercase,
/// because `reviewer-b`, `reviewer-b `, `Reviewer-B`, and `Alvaro  Alvarez`
/// with two spaces are one person every time they differ. Only the comparison
/// is normalized — records keep the label exactly as declared, so an auditor
/// reads what was typed rather than what was matched.
///
/// Total and unambiguous because [`refuse_unusable`] has already established
/// the identifier is ASCII. Do not call this on unvalidated input.
fn normalize(actor: &str) -> String {
    actor
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Refuses an identifier this module cannot compare safely.
///
/// **Declared actor identifiers are ASCII.** That is a restriction on what may
/// be typed, and it is deliberate, because the alternative was three rounds of
/// getting the comparison wrong:
///
/// Exact equality made `Operator` a different person from `operator`. Adding
/// simple lowercase mapping fixed that and was defeated by the small sharp s,
/// which lowercases to itself while its uppercase spelling lowercases to a
/// double s. Comparing both mappings fixed *that* and was defeated by the
/// capital sharp s, which lowercases to the small form and uppercases to
/// itself, so the relation was not even transitive — and a reviewer's
/// exhaustive sweep of every Unicode scalar found it, not the previous fix's
/// reasoning. Separately, canonically equivalent spellings and zero-width
/// characters produce identifiers that render identically and compare
/// differently.
///
/// Each fix closed the instance it was shown and left the class open. Correct
/// Unicode identity needs case folding *and* normalization, which needs tables
/// this crate does not carry. So the comparison stops trying to be clever about
/// characters it cannot reason about and refuses them instead: within ASCII,
/// `to_ascii_lowercase` is total, there is one encoding per glyph, and nothing
/// is invisible.
///
/// The cost is real. A name like `Álvaro` cannot be an actor identifier. An
/// actor id is an identifier for separation, not a display name, and refusing
/// it with a clear message beats silently treating two spellings of one person
/// as two people at the step that authorizes publishing a commit.
///
/// # Errors
///
/// Returns [`ErrorCode::PolicyIncompleteReview`] when the identifier is empty,
/// non-ASCII, or contains an ASCII control character.
pub fn refuse_unusable(role: &str, actor: &str) -> Result<(), HarnessError> {
    let trimmed = actor.trim();
    let reject = |reason: String| HarnessError::Control {
        reason,
        code: ErrorCode::PolicyIncompleteReview,
    };
    if trimmed.is_empty() {
        return Err(reject(format!("the {role} must be named")));
    }
    if !trimmed.is_ascii() {
        return Err(reject(format!(
            "the {role} `{trimmed}` is not ASCII; actor identifiers are compared for separation and must be ASCII, because outside it one name has spellings that render identically and compare differently. Use an ASCII identifier"
        )));
    }
    if trimmed.chars().any(|value| value.is_ascii_control()) {
        return Err(reject(format!(
            "the {role} contains a control character; actor identifiers must be printable ASCII"
        )));
    }
    Ok(())
}

/// Whether two declared identifiers name the same actor.
///
/// Both sides must already have passed [`refuse_unusable`]; within ASCII this
/// comparison is total, which is the entire reason for that restriction.
#[must_use]
pub fn same(left: &str, right: &str) -> bool {
    normalize(left) == normalize(right)
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
    refuse_unusable(&format!("{role} for {subject}"), actor)?;
    for (card_id, implementer) in implementers {
        // Both sides, because a comparison is only as total as its worse half:
        // an ASCII acceptance owner against a non-ASCII implementer is exactly
        // the shape that let an author bless their own work.
        refuse_unusable(&format!("implementer of {card_id}"), implementer)?;
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
    fn every_case_variant_that_defeated_this_check_is_now_refused() {
        // Three rounds of fixes each closed the character they were shown.
        // Exact equality lost to `Operator`; simple lowercase lost to the small
        // sharp s; comparing both mappings lost to the capital sharp s, whose
        // orbit it split non-transitively. The class is closed by refusing what
        // cannot be compared rather than by widening what is compared, so every
        // row below is a refusal and none depends on having enumerated it.
        for identifier in [
            "Stra\u{df}e",                  // small sharp s
            "STRA\u{1e9e}E",                // capital sharp s, the round-7 bypass
            "\u{130}zmir",                  // Turkish dotted I
            "\u{3bf}\u{3b4}\u{3cc}\u{3c2}", // Greek final sigma
            "caf\u{e9}",                    // composed
            "cafe\u{301}",                  // decomposed, renders identically
            "alv\u{200b}aro",               // zero-width space, invisible
            "\u{430}lvaro",                 // Cyrillic homoglyph
        ] {
            assert!(
                refuse_unusable("acceptance owner", identifier).is_err(),
                "{identifier:?} must be refused, not compared"
            );
        }
    }

    #[test]
    fn ordinary_ascii_identifiers_still_compare_as_people_expect() {
        assert!(same("Operator", "OPERATOR"));
        assert!(same("reviewer-b ", " Reviewer-B"));
        assert!(same("Alvaro  Alvarez", "alvaro alvarez"));
        assert!(!same("reviewer-b", "reviewer-c"));
        for usable in ["operator", "Alvaro Alvarez", "reviewer-b", "o"] {
            assert!(refuse_unusable("reviewer", usable).is_ok(), "{usable}");
        }
    }

    #[test]
    fn an_unnamed_or_unprintable_actor_is_refused() {
        for blank in ["", "   ", "\t"] {
            let error = refuse_unusable("acceptance owner", blank).unwrap_err();
            assert_eq!(error.code(), ErrorCode::PolicyIncompleteReview, "{blank:?}");
        }
        assert!(refuse_unusable("acceptance owner", "oper\u{7}ator").is_err());
    }

    #[test]
    fn a_non_ascii_implementer_cannot_be_compared_away() {
        // The other half: an ASCII blesser against a non-ASCII implementer is
        // the shape that let an author bless their own work, so both sides are
        // validated rather than only the one being challenged.
        let error = refuse_author_acting_as(
            "acceptance owner",
            "owner",
            "INT-001",
            [("F-001", "STRA\u{1e9e}E")],
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::PolicyIncompleteReview);
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
