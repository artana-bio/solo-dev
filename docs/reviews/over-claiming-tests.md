# Over-claiming tests

## [high] tests/archive_cleanup.rs :: archiving_creates_a_ref_for_the_landing_and_every_candidate
- CLAIMS: An archive ref is created for the landing commit AND for every card's candidate commit — the refs that are the sole justification for `archive close` deleting branches and worktrees.
- ASSERTS: That `refs/archive/integrations/<id>` equals the authority head, and that `git rev-parse refs/archive/cards/<card>` returns a string 40 characters long. It never compares a card's archive ref to that card's candidate SHA.
- SURVIVING MUTATION: In `src/commands/archive.rs::refs_for`, change the member loop to `sha: landing_sha.clone()` (clone `landing_sha` into the first `ArchivedRef` so it stays available). Every card archive ref then points at the landing commit and no candidate commit is archived at all. Verified: all 17 tests in tests/archive_cleanup.rs and all 11 in tests/backup.rs pass.
- REPAIRED: The test reads each integration member's exact `candidate_sha` and requires the corresponding card archive ref to equal it. The recorded `landing_sha.clone()` mutation now fails on that identity assertion.

## [high] tests/archive_cleanup.rs :: landed_commits_remain_reachable_after_cleanup
- CLAIMS: After `archive close` deletes the branches and `gc --prune=now` runs, every landed candidate commit is still in the object database — i.e. the archive refs did their job.
- ASSERTS: That each candidate SHA still passes `cat-file -e`. It happens to hold for a reason unrelated to card archive refs: every candidate is a second-parent ancestor of the landing commit, which the integration archive ref keeps alive.
- SURVIVING MUTATION: The same `refs_for` mutation (all card archive refs point at `landing_sha`). No card candidate is archived, yet the test stays green because reachability comes through the landing merge. Verified.
- REPAIRED: After cleanup the test deletes every non-card-archive ref and proves each candidate is retained solely by its own card archive ref before collection. The recorded `landing_sha.clone()` mutation now fails that discriminator assertion.

## [high] tests/lifecycle.rs :: every_advertised_dry_run_changes_nothing
- CLAIMS: The whole `--dry-run` surface is checked, explicitly "because the failure mode is a flag that parses and is then forgotten" (Tier 2 defect 7).
- ASSERTS: Two invocations: `integration verify --dry-run` and `integration preflight`. Eleven other commands advertise `--dry-run` and are not touched here.
- SURVIVING MUTATION: In `src/commands/work.rs::run_checkpoint`, change `if args.dry_run {` to `if false {` so `work checkpoint --dry-run` performs the real mutation. Verified: the entire suite — 801 tests — passes. No test anywhere passes `--dry-run` to `work checkpoint`, `work block`, or `work resume`.
- REPAIRED: Renamed `stateful_dry_runs_do_not_change_state`, the lifecycle test covers the stateful previews requiring its fixtures: `work checkpoint`, `work block`, `work resume`, `cycle create`, `cycle declare-group`, `cycle abandon`, `card create`, `card revise`, `handoff revoke`, and `review begin`. It snapshots control and authority before each. Disabling each recorded guard independently now fails its named `wrote to control state` assertion. `backup verify` remains separately tracked because it has no guard.

## [high] tests/control_state.rs :: a_held_lock_makes_a_second_mutation_fail_as_policy
- CLAIMS: With the project lock held, a second mutating command fails, and fails in the policy category.
- ASSERTS: That the lock file still exists, and `output.status.success() || output.status.code() == Some(5)` — a disjunction that admits complete success. Nothing about the second command's outcome can make this test fail.
- SURVIVING MUTATION: In `src/control/lock.rs::acquire`, change the arm `Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists =>` to `... && false =>` so acquisition never reports contention. Verified: this test passes; only `a_second_acquisition_loses_while_the_first_is_held` (honestly named) fails.
- REPAIRED: The test now issues a real `cycle create` mutation under the held lock and requires an unsuccessful exit with policy code 5. The recorded `&& false` mutation now fails that outcome assertion.

## [high] tests/audit.rs :: the_timeline_reconstructs_the_cycle_in_order
- CLAIMS: The audit timeline reconstructs what happened in a cycle, in order — the reconstruction an auditor reads.
- ASSERTS: Only the relative index of three `type` strings within the array. The `at` timestamp on every timeline entry is never read. It is in fact always `null`: `src/commands/audit.rs` builds it from `event["recorded_at"]`, and `Event` has no such field — the timestamp is `occurred_at`. The audit trail's timeline carries no times today and no test notices.
- SURVIVING MUTATION: Delete the line `"at": event["recorded_at"],` from both `serde_json::json!` blocks in `src/commands/audit.rs` (the timeline map and `promotions`). Verified: all 12 tests in tests/audit.rs pass.
- REPAIRED: `audit.rs` now reads `occurred_at`, and this test requires every timeline timestamp to equal the `occurred_at` value of its source event. Deleting the timeline `at` line or substituting a fixed timestamp now fails that identity assertion.

## [high] tests/audit.rs :: the_report_names_the_exact_protected_branch_transition
- CLAIMS: The report names the exact protected-branch transition — when it happened, from what, to what, under whose acceptance.
- ASSERTS: `from`, `to` and `acceptance_id`. The `at` field is never asserted and is always `null` for the same `recorded_at`/`occurred_at` reason, so the recorded transition has no time attached.
- SURVIVING MUTATION: Same deletion of `"at": event["recorded_at"],` from `promotions()` in `src/commands/audit.rs`. Verified green.
- REPAIRED: The test requires the protected-branch transition timestamp to equal its source promotion event's `occurred_at`. Deleting the `promotions()` `at` line or substituting a fixed timestamp now fails that identity assertion.

## [medium] tests/ownership.rs :: an_exclude_lets_two_cards_share_a_directory
- CLAIMS: An exclude releases part of one card's scope so a second card can claim it — the mechanism that lets two cards share a directory.
- ASSERTS: That a card including `src/handwritten.rs` (excluding `src/generated/**`) and a card including `src/generated/api.rs` both activate. Those two includes are disjoint literals, so they would never have overlapped; the exclude is inert in the fixture.
- SURVIVING MUTATION: In `src/policy/paths.rs`, replace the body of `Scope::effective_includes` with `self.include.iter().collect()`, discarding excludes entirely from overlap computation. Verified: tests/ownership.rs (15), tests/worktree_allocation.rs (21) and all 57 `--lib policy` tests pass. `effective_includes` has no direct unit test.
- REPAIRED: The fixture first proves that broad `src/**` ownership collides with `src/generated/**`, then requires `src/**` minus `src/generated/**` to share the directory with the card owning that subtree. This exposed and fixed a production defect: overlap considered only whole includes, so it ignored a nested carve-out even though path-level ownership honored it. A second direct discriminator fully cancels `src/generated/**` before another card claims broader `src/**`; discarding effective excludes now fails at `a fully cancelled include must not retain an ownership claim` with `CH-POLICY-OWNERSHIP-OVERLAP`.

## [medium] tests/dry_run_parity.rs :: work_start_previews_an_overlapping_scope_refusal
- CLAIMS: `work start --dry-run` reproduces the refusal the real `work start` gives for an overlapping scope.
- ASSERTS: Starting the same card twice, which refuses with `CH-POLICY-LEASE-HELD`. `work start` has no scope-overlap check at all — overlap is enforced at `card activate` — so the named refusal does not exist on this path.
- SURVIVING MUTATION: In `src/commands/work.rs::preflight_start`, delete both the `if worktree::branch_exists(&scope, &branch)?` block and the `if path.exists()` block (the two checks the function's own comment says the preview used to skip). Verified: all 8 tests in tests/dry_run_parity.rs pass.
- CORRECTED AND REPAIRED: The original test is now accurately named `work_start_previews_a_held_lease_refusal`; scope overlap remains owned by `card activate`. A separate `work_start_previews_an_existing_branch_refusal` proves the real command reaches `CH-PRECONDITION-BRANCH-EXISTS` and then requires dry-run parity. Deleting both preflight checks now fails at `the dry run reported success for something the real command refuses`.

## [medium] tests/project_neutrality.rs :: the_engine_carries_no_language_specific_configuration
- CLAIMS: Nothing in the engine names a language, a build tool, or a file extension — incorrectly attributed here to D-001 rather than D-002.
- ASSERTS: That the string `project/project.json` and the output of `--help` contain none of seven terms. It never looks at the engine's code paths, which hardcode a ten-entry language-specific list (`DEPENDENCY_MANIFESTS`: Cargo.toml, package.json, go.mod, requirements.txt, …).
- SURVIVING MUTATION: In `src/policy/verification.rs`, replace `DEPENDENCY_MANIFESTS` with `const DEPENDENCY_MANIFESTS: [&str; 1] = ["Cargo.lock"];`, so a Python or Node card can silently change its dependency manifest. Verified: tests/project_neutrality.rs (3), tests/candidate_verification.rs (13) and all 21 `--lib policy::verification` tests pass.
- CORRECTED AND REPAIRED: The universal name was false because the engine deliberately carries a finite cross-language manifest policy. The test is now `language_neutral_configuration_still_guards_a_python_dependency_manifest`: it retains every project-document and help assertion, proves an ordinary Python-project path passes under `**`, then requires an undeclared nested `app/requirements.txt` to fail specifically through dependency-manifest policy. Reducing the list to `Cargo.lock` now fails at the exit assertion (`Some(0)` rather than `Some(5)`). The still-finite engine-owned list is recorded as a separate open design defect rather than hidden by this narrower name.

## [medium] src/policy/verification.rs :: tests::an_undeclared_dependency_manifest_blocks
- CLAIMS: A dependency manifest changed without being named in the card's write scope is a blocking finding — the supply-chain check.
- ASSERTS: One case, `Cargo.lock`. Nine of the ten declared manifests are never exercised anywhere in the suite.
- SURVIVING MUTATION: Same reduction of `DEPENDENCY_MANIFESTS` to `["Cargo.lock"]`. Verified green across the verification unit tests and tests/candidate_verification.rs.
- REPAIRED: The test carries an independent ten-name expectation and requires every root and nested form to produce exactly one blocking tuple with the exact path. The list-reduction mutation now fails first on `Cargo.toml` with no finding. This repair also exposed and fixed a production bypass: recognized manifests below the repository root were previously ignored.

## [medium] tests/artifacts.rs :: a_card_claiming_a_shared_artifact_in_its_own_scope_is_refused
- CLAIMS: A card that also claims a shared artifact in its write scope is refused, because that gives one path two owners.
- ASSERTS: One literal-equality case: include `"dist/bundle.js"` versus artifact path `dist/bundle.js`. The original audit correctly identified the then-live `==` defect, but commit `00f242d` had already fixed production and added sibling glob tests before F-025's baseline; only this named test remained non-discriminating.
- SURVIVING MUTATION: In `src/domain/card.rs::validate_generated_artifacts`, change the `Shared` arm's predicate to `.any(|pattern| pattern == &artifact.path && !pattern.contains('*'))`, disabling the check for every glob scope. At the F-025 baseline the named test still passed, but the audit's suite-wide pass count was stale because the newer sibling tests already failed it.
- REPAIRED: The named fixture now uses glob-vs-glob intersection (`dist/*.js` against `dist/bundle.*`), distinct from the existing glob-vs-literal sibling. The literal-only mutation now fails at the test's refusal assertion. Follow-up containment review also exposed and fixed a production bypass where a narrow exclude such as `src/*` falsely carved out the full shared glob `src/**`; concrete-witness and fully-carved-out guards cover both sides.

## [medium] tests/artifacts.rs :: a_per_card_artifact_generated_from_sources_outside_the_scope_is_refused
- CLAIMS: A per-card artifact generated from sources the card does not own is refused, because it would go stale.
- ASSERTS: One case where the source (`schema/**`) and the include (`src/**`) differ in their first segment. The original audit correctly identified the then-live equality defect, but commit `00f242d` had already replaced it before F-025's baseline; this named fixture still could not distinguish containment from first-segment equality.
- SURVIVING MUTATION: In the same function, replace the per-card ownership decision with `.any(|pattern| pattern.split('/').next() == source.split('/').next())`. At the F-025 baseline the named test still passed, but the audit's whole-binary claim was stale because newer sibling guards failed it.
- REPAIRED: The fixture now uses `src/a/**` versus unowned `src/b/**`. The first-segment mutation accepts it and fails at the test's refusal assertion. Strengthening this path exposed two production defects: treating a source glob as one literal path admitted broader or excluded source regions, and generated paths/sources bypassed repository-path validation so `src/../secret/**` could be accepted. Conservative containment plus repository-boundary validation now close both with positive and negative guards; the shared validator was also hardened against normalized and case-varied `.git` aliases found during review.

## [medium] src/control/event_store.rs :: tests::identifiers_are_dense_and_monotonic
- CLAIMS: Event identifiers are dense and monotonic — the property that stops a new authoritative event from overwriting an existing one.
- ASSERTS: That an empty store yields `E-000001` and that after one append the next is `E-000002`. Both hold for any counting scheme; neither density nor monotonicity is exercised past n=1.
- SURVIVING MUTATION: In `EventStore::next_id`, replace `.max().unwrap_or(0)` with `.count() as u64`. With a gap in the sequence (E-000001 and E-000003 present) the next id collides with E-000003 and `write_atomic` overwrites an authoritative event. Verified: all 381 lib tests pass.
- REPAIRED: The test appends E-000001 through E-000003, proves they are ordinarily dense, removes E-000002, explicitly guards that the gap and E-000003 both exist, requires E-000004 next, and verifies E-000003's serialized bytes remain unchanged. Counting files now fails at `the next identifier must advance beyond every existing event` (`E-000003` rather than `E-000004`).

## [medium] tests/merge_preflight.rs :: untracked_feature_state_cannot_enter_the_integration
- CLAIMS: Untracked content sitting in a feature worktree at merge time cannot reach the integration.
- ASSERTS: `ls-tree` of the member's *candidate commit* — not of the integration head or tree. An uncommitted file is absent from that commit by construction, whatever the merge does, so the assertion is independent of the mechanism named.
- SURVIVING MUTATION: In `src/git/integration_worktree.rs::merge`, add `"--strategy=ours"` to the argv, so every candidate's content is discarded and the integration tree is the bare authority baseline. Verified: this test passes (2 of 16 in the file fail, both smoke-gate tests).
- REPAIRED: The fixture proves the candidate tree differs from authority, reads the actual integration head and tree, requires a one-member integration to equal the pinned candidate tree, and only then checks that `scratch.txt` is absent from that integration head. `--strategy=ours` now fails the candidate-tree identity assertion (`853694…` rather than `3da24f…`).

## [medium] tests/merge_preflight.rs :: a_clean_merge_records_the_integration_head_and_tree
- CLAIMS: A clean merge records the integration head and tree.
- ASSERTS: That both are 40 characters, that the head differs from the authority head, and that the protected branch did not move. Nothing about the tree's contents, so a merge that combines nothing still satisfies it.
- SURVIVING MUTATION: Same `--strategy=ours` addition in `src/git/integration_worktree.rs::merge`. Verified green.
- REPAIRED: The test guards that clean preflight produces a tree different from authority, then requires the recorded tree to equal both the actual head's tree and the preflight result, including the inspect and event records. `--strategy=ours` now fails at the preflight-to-integration tree identity assertion (`853694…` rather than `2a3a55…`).

## [medium] tests/gate_runner.rs :: logs_are_written_outside_git_history_and_their_digests_recorded
- CLAIMS: Gate logs live outside Git history and their digests are recorded in the versioned receipt (invariant 7.4.2).
- ASSERTS: That `stdout_digest` starts with `sha256:`. It never checks the digest is of the log it names — so a receipt can carry a digest that does not identify the evidence it points at.
- RECORDED MUTATION, CORRECTED: In `src/commands/gate.rs` where the `Receipt` is built, the audit said to replace both digest fields with `Digest::of_bytes(b"")`. That exact text does not compile there because `Digest` is not imported. The equivalent fully qualified mutation, `crate::domain::digest::Digest::of_bytes(b"")`, does compile and survives the old gate-runner and audit assertions; the real digest was asserted only one layer below the receipt.
- REPAIRED: The gate writes distinct non-empty stdout and stderr; the test preserves the algorithm-prefix assertion, reads the exact committed `receipts/<id>.json` from `HEAD`, proves it is the receipt reported by the command, and requires both persisted digests to identify their named logs. Replacing both with the empty digest fails the versioned stdout identity assertion; replacing only stderr fails the versioned stderr identity assertion.

## [medium] src/git/mod.rs :: tests::probe_reports_minimum_version_compliance_and_worktree_support
- CLAIMS: The probe reports whether the installed Git supports the worktree subcommands.
- ASSERTS: `assert!(probe.supports_worktrees)` on a host whose Git does support them, so the value is indistinguishable from a constant.
- SURVIVING MUTATION: In `GitClient::probe`, replace `supports_worktrees: inspect::supports_worktrees()?,` with `supports_worktrees: true,`, deleting the probe. Verified: all 82 `--lib git::` tests and all 24 in tests/cli.rs pass. (Recorded as still-open in docs/DEFECT-REGISTER.md; confirmed here.)
- REPAIRED: The test re-runs the public probe in an isolated child whose fake Git 2.4.0 rejects `worktree`; a parsed-version guard proves the fake is active. Hardcoding `true` now fails at `a Git that rejects the worktree subcommand must not report worktree support`. The discriminator exposed and fixed a live false positive: the old implementation treated Git's standard "not a git command" diagnostic as evidence of support.

## [medium] tests/candidate_verification.rs :: verification_is_identical_from_a_second_clean_clone
- CLAIMS: Verification is a pure function of committed objects, reproducible from a second clean clone — i.e. it consults nothing worktree-local.
- ASSERTS: That `work verify` run twice in the *same* workspace produces the same `data`. No second clone is made and no source repository is removed, so it tests determinism, not clone-independence.
- SURVIVING MUTATION: In `src/policy/verification.rs::check_path`, change `if !scope.allows(path) {` to `if false && !scope.allows(path) {`, removing scope enforcement entirely. Verified: this test passes (5 other tests in the file fail). Any deterministic mutation survives it.
- REPAIRED: The fixture commits an out-of-scope change, plants a permissive ignored card only in the original clean worktree, verifies once, then replaces the allocated path with an independent `--no-local` clone at the exact branch and SHA, makes the source repository unavailable, and verifies again without changing control or lease state. It requires identical JSON refusals and exact `CH-POLICY-CANDIDATE-OUT-OF-SCOPE` outcomes. The recorded deterministic mutation now reaches the clone and fails at `the same out-of-scope change must be refused from the clone` (`Some(0)` rather than `Some(5)`); a separate successful-clone regression preserves and strengthens the original `first["data"] == second["data"]` assertion. A cached-card mutation fails the refusal-envelope clone-identity assertion, the direct non-vacuity proof for worktree-local independence.


# Patterns
Five shapes account for nearly every instance, in descending frequency.

1. **Asserting shape rather than identity.** The commonest by far: `len() == 40` for a SHA, `starts_with("sha256:")` for a digest, `is_string()`, `as_array().len() == 3`. The assertion confirms a well-formed value is present and never that it is the *right* value. This is what lets every card's archive ref point at the landing commit (`archiving_creates_a_ref_for_the_landing_and_every_candidate`), lets a receipt carry the digest of empty bytes (`logs_are_written_outside_git_history_and_their_digests_recorded`), and lets an integration tree contain none of its members' work (`a_clean_merge_records_the_integration_head_and_tree`). It is the same family as the `git rev-parse` tautology already in the register: `rev-parse` returns a well-shaped SHA whether or not the object exists.

2. **A universal name over a single literal fixture.** `every_advertised_dry_run_changes_nothing` covers 2 of 13 commands. `an_undeclared_dependency_manifest_blocks` covers 1 of 10 manifests. `identifiers_are_dense_and_monotonic` covers n=1. `a_card_claiming_a_shared_artifact_in_its_own_scope_is_refused` covers the one case where the glob happens to be a literal — which is exactly the case the `==` comparison in `validate_generated_artifacts` gets right and every other case it gets wrong. The quantifier in the name is doing work the fixture does not.

3. **Refusing for a reason other than the named one.** `work_start_previews_an_overlapping_scope_refusal` observes a lease-held refusal for a check (`work start` scope overlap) that does not exist. `a_cached_card_in_the_worktree_cannot_alter_verification` is the sound counterexample — it turned out to discriminate — which is why the pattern has to be checked by mutation and not by reading: the observed refusal is right about as often as it is wrong.

4. **An inert fixture: the mechanism under test is not engaged.** `an_exclude_lets_two_cards_share_a_directory` uses two includes that were already disjoint, so the exclude is decoration; excludes can be deleted from overlap computation entirely and it stays green. `verification_is_identical_from_a_second_clean_clone` never makes a clone.

5. **An assertion that cannot fail.** `a_held_lock_makes_a_second_mutation_fail_as_policy` asserts `success() || code == Some(5)`. `the_timeline_reconstructs_the_cycle_in_order` and `the_report_names_the_exact_protected_branch_transition` read a field (`at`) that is null in production and never assert on it — the audit's timestamps are dead today and the two tests named for the timeline are the ones that should have seen it.

The correlate of the register's own observation holds: where a test contains an explicit vacuity guard ("the fixture must actually have written the secret", "the tamper must change the plan", "the gate must depend on the uncommitted file, or this test is vacuous", removing the source before a restore drill), the test survived mutation without exception. Every finding above is in a test with no such guard.

# Notes
Worktree: /Users/alvaro/Documents/Code/change-harness/.claude/worktrees/wf_81894ca4-835-15 — left clean, all mutations reverted, `git status` empty. Baseline before and after: 801 passing.

Method: every entry above was produced by editing src/, rebuilding, and running the named test binary; the mutation text is exactly what I applied. Nineteen tests across fourteen distinct mutations. This is a partial enumeration, not the full ~45 — I worked outward from the load-bearing mechanisms (archive refs, locks, dry-run parity, the audit trail, scope ownership, merge, receipts) and stopped there.

**Three findings are live defects, not just weak tests.** Repair them regardless of what happens to the tests:

- `src/commands/audit.rs` reads `event["recorded_at"]` twice; `Event` (src/control/event_store.rs) declares `occurred_at`. Every `at` in the audit timeline and in every protected-branch transition is `null` in production right now.
- `work checkpoint --dry-run`, `work block --dry-run` and `work resume --dry-run` are never exercised by any test. Deleting the dry-run guard from `run_checkpoint` passes all 801. This is Tier 2 defect 7 in three more commands; I did not audit whether they are actually correct, only that nothing would notice if they were not.
- The generated-artifact scope checks were `==` comparisons when this audit was written. That production defect was fixed before F-025's baseline; the corrected medium entries above distinguish the stale suite-wide claim from the two named fixtures that still needed repair. F-025 also fixed the source-glob containment, repository-path validation (including normalized `.git` aliases), and partial shared-carve-out bypasses exposed while strengthening them.

**Suspicious but sound — checked and cleared, so nobody re-checks them:**
- `tests/landing_commit.rs::the_landing_commit_is_retained_by_a_harness_ref` and `src/git/landing.rs::tests::a_retained_landing_commit_survives_aggressive_collection` — both fail when `landing::retain` is stubbed out. The register's repair holds.
- `tests/concurrency.rs::a_lock_survives_being_diagnosed_under_a_different_locale` — fails when `.env("LC_ALL","C")` is removed from `process_liveness`. Genuinely discriminating on this host.
- `tests/control_state.rs::an_interruption_at_any_journal_boundary_is_diagnosable` — asserts the recorded step list, not just that recovery is required.
- `tests/candidate_verification.rs::a_rename_out_of_scope_is_caught_on_the_source_side` — the exit-code assertion is load-bearing; the `contains("README.md")` half is not (rename sources appear in `changed_paths` regardless), but the test would fail if only the destination were checked.
- `ABSENT_PID = 99_999` in tests/concurrency.rs and `pid: 99_999` in src/control/lock.rs: confirmed on this host that `ps -o lstart= -p 99999` exits 1 with empty stderr (a real report of death) while `-p 4294967294` writes "process id too large" to stderr. The defect-13 fixture repair is correct.

**Left out for want of a verified mutation** — real over-claims I could not substantiate to the standard set, listed so a follow-up can start here rather than rediscover them:
- `src/runner/receipt.rs::tests::a_receipt_records_a_failing_attempt_too` constructs the `Receipt` inside the test and asserts its own literals; it exercises no src logic beyond serde and cannot fail for any change to the runner.
- `src/git/command.rs::tests::argument_metacharacters_are_never_interpreted` — the second assertion, `!stderr.contains("root")`, is vacuous on any host where the suite is not run as root.
- `tests/ownership.rs::a_refused_activation_leaves_control_untouched` ends on `assert_ne!(before, "")`, which asserts nothing about the refusal.
- `tests/control_state.rs::control_history_contains_no_partial_authoritative_record` walks a history one commit long.
- `tests/recovery.rs::every_mutating_command_names_at_least_one_boundary` greps each file for `with_transaction(` and `steps.at(` at *module* granularity, so a module with several transactions passes when only one names a boundary.
- `src/git/mod.rs::tests::version_comparison_discriminates_around_the_minimum` closes with `let compliant = |v| v >= minimum;` — a re-implementation of the field it is checking. The name does not claim more than it does, which is why it is here and not above.
