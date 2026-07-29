# generated-artifact-scope-glob-equality  [medium]  fix_accepted=True

## Summary
`CardDraft::validate_generated_artifacts` compares write-scope glob patterns to artifact paths with string `==` instead of glob matching, so a shared artifact covered by a glob include (`src/**` vs `src/generated/api.rs`) is accepted — one path with two owners — while a per-card source the card plainly owns (`src/schema.toml` under `src/**`) is refused.

## Root cause
src/domain/card.rs, `CardDraft::validate_generated_artifacts`, both at HEAD (dc9e490):

- line 344: `.any(|pattern| pattern == source)` — per-card sources checked for string equality against write-scope include patterns. Should be scope containment. Fails closed (false refusal).
- line 358: `.any(|pattern| pattern == &artifact.path)` — shared artifact path checked for string equality against write-scope include patterns. Should be glob intersection minus excludes. Fails open (missed refusal).

Both ignore `write_scope.exclude` entirely and both are case-sensitive even on a case-insensitive host, where `crate::policy::paths::CaseSensitivity::host()` is `Insensitive` — so `include: ["SRC/generated.rs"]` with shared artifact `src/generated.rs` also slips through on macOS.

NOT a defect, do not "fix" it: src/policy/verification.rs:276 `.any(|pattern| pattern == path)` for DEPENDENCY_MANIFESTS is deliberate — the finding is "must be named explicitly in the card's write scope", so exact naming is the requirement, not a matching bug.

## Files
src/domain/card.rs, src/policy/verification.rs, tests/artifacts.rs, docs/DEFECT-REGISTER.md

## Proposed fix
In `validate_generated_artifacts` (src/domain/card.rs:329), build the scope once:

    let case = crate::policy::paths::CaseSensitivity::host();
    let scope = crate::policy::paths::Scope::new(&self.write_scope.include, &self.write_scope.exclude);

PerCard arm — replace the `pattern == source` test with containment:

    if !scope.allows(source) { /* same "go stale" error, unchanged wording */ }

Containment, not intersection: a source must be entirely inside what the card owns, and `Scope::allows` already applies deny-by-default and lets an exclude override an include.

Shared arm — replace the `pattern == &artifact.path` test with intersection minus excludes:

    let covered_by_exclude = self.write_scope.exclude.iter()
        .any(|pattern| crate::policy::paths::matches(pattern, &artifact.path, case));
    let claimed = !covered_by_exclude
        && self.write_scope.include.iter()
            .any(|pattern| crate::policy::paths::patterns_intersect(pattern, &artifact.path, case));
    if claimed { /* same "two owners" error, unchanged wording */ }

Intersection because artifact.path may itself be a glob (verification.rs:294 already matches it as a pattern); `matches` alone would miss include `src/generated/api.rs` vs artifact `src/generated/*.rs`. Over-approximating toward refusal here is the documented policy of `patterns_intersect`. An exclude that *covers* the artifact path (checked with `matches`, not intersection) means the card does not claim it, so it stays valid.

I implemented exactly this in the worktree. `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, and every test binary pass.

FAILING-FIRST TESTS (add to tests/artifacts.rs; its `draft` helper hardcodes `exclude: []` and must gain an exclude parameter):

1. `a_shared_artifact_covered_by_a_glob_include_is_refused` — include `["src/**"]`, shared `src/generated/api.rs`, no exclude. Assert exit != 0, code CH-POLICY-INVALID-ARTIFACT, message contains "two owners".
2. `a_per_card_source_covered_by_a_glob_include_is_accepted` — include `["src/**"]`, per_card `src/generated.rs` with `sources: ["src/schema.toml"]`. Assert `card validate` succeeds.

MUTATION EVIDENCE (I ran this — `git stash` of src/domain/card.rs back to the `==` form, `cargo test --test probe_artifact_scope`):
  a_shared_artifact_covered_by_a_glob_include_is_refused ... FAILED
    panicked at probe_artifact_scope.rs:61: "`src/**` claims `src/generated/api.rs`; that is one path with two owners"
  a_per_card_source_covered_by_a_glob_include_is_accepted ... FAILED
    "per-card artifact `src/generated.rs` is generated from `src/schema.toml`, which this card does not own"
  (the four guard tests below passed under both, as they must)
Each fails at the assertion that matters, not incidentally.

TWO EXISTING FIXTURES ASSERT THE DEFECT AND MUST BE CORRECTED — these are the only two tests in the suite that break under the fix, and both break because they construct a card that a correct check refuses:

a) src/policy/verification.rs:704 `committing_a_shared_artifact_blocks` — `card_with(&["src/**"], &[], &[artifact("src/generated.rs", Shared)])`. Under the fix, `CardRecord::activate` returns Err and the test panics at verification.rs:431 with "…gives one path two owners". Fix: pass the shared path as an exclude — `card_with(&["src/**"], &["src/generated.rs"], …)`. The verify report then carries both `excluded-path` and `generated-artifact-not-owned`; the test's assertion targets the latter and still holds.

b) tests/artifacts.rs:189 `committing_a_shared_path_fails_verification` — include `"src/**"` with shared `src/shared.gen.rs`. Same cause; `card create` now refuses. Fix: change the include to `"src/work/**"` (or give the helper an exclude). The envelope message joins *all* blocking details (src/commands/work.rs:983-989), so the assertion on "integration generates it" survives the extra `out-of-scope` finding. I verified this: the test passes after the one-word change.

Also worth correcting for honesty (does not fail either way): tests/artifacts.rs:85 `a_per_card_artifact_generated_from_owned_sources_is_accepted` declares `sources: ["src/**"]` against include `["src/**"]` — it passes only because the two strings are identical, so it certifies `==`, not ownership. Change its source to a real file such as `src/schema.toml`, at which point it becomes failing-first test 2. Likewise tests/artifacts.rs:97 `a_card_claiming_a_shared_artifact_in_its_own_scope_is_refused` passes under both the broken and the fixed code: its include list literally contains the string `dist/bundle.js`, and the `src/**` alongside it is a decoy — it cannot distinguish the two implementations.

Update docs/DEFECT-REGISTER.md (the claim currently sits unresolved in the prose list around line 90) with a numbered ✅ FIXED entry naming both directions and the two fixtures that had certified the defect.

## Over-correction risk
Two opposite failures, one per arm, and I mutation-tested both.

1. Shared arm made too aggressive — dropping the exclude check, or refusing whenever the card has any include. This forbids the only legitimate way to declare a shared artifact at all: a card that names the integration-owned path and deliberately carves it out of its own scope. Since a card must declare the artifact to get the `generated-artifact-not-owned` protection at verify time, an over-aggressive shared check makes the entire Shared class undeclarable by any card whose scope is a glob over the same tree.
   GUARDS: `a_shared_artifact_excluded_from_the_scope_is_accepted` — include `["src/**"]`, exclude `["src/generated/api.rs"]`, shared `src/generated/api.rs`, must validate; and `a_shared_artifact_outside_the_scope_is_accepted` — include `["src/**"]`, shared `dist/bundle.js`, must validate.
   MUTATION RUN: forcing `let claimed = true` (ignoring excludes) → `a_shared_artifact_excluded_from_the_scope_is_accepted ... FAILED`, other five pass. The guard is load-bearing.

2. Sources arm made too loose — reaching for `patterns_intersect` because the shared arm uses it. That would let a card owning only `src/a/**` declare `src/**` as a source, which is precisely the staleness the rule exists to prevent, and would also un-enforce excludes.
   GUARDS: `a_per_card_source_broader_than_the_scope_is_refused` — include `["src/a/**"]`, per_card `src/a/generated.rs` with `sources: ["src/**"]`, must be refused with "go stale"; and `a_per_card_source_excluded_from_the_scope_is_refused` — include `["src/**"]`, exclude `["src/schema.toml"]`, source `src/schema.toml`, must be refused.
   MUTATION RUN: swapping `scope.allows(source)` for `patterns_intersect` → both guards FAILED, the two failing-first tests still passed. Without these guards the loose fix looks correct.

3. Collateral over-fix: someone grepping for the same shape will find src/policy/verification.rs:276 `pattern == path` for DEPENDENCY_MANIFESTS and "fix" it to `matches`. That would silently drop the undeclared-dependency-manifest guard for every card whose scope is `**` or `*.toml`, because the rule is that Cargo.toml must be named *explicitly*. Existing coverage of that finding should be confirmed to still fail if the comparison is loosened; if it does not, the fix must not touch that line.

4. Pre-existing activated cards: the shared arm gets strictly stricter, so a stored card with a shared artifact inside its scope would now fail re-validation. Not an issue here — IMPLEMENTATION_PLAN.md records that every stored card in this project declares an empty `generated_artifacts` list — but the implementer should not add a grandfather clause, since that would reintroduce the hole under a new name.

## VERIFIER objections
I tried to refute this and could not. Everything load-bearing reproduced in my own worktree.

REPRODUCTION — confirmed, both directions, driving the real binary (tests/probe_artifact_scope.rs, 7 tests).
- Shared arm fails open: include ["src/**"], shared `src/generated/api.rs` → `card validate` exit 0, `card create` exit 0, `card activate` exit 0, state ready. Verbatim as claimed.
- Sources arm fails closed: include ["src/**"], per_card `src/generated.rs` sources ["src/schema.toml"] → exit 5, CH-POLICY-INVALID-ARTIFACT, "...which this card does not own; it would go stale...". Verbatim as claimed.

NOT DELIBERATE. No D-xxx sanctions `==` here. WP-540's own prose (docs/IMPLEMENTATION_PLAN.md:2716-2719) states the rule in ownership terms ("sources the card does not own", "inside a card's write scope"), which is containment, not string identity. And docs/DEFECT-REGISTER.md:90 already carries "Generated-artifact scope checks compare globs with `==`" in the outstanding-claims prose, so this is a known-unrepaired claim, not a recorded choice.

FIX — I implemented it exactly as proposed and it is correct. cargo fmt --check clean, clippy --all-targets --all-features -D warnings clean, lib 381/381, probes 7/7.

MUTATION EVIDENCE — I ran all four, not just the two the investigator ran.
- M1 revert card.rs to `==`: both failing-first probes FAIL, each at its own assertion (shared probe fails because validate *succeeded*, so no earlier refusal is even possible; sources probe fails on the exact "go stale" message). No trap here.
- M2 drop the exclude check (`claimed` ignores excludes): only `probe_shared_artifact_excluded_from_the_scope_is_accepted` FAILS. Guard is load-bearing, exactly as claimed.
- M3 swap `scope.allows(source)` for `patterns_intersect`: both source guards FAIL, both failing-first probes still PASS. Without the guards the loose fix looks correct — claim confirmed.
- M4 (the investigator only *recommended* this, did not run it) loosen src/policy/verification.rs:276 to `matches`: `an_undeclared_dependency_manifest_blocks` FAILS on `assert!(!report.passed)`. The DEPENDENCY_MANIFESTS `==` is genuinely guarded. Its "do not fix this" note is correct and now verified.

BREAKAGE COUNT — verified by a full `cargo test --no-fail-fast` under the fix. Exactly two failures, precisely the two named: `policy::verification::tests::committing_a_shared_artifact_blocks` (panics inside card_with at verification.rs:431 because CardRecord::activate now errs) and `tests/artifacts.rs::committing_a_shared_path_fails_verification` (card create refuses). Every other binary green. Both proposed repairs work: I applied them and both tests pass.

Objections that survive, none fatal:

1. "Two existing fixtures assert the defect" overstates. Neither asserts the broken comparison; they merely *construct* a card the hole permitted. Consequence: after repair both remain completely insensitive to this defect — they pass identically against `==` and against the fix. The register entry should not claim they "had certified the defect"; they certified the verify-side behaviour and needed a legal card to do it.

2. Both repaired fixtures now block for two reasons at once (excluded-path or out-of-scope, plus generated-artifact-not-owned). Their `assert!(!passed)` / `assert!(!status.success())` lines no longer discriminate anything. They survive only because each also asserts the finding kind or the "integration generates it" wording. That is fine today but is precisely the shape that decays — if anyone later trims those tests to the bare block assertion they become vacuous.

3. A consequence the investigator drew halfway and stopped. After the fix a Shared artifact can never sit inside the declaring card's scope, so committing one always also trips out-of-scope or excluded-path. `generated-artifact-not-owned` for Shared is therefore never the sole blocking finding — the verify-side shared branch is now strictly redundant as a blocker (it still contributes its message). This also undercuts the finding's own blast-radius argument: post-fix, verify catches the commit because the path is unowned by scope, not because of the artifact check.

4. Asymmetry inside the shared arm: excludes are tested with `matches` while includes are tested with `patterns_intersect`. So exclude `src/generated/api.rs` against a *glob* artifact path `src/generated/*.rs` does not count as covering it, and the card is refused. Conservative and defensible, but it is a real behaviour difference that exists nowhere else and is not stated at the call site. The comment above the function should say it.

5. `CaseSensitivity::host()` makes draft validation machine-dependent: include ["SRC/**"] with shared `src/x.rs` validates on Linux and is refused on macOS. Scope already works this way everywhere, but this is the first time it governs *card validation*, which mints an immutable record. Worth one sentence in the register rather than silence.

6. Cosmetic: `scope` is built unconditionally even for drafts with no per-card artifacts, and the shared arm does not use it (it goes to the raw include/exclude lists). Harmless.

## VERIFIER missed
Four things, one of which is a genuine sibling hole left open.

A. THE CROSS-CARD HALF OF "ONE PATH TWO OWNERS" IS STILL UNCHECKED, and the fix's own recommended workaround leads authors straight into it. The rule only ever compares an artifact against *the declaring card's* scope. src/policy/allocation.rs:84 compares scope-to-scope only (`candidate.scope().overlaps(&other.scope())`); nothing compares card A's declared shared artifact against active card B's write scope. Card A: include ["src/a/**"], shared `src/generated/api.rs` — accepted before and after the fix, since the path is outside A's scope. Card B: include ["src/generated/**"] — activates cleanly, no scope overlap with A. B now write-owns a path A declared integration-owned, and B's own verify sees no artifact declaration so nothing blocks the commit. That is the exact collision the classes exist to prevent, and it is untouched. Post-fix the standard way to declare a shared artifact becomes "carve it out of my scope", which makes this configuration the normal one rather than the exotic one.

B. THE ARTIFACT'S OWN `path` IS NEVER VALIDATED AGAINST THE WRITE SCOPE. The per-card arm checks `sources` and ignores `path`. A card with include ["src/**"] may declare per_card `dist/bundle.js` with sources ["src/schema.toml"] and it validates — the card claims to generate a file it does not own. The acceptance criterion only names sources, so this is arguably out of WP-540's scope, but it is the symmetric hole to the one being fixed and the finding does not mention it.

C. `ArtifactClass::Transient | ArtifactClass::Serialized => {}` — the two remaining arms do nothing at all in validate_generated_artifacts. A transient artifact inside the scope is exactly as much a two-owner problem as a shared one (nobody owns it, yet the card claims it), and it is silently accepted. Not this defect, but the same function, and worth naming rather than leaving for the next reviewer to rediscover.

D. REGISTER MECHANICS. docs/DEFECT-REGISTER.md:90 already carries this claim inside a run-on Tier-4 prose sentence alongside five other unresolved claims. Adding a numbered ✅ FIXED entry without editing that sentence leaves the register asserting the defect is outstanding and fixed at once — the register's whole purpose is to be the artifact a future reader trusts, so the prose clause must be struck in the same edit.

Also worth stating for the record: I verified the two claims the investigator asserted without running them. `require_no_generated_artifacts` (src/commands/integration.rs:1402) does refuse landing any integration whose member declares a Shared artifact, and no stored card is ever re-validated — validation runs only on CardDraft at validate/activate (src/domain/card.rs:442), so there is no grandfathering exposure and no reason for a grandfather clause.
