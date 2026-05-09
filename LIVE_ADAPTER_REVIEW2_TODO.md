# Live-Adapter Review-2 — Outstanding Issues

Tracking document for the second-round external review on the
`live-adapter-mvp` branch (PR #650). Eight findings: 3 architectural P1s
(refresh/open semantics + projection snapshot completeness), 2 P1 test
failures surfaced on BuildBuddy, 2 P2s (response identity + refresh ack
honesty), and 1 P3 (snapshot version monotonicity).

**Convention:** each item carries two checkboxes — `[ ] fix` `[ ] verify`.
The first is the implementer; the second is an independent agent
confirming the fix actually addresses the finding (not just compiled
away or papered over). A third state `[~] fix` means *partial* — the
in-scope work is done but a cross-file remainder is still open;
verification waits until `[~]` advances to `[x]`.

Tags: `[U]` = user-found, `[R]` = reviewer-only, `[U+R]` = both.

---

## P1 — Block MVP

### A. Refresh / Open semantics (architectural)

- [x] fix · [x] verify · **R1.** `LiveAdapterCommand::Refresh` ignores
  every field of `LiveProjectionSnapshot` except `seed_messages`. `[U+R]`
  Location: `meerkat-openai/src/live.rs:2797-2810`. The Refresh arm in
  `execute_openai_live_command` only calls `seed_history_projection`,
  so a `config/patch` that flips `model_id` / `provider_id` /
  `visible_tools` / `system_prompt` / `audio_config` leaves the hosted
  OpenAI realtime session running on stale state while RPC reports
  `refreshed: true`. Required: rebuild or reconfigure the provider
  session against the new snapshot fields. At minimum the realtime
  session needs a `session.update` event for tool list / instructions /
  audio config; for model swaps the live adapter must close and reopen
  (or reject the refresh as `Unsupported` so the caller can teardown).

  **Fix note (R-OPENAI).** The Refresh arm in
  `execute_openai_live_command` (`meerkat-openai/src/live.rs`) now does
  three things in order before returning. (1) Detects model / provider /
  audio-rate swaps via `current_model_id` / `current_provider_id`
  fields newly stamped on `OpenAiRealtimeSession` (struct fields ~line
  1146-1155, setter `set_current_identity` ~line 1196-1209, populated
  from `open_config.llm_identity` in both
  `OpenAiRealtimeSessionFactory::open_session` and `open_live_adapter`).
  Any of these returns a typed `LlmError::InvalidRequest` whose message
  names the swap and directs the caller to "close + reopen"; the pump
  translates that into a `LiveAdapterObservation::Error` with
  `ProviderError` code carrying the same message. We did not add a new
  variant to `LiveAdapterError` / `LiveAdapterErrorCode` because that
  enum lives in `meerkat-core/src/live_adapter.rs` and is owned by the
  foundation/contracts agent — the message text is the typed signal.
  (2) On a clean refresh, calls a new
  `apply_refresh_session_update_from_snapshot` method (~line 2046-2058)
  which sends `ClientEvent::SessionUpdate` built from the snapshot's
  `system_prompt` / `visible_tools` / `runtime_system_context` /
  `audio_config` (helper `openai_refresh_session_update_from_snapshot`
  ~line 631-685, with `openai_refresh_instructions_from_snapshot` ~line
  687-720) and waits for the matching `SessionUpdated` ack via the new
  shared `await_session_updated_ack` helper (~line 2065-2078, also
  reused by `refresh_projection_update`). Audio config is only emitted
  when the snapshot's pcm rate + channels match the OpenAI live
  constraints (24 kHz mono); a mismatch is rejected up front by step (1)
  with the same close+reopen error shape. (3) After the in-place
  reconfigure lands, re-runs `seed_history_projection` so newly-injected
  runtime system context and seed messages still flow.

  Regression tests (all in `meerkat-openai/src/live.rs`):
  `refresh_command_emits_session_update_for_mutated_prompt_and_tools`
  asserts exactly one `session.update` ClientEvent reaches the provider
  carrying the snapshot's `system_prompt` text inside `instructions`
  and the snapshot's `fresh_tool` inside `tools`.
  `refresh_command_rejects_model_swap_without_emitting_session_update`
  drives a snapshot with a swapped `model_id`, expects a typed
  `LiveAdapterObservation::Error` whose message contains "model swap",
  the new model id, and "close + reopen", and asserts NO
  `session.update` ClientEvent reaches the provider on the rejection
  path. The pre-existing
  `refresh_command_reseeds_provider_with_snapshot_messages` was updated
  for the new flow (also asserts the SessionUpdate now precedes the
  seed events) and migrated to a new `ChannelOpenAiLiveSession` test
  fake that pushes server-event acks AFTER each outbound client event
  is recorded — the prior `ParkingOpenAiLiveSession` raced with the
  pump's biased-select idle `next_event` poll, which would silently
  consume pre-queued acks before the Refresh command executed.

- [x] fix · [x] verify · **R2.** `LiveAdapterCommand::Open { snapshot }`
  causes a *second* round of seeding because the factory already seeded
  history. `[U]` Location: `meerkat-rpc/src/handlers/live.rs:221-232`.
  `factory.open_live_adapter(&open_config)` already passes
  `open_config.seed_messages` + `runtime_system_context` to the provider
  session before wrapping the adapter; then `handle_live_open`
  dispatches `LiveAdapterCommand::Open { snapshot }`, and the OpenAI
  arm replays `seed_history_projection` with the same messages. Result:
  duplicated history in the provider conversation before the first live
  turn. Required: either drop the `Open` dispatch in `live/open`
  entirely (rely on factory seeding) or change the OpenAI Open arm to a
  no-op when the snapshot matches what the factory already seeded; the
  refresh-time semantics (R1) are the legitimate consumer of `Open`.

  **Fix note (R-RPC).** Dropped the post-attach
  `LiveAdapterCommand::Open` dispatch from `handle_live_open` in
  `meerkat-rpc/src/handlers/live.rs` (around lines 228-244). The
  handler still builds the snapshot locally so
  `continuity_from_snapshot` can reflect the seeded state honestly,
  but it no longer issues a follow-up `host.send_command(Open)` —
  factory-time seeding owns the seed path. Re-purposed the
  `LiveAdapterCommand::Open` doc-comment in
  `meerkat-core/src/live_adapter.rs:96-110` to state explicitly that
  the variant is reserved for cross-session re-seed scenarios (resume,
  cross-session attach) where no factory-time seeding has happened
  yet; `live/open` does NOT dispatch it. The OpenAI Open arm in
  `meerkat-openai/src/live.rs:2783-2800` is left in place for those
  future scenarios — out of R-RPC scope. Regression test
  `live_open_does_not_dispatch_open_command_after_attach` in
  `meerkat-rpc/src/handlers/live.rs` proves a `RecordingAdapter`
  attached via the same host path observes zero post-attach commands.
  The OpenAI test
  `open_command_seeds_provider_with_snapshot_messages` (in
  `meerkat-openai/src/live.rs`, R-OPENAI's file) is intentionally
  untouched — it asserts that *if* the Open command is dispatched
  (e.g. by future cross-session attach paths), the OpenAI arm still
  seeds correctly. That's the correct contract under R2's new doc.

- [x] fix · [x] verify · **R3.** `build_live_projection_snapshot` drops
  `runtime_system_context`. `[U]` Location:
  `meerkat-rpc/src/handlers/live.rs:56-63`.
  `RealtimeSessionOpenConfig` keeps `runtime_system_context` separate
  from `seed_messages`, but `build_live_projection_snapshot` copies
  only `seed_messages` and the doc-comment falsely claims the runtime
  context is folded into history. Both `live/open` and the runtime
  refresh helper (`propagate_config_to_live_channels`) build this
  thinner snapshot, so typed runtime facts (peer terminal context,
  injected ops_lifecycle context, etc.) disappear from the adapter
  refresh / open command path. Required: extend
  `LiveProjectionSnapshot` with a typed `runtime_system_context` field
  (or fold it into `seed_messages` with provenance preserved) and
  thread through both seam producers.

  **Fix note (foundation phase).** Added
  `runtime_system_context: Vec<PendingSystemContextAppend>` to
  `LiveProjectionSnapshot` (`meerkat-core/src/live_adapter.rs:294-310`)
  with `#[serde(default, skip_serializing_if = "Vec::is_empty")]` so
  empty-context snapshots stay off the wire. Updated round-trip tests
  including a new `snapshot_round_trips_with_runtime_system_context`
  case (`meerkat-core/src/live_adapter.rs:706-741`). Producers
  populated: `build_live_projection_snapshot` in
  `meerkat-rpc/src/handlers/live.rs:49-86` (live/open path) and
  `build_live_projection_snapshot_for_runtime` in
  `meerkat-rpc/src/session_runtime.rs:313-336` (refresh path). Consumer:
  the OpenAI Open and Refresh arms in
  `meerkat-openai/src/live.rs:2783-2830` now forward
  `&snapshot.runtime_system_context` into `seed_history_projection`'s
  second argument instead of the previous hard-coded `&[]`.
  Doc-comments updated to drop the falsehood about folding into
  history.

### B. Test failures on BuildBuddy

- [x] fix · [x] verify · **R4.** All-features OpenAI transcript test
  fails because `AssistantTranscriptFinal` is queued behind the trailing
  delta. `[U+R]` Location: `meerkat-openai/src/live.rs:4976-4983`. The
  A13 flow in `map_server_event` queues
  `AssistantTranscriptFinal` after the trailing
  `OutputTextDeltaForItem`, so this `next_event` call receives the
  queued final for `item_audio_1` before it can read the later
  `item_audio_2` `done` event. Test:
  `provider_neutral_session_projects_output_audio_transcripts_into_text_deltas`.
  Required: drain the queued `AssistantTranscriptFinal` and assert it
  in the test, or change the event ordering intentionally so the test
  flow matches reality. The BuildBuddy
  `cargo-all-features-clippy` / `test-unit` lane fails here today.

  **Fix note (R-OPENAI).** Restructured the assertion sequence in
  `provider_neutral_session_projects_output_audio_transcripts_into_text_deltas`
  (`meerkat-openai/src/live.rs`, ~line 4992-5026 in the new file) to
  read events in the actual emit order: `item_audio_1` streaming delta
  ("birch ") → suffix delta ("seventeen") → queued
  `AssistantTranscriptFinal` for `item_audio_1` (text="birch
  seventeen") → `item_audio_2` non-incremental delta ("silver harbor")
  → queued `AssistantTranscriptFinal` for `item_audio_2` (text="silver
  harbor") → `None`. Each delta assertion now also pins `item_id` so
  cross-item bleed regresses cleanly. Production semantics unchanged —
  only the test alignment with the queue ordering moved. Verified
  green via `./scripts/repo-cargo test -p meerkat-openai
  --features=realtime --lib
  provider_neutral_session_projects_output_audio_transcripts_into_text_deltas`
  and `./scripts/repo-cargo test -p meerkat-openai --all-features
  --lib` (177 tests, all green).

- [x] fix · [x] verify · **R5.** WS roundtrip test sends input to a
  channel that has no attached adapter. `[U+R]` Location:
  `meerkat-live/src/transport.rs:625-637`. The test opens a channel,
  never attaches a ready adapter, then sends input and unwraps a
  client `Close`. The server is allowed to tear the socket down once
  `send_input` or `next_observation_raw` fails — exactly the Linux
  BuildBuddy broken-pipe failure. Required: attach a ready stub
  adapter if the test really is exercising input roundtrip, or narrow
  the assertion to the token / upgrade path only. Reproduces in BB's
  `meerkat_live_unit_test` lane today.

  **Fix note (R-TRANS, Option A).** The test name +
  shape (`write.send(text input)` then `Close`) clearly intends to
  exercise the full input roundtrip path (upgrade → consume_token →
  `host.send_input` → adapter.send_command), so the proper fix is to
  attach a Ready adapter rather than narrow assertions. Updated
  `websocket_roundtrip_with_token` at
  `meerkat-live/src/transport.rs:611-660` to call
  `host.attach_adapter(&channel_id, Arc::new(IdleAdapter)).await.unwrap()`
  followed by
  `host.apply_status_update(&channel_id, LiveAdapterStatus::Ready).await.unwrap()`
  before minting the token. `IdleAdapter` already lived in the same
  test mod (line 667) — its `send_command` returns `Ok(())` and
  `next_observation` is `pending::<()>()` forever, so the WS pump's
  observation arm idles cleanly and the server only exits when the
  client's `Close` frame arrives. The test no longer relies on any
  server-initiated close frame; in fact it never reads from `_read`
  at all. Inline doc-comment on the test body explains the intent
  and why the stub adapter is required. Verified locally:
  `./scripts/repo-cargo nextest run -p meerkat-live transport::tests`
  → 15/15 pass.

---

## P2 — Must fix before real merge

### C. Response identity & refresh ack

- [x] fix · [x] verify · **R6.** `RealtimeSessionEvent::TurnCompleted`
  carries `response_id` but the OpenAI translator erases it before
  emitting `LiveAdapterObservation::TurnCompleted`. `[R]` Location:
  `meerkat-openai/src/live.rs:2936-2938`. The projection sink buffers
  assistant finals only by `SessionId`, so an interrupted, stale, or
  overlapping response can cause the next `response.done` (with the
  newer turn's `stop_reason`/`usage`) to flush the *wrong* buffered
  transcript. Required: extend
  `LiveAdapterObservation::TurnCompleted` with
  `response_id: Option<String>`, plumb it through
  `LiveProjectionSink::signal_turn_completed`, and key the pending-turn
  buffer on `(SessionId, response_id)`.

  **Fix note (foundation phase).** Extended
  `LiveAdapterObservation::TurnCompleted` with
  `response_id: Option<String>` (`#[serde(default,
  skip_serializing_if = "Option::is_none")]`) at
  `meerkat-core/src/live_adapter.rs:215-227`; new round-trip test
  `observation_turn_completed_round_trips_with_response_id` plus the
  existing test updated to assert absent-on-wire skip
  (`meerkat-core/src/live_adapter.rs:786-825`). OpenAI translator
  populated at `meerkat-openai/src/live.rs:2944-2956` from
  `RealtimeSessionEvent::TurnCompleted.response_id`. Trait signatures
  on `LiveProjectionSink::append_assistant_final` and
  `LiveProjectionSink::signal_turn_completed` extended with
  `response_id: Option<&str>`
  (`meerkat-live/src/host.rs:255-318`); `apply_observation` arms in
  `meerkat-live/src/host.rs:830-907` thread the id from
  `AssistantTranscriptFinal.response_id` and `TurnCompleted.response_id`
  through. Sink buffer rekeyed from `HashMap<SessionId, PendingTurn>`
  to `HashMap<(SessionId, Option<String>), PendingTurn>` in
  `meerkat-rpc/src/live_projection_sink.rs:67-138`; new helper
  `drain_all_pending_turns` cleans every slot for a session on
  interrupt/terminal-error. New regression test
  `r6_assistant_final_buffer_keys_on_response_id` at
  `meerkat-rpc/src/live_projection_sink.rs:1024-1075` proves
  interleaved responses do not pool. `RecordingProjectionSink` test
  fixtures in `meerkat-live/src/host.rs:2615-2733` updated to record
  the new `response_id` slot in their tuple shape.

- [x] fix · [x] verify · **R7.** `live/refresh` returns
  `refreshed: true` immediately after `host.send_command`, before the
  adapter pump has actually applied the refresh. `[R]` Location:
  `meerkat-rpc/src/handlers/live.rs:436-440`.
  `execute_openai_live_command` can fail later and only surface as an
  async `Error` observation — but the RPC has already returned success.
  Required: either await an acknowledgement from the adapter (extend
  the command channel with a oneshot reply) or rename the field to
  `refresh_enqueued: true` and document that callers must observe the
  realtime stream for the actual outcome. Preferred: oneshot reply.

  **Fix note (R-RPC).** Took the rename + honest-doc path. Oneshot
  reply was rejected because `OpenAiLiveAdapter::send_command` is a
  fire-and-forget mpsc enqueue
  (`meerkat-openai/src/live.rs:2622-2630`); wiring an end-to-end ack
  would require modifying the OpenAI pump to send back through a per-
  command reply channel, and that file is R-OPENAI's scope this wave.
  Renamed the success-reply field from `refreshed: true` to
  `refresh_enqueued: true` in
  `meerkat-rpc/src/handlers/live.rs::handle_live_refresh` Ok arm.
  Doc-comment on `handle_live_refresh` now states explicitly that the
  reply only confirms enqueue and that the realtime stream is the
  source of truth for the actual outcome (failures appear as
  `LiveAdapterObservation::Error`). Inline comment notes that a future
  revision can add per-provider ack plumbing without breaking the
  field name. Regression test
  `live_refresh_success_reply_is_refresh_enqueued_not_refreshed` in
  `meerkat-rpc/src/handlers/live.rs` pins both the new field name and
  the absence of the old `refreshed` key. No host-side reply channel
  was added; `host.send_command` and `next_snapshot_version` continue
  to be the only host surfaces touched by this wave.

---

## P3 — Cleanup before final

### D. Snapshot version monotonicity

- [x] fix · [x] verify · **R8.** Refresh snapshots are stamped with
  `snapshot_version: 0` instead of pulling from
  `host.next_snapshot_version()`. `[R]` Location:
  `meerkat-rpc/src/session_runtime.rs:313-326`. The doc-comment says
  the host owns version monotonicity, but
  `propagate_config_to_live_channels` builds the snapshot inline and
  ships it directly. Current OpenAI ignores the field, but any future
  adapter relying on `snapshot_version` for stale-refresh detection
  will see every refresh as generation zero. Required: route
  refresh-snapshot construction through the host's
  `next_snapshot_version` accessor (or move the snapshot stamping
  inside `host.send_command(Refresh)` so the host is the single owner
  of the field).

  **Fix note (R-RPC).** Wired the existing host accessor
  `LiveAdapterHost::next_snapshot_version(&channel_id)` (already
  present at `meerkat-live/src/host.rs:1216-1227`, backed by
  `ChannelState.snapshot_version: u64` increment under the host
  lock) into both Refresh dispatch sites. (1)
  `handle_live_refresh` in `meerkat-rpc/src/handlers/live.rs` now
  builds the snapshot, then calls `host.next_snapshot_version` and
  overwrites `snapshot.snapshot_version` before
  `host.send_command(Refresh)` — `ChannelNotFound` from the version
  call is mapped to `INVALID_PARAMS` consistent with the rest of the
  handler. (2) `propagate_config_to_live_channels` in
  `meerkat-rpc/src/session_runtime.rs` does the same; a benign
  `ChannelNotFound` here (another task closed the channel between
  `active_channels()` and the stamp call) is logged at `debug` and
  skipped, matching the surrounding sweep semantics. Doc-comments on
  both `build_live_projection_snapshot` and
  `build_live_projection_snapshot_for_runtime` now state explicitly
  that `snapshot_version: 0` is a placeholder and the dispatcher
  overwrites it. Regression tests in
  `meerkat-rpc/src/handlers/live.rs`:
  `host_next_snapshot_version_is_strictly_monotonic_per_channel` (3
  consecutive calls strictly increase) and
  `refresh_dispatch_stamps_snapshot_version_from_host` (a recording
  adapter receives 3 Refresh commands with strictly monotonic
  `snapshot_version` values, none of which are the placeholder 0).

---

## Confirmed clean from previous review

- WS audio format negotiation (was H43, P1#4) — `&format=pcm_24k_mono`
  is wired and tested.
- Structured transcript routing — full identity tuple plumbed through
  `LiveTranscriptIdentity`.
- Real truncation IDs — `truncate_assistant_transcript` rejects
  fabricated empties via typed `Rejected`.
- User transcript idempotency — `UserTranscriptFinal` routed through
  `RealtimeTranscriptEvent` when item_id present.
- Capability discovery — `LiveAdapter::capabilities()` returns truthful
  values per provider.
- No-dispatcher tool errors — `ObservationOutcome::ToolCallSkipped`
  submits `SubmitToolError`.
- Stale chunking expectation — frame count derives from constants.

---

## Counts

- **P1 (block):** 5 items (R1-R5)
- **P2 (must-fix):** 2 items (R6-R7)
- **P3 (cleanup):** 1 item (R8)
- **Total actionable:** 8
