# toolkit-mobkit native-host test debt

The unit tests of this crate's native host (`installed_host`,
`private_originals`, `private_host`, `original_ingress`) compile only with the
`native-host` feature, which `meerkat-connect` turns on. CI runs them in the
`toolkit-mobkit[native-host]` unit row on the push to main, built together
with `meerkat-connect` (`FEATURE_UNIT_SUITES` in `scripts/ci-cargo-lanes.mjs`).

The tests below fail today. The row excludes each one by name, and the
known-red list in `scripts/ci-cargo-lanes.mjs` points each at its section
here; any other failure fails the row. The row then runs the excluded tests
that fail every time and fails unless every one of them still fails within
a minute: when a fix makes one pass, take it off this page and off the list
in the same change. The intermittent ones are excluded from both runs. The
plan checks the list and this page against each other, so a test is on both
or on neither, under the same section.

The owner of every class is Connect. Test names are relative to the crate.

## Cold restore refusals

Status: open. Kind: product or test fixture, not yet diagnosed.

A cold restore refuses (`OwnerUnavailable`, `Unavailable` or `Conflict`)
where the test expects the recovery to go on. One instrumented run with a
diagnostic before each check in `cold_setup_admission.rs` and
`custody_recovery/cold.rs` names the refusing step; then fix that step. The
refusing step can vary with load: the two `custody_recovery_helper_cold_tests`
fail at the first original's promoted-custody restore when run alone, and at
the helper's restore in the parallel known-red run, with the same refusal
(`Unavailable`), so the diagnostic run should cover both.

- `installed_host::acquisition::continuation::owner_tests::cold_source_tests::cold_setup_a_historical_custody_cannot_validate_another_run_or_submission`
- `installed_host::acquisition::continuation::owner_tests::cold_source_tests::durable_owner_reopen_drains_same_pending_completion_after_source_recovery`
- `private_originals::governed_submit::tests::first_input_tests::approved_fixture::custody_recovery_helper_cold_tests::cold_pending_helper_settles_retired_input_without_resubmission`
- `private_originals::governed_submit::tests::first_input_tests::approved_fixture::custody_recovery_helper_cold_tests::cold_promoted_helper_restores_exact_input_and_refuses_tampered_or_legacy_bytes`
- `private_originals::service_approved_input::admission_tests::cold_service_tests::cold_b_restores_actual_admission_and_phase_without_replay_or_new_issuer`
- `private_originals::service_approved_input::attachment::tests::native::batch::admitted_batch_target_independence_and_settled_cold_restore_after_alpha_revoke`

## Interaction lineage

Status: open. Kind: stale test.

The authority context now carries the admitted input's interaction lineage.
Fix: in `enrollment/tests.rs`, assert that the lineage equals the admitted
input id, or strip it before the authority checks.

- `installed_host::acquisition::enrollment::tests::production_leaves_reach_actual_bound_owner_but_not_missing_native_continuation`

## Stale expectations

Status: open. Kind: stale tests that assert an older contract of this
crate's own code. The fix for each is the expectation to update.

| Test | Fix |
|---|---|
| `installed_host::tests::direct_rust_access_is_fenced_by_owner_close_or_selection_replacement` | Count the access rows from a baseline taken after `open`, or leave out the `toolkit.application-service-tool-policy` schema as `identity_binding.rs` does. |
| `installed_host::tests::direct_rust_original_cancellation_is_fenced_and_does_not_revoke_standing_access` | The same. |
| `installed_host::operator_setup::tests::owner_tests::captured_answer_loss_during_child_admission_prevents_native_child_write` | Expect `Err(PrivateOriginalError::Denied)` and the row's `issuance.state == "native_refused"`. |
| `private_originals::service_approved_input::admission_tests::live_attachment_preparation_reopens_cancelled_task_and_never_submits_b` | Build the control with `prepare_service_mcp_control`, as `attachment_entry_tests.rs` does. |
| `private_originals::service_approved_input::admission_tests::cold_service_tests::cold_b_requires_complete_reserved_selection_and_retained_input` | Write the raw `RecordData`. |
| `private_originals::service_approved_input::admission_tests::source_status_tests::unrelated_publication_is_readable_but_terminal_bound_association_mismatch_refuses` | Rebuild `custody_attempt` for the tampered input. |
| `private_originals::service_grants::mcp::tests::application_policy_pending_record_keeps_exact_provenance_without_reissue` | Use the owner `toolkit/acquisition/<scope owner>`. |
| `private_originals::service_approved_input::admission_tests::application_policy_tests::application_task_cancellation_before_issue_keeps_only_native_a` | Also require the `toolkit.authenticated-service-access-decision` schema. |
| `private_originals::service_approved_input::admission_tests::application_policy_tests::invalidated_application_policy_keeps_only_native_a` | The same. |
| `private_originals::service_approved_input::admission_tests::service_recovery_tests::operator_recovery_preserves_new_publication_and_other_approval_child` | Check only the published parent. Open question: whether `can_recover` should read `parent_state`. |
| `private_originals::service_grants::mcp::tests::whole_set_replays_without_reissue_and_matches_only_its_access_bound_source` | Expect `Unavailable` for the unknown server and keep `Denied` for the known one. |
| `private_originals::governed_submit::tests::first_input_tests::acquisition_approved::reviewed_original_runs_enrolled_acquisition_and_one_controller_only_helper` | Assert the enrolled form (revision 3, version 5), not equality with the held snapshot. |

## Decision source unknown field

Status: open. Kind: product defect, low severity.

`ServiceParentDecisionSource` decodes
`{"kind":"standing_rule_decision","enrollment_id":7}` as a standing rule
decision and drops the id: serde skips the remaining entries of an
internally tagged unit variant, so `deny_unknown_fields` never applies. Fix:
make the variant `StandingRuleDecision {}` and update its match in
`meerkat-connect`; the wire form does not change.

- `private_host::tests::service_parent_controls::service_result_and_retire_cannot_supply_an_authorization_answer`

## Intermittent races

Status: open. Kind: test-side races. These pass on some runs, so the row
excludes them without requiring them to fail.

- `private_originals::service_approved_input::attachment::tests::native::actual_public_attachment_loads_catalog_calls_mapped_read_and_completes_exact_b_once_with_cancellation`:
  tolerate a leading `Refused { ReprepareRequired }` in the controller
  audit; its later cold restore then belongs to the first class above.
- `private_originals::governed_submit::tests::first_input_tests::helper_entry_native::fresh_helper_reset_catalog_calls_require_explicit_successor_parent`:
  a single claim attempt right after the run settles; retry it within a
  deadline.
- `installed_host::acquisition::continuation::owner_tests::cold_source_tests::custody_recovery_wake_tests::terminal_census_wakes_existing_completion_after_native_source_restore`:
  the terminal census does not wake the existing completion after the
  native source restore, and the test's wait ended in `Elapsed` on every
  run until it passed in the row's known-red run on main push 41635572661.
  Its class is not yet determined: the reopen fixture's attachment and
  settlement signals may race the delivery owner. First decide which
  answer a promoted row awaiting its restore owes, then let the signals
  drain before the owner arms.

## Refusal after prepared

Status: closed. No test of this class is on the known-red list: its one
test was fixed in test code and runs in the row again. The section stays as
the record of the decision the tests' audit check implements.
Kind: test-side (the audit check, and a fixture that reopened without the
close's final release).

Decided: a refusal to rebuild recorded after an operation's `Prepared`
audit entry is a waited-out re-preparation, not a defect, when all of this
holds: its reason is `reprepare_required`; its read basis is
`change_in_progress` or `publication_changed` (one read state, a
publication that changed under the attempt: the first while the change was
still in progress as the attempt ended, the second once it had completed);
a later `Prepared` entry of the same operation follows it; and the
operation's success follows that entry. It is what a check made again
during an owner change leaves once its bounded wait settled: Core's own
staging of a dispatch, a Connect check around a tool's effect
(`current_check::check_after_owner_change`), or a test gate that uses it.
`tool_succeeded_in` accepts exactly that shape
(`a_tool_checked_again_during_an_owner_change_after_its_preparation_still_succeeded`
pins it and its limits: both read bases are accepted, an owner answer and a
refusal with no read are not; the later `Prepared` and the success behind
it must be the checked operation's own; a success recorded before the later
`Prepared` does not count).

The check examines the one operation that prepared the named tool. Another
operation's `Prepared` or success never excuses the checked operation's
refusal, and a second operation of the same tool fails "must prepare once".

Kept fatal, and these are real product outcomes, not only test shapes: a
`reprepare_required` refusal after `Prepared` that no later `Prepared` and
success follow. Product paths leave it, among them:
- a waiting check whose bounded wait does not settle
  (`current_check::waiting` returns the last refusal: one deadline of
  `OWNER_CHANGE_WAIT_BOUND` from its first wait, within the call's remaining
  tool time, and the free rebuilds, neither renewed);
- a bare check answered at once by a caller that holds custody or is past
  its deadline (`check_before_effect`, the only check of `current_check`
  that product code uses bare; for example `refused_at_deadline` in
  `installed_host/acquisition/task_approval.rs`).
In these two the caller gets Core's typed refusal to rebuild and the call
ends refused. The account request tools check without a wait and are
tracked as their own product item.
A refusal of any other reason and an unavailable answer stay fatal whatever
follows them.

Follow-ups, residuals of the test's audit check (noted, not changed):
- the count is unbounded: the check accepts any number of refusals to
  rebuild and re-preparations of the operation before its success, where a
  product check has a few free rebuilds and one bounded wait;
- an error outcome recorded between the re-preparation and the success is
  not examined: the check reads only the operation's refused and
  unavailable entries.

History. The class had one test, an intermittent known red:
`fresh_host_restores_governed_peer_binding_and_replays_it_exactly` of
`private_originals::governed_submit::tests::dynamic_member_native_tests::peer_ingress_tests`.
It had two failures. (1) The restored peer binding's `reply_to_peer`
operation is refused `reprepare_required` (basis `change_in_progress`)
after its `Prepared` audit entry, by Core's staging between the dispatch
entry and the execution policy entry, then prepared again and successful;
the check at `peer_ingress_tests.rs:344` refused that shape (1 of 20 runs
alone on the 0.9.10 tree, 1 of 100 on 4 CPUs shared with 8 busy loops on
0.9.12) and accepts it now. (2) Under the same load the fresh host's
restore open answered `Unavailable`
(`installed_host/controller_first_input_fixture.rs:1653`, 6 and 12 of 100
runs on 0.9.12, 13 of 100 on 0.9.11), before any audit entry was read: the
grant store's owner lease, an exclusive lock that an open takes without
waiting, was still held by the closed host. This fixture's reopen never
made the close's final release (`release_native_custody`), which the
process's close makes after its joins, so the lease ended only when the
last reference to the old owner dropped. The failed open wrote nothing, and
the same open a moment later succeeded with the binding intact.
`reopen_with_recorder` makes the release now. With both fixes the test
passed 100 of 100 runs under that load and left the known-red list. It was
an intermittent entry, so the suite's filter went from 25 excluded tests to
24, and the ratchet, which runs only the known reds that fail every time,
stayed at 20. A successor process started while its predecessor still holds
the lease gets the same refusal at once, with no retry; that product
behaviour is tracked on its own.

## Unavailable right after a turn

Status: closed. No test of this class is on the known-red list: its rows
make the refused step again in test code. The section stays as the record of
what the rows do and of the product behaviour behind it.
Kind: test-side ordering over two product entry points that do not wait.

Rows of the planning launch make two host-side calls that do not wait, the
planning parent's `submit_authenticated_original` and
`prepare_workflow_planning`, often right after a turn completed. A completed
turn's source custody settles in the background, and that settlement
changes the source's owners. Made while they change, each call is answered
"unavailable now", with nothing admitted or prepared. The submission answers
`NativeOutcomeUnavailable`: the first input's controller binding resolves
the controller lineage once, with no wait, and a publication that changed
under that read is unavailable. The preparation answers `Unavailable`: both
of its observations of the publication, the first and its one rebuild, met
a change before or during the read.

The rows treated either answer as fatal. Seen: the second host input row
(`second_host_input_keeps_dedicated_g_and_refuses_foreign_and_cancelled_parent`)
failed 6 of 100 runs on 4 CPUs shared with 8 busy loops, in both shapes;
`dedicated_held_member_admits_exact_g_and_refuses_foreign_history` failed
this row's job on a push to main on 2026-11-12, in the second shape.

Decided, in two parts. Those two rows wait for each completed turn's custody
to be promoted before their next step (`custody_promoted`: the source row
records the attempt Promoted, and the source writer was taken once after
it). And the rows over the dedicated fixture's planning parent make an
unavailable step again in test code, so a row without that wait, or an owner
change of another origin, is covered too. The rule: a row makes a call again
only when it saw an owner change of another actor, and says what it saw.

Through one helper of the launch tests (`made_again` over
`OwnerChangeRetry`) a row observes the grant authority's publication before
each call. After an unavailable answer it makes the step again only when
the step published nothing itself under the call and the row saw one of
three things: that observation met a change being published, or the
observation's stamp is no longer current, or the stamp is still current and
the grant authority names a change in progress
(`LocalGrantAuthority::owner_change_in_progress`). Otherwise the answer
fails the row at once, with the answer in the failure.

The condition on the step's own publication is there because the
publication is one sequence that every owner shares, and a stamp cannot
tell who moved it. A step publishes on it too: a submission's bind issues
the original's controller grant and then persists its binding, and a
preparation's last step writes the lead source's planning evidence. So the
helper reads those facts before the call and after the answer
(`submission_published`, `planning_published`), and what the row saw counts
only when they are unchanged: a stamp that moved may be the step's own
doing, and a change the row met before the call did not refuse a step that
published after it. A step that was refused after its own first publication
is not made again, whatever refused it.

Two things about that reading. After an admitted submission the helper
checks that both of the submission's facts read as published, so the
submission's reading cannot go blind without a row failing. The
preparation's reading has no check of its own: a lead that is already bound
without evidence is prepared without the write, so "a prepared lead carries
the evidence" does not hold for every caller. That is accepted because the
evidence write is the preparation's last fallible step (after it only the
plan list's lock and its cap of 128 plans can answer unavailable), so a
reading gone blind could let through only a refusal that has almost nothing
to come from. And the reading compares before with after, so it sees
nothing for an original that is already bound before the call: the helper
is for a first submission, which all thirteen call sites make, and not for
a replay.

With a change seen the row waits while one is named, within Core's bound for
an owner change counted from the step's first unavailable answer, and then
makes the step again; with none named any more it makes it again at once,
at most Core's free rebuilds times for one step. A submission that was made
again must be the first admission of its original and the only one
(`Admitted`, not deduplicated, with its turn ticket); a row never accepts
the replay or the reconciliation of an earlier call that answered
unavailable. Only those two answers are made again: a conflict, a denial, an
error of the submission or any other outcome goes back to the row after the
one call, and an unavailable answer past either bound fails the row with
the step's name. The bounds are the retry's own, so no row's deadline is
extended. `an_unavailable_step_is_made_again_only_through_an_owner_change`
pins what counts as seen, that nothing counts for a step that published
itself, and the bounds,
`only_the_two_unavailable_answers_are_made_again` pins what is made again
(a step is read under every call, a later one included),
and `a_submission_made_again_must_admit_for_the_first_time` pins the
admission.

Which rows. The rows over the dedicated fixture's planning parent go through
the helper: thirteen call sites in eleven test files (twelve through
`submit_planning_parent` with `prepare_planning`, and the second host input
row's own loop, which also checks that the parent session's input count did
not move). The rows over fixture originals (`prepare_with_waiter_options`,
24 call sites in 8 test files, the replay controls among them) do
not: their planning parent's submission goes through `submit_until_answered`,
an older rule (it checks that nothing was admitted and submits again after a
backoff, for up to 30 s, and asks for no owner change), and their
`prepare_workflow_planning` is made bare, twice, and unwrapped. No kept run
failed there.

Limits of the rule. In one direction, what a row sees is evidence of the
cause, not proof of it: an unavailable answer of another origin that was
decided before the step's own first publication is still made again when
the row saw another actor's owner change in its window. That window is
wider than the call: it runs from the row's stamp, taken before the call,
to the row's check of it, made after the answer and after the second
reading of the step's own facts. Such a step then fails the row at the next
call in whose window no such change is seen, or at the retry's bounds.

In the other direction, three true refusals fail the row, so that the first
such red is read as one of these and not as a new defect:
- a submission that was refused by another actor's owner change after its
  own bind had issued the controller grant (the bind resolves the lineage
  again after the issuance): the stamp moved, but the step published too,
  and the row cannot tell the two apart;
- a hold of the publication's writer that ended before the row looked, and
  published nothing: nothing is left to see, where Core's own waiter rebuilds
  once for free;
- a short hold of the source collection that publishes nothing: the
  preparation ends by taking the source collection without waiting, and a
  reader that holds it at that instant makes the preparation answer
  `Unavailable` with nothing published and nothing named.

Not changed, and not established: the two entry points answer at once in an
owner change (the preparation after one rebuild); whether their production
callers make the call again was not established.

## Slow runner timeouts

Status: open. Kind: test-side bound. The row excludes it without requiring
it to fail.

- `private_originals::setup_workflow::launch::context_transfer::metadata::worker_stdio::actual_stdio_ready_runs_retained_metadata_page_and_joins_shutdown`:
  on a 4 vCPU hosted runner the child's 60 s wait for the second selected
  source's effect (`worker_stdio.rs`, after the held first source has been
  polled twice) ended in `Elapsed` in 2 of its first 3 runs, and the
  parent's 180 s bound ended the run; the child shut down cleanly each
  time. Fix: per-step diagnostics in the child, then bounds sized from the
  measured progress.
