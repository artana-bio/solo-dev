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
/// Trimmed and case-folded, because `reviewer-b`, `reviewer-b `, and
/// `Reviewer-B` are one person every time they differ, and a separation check
/// that a trailing space defeats is not a check. Only the comparison is
/// normalized — records keep the label exactly as it was declared, so an
/// auditor reads what was typed rather than what was matched.
#[must_use]
pub fn normalize(actor: &str) -> String {
    actor.trim().to_lowercase()
}

/// Whether two declared identifiers name the same actor.
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
/// # Errors
///
/// Returns [`ErrorCode::PolicySameActor`] naming the role, the actor, and the
/// card they wrote.
pub fn refuse_author_acting_as<'a>(
    role: &str,
    actor: &str,
    subject: &str,
    implementers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), HarnessError> {
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
