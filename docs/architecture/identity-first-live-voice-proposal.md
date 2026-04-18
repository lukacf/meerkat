# Realtime Channel Product Layer

Status: In implementation

## Summary

This document defines the final product contract for realtime interaction in
Meerkat.

The current `realtime_attachment` work is the substrate. It is necessary, but it is
not by itself a usable end-product live experience. The missing product is a
public `RealtimeChannel` layer on top of that substrate.

The final product shape is:

- one semantic system for sessions and mob members
- one additional delivery mode for continuous realtime channels
- modality-neutral chunk types for text, audio, and video
- a canonical public duplex transport hosted by `rkat-rpc` as a sibling
  WebSocket listener
- first-class Rust, Python, and TypeScript SDK clients
- a low-level CLI bridge, not a built-in voice or media application
- REST and MCP bootstrap/control surfaces only

The core architectural stance is:

- Meerkat owns session/member identity, channel lifecycle, interruption,
  replacement, canonical history, and tool authority
- Meerkat does not own microphones, speakers, cameras, VAD, AEC, or app UX
- provider live APIs are treated as session-managed ingress/egress substrates,
  not as the system model

The shipped product and advanced control-plane vocabulary is `realtime_*`.
The old `live_*` / `live_attachment_*` naming is historical and is not part of
the shipped surface for this feature.

## Implementation Context

This work lands before the machine DSL migration. The implementation must use
the current handwritten schema catalog, the current direct
`Arc<MeerkatMachine>` runtime integration pattern, and the current runtime-backed
surface architecture.

Current ownership already matters:

- `MeerkatMachine` is the session-scoped runtime authority
- `MobMachine` owns durable mob state and identity-first member orchestration
- `meerkat-client` owns provider transport mechanics
- surfaces are skins, not authorities

The current shipped tranche already provides:

- runtime-owned attachment truth through `runtime/realtime_attachment_status`
- mob-owned durable member intent through `mob/realtime_attach`,
  `mob/realtime_detach`, and `mob/member_status.realtime_attachment_status`
- OpenAI-specific provider orchestration in `meerkat-client`

This proposal does not discard that work. It defines the missing public product
layer and the required convergence work so the substrate becomes a coherent
realtime product.

## Goals / Non-goals

Goals:

- expose one public `RealtimeChannel` abstraction for realtime interaction
- keep text, audio, and video in one semantic system
- separate product semantics from provider transport peculiarities
- keep public surfaces transport-consistent across Rust, Python, TypeScript,
  RPC, REST, MCP, and CLI
- support provider-managed turning by default
- support explicit turn commit where providers/adapters can truthfully honor it
- keep bounded recovery and channel multiplexing explicit from day one
- preserve the existing runtime/mob substrate as the canonical owner of
  realtime lifecycle truth

Non-goals:

- shipping a built-in mic/speaker/camera application in this PR
- pushing duplex media through REST, JSON-RPC request/response, or MCP
- exposing raw provider session ids or transport handles as the public product
  noun
- letting surfaces invent provider-specific realtime semantics
- permanently keeping public `realtime_*` names on top of internal
  `realtime_attachment_*` names

## Final Vision

Meerkat has one interaction model.

Text, audio, and video are content modalities.
Realtime is a delivery mode with continuous ingress and incremental egress.

The final system is therefore:

- one semantic session/member/tool/history model
- one additional channel layer for continuous transport and realtime control

This means:

- text interaction continues to use the existing turn APIs directly
- realtime interaction uses a `RealtimeChannel`, but still commits into the
  same session/member/tool/history system

The core difference is not “voice” versus “text.”
The core difference is continuous channelized ingress.

## Product Boundary

### What Meerkat owns

- `RealtimeChannel` lifecycle
- session/member targeting
- canonical history commit points
- interruption
- replacement and reattach
- tool authority and tool lifecycle sequencing
- public status publication
- multiplexing rules
- reconnect policy

### What Meerkat does not own

- audio devices
- video devices
- speaker playback queues
- microphone capture
- camera capture
- VAD UX policy
- AEC / noise suppression
- push-to-talk UX
- waveform or app presentation

### Product consequence

The product is an API-first realtime channel system.

It is not:

- a built-in voice app
- a microphone client
- a speaker client
- a bundled media UX

Optional app-layer binaries may exist later, but they are not part of this
product contract.

## Canonical Facts And Owners

| Fact | Canonical owner | Notes |
| --- | --- | --- |
| Durable member-targeted realtime intent | `MobMachine` | Only for `MobMemberTarget` channels |
| Active session realtime binding | `MeerkatMachine` runtime authority | Ephemeral, session-scoped |
| Binding readiness / replacement / reattach truth | `MeerkatMachine` runtime authority | Surfaces read this, never re-derive it |
| Channel reconnect policy and retry overlay | channel host layer | Product-layer overlay on runtime truth |
| Provider transport handles and pumps | `meerkat-client` | Mechanical only, never semantic owner |
| Open-token issuance and channel registry | realtime RPC host | Transport/control host concern |
| Canonical transcript/history | existing session/runtime commit path | Partial transcript is not canonical history |
| Tool execution truth | ordinary Meerkat tool execution | Channel host sequences continuation only |

## Public Product Contract

### Public noun

The public realtime noun is `RealtimeChannel`.

Targets:

- `SessionTarget { session_id }`
- `MobMemberTarget { mob_id, agent_identity }`

### Public control/bootstrap methods

RPC, REST, and MCP expose bootstrap/control methods:

- `realtime/open_info`
- `realtime/status`
- `realtime/capabilities`

Rust, Python, and TypeScript SDKs expose a first-class client:

- `RealtimeChannel.session(...)` / `RealtimeChannel.mob_member(...)`
- `channel.open_request()`
- `channel.open_info()`
- `channel.status()`
- `channel.capabilities()`
- `channel.connect(...)`
- `connection.send_input(...)`
- `connection.commit_turn()`
- `connection.interrupt()`
- `connection.close()`
- `connection.next_frame()` / equivalent event receive helpers

Phase 4 ships the executable SDK websocket clients on top of the realtime
WebSocket host. The SDKs stay thin: they speak the canonical channel protocol
and do not invent surface-local channel semantics.

CLI exposes low-level infrastructure commands:

- `rkat realtime open-info`
- `rkat realtime status`
- `rkat realtime bridge`

The CLI bridge is a typed JSON-frame bridge over stdin/stdout. It is not a
device client. Phase 4 ships executable bridge transport on top of
`realtime/open_info` plus the canonical realtime WebSocket host.

### Product-layer types

The product layer defines:

- `RealtimeOpenRequest`
- `RealtimeOpenInfo`
- `RealtimeChannelStatus`
- `RealtimeCapabilities`
- `RealtimeReconnectPolicy`
- `RealtimeTurningMode`
- `RealtimeChannelRole`
- `RealtimeInputChunk`
- `RealtimeOutputChunk`
- `RealtimeEvent`

`RealtimeOpenInfo` includes:

- `ws_url`
- `open_token`
- `expires_at`
- `target`
- `supported_protocol_versions`
- `default_protocol_version`
- `capabilities`

`open_token` is:

- single-use
- bound to the target
- bound to the requested role
- 60-second TTL by default

`RealtimeCapabilities` includes at minimum:

- `input_kinds`
- `output_kinds`
- `turning_modes`
- `interrupt_supported`
- `transcript_supported`
- `tool_lifecycle_events_supported`
- `video_supported`

### Multiplexing

`RealtimeChannelRole` is explicit:

- `primary`
- `observer`

Rules:

- one `primary` channel may be open per target
- additional `primary` opens are rejected with `target_busy`
- multiple `observer` channels are allowed
- observers receive read-only status/output/transcript/tool lifecycle events
- observers cannot send input, interrupt, commit turns, or close the primary
  channel

## Transport Model

The canonical public duplex transport is a sibling WebSocket listener hosted by
`rkat-rpc`.

This is deliberate:

- the existing JSONL JSON-RPC transport is not a duplex media protocol
- REST is appropriate for bootstrap/control, not media streaming
- MCP is appropriate for bootstrap/control, not media streaming

The realtime WebSocket listener shares the same runtime host and realm/runtime
state as the ordinary RPC host. It is not a separate semantic authority.

## WebSocket Protocol

The canonical path is a realtime WebSocket endpoint in the `rkat-rpc` host.
`RealtimeOpenInfo.ws_url` points at that endpoint.

The first client frame is always `channel.open`.

`channel.open` includes:

- `protocol_version`
- `open_token`
- `role`
- `turning_mode`

Server response:

- `channel.opened`
- or `channel.error`, followed by close

Protocol versioning is mandatory from day one:

- clients must send `protocol_version`
- the server validates it against `supported_protocol_versions`
- unsupported versions return `unsupported_protocol_version` and close the
  socket

Canonical client frames:

- `channel.open`
- `channel.input`
- `channel.commit_turn`
- `channel.interrupt`
- `channel.close`

Canonical server frames:

- `channel.opened`
- `channel.status`
- `channel.event`
- `channel.error`
- `channel.closed`

`RealtimeInputChunk` is modality-neutral:

- `TextChunk`
- `AudioChunk`
- `VideoChunk`

`RealtimeOutputChunk` is modality-neutral:

- `TextDelta`
- `AudioChunk`
- `VideoChunk`

Media payloads are opaque typed payloads with codec/MIME metadata.
The protocol does not encode device semantics.

## Turning Modes

`RealtimeTurningMode` is explicit:

- `provider_managed`
- `explicit_commit`

Default:

- `provider_managed`

Rules:

- `provider_managed` is the default because provider live APIs currently own
  the ingress-turning behavior by default
- `explicit_commit` is part of the public contract from day one
- opening a channel with `explicit_commit` is rejected if the chosen
  provider/adapter cannot support it truthfully
- `channel.commit_turn` is legal only when `turning_mode = explicit_commit`

This makes provider-managed turning the default, not the only possible
realtime model.

## Transcript And History Semantics

`RealtimeEvent` includes at minimum:

- `InputTranscriptPartial`
- `InputTranscriptFinal`
- `TurnStarted`
- `TurnCommitted`
- `TurnCompleted`
- `OutputTextDelta`
- `OutputAudioChunk`
- `OutputVideoChunk`
- `Interrupted`
- `ToolCallRequested`
- `ToolCallCompleted`
- `ToolCallFailed`
- `StatusChanged`
- `NeedsReattach`

Transcript and history rules are explicit:

- `InputTranscriptPartial` is a channel observation only
- `InputTranscriptFinal` is still a channel observation until a turn boundary
- canonical session history updates only at a committed turn boundary
- in `provider_managed`, the provider/adapter determines the boundary
- in `explicit_commit`, the boundary occurs only after `channel.commit_turn`
- `Interrupted` is emitted when active assistant output is observably preempted;
  clients must not rely on it as the sole proof of successful barge-in
- if a channel closes or crashes before a turn boundary, uncommitted transcript
  is not written to canonical history
- `session/read` reflects committed turns only

Current implementation note:

- session-target text chunks are already wired through the channel host
- `provider_managed` text input commits immediately after the chunk is admitted
- `explicit_commit` text input stays transport-local until `channel.commit_turn`
- the websocket host now polls the underlying runtime/mob attachment status and
  emits `channel.status` plus `StatusChanged` when the visible product status
  changes
- audio/video chunk commit still depends on the provider-backed realtime
  adapter path and is not yet wired through the generic host

## Tool Lifecycle Semantics

Provider-originated tool calls always route through the ordinary Meerkat tool
authority.

Rules:

- the channel host emits `ToolCallRequested`
- ordinary Meerkat tool execution runs
- the channel host holds provider continuation open while tool execution runs
- completion emits `ToolCallCompleted` or `ToolCallFailed`
- clients observe tool lifecycle events as read-only status/events
- clients do not provide tool results directly in this contract

Client-supplied tool results are future work and must use the ordinary callback
and capability model, not ad hoc realtime channel frames.

## Recovery And Reattach

The existing runtime substrate already owns attachment truth and
`reattach_required`.

The product layer adds a bounded recovery overlay on top of that truth.

`RealtimeReconnectPolicy` defaults:

- `max_attempts = 3`
- `initial_backoff_ms = 500`
- `max_backoff_ms = 5000`
- `max_total_ms = 30000`

Backoff is exponential with jitter.

Product-layer rules:

- channel status surfaces `reconnecting`, `attempt_count`, and `next_retry_at`
- transient provider disconnects use bounded reconnect
- successful replacement/reattach returns the channel to `ready`
- once reconnect bounds are exhausted, the channel emits `error` and then
  `closed`

The product layer hides ordinary substrate-level recovery when it succeeds.
Only unrecoverable failure becomes terminal product-layer closure.

Current implementation note:

- primary session-target and member-target channels now own a bounded reconnect
  overlay in the websocket host
- when the substrate reports `reattach_required`, the host emits
  `reconnecting` with surfaced `attempt_count` and `next_retry_at`, emits
  `NeedsReattach`, and reissues a substrate attach after the configured backoff
- observers do not own retries; they only reflect the underlying target status
- once the retry budget is exhausted, the host emits `error` followed by
  `closed`
- provider-backed reconnect beyond the substrate reattach seam still lands in
  the provider integration phases

## Substrate Mapping

### Session-targeted channels

Session-targeted channels map onto the existing runtime substrate:

- runtime owns attachment truth
- channel host projects transport/control into that runtime truth
- channel close detaches only the session-level binding

### Member-targeted channels

Member-targeted channels map onto the existing mob substrate plus runtime
substrate:

- opening a `primary` member-targeted channel sets durable member-targeted
  realtime intent
- the current bridge session is attached through the runtime substrate
- close clears that durable channel-scoped intent and detaches
- respawn/repair/reprovision reuse the existing mob durability and recovery
  seams

The channel registry never becomes the durable owner of member-targeted intent.

## Surface Roles

### RPC host

Owns:

- bootstrap/control handlers
- sibling realtime WebSocket listener
- token issuance and validation
- channel registry and multiplexing rules

Does not own:

- runtime lifecycle semantics
- mob lifecycle semantics
- tool semantics

### REST

Owns:

- bootstrap/control endpoints returning typed realtime metadata

Does not own:

- duplex media
- channel semantics

### MCP

Owns:

- bootstrap/control tools returning typed realtime metadata

Does not own:

- duplex media
- channel semantics

### SDKs

Rust, Python, and TypeScript SDKs are first-class product clients.

They must share:

- the same protocol vocabulary
- the same event vocabulary
- the same turning-mode semantics
- the same reconnect semantics

### CLI

The CLI is a low-level bridge to the product layer.
It exposes typed frame transport, not an app-specific media UX.
The shipped bridge spawns `rkat-rpc` with realtime websocket hosting enabled,
requests `realtime/open_info`, and proxies canonical typed JSON channel frames
over stdin/stdout. It does not own media devices or app-layer capture/playback
policy.

## Provider Integration Plan

### Provider-neutral client seam

`meerkat-client` owns provider transport mechanics.

The provider-neutral realtime session seam must support:

- open provider session
- send input chunks
- optional explicit commit
- interrupt
- close
- output/event pump
- capability reporting

The shipped client seam lives in `meerkat-client::realtime_session`:

- `RealtimeSession`
- `RealtimeSessionFactory`
- `RealtimeSessionEvent`
- `RealtimeExternalSessionTarget`

The first implementation is the OpenAI adapter in `meerkat-client::openai_live`.
That adapter now reports honest OpenAI capabilities, exposes provider-neutral
`send_input` / `commit_turn` / `interrupt` / `close` operations, and maps raw
OpenAI server events into normalized realtime session events without moving any
semantic lifecycle truth into the client crate.

Provider mechanics remain in `meerkat-client`.
They do not move into `meerkat-runtime`, `meerkat-rpc`, or surface crates.

### OpenAI

OpenAI is in scope for this PR.

Requirements:

- use the vendored `vendor/oai-rt-rs` dependency for the authoritative snapshot
- provider-created realtime session path is the primary product path
- external `call_id` attach remains an advanced/internal seam, not the primary
  product flow
- when `rkat-rpc` is configured with the OpenAI realtime factory, session-target
  `realtime/capabilities` and `realtime/open_info` reflect the factory's
  capability set rather than the conservative substrate-only placeholder
- channel reconnect for a provider-created session reopens through the same
  provider factory `open_session(turning_mode)` path; it does not fall back to
  external attach

### Google

Google Live is not in scope for this PR.

However, the public product contract is deliberately modality-neutral and
provider-neutral so Gemini audio/video can implement the same `RealtimeChannel`
surface later without changing the product contract.

## Vocabulary Convergence

Shipped state:

- product-layer code uses `realtime_*`
- advanced control-plane surfaces use `realtime_*`
- substrate naming is converged anywhere it is load-bearing for the shipped
  feature
- the remaining formal `attachment_live` machine field is an implementation
  discriminant, not a public vocabulary term

The feature is only considered complete when a mixed `realtime`/`live`
product/control-plane vocabulary is no longer visible.

## Testing And Gates

Each code-bearing slice uses TDD:

- `Red`: add failing tests first
- `Green`: implement the minimum to pass
- `Refactor`: clean up and sync docs

Required verification categories:

- wire contract tests
- RPC/REST/MCP surface tests
- Rust/Python/TypeScript SDK tests
- CLI parser/help and bridge tests
- realtime WebSocket protocol tests
- runtime/mob integration tests
- provider integration tests
- public `e2e-smoke`
- internal `e2e-live`

Product gating rule:

- `e2e-smoke` must use only public product surfaces
- internal attach/seam tests belong in `e2e-live`

## Assumptions And Explicit Defaults

- canonical public product noun is `RealtimeChannel`
- canonical duplex transport is a sibling WebSocket listener inside
  `rkat-rpc`
- Rust, Python, and TypeScript SDKs are part of product completeness from day
  one
- REST and MCP are bootstrap/control only and never carry media
- default turning mode is `provider_managed`
- `explicit_commit` is part of the contract from day one but only available
  where providers/adapters can truthfully support it
- default reconnect policy is 3 attempts with exponential backoff and a 30s
  total bound
- one primary writer per target, multiple read-only observers
- no device stack and no `rkat-live` binary are part of this plan
- this document is the authoritative design document for the feature; no
  external document is authoritative for load-bearing behavior

## Completeness Checklist

- [x] product boundary is stated directly
- [x] substrate vs product distinction is stated directly
- [x] canonical public noun `RealtimeChannel` is stated directly
- [x] canonical duplex transport is stated directly
- [x] Rust, Python, and TypeScript SDK roles are stated directly
- [x] REST and MCP bootstrap/control role is stated directly
- [x] WebSocket protocol versioning is stated directly
- [x] turning modes and `explicit_commit` legality are stated directly
- [x] transcript commit timing is stated directly
- [x] tool lifecycle sequencing is stated directly
- [x] reconnect bounds and surfaced counters are stated directly
- [x] primary/observer multiplexing rules are stated directly
- [x] vocabulary convergence requirement is stated directly
- [x] OpenAI in-scope and Google out-of-scope status are stated directly
- [x] no authoritative external file reference remains for load-bearing behavior
