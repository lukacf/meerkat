# meerkat-connect console test debt

The unit tests of this crate's shared setup native cases
(`resources::shared_setup::native_case`) compile only with the `console`
feature. CI runs them in the `connect[console]` unit row on the push to main
(`FEATURE_UNIT_SUITES` in `scripts/ci-cargo-lanes.mjs`).

The tests below fail in some runs. The row excludes each one by name, and
the known-red list in `scripts/ci-cargo-lanes.mjs` points each at its
section here; any other failure fails the row. The plan checks the list and
this page against each other, so a test is on both or on neither, under the
same section. When a fix makes one pass every time, take it off this page
and off the list in the same change.

The owner of every class is Connect. Test names are relative to the crate.

## Setup contention

Status: open. Kind: known limitation of Connect 0.9.10, not test debt.

These cases run two originals' setups side by side in one namespace. Under
that contention a submission or a setup helper's call can be refused before
it runs:

- a first submission, an original's or a setup helper's, is refused while
  its input is bound for native admission, or its worker submission answers
  `not_admitted`;
- the planning observation's publication changes and its rebuild runs out
  (`PublicationError::Changed`, then `ObservationChangedRebuildExhausted`),
  or its workflow commit meets a concurrent one, and the Find is refused as
  unavailable;
- a setup helper's call is refused as unavailable at its preparation while
  another call holds the selected component's entry, or is refused for a
  reprepare before it is dispatched.

Nothing runs, and each refusal is retryable. The Connect 0.9.10 release
notes list the refused first submissions under Known limitations. In the
same runs a Find can also be refused as not allowed by policy
(`access_denied`) at its work step, although no policy refused it, when its
contributing original's input is neither admitted nor awaiting admission;
the release notes list that too.

The rows below script one fixed sequence of calls, with at most one fresh
attempt after a certified refusal, so they fail in the runs where such a
refusal happens: at the refused call, or when a later wait for the work
that never ran times out. A refusal logged as denied after a row has
already failed comes from the case closing its host during cleanup, not
from the contention. The structural fix of Connect's submission path
removes the limitation, and with it these entries.

- `resources::shared_setup::native_case::two_sdk_children_share_one_preparation_and_cancel_independently`:
  a setup helper's call is refused as unavailable (`native_case.rs`,
  "refused with tool_unavailable"), and the wait for both helpers to join
  times out.
- `resources::shared_setup::native_case::observe_controls::owner_change_at_a_finds_helper_observation_is_waited_out_and_observed_again`:
  a setup helper's call is refused at its preparation, and the case times
  out waiting for its completion (`native_case.rs`, "honest native
  continuation must settle").
- `resources::shared_setup::native_case::planning_admission::two_sdk_originals_admit_one_fresh_dedicated_planning_input`:
  an original's first submission is refused at its bind.
- `resources::shared_setup::native_case::planning_admission::console_consent_case::entered_operation::registered_r2_find_enters_once_per_call_and_commits_real_context_graphs`:
  an original's first submission is refused at its bind, or the planning
  Find is refused (`planning_admission.rs`, "the actual planning Find was
  refused").
- `resources::shared_setup::native_case::planning_admission::context_delivery_tests::closed_workflow_refuses_context_replay_after_automatic_admission`:
  an original's first submission is refused at its bind.
- `resources::shared_setup::native_case::planning_admission::context_delivery_tests::committed_waiter_context_is_delivered_automatically_once_and_survives_departure`:
  the planning Find's worker submission answers `not_admitted`.
- `resources::shared_setup::native_case::planning_admission::context_transfer_tests::sdk_completed_context_survives_a_departure_and_refuses_closed_recipient`:
  the planning Find is refused as unavailable after its rebuild runs out or
  its workflow commit meets a concurrent one.
- `resources::shared_setup::native_case::planning_admission::context_transfer_tests::sdk_completed_context_validates_mutable_payload_without_replaying_find`:
  an original's first submission is refused at its bind, or the planning
  Find's worker submission answers `not_admitted` (`planning_admission.rs`,
  the `observation["state"] == "native"` check).
- `resources::shared_setup::native_case::post_effect_controls::owner_change_at_the_check_after_the_release_is_waited_out_once`:
  a setup helper's call is refused as unavailable.
- `resources::shared_setup::native_case::post_effect_controls::owner_change_at_the_install_check_is_refused_certified_and_installed_fresh`:
  a setup helper's call is refused as unavailable.
- `resources::shared_setup::native_case::post_effect_controls::owner_change_outlasting_the_check_after_the_join_ends_typed_without_replay`:
  a setup helper's call is refused as unavailable, a first submission is
  refused at its bind, or the check under control is refused for a
  reprepare before it is dispatched, so the control never sees it run.
- `resources::shared_setup::native_case::post_effect_controls::owner_change_outlasting_the_install_reservation_is_refused_certified_and_installed_fresh`:
  a setup helper's call is refused as unavailable, or an original's first
  submission is refused at its bind ("fresh native Original admission
  remains unestablished").
