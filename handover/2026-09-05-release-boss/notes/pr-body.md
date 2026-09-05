## Summary

`discard_stale_live_session` now captures the exact current `RuntimeSessionRegistrationWitness` and awaits `unregister_session_registration_until_terminal_if_current` instead of the SessionId-keyed `unregister_session` with its ordinary 2-second `CallerGrace`.

## Why

Traced BuildBuddy invocation `3c7933e3-2c60-40dc-80f5-9634d4e48fc2` (failed 2/2) showed the real failing path in `durable_jobs_workgraph_recovery`:

`turn/start` -> stale live-session detection -> `discard_stale_live_session` -> `runtime_adapter.unregister_session` (2s `CallerGrace`) -> `UnregisterInProgress` at reopened RPC request 6.

Teardown itself starts normally and reaches runtime-loop and comms-drain quiescence, but under RBE latency it exceeds the 2-second grace. This happens before `prepare_bindings`, so #1088 could not cover it.

## Behavior

- The caller about to rematerialize the session waits for the owned teardown saga to reach terminal completion. The saga is coordinator-owned, so dropping the RPC future never aborts teardown.
- The exact witness keeps any same-SessionId replacement registration outside this teardown authority (`Ok(false)` is treated as clean).
- An absent registration is already clean.
- The existing discard/unregister error combination is preserved.

## Validation

- Local: `repo-cargo fmt --check`, `clippy -p meerkat --all-targets --all-features -D warnings`, nextest `-p meerkat -p meerkat-rpc --lib`: 1086 passed.
- Local targeted: `smoke_shared_realm e2e_smoke_durable_jobs_workgraph_recovery -- --ignored`: PASS (110.42s).
- RBE targeted: `//:e2e_smoke_turbo_s_durable_jobs_workgraph_recovery` on this branch: see PR comment.
- Reviews: rust-quality gate, architecture (meerkat-rust-zealot), MobKit lead veto: see PR comments.

Emergency train for 0.8.33. PR #1087 is excluded from this train.
