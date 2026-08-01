//! Advisory signals that a card is too big, or is not converging.
//!
//! Both are **report-only** and cannot be made to block. That is the design,
//! not a limitation. Counting rounds and findings is mechanical; deciding to
//! split a card is judgment, and a harness that split cards automatically
//! would be making a product decision from a file count. What it can do is
//! stop the signal being invisible.
//!
//! It was invisible. `F-027` bundled seven unrelated issues across 24 files,
//! ran four review rounds and about seventeen findings before anyone said the
//! word "split", and the control repository held every number needed to say it
//! after round two. Nothing was missing except a line of output.
//!
//! The two checks sit at opposite ends. Scope breadth is *leading* — it asks
//! the question at activation, when the answer is cheap. Convergence is
//! *lagging* — it can only speak once rounds exist, but by then it knows
//! something the first check cannot: whether the work is actually settling.

use std::{collections::BTreeSet, fmt::Write as _};

/// How wide a card's declared scope is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeBreadth {
    /// Distinct paths the card may write.
    pub paths: usize,
    /// Distinct top-level areas those paths fall under.
    pub areas: usize,
    /// The areas themselves, in a stable order, for the message.
    pub area_names: Vec<String>,
}

/// Paths above which a card stops looking like one reviewable outcome.
///
/// Not a rule about repositories in general — a threshold this crate can
/// defend. Every card that has landed here touched fewer than this; the one
/// that did not was `F-027`, at 24, which took eight review rounds and a split.
/// A card at the boundary is asked a question, not refused.
pub const BROAD_PATH_COUNT: usize = 12;

/// Areas above which a card is probably several cards.
///
/// "One independently reviewable outcome" is the plan's phrase. A reviewer
/// holding four unrelated areas in their head at once is not reviewing one
/// outcome, whatever the card says it is about.
pub const BROAD_AREA_COUNT: usize = 4;

impl ScopeBreadth {
    /// Measures a card's include list.
    ///
    /// The area of a path is its first component — `src`, `tests`, `docs` — or
    /// its second where the first is `src`, so `src/policy/**` and
    /// `src/commands/**` count separately. That is the granularity at which
    /// this codebase's cards actually differ.
    #[must_use]
    pub fn measure(include: &[String]) -> Self {
        let mut areas = BTreeSet::new();
        for path in include {
            let trimmed = path.trim_start_matches("./");
            let mut parts = trimmed.split('/').filter(|part| !part.is_empty());
            let area = match (parts.next(), parts.next()) {
                (Some("src"), Some(second)) if !second.contains('*') => {
                    format!("src/{second}")
                }
                (Some(first), _) => first.to_owned(),
                (None, _) => continue,
            };
            areas.insert(area);
        }
        Self {
            paths: include.len(),
            areas: areas.len(),
            area_names: areas.into_iter().collect(),
        }
    }

    /// The advisory to show at activation, when there is one.
    ///
    /// Phrased as a question. The card may well be right — a mechanical rename
    /// touches fifty files and is one outcome — and the author is the one who
    /// knows.
    #[must_use]
    pub fn advisory(&self) -> Option<String> {
        if self.paths < BROAD_PATH_COUNT && self.areas < BROAD_AREA_COUNT {
            return None;
        }
        Some(format!(
            "this card declares {} path(s) across {} area(s) ({}); a card is meant to be one independently reviewable outcome, and past that breadth a reviewer is holding several at once. If it is several, splitting now is far cheaper than splitting after the review rounds",
            self.paths,
            self.areas,
            self.area_names.join(", ")
        ))
    }
}

/// One review round, reduced to what the trend needs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Round {
    /// Findings the reviewer left open.
    pub open_findings: usize,
    /// Distinct locations those findings named.
    pub locations: BTreeSet<String>,
}

impl Round {
    /// Builds a round from a review's open findings.
    #[must_use]
    pub fn new<'a>(open_locations: impl IntoIterator<Item = &'a str>) -> Self {
        let locations: BTreeSet<String> =
            open_locations.into_iter().map(ToOwned::to_owned).collect();
        Self {
            open_findings: locations.len(),
            locations,
        }
    }
}

/// What the rounds so far say about whether the card is settling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trend {
    /// How many rounds have been recorded, including this one.
    pub rounds: usize,
    /// Open findings in each round, oldest first.
    pub per_round: Vec<usize>,
    /// Locations in this round that no earlier round named.
    pub new_areas: Vec<String>,
}

/// Rounds below which a trend says nothing worth printing.
///
/// Two points is a line through any two numbers. Three is the first round
/// where "still not settling" is a statement rather than an observation.
pub const MIN_ROUNDS_FOR_TREND: usize = 3;

impl Trend {
    /// Computes the trend across every round recorded for a card.
    ///
    /// `rounds` is oldest first and includes the round being recorded.
    #[must_use]
    pub fn measure(rounds: &[Round]) -> Self {
        let seen: BTreeSet<&String> = rounds
            .iter()
            .rev()
            .skip(1)
            .flat_map(|round| round.locations.iter())
            .collect();
        let new_areas = rounds
            .last()
            .map(|latest| {
                latest
                    .locations
                    .iter()
                    .filter(|location| !seen.contains(location))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        Self {
            rounds: rounds.len(),
            per_round: rounds.iter().map(|round| round.open_findings).collect(),
            new_areas,
        }
    }

    /// True when the open-finding count is not falling across the rounds.
    ///
    /// Volume alone is not the signal. A round of twelve findings that becomes
    /// six then two is a card being finished. Four, then four, then five, is a
    /// card whose bottom nobody has found.
    #[must_use]
    pub fn is_flat(&self) -> bool {
        let Some(first) = self.per_round.first() else {
            return false;
        };
        let Some(last) = self.per_round.last() else {
            return false;
        };
        *last >= *first && *last > 0
    }

    /// The advisory to show after recording, when there is one.
    ///
    /// Silent until there is enough history to mean anything, and silent when
    /// the card is converging — a card that is nearly done should not be
    /// nagged about its size.
    #[must_use]
    pub fn advisory(&self) -> Option<String> {
        if self.rounds < MIN_ROUNDS_FOR_TREND {
            return None;
        }
        let counts: Vec<String> = self.per_round.iter().map(ToString::to_string).collect();
        let spreading = !self.new_areas.is_empty();
        if !self.is_flat() && !spreading {
            return None;
        }

        let mut reason = String::new();
        if self.is_flat() {
            reason.push_str("open findings are not falling");
        }
        if spreading {
            if !reason.is_empty() {
                reason.push_str(" and ");
            }
            write!(
                reason,
                "round {} raised {} finding(s) in area(s) no earlier round named ({})",
                self.rounds,
                self.new_areas.len(),
                self.new_areas.join(", ")
            )
            .expect("writing to a String cannot fail");
        }
        Some(format!(
            "this card is on review round {} with open findings per round of {}; {reason}. Findings that keep appearing in new places usually mean the card is several cards. Consider splitting it — this is a signal, not a refusal, and the judgment is yours",
            self.rounds,
            counts.join(" → ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn an_ordinary_card_draws_no_scope_advisory() {
        // Every card that landed cleanly in this repository is this shape.
        let narrow = ScopeBreadth::measure(&paths(&["src/policy/actors.rs", "tests/promotion.rs"]));
        assert_eq!(narrow.areas, 2);
        assert!(narrow.advisory().is_none());
    }

    #[test]
    fn the_card_this_check_exists_for_is_flagged() {
        // F-027's declared scope, verbatim: seven unrelated issues, 24 paths,
        // eight review rounds, and a split that should have happened at round
        // two. The check has to catch this one or it catches nothing.
        let f027 = ScopeBreadth::measure(&paths(&[
            ".claude/skills/change-harness/SKILL.md",
            "docs/IMPLEMENTATION_PLAN.md",
            "src/cli/output.rs",
            "src/commands/acceptance.rs",
            "src/commands/gate.rs",
            "src/commands/integration.rs",
            "src/control/repository.rs",
            "src/domain/acceptance.rs",
            "src/domain/card.rs",
            "src/domain/gate.rs",
            "src/domain/handoff.rs",
            "src/domain/review.rs",
            "src/error.rs",
            "src/main.rs",
            "src/policy/actors.rs",
            "src/policy/hygiene.rs",
            "src/policy/mod.rs",
            "src/runner/mod.rs",
            "tests/audit.rs",
            "tests/authority.rs",
            "tests/gate_registry.rs",
            "tests/promotion.rs",
            "tests/record_hygiene.rs",
            "tests/review.rs",
        ]));
        assert_eq!(f027.paths, 24);

        let advisory = f027.advisory().expect("F-027 must be flagged");
        assert!(advisory.contains("24 path(s)"), "{advisory}");
        assert!(
            advisory.contains("splitting now is far cheaper"),
            "the advisory has to say what to do about it: {advisory}"
        );
    }

    #[test]
    fn a_wide_but_shallow_card_is_flagged_on_paths_alone() {
        let many = ScopeBreadth::measure(&paths(&[
            "src/domain/a.rs",
            "src/domain/b.rs",
            "src/domain/c.rs",
            "src/domain/d.rs",
            "src/domain/e.rs",
            "src/domain/f.rs",
            "src/domain/g.rs",
            "src/domain/h.rs",
            "src/domain/i.rs",
            "src/domain/j.rs",
            "src/domain/k.rs",
            "src/domain/l.rs",
        ]));
        assert_eq!(many.areas, 1, "all one area");
        assert!(many.advisory().is_some(), "but twelve paths is broad");
    }

    #[test]
    fn a_deep_but_narrow_card_is_flagged_on_areas_alone() {
        let scattered = ScopeBreadth::measure(&paths(&[
            "src/policy/a.rs",
            "src/runner/b.rs",
            "tests/c.rs",
            "docs/d.md",
        ]));
        assert_eq!(scattered.paths, 4);
        assert_eq!(scattered.areas, 4);
        assert!(
            scattered.advisory().is_some(),
            "four areas is several cards"
        );
    }

    #[test]
    fn a_glob_under_src_counts_as_its_own_area() {
        let breadth = ScopeBreadth::measure(&paths(&["src/policy/**", "src/commands/**"]));
        assert_eq!(breadth.area_names, vec!["src/commands", "src/policy"]);
        let bare = ScopeBreadth::measure(&paths(&["src/**"]));
        assert_eq!(bare.area_names, vec!["src"], "a bare src glob is one area");
    }

    #[test]
    fn a_trend_says_nothing_before_it_can_mean_anything() {
        // Two points is a line through any two numbers.
        for rounds in 1..MIN_ROUNDS_FOR_TREND {
            let history: Vec<Round> = (0..rounds).map(|_| Round::new(["src/a.rs"])).collect();
            assert!(
                Trend::measure(&history).advisory().is_none(),
                "{rounds} round(s) is not a trend"
            );
        }
    }

    #[test]
    fn a_card_that_is_settling_is_left_alone() {
        // Twelve findings becoming six becoming one is a card being finished,
        // and nagging it about its size would train people to ignore this.
        let history = [
            Round::new(["src/a.rs", "src/b.rs", "src/c.rs"]),
            Round::new(["src/a.rs", "src/b.rs"]),
            Round::new(["src/a.rs"]),
        ];
        let trend = Trend::measure(&history);
        assert_eq!(trend.per_round, vec![3, 2, 1]);
        assert!(!trend.is_flat());
        assert!(trend.advisory().is_none());
    }

    #[test]
    fn findings_that_stay_flat_are_flagged() {
        let history = [
            Round::new(["src/a.rs", "src/b.rs"]),
            Round::new(["src/a.rs", "src/b.rs"]),
            Round::new(["src/a.rs", "src/b.rs"]),
        ];
        let advisory = Trend::measure(&history)
            .advisory()
            .expect("flat is a signal");
        assert!(advisory.contains("2 → 2 → 2"), "{advisory}");
        assert!(advisory.contains("not falling"), "{advisory}");
        assert!(
            advisory.contains("not a refusal"),
            "an advisory has to say it is advisory: {advisory}"
        );
    }

    #[test]
    fn findings_that_keep_moving_to_new_places_are_flagged_even_while_falling() {
        // The F-027 shape, and the reason volume alone is the wrong measure:
        // the count came down while every round found a defect somewhere the
        // last round had not looked.
        let history = [
            Round::new(["src/policy/hygiene.rs", "src/domain/card.rs", "src/main.rs"]),
            Round::new(["src/control/repository.rs", "src/policy/hygiene.rs"]),
            Round::new(["src/policy/actors.rs"]),
        ];
        let trend = Trend::measure(&history);
        assert!(!trend.is_flat(), "the count is falling");
        assert_eq!(trend.new_areas, vec!["src/policy/actors.rs"]);

        let advisory = trend.advisory().expect("spreading is a signal on its own");
        assert!(advisory.contains("no earlier round named"), "{advisory}");
    }

    #[test]
    fn a_round_with_nothing_open_is_not_flat() {
        // An approval closes the card. Zero open findings is the end state, not
        // a plateau, however many rounds it took to get there.
        let history = [
            Round::new(["src/a.rs"]),
            Round::new(["src/a.rs"]),
            Round::new([]),
        ];
        let trend = Trend::measure(&history);
        assert!(!trend.is_flat());
        assert!(trend.advisory().is_none());
    }

    #[test]
    fn repeated_locations_within_one_round_count_once() {
        let round = Round::new(["src/a.rs", "src/a.rs", "src/b.rs"]);
        assert_eq!(round.open_findings, 2);
    }
}
