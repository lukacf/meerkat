# Live-Adapter Round-4 — Live-output lane split

Tracking document for the architectural follow-up on PR #650 after
Review-2 + Review-3 closed (12/12). Round-4 stops the live boundary
from collapsing distinct output lanes (display text, spoken transcript,
playback audio) into a single `AssistantBlock::Text` slot, locks the
wire shape for multimodal input, and removes the speculative WebRTC
bootstrap variant.

**Convention:** each item carries two checkboxes — `[ ] fix` `[ ] verify`.
First box: implementer. Second: independent adversarial verifier.

Tags: `[U]` = user-found, `[R]` = reviewer-only, `[U+R]` = both.

**Coordination critical:** there is a `stash@{0}`
(`codex-cloudbuild-bb-migration-wip-before-main-rebase`) from another
worktree's WIP. Never drop, pop, or apply it.

**Locked design decisions:**
- (A) Delete `LiveTransportBootstrap::Webrtc` from #650; keep enum
  `non_exhaustive`. WebRTC reintroduced in follow-up PR with real
  signaling shape (browser offer → Meerkat terminator answer →
  media/data channel binds to existing live channel identity).
- (B) `AssistantBlock::Transcript { text, source: TranscriptSource,
  meta: Option<Box<ProviderMeta>> }` with `enum TranscriptSource {
  Spoken }` (single variant, future-expandable). No `audio_handle` —
  retention story is a future PR. Must be wired through persistence,
  wire, schema, SDK, projection in this PR.

---

## Phase 1 — Canonical transcript content (foundation)

- [x] fix · [x] verify · **T1.** Add `AssistantBlock::Transcript`
  variant. `[U]` Add to `meerkat-core/src/types.rs`:
  ```rust
  AssistantBlock::Transcript {
      text: String,
      source: TranscriptSource,
      meta: Option<Box<ProviderMeta>>,
  }
  enum TranscriptSource { Spoken }
  ```
  `TranscriptSource` is `#[non_exhaustive]` and uses
  `serde(rename_all = "snake_case")`. Round-trip tests on a message
  carrying mixed `Text` + `Transcript` blocks. Variant must implement
  whatever traits the existing `AssistantBlock::Text` does (`Clone`,
  `Debug`, `PartialEq`, `Eq` if present, `Serialize`, `Deserialize`,
  `JsonSchema` if behind a feature gate).

  **Fix:** `meerkat-core/src/types.rs:474-480` adds
  `pub enum TranscriptSource { Spoken }` (non_exhaustive,
  rename_all = "snake_case", derives Clone+Copy+Debug+Eq+Hash+
  Serialize+Deserialize). `meerkat-core/src/types.rs:500-512` adds
  `AssistantBlock::Transcript { text, source, meta }` with the same
  derives as the rest of the enum (non_exhaustive Debug+Clone+
  Serialize+Deserialize; PartialEq via the existing hand-written impl
  extended at types.rs:574-585). `AssistantBlock` does not derive
  `JsonSchema` in core; the wire mirror in T3 carries the schema. New
  unit tests in `meerkat-core/src/types/tests.rs:898-1003`:
  `test_assistant_block_transcript_spoken_roundtrip`,
  `test_assistant_block_transcript_with_provider_meta_roundtrip`,
  `test_message_block_assistant_mixed_text_transcript_roundtrip`,
  `test_transcript_source_roundtrip_snake_case`. `TranscriptSource`
  re-exported from `meerkat-core/src/lib.rs:302`.

  **FIX-A pass (CC8):** updated drifted ranges — `474-484`→`474-480`
  (`TranscriptSource` enum), `497-510`→`500-512` (`AssistantBlock::
  Transcript` variant), and `558-572`→`574-585` (PartialEq Transcript
  arm). Source rearrangement after T1 shifted the blocks slightly;
  no behavioral change.

- [x] fix · [x] verify · **T2.** Projection / compaction must render
  transcript blocks correctly. `[R]` Find every consumer of
  `AssistantBlock::Text` (grep workspace). For each:
  - Text-projection (`text_projection` / `MessageContent::as_text` /
    similar): render `Transcript` blocks alongside `Text` so the
    transcript is visible in projections, but distinguishable
    structurally.
  - Compaction: don't lose transcript blocks during context summarization;
    transcripts are part of canonical history.
  - History replay: `Transcript` survives full session round-trip.

  **Fix:** Text-projection sites updated to include `Transcript`:
  `meerkat-core/src/types.rs:1246-1253` (`BlockAssistantMessage::
  text_blocks`), `meerkat-core/src/types.rs:1261-1273`
  (`Display for BlockAssistantMessage`),
  `meerkat-core/src/session.rs:1331-1356`
  (`Session::last_assistant_text`),
  `meerkat-session/src/ephemeral.rs:3736-3749`
  (`AppendExternalAssistantOutput` text projection),
  `meerkat-mob/src/runtime/actor.rs:189-205` (mob supervisor render),
  `meerkat-cli/src/main.rs:8612-8619` (CLI session-show prints
  transcripts as `[transcript] ...`). Provider replay paths
  (`Transcript` → `Text` re-serialization for outgoing history):
  `meerkat-gemini/src/client.rs:101-126`,
  `meerkat-openai/src/text_adapter.rs:95-115`,
  `meerkat-openai/src/client.rs:148-185`,
  `meerkat-openai/src/client_compatible.rs:277-292`,
  `meerkat-anthropic/src/client.rs:211-242`. Compaction
  (`meerkat-session/src/compactor.rs`): no edits needed — both
  `prepare_for_summarization` and `rebuild_history` clone
  BlockAssistant messages verbatim, so Transcript blocks survive
  summarization input and post-compact retained-turn replay
  unchanged. Regression coverage: the mixed Text+Transcript+Text
  round-trip test in T1 exercises `Display` and `text_blocks` lane
  fan-out; broader projection regressions are inherited from each
  touched site's existing `meerkat-session` / `meerkat-core` test
  modules (1272 nextest pass).

  **FIX-A pass (CC8):** updated drifted line ranges in this Fix note —
  `text_blocks` is now `meerkat-core/src/types.rs:1246-1253` (was
  cited as `1207-1219`) and `Display for BlockAssistantMessage` is now
  `meerkat-core/src/types.rs:1261-1273` (was cited as `1225-1240`).
  Source rearrangement during T1 doc-comment expansion shifted the
  blocks; no behavioral change.

- [x] fix · [x] verify · **T3.** Wire format + schema regen. `[R]`
  Update `meerkat-contracts/src/wire/session.rs` so the new variant
  serializes distinctly. Run `make regen-schemas`. SDK generated types
  (Python, TypeScript, Web) must include the new variant. Verify each
  SDK's projection helpers know how to extract `text` from a
  `Transcript` block.

  **Fix:** `meerkat-contracts/src/wire/session.rs:421-428` adds
  `WireAssistantBlock::Transcript { text, source, meta }` (with
  `JsonSchema` derive via `feature = "schema"`).
  `meerkat-contracts/src/wire/session.rs:472-497` adds
  `WireTranscriptSource::Spoken` with `From` conversions in both
  directions. `WireAssistantBlock::From<AssistantBlock>` extended at
  `meerkat-contracts/src/wire/session.rs:506-510` to project the
  `Transcript` arm. `make regen-schemas` produces the new
  `WireAssistantBlockTranscript` types in
  `sdks/typescript/src/generated/types.ts:2081-2084` and
  `sdks/python/meerkat/generated/types.py:2268-2271` (web SDK does
  not generate `WireAssistantBlock`, so no diff there). Hand-written
  SDK projection helpers updated: Python
  `sdks/python/meerkat/types.py:469-498` adds optional
  `source: str | None` field; `sdks/python/meerkat/client.py:3104-
  3128` extracts `block_data["source"]`. TypeScript
  `sdks/typescript/src/types.ts:207-235` adds optional
  `source?: string`; `sdks/typescript/src/client.ts:3198-3214`
  reads `blockData.source`. Both helpers continue to extract `text`
  uniformly from `Text` and `Transcript` blocks (the `data.text`
  field shape is identical). Regression: schema-freshness diff
  (`artifacts/schemas/wire-types.json`,
  `artifacts/schemas/rest-openapi.json`) shows only the
  `WireAssistantBlockTranscript` + `WireTranscriptSource` additions
  (plus pre-existing `live/refresh` RPC catalog drift from a prior
  commit).

  **FIX-A pass (CC1 + CC8 + CC10):**
  - **CC8 line-drift:** updated `421-429`→`421-428` (Transcript variant),
    `481-507`→`472-497` (`WireTranscriptSource` block + bidirectional
    `From` impls), and `519-523`→`506-510` (forward `Transcript` arm)
    in this Fix note. Source movement during T3+T10 work shifted the
    blocks; no behavioral change.
  - **CC1:** added the inverse `From<WireAssistantBlock> for AssistantBlock`
    in `meerkat-contracts/src/wire/session.rs` (immediately after the
    forward impl), with a private helper
    `wire_provider_meta_to_core(WireProviderMeta) -> Option<ProviderMeta>`
    that returns `None` for `WireProviderMeta::Unknown` (lossy floor
    for future provider variants). Wire-side `Unknown` decays to an
    empty `AssistantBlock::Text { text: "", meta: None }` — the
    symmetric counterpart of the forward direction's `_ => Self::Unknown`
    fallback. Round-trip test
    `test_assistant_block_core_wire_core_round_trip_symmetric` covers
    Text + Reasoning + Transcript (with provider meta on each), ToolUse
    (opaque `Box<RawValue>` args), ServerToolContent, and Image.
  - **CC10:** the `From<TranscriptSource> for WireTranscriptSource`
    wildcard arm previously silently coerced unknown variants to
    `Spoken`. Replaced with `debug_assert!(false, ...)` (so debug/CI
    builds panic loudly when a new `TranscriptSource` variant lands
    without an explicit arm) plus a documented release-build fall-back
    to `Spoken`. `meerkat-contracts` does not depend on `tracing`, so
    a `tracing::error!` log was rejected; `unreachable!()` was
    rejected by the no-panic-in-library-code rule.
  - **T3-extension:** the round-trip fixture
    `test_wire_session_history_roundtrip_mixed_messages` now also
    exercises `WireAssistantBlock::Transcript { source: Spoken }`
    alongside the existing `Reasoning`+`Text` blocks, pinning the
    spoken-transcript variant through the JSON round-trip.

- [x] fix · [x] verify · **T4.** Delete
  `LiveTransportBootstrap::Webrtc`. `[U]` Remove the variant from
  `meerkat-core/src/live_adapter.rs` (and `LiveIceServer` if not
  reused elsewhere — grep first). Keep the enum `#[non_exhaustive]`.
  Update tests + SDK codegen. Inline doc-comment on the enum
  documenting why the variant was removed and that a future
  signaling-based shape will reintroduce WebRTC.

  **Fix:** `meerkat-core/src/live_adapter.rs:386-410` —
  `LiveTransportBootstrap::Webrtc` and the `LiveIceServer` struct
  (only consumer was the deleted variant; cross-crate grep found no
  other uses) are deleted; `LiveTransportBootstrap` retains
  `#[non_exhaustive]` and only `Websocket { url, token }`. New
  doc-comment lives at lines 386-405 explaining the reintroduction
  plan: future `live/webrtc/open` (or extended `live/open`) RPC
  returns a signaling endpoint + token; the browser owns
  `RTCPeerConnection`, calls `getUserMedia`, generates the offer SDP,
  POSTs it to the terminator which negotiates with the upstream
  provider and returns answer SDP + ICE; resulting media/data
  channel binds to the existing live channel identity. Test bodies
  removed at `meerkat-core/src/live_adapter.rs:~1167` (formerly
  `transport_bootstrap_is_tagged_not_bare_url` body shrunk to keep
  only the websocket assertion; the four ice/webrtc tests
  `ice_server_round_trips_with_credentials`,
  `ice_server_omits_optional_fields_when_absent`,
  `webrtc_bootstrap_carries_typed_ice_servers`, plus the webrtc half
  of `transport_bootstrap_is_tagged_not_bare_url` are removed). SDK
  regen via `make regen-schemas` drops the `Webrtc` variant from the
  generated types automatically; no hand-written SDK code referenced
  it (grep was clean).

  **FIX-A pass (CC9):** the `meerkat-rpc/src/handlers/live.rs`
  module-doc comment still claimed the transport bootstrap was tagged
  `(websocket/webrtc)`, which referenced the deleted `Webrtc`
  variant. Updated to read "currently `websocket` only" with a link
  to `LiveTransportBootstrap` for the reintroduction note.

---

## Phase 2 — Typed lane split at the live boundary

- [x] fix · [x] verify · **T5.** Split `LiveAdapterObservation`
  output lanes. `[U+R]` Today many providers send multiple "output"
  events but the Observation collapses them onto one assistant text
  channel. Required typed variants (some already exist; confirm
  they're distinct from `AssistantTextDelta`):
  - `AssistantTextDelta { text, identity }` — authored / display text
  - `AssistantTranscriptDelta { text, identity }` — spoken transcript
    (already exists as `RealtimeTranscriptEvent::AssistantTextDelta`
    routed through `AppendRealtimeTranscript` — confirm or split)
  - `AssistantTranscriptFinal { text, identity, stop_reason, usage }`
    (already exists)
  - `AssistantAudioChunk` (already exists, playback audio)
  - `UserTranscriptFinal` (already exists)
  - `ToolCallRequested` (already exists)
  - `TurnInterrupted` (barge-in) — must be documented to apply only
    to transcript + audio, not display text.

  **Fix:** `meerkat-core/src/live_adapter.rs:168-187` adds
  `AssistantTranscriptDelta { provider_item_id, previous_item_id,
  content_index, response_id, delta_id, delta }` (mirrors
  `AssistantTextDelta` identity shape so per-response buffering and
  barge-in scoping apply uniformly). Doc-comment lives on the variant
  and on `AssistantTextDelta` distinguishing display vs. spoken lanes.
  `meerkat-core/src/live_adapter.rs:241-256` rewrites the
  `TurnInterrupted` doc-comment to scope barge-in to spoken-transcript
  + audio playback only (display text preserved). New regression tests
  `assistant_transcript_delta_round_trips_with_full_ordering_identity`
  + `assistant_transcript_delta_round_trips_without_optional_identity`
  at `meerkat-core/src/live_adapter.rs:1054-1098` pin the distinct
  `observation: assistant_transcript_delta` snake_case tag and assert
  no collision with the text-delta tag.

- [x] fix · [x] verify · **T6.** Split `LiveProjectionSink` trait.
  `[U]` Currently `append_assistant_delta` / `append_assistant_final`
  cover all assistant output. Split:
  - `append_assistant_text_delta` / `append_assistant_text_final`
    (display text → flushed as `AssistantBlock::Text`)
  - `append_assistant_transcript_delta` /
    `append_assistant_transcript_final` (spoken → flushed as
    `AssistantBlock::Transcript`)
  Two parallel pending-turn buffers in
  `SessionServiceProjectionSink`, both keyed on
  `(SessionId, Option<response_id>)` per R6. Update production sink +
  every test fixture (`RecordingProjectionSink`, `BufferingTestSink`).

  **Fix:** Picked **Option A** (single buffer + lane-tagged enum).
  `meerkat-live/src/host.rs:247-307` replaces
  `append_assistant_delta` / `append_assistant_final` with four
  methods: `append_assistant_text_delta`,
  `append_assistant_transcript_delta`, `append_assistant_text_final`,
  `append_assistant_transcript_final` — symmetric identity +
  `response_id` plumbing on both lanes. `host.rs:881-984` updates
  `apply_observation` to route `AssistantTextDelta` →
  `append_assistant_text_delta`, `AssistantTranscriptDelta` →
  `append_assistant_transcript_delta`, `AssistantTranscriptFinal` →
  `append_assistant_transcript_final`. `host.rs:1320-1325` adds
  `AssistantTranscriptDelta` to `classify_observation`. Production
  sink `meerkat-rpc/src/live_projection_sink.rs:50-72` introduces
  `enum PendingAssistantContent { Text(String), Transcript(String) }`
  buffered in `PendingTurn { blocks: Vec<PendingAssistantContent> }`
  on the existing `(SessionId, Option<String>)` map.
  `live_projection_sink.rs:172-216` adds `collapse_pending_blocks` /
  `pending_to_block` helpers that group consecutive same-lane
  fragments into a single `AssistantBlock::Text` or
  `AssistantBlock::Transcript { source: TranscriptSource::Spoken }`,
  preserving inter-lane order. Test fixtures
  `RecordingProjectionSink` (`meerkat-live/src/host.rs:2856-2998`),
  `RecordingSink` + `BufferingTestSink`
  (`meerkat-rpc/src/live_projection_sink.rs:514-630, 875-1003`)
  rewritten with split text/transcript lanes. New regression
  `assistant_transcript_delta_routes_to_transcript_lane`
  (`meerkat-live/src/host.rs:2105-2135`) pins the host's
  observation→sink-method routing.

- [x] fix · [x] verify · **T7.** Barge-in truncation applies only to
  spoken-transcript / audio state. `[U]` `TurnInterrupted` →
  `signal_turn_interrupt` must drain only the transcript pending-turn
  buffer (and any audio playback state); the display-text buffer is
  preserved (display text isn't being spoken; it doesn't get
  truncated by user voice). Add a regression test that interleaves a
  display-text final with an in-flight transcript and verifies
  barge-in commits the text but discards the transcript.

  **Fix:** `meerkat-rpc/src/live_projection_sink.rs:138-160` replaces
  the prior `drain_all_pending_turns` call in
  `signal_turn_interrupt` with `drop_transcript_lane_for_session`,
  which iterates every per-session buffer slot and retains only
  `PendingAssistantContent::Text(_)` fragments. Slots are kept even
  when emptied so the `(SessionId, response_id)` keying contract
  (R6) holds for any subsequent text-final or completion. Terminal
  error (`signal_terminal_error`) still drains everything via
  `drain_all_pending_turns`. Regression test
  `t7_barge_in_drops_only_transcript_lane_keeps_display_text`
  (`meerkat-rpc/src/live_projection_sink.rs:1273-1334`) buffers a
  display-text final + a transcript final, calls
  `signal_turn_interrupt`, then drains via `signal_turn_completed`
  and asserts exactly one block of `AssistantBlock::Text` survives.

- [x] fix · [x] verify · **T8.** Capabilities matrix grows typed
  fields. `[U]` `LiveChannelCapabilities` (currently a struct of
  booleans?) becomes:
  ```rust
  struct LiveChannelCapabilities {
      audio_in: bool,
      audio_out: bool,
      text_in: bool,
      text_out: bool,
      image_in: bool,
      video_in: bool,
      transcript_supported: bool,
      barge_in_supported: bool,
      provider_native_resume: bool,
  }
  ```
  Drop any "kinds" lists (e.g., `input_kinds: Vec<String>`) — typed
  booleans only. OpenAI today reports
  `audio_in=true, audio_out=true, text_in=true, text_out=true,
  image_in=false, video_in=false, transcript_supported=true,
  barge_in_supported=true, provider_native_resume=false`. Anticipate
  `gpt-realtime-2` (image_in=true) and Gemini Live (video_in=true)
  without provider-specific fields.

  **Fix:** `meerkat-core/src/live_adapter.rs:419-477` rewrites
  `LiveChannelCapabilities` with the typed boolean matrix
  (`audio_in`, `audio_out`, `text_in`, `text_out`, `image_in`,
  `video_in`, `transcript_supported`, `barge_in_supported`,
  `provider_native_resume`); `Default` reflects the GA OpenAI
  Realtime baseline (`image_in: false, video_in: false`). The trait
  default impl at `meerkat-core/src/live_adapter.rs:531-533` now
  delegates to `LiveChannelCapabilities::default()`. OpenAI's
  override at `meerkat-openai/src/live.rs:2872-2895` reports
  `audio_in=true, audio_out=true, text_in=true, text_out=true,
  image_in=false, video_in=false, transcript_supported=true,
  barge_in_supported=true, provider_native_resume=false`.
  `meerkat-rpc/src/handlers/live.rs:207-217` rewrites the
  conservative all-false placeholder in `handle_live_open` to use
  the new field set. Test sites updated:
  `meerkat-core/src/live_adapter.rs:1325-1359`
  (`default_capabilities_enable_core_features_but_not_provider_resume`,
  `capabilities_carry_typed_image_and_video_booleans`),
  `meerkat-rpc/src/handlers/live.rs:879-892, 1102-1124`,
  `meerkat-openai/src/live.rs:6862-6878` updated to assert on the
  new field names. The wire mirror at
  `meerkat-contracts/src/wire/live.rs:46` continues to project
  `LiveChannelCapabilities` as opaque JSON (`schemars(with =
  serde_json::Value)`) so SDK codegen sees no diff for this PR;
  `make regen-schemas` confirms only Phase-1 transcript-block diffs
  in `artifacts/schemas/wire-types.json`.

---

## Phase 3 — OpenAI mapping correctness

- [x] fix · [x] verify · **T9.** OpenAI translator routes each output
  type to the correct lane. `[U]`
  - `ResponseOutputTextDelta` → `AssistantTextDelta` (display)
  - `ResponseOutputAudioTranscriptDelta` → `AssistantTranscriptDelta`
  - `ResponseOutputAudioTranscriptDone` → `AssistantTranscriptFinal`
  - `ResponseOutputAudioDelta` → `AssistantAudioChunk`
  - Truncation events (`ConversationItemTruncated` etc.) target only
    transcript + audio item ids; display-text items are not
    truncatable from a barge-in.
  Regression tests: drive a mock OpenAI server-event stream that
  emits both display text AND audio transcript in the same response;
  assert the projection sink received `Text` blocks for the former
  and `Transcript` blocks for the latter, ordered correctly.

  **Fix:** `meerkat-llm-core/src/realtime_session.rs:71-90` adds
  `RealtimeSessionEvent::OutputAudioTranscriptDeltaForItem` (mirrors
  `OutputTextDeltaForItem`'s identity shape) so the spoken-transcript
  lane has its own provider-neutral wire variant.
  `meerkat-openai/src/live.rs::note_output_audio_transcript_delta`
  (lines 1462-1490) and `note_output_audio_transcript_done`
  (lines 1505-1548) both emit `OutputAudioTranscriptDeltaForItem`
  instead of the display-text `OutputTextDeltaForItem` (streaming
  + trailing-delta). `meerkat-openai/src/live.rs:3261-3278` adds
  the translator arm `OutputAudioTranscriptDeltaForItem →
  LiveAdapterObservation::AssistantTranscriptDelta`.
  `ResponseOutputTextDelta` continues to map to
  `OutputTextDeltaForItem → AssistantTextDelta`;
  `ResponseOutputAudioTranscriptDone` still queues
  `RealtimeSessionEvent::AssistantTranscriptFinal` (its
  trailing-delta primary now flows on the transcript lane);
  `ResponseOutputAudioDelta → OutputAudioChunk → AssistantAudioChunk`
  unchanged. Truncation (`ConversationItemTruncated`) only emits
  `AssistantTranscriptTruncated` (transcript + audio scope) — display
  text is intrinsically excluded.
  Regression tests in `meerkat-openai/src/live.rs::live::tests`:
  `mapping_routes_response_output_text_delta_to_assistant_text_delta`,
  `mapping_routes_response_output_audio_transcript_delta_to_transcript_lane`,
  `mapping_routes_response_output_audio_transcript_done_to_assistant_transcript_final`,
  `mapping_routes_response_output_audio_delta_to_audio_chunk`,
  `mixed_response_emits_text_and_transcript_in_order`. Two
  pre-existing tests that inject `ResponseOutputAudioTranscriptDelta`
  (`provider_neutral_session_synthesizes_interrupted_on_response_done_cancelled_during_active_output`,
  `provider_neutral_session_does_not_double_emit_interrupted_when_response_done_cancelled_follows_speech_started`)
  + the renamed
  `provider_neutral_session_projects_output_audio_transcripts_into_transcript_deltas`
  (was `_into_text_deltas`) updated to assert against
  `OutputAudioTranscriptDeltaForItem`. All 203 realtime tests pass.

- [x] fix · [x] verify · **T10.** Projection sink no longer flushes
  transcript-as-text. `[U]` `meerkat-rpc/src/live_projection_sink.rs`
  splits the assistant-final flush into two: text final flushes
  `AssistantBlock::Text`; transcript final flushes
  `AssistantBlock::Transcript { text, source: TranscriptSource::Spoken,
  meta: ... }`. The `(SessionId, Option<response_id>)` buffer keying
  from R6 stays, but each key now maps to a typed enum
  `PendingAssistantContent::Text(...) | ::Transcript(...)` so a
  single response can persist both block types in order.

  **Fix:** Closes the Phase-2 follow-up: pre-T10 the spoken-transcript
  delta path was reusing `RealtimeTranscriptEvent::AssistantTextDelta`
  for staging, which forced the materializer at
  `meerkat-core/src/session.rs:1182-1206` (was 1161-1168) to flush
  every assistant transcript as `AssistantBlock::Text`. Fix:
  - `meerkat-core/src/realtime_transcript.rs:25-44` adds
    `pub enum TranscriptLane { Display, Spoken }` (non_exhaustive,
    rename_all=snake_case, default=Display).
  - `meerkat-core/src/realtime_transcript.rs:71-86` adds
    `RealtimeTranscriptEvent::AssistantTranscriptDelta` (same fields
    as `AssistantTextDelta`) — the dedicated spoken-transcript event.
  - `meerkat-core/src/realtime_transcript.rs:91-110` extends
    `RealtimeTranscriptMaterializedMessage::Assistant` with a
    `lane: TranscriptLane` field.
  - `meerkat-core/src/session.rs:131-155` adds `lane` field to
    `RealtimeTranscriptItemState` (`#[serde(default)]` so legacy
    state still deserializes).
  - `meerkat-core/src/session.rs:976-1018` handles the new
    `AssistantTranscriptDelta` event arm (parallel to
    `AssistantTextDelta`); both arms call the new
    `promote_item_lane` helper at
    `meerkat-core/src/session.rs:2031-2057` to tag the owning item.
  - `meerkat-core/src/session.rs:1182-1206` (the materializer) now
    dispatches on `lane`: `Display → AssistantBlock::Text { ... }`,
    `Spoken → AssistantBlock::Transcript { source: TranscriptSource::Spoken, .. }`
    — the exact flush flip that was deferred from Phase-2.
  - `meerkat-rpc/src/live_projection_sink.rs::append_assistant_transcript_delta`
    (lines 298-326) rewritten to construct
    `RealtimeTranscriptEvent::AssistantTranscriptDelta`. Helpers
    `build_assistant_text_delta_event` /
    `build_assistant_transcript_delta_event`
    (`meerkat-rpc/src/live_projection_sink.rs:175-209`) extracted so
    the event shape is unit-testable without a real `SessionRuntime`.
    The buffered finals path was already lane-correct (Phase-2 work).

  Regression tests:
  - `meerkat-core/src/session.rs::tests::realtime_transcript_assistant_transcript_delta_materializes_transcript_block`
    drives `AssistantTranscriptDelta` + `AssistantTurnCompleted`
    end-to-end through `Session::append_realtime_transcript_event`
    and asserts the materialized message is
    `AssistantBlock::Transcript { source: Spoken }`, not `Text`.
  - `meerkat-core/src/session.rs::tests::realtime_transcript_assistant_text_delta_still_materializes_text_block`
    counter-regression — display-text continues to flush as
    `AssistantBlock::Text`.
  - `meerkat-rpc/src/live_projection_sink.rs::tests::t10_assistant_transcript_delta_helper_builds_transcript_delta_event`
    pins the production sink's event-construction shape.
  - `meerkat-rpc/src/live_projection_sink.rs::tests::t10_assistant_text_delta_helper_builds_text_delta_event`
    counter-regression for the display-text helper.
  - `meerkat-rpc/src/live_projection_sink.rs::tests::t10_signal_turn_completed_flushes_transcript_block_after_transcript_final_buffered`
    end-to-end through the buffer → `collapse_pending_blocks` →
    `pending_to_block` flush path.

  Pre-1.0 dogma: `lane` added with `#[serde(default)]` so any
  on-disk transcript state still deserializes — defaulting to
  `Display`, which matches every pre-T10 session (no spoken-transcript
  items ever reached the staging map under the old code path; they
  were collapsed onto display-text deltas at the OpenAI translator).

---

## Phase 4 — Multimodal input + continuity

- [x] fix · [x] verify · **T11.** Extend `LiveInputChunk` /
  `LiveInputChunkWire` with typed image + video-frame variants. `[U]`
  ```rust
  LiveInputChunk::Audio { ... }    // existing
  LiveInputChunk::Text { ... }     // existing
  LiveInputChunk::Image { mime: String, data: Vec<u8> }
  LiveInputChunk::VideoFrame { codec: String, data: Vec<u8>, timestamp_ms: u64 }
  ```
  No provider implementation in this PR. The OpenAI adapter rejects
  image / video-frame variants with `LiveAdapterErrorCode::ConfigRejected
  { reason: "image_input_not_implemented" }` (or `"video_input_..."`).
  Wire format must round-trip via `LiveInputChunkWire` (b64 for
  binary). SDKs (Python / TypeScript / Web) get typed helpers for the
  new variants. Regression tests: each variant round-trips through
  `live/send_input` → wire → adapter; the rejection cases produce the
  typed error.

  **Fix:** `meerkat-core/src/live_adapter.rs:411-461` adds
  `LiveInputChunk::Image { mime: String, data: Vec<u8> }` and
  `LiveInputChunk::VideoFrame { codec: String, data: Vec<u8>,
  timestamp_ms: u64 }`, both with `#[serde(with = "base64_bytes")]` on
  the `data` field — same wrapper used by `AssistantAudioChunk` and
  the existing `Audio` variant, so binary payloads survive JSON
  round-trip as base64 strings rather than 6×-overhead integer arrays.
  Wire mirror at `meerkat-contracts/src/wire/live.rs:78-105` adds
  `LiveInputChunkWire::Image { mime, data }` and
  `LiveInputChunkWire::VideoFrame { codec, data, timestamp_ms }`
  (already-base64 strings on the wire side, mirrors how
  `LiveInputChunkWire::Audio` already represents binary). Decoder at
  `meerkat-rpc/src/handlers/live.rs:587-641` extends
  `live_input_chunk_from_wire` to base64-decode both new variants;
  `LiveSendInputError::InvalidImageBase64` and
  `LiveSendInputError::InvalidVideoFrameBase64` give the malformed-input
  error path the same typed-shape contract D24 already imposes on audio.
  OpenAI rejection at `meerkat-openai/src/live.rs:3166-3185` adds
  explicit `LiveInputChunk::Image { .. }` and
  `LiveInputChunk::VideoFrame { .. }` arms returning
  `LlmError::InvalidRequest { message: "image_input_not_implemented" }` /
  `"video_frame_input_not_implemented"`; the existing pump (line ~2963)
  already maps `LlmError::InvalidRequest` → `LiveAdapterErrorCode::
  ConfigRejected { reason }`. A wildcard `_` arm (required by
  `non_exhaustive`) returns a generic `"unsupported_input_chunk_variant"`
  for future unknown variants. The pump's site-list comment at
  `meerkat-openai/src/live.rs:2948-2956` is updated to reflect the
  Image/VideoFrame split. SDK helpers added at
  `sdks/python/meerkat/client.py:2499-2557` (`live_send_input_image`,
  `live_send_input_video_frame`) and
  `sdks/typescript/src/client.ts:2295-2342`
  (`liveSendInputImage`, `liveSendInputVideoFrame`); both build the
  nested-shape `chunk` payload mandated by `BREAKING_LIVE_WIRE_FORMAT_V1`.
  Regression tests: `live_input_image_round_trips_with_base64_bytes`
  and `live_input_video_frame_round_trips_with_codec_and_timestamp`
  (`meerkat-core/src/live_adapter.rs:1339-1402`) pin the core serde
  shape; `live_send_input_params_image_chunk_round_trip` and
  `live_send_input_params_video_frame_chunk_round_trip`
  (`meerkat-contracts/src/wire/live.rs:181-220`) pin the wire round-trip;
  `send_input_image_chunk_is_rejected_as_config_rejected` and
  `send_input_video_frame_chunk_is_rejected_as_config_rejected`
  (`meerkat-openai/src/live.rs:6889-7027`) drive each variant through
  the OpenAI adapter pump and assert the typed `ConfigRejected` reason.

- [x] fix · [x] verify · **T12.** `LiveContinuityMode::ProviderNativeResume`
  variant added. `[R]` Today: `Fresh` / `TranscriptOnly` / `Degraded`.
  Add `ProviderNativeResume { provider_session_id: String }` for the
  future case where the provider gives us a session id to attach
  back. Document inline that `TranscriptOnly` is honest about AEC /
  playback / pronunciation loss; `ProviderNativeResume` is the only
  mode that preserves provider-native state.

  **Fix:** `meerkat-core/src/live_adapter.rs:524-565` rewrites
  `LiveContinuityMode` with the four documented variants in fidelity
  order: `Fresh` (no prior history), `TranscriptOnly` (canonical
  transcript only — honest about AEC / playback-cursor / pronunciation /
  provider-native-state loss), `Degraded` (history seeded with known
  gaps), `ProviderNativeResume { provider_session_id }` (the only mode
  that preserves provider-native state across reconnects; reserved for
  a future provider that surfaces a usable resume id). The placeholder
  payload-less `ProviderNative` variant from the prior shape is
  removed (pre-1.0 dogma: no shims). Serde shape moves from a bare
  string (`"transcript_only"`) to internally-tagged
  (`#[serde(tag = "mode", rename_all = "snake_case")]`) so the new
  variant's payload field has somewhere to live; this matches the
  `LiveAdapterStatus` / `LiveDegradationReason` / `LiveAdapterCommand`
  pattern used elsewhere in the file. Enum is now `#[non_exhaustive]`.
  Wire mirror: the `meerkat-contracts/src/wire/live.rs` re-export
  picks up the new shape transparently (`LiveOpenResult.continuity` is
  declared as the core type with `schemars(with =
  serde_json::Value)`, so SDK codegen sees the change as a
  `serde_json::Value` shape diff rather than a typed-variant addition;
  the actual `LiveContinuityMode` JSON shape is what's authoritative
  on the wire). `continuity_from_snapshot`
  (`meerkat-rpc/src/handlers/live.rs:140-146`) is unchanged: still
  returns `Fresh` for empty seed history and `TranscriptOnly`
  otherwise — never `ProviderNativeResume` (no provider yields a
  resume id today). Regression tests:
  `continuity_mode_provider_native_resume_round_trips` and
  `continuity_mode_payload_less_variants_round_trip`
  (`meerkat-core/src/live_adapter.rs:1305-1336`) pin the new wire
  shape and the resume-variant payload survives serde;
  `continuity_from_snapshot_never_synthesizes_provider_native_resume`
  (`meerkat-rpc/src/handlers/live.rs:924-960`) asserts the open-time
  helper only emits `Fresh` / `TranscriptOnly`, locking the
  "future-only" contract.

---

## Phase 5 — Adversarial verification

- [ ] fix · [ ] verify · **T-VERIFY.** Strict adversarial pass on
  T1-T12. `[R]` Static analysis only; no cargo. Per-finding:
  - Trace full data path end-to-end (producer → wire → consumer).
  - Probe with at least 3 adversarial questions per finding.
  - Hunt for collateral issues introduced by the fix.
  - Re-examine architectural design.
  - Be skeptical of test names — does the test actually catch the
    failure mode in production?
  Acceptance test mapping (every test must be exercised somewhere in
  the codebase):
  - OpenAI audio transcript commits a transcript block, not regular
    assistant text. (T9 + T10)
  - OpenAI output text commits regular assistant text, not transcript.
    (T9 + T10)
  - A mixed response can persist display text and spoken transcript
    as distinct ordered blocks. (T9 + T10 mixed-response test)
  - Barge-in truncates only spoken-transcript / audio state. (T7)
  - `live/send_input` round-trips audio, text, image, and video-frame
    variants. (T11)
  - Capabilities can represent OpenAI `gpt-realtime-2` and Gemini Live
    without provider-specific hacks. (T8)

---

## Round-4 follow-ups (post-verifier)

The adversarial verification pass (T-VERIFY) surfaced three architectural
concerns CC2/CC3/CC4 about the **assistant-transcript commit path**:
two parallel commit paths existed (realtime-staging vs.
buffered-final-on-pending_turns) and the production OpenAI translator
emits `LiveAdapterObservation::TurnCompleted` directly — bypassing
`RealtimeTranscript { event: AssistantTurnCompleted }` — so the
realtime-staging materializer was never invoked in production.
Net effect: staged transcript items leaked in
`SessionRealtimeTranscriptState` metadata forever; only the
buffered-final path committed; barge-in did not coordinate the two
layers.

**Architectural decision (Option A): the realtime-staging path is
canonical for spoken transcript.** The buffered-final path is
restricted to **display text only** (the spoken-transcript lane is
removed from the buffer). `signal_turn_completed` synthesizes
`RealtimeTranscriptEvent::AssistantTurnCompleted` BEFORE flushing the
buffered display-text drain so the materializer commits staged
deltas as `AssistantBlock::Transcript { source: Spoken }`. Barge-in
fans `RealtimeTranscriptEvent::AssistantTurnInterrupted` to every
in-flight provider response_id (discovered via a new
`Session::in_flight_realtime_assistant_response_ids`).

- [x] fix · [x] verify · **CC2.** Production transcript commit path
  is canonical via realtime-staging materializer. `[R]`
  - `meerkat-rpc/src/live_projection_sink.rs::SessionServiceProjectionSink::signal_turn_completed`
    now synthesizes `RealtimeTranscriptEvent::AssistantTurnCompleted`
    via `runtime.append_realtime_transcript_event` BEFORE the
    buffered drain. Outcome's `is_inert()` decides usage accounting
    on the drain (skip empty drain or zero usage to prevent
    double-count).
  - `meerkat-rpc/src/live_projection_sink.rs::PendingAssistantContent`
    reduced to `Text(String)` only. The `Transcript` variant deleted;
    `collapse_pending_blocks` simplified to a single text concatenation;
    `pending_to_block` deleted.
  - `meerkat-rpc/src/live_projection_sink.rs::SessionServiceProjectionSink::append_assistant_transcript_final`
    converted to a documented no-op — the spoken text is already
    flowing through the staging path; double-buffering would
    double-commit. Doc comment explains the architectural reconciliation.
  - **Note:** `meerkat-openai/src/live.rs` (Phase-3 owned, out of
    scope for FIX-B) was left untouched. The fix lives at the host's
    sink layer rather than the translator. A future provider whose
    realtime translator routes `TurnCompleted` through
    `RealtimeTranscript { event: AssistantTurnCompleted }` will
    dedup at the staging layer (the second event for the same
    response_id is no-op'd by
    `state.assistant_completions.entry(...).or_insert(...)`).
  - Tests:
    `round4_cc2_signal_turn_completed_synthesizes_assistant_turn_completed_first`,
    `round4_cc2_realtime_materialized_skips_empty_buffered_drain`,
    `round4_cc2_realtime_materialized_with_text_drains_zero_usage`
    (`meerkat-rpc/src/live_projection_sink.rs`); session-layer
    end-to-end pin
    `round4_cc2_assistant_turn_completed_after_transcript_deltas_materializes_transcript`
    (`meerkat-core/src/session.rs`).

- [x] fix · [x] verify · **CC3.** `pending_turns` is text-only;
  `AssistantTextFinal` does not exist as a `LiveAdapterObservation`
  variant in this PR. `[R]` Per the architectural reconciliation in
  CC2, the buffered-final path is reserved for the display-text lane.
  No production observation currently emits a "display text final"
  event (production providers stream display-text deltas only); the
  buffer is exercised by tests today. The CC3 doc comment on
  `PendingAssistantContent` makes the rationale explicit so a future
  display-text-final-bearing provider knows where to wire its
  observation arm. The deleted T7 test
  (`t7_barge_in_drops_only_transcript_lane_keeps_display_text`) and
  the deleted T10 buffered-final test
  (`t10_signal_turn_completed_flushes_transcript_block_after_transcript_final_buffered`)
  are documented in-place where they used to live.

- [x] fix · [x] verify · **CC4.** Barge-in coordinates the two
  layers. `[R]`
  - `meerkat-core/src/session.rs::Session::in_flight_realtime_assistant_response_ids`
    enumerates distinct provider `response_id`s with at least one
    unmaterialized assistant item that is not already discarded.
  - `meerkat-rpc/src/session_runtime.rs::SessionRuntime::in_flight_realtime_assistant_response_ids`
    wraps the session-level helper so the sink can call it.
  - `meerkat-rpc/src/live_projection_sink.rs::SessionServiceProjectionSink::signal_turn_interrupt`
    now: (1) discovers in-flight response_ids via the runtime helper,
    (2) synthesizes `RealtimeTranscriptEvent::AssistantTurnInterrupted`
    for each so the staging layer's
    `discard_realtime_assistant_response` clears them, (3) calls the
    existing machine-authority interrupt. The display-text buffer is
    untouched (the user is not "speaking over" written output).
  - The previous `drop_transcript_lane_for_session` helper was
    deleted (the transcript lane no longer exists in the buffer).
  - Tests:
    `round4_cc4_barge_in_fans_assistant_turn_interrupted_to_in_flight_responses`
    (`meerkat-rpc/src/live_projection_sink.rs`);
    `round4_cc4_in_flight_response_ids_lists_distinct_unmaterialized_responses`
    (`meerkat-core/src/session.rs`).

**Files touched (FIX-B scope):**
- `meerkat-core/src/session.rs` — `Session::in_flight_realtime_assistant_response_ids`
  accessor + 2 new tests.
- `meerkat-rpc/src/session_runtime.rs` —
  `SessionRuntime::in_flight_realtime_assistant_response_ids` wrapper
  (single new method, no other edits).
- `meerkat-rpc/src/live_projection_sink.rs` —
  `PendingAssistantContent::Transcript` removed; `collapse_pending_blocks`
  simplified; `append_assistant_transcript_final` now no-op;
  `signal_turn_completed` synthesizes `AssistantTurnCompleted` first;
  `signal_turn_interrupt` fans `AssistantTurnInterrupted` to every
  in-flight response_id; legacy T7/T10 buffered-transcript tests
  replaced with round4 equivalents; existing P1#1/R6 tests adapted
  to the new "synthesis + drain" call shape (filter to the
  `ExternalAssistantOutput` rather than expecting a single-call shape).
- `LIVE_ADAPTER_ROUND4_TODO.md` — this section.

**Out of scope (preserved for FIX-A / FIX-C / future):**
- `meerkat-openai/src/live.rs` translator's `TurnCompleted` mapping is
  unchanged. The reconciliation lives at the sink layer; a future
  provider that wraps `TurnCompleted` in `RealtimeTranscript { event }`
  will dedup at the staging layer.
- `meerkat-live/src/host.rs` `apply_observation` routing is unchanged.
  The sink alone owns the architectural reconciliation.
- The host trait `LiveProjectionSink::append_assistant_transcript_final`
  remains in place for source-level test fixture compatibility (the
  production sink no-ops it; the `RecordingSink` test fixture still
  records it for host-level routing assertions).

**Verification commands run by FIX-B:**
- `./scripts/repo-cargo check -p meerkat-core -p meerkat-rpc -p meerkat-live -p meerkat-openai` → clean.
- `./scripts/repo-cargo nextest run -p meerkat-rpc --lib` → 481 passed, 2 skipped.
- `./scripts/repo-cargo nextest run -p meerkat-core --lib session::tests` → 41 passed (incl. 2 new round4 tests).
- `git stash list | head -3` → `stash@{0}` (`codex-cloudbuild-bb-migration-wip-before-main-rebase`) preserved.

---

## Round-4 follow-ups (FIX-C, post-verifier) — typed SDK wire surfaces

The CC5/CC6 verifier findings concerned the **wire-side projection of
`LiveChannelCapabilities` and `LiveContinuityMode`** in
`meerkat-contracts/src/wire/live.rs`: both fields were declared as the
core enum/struct but with `#[cfg_attr(feature = "schema", schemars(with =
serde_json::Value))]`, so SDK codegen produced opaque `Any` / `unknown`
typed shapes for them on `LiveOpenResult`. Net effect: SDK consumers had
to handcraft access to capability booleans (`image_in`, `video_in`,
`barge_in_supported`, …) and to the `LiveContinuityMode` discriminator.
T8's "anticipate `gpt-realtime-2`" goal and T12's bare-string-→-tagged
serde shape change were both invisible at the SDK boundary.

- [x] fix · [x] verify · **CC5.** Type the wire shape for
  `LiveChannelCapabilities`. `[R]` Replace the opaque
  `serde_json::Value` projection on `LiveOpenResult.capabilities` with
  the typed `WireLiveChannelCapabilities` mirror (struct of typed
  booleans). Add `From<LiveChannelCapabilities> for
  WireLiveChannelCapabilities` and the inverse so production handlers
  (`meerkat-rpc/src/handlers/live.rs::handle_live_open`) convert at the
  boundary. Round-trip tests on the wire mirror, on the wire ↔ core
  conversion, and on the new `LiveOpenResult` shape.

  **Fix:** `meerkat-contracts/src/wire/live.rs:55-130` adds
  `WireLiveChannelCapabilities` (typed bools: `audio_in`, `audio_out`,
  `text_in`, `text_out`, `image_in`, `video_in`,
  `transcript_supported`, `barge_in_supported`,
  `provider_native_resume`) with `JsonSchema` derive (feature-gated),
  symmetric `From` impls (lines 84-130), and the `LiveOpenResult.
  capabilities` field type swapped from `LiveChannelCapabilities` to
  `WireLiveChannelCapabilities` (lines 39-51). Handler
  `meerkat-rpc/src/handlers/live.rs:386-396` updated to call
  `capabilities.into()` at the boundary; the unit test at
  `live_open_result_roundtrip` (handlers/live.rs:909-933) constructs
  the typed wire mirror directly. Schema emit list extended at
  `meerkat-contracts/src/emit.rs:130-135`. Codegen
  `tools/sdk-codegen/generate.py:315-330, 855-870, 1217-1224` emits the
  typed shape in Python (`@dataclass`) and TypeScript
  (`interface`); `_promote_nested_schema_def` allow-list extended so
  `LiveOpenResult.capabilities` references `WireLiveChannelCapabilities`
  by name instead of inlining the schema-local `$defs` as
  `dict[str, Any]` / `unknown`. Hand-written SDK helpers updated:
  Python `sdks/python/meerkat/types.py:69-79` re-exports the wire
  mirror types; `sdks/python/meerkat/client.py:2429-2469` documents
  the typed-access contract on `live_open`. TypeScript
  `sdks/typescript/src/index.ts:199-217` re-exports the typed shapes
  (already returned typed from `liveOpen` since the `LiveOpenResult`
  interface now references the typed mirror by name). Tests:
  `wire_live_channel_capabilities_round_trip_serde`,
  `wire_live_channel_capabilities_round_trip_through_core`,
  `wire_live_channel_capabilities_anticipates_future_modalities`,
  `live_open_result_typed_capabilities_and_continuity_round_trip`
  (`meerkat-contracts/src/wire/live.rs::tests`); Python
  `test_generated_wire_live_channel_capabilities_exposes_typed_booleans`,
  `test_generated_wire_live_channel_capabilities_constructs_for_gpt_realtime_2`,
  `test_generated_live_open_result_references_typed_capabilities_and_continuity`
  (`sdks/python/tests/test_types.py`).

- [x] fix · [x] verify · **CC6.** Type the wire shape for
  `LiveContinuityMode`. `[R]` Same pattern as CC5: typed
  `WireLiveContinuityMode` mirror (internally tagged on `mode`) on
  `LiveOpenResult.continuity`, with both-direction `From` conversions
  and round-trip tests. Wire shape stays byte-identical to the core
  enum (T12's `#[serde(tag = "mode", rename_all = "snake_case")]`
  shape); the wire mirror exposes that shape to schema codegen so SDKs
  see a discriminated union, not an opaque blob.

  **Fix:** `meerkat-contracts/src/wire/live.rs:132-205` adds
  `WireLiveContinuityMode` (internally-tagged on `mode`,
  `#[non_exhaustive]`, `JsonSchema` derive) with all four core variants
  (`Fresh`, `TranscriptOnly`, `Degraded`, `ProviderNativeResume {
  provider_session_id }`). Forward `From<LiveContinuityMode>` carries a
  `debug_assert!(false, ...)` wildcard fallback (matching the
  established `WireTranscriptSource` pattern at
  `meerkat-contracts/src/wire/session.rs:482-510` — the crate doesn't
  depend on `tracing` and the no-panic-in-library-code rule forbids
  `unreachable!()`); reverse `From<WireLiveContinuityMode>` is
  exhaustive without a wildcard (the wire mirror is owned by this
  crate, so `#[non_exhaustive]` only constrains downstream callers).
  Handler `meerkat-rpc/src/handlers/live.rs:386-396` converts
  `continuity.into()` at the boundary. Tests:
  `wire_live_continuity_mode_payload_less_variants_round_trip`,
  `wire_live_continuity_mode_provider_native_resume_round_trip`,
  `wire_live_continuity_mode_byte_compatible_with_core`,
  `wire_live_continuity_mode_round_trips_through_core`
  (`meerkat-contracts/src/wire/live.rs::tests`). The
  `byte_compatible_with_core` test specifically asserts `serde_json::
  to_value(core) == serde_json::to_value(wire)` for both the
  payload-less and `provider_native_resume` variants — closing the
  verifier's "T12 breaking serde shape change is invisible at the
  schema layer" finding by making the wire mirror the canonical typed
  shape that SDK codegen sees. Python tests
  (`test_generated_wire_live_continuity_mode_is_tagged_union`,
  `test_generated_live_open_result_references_typed_capabilities_and_continuity`)
  assert each variant is a `TypedDict` with `mode: Required[Literal
  [...]]` so mypy/pyright can narrow on the discriminator, and the
  `provider_native_resume` payload field is typed `str` not `Any`.
  TypeScript codegen produces a discriminated union on `mode`
  (`WireLiveContinuityModeFresh | WireLiveContinuityModeTranscriptOnly
  | WireLiveContinuityModeDegraded |
  WireLiveContinuityModeProviderNativeResume`) — TS narrowing on
  `result.continuity.mode === "provider_native_resume"` exposes
  `provider_session_id: string` automatically.

**Files touched (FIX-C scope):**
- `meerkat-contracts/src/wire/live.rs` — typed wire mirrors for
  capabilities + continuity; 7 new round-trip tests.
- `meerkat-contracts/src/wire/mod.rs`, `meerkat-contracts/src/lib.rs` —
  re-export the new types.
- `meerkat-contracts/src/emit.rs` — schema emit list.
- `meerkat-rpc/src/handlers/live.rs` — handler converts core ↔ wire at
  the boundary; existing test rebuilt to construct the wire types.
- `tools/sdk-codegen/generate.py` — Python + TypeScript codegen
  registrations and `_promote_nested_schema_def` allow-list.
- `sdks/python/meerkat/types.py`,
  `sdks/python/meerkat/client.py` — re-export new types; document
  typed-access contract on `live_open`.
- `sdks/typescript/src/index.ts` — re-export new types.
- `sdks/python/meerkat/generated/types.py`,
  `sdks/typescript/src/generated/types.ts`,
  `sdks/web/src/generated/*` — regenerated.
- `artifacts/schemas/wire-types.json`,
  `artifacts/schemas/rest-openapi.json` — regenerated.
- `sdks/python/tests/test_types.py` — 4 new typed-access tests.
- `LIVE_ADAPTER_ROUND4_TODO.md` — this section.

**Out of scope (preserved for future):**
- `LiveOpenResult.transport` — the `LiveTransportBootstrap` projection
  is still `serde_json::Value` because T4 reintroduces WebRTC with a
  real signaling shape; the typed mirror is tracked separately.
- `LiveStatusResult.status` — `LiveAdapterStatus` is a typed core enum
  but still projected as opaque JSON; if the verifier flags it as a
  CC7 follow-up the same wire-mirror pattern applies.

**Verification commands run by FIX-C:**
- `./scripts/repo-cargo check -p meerkat-contracts` → clean.
- `./scripts/repo-cargo nextest run -p meerkat-contracts -E
  'test(round_trip)'` → 47/47 passed (including 7 new wire/live tests).
- `./scripts/repo-cargo nextest run -p meerkat-contracts wire::live` →
  14/14 passed.
- `make regen-schemas` → succeeds; diffs in
  `artifacts/schemas/wire-types.json`,
  `sdks/python/meerkat/generated/types.py`,
  `sdks/typescript/src/generated/types.ts` show the typed shapes
  replacing the opaque `Any` / `unknown` projections.
- `cd sdks/python && python -m pytest tests/test_types.py -k "wire_live
  or live_open_result_references"` → 4/4 passed.
- `git stash list | head -3` → `stash@{0}` preserved.

---

## Round-4 follow-ups (FIX-D, post-verifier) — mixed-response end-to-end

The CC7 verifier finding was that the prior acceptance test "mixed response
persists display text and spoken transcript as distinct ordered blocks" was
only halfway covered: `mixed_response_emits_text_and_transcript_in_order`
(`meerkat-openai/src/live.rs::tests`) verified only translation to distinct
observation variants; `t6_mixed_lane_finals_collapse_into_ordered_blocks`
(`live_projection_sink.rs::tests`, since deleted by CC2/CC3) drove
buffered-final paths the production text path no longer invokes. There was
no end-to-end test where text deltas + transcript deltas drive the
production materializer through to ordered Text+Transcript blocks in
canonical history.

- [x] fix · [x] verify · **CC7.** End-to-end mixed-response materialization
  pin. `[R]`

  **Architectural fix surfaced by the test:** the materializer at
  `meerkat-core/src/session.rs::Session::materialize_realtime_transcript_ready_items`
  previously called `append_external_assistant_blocks` once per staged item,
  which split a mixed-modality response (display item + spoken item under
  the same response_id) into TWO `Message::BlockAssistant` messages.
  CC7 fix: lift the pending-blocks accumulator OUT of the inner batch loop
  so it persists across outer-loop batches (chained items via
  `previous_item_id` force serial materialization across batches; same
  response_id must still collapse into one message). Walk the materialized
  batch, accumulating consecutive same-response_id assistant items into a
  single pending group. Flush when:
  - a User item lands (canonical-history ordering boundary),
  - an assistant item with a different response_id lands, or
  - the outer materialization loop terminates.
  Usage / stop_reason are seeded from the first item in the group;
  subsequent items in the same group naturally see `Usage::default()`
  because `usage_consumed` flips on the first assistant item, so token
  accounting stays single-counted.

  Tests added:
  - `round4_cc7_mixed_response_persists_text_and_transcript_in_order`
    (`meerkat-core/src/session.rs::tests`) — drives the production
    materializer end-to-end through `Session::append_realtime_transcript_event`:
    two `AssistantTextDelta` events + two `AssistantTranscriptDelta` events
    + one `AssistantTurnCompleted`, all under `response_id = "resp_mixed_1"`.
    Asserts `session.messages()` contains exactly ONE `Message::BlockAssistant`
    with TWO blocks in arrival order: `AssistantBlock::Text { "Here's the
    report: (still writing)", .. }` followed by `AssistantBlock::Transcript
    { text: "I'm reading the report aloud: sentence two.", source: Spoken,
    .. }`. Also asserts session usage is single-counted.
  - `round4_cc7_mixed_response_barge_in_discards_entire_in_flight_response`
    (sibling regression in same module) — same delta sequence followed by
    `AssistantTurnInterrupted` instead of `AssistantTurnCompleted`. Asserts
    NO `BlockAssistant` is committed (both lanes are discarded as a unit;
    the in-flight response is treated atomically because the user spoke
    over the assistant's reply mid-stream). A late `AssistantTurnCompleted`
    arriving after barge-in must NOT resurrect the discarded items.
  - `round4_cc7_host_apply_observation_routes_mixed_response_through_split_lanes`
    (`meerkat-rpc/src/live_projection_sink.rs::tests`) — drives
    `LiveAdapterHost::apply_observation` for the same mixed sequence and
    asserts the host routes display-text deltas to
    `RecordingSink.text_deltas` and spoken-transcript deltas to
    `RecordingSink.transcript_deltas` (distinct lanes), then routes
    `LiveAdapterObservation::TurnCompleted` to `signal_turn_completed`.
    This pins the host-level routing the production sink relies on; the
    materializer end-to-end is covered by the session.rs CC7 pin.
  - `round4_cc7_host_apply_observation_barge_in_after_mixed_deltas_signals_interrupt`
    (sibling) — barge-in after the mixed sequence routes to
    `signal_turn_interrupt`, never `signal_turn_completed`.

  **Quirks for the verifier:**
  - The session.rs CC7 test uses `previous_item_id: Some("item_display")`
    on the spoken items to mirror provider arrival semantics. This forces
    cross-batch materialization (item_display materializes in batch 1,
    item_spoken in batch 2), which is precisely the case the inner-loop-only
    accumulator missed. The fix verifies cross-batch grouping, not just
    within-batch grouping.
  - The barge-in sibling test uses `Session::append_realtime_transcript_event`
    directly — the full sink → runtime cross-layer fan-out (CC4) is covered
    by `round4_cc4_*` tests. CC7's barge-in sibling pins the per-response
    discard contract: both lanes go away as a unit, no late completion can
    resurrect them.
  - The host-level CC7 tests use `RecordingSink` (a fake; the real
    production sink is `SessionServiceProjectionSink`). The
    fake-vs-production gap is bridged by the session-level CC7 pin which
    drives the same materializer code path the production sink ultimately
    invokes via `runtime.append_realtime_transcript_event`.

**Files touched (FIX-D scope):**
- `meerkat-core/src/session.rs` — `materialize_realtime_transcript_ready_items`
  refactored to lift the pending-blocks accumulator out of the inner batch
  loop (CC7 architectural fix); two new tests
  (`round4_cc7_mixed_response_persists_text_and_transcript_in_order`,
  `round4_cc7_mixed_response_barge_in_discards_entire_in_flight_response`).
- `meerkat-rpc/src/live_projection_sink.rs` — two new host-level routing
  tests
  (`round4_cc7_host_apply_observation_routes_mixed_response_through_split_lanes`,
  `round4_cc7_host_apply_observation_barge_in_after_mixed_deltas_signals_interrupt`).
- `LIVE_ADAPTER_ROUND4_TODO.md` — this section.

**Verification commands run by FIX-D:**
- `./scripts/repo-cargo check -p meerkat-core -p meerkat-rpc -p meerkat-live -p meerkat-openai` → clean.
- `./scripts/repo-cargo nextest run -p meerkat-rpc -E 'test(round4_cc7)'` → 2/2 passed.
- `./scripts/repo-cargo nextest run -p meerkat-core --lib` → 873/873 passed.
- `./scripts/repo-cargo nextest run -p meerkat-rpc --lib` → 483/483 passed.
- `git stash list | head -3` → `stash@{0}` preserved.

---

## Counts

- **Phase 1 (foundation):** 4 items (T1-T4)
- **Phase 2 (typed lanes):** 4 items (T5-T8)
- **Phase 3 (OpenAI mapping):** 2 items (T9-T10)
- **Phase 4 (multimodal + continuity):** 2 items (T11-T12)
- **Phase 5 (verification):** 1 item (T-VERIFY)
- **Round-4 follow-ups (post-verifier):** 6 items (CC2, CC3, CC4, CC5, CC6, CC7)
- **Total actionable:** 19
