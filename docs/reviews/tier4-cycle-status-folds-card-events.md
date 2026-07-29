# cycle-status-folds-card-events  [medium]  fix_accepted=False

## Summary
`cycle status` derives the cycle's status by folding every event attributed to the cycle — including card, work, integration and acceptance events — so a card's or an integration's state is reported as the cycle's own status, and a healthy cycle is simultaneously accused of tampering.

## Root cause
Two collaborating causes.

1. `derived_status`, src/commands/cycle.rs:191-201:
     let records = events.for_cycle(cycle_id)?;
     let transitions: Vec<&str> = records
         .iter()
         .filter_map(|event| event.next_state.as_deref())
         .collect();
     Ok(status_from_events(transitions))
   `EventStore::for_cycle` (src/control/event_store.rs:280-286) filters on `cycle_id` alone. Every command in a cycle calls `.cycle(...)` on its EventDraft so its event can be attributed to the cycle — 27 call sites across card.rs, work.rs, gate.rs, handoff.rs, review.rs, integration.rs, acceptance.rs, archive.rs. `for_cycle` therefore returns the cycle's ENTIRE subtree, not the cycle's own transitions, and `derived_status` folds all of it.

2. The state-name namespaces collide, so `parse_status` (src/domain/cycle.rs:311-323) accepts card and integration states as cycle statuses. `CycleStatus::name` (src/domain/cycle.rs:70-81) yields draft/active/integrating/accepted/landed/closed/blocked/abandoned. `CardState::name` (src/domain/card.rs:101-117) yields all eight of those plus ready/leased/handed_off/review_pending/changes_requested/approved. `IntegrationStatus::name` (src/domain/integration.rs:63-71) yields draft/accepted/blocked/abandoned among others. Every shared name is folded; every non-shared name is silently dropped, which is what makes the corruption intermittent.

The record has no field naming the event's subject: `card.created` (src/commands/card.rs:406-411) sets `.cycle(...)` but never `.card(...)`, and every `integration.*` and `acceptance.*` event sets `.cycle(...)` with no card id. So `card_id` cannot be used to identify the subject — see the overcorrection note.

Consequence for the drift check (src/commands/cycle.rs:389, 437-445): the stored `status` field only ever holds draft/active/abandoned (those are the only cycle transitions any command writes), so the polluted derived value disagrees with it constantly. The warning that exists to say "something wrote state outside the harness" fires on healthy cycles caused by the harness itself.

## Files
src/commands/cycle.rs, src/control/event_store.rs, tests/cycle_model.rs

## Proposed fix
Verified end to end in the worktree: full gate green (`cargo fmt --check` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo test` = 32 suites, 806 passed, 0 failed — 801 baseline + 5 new).

STEP 1 — name the subject. In src/control/event_store.rs, inside `impl Event` (after `relative_path`, ~line 76):

    /// True when this event transitions the cycle itself.
    ///
    /// Every event inside a cycle carries `cycle_id` so it can be attributed
    /// to the cycle; only `cycle.*` events transition the cycle. Card states
    /// share all eight cycle-status names and integration statuses share four,
    /// so folding by name alone reads a blocked card as a blocked cycle.
    #[must_use]
    pub fn is_cycle_transition(&self) -> bool {
        self.cycle_id.is_some()
            && self.card_id.is_none()
            && self.next_state.is_some()
            && self.event_type.split('.').next() == Some("cycle")
    }

The `cycle.` prefix is the load-bearing clause; `card_id.is_none()` is belt-and-braces and must NOT be used alone. Enumerated every emitted type via `grep -rho 'EventDraft::new("[^"]*"' src/`: exactly four start with `cycle.` — cycle.created, cycle.activated, cycle.abandoned (all carry next_state) and cycle.group-declared (carries none, excluded by the next_state clause). No non-cycle-subject type starts with `cycle.`.

STEP 2 — src/commands/cycle.rs:196, inside `derived_status`:

    let transitions: Vec<&str> = records
        .iter()
        .filter(|event| event.is_cycle_transition())
        .filter_map(|event| event.next_state.as_deref())
        .collect();

STEP 3 — stop losing the subject in the report. src/commands/cycle.rs:406-419, replace the history loop:

    for event in &history {
        let subject = event
            .card_id
            .as_ref()
            .map_or_else(|| cycle_id.to_string(), ToString::to_string);
        let _ = write!(
            text,
            "\n  {} {} {subject} {} -> {}",
            event.occurred_at,
            event.event_type,
            event.previous_state.as_deref().unwrap_or("none"),
            event.next_state.as_deref().unwrap_or("none")
        );
    }

STEP 4 — stop overstating what the status was derived from. Before the `format!` at line 390:

    let transitions = history.iter().filter(|e| e.is_cycle_transition()).count();

then change the summary line to `"status: {derived} (derived from {} cycle transition(s) of {} event(s))"` with `transitions, history.len()`, and add `"cycle_transition_count": transitions` to the JSON `data`. Leave `event_count` meaning what its name says (all attributed events) so `tests/cycle_model.rs:197` keeps asserting what it always did.

STEP 5 — FAILING-FIRST TESTS. Add to tests/cycle_model.rs. Each was confirmed to fail against the unfixed code at the assertion that matters (mutation: make `is_cycle_transition` return `true`, i.e. exactly today's behaviour):

  a_card_transition_is_not_the_cycles_own_status
    activate cycle, activate card F-001; assert data.status == "active" (fails: left "draft"),
    status_matches_history == true, warnings empty; then work start + work block F-001,
    assert data.status == "active" (fails: left "blocked") and warnings empty.

  an_abandoned_integration_does_not_terminate_the_cycle
    activate cycle, approve F-001, integration prepare, integration abandon;
    assert data.status == "active" (fails: left "abandoned").

  the_history_names_the_subject_of_every_transition
    text output must contain "work.started F-001 ready -> active" and
    "cycle.activated C-001 draft -> active". (Run the binary directly — the
    `Workspace::cycle_*` helpers hardcode `--output json`, and the history text
    is not in the JSON envelope.)

MUTATION EVIDENCE (all run):
  predicate -> `true` (today's code): all 14 existing tests/cycle_model.rs tests PASS; 3 of the 5 new tests fail with `left: "draft"/"abandoned", right: "active"`.
  predicate -> `self.card_id.is_none() && self.next_state.is_some()` (the obvious naive fix): all 14 existing tests still PASS; the same 3 new tests still fail, because card.created and integration.abandoned carry no card_id. This is the mutation the new tests exist to catch.

ADJACENT, NOT PART OF THIS FIX — flag separately, do not bundle:
  (a) `card.created` (src/commands/card.rs:406-411) sets `.cycle(...)` but never `.card(...)`, so `EventStore::for_card` misses a card's own creation.
  (b) `work start` returns exit 0 on a card whose cycle is abandoned (REPRO 4); no command in work.rs/handoff.rs/review.rs/gate.rs consults cycle status at all.
  (c) After this fix the derived status can only ever be draft/active/abandoned, because no command emits any other cycle transition. That is the separate register item "Five cycle statuses ... are unreachable". This fix stops card noise from disguising it; do not "fix" that by folding card events back in.

## Over-correction risk
The opposite failure is a filter that excludes the cycle's own transitions, leaving `status_from_events` folding nothing and every cycle reporting the INITIAL value `draft` forever — which would also silently destroy the tamper detection, since a hardcoded "draft" disagrees with a tampered stored field and still produces a (meaningless) warning. A lazier variant is to delete the drift check entirely once the false warnings become annoying; that removes the only signal that something wrote control state outside the harness.

Verified guard, by mutation: making `is_cycle_transition` return `false` fails 4 of the 14 pre-existing tests in tests/cycle_model.rs, each at the status assertion:
  an_abandoned_cycle_is_terminal_and_accepts_nothing_further (line 167) — left "draft", right "abandoned"
  status_is_derived_from_authoritative_events (line 196) — left "draft", right "active"
  a_stored_status_that_disagrees_with_history_is_surfaced_not_trusted (line 218) — left "draft", right "active"
  an_authority_move_does_not_disturb_an_active_cycle (line 124) — left "draft", right "active"

Those four already hold the door open, but only for a cycle with no cards — every one of them was written against a cycle that never declares a card, which is precisely why all 14 pass against the broken code. Add two guards that hold it open in the presence of cards (both verified to pass with the fix and to fail under the always-false mutation):

  the_cycles_own_abandonment_is_still_folded_with_cards_present
    activate cycle, activate F-001, then `cycle abandon`; assert data.status == "abandoned"
    AND status_matches_history == true. Catches a filter that drops cycle.abandoned.

  tampering_is_still_detected_when_the_cycle_holds_cards
    activate cycle, activate F-001, tamper_cycle_status("C-001", "closed");
    assert data.status == "active", stored_status == "closed",
    status_matches_history == false, and a warning containing "disagrees".
    Catches deleting or weakening the drift check under cover of this fix.

## VERIFIER objections
VERDICT: the defect is real and I reproduced every claim independently. STEPS 1, 2 and 4 of the fix are correct and I verified them by mutation. STEP 3 is defective and its accompanying test does not catch the defect it introduces, so I cannot pass the fix as a whole.

WHAT I CONFIRMED (my own probes, real Git repos in temp dirs, /Users/alvaro/Documents/Code/change-harness/.claude/worktrees/wf_81894ca4-835-14/tests/probe_fix_guards.rs)
- REPRO 1: cycle create+activate, then activate F-001 -> status "draft", stored "active", matches=false, warning "disagrees". The culprit event is exactly as claimed: `card.created cycle="C-001" card=null null -> "draft"` (card.created sets .cycle() but never .card()).
- REPRO 2: work block F-001 -> cycle status "blocked".
- REPRO 3: reproduces, but the finding's command line does not run. `integration prepare` requires `--actor-id` (omitted), and `integration abandon` takes `--integration-id`, NOT `--cycle-id` (the finding's invocation exits 2 with CH-USAGE-INVALID-ARGUMENTS). The observed event is `prepared -> abandoned`, not `accepted -> abandoned` as stated. With the correct flags the outcome is exactly as claimed: cycle status "abandoned". So the claim "All output below is real, captured from probe tests" is not true of REPRO 3 as written; the conclusion survives, the transcript does not.
- REPRO 4: `work start` on a card whose cycle was abandoned exits 0 and flips the derived status from "abandoned" back to "active".
- REPRO 5: confirmed by code reading (acceptance.recorded emits IntegrationStatus::Accepted.name() == "accepted", a cycle-status name; integration.promoted emits "promoted", dropped).
- Text report: `status: blocked (derived from 6 event(s))` with six subject-less `prev -> next` lines. JSON emits no history at all.

NOT DELIBERATE. docs/DEFECT-REGISTER.md:89 already lists "Cycle status folds card events" in the not-yet-fixed Tier 4 list. No D-nnn covers it. IMPLEMENTATION_PLAN.md Section 11.4 settles it affirmatively: the "Resulting state" column names a card or integration state for every non-cycle command and never a cycle state, so no non-`cycle.*` event is meant to move the cycle. D-053 (line 3320) says an abandoned integration returns members to `approved` so another can be planned — the opposite of "the cycle is dead".

THE FILTER REFUSES NOTHING IT MUST ACCEPT — verified, not assumed. I enumerated every `.transition(` call site: every non-`cycle.` event's next_state is a CardState or IntegrationStatus. `archive.closed` is the one that looked dangerous (name suggests cycle `closed`) and it actually emits IntegrationStatus::Archived, already dropped by parse_status. `cycle.group-declared` carries no transition. 27 `.cycle()` call sites across 9 files, matching the finding exactly.

MUTATION EVIDENCE (mine, all run)
- predicate -> `true` (today's behaviour): all 14 pre-existing tests/cycle_model.rs tests PASS; 4 of my 6 new tests fail at the status assertion (left "draft"/"abandoned", right "active").
- predicate -> `card_id.is_none() && next_state.is_some()` (naive fix): all 14 still PASS; the same 4 still fail. The prefix clause is genuinely load-bearing.
- predicate -> `false` (the overcorrection): 4 pre-existing tests fail at cycle_model.rs:124, 167, 196, 218 — the exact lines the finding names — and 5 of my 6 new ones fail. The door is held open in both directions.
- Full gate with the fix applied: `cargo fmt --check` clean, `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo test` 33 binaries, 807 passed, 0 failed (801 baseline + 6 mine).

OBJECTION 1 — STEP 3 REINTRODUCES THE CONFUSION IT CLAIMS TO FIX. `subject = card_id.unwrap_or(cycle_id)` mislabels `card.created`, because card.created sets `.cycle()` and not `.card()` — the very asymmetry the finding documents in its own root-cause section and then flags as adjacent item (a). Real output with the fix applied:
    2026-07-29T14:54:44Z cycle.created  C-001 none -> draft
    2026-07-29T14:54:44Z card.created   C-001 none -> draft
    2026-07-29T14:54:44Z card.activated F-001 draft -> ready
Two lines now assert that C-001 went none -> draft. The subject column is wrong for the one event the whole defect turns on. Either fix adjacent item (a) first (add `.card(...)` to card.created) or derive the subject from the event_type namespace, not from card_id.

OBJECTION 2 — THE TEST NAMED FOR THAT BEHAVIOUR CANNOT CATCH IT. `the_history_names_the_subject_of_every_transition` asserts only on `work.started F-001 ...` and `cycle.activated C-001 ...`, both correctly labelled. It says "every transition" and checks two hand-picked ones, both of which the buggy code gets right. I added `probe_the_subject_column_for_card_created` asserting `!text.contains("card.created C-001")`; it FAILS against the proposed fix. This is exactly the "test name does not describe what it asserts" pattern the repair sequence exists to eliminate — the fix must not land carrying a fresh instance of it.

OBJECTION 3 — THE PREDICATE RESTS ON AN UNENFORCED NAMING CONVENTION AND HAS NO UNIT TEST. `is_cycle_transition` is a new public method in src/control/event_store.rs with no test in that module; the whole fix is guarded only from the CLI. The `cycle.` prefix is a convention nothing checks. The natural way to fix the adjacent register item "five cycle statuses are unreachable" is to have integration prepare/acceptance/promote move the cycle too; if that is implemented as a second transition on the existing `integration.*` event rather than a new `cycle.*` event, this filter swallows it silently and NOT ONE test fails. The doc comment must state the contract ("a command that moves the cycle emits a `cycle.*` event") and a unit test should pin the four emitted `cycle.*` types.

OBJECTION 4 — `card_id.is_none()` is dead weight that can only ever subtract. Given the prefix clause it excludes nothing today, and if a future `cycle.*` event ever names a card it silently drops a real cycle transition. The finding calls it "belt-and-braces"; it is a latent over-restriction with no test standing behind it.

OBJECTION 5 — the finding identifies "in JSON the history is not emitted at all; a JSON consumer gets only event_count and a wrong status" and then fixes only the text rendering. After the fix the JSON consumer still gets no history, just a corrected `status` plus a new count. Half of the stated information-loss defect is left standing without being recorded as deliberately deferred.

OBJECTION 6 — adjacent item (b) is asserted as a defect without checking the spec. Section 11.4's "Required state" column gives `work start` "Card `ready`" and no cycle precondition, while `card activate` explicitly requires "cycle `active`". The absence of a cycle check in work start is what the table specifies. It may still be wrong, but the finding states it as a defect having not looked.

## VERIFIER missed
1. `audit cycle` (src/commands/audit.rs:211-242) is the other command that answers "what is this cycle's status", and the investigator never opened it. It reports `cycle.status` — the STORED field — while `cycle status` insists history is authoritative. Before the fix the two commands disagree on any cycle holding a card (audit says "active", cycle status says "draft"); after it they agree. That is free independent corroboration the investigator did not collect. It also leaves a real inconsistency untouched: `audit cycle`, the command whose entire job is finding discrepancies, trusts the cache and never runs the drift check. Worth its own register entry.
2. `EventStore::for_cycle` has one other caller — audit.rs:211 — which wants the whole subtree and is correctly unaffected. The finding asserts scope from `derived_status` alone and never states that `for_cycle` itself must keep returning everything. Someone reading "for_cycle returns the cycle's ENTIRE subtree, not the cycle's own transitions" as a bug report could reasonably narrow `for_cycle` and break the audit timeline. The fix is right to filter at the call site; the write-up does not say so.
3. No check that `event_count` is consumed anywhere else. tests/cycle_model.rs:197 asserts it, which the finding knew, but nothing establishes that no other suite or doc pins the cycle.status JSON key set before adding `cycle_transition_count`. I checked: no golden-envelope test, and IMPLEMENTATION_PLAN.md documents only the command name (line 855), so the addition is safe — but the finding did not verify it.
4. The predicate's interaction with the "five unreachable cycle statuses" register item is noted as "do not fix that by folding card events back in" but the real hazard runs the other way: whoever fixes that item must emit `cycle.*` events, and nothing in the code or tests tells them so. See objection 3.
5. Files: fix applied at src/control/event_store.rs:77-84 and src/commands/cycle.rs:196, 390-393, 411-421, 437; my guards and the failing subject-column probe at /Users/alvaro/Documents/Code/change-harness/.claude/worktrees/wf_81894ca4-835-14/tests/probe_fix_guards.rs.
