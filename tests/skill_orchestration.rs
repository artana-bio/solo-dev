//! Pins the portable operating guide's multi-agent assignment boundary.
//!
//! The Harness can mechanically bind a card, worktree, handoff, review, and
//! promotion without making the prompts around those objects safe. This test
//! keeps the guide from regressing to nominal role separation while dropping
//! the concrete implementation packet, fresh-review context rule, or reporting
//! contract that makes the separation operational.

const ORCHESTRATION_HEADING: &str = "## Multi-agent orchestration";
const IMPLEMENTER_PROMPT_HEADING: &str = "### Start an implementer task";
const REVIEWER_PROMPT_HEADING: &str = "### Start a genuinely fresh reviewer task";

fn skill_md() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/SKILL.md");
    std::fs::read_to_string(path).expect("SKILL.md should be readable at the repository root")
}

fn section<'a>(source: &'a str, heading: &str, next_level_prefix: &str) -> &'a str {
    let start = source
        .find(heading)
        .unwrap_or_else(|| panic!("SKILL.md should contain {heading:?}"));
    let body = &source[start + heading.len()..];
    let end = body
        .find(&format!("\n{next_level_prefix}"))
        .unwrap_or(body.len());
    &body[..end]
}

fn text_prompt<'a>(source: &'a str, heading: &str) -> &'a str {
    let body = section(source, heading, "### ");
    let fence = body
        .find("```text\n")
        .unwrap_or_else(|| panic!("{heading:?} should carry a fenced text prompt"));
    let prompt = &body[fence + "```text\n".len()..];
    let end = prompt
        .find("\n```")
        .unwrap_or_else(|| panic!("the text prompt under {heading:?} should be terminated"));
    &prompt[..end]
}

fn assert_contains_all(haystack: &str, required: &[&str], subject: &str) {
    for needle in required {
        assert!(
            haystack.contains(needle),
            "{subject} should contain {needle:?}"
        );
    }
}

#[test]
fn orchestration_defines_card_slicing_and_work_distribution() {
    let guide = skill_md();
    let body = section(&guide, ORCHESTRATION_HEADING, "## ");

    assert_contains_all(
        body,
        &[
            "Convert a user request into cards",
            "Plan before executing",
            "complete card set currently known for the\ncycle",
            "does not let agents create cards opportunistically while coding",
            "one activated card, one lease, one\nallocated worktree, and one feature actor",
            "independently reviewable outcome",
            "ownership, dependency, and\nexclusive-resource boundaries",
            "use serial execution when a declared dependency or exclusive resource",
            "use parallel execution only for cards with accepted disjoint ownership",
            "The coordinator owns fan-out and fan-in",
            "Coordinator integrates only exact candidates whose Harness state is approved",
        ],
        "multi-agent orchestration section",
    );
}

#[test]
fn implementer_prompt_carries_the_complete_assignment_and_reporting_contract() {
    let guide = skill_md();
    let prompt = text_prompt(&guide, IMPLEMENTER_PROMPT_HEADING);

    assert_contains_all(
        prompt,
        &[
            "Role: Implementer for card",
            "Control repository:",
            "baseline:",
            "digest:",
            "Lease:",
            "Worktree:",
            "Complete assigned context:",
            "Work only in the allocated worktree",
            "Do not widen scope or delegate part of this card",
            "Progress:",
            "Clarification:",
            "Blocked:",
            "Complete:",
            "This packet is the complete assigned context",
        ],
        "implementer prompt",
    );
}

#[test]
fn reviewer_prompt_requires_fresh_uncontaminated_read_only_review() {
    let guide = skill_md();
    let body = section(&guide, REVIEWER_PROMPT_HEADING, "### ");
    let prompt = text_prompt(&guide, REVIEWER_PROMPT_HEADING);

    assert_contains_all(
        body,
        &[
            "not forked,\ncloned, resumed, or summarized from the implementer task",
            "the review is not\nindependent: do not record it as approval",
            "exact baseline and candidate SHAs",
            "complete diff",
            "receipts",
            "authoritative prior\nfindings",
        ],
        "fresh reviewer procedure",
    );
    assert_contains_all(
        prompt,
        &[
            "Role: Independent reviewer",
            "Run `review begin` yourself",
            "new task with no inherited implementation conversation",
            "Do not request or inspect the implementer's conversation",
            "Do not edit the candidate branch",
            "Do not assume approval is desired",
            "Run at least one narrow mutation the implementer did not declare",
            "decision, findings first",
        ],
        "reviewer prompt",
    );
}
