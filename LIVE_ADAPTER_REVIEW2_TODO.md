# Live-Adapter Review-2 — Outstanding Issues

Tracking document for the second-round external review on the
`live-adapter-mvp` branch (PR #650). Eight original findings: 3
architectural P1s (refresh/open semantics + projection snapshot
completeness), 2 P1 test failures surfaced on BuildBuddy, 2 P2s
(response identity + refresh ack honesty), and 1 P3 (snapshot version
monotonicity). **All 8 closed.**

**Round-3 review** added 4 findings on the refresh-time semantics that
the round-2 work introduced: full-history replay (R9), missing system
prompt (R10), no model-switch close+reopen path (R11), and
provider-error collapse (R12). The BB-side SLO-timeout finding from the
same review is owned by the user.

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

## Round-3 follow-up findings (refresh-time semantics + error truth)

These four findings emerged from the round-3 review of `28555320b8` after
R1-R8 landed. R1 introduced `session.update` + post-update re-seed; the
review uncovered four issues with that model.

### E. Refresh content semantics

- [x] fix · [x] verify · **R9.** `Refresh` replays full canonical
  history into the already-open OpenAI conversation. `[R]`
  Location: `meerkat-openai/src/live.rs:3048-3053`. The Refresh arm now
  applies `session.update` and *then* calls
  `seed_history_projection(&snapshot.seed_messages, &snapshot.runtime_system_context)`.
  But `live_open_config_for_session` builds `seed_messages` from the
  full canonical session history, not from a refresh-time delta;
  `seed_history_projection` mints fresh `conversation.item.create`
  events with no stable ids; so every `live/refresh` (and every
  `propagate_config_to_live_channels` after `config/patch`) duplicates
  every prior user/assistant turn + every system-context entry inside
  the hosted realtime conversation. After N refreshes the provider sees
  N+1 copies of the same transcript. Required: at refresh time, do NOT
  call `seed_history_projection` on the full snapshot. The provider's
  conversation already has the prior turns from the original `open` (or
  the previous refresh). Either skip seeding entirely on Refresh
  (config-only), or compute a delta of newly-injected runtime context
  vs. what the provider has already seen and forward only the new
  entries. Preferred: skip seeding on Refresh; runtime-context
  injection happens via `session/inject_context` → next-turn boundary,
  not via live-adapter refresh.

  **Fix note (R-OPENAI2).** The post-`session.update` call to
  `seed_history_projection` was deleted from the Refresh arm in
  `meerkat-openai/src/live.rs` (the call previously sat right after
  `apply_refresh_session_update_from_snapshot`, around the old line
  3074; the surrounding identity / audio guards are unchanged).
  Refresh is now config-only: emit one `session.update` carrying the
  snapshot's instructions / tools / audio config and stop. The Open
  arm (separate variant) keeps its `seed_history_projection` call —
  that remains the single authoritative seed point. Newly-injected
  runtime system context still reaches the provider because
  `apply_refresh_session_update_from_snapshot` folds
  `runtime_system_context` into the `instructions` payload via
  `openai_refresh_session_update_from_snapshot`; anything that
  genuinely needs to appear as an in-conversation item belongs to
  `session/inject_context` → next-turn boundary, NOT to live-adapter
  refresh. The previously-misnamed regression test
  `refresh_command_reseeds_provider_with_snapshot_messages` was
  renamed to `refresh_command_does_not_replay_history` and its body
  rewritten to drive 3 consecutive `Refresh` dispatches with
  non-empty `seed_messages` AND non-empty `runtime_system_context`,
  asserting the recording adapter sees exactly 3 `SessionUpdate`
  events and 0 `ConversationItemCreate` events. The test uses
  `ChannelOpenAiLiveSession` to push the matching `SessionUpdated`
  ack after each `SessionUpdate` is observed (the pump's idle
  `next_event` poll would race-consume a pre-queued ack). All 4
  refresh-related tests in `meerkat-openai` pass under
  `--features realtime`.

- [x] fix · [x] verify · **R10.** Refresh `session.update`
  instructions field is built only from `snapshot.system_prompt` +
  `runtime_system_context`, but both real snapshot builders set
  `system_prompt: None`. `[R]` Location:
  `meerkat-openai/src/live.rs:649-655`. The root system prompt lives
  inside `seed_messages` as a `Message::System` entry; the
  history-event projector explicitly drops `Message::System` and
  `SystemNotice`; so the refresh `session.update` replaces the
  provider instructions with just the language-pin / runtime-context
  text, losing the actual Meerkat system prompt. Required: populate
  `LiveProjectionSnapshot.system_prompt` in both
  `build_live_projection_snapshot` and
  `build_live_projection_snapshot_for_runtime` from the resolved
  `RealtimeSessionOpenConfig.system_prompt` (or extract it from the
  first `Message::System` in `seed_messages`). Without this fix, every
  refresh wipes the system prompt.

  **Fix note (foundation-2 phase).** `RealtimeSessionOpenConfig`
  does not yet model `system_prompt` as a typed field; the resolved
  root prompt is materialized into `seed_messages[0]` as a
  `Message::System` by `realtime_projection_root_system_message` /
  `realtime_projection_messages` in
  `meerkat-rpc/src/session_runtime.rs:341-389`. Both snapshot builders
  now consult that invariant via small extractors that match
  `seed_messages.first()` and lift the `SystemMessage.content` string.
  Producers: `extract_system_prompt_from_seed_messages` in
  `meerkat-rpc/src/handlers/live.rs:42-62` (consumed by
  `build_live_projection_snapshot` at the rebuilt `system_prompt:`
  field around line 86); mirror
  `extract_system_prompt_from_seed_messages_runtime` in
  `meerkat-rpc/src/session_runtime.rs:304-318` (consumed by
  `build_live_projection_snapshot_for_runtime` around line 340). The
  pre-existing snapshot round-trip test
  `snapshot_round_trips_with_runtime_system_context` and the new
  producer-side regressions
  `extract_system_prompt_returns_first_system_message_content` and
  `extract_system_prompt_returns_none_when_no_system_message` in
  `meerkat-rpc/src/handlers/live.rs:748-786` pin the contract. We
  preferred the typed extractor over a future
  `RealtimeSessionOpenConfig.system_prompt` field because the
  projection invariant is already enforced upstream and adding a
  parallel field would create two truths to keep in sync; an inline
  comment at both call sites documents the choice.

  **Fix note (FIX-R10, SystemNotice extension).** The original R10 fix
  only matched `Message::System` at index 0. An adversarial verifier
  observed that `realtime_projection_messages`
  (`meerkat-rpc/src/session_runtime.rs:435-444`) only rewrites
  `seed_messages[0]` when `realtime_projection_root_system_message`
  returns `Some` — when it returns `None`, the canonical session
  transcript's original first message is left in place and that can
  legitimately be a `Message::SystemNotice` (e.g. an idle pre-prompt
  session whose only lead is a runtime-injected
  `[SYSTEM NOTICE][MCP_PENDING]` notice). Both extractors
  (`extract_system_prompt_from_seed_messages` in
  `meerkat-rpc/src/handlers/live.rs` and the runtime mirror in
  `meerkat-rpc/src/session_runtime.rs`) now also match
  `Message::SystemNotice(n)` and return `n.rendered_text()` — the
  prefix-tagged form, matching the projection the root-system helper
  itself emits at `session_runtime.rs:405`. Without this arm, a
  refresh whose snapshot leads with a `SystemNotice` would silently
  emit empty instructions on `session.update` and wipe the realtime
  provider's session-level instructions. New regression tests:
  `extract_system_prompt_returns_rendered_text_for_first_system_notice`
  in the handler tests module and
  `extract_system_prompt_runtime_returns_rendered_text_for_first_system_notice`
  in the session-runtime tests module pin the rendered-text shape on
  both extractors. Inline doc-comments at both extractors now document
  why both `Message::System` and `Message::SystemNotice` are valid
  lead messages for a live snapshot and why we prefer `rendered_text()`
  over the raw `body` field.

### F. Realtime model switching

- [x] fix · [x] verify · **R11.** `config/patch` model switch has no
  close-and-reopen path for active live channels. `[R]`
  Location: `meerkat-rpc/src/session_runtime.rs:5326-5330`.
  `propagate_config_to_live_channels` only prechecks that the new
  resolved model is realtime-capable, then always enqueues `Refresh`.
  After R1 the OpenAI adapter rejects model drift via
  `LlmError::InvalidRequest` (because OpenAI realtime's
  `session.update` cannot change models), but that rejection is async
  and there is no runtime path here to close the channel and reopen
  it with the new model. A model swap therefore leaves the live
  channel in an error state rather than seamlessly switching.
  Required: detect model change in
  `propagate_config_to_live_channels` (compare new resolved
  `model_id`/`provider_id` against `LiveAdapterHost`'s recorded
  current identity for the channel), and on mismatch call
  `host.close_channel(&channel_id, CloseReason::ModelSwap)` followed
  by emitting a runtime event the surface caller can use to issue a
  new `live/open` (or surface a typed runtime event the SDK observes
  to trigger reopen). At minimum, do NOT enqueue Refresh when the
  precheck identifies a model change.

  **Fix note (R-RPC2).** Added `bound_llm_identity:
  Option<SessionLlmIdentity>` to `ChannelState` plus a thin
  `set_channel_llm_identity` setter and `channel_llm_identity` getter
  on `LiveAdapterHost` (`meerkat-live/src/host.rs`, struct field
  ~line 372-389, setter ~line 634-656, getter ~line 658-672). The
  host had no existing identity store and no typed `CloseReason`
  enum, so this is the minimal storage edit needed to support a clean
  read accessor; `open_channel` initializes the new field to `None`.
  The `live/open` handler in `meerkat-rpc/src/handlers/live.rs`
  stamps the identity immediately after `attach_adapter` succeeds
  (~line 259-289); a stamp failure is logged but does not abort
  `live/open` (worst case is a fall-through to the legacy Refresh
  path on a later config patch). `propagate_config_to_live_channels`
  (`meerkat-rpc/src/session_runtime.rs:5343-5398`) now reads the
  bound identity via the new accessor *before* enqueuing Refresh; on
  model_id or provider mismatch it logs a structured
  `tracing::info!` event with `reason = "model_swap"`, fields
  `session_id` / `channel_id` / `old_model_id` / `new_model_id` /
  `old_provider_id` / `new_provider_id`, then calls
  `host.close_channel(&channel_id)` (the host's existing single-arg
  signature — no typed `CloseReason` parameter to thread). The
  log/event is the only SDK-visible signal; reopen is the caller's
  responsibility per the inline contract comment. Audio rate change
  is intentionally out of scope (R11 covers model + provider only);
  audio mismatches still surface as the existing async
  `ConfigRejected` error from the OpenAI Refresh arm. Pure helper
  extracted as `live_channel_requires_close_for_identity_change`
  (`meerkat-rpc/src/session_runtime.rs:381-403`) so the swap-detection
  rule is unit-testable independently of the propagate flow.

  Regression tests (all in
  `meerkat-rpc/src/session_runtime.rs::tests`): pure-helper coverage
  via `r11_close_required_when_model_id_swapped`,
  `r11_close_required_when_provider_swapped`,
  `r11_close_not_required_when_identity_unchanged`,
  `r11_close_not_required_when_no_bound_identity_recorded`. Host
  accessor coverage via
  `r11_host_records_and_reads_back_bound_llm_identity` and
  `r11_channel_llm_identity_reports_channel_not_found_for_missing_channel`.
  End-to-end propagate coverage via
  `r11_propagate_closes_channel_on_model_id_swap` (records a
  *stale* identity on the host so the channel-bound identity differs
  from the session's resolved identity, drives propagate, asserts NO
  Refresh recorded on the recording adapter and host status is
  `Closed`) and `r11_propagate_dispatches_refresh_when_identity_unchanged`
  (matches the host-recorded identity to the session's resolved
  identity, drives propagate, asserts exactly one `Refresh`
  dispatched and channel status is NOT `Closed`; tolerates the
  precheck-close fallback when the mock model is not realtime-capable
  but still pins the no-Refresh-before-close property). Verified via
  `./scripts/repo-cargo test -p meerkat-live -p meerkat-rpc --lib`
  (472 passed, 0 failed) and `./scripts/repo-cargo clippy
  -p meerkat-live -p meerkat-rpc --tests -- -D warnings` (clean).

  **Fix note (R-RPC2 follow-up — CC1 + CC2).** The first R-RPC2 land
  shipped two latent gaps that the verifier caught.

  *CC1 — typed swap signal on the wire.* The original
  `propagate_config_to_live_channels` model-swap branch called
  `host.close_channel(&channel_id)` and emitted a server-side
  `tracing::info!`. SDK clients connected over WS only saw a TCP
  close indistinguishable from a network drop — no typed reason on
  the close frame, no actionable signal that the channel needs a
  reopen against a new model. Fixed by adding
  `LiveAdapterHost::signal_terminal_error(&channel_id, code:
  LiveAdapterErrorCode)` in `meerkat-live/src/host.rs`. The method
  enqueues a synthetic `LiveAdapterObservation::Error { code,
  message }` on a new per-channel `pending_synthetic_obs:
  Option<LiveAdapterObservation>` slot in `ChannelState`, then closes
  the underlying adapter via the existing `close_channel` path.
  `next_observation_raw` now checks the pending slot BEFORE
  `adapter_for` so the synthetic observation survives even after the
  adapter has been released — the WS pump in `transport.rs` reads it
  like any provider-emitted Error, `apply_observation` returns
  `ObservationOutcome::Terminal { code: ConfigRejected { reason } }`,
  and the existing `live_adapter_error_code_slug` derivation produces
  a typed `terminal:config_rejected` close-frame reason that clients
  can key on. `propagate_config_to_live_channels` in
  `meerkat-rpc/src/session_runtime.rs` replaces the bare
  `close_channel` call in the swap branch with
  `signal_terminal_error(channel_id, ConfigRejected { reason:
  format!("model_swap: {old} -> {new}") })`. New regression tests in
  `meerkat-live/src/host.rs::tests`:
  `signal_terminal_error_enqueues_synthetic_error_obs_and_closes_channel`,
  `synthetic_terminal_error_routes_through_apply_observation_to_terminal_outcome`,
  and `signal_terminal_error_on_missing_channel_returns_channel_not_found`
  pin enqueue, the apply-observation→Terminal trace, and the
  `ChannelNotFound` path.

  *CC2 — tests now actually exercise the swap branch.* The original
  `r11_propagate_closes_channel_on_model_id_swap` set
  `bound_llm_identity = gpt-realtime-1.5` against a session resolved
  to `claude-sonnet-4-5` (the default `mock_build_config()`).
  `propagate_config_to_live_channels` runs `precheck_live_open`
  first, which closed the channel via the realtime-capability gate
  before the swap branch ever fired — deleting the entire R11 swap-
  detection logic block would not have regressed the test. Both R11
  propagate tests now build their session via a new
  `realtime_build_config(model)` helper that sets `model =
  "gpt-realtime-1.5"`, `provider = Some(Provider::OpenAI)`, and the
  same `MockLlmClient` override; both materialize via `start_turn`
  before stamping the bound identity. The swap test now uses
  realtime-capable identities on both sides (`gpt-realtime` for the
  bound side vs. `gpt-realtime-1.5` for the session-resolved side),
  asserts the synthetic `ConfigRejected` Error obs is queued via a
  new `drain_pending_synthetic` test helper, and pins the reason
  string contains both old and new model ids. The no-swap sibling
  test now strictly requires Refresh to land and the channel to
  remain Open — the prior precheck-close tolerance is gone, so a
  regression that closes channels via the wrong gate fails the
  assertion immediately. Verified via
  `./scripts/repo-cargo test -p meerkat-live -p meerkat-rpc --lib`
  (538 passed, 0 failed: 64 in meerkat-live + 474 in meerkat-rpc) and
  `./scripts/repo-cargo clippy -p meerkat-live -p meerkat-rpc --tests
  -- -D warnings` (clean).

### G. Error code truth

- [x] fix · [x] verify · **R12.** Local config rejections from the
  Refresh guards surface as `LiveAdapterErrorCode::ProviderError`. `[R]`
  Location: `meerkat-openai/src/live.rs:2907-2912`. The R1 model /
  provider / audio-rate guards return `LlmError::InvalidRequest`, but
  the pump wraps every command failure as
  `LiveAdapterErrorCode::ProviderError`. That collapses
  configuration-rejected truth into provider-outage truth and forces
  clients/runtimes to parse message text to distinguish a needs-close-
  and-reopen rejection from an actual provider failure. Required: add
  a typed `LiveAdapterErrorCode::ConfigRejected { reason: ... }` (or
  similar — see the existing `Other { raw }` variant for shape
  precedent) and map `LlmError::InvalidRequest` to it in the pump's
  command-failure branch. Tests should pin which error code each
  guard's rejection produces.

  **Fix note (foundation-2 phase).** Added new variant
  `LiveAdapterErrorCode::ConfigRejected { reason: String }` at
  `meerkat-core/src/live_adapter.rs:281-294`, sitting alongside
  `ConfigurationRejected` (legacy, kept on the enum until callers
  migrate). Serializes with the snake_case tag `config_rejected` so the
  WS close-frame slug derivation in
  `meerkat-live/src/transport.rs::live_adapter_error_code_slug`
  picks it up cleanly without changes; new round-trip test
  `adapter_error_code_config_rejected_round_trips` in
  `meerkat-core/src/live_adapter.rs:1310-1334` and slug pin
  `live_adapter_error_code_slug_emits_serde_tag` extension at
  `meerkat-live/src/transport.rs:680-687` cover both serdes. Pump
  mapping site:
  `meerkat-openai/src/live.rs::run_openai_live_pump` command-failure
  arm around line 2906-2935 — `LlmError::InvalidRequest { message }`
  now maps to `ConfigRejected { reason: message.clone() }`; every
  other error variant stays on the existing `ProviderError` path. The
  pre-existing R1 regression
  `refresh_command_rejects_model_swap_without_emitting_session_update`
  was tightened (around line 6708-6745) to assert the typed
  `ConfigRejected { reason }` shape with the swap text, replacing the
  prior message-substring check on `ProviderError`.

  **Fix note (R12 review-3 cleanup, FIX-R12).** CC3: deleted the dead
  `LiveAdapterErrorCode::ConfigurationRejected` variant entirely from
  `meerkat-core/src/live_adapter.rs` (was sibling to `ConfigRejected`,
  zero production producers / consumers, only referenced in its own
  declaration and a single round-trip test). The dead variant carried
  a distinct serde slug `configuration_rejected` that risked drift
  against the live `config_rejected` slug. Removed the variant from
  the round-trip test loop in the same pass. Per pre-1.0 dogma:
  no compat shim. Confirmed via `rg "configuration_rejected|
  ConfigurationRejected"` — zero hits in tree (sdks/, artifacts/,
  docs/, code).

  CC5: broadened the doc-comment on the pump's command-failure
  mapping branch in `meerkat-openai/src/live.rs:~2922` to make
  explicit that `ConfigRejected` covers ALL local-guard rejections —
  not just close-and-reopen swaps. Enumerated the five concrete
  `LlmError::InvalidRequest` call sites that currently feed the
  mapping (`live.rs:3049` model swap, `:3060` provider swap,
  `:3073` audio mismatch, `:3109` unsupported input chunk, `:3140`
  unsupported command catch-all) and noted that swap-detecting
  clients must inspect the `reason` field rather than treating
  `ConfigRejected` as a swap signal alone. Left a `TODO(future):`
  marker for splitting `ConfigRejected` into typed sub-variants
  (e.g. `Swap` vs `UnsupportedInput`) if downstream branching
  pressure ever justifies it. Doc-only on this pass.

---

## Counts

- **P1 (block):** 5 items (R1-R5) — all closed
- **P2 (must-fix):** 2 items (R6-R7) — all closed
- **P3 (cleanup):** 1 item (R8) — closed
- **Round-3 follow-ups:** 4 items (R9-R12)
- **Total actionable:** 12 (8 closed + 4 open)
