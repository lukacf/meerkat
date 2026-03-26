# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.5.0] - 2026-03-26

Meerkat 0.5 is a large architecture and surface cutover. It formalizes runtime ownership around generated authorities and runtime-backed session services, removes a wide set of legacy public-surface residue, brings persistent session and mob recovery much closer to truthful replay, and adds a realm blob store for image content.

### Highlights

- Runtime-backed semantics are now the canonical public model across CLI, REST, RPC, MCP, Rust, Python, TypeScript, and Web surfaces.
- `host_mode` has been fully replaced by `keep_alive`, with stricter validation, clearer tri-state behavior, and consistent cross-surface ownership.
- Generated machine authorities, formal schemas, and seam-audit enforcement now back the most important runtime, mob, comms, turn, and ops-lifecycle semantics.
- Durable image handling moved to the new blob-backed model, with aligned history/read semantics and better multimodal behavior across providers and comms paths.
- Persistent session and mob resume behavior is much closer to truthful replay, including stronger identity continuity, broken-member handling, and runtime-backed recovery.
- Surface cancellation, commit-boundary, and external-event behavior were tightened so successful committed work is preserved and invalid/runtime-owned requests fail more honestly.

### Upgrade notes

- Treat 0.5 as a real semantic cutover from 0.4.x, not a small additive release.
- Runtime-backed session services are now the intended integration path; direct low-level construction is an expert/internal escape hatch.
- Expect `keep_alive`, blob-backed image durability, richer structured content/history models, and stronger typed lifecycle semantics across surfaces and SDKs.
- If you are upgrading an older integration, use the 0.4x -> 0.5 migration guidance in the docs/skills rather than assuming 0.4 behavior still holds.

### Added

#### Formal runtime authorities, machine schema, and seam enforcement
- New machine-authority toolchain:
  - `meerkat-machine-schema`
  - `meerkat-machine-codegen`
  - `meerkat-machine-kernels`
- New generated authority / protocol artifacts across runtime, mob, comms, external tools, and ops lifecycles.
- New formal specs and compositions under `specs/machines` and `specs/compositions`, plus `xtask` support for machine/codegen/audit workflows.
- New architecture doctrine and audit material:
  - `docs/architecture/meerkat-runtime-dogma.md`
  - `docs/architecture/formal-seam-closure.md`
  - `docs/architecture/RMAT.md`
  - `docs/architecture/finite-ownership-ledger.md`
- New CI / pre-push enforcement around schema freshness, generated artifacts, clippy cleanliness, and seam-audit drift.

#### Runtime-backed request execution and cancellation
- Shared cancellable surface request execution helpers in `meerkat::surface`.
- Runtime-backed request lifecycle and cancellation support added across:
  - JSON-RPC
  - REST
  - MCP stdio hosting
  - `examples/034-codemob-mcp`
- JSON-RPC now supports explicit request cancellation notifications.
- MCP stdio hosting now supports long-running cancellable tool execution without serially blocking the read loop.
- Successful state-advancing operations now publish committed success correctly instead of being rewritten to cancellation by late races.

#### Realm blob storage for image content
- New `BlobId`, `BlobRef`, `BlobPayload`, and `BlobStore` contracts in core.
- New built-in blob store implementations:
  - `MemoryBlobStore`
  - `FsBlobStore`
- `PersistenceBundle` now owns a matched set of:
  - `SessionStore`
  - `RuntimeStore`
  - `BlobStore`
  - `RuntimeSessionAdapter`
- New blob fetch surfaces:
  - REST `GET /blobs/{blob_id}`
  - RPC `blob/get`
  - MCP `meerkat_blob_get`
  - CLI `rkat blob get`
  - SDK blob-get helpers

#### New and expanded contracts on public surfaces
- Public session history is now fully runtime-backed and aligned across REST/RPC/MCP/SDK surfaces.
- REST external-event ingress is now canonical at `POST /sessions/{id}/external-events`.
- JSON-RPC external-event ingress is now canonical at `session/external_event`.
- REST/RPC/MCP contracts were regenerated and expanded:
  - richer RPC and REST catalogs
  - refreshed wire types and schema artifacts
  - generated web event types from contracts
- New `ErrorCode::RequestCancelled` / request-cancelled semantics in contracts.

#### Mob runtime and orchestration improvements
- New runtime-owned Broken-member projection for partial persistent resume.
- Persistent mob resume now restores missing member sessions from durable state with:
  - same `session_id`
  - preserved transcript/history
  - preserved durable LLM identity
  - preserved native inproc comms identity / `peer_id`
- New stronger mob lifecycle/orchestrator/runtime authorities and kernels.
- New real-API mob smoke coverage, including collaborative-resume and multimodal pictionary scenarios.

### Changed

#### Runtime architecture and ownership
- Runtime, comms, mob, and surface semantics now route through explicit authorities instead of shell-side lifecycle decisions.
- Input lifecycle, runtime ingress, runtime control, comms drain lifecycle, peer comms, peer reachability, ops lifecycle, turn execution, mob lifecycle, mob orchestrator, and flow-run semantics were all formalized and tightened.
- Surface code is now more explicitly “skin/mechanics only,” with semantic truth moved into authorities, protocols, and typed control seams.

#### Session service and public surface defaults
- Runtime-backed `SessionService` embedding is now the documented and tested default across Rust docs, examples, CLI, REST, RPC, MCP, and SDK guides.
- Direct `AgentBuilder` construction is now treated as an expert/internal escape hatch rather than the primary integration path.
- Session create/continue/resume behavior is more explicitly split between:
  - live/runtime-backed mutation
  - rebuild-required paths
  - committed-create vs pre-commit failure behavior

#### `host_mode` → `keep_alive`
- `host_mode` was renamed to `keep_alive` across:
  - core/session metadata
  - RPC/REST/MCP/CLI surfaces
  - SDKs
  - docs/examples/skills
  - schema/codegen artifacts
- Keep-alive now follows stricter tri-state and validation rules:
  - explicit overrides are preserved across resumed/rebuilt sessions
  - invalid keep-alive requests are rejected before stateful execution
  - disabling keep-alive now actually stops existing drain ownership

#### Image content storage and history semantics
- Durable session/runtime state no longer treats inline base64 image bytes as canonical truth.
- Durable session history and durable runtime inputs now store blob-backed image data instead of inline-only image payloads.
- Compaction now strips session images from active history, replacing them with textual placeholders, so compacted sessions do not keep paying context cost for image-bearing turns.
- History/read surfaces now align with the new blob-backed image model instead of implying old inline-image durability.

#### Mobs and comms behavior
- `AutonomousHost` behavior was tightened so active autonomous members remain live for peer ingress instead of depending on a one-shot loop handle.
- Mob runtime now uses one canonical runtime adapter per runtime instance instead of splitting turn execution and comms ingress across different adapters.
- Persistent resume no longer silently fresh-creates missing session-backed members on persistent services.
- Broken members are now consistently excluded from wiring/selection/host-loop startup paths while remaining inspectable and repairable.

#### SDKs and generated types
- Python, TypeScript, and Web SDKs were realigned with the runtime-backed/session-first API.
- Generated SDK types and helpers were updated to reflect:
  - deferred sessions
  - richer session history contracts
  - blob-backed image content
  - regenerated event/catalog artifacts
- Python and TypeScript history parsing now preserve structured text/image content instead of flattening it back to plain strings.

### Removed

- Legacy `host_mode` terminology from public docs/examples/SDKs/contracts.
- Removed old `docs/architecture/0.5/*` planning dump in favor of the new normative architecture docs plus `.rct` material.
- Removed / scrubbed legacy delegated/helper-agent and sub-agent public-surface residue that no longer matched the settled 0.5 surface model.
- Removed a variety of dead or obsolete runtime shell helpers, stale driver entry methods, and old host-mode ownership residue that no longer matched authority-owned semantics.

### Fixed

#### Persistent resume, recovery, and identity continuity
- Fixed persistent mob resume so session-backed members preserve durable identity instead of silently coming back as fresh sessions.
- Fixed idle-live session detection during mob resume; persisted-only summaries no longer masquerade as live sessions, and live idle sessions are no longer misclassified as missing.
- Fixed session-scoped native comms identity so resumed sessions preserve `peer_id` across runtime roots.
- Fixed autonomous member runtime ownership so comms drains and runtime turns use the same canonical adapter.

#### Keep-alive / continue / resume correctness
- Fixed keep-alive ordering so validation happens before stateful execution and rejected requests are side-effect free.
- Fixed keep-alive propagation across RPC/REST/MCP/CLI resume and turn paths.
- Fixed drain survival / cleanup semantics for committed keep-alive sessions on error paths.
- Fixed late-cancel races so successful committed `turn/start` / `meerkat_resume` / similar operations are not rewritten to `REQUEST_CANCELLED`.

#### Multimodal comms and image handling
- Fixed multimodal peer ingress for autonomous mob members after kickoff completion.
- Fixed comms/runtime paths that flattened or dropped multimodal image blocks.
- Fixed multimodal body-vs-rendered-text handling so raw peer message bodies are preserved instead of replaced by lossy projections.
- Fixed provider-specific image serialization regressions (including Gemini user messages and Anthropic tool-result images).

#### Surface and contract regressions
- Fixed CLI runtime-backed teardown/output pipeline regressions.
- Fixed RPC/REST/MCP runtime-backed ingress, resume, and external-event regressions.
- Fixed REST request cancellation races and cleanup paths.
- Fixed MCP cancellability and responsiveness in `034-codemob-mcp`.
- Fixed runtime batch staging, metadata merge, UTF-8 panic, callback timeout, and retry-hint propagation regressions.

#### Compaction and budget correctness
- Fixed compaction cadence fallback across reused / legacy sessions.
- Fixed compaction token estimation so base64 image data and tiny text blocks are not miscounted.
- Fixed timeout / time-budget terminalization so structured timeout conditions retain their typed meaning.

#### Clippy, WASM, CI, and publishability
- Brought the workspace to clean `clippy -D warnings` status across a very large legacy warning backlog.
- Fixed multiple WASM compilation and gating issues across `meerkat-web-runtime`, web bindings, and example paths.
- Fixed publish / path-dependency / workspace packaging issues across crates.
- Hardened pre-push and CI gates for feature branches and generated artifact freshness.

### Breaking changes

- `host_mode` has been renamed to `keep_alive` across public surfaces and generated SDK/contracts.
- Runtime-backed session services, not direct builder execution, are now the intended public integration path.
- Durable image/history semantics changed to the new blob-backed model; old inline-image durable formats are not preserved.
- Python/TypeScript/Web SDK content and history models changed to preserve structured content instead of flattening it.
- A significant amount of stale legacy/internal surface residue was removed or renamed to match the settled 0.5 contracts.

## [0.4.13] - 2026-03-16

### Fixed

- **Mob multimodal content silently discarded** — `MobHandle::send_message` took `String`, flattening multimodal `ContentInput` (images + text) to plain text before it reached the session service. Threaded `ContentInput` end-to-end through `MobHandle`, `MobCommand`, actor dispatch, `SpawnMemberSpec`, and `to_create_session_request`. TurnDriven mode now passes `ContentInput` directly to `StartTurnRequest`; AutonomousHost mode extracts text at the `EventInjector` boundary (known limitation).
- **Wire boundaries blocked multimodal mob content** — JSON-RPC mob param structs (`MobSpawnParams`, `MobSendParams`, `MobRespawnParams`) accepted `String` instead of `ContentInput`. Updated to accept `ContentInput` directly via serde untagged deserialization (backward compatible — plain strings still work). WASM bindings now parse incoming strings as `ContentInput` JSON with text fallback. Python SDK mob methods widened to `str | list[dict]`, TypeScript/Web SDK mob methods widened to `string | ContentBlock[]`.

## [0.4.12] - 2026-03-16

### Added

#### Multimodal Content Support (Images) (#154)
- **`ContentBlock` type** — `Text` and `Image` variants in `meerkat-core`, threaded through tool results (`ToolResult.content: Vec<ContentBlock>`), user messages (`UserMessage.content: Vec<ContentBlock>`), and all provider adapters. Backwards-compatible serde: plain strings deserialize to `[Text]`, text-only content serializes as string.
- **`ContentInput` type** — untagged `Text(String) | Blocks(Vec<ContentBlock>)` accepted by `CreateSessionRequest.prompt` and `StartTurnRequest.prompt` across all surfaces (REST, RPC, CLI, MCP Server).
- **`ToolOutput` enum** — `Json(Value) | Blocks(Vec<ContentBlock>)` replaces `Value` return on `BuiltinTool::call()`, enabling tools to return multimodal content.
- **`view_image` builtin tool** — reads images from disk with path sandboxing (symlink-safe via `canonicalize`), 5MB size limit, extension validation (PNG/JPEG/GIF/WebP/SVG). Returns `ToolOutput::Blocks` with base64-encoded image data. Guarded with `#[cfg(not(target_arch = "wasm32"))]`.
- **Provider capability gating** — `ModelProfile` gains `vision` and `image_tool_results` fields. `view_image` hidden via `ToolScope` external filter for models that can't process image tool results (OpenAI). Dynamic refresh on model hot-swap: filter composes with existing restrictions instead of clobbering.
- **Provider image serialization** — Anthropic: native `image.source.base64` format in user messages and tool results. OpenAI: `image_url` data URIs in user messages, text degradation for tool results. Gemini: `inlineData` parts in user messages and alongside `functionResponse` for tool results.
- **MCP image passthrough** — `McpConnection::call_tool()` returns `Vec<ContentBlock>`, capturing `image` content from MCP servers as `ContentBlock::Image`.
- **Comms multimodal plumbing** — `blocks: Option<Vec<ContentBlock>>` added alongside `body: String` at every comms layer (`MessageKind`, `CommsContent`, `InteractionContent`, `CommsCommand`, `PlainMessage`, `InboxItem`, `SendInput`). CBOR backwards compat verified. Turn-boundary drain and host-mode batching paths preserve blocks.
- **Runtime multimodal routing** — `CoreRenderable::Blocks` variant, `PromptInput.blocks` and `PeerInput.blocks` fields, `extract_prompt()` returns `ContentInput` on both RPC and REST runtime executors.
- **Wire types** — `WireContentBlock` (no `source_path`), `WireContentInput`, `WireToolResultContent` in `meerkat-contracts`. Schema regenerated. Forward-compatible `Unknown` variant.
- **SDK multimodal prompts** — Python: `prompt: str | list[dict]` on all session methods. TypeScript: `prompt: string | ContentBlock[]`. Web SDK: `ContentInput` parsed at WASM bridge.
- **Hook `has_images` flag** — `HookToolResult.has_images` and `ToolExecutionCompleted.has_images` for downstream consumers.
- **Hook patch rebuild rule** — deterministic: strip text blocks, prepend patched text, append image blocks in original order.
- **Compaction image stripping** — `strip_images_for_compaction()` replaces images with `[image: {media_type}]` placeholders. `source_path` excluded from placeholders to prevent filesystem path leaks.
- **`Display` impl for `ContentBlock`** — delegates to `text_projection()`.
- **Dispatch-time tool gating** — hidden tools (via ToolScope external filter) are blocked at execution time, not just advertisement time.

### Changed

- **`ToolResult.content`** — `String` → `Vec<ContentBlock>` (breaking Rust API). `ToolResult::new()` still accepts `String`. Use `.text_content()` for string access.
- **`UserMessage.content`** — `String` → `Vec<ContentBlock>` (breaking Rust API). `UserMessage::text()` constructor for common case.
- **`BuiltinTool::call()` return** — `Result<Value, _>` → `Result<ToolOutput, _>` (breaking for custom tool implementors).
- **`CreateSessionRequest.prompt` / `StartTurnRequest.prompt`** — `String` → `ContentInput` (breaking, use `.into()` from String).
- **`AgentRunner::run()` / `run_with_events()`** — accept `ContentInput` instead of `String`.
- **`content_blocks_serde`** — only collapses single text block to string; multi-block text arrays serialize as arrays to preserve block boundaries.
- **`source_path`** — `#[serde(skip_serializing)]`: never persisted to session stores, only used in-memory for compaction re-read hints.

## [0.4.11] - 2026-03-15

### Fixed

- **RPC in-session model switching silently dropped** — `turn/start` model/provider/provider_params overrides were built in the handler but never reached the executor because `RuntimeTurnMetadata` did not carry those fields. Added model/provider/provider_params to `RuntimeTurnMetadata`, extracted `hot_swap_llm_client()`, and wired it into `apply_runtime_turn` so overrides propagate end-to-end.
- **MCP drain race in tests** — `set_inflight_calls_for_testing` could fire before the MCP router was ready. Now waits for `wait_until_ready` first.

## [0.4.10] - 2026-03-15

### Added

#### `meerkat-models` — Curated Model Catalog (#148)
- New leaf crate `meerkat-models` as the single source of truth for model defaults, allowlists, capability detection, and parameter schemas. Consolidates model data previously scattered across `config.rs`, `config_template.toml`, and client adapters.
- Catalog module with curated entries for all supported providers (Anthropic, OpenAI, Gemini) including default models, allowed model lists, and per-model parameter schemas.
- Profile module with provider-specific rules: per-model param schemas that document exactly which `provider_params` keys each adapter reads and processes (e.g., opus-4-6 gets adaptive thinking + effort + compaction; non-gpt-5 models don't advertise `reasoning_effort`).
- `models/catalog` endpoint on all surfaces: CLI (`rkat models catalog`), REST, RPC (`models/catalog`), and MCP Server.
- Wire types in `meerkat-contracts` (`ModelsCatalogResponse`) with SDK codegen.

#### Mid-Session Model/Provider Hot-Swap (#147)
- `Agent::replace_client()` swaps the LLM client on a live agent without rebuilding.
- `SessionAgent::replace_client()` trait method (default no-op) and `SessionService::set_session_client()` (default `Unsupported`).
- RPC `turn/start` now accepts `model`/`provider`/`provider_params` on materialized sessions, builds a new client via factory and hot-swaps before the turn.

### Fixed

#### Comms Interrupt Regressions (#147)
- **False wakes from non-actionable traffic** — raw `inbox_notify` woke `wait` on responses, acks, lifecycle traffic, and plain events. Added single-pass ingress classification in `meerkat-comms` with narrow `actionable_input_notify` that fires only for `ActionableMessage`/`ActionableRequest`. Untrusted items dropped at ingress with snapshot semantics.
- **Wait tool not interruptible on some dispatcher paths** — override and WASM dispatcher paths never wired wait interruption. Added `bind_wait_interrupt()` and `supports_wait_interrupt()` on `AgentToolDispatcher` trait with implementations on `CompositeDispatcher`, `ToolGateway`, and `FilteredToolDispatcher`. Factory probes before consuming bind and falls back gracefully.
- **Wait budget overshoot** — wait could overshoot `max_duration` by up to 1800s. `MAX_WAIT_SECONDS` reduced to 60s.
- **Trust state split between async and sync locks** — collapsed `tokio::sync::RwLock` and `parking_lot::RwLock` sidecar into single `Arc<parking_lot::RwLock<TrustedPeers>>` shared by Router, `IngressClassificationContext`, and `trusted_peers_shared()` callers. Mutations through any handle are immediately visible to classification.
- **ChildInproc trust not synced at construction** — `CommsBootstrap::prepare` now uses `upsert_trusted_peer()` so parent is trusted at ingress immediately.
- **Lifecycle intent serialization** — `PeerAdded`/`PeerRetired` now serialize as `"mob.peer_added"`/`"mob.peer_retired"` via explicit `#[serde(rename)]`.
- **Host-mode hot loop spin** — legacy fallback falls through to `tokio::select!` when no work performed instead of unconditional continue that spins until budget exhaustion.
- **Legacy drain classification** — turn-boundary drain fallback now classifies by `InteractionContent` (peer lifecycle batching, response inline injection) instead of raw concat. Host-mode drain routes per-interaction with subscriber/tap/sink support.
- **Shared dispatcher consumed during bind** — factory now checks `Arc::strong_count` before `bind_wait_interrupt` and skips binding for shared dispatchers instead of consuming the caller's dispatcher.

### Changed

- **`meerkat-core` reads model defaults from catalog** — `ModelDefaults` no longer reads from `config_template.toml`; all model defaults come from `meerkat-models` catalog.
- **OpenAI adapter delegates detection to shared profiles** — capability detection logic moved from `meerkat-client` to `meerkat-models` profile rules.
- **Legacy delegated-agent validation uses catalog for fallback resolution** — the remaining `sub-agents` compatibility path in `meerkat-tools` now resolves fallback models via catalog instead of hardcoded strings.
- **Stale template defaults updated** — `gpt-4.1` → `gpt-5.2`, `gemini-1.5-pro` → `gemini-3-flash-preview`.
- **`SessionError::Unsupported` variant** — new error variant for capability negotiation across session service implementations.

## [0.4.9] - 2026-03-14

### Fixed

- **MCP tools invisible via RPC** — `start_turn_via_runtime()` (the V9 runtime path used by all RPC sessions) bypassed `apply_mcp_boundary()`, so MCP servers staged via `mcp/add` were never connected or made visible to agents. MCP tools were completely broken on the RPC surface since 0.4.7.
- **Wait tool not interruptible by peer comms** — `WaitTool` was created without interrupt support, so peer messages arriving during a `wait()` call queued silently until the wait completed. Added `WakeMode::InterruptYielding` policy for `peer_message`/`peer_request` while running, wired comms `inbox_notify` → `WaitTool` interrupt channel via factory bridge task. Agents now respond to peer messages during wait instead of blocking for up to the full requested duration.
- **Wait tool cap raised** from 300s to 1800s — sleep costs zero budget; the old 300s cap forced unnecessary LLM round-trips. Note: budget checks happen at loop boundaries, not during tool dispatch, so the cap balances responsiveness against overshoot risk in non-comms sessions.

### Changed

- **`WakeMode::InterruptYielding`** — new policy variant for peer inputs while running. Interrupts cooperative yielding points (e.g., wait tool) without cancelling active work or waking idle runtimes. Applied to `peer_message` and `peer_request` in `DefaultPolicyTable`.

## [0.4.8] - 2026-03-13

### Fixed

- **Cross-crate `include_str!` in facade** — `meerkat` crate referenced `../../meerkat-mob/skills/mob-communication/SKILL.md` which works in workspace builds but breaks when the crate is pulled from crates.io. Copied the skill file into the facade crate.
- **`meerkat-runtime` missing from crate publish order** — `meerkat-session` depends on `meerkat-runtime` but it wasn't in the CI publish sequence, causing registry publish failures.
- **Path-only dependencies break `cargo publish`** — 6 crates referenced `meerkat-runtime` via `path = "../meerkat-runtime"` without `workspace = true`, causing `cargo publish` to reject them. Fixed all to use workspace dependencies.
- **Release workflow publish decoupled from binary builds** — `publish_registries` job no longer depends on `build_binaries`, enabling manual dispatch to re-run publishing without rebuilding all 5 platforms.

## [0.4.7] - 2026-03-13

### Added

#### `meerkat-runtime` — Canonical Lifecycle Runtime (#140)
- New `meerkat-runtime` crate implementing the v9 Canonical Lifecycle and Execution Specification with a strict 3-layer model: Core (run primitives), Runtime (input lifecycle), and Surfaces (protocol adapters).
- 6 input type variants with `InputHeader`, `InputOrigin`, `PeerConvention`.
- `InputState` lifecycle: 9 states with validated transition table (`AppliedPendingConsumption` → `Queued` explicitly rejected).
- Policy engine: `DefaultPolicyTable` with `ApplyMode`, `WakeMode`, `QueueMode`, `ConsumePoint`, and `record_transcript`.
- `RuntimeState`: 7 states with strict transition table.
- Durability validation: derived forbidden for required input types.
- Coalescing/supersession: scope-based, cross-kind forbidden.
- `EphemeralRuntimeDriver` and `PersistentRuntimeDriver` (durable-before-ack via `RuntimeStore`).
- `InMemoryRuntimeStore` and `RedbRuntimeStore` backends.
- `SqliteRuntimeStore` — SQLite backend for runtime state persistence.
- `CommsInputBridge`: `InboxInteraction` → `PeerInput` conversion.
- `SessionServiceRuntimeExt` + `RuntimeSessionAdapter`: per-session driver registry.
- `MobRuntimeAdapter`: flow step delivery, member registration.
- Core lifecycle primitives in `meerkat-core/src/lifecycle/`: `RunPrimitive`, `RunEvent`, `RunBoundaryReceipt`, `RunControlCommand`, `CoreExecutor` trait, `RunId`/`InputId` newtypes.
- 222+ tests across the crate.

#### JSON-RPC Parity — 9 Wire-Feasible Gaps Closed (#138)
- **Deferred session creation**: `session/create` with `initial_turn: false` returns session ID without running a turn. `DeferredSession` class in Python and TypeScript SDKs.
- **Per-turn overrides**: `model`, `provider`, `max_tokens` overrides on `turn/start` for pending sessions.
- **Rich session responses**: `session/list` returns `WireSessionSummary` (timestamps, message counts, token totals) with pagination (`limit`/`offset`). `session/read` returns `WireSessionInfo` (timestamps, message counts, last assistant text).
- **Batch mob spawning**: `mob/spawn_many` with per-spec error reporting.
- **Scoped event streaming**: `scope_id`/`scope_path` preserved on `session/stream_event` notifications for delegated child-scope / flow-scope forwarding.
- **Persistent session recovery**: `turn/start` recovers persisted sessions after runtime restart, mirroring the REST recovery path. Archived sessions are rejected.
- **Callback tool protocol**: bidirectional JSON-RPC request/response for SDK-provided tool implementations. `ToolCallbackHandler` helpers in Python and TypeScript SDKs.
- 6 new RPC handler methods with legacy-mode guard.

#### Session History Across Surfaces (#142)
- Public session history (message listing) exposed on all surfaces: CLI `session history`, REST `GET /sessions/{id}/history`, RPC `session/history`, MCP `meerkat_session_history`.
- `SessionService` trait extended with history read capability.
- Wire types (`WireSessionHistory`, `WireSessionMessage`) added to `meerkat-contracts`.
- SDK codegen updated for Python and TypeScript.

#### SQLite Persistent Realm Backend (#143)
- `SqliteSessionStore` in `meerkat-store` — SQLite-backed session persistence for realm backends.
- SQLite is now the default persistent realm backend.
- Backend-owned persistence bundle pattern: realm backends own their persistence infrastructure rather than having it constructed externally.

### Changed

- **Persistence architecture** — persistence bundles are now opened from realm backends rather than assembled externally. Surfaces (CLI, REST, RPC) use the new bundle pattern for cleaner resource lifecycle.
- **Documentation** — refreshed persistence and session history docs across API reference, concepts, SDK guides, and architecture pages.

### Fixed

- **Host loop comms continuation** — idle host loop now triggers a continuation run when terminal comms responses (`Completed`/`Failed`) are delivered, instead of leaving agents unresponsive until the next external message. Scoped narrowly to terminal responses only; `Accepted` does not wake the host. Emits `RunStarted` before the continuation for correct stream lifecycle (#139).
- **Archived session recovery** — RPC and REST runtime recovery now rejects archived sessions instead of attempting to reconstruct them, preventing stale session resurrection.
- **Persistence helper panic** — avoided panic in persistence helper on unexpected backend state.
- **MCP tools schema test counts** — corrected test assertions after schema changes.
- **SQLite store clippy** — fixed clippy style issues in new sqlite stores.

## [0.4.6] - 2026-03-10

### Changed

- **Clean-cut comms/observability split** — removed mixed public interaction-stream APIs across Rust SDK, CLI, REST, RPC, MCP, WASM, and both Python/TypeScript SDKs. Public comms now exposes delivery (`inject`, `send_message`, `comms/send`) and explicit observation surfaces separately; interaction-scoped comms stream helpers remain runtime-internal only.
- **First-class mob and session parity across all SDKs** — Python SDK, TypeScript SDK, and Web SDK now expose explicit `Mob` and `Session` classes with typed mob lifecycle, member management, flow control, and event subscription methods. RPC surface adds dedicated `mob/*` methods (`mob/create`, `mob/list`, `mob/status`, `mob/members`, `mob/spawn`, `mob/retire`, `mob/respawn`, `mob/wire`, `mob/unwire`, `mob/lifecycle`, `mob/send`, `mob/events`, `mob/stream_open`, `mob/stream_close`, `mob/append_system_context`, `mob/flows`, `mob/flow_run`, `mob/flow_status`, `mob/flow_cancel`) as the canonical typed substrate for SDKs.
- **EventSubscription replaces CommsEventStream** — Python SDK exports `EventSubscription` instead of `CommsEventStream`/`CommsStreamEvent`. TypeScript SDK exports `EventSubscription<T>` (generic, async-iterable). Web SDK's `EventSubscription<T>` is now generic with a `parseEvents` callback.
- **Standalone session event streaming** — RPC adds `session/stream_open` and `session/stream_close` for explicit event stream lifecycle. Web SDK adds `Session.subscribe()` returning `EventSubscription<EventEnvelope>`.
- **Mob subscription methods are now async** — Web SDK `Mob.subscribe()` and `Mob.subscribeAll()` return `Promise<EventSubscription<T>>` instead of synchronous handles. `mob_member_subscribe` and `mob_subscribe_events` WASM exports are now async.
- **Mob events use numeric cursors** — `mob_events` WASM export takes a numeric `afterCursor` parameter instead of a string. Web SDK `Mob.events()` converts string cursors to numbers internally.
- **Mob create returns string directly** — `mob_create` WASM export returns a plain string mob ID instead of JSON. `mob_run_flow` similarly returns a plain string run ID.
- **Typed mob observation** — `Mob.subscribeAll()` returns `EventSubscription<AttributedEvent>` (source + profile + envelope) on Web SDK. `Mob.events()` returns `MobEvent[]` (cursor + timestamp + mob_id + kind). TypeScript RPC SDK uses `AttributedMobEvent` and `AgentEventEnvelope` types for mob/member subscriptions.
- **Web SDK types refined** — `SpawnResult` now has `status: 'ok' | 'error'` with optional `member_ref` and `error` fields. `MobMember` includes `member_ref`, `runtime_mode`, `state`, `wired_to`, and `labels`. `MobStatus` uses `state` field instead of `status` + `member_count`.

### Removed

- **`inject_and_subscribe`** — removed from WASM exports, Web SDK `Mob` class, and `MobWasmBindings` interface. Use `Mob.sendMessage()` + `Mob.subscribe()` separately.
- **`CommsEventStream` / `CommsStreamEvent`** — removed from Python SDK. Use `EventSubscription` instead.
- **`openCommsStream` / `sendAndStream`** — removed from TypeScript SDK `MeerkatClient` and `Session`. Use explicit mob/session observation subscriptions instead.

### Fixed

- **Stream termination and WASM subscription cleanup** — fixed event stream termination handling and subscription resource cleanup in the WASM runtime and RPC router.
- **Config runtime lock cleanup** — hardened lock cleanup in the config runtime to prevent leaked locks on error paths.
- **Mob-backed session routing** — hardened routing and control for mob-backed sessions, including proper session resolution for mob members.
- **MCP run handler without comms** — fixed MCP `meerkat_run` handler crash when the `comms` feature is not compiled in.
- **Pre-push unit hook stability** — serialized pre-push unit test runs across worktrees with a per-tree cache and one retry if `nextest` discovery hangs.

## [0.4.5] - 2026-03-07

### Fixed

- **Gemini tool schema validation** — `strip_gemini_function_parameters_unsupported_keywords` no longer strips property names from `properties` maps. A user-defined field named `"title"` was being removed as a JSON Schema keyword, causing Gemini to reject the schema with `required[1]: property is not defined`. The stripper now recurses into each property's schema individually without touching the properties map keys.
- **Example 033 sprite 404s** — sprite loader capped at 6 frames (00–05) instead of 8; eliminates 40 console 404s per page load.
- **API key leak in Vite bundle** — removed `define` block from example 033's `vite.config.ts` that baked raw `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `GEMINI_API_KEY` into the production bundle. Only `import.meta.env.VITE_*` (opt-in) pattern remains.
- **WASM not shipped in `@rkat/web`** — `build:wasm` script now removes wasm-pack's generated `.gitignore` (which contained `*`) so npm includes wasm artifacts in the published package.

### Added

#### Example 034: codemob-mcp — Implement Pack & User Mob CRUD
- **`implement` pack** — gated implementation with iterative review loop. Two comms-based agents (implementer + reviewer) iterate until the reviewer approves (max 3 rounds). Uses diverse models (claude-sonnet-4-6 + gpt-5.3-codex) for genuine review independence.
- **User mob CRUD tools** — `create_mob`, `get_mob`, `update_mob`, `delete_mob` MCP tools. User-created mobs stored as JSON under `.codemob-mcp/mobs/` and loaded dynamically into the pack registry without MCP restart. Both `comms` and `flow` execution modes supported.
- **Activity-based comms termination** — comms-based execution now tracks active agent turns via `RunStarted`/`RunCompleted` events instead of a fixed quiescence timeout. Agents can work for as long as needed (up to 1 hour hard cap); termination triggers when all agents are idle for a 30-second grace period.
- **Dynamic orchestrator routing** — comms initial message target and result capture now use the mob definition's orchestrator profile instead of hardcoded `"moderator"`.

## [0.4.4] - 2026-03-06

### Added

#### Session System-Context Control Plane (#121)
- `SessionServiceControlExt::append_system_context()` — inject runtime system context into a live session without rebuilding it.
- Staged appends are applied at the `CallingLlm` boundary just before the next model call.
- Idempotency enforced per session via `idempotency_key`. Duplicate keys return `Duplicate` status.
- Canonical system prompt remains the first `Message::System`; appended context follows as additional system messages.
- State carried through checkpoints, clones, forks, and persistence.
- Wired across all surfaces:
  - CLI: `session inject-context`
  - REST: `POST /sessions/{id}/system_context`
  - RPC: `session/inject_context`
  - WASM: `append_system_context(handle, request_json)`
  - Web SDK: `Session.appendSystemContext(options)`

#### Mob Member System-Context Control (#122)
- `mob_append_system_context(mob_id, meerkat_id, request_json)` WASM export — append system context to an individual mob member's session by resolving its live session through the mob roster.
- Web SDK: `Mob.appendSystemContext(meerkatId, options)` with `MobAppendSystemContextResult` type.
- Shared `resolve_mob_member_session_id()` helper deduplicates roster lookup logic with `mob_member_subscribe`.

#### Fire-and-Forget JS Tool Registration (#120)
- `register_js_tool(name, description, schema_json)` WASM export for tools that return `"acknowledged"` immediately without a JS callback.
- Agents get proper schema-validated tool calling; the host watches `ToolCallRequested` events in the stream and responds asynchronously via `mob_send_message`.
- Existing `register_tool_callback` (callback-based) unchanged — backwards compatible.
- Duplicate tool names are latest-wins across both registration modes.
- Web SDK: `MeerkatRuntime.registerFireAndForgetTool()` static method.

#### Structured Output Extraction Fix (#118)
- Structured-output extraction now unwraps provider-style named envelopes (e.g., `{"advisor": {...}}`) when the inner object matches the configured schema shape.
- `FlowStepSpec.output_format` option: `"json"` (default) or `"text"` to allow non-JSON agent outputs without parse failures.

#### Example 033: The Office — 10-Agent WASM Multi-Agent Demo (#123)
- Browser-based demo: 10 autonomous AI agents in a pixel-art office process events, coordinate via desk phone calls, store knowledge, and route actions through compliance.
- Demonstrates: mob orchestration in WASM, `autonomous_host` mode, inter-agent comms with visual phone arcs, fire-and-forget JS tools (`request_human_approval`, `upsert_record`, `revoke_access`, `restore_access`), system context injection for admin trust policy.
- Dieter Rams inspired UI: warm cream/copper chrome, tabbed log/records/graph panel, Cytoscape.js knowledge graph, floating approval panel, trust toggle (The Boss / Outsider).
- 6 pre-built scenarios: Client Escalation, Server Room Alert, Expense Report, Calendar Conflict, New Hire Onboarding, Security Breach.

#### Example 034: force-mcp — Multi-Agent Teams as MCP Tools (#119)
- Standalone MCP server exposing Meerkat mobs as `consult`, `deliberate`, and `list_packs` tools for Claude Code.
- 7 mobpacks: advisor, review, architect, brainstorm, red-team, panel (comms-driven), rct (full pipeline).
- MCP progress notifications for live progress bars during deliberation.

### Changed

- **Model names**: Updated model table — added `gpt-5.3-codex`, `gemini-3.1-pro-preview`, `gemini-3.1-flash-lite-preview`, `claude-sonnet-4-6`. Removed deprecated `o1-*`/`o3-*`/`o4-*` prefixes from provider inference (#117).
- **Demo server mode**: Diplomacy (031) and WebCM (032) demos support `?proxy=` query param for hosting behind `@rkat/web` proxy with server-side key injection (#116).
- **MCP readiness**: Aligned readiness waits with the real async connect budget; reduced adapter polling latency. Fixes flakiness under full-suite load (#121).

### Fixed

- Session context persistence: live persistent appends no longer mutate runtime state before durable save succeeds.
- Unknown session IDs no longer leak checkpointer gates.
- Pending-session promotion preserves injected context during first turn/start.
- Successful promotion no longer reports a false turn failure if replay staging has a post-create problem.
- Diplomacy demo map: filled Kosovo, North Macedonia, Albania gaps; Croatia/Slovenia render as Austria-Hungary; resolution mode cropping fix.
- Proxy CORS: `x-goog-api-key` added to allowed headers.

## [0.4.2] - 2026-03-04

### Added

#### `@rkat/web` npm Package
- New `sdks/web/` TypeScript wrapper around wasm_bindgen exports with idiomatic camelCase API.
- `MeerkatRuntime` class: `init()`, `initFromMobpack()`, `createMob()`, `createSession()`, version validation.
- `Mob` class: spawn, wire, retire, flows, event subscriptions.
- `Session` class: multi-turn agent loop, event polling.
- `EventSubscription` class: typed `poll()` / `close()` over WASM subscription handles.
- `registerTool()` static method for JS tool callback registration before init.
- TypeScript types aligned to exact Rust serde wire format (`AgentEvent`, `Profile`, `WiringRules`, `ToolConfig`).

#### Provider Proxy
- Node.js auth-injecting reverse proxy in `sdks/web/proxy/`.
- `npx @rkat/web proxy --port 3100` standalone CLI.
- Composable `createProxyHandler()` for integration into existing Node.js servers.
- Routes: `/anthropic/*`, `/openai/*`, `/gemini/*` with per-provider auth injection.
- CORS support, SSE streaming, `Accept-Encoding`/`Origin`/`Referer` header stripping.

#### Per-Provider Base URLs
- `anthropic_base_url`, `openai_base_url`, `gemini_base_url` on `Credentials`, `RuntimeConfig`, and `SessionConfig` in the WASM runtime.
- Backward-compatible: single `base_url` still works as fallback for the default model's provider.

#### MCP Server Loading
- `--wait-for-mcp` flag on `run`/`resume` blocks until all MCP servers finish connecting before first turn.
- Non-blocking parallel MCP server loading: servers connect in background, tools become available as each completes.
- Per-server `connect_timeout_secs` in `.rkat/mcp.toml` (default: 10s).
- `[MCP_PENDING]` system notice informs the LLM while servers are still connecting.

#### Mob Enhancements
- `SpawnMemberSpec.additional_instructions`: per-member system prompt additions, wired through `BuildAgentConfigParams` to `AgentBuildConfig`.
- `runtime_version()` wasm_bindgen export for JS/WASM version mismatch detection.

### Changed

- **Gemini auth**: use `x-goog-api-key` header instead of `?key=` query parameter in URL.
- **wasm32 clippy clean**: cfg-gated filesystem-only functions, removed dead imports, zero warnings with `-D warnings`.
- **CI**: added wasm32 clippy step to CI workflow.
- **Version parity**: web SDK added to `bump-sdk-versions.sh`, `verify-version-parity.sh` (6 files must now agree on version).
- **Release CI**: `@rkat/web` publish step added to release workflow, `meerkat-mob-pack` added to crate publish order.

### Fixed

- Backfill empty `Response.url` to prevent reqwest panic on wasm32 (#111).
- Proxy: strip `Accept-Encoding` from forwarded requests, `Content-Encoding`/`Transfer-Encoding` from responses (prevents `ERR_CONTENT_DECODING_FAILED`).
- Proxy: strip `Origin`/`Referer` headers to prevent Anthropic CORS rejection (#113).
- Gemini function schema lowering for type arrays and const values.
- Gemini `additionalProperties` normalization for nested schemas.
- Mob provider params propagation and Gemini reasoning deltas.
- Documentation accuracy audit: 20+ fixes across reference, guides, examples, and skills.

## [0.4.1] - 2026-02-28

### Added

#### WASM Browser Runtime
- `meerkat-web-runtime` crate with 25+ `wasm_bindgen` exports for browser deployment.
- 9 crates compile for `wasm32-unknown-unknown`: meerkat-store, meerkat-skills, meerkat-hooks, meerkat-comms, meerkat-tools, meerkat-session, meerkat (facade), meerkat-mob, meerkat-mob-mcp.
- Override-first resource injection: `AgentBuildConfig` accepts `tool_dispatcher_override`, `session_store_override`, `hook_engine_override`, `skill_engine_override` — bypasses filesystem resolution on wasm32.
- `AgentFactory::minimal()` filesystem-free factory constructor for browser environments.
- `CompositeDispatcher::new_wasm()` tool dispatcher without shell tools.
- `MobStorage::in_memory()` ephemeral mob storage for browser-hosted mobs.
- `FactoryAgentBuilder` default injection propagates wasm32-compatible resources to mob-spawned sessions automatically.
- Time compatibility layer (`meerkat_core::time_compat`) using `web-time` for `SystemTime`/`Instant` on wasm32.
- Anthropic client auto-adds `anthropic-dangerous-direct-browser-access` header on wasm32.

#### Tool Scoping and Runtime Tool Control
- `ToolScope` contract with `ToolFilter` enum (whitelist/blacklist) for per-turn tool visibility.
- Live MCP mutation: `mcp/add`, `mcp/remove`, `mcp/reload` RPC methods for runtime server provisioning.
- `tool_scope` field on `SessionBuildOptions` for session-wide tool visibility defaults.

#### Mob API Enhancements (Architecture Review)
- `Roster::session_id()` convenience accessor for direct session ID retrieval.
- `MobHandle::subscribe_agent_events()` per-member event subscription with point-in-time snapshots.
- `AttributedEvent` type for mob-level event sourcing with member attribution.
- Non-blocking `respawn` command (atomic retire + spawn).
- `SpawnPolicy` trait as extension point for auto-provisioning strategy on external turns.
- `MobEventRouter` independent async task merging per-member event streams.
- `inject_and_subscribe()` request-reply pattern for sync-like interactions over autonomous agents.

#### EventEnvelope Standardization
- Hard-cut canonical `EventEnvelope` contract: `{timestamp_ms, source_id, seq, event_id, payload}` across all surfaces.
- Strict `seq` ordering for replay and idempotency.
- Client-side malformed event guards with structured logging.

#### Schema Hardening
- Recursive `additionalProperties: false` injection at all nesting levels for Anthropic schema compliance.
- Handles arrays of objects, union types (`anyOf`), and deeply nested properties.
- Preserves user-provided `additionalProperties` settings.

#### Mobpack Archive Format
- `meerkat-mob-pack` crate: portable multi-agent deployment bundles.
- Pack/deploy/sign pipeline with Ed25519 signing, digest verification, and allowlist trust.
- WASM web build target support for browser deployment of mobpacks.

#### Documentation
- 10 new feature pages: sessions, tools, structured output, hooks, skills, memory, comms, mobs, mobpack, WASM.
- Examples gallery with 11 curated showcase applications.
- Universal surface tabs showing CLI, RPC, REST, MCP, Python, TypeScript, and Rust implementations.
- 20+ stale references fixed (version numbers, crate counts, CI descriptions, build matrix).

#### Examples
- Expanded to 31 polished examples (up from 27) covering all features and surfaces.
- All examples compile and exercise real features (validated in CI).
- WASM mini-diplomacy demo: 3-faction browser app with real mob orchestration.

#### CI/CD
- WASM compilation check job in CI workflow.
- `cargo fmt --all` auto-fix on pre-commit hook stage.
- `cargo build --workspace` added to pre-push hook stage.

### Changed
- CI parallelized to 8 jobs (~3.5 min): fmt-lint, test, test-minimal, test-feature-matrix-lib, test-feature-matrix-surface-checks, test-surface-smoke, audit, gate.
- Adopted `nextest` for faster parallel test execution across all CI jobs.
- Pedantic clippy with `-D warnings` enforced across full feature matrix.
- Toolchain updated to 1.93.1 with provenance tracking.
- Cross-surface parity: all 7 surfaces (CLI, RPC, REST, MCP, Python SDK, TypeScript SDK, Rust SDK) support EventEnvelope, tool scoping, live MCP controls, and mob feature-gating.
- Facade re-exports expanded: `ConfigStore`, `ConfigError`, `SessionServiceCommsExt`.
- CODEOWNERS file added for maintenance coverage.

### Fixed
- WASM32 runtime panics: `SystemTime`/`Instant` replaced with `web-time` shims, CORS headers for Anthropic, JSON schema validation restored.
- Mob spawn pipeline on WASM: `FactoryAgentBuilder` default injection ensures mob-spawned sessions inherit wasm32-compatible resources.
- Inproc comms enabled on wasm32 with override-first pattern.
- All 31 examples compile without warnings (non-exhaustive match, unused_mut, vec![] → array literals).
- Import ordering fixed for `cargo fmt` compliance across workspace.

## [0.4.0] - 2026-02-23

### Added

#### Mobs (Multi-Agent Orchestration)
- Introduced first-class mob runtime (`meerkat-mob`) for built-in multi-agent orchestration.
- Added DAG-based flow engine with conditions, branching, fan-out/fan-in, and dependency-aware step execution.
- Added full mob lifecycle operations with in-memory and redb-backed persistence.
- Added parallel spawn provisioning/finalization paths to support large swarm initialization.
- Added autonomous-host default runtime mode with supervision and escalation behavior.
- Added dedicated mob MCP surface (`meerkat-mob-mcp`) and integrated mob tools into CLI run/resume workflows.
- **Consolidated mob tool surface from 19 → 12 tools** with clear mob-level (`mob_*`) and member-level (`meerkat_*`) taxonomy.
- **Gated mob tools behind opt-in `--enable-mob` / `-M` flag** (default: disabled). Mob tools no longer pollute regular agent sessions.
- Mob enablement persists in session metadata for deterministic resume.

#### CLI UX
- **Stdin pipe support**: `cat file.txt | rkat run "Summarize"` reads piped input as context. Supports chaining: `cat data | rkat run "Extract" | rkat run "Analyze"`.
- **Live event streaming**: `tail -f app.log | rkat run --host --stdin "Monitor"` reads stdin line-by-line as events (infinite pipes).
- **Default `run` subcommand**: `rkat "hello"` is equivalent to `rkat run "hello"`.
- **Compact session references**: `rkat resume last`, `rkat resume ~2`, `rkat resume 019c8b99` (short prefix, git-style).
- **`continue` command**: `rkat continue "keep going"` (alias: `rkat c`) resumes the most recent session.
- Session output shows compact 8-char ID instead of full UUID.

#### MobHandle SDK Renames
- `external_turn` → `send_message`, `list_meerkats` → `list_members`, `get_meerkat` → `get_member`.
- `spawn_member_ref*` → `spawn` / `spawn_with_backend` / `spawn_with_options`.
- `spawn_many_member_refs` → `spawn_many`. `spawn()` now returns `MemberRef` (old `SessionId` compat path removed).

#### Skills v2.1
- Added strict source-pinned skill identity model (`SkillKey`) and structured skill refs.
- Enforced explicit model-mediated skill activation (`load_skill`) and removed legacy fuzzy fallback behavior.
- Brought CLI and SDK parity for skills v2.1 references and diagnostics.

#### Docs and Examples
- Added comprehensive examples library (27 examples) covering providers, hooks, memory, comms, and mob coordination.
- Added design philosophy reference and updated architecture summaries.
- Rewrote README for clearer platform/surface positioning.

### Changed
- Added config CLI CAS UX (`--expected-generation`) and generation-aware responses.
- Added configurable compaction settings to config surface and runtime wiring.
- Documented and tested config merge/override semantics across scalar/list/map fields.
- Added deferred initial-turn policy support for session creation, used by mob spawns.
- Added session-wide event stream API parity across services/surfaces.
- TypeScript SDK package renamed from `@meerkat/sdk` to `@rkat/sdk`.
- Release pipeline now publishes all 18 Rust crates (added `meerkat-mob`, `meerkat-mob-mcp`, `rkat`).

### Fixed
- Fixed OpenAI streaming duplicate/replay edge cases and strengthened error mapping.
- Fixed skills source fallback and identity resolution bugs across CLI/SDK surfaces.
- Fixed multiple mob lifecycle correctness issues (ordering races, shutdown/startup sequencing, duplicate wire side-effects).
- Stabilized host-mode and provider-agnostic integration tests.
- Addressed workspace clippy blockers and CI/push gate regressions.
- Fixed release tooling portability (`sed`-portable hook path) and lock-step contract/package version handling.

## [0.3.0] - 2026-02-14

### Added

#### Comms Command Plane Redesign
- Canonical `send` and `peers` tools replacing 4 legacy tools (`send`, `send_request`, `send_response`, `peers`)
  - `send`: unified command dispatch with flat `kind` discriminator for all comms operations
  - `peers`: list all visible peers
- `comms/send` and `comms/peers` RPC methods with flat-schema validation
- `POST /comms/send` and `GET /comms/peers` REST endpoints
- Python SDK methods: `send()` and `peers()` replacing `push_event()`
- TypeScript SDK methods: `send()`, `send_and_stream()`, and `peers()` replacing `pushEvent()`
- Optional peer authentication with fallback to in-process peer context
- `TrustedAndInproc` trust source for hybrid peer resolution

#### Interaction-Scoped Event Streaming
- `EventTap` mechanism for scoped event subscription per interaction
- `SubscribableInjector` extending `EventInjector` with `inject_with_subscription()` for dedicated interaction streams
- `InteractionSubscription`, `InteractionId`, `InteractionContent`, and `ResponseStatus` types in meerkat-core
- Host-mode interaction FSM with scoped event delivery
- Terminal completion events (`InteractionComplete`, `InteractionFailed`) for stream lifecycle management

#### CD Infrastructure
- Version parity verification: `make verify-version-parity` enforces Rust workspace, Python SDK, TypeScript SDK, and contract version alignment as a CI gate
- Schema freshness check: `make verify-schema-freshness` detects stale committed schema artifacts
- `cargo-release` configuration with pre-release hook that bumps SDK versions, regenerates schemas, and verifies parity
- Release scripts: `scripts/verify-version-parity.sh`, `scripts/bump-sdk-versions.sh`, `scripts/release-hook.sh`, `scripts/verify-schema-freshness.sh`
- `make regen-schemas` target for re-emitting schemas and running SDK codegen
- `make release-preflight` for full pre-release checklist (CI + schema freshness)
- `make publish-dry-run` for cargo publish readiness checks across all crates

### Changed

#### Versioning (Breaking)
- Package version and contract version are now lock-stepped (both `0.3.0`)
- Contract version bumped to `0.3.0` reflecting comms API changes
- All schema artifacts and SDK generated types regenerated for contract version `0.3.0`

#### Comms API (Breaking)
- Comms tools reduced from 4 to 2: `send` (with `kind` discriminator) and `peers`
- RPC: `event/push` removed, replaced by `comms/send`
- REST: `POST /sessions/{id}/event` deprecated in favor of `POST /comms/send`
- SDK: `push_event()`/`pushEvent()` removed; use `send()`/`peers()` instead

#### Host Mode
- Strict state transitions via `.transition()` instead of raw assignment
- Interaction processing classified into individual vs batched modes
- Host drain state reset on all exit paths

#### Dependencies
- Removed vendored `hnsw_rs` (was unmodified upstream v0.3.3); now resolved from crates.io
- `verify-version-parity` wired into `make ci` pipeline

### Fixed
- `ToolUse` args deserialization robust under Message buffering with custom deserializer
- Idempotent `stream_close` preventing duplicate close errors
- Comms stream completion cleanup preventing reservation leaks
- Comms self-input guard preventing agents from responding to their own messages
- E2E test model names updated to canonical providers (`gpt-5.2`, `gemini-3-pro-preview`)
- Clippy fix: `.or_insert_with(Vec::new)` → `.or_default()` in `SessionProjector`

### Removed
- 4 legacy comms tools (`send`, `send_request`, `send_response`, `peers`) -- now return "Unknown tool"
- `event/push` RPC method
- `push_event()`/`pushEvent()` SDK methods
- `vendor/hnsw_rs/` directory and `[patch.crates-io]` section

## [0.2.0] - 2026-02-12

### Added

#### Contracts and Capabilities
- `meerkat-contracts` crate: single source of truth for all wire-facing types, capability model, error contracts, and schema emission
- `CapabilityId` enum with distributed `inventory`-based registration across feature-gated crates
- `CapabilityStatus` (Available, DisabledByPolicy, NotCompiled, NotSupportedByProtocol) for runtime status
- `WireError` canonical error envelope with `ErrorCode` projections to JSON-RPC codes, HTTP status, and CLI exit codes
- `ContractVersion` with semver compatibility checking (currently 0.1.0)
- Composable request fragments: `CoreCreateParams`, `StructuredOutputParams`, `CommsParams`, `HookParams`, `SkillsParams`
- Wire response types: `WireUsage`, `WireRunResult`, `WireEvent`, `WireSessionInfo`, `WireSessionSummary`
- Feature-gated `JsonSchema` derives on all wire types
- `emit-schemas` binary for deterministic schema artifact generation (`artifacts/schemas/`)
- `capabilities/get` endpoint on all four surfaces (CLI, REST, MCP Server, JSON-RPC)

#### Skills System
- `meerkat-skills` crate with skill sources (filesystem, embedded, in-memory, composite), parser, resolver, renderer, and engine
- Core skill contracts in `meerkat-core/src/skills/`: `SkillId`, `SkillScope`, `SkillDescriptor`, `SkillDocument`, `SkillError`, `SkillSource` and `SkillEngine` traits
- 8 embedded skills: `task-workflow`, `shell-patterns`, `sub-agent-orchestration` (legacy name), `multi-agent-comms`, `mcp-server-setup`, `hook-authoring`, `memory-retrieval`, `session-management`
- Skill inventory section injected into system prompt via `extra_sections` slot
- Per-turn skill injection via `<skill>` tagged blocks prepended to user messages
- `SkillsResolved` and `SkillResolutionFailed` agent events
- Filesystem skill sources: `.rkat/skills/` (project) and `~/.rkat/skills/` (user)

#### Python and TypeScript SDKs
- SDK codegen pipeline (`tools/sdk-codegen/`) reading from `artifacts/schemas/`
- Python SDK (`sdks/python/`): async MeerkatClient with subprocess lifecycle, capability gating, version checks
- TypeScript SDK (`sdks/typescript/`): MeerkatClient with subprocess lifecycle, capability gating, version checks
- Generated types committed (Python: dataclasses, TypeScript: interfaces)
- SDK error types: `MeerkatError`, `CapabilityUnavailableError`, `SessionNotFoundError`, `SkillNotFoundError`
- Python conformance tests (8 type/error tests)

#### SDK Builder
- Builder tool (`tools/sdk-builder/build.py`): resolves features, builds runtime, emits schemas, runs codegen, emits bundle manifest
- Profile presets: `profiles/minimal.toml`, `profiles/standard.toml`, `profiles/full.toml`
- Bundle manifest with source commit, features, contract version, hashes, timestamp

#### Hooks System
- `meerkat-hooks` crate with `DefaultHookEngine`
- 3 hook runtimes: in-process (Rust handlers), command (stdin/stdout JSON), HTTP (remote endpoints)
- 8 hook points: `run_started`, `run_completed`, `run_failed`, `pre_llm_request`, `post_llm_response`, `pre_tool_execution`, `post_tool_execution`, `turn_boundary`
- Guardrail semantics: first deny short-circuits, deny always wins over allow
- Patch semantics: foreground patches applied in `(priority ASC, registration_index ASC)` order
- Background hooks with observe-only pre-hooks and `HookPatchEnvelope` post-hooks
- Failure policies: observe defaults to fail-open, guardrail/rewrite default to fail-closed
- Per-run hook overrides via `HookRunOverrides` (add entries, disable hooks)

#### Legacy Sub-Agent Surface (pre-0.5)
- `agent_spawn` and `agent_fork` tools for parallel delegated child work
- `agent_status`, `agent_cancel`, `agent_list` management tools
- `SubAgentManager` with concurrency limits, nesting depth control, and budget allocation
- `ContextStrategy` for spawn context: `FullHistory`, `LastTurns(n)`, `Summary`, `Custom`
- `ToolAccessPolicy`: `Inherit`, `AllowList`, `DenyList` for delegated child-agent tool filtering
- `ForkBudgetPolicy`: `EqualSplit`, `Proportional`, `Fixed` for budget allocation
- Model allowlists per provider for delegated child-agent spawns

#### Comms (Inter-Agent Communication)
- `meerkat-comms` crate with `Router`, `Inbox`, `InprocRegistry`
- 3 transport backends: Unix Domain Sockets (UDS), TCP, in-process
- `Keypair`/`PubKey`/`Signature` identity system with Ed25519
- `TrustedPeers` trust model with peer verification
- `Envelope` wire format with `MessageKind` variants: `Message`, `Request`, `Response`, `Ack`
- Comms tools: `comms_send`, `comms_request`, `comms_response`, `comms_peers`
- Host mode for long-running agents that process comms messages

#### Memory and Compaction
- `meerkat-memory` crate with `HnswMemoryStore` (hnsw_rs + redb)
- `SimpleMemoryStore` for testing
- `MemoryStore` trait in meerkat-core: `index`, `search`, similarity scoring
- `memory_search` builtin tool for agent access to semantic memory
- Memory indexing of compaction discards wired into agent loop
- `DefaultCompactor` in meerkat-session: auto-compact at token threshold, LLM summary, history rebuild
- `CompactionConfig` for threshold tuning

#### Structured Output
- `OutputSchema` type with `MeerkatSchema`, name, strict mode, compat, and format options
- Schema validation and retry logic for structured output
- `SchemaWarning` for compilation issues
- Provider-specific schema adaptation (Anthropic, OpenAI, Gemini)

#### Session Management
- `SessionService` trait in meerkat-core: create, turn, interrupt, read, list, archive
- `EphemeralSessionService` (in-memory) and `PersistentSessionService` (redb-backed)
- `RedbEventStore` append-only event log
- `SessionProjector` materializing `.rkat/sessions/` files from events
- `RedbSessionStore` for session persistence
- All four surfaces (CLI, REST, MCP Server, JSON-RPC) route through `SessionService`

#### JSON-RPC Server
- `meerkat-rpc` crate with JSON-RPC 2.0 over JSONL stdin/stdout
- `SessionRuntime`: stateful agent manager with dedicated tokio tasks per session
- Methods: `initialize`, `session/create`, `session/list`, `session/read`, `session/archive`, `turn/start`, `turn/interrupt`, `config/get`, `config/set`, `config/patch`
- `session/event` notifications with `AgentEvent` payload during turns

#### Builtin Tools
- Task management: `task_create`, `task_update`, `task_get`, `task_list`
- Shell execution: `shell` (Nushell backend), `shell_jobs`, `shell_job_status`, `shell_job_cancel`
- Utility: `wait`, `datetime`
- Three-tier tool policy: `ToolPolicyLayer` soft policies, `EnforcedToolPolicy` hard constraints, per-tool `default_enabled()`

#### MCP Server Capabilities
- `meerkat-mcp-server` crate exposing `meerkat_run` and `meerkat_resume` as MCP tools
- `McpRouterAdapter` relocated from CLI to `meerkat-mcp` for all surfaces

#### Build Profiles
- Profile presets for controlling feature composition: `profiles/minimal.toml`, `profiles/standard.toml`, `profiles/full.toml`
- Profiles drive SDK builder feature resolution and bundle manifests

#### E2E Tests
- 21-scenario E2E smoke test suite across 5 surfaces (CLI, REST, MCP Server, RPC, SDK)
- Integration-real tests for process spawning and live APIs
- Fast test suite gating for CI (unit + integration-fast, skipping doctests)
- Kitchen-sink compound RPC test replacing mock-only coverage

#### Prompt Assembly
- Unified `assemble_system_prompt` with documented precedence: per-request override > config file > config inline > default + AGENTS.md
- `extra_sections` slot for skill inventory injection
- Config fields `agent.system_prompt`, `agent.system_prompt_file`, `agent.tool_instructions` fully wired

### Changed
- Project renamed from "raik" to "Meerkat" with CLI binary `rkat`
- `AgentFactory::build_agent()` is now the centralized agent construction pipeline for all surfaces
- `FactoryAgentBuilder` bridges `AgentFactory` into `SessionAgentBuilder` trait
- All wire types consolidated into `meerkat-contracts` (removed per-surface duplicates)
- Error handling unified via `WireError` with protocol-specific projections
- Helper functions deduplicated: `resolve_host_mode()` to meerkat-comms, `resolve_store_path()` to meerkat-store, `spawn_event_forwarder()` to facade
- OpenAI and Gemini added to default CLI features
- Test infrastructure stabilized: fast test target isolation, real E2E gating, pre-commit hook fixes for bin-only crates

### Changed - Feature defaults
- `meerkat-tools`: comms, mcp, and the legacy `sub-agents` compatibility feature are now optional features (default: on)
  - `--no-default-features` builds tools with zero optional deps
  - Features: `comms`, `mcp`, `sub-agents`
- `meerkat` facade: comms, mcp, and the legacy `sub-agents` compatibility feature are now optional features (default: on)
  - Features: `comms`, `mcp`, `sub-agents`
- `meerkat-rpc`: comms, mcp, mob, and the legacy `sub-agents` compatibility feature are optional features (default: on)
  - `--no-default-features` builds the minimal server surface
  - Features: `comms`, `mcp`, `mob`, `sub-agents`
- `meerkat-rest`: comms is opt-in (default: on), no comms code when disabled
- `meerkat-mcp-server`: comms is opt-in (default: on), no comms code when disabled
- `meerkat-cli`: comms and mcp are opt-in (default: on), all inline code cfg-gated
- `agent_spawn` tool: `host_mode` field removed from schema when comms feature is off

### Fixed
- Anthropic streaming: emit `ToolCallComplete` on `content_block_stop`
- SDK E2E tests: session list uses `session_id` not `id`
- Python SDK async issues and TypeScript SDK brought to feature parity
- `active_skill_ids` now collects from all skill sources (not just embedded)
- SDK builder memory-store feature resolution
- SDK builder feature forwarding and dead `usage_instructions` removal
- `CapabilityStatus` parsing in SDKs and `contract_version` field inclusion
- RPC `session/create` expanded to full `AgentBuildConfig` parity
- Provider schema lowering moved from core to adapters, removing provider leakage
- `thought_signature` removed from generic `ToolCall`/`ToolResult` (provider-specific only)
- Config-driven delegated child-agent compatibility policy with fail-closed validation
- Legacy `sub-agents` compatibility, comms, and memory enabled through RPC/SDK surfaces

### Removed
- Dead files in meerkat-core: `comms_runtime.rs`, `comms_bootstrap.rs`, `comms_config.rs`, `agent/comms.rs`
- Duplicate `LlmClientAdapter`/`DynLlmClientAdapter` in meerkat-tools (uses canonical from meerkat-client)
- Per-surface wire type definitions (replaced by `meerkat-contracts`)
- Duplicated helper functions across surface crates

## [0.1.0] - 2026-01-15

Initial development release.

[Unreleased]: https://github.com/lukacf/meerkat/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/lukacf/meerkat/compare/v0.4.9...v0.5.0
[0.4.9]: https://github.com/lukacf/meerkat/compare/v0.4.8...v0.4.9
[0.4.8]: https://github.com/lukacf/meerkat/compare/v0.4.7...v0.4.8
[0.4.7]: https://github.com/lukacf/meerkat/compare/v0.4.6...v0.4.7
[0.4.6]: https://github.com/lukacf/meerkat/compare/v0.4.5...v0.4.6
[0.4.5]: https://github.com/lukacf/meerkat/compare/v0.4.4...v0.4.5
[0.4.4]: https://github.com/lukacf/meerkat/compare/v0.4.2...v0.4.4
[0.4.2]: https://github.com/lukacf/meerkat/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/lukacf/meerkat/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/lukacf/meerkat/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/lukacf/meerkat/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/lukacf/meerkat/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/lukacf/meerkat/releases/tag/v0.1.0
