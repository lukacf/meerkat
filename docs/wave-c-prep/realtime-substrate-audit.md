# Realtime Substrate Audit (Wave C Prep)

Scope: end-to-end audit of Meerkat's realtime/voice substrate after wave (a)
demolition and in-flight wave (b) typed foundations. Emphasis on remaining
inconsistencies, mob-member rotation edge cases, cancellation safety, and a
concrete wave (c) realtime workstream proposal.

Reference: `docs/architecture/identity-first-live-voice-proposal.md`
(accepted design record; no caller-facing attach/detach RPC; status is the
canonical observation API; fenced by `RealtimeAttachmentSignalAuthority`).

---

## 1. Lifecycle trace (one session lifetime)

### 1.1 Session created with `ModelCapabilities.realtime = true`
- **Who owns state**: `MeerkatMachine` DSL authority (session-scoped entry).
  `entry.current_capability_surface` carries the resolved capabilities.
  See `meerkat-runtime/src/meerkat_machine/llm_reconfigure.rs:621`
  (`apply_capability_driven_realtime_transport`).
- **Who decides attach**: the runtime, not the caller — matching the
  accepted contract (`identity-first-live-voice-proposal.md` final contract
  bullet 3). Policy table is the `(surface.realtime, is_bound)` match at
  `llm_reconfigure.rs:671`.
- **Exposure**: `session/realtime_attachment_status[es]` RPC methods and the
  `meerkat_realtime_status` MCP tool (`meerkat-mob-mcp/src/public_mcp.rs:333`).

### 1.2 Provider WebSocket open + auth
- **Owner**: `RealtimeAttachmentToolDispatchHost` implementation. For OpenAI:
  `meerkat-openai/src/realtime_attachment.rs`. The attach path mints a
  `RealtimeAttachmentSignalAuthority` via
  `runtime.publish_realtime_attachment_signal(authority, BindingReady)`
  (`realtime_attachment.rs:107`).
- **Background pump**: `tokio::spawn(run_openai_live_event_loop(...))`
  (`realtime_attachment.rs:122`). Handle is stored in `tasks` map so
  `detach()` can abort it.
- **What can go wrong**: auth fails → returns `LlmError::AuthenticationFailed`
  mapped to `RuntimeDriverError::ValidationFailed` (map at
  `realtime_attachment.rs:151`). Binding is rolled back (`detach_live` called
  inline). OK.

### 1.3 Bootstrap via `rkat-rpc` realtime WS host
- **Owner**: `RealtimeWsHost` in `meerkat-rpc/src/realtime_ws.rs`.
- **Flow**: caller calls `realtime/open_info` (handler at
  `meerkat-rpc/src/handlers/realtime.rs:101`). Host mints a one-use open
  token with TTL 60s (`realtime_ws.rs:46` `DEFAULT_OPEN_TOKEN_TTL`) and
  realm scope capture (`PendingOpenEntry.realm_id`,
  `realtime_ws.rs:65`). Caller then opens WS to `/realtime/ws` (path at
  `realtime_ws.rs:45`) and sends `ChannelOpen`.
- **Capability gate**: `realtime/capabilities` and `realtime/open_info`
  both query live `RealtimeWsHost::session_factory_capabilities()` first
  (`handlers/realtime.rs:94,128`), only falling back to
  `conservative_phase_one_capabilities()` when the host is absent. Consistent
  with the contract.

### 1.4 Session membership wire (mob member)
- **Typed target**: `RealtimeChannelTarget::MobMember { mob_id, agent_identity }`.
- **Resolution seam**: `MobMcpState::resolve_realtime_target_session`
  (`meerkat-mob-mcp/src/lib.rs:1370`) reads from the MobMachine's canonical
  binding map via `mob_handle.current_realtime_binding(identity)`
  (`public_mcp.rs:320`).
- **WS binding lifecycle**: MobMember primaries subscribe to the mob's
  `MemberRealtimeBindingEvent` broadcast at bind time
  (`realtime_ws.rs:1794` select arm).

### 1.5 Caller-side exposure
- **RPC**: `realtime/capabilities`, `realtime/open_info`, `realtime/status`,
  the WS itself, and `session/realtime_attachment_status[es]`.
- **MCP**: only `meerkat_realtime_status` now (after wave-a deletion of
  `meerkat_realtime_capabilities` and `meerkat_realtime_open_info`, commit
  `927a8d2d2`). Remaining MCP surface reads the same runtime authority,
  so status is consistent between RPC and MCP.
- **CLI**: `rkat realtime` subcommands (`meerkat-cli/src/main.rs:481,1540`).
  Typed target enum has two variants — `Session { session_id }` and
  `Member { mob_id, agent_identity }` — and serializes via
  `realtime_target_wire` (callsite `main.rs:752`).
  The retired top-level `mob_member_target` discriminator (row #25) is
  absent from current HEAD; the typed enum matches
  `RealtimeChannelTarget`.
- **REST**: wave-a deleted both attach/detach endpoints (commit
  `488944b7d`) — REST no longer exposes realtime at all; surface is
  RPC/MCP/CLI/SDK only. Consistent with the proposal ("no caller-facing
  attach/detach RPC").
- **SDK** (`meerkat/src/realtime.rs`): `RealtimeChannel::mob_member` is
  `#[deprecated]` and panics at runtime (`realtime.rs:44-52`) — keeps old
  callsites loud while smoke-test migration catches up. See risk in §2.

### 1.6 Cancellation / reconnect
- **Network partition on provider socket**: provider event loop returns
  `Ok(None)` or `Err`. Both paths call
  `require_realtime_attachment_reattach_for_authority`
  (`realtime_attachment.rs:183-195`), which rotates the DSL authority into
  `ReattachRequired`.
- **WS socket observes `ReattachRequired`**: poll loop at
  `realtime_ws.rs:1996` catches
  `RealtimeBindingProjection::ReattachRequired` for Primary roles,
  enters `reconnect_overlay` with typed backoff+jitter, and emits status.
- **Mob member respawn**: MobMachine publishes
  `MemberRealtimeBindingEvent::Rotated`; WS picks it up at
  `realtime_ws.rs:1802`, calls `rotate_live_realtime_binding`
  (`realtime_ws.rs:3501`), registers the new bridge session, detaches the
  old, rebinds observers. Clean.
- **Local interrupt**: `ChannelInterrupt` client frame →
  `RealtimeProductSessionCommand::Interrupt` → provider session
  `.interrupt()` under the actor (`realtime_ws.rs:2925`).

### 1.7 Termination
- **Clean close**: `ChannelClose` from client triggers product-session
  `Close` command (breaks actor loop, `.close()`s session), then
  `cleanup_realtime_binding` (`realtime_ws.rs:2618`) which is primary-only
  and calls `detach_live(current_session_id)`. Observer channels never
  detach — correct (observers don't own the transport authority).
- **Server-initiated binding release**: `Released` event at
  `realtime_ws.rs:1912` emits typed `BindingReleased` error frame and
  closes the channel.

---

## 2. Remaining inconsistencies after waves (a)+(b)

### 2.1 Panicking deprecation of `RealtimeChannel::mob_member`
- **Evidence**: `meerkat/src/realtime.rs:44-52` —
  `#[deprecated]` + `panic!(...)` at runtime.
- **Correct typed behavior**: remove the method entirely. A deprecated
  method that panics is a runtime trap; the typed boundary is
  `RealtimeChannelTarget::MobMember`, and callers already have to go
  through `mob/member_status` to resolve the session. Leaving the panicking
  shim invites release-week surprises if any downstream user hits it.
- **Wave**: wave (c) — it's a public SDK surface cleanup, out of the
  typed-foundations (wave b) scope.

### 2.2 Dead `realtime_status_from_mob_status` projection
- **Evidence**: `meerkat-mob-mcp/src/public_mcp.rs:259` —
  `#[allow(dead_code)]` helper that parses mob-status *strings* into
  `RealtimeChannelStatus`. This is the retired path; the live path uses
  `realtime_status_from_runtime` (typed enum, `public_mcp.rs:227`).
- **Correct typed behavior**: delete. One projection path, typed end-to-end.
- **Wave**: wave (c) — sibling of wave-a MCP realtime tool deletions.

### 2.3 `RealtimeChannelStatus.attempt_count` is not a projection
- **Evidence**: `realtime_ws.rs:2640-2678`
  (`projection_to_channel_status`) hard-codes `attempt_count = 0` or `1`
  per discriminant. `public_mcp.rs:213-225` `channel_status()` takes it as
  an argument but the runtime projection callers always pass a constant.
- **Correct typed behavior**: `attempt_count` should come from the
  reconnect overlay (`reconnect_overlay.attempt_count()`), which *is*
  tracked on the realtime-WS side but never reaches the RPC/MCP status
  projection. As-shipped, `realtime/status` from outside the active socket
  always returns `1` during reconnect, hiding genuine retry-budget
  exhaustion signal.
- **Wave**: wave (c) — needs a typed `attempt_count` in the authority
  projection; would have been wave (b) if caught during B-6/B-9 but is
  not yet represented in the typed metadata contract.

### 2.4 Bespoke error classification in `realtime_client_error_frame`
- **Evidence**: `realtime_ws.rs:3858-3877`. Collapses
  `{InvalidRequest, AuthenticationFailed, ContentFiltered, ModelNotFound}`
  all into `RealtimeErrorCode::InvalidTarget`. Auth failure is
  semantically ≠ invalid target; content filtering is ≠ either.
- **Correct typed behavior**: the typed `ToolError` class landed in B-9
  V7 (`WireToolErrorClass`, `c423559d6`). The same typed discipline should
  apply to realtime provider errors — at minimum distinguish
  `AuthenticationFailed` → `RealtimeErrorCode::Unauthorized{Realm}`,
  `ContentFiltered` → a new `ContentFiltered` code, and keep
  `InvalidTarget` for true target mismatches.
- **Wave**: wave (c) — contract addition (new `RealtimeErrorCode`
  variants) plus classifier rewrite.

### 2.5 `preemptive_interrupt_can_be_ignored` string-matches the message
- **Evidence**: `realtime_ws.rs:3879-3882` — checks
  `error.code == InvalidTarget` AND
  `error.message.contains("realtime interrupt failed")`. This is
  message-shape reasoning, exactly the category the dogma pass is
  eliminating.
- **Correct typed behavior**: the product-session actor should return a
  typed `RealtimeActionResult` (e.g. `Idempotent`, `RejectedInvalid`,
  `Ok`) so the caller branches on structure, not prose.
- **Wave**: wave (c) — needs a typed action-result enum on
  `RealtimeProductSessionCommand::Interrupt`.

### 2.6 MCP `realtime_status_from_runtime` vs RPC
  `projection_to_channel_status` are two copies of the same truth
- **Evidence**: `public_mcp.rs:227` and `realtime_ws.rs:2640`. They
  agree today, but each surface owns its own copy of the mapping.
- **Correct typed behavior**: move the projection into
  `meerkat-contracts` (or `meerkat-runtime`) as a single typed mapping
  impl and have both surfaces delegate. One typed truth, one mapping.
- **Wave**: wave (c). Row #19 intent (channel-status semantics drift)
  will regrow from two copies drifting if left un-consolidated.

---

## 3. Mob-member realtime edge cases

### 3.1 Respawn while realtime-attached
- **Handled path**: the mob publishes `MemberRealtimeBindingEvent::Rotated`
  via broadcast (`realtime_ws.rs:1794-1910`). WS binding mutates
  `current_session_id` in place, calls
  `rotate_live_realtime_binding` which:
  1. Calls `runtime.ensure_runtime_session_for_rotation(&new)`
     (`realtime_ws.rs:1836`) — this is the s58 fix: pre-register the
     rotated bridge session with the runtime adapter so the next
     `realtime_attachment_status` poll doesn't hit `NotReady(Destroyed)`.
  2. Opens a new product-session bridge (reconstruction, not patching —
     see `refresh_product_session_projection:3404-3415`).
  3. `detach_live(old_session_id)` after the new bridge is open.
- **Risk**: if step 1 fails (adapter transiently unavailable) we log
  `warn!` and proceed (`realtime_ws.rs:1843-1850`). The next status poll
  on the new session may surface `InvalidTarget`. The comment calls this
  out but the failure path does not emit a typed status-degraded frame
  to the caller. Wave (c) candidate: make step-1 failure a typed
  `Reconnecting` with `reason: "adapter register pending"`.

### 3.2 Rotation of member runtime ID while attached
- **Identity vs runtime**: `AgentIdentity` is stable;
  `AgentRuntimeId`/`SessionId` rotates on respawn. The WS binding keys
  on `(mob_id, agent_identity)` and carries `current_session_id` as a
  mutable pointer (`realtime_ws.rs:1810`). Correct by design — identity
  is the durable seam per the accepted contract.
- **State at each layer during rotation**:
  - WS host: `current_session_id` mutates in-place under the socket's
    event-loop ownership (no shared state); single writer.
  - MCP state: `resolve_realtime_target_session` re-reads the mob
    binding map on every call, no caching (`public_mcp.rs:301-331`).
  - Runtime adapter: `attach_live(new)` then `detach_live(old)` is
    the fence — ordered so there is never a moment when neither
    session has the authority.

### 3.3 Broadcast lag
- **Handled**: `binding_event_future` at `realtime_ws.rs:2342` returns
  `Err(())` on `RecvError::Lagged`, the consumer at `realtime_ws.rs:1965`
  logs `warn!` and relies on the 10ms poll loop
  (`RECONNECT_POLL_INTERVAL`, `realtime_ws.rs:47`) re-reading the
  authority. Comment at `realtime_ws.rs:1966-1974` documents this.
  Row #20 ("WS rotation can pin to retired session on lagged binding
  events") is therefore **closed by the poll-driven reread** — as long as
  `current_session_id` has been updated by an earlier non-lagged event
  or re-resolves from the mob binding map. **Gap**: the poll loop
  currently re-reads *attachment status on current_session_id*, not the
  *binding map*. If the first rotation event is lost (lag on a fresh
  subscriber before first tick), the socket stays pinned to the old
  session_id indefinitely. Wave (c) fix: on `Lagged`, re-resolve via
  `mob_handle.current_realtime_binding(identity)` and compare to
  `current_session_id`.

### 3.4 Provider-side eviction while machine thinks session is live
- **Detection**: provider event loop `next_event()` returns `Ok(None)` or
  `Err(...)` (`realtime_attachment.rs:182-197`). Both paths stage
  `ReattachRequired` in the DSL via
  `require_realtime_attachment_reattach_for_authority`.
- **Retry policy**: the WS-side `reconnect_overlay` owns backoff+jitter
  (splitmix64 RNG, `realtime_ws.rs:155-189`). Provider side does not
  retry — it surrenders the socket and leaves reattach decisions to the
  DSL, which is correct (DSL is the authority per dogma).
- **Backoff**: per-channel full-jitter, bounded by reconnect policy on
  the `RealtimeOpenRequest`. Status frames emit `next_retry_at` /
  `deadline_at` — but see §2.3, `attempt_count` is not accurately
  projected to non-WS status queries.

---

## 4. Cancellation audit (`tokio::spawn` / `tokio::select!`)

### 4.1 `run_product_session_actor` (`realtime_ws.rs:2885`)
- Spawned at `realtime_ws.rs:2600` and `realtime_ws.rs:3422` (rotation).
- `tokio::select!` over `command_rx.recv()` and `session.next_event()`.
- **Drop semantics**: command channel dropped → `recv()` returns `None`
  → `session.close().await` + `break` (line 2894). Clean.
- **Event-pump error**: sends `Update::Error { retryable }` and breaks
  (line 2991). Clean.
- **Leak risk**: if both ends drop mid-`session.close().await`, the
  `.await` happens on the task's stack before `break`; session drop
  then runs. Unless `close()` itself hangs, no leak. Provider
  `close()` implementations need to have their own cancel-safety
  guarantees; OpenAI impl uses tungstenite `close_frame`, generally
  bounded. **Potential risk**: `session.close()` has no timeout. Wave
  (c): wrap in `tokio::time::timeout` so a stuck provider close can't
  pin the actor task indefinitely.

### 4.2 Socket event loop select (`realtime_ws.rs:808`)
- Arms: `socket.recv()`, `update_rx.recv()`, observer frame future,
  `binding_event_future(...)`, `poll_interval.tick()`,
  `wake_rx.recv()`.
- **Cancellation**: axum owns the upgraded socket; when the WS task
  terminates (break from the loop), the `binding` drop path runs
  `cleanup_realtime_binding` (primary-only detach). Observer guards
  (`_wake_observer_guard`, `_bridge_observer_guard`) drop-uninstall on
  scope exit — the DSL authority loses those observers cleanly.
- **Drop-mid-send**: all socket sends use `.await`; cancellation at the
  outer task level is possible only on client disconnect, which axum
  already accounts for. No `tokio::spawn` of child tasks inside the
  loop that would outlive it — good.

### 4.3 `run_openai_live_event_loop` (`realtime_attachment.rs:163`)
- Spawned from `attach()`. Handle stored in `tasks` map.
- **Cancellation via `detach()`**: `handle.abort()` at
  `realtime_attachment.rs:145`. **Risk**: `abort()` is cooperative on
  `.await` points; the task may still be inside `handle_openai_live_event`
  when aborted. Since the loop already calls
  `require_realtime_attachment_reattach_for_authority` on exit
  *inside* the loop body (lines 183,192), an abort on `next_event().await`
  skips that reattach-required signal — the DSL authority is then left
  in whatever state it was before abort.
- **Mitigation today**: `detach()` calls `runtime.detach_live(...)`
  *after* `abort()` (line 147), which resets the binding explicitly.
  This is equivalent for the detach case. **Gap**: if someone calls
  only `tasks.lock().await.remove(...).abort()` without the subsequent
  `detach_live`, the DSL is orphaned. Wave (c): ensure there is no such
  call site. One grep check needed.
- **Leak risk**: the `tasks` map entry is removed by the task body
  itself on natural exit (`realtime_attachment.rs:131-133`). On
  `handle.abort()` the body doesn't run to completion; the map is
  cleaned up by `detach()` (line 143). Good, but only if `detach()` is
  the sole abort caller.

### 4.4 Observer callbacks under sync locks
- `BridgeProjectionToProductTurn` (`realtime_ws.rs:3565`) and
  `RealtimeSocketFreshnessWake` (`realtime_ws.rs:3591`) fire under the
  DSL authority mutex; the bridge re-enters after release, the wake uses
  `try_send`. Both documented, no deadlock/leak visible.

---

## 5. Recommended wave-c realtime workstream

**Theme**: typed projections, one truth, no string-shape reasoning, no
runtime panics on deprecated public API.

| # | Subtask | Files (allowlist) | Deliverable | Deps |
|---|---------|-------------------|-------------|------|
| C-R1 | Delete panicking `RealtimeChannel::mob_member` and the deprecation stub | `meerkat/src/realtime.rs` | SDK compiles cleanly; callers must use `RealtimeChannel::session` with pre-resolved id | none |
| C-R2 | Delete `realtime_status_from_mob_status` dead-code projection | `meerkat-mob-mcp/src/public_mcp.rs` | One typed projection from `RealtimeAttachmentStatus` only | none |
| C-R3 | Move `projection_to_channel_status` / `realtime_status_from_runtime` to a single canonical impl | `meerkat-runtime/src/meerkat_machine_types.rs` (or new `meerkat-contracts` helper), `meerkat-rpc/src/realtime_ws.rs`, `meerkat-mob-mcp/src/public_mcp.rs` | `impl From<RealtimeAttachmentStatus> for RealtimeChannelStatus` owned in one place; RPC+MCP delegate | C-R2 |
| C-R4 | Typed `attempt_count` / `next_retry_at` in `RealtimeAttachmentStatus` | `meerkat-runtime/src/meerkat_machine_types.rs`, `meerkat-runtime/src/meerkat_machine/dispatch_control.rs`, `meerkat-rpc/src/realtime_ws.rs` (reconnect overlay → DSL input) | Status queries see real retry state, not hard-coded `0/1` | C-R3 |
| C-R5 | Typed `RealtimeActionResult` for interrupt/close on `RealtimeProductSessionCommand` — kill `preemptive_interrupt_can_be_ignored` message match | `meerkat-contracts/src/wire/realtime.rs`, `meerkat-rpc/src/realtime_ws.rs`, `meerkat-client/src/realtime_session.rs` | No `error.message.contains(...)` in realtime path | none |
| C-R6 | Typed realtime error classification — expand `RealtimeErrorCode` and rewrite `realtime_client_error_frame` | `meerkat-contracts/src/wire/realtime.rs`, `meerkat-rpc/src/realtime_ws.rs` | Auth/ContentFiltered/ModelNotFound each get a typed code | schema regen |
| C-R7 | `session.close()` timeout in product-session actor | `meerkat-rpc/src/realtime_ws.rs:2885` | Stuck provider close cannot pin the actor task | none |
| C-R8 | Rebinding map re-resolve on `Lagged` | `meerkat-rpc/src/realtime_ws.rs:1965` | After a lost rotation event the socket converges on canonical binding, not the retired session_id | none |
| C-R9 | Invariant check: every `handle.abort()` on realtime task is paired with `detach_live` | `meerkat-openai/src/realtime_attachment.rs` + grep audit under `rg "\.abort\(\)" meerkat-openai/ meerkat-rpc/` | No DSL-orphaning cancel path | none |
| C-R10 | Schema regen + version parity | `artifacts/schemas/`, SDK generated types | `make regen-schemas && make verify-version-parity` pass | C-R4, C-R5, C-R6 |

**Dependencies summary**: C-R3 is the keystone — once the projection has
one home, the rest either extend the typed shape (C-R4) or replace
string/message inspection with typed enums (C-R5, C-R6). C-R1, C-R2,
C-R7, C-R8, C-R9 are independent small edits.

**Scope boundary**: do **not** re-land MCP `realtime_capabilities` /
`realtime_open_info` tools. Wave (a) correctly deleted them; callers
drive realtime via RPC and session status. MCP exposure is intentionally
read-only through `meerkat_realtime_status`.

**Tests to extend (not new files, not listed above)**:
`meerkat-rpc/tests/` realtime WS rotation tests — ensure a synthetic
broadcast `Lagged` followed by an out-of-band binding change triggers
a reread (C-R8 behavioral test) rather than silent pinning.

---

## Closing assessment

The substrate is substantially healthier than at 0.5: the attachment
authority is typed, mob-member rotation has a clean identity/runtime
split, and the accepted contract is materialized (no attach/detach RPC,
status-driven observation). Remaining wave-c work is narrow typed
projection plus two genuine bug risks (§3.3 Lagged re-resolve, §4.1
stuck provider close). None of these block 0.6 on the happy path, but
§2.3/§2.5/§2.6 are exactly the shape of drift row #19 captured and will
regrow if left un-consolidated.
