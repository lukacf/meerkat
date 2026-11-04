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
`custody_recovery/cold.rs` names the refusing step; then fix that step.

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
