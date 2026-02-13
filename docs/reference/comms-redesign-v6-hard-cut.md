# Comms Redesign V6 — Hard-Cut Plan

## Unified TDD Master Plan V6: Hard-Cut Comms Redesign (Final)

## Summary

This is the final, decision-complete plan for a full comms redesign with no external compatibility requirements.

Target public model across all surfaces:

- Command plane: `send(CommsCommand)`
- Stream plane: `stream(StreamScope)` (RPC transport via `stream_id`)
- Discovery plane: `peers()`

This plan explicitly hardens cutover rules, removes all legacy public names, and enforces a mandatory contract bump to `0.3.0`.

## Hard-Cut Policy (non-negotiable)

1. No legacy public surface remains at cutover:

- `event/push`
- `send_message`
- `send_request`
- `send_response`
- `list_peers`

2. No fallback aliases in any public layer:

- RPC handlers/method registry
- MCP tool registry/dispatch
- SDK methods
- REST endpoints
- docs/examples/skills

3. Internal-only staging scaffolding is allowed only if:

- same module/implementation boundary
- not exported from crate/module public API
- not documented
- deleted before merge to main in the same merge sequence

4. Contract bump is mandatory:

- `0.2.x -> 0.3.0`
- schema + generated SDK artifacts updated in lockstep

---

## Canonical Public API and Types

### Commands

```rust
pub struct PeerName(pub String);

pub enum InputSource {
    Tcp,
    Uds,
    Stdin,
    Webhook,
    Rpc,
}

pub enum InputStreamMode {
    None,
    ReserveInteraction,
}

pub enum CommsCommand {
    Input {
        session_id: SessionId,
        body: String,
        source: InputSource,
        stream: InputStreamMode,
        allow_self_session: bool, // default false
    },
    PeerMessage {
        to: PeerName,
        body: String,
    },
    PeerRequest {
        to: PeerName,
        intent: String,
        params: serde_json::Value,
        // Local stream reservation only (not remote execution stream)
        stream: InputStreamMode,
    },
    PeerResponse {
        to: PeerName,
        in_reply_to: InteractionId,
        status: ResponseStatus,
        result: serde_json::Value,
    },
}
```

### Receipts and stream

```rust
pub enum SendReceipt {
    InputAccepted {
        interaction_id: InteractionId,
        stream_reserved: bool,
    },
    PeerMessageSent {
        envelope_id: uuid::Uuid,
        acked: bool,
    },
    PeerRequestSent {
        envelope_id: uuid::Uuid,
        interaction_id: InteractionId,
        stream_reserved: bool,
    },
    PeerResponseSent {
        envelope_id: uuid::Uuid,
        in_reply_to: InteractionId,
    },
}

pub enum StreamScope {
    Session(SessionId),
    Interaction(InteractionId),
}

pub type EventStream = Pin<Box<dyn futures::Stream<Item = AgentEvent> + Send>>;

pub enum StreamError {
    NotReserved(InteractionId),
    NotFound(String),
    AlreadyAttached(InteractionId),
    Closed,
    PermissionDenied(String),
    Timeout(String),
    Internal(String),
}

pub enum SendAndStreamError {
    Send(SendError),
    StreamAttach {
        receipt: SendReceipt,
        error: StreamError,
    },
}
```

### Core seam ownership

```rust
#[async_trait::async_trait]
pub trait CommsRuntime: Send + Sync {
    async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError>;
    fn stream(&self, scope: StreamScope) -> Result<EventStream, StreamError>;
    async fn peers(&self) -> Vec<PeerDirectoryEntry>;
    async fn send_and_stream(
        &self,
        cmd: CommsCommand,
    ) -> Result<(SendReceipt, EventStream), SendAndStreamError>;

    async fn drain_inbox_interactions(&self) -> Vec<InboxInteraction>;
    fn take_interaction_stream_sender(
        &self,
        id: &InteractionId,
    ) -> Option<tokio::sync::mpsc::Sender<AgentEvent>>;
}
```

---

## Deterministic Stream Lifecycle (scope vs stream_id)

- `StreamScope` is core logical selector.
- RPC creates transport `stream_id` on `comms/stream_open`.
- `stream_id` is server-generated UUIDv7, unique, never reused.
- Ownership is bound to RPC connection/session.
- `comms/stream_event` carries `stream_id`, `scope`, `sequence`, `event`.
- `comms/stream_close(stream_id)` is idempotent and ownership-validated.
- Connection drop auto-closes all owned streams.

Response shape for close:

```json
{ "closed": true, "already_closed": false }
```

---

## Stream Reservation Semantics

State machine:

- Reserved
- Attached
- Completed
- Expired
- ClosedEarly

Rules:

- One active attachment per interaction ID.
- Duplicate attach returns `AlreadyAttached`.
- Reservation TTL default `30s`.
- Terminal event (`InteractionComplete` or `InteractionFailed`) transitions to `Completed` and cleanup.
- Early client close transitions to `ClosedEarly` and cleanup.
- Attach after `Completed`/`Expired`/`ClosedEarly` returns `NotReserved`.

Race handling:

- close vs terminal is exactly-once cleanup (CAS/atomic state transition).
- duplicate close is safe.
- timeout vs attach has deterministic winner; loser gets explicit error.

---

## send_and_stream Failure Mapping

Flow:

1. Validate command.
2. If reservable, create reservation first.
3. Bind stream handle.
4. Dispatch command.
5. Return `(receipt, stream)`.

Failure contract:

- If send fails before acceptance: `SendAndStreamError::Send`.
- If send succeeds but stream attach fails: `SendAndStreamError::StreamAttach { receipt, error }`.
- No rollback after accepted send.
- Immediate cleanup for failed attachment/reservation paths.

---

## Peer Truthfulness Contract

`peers()` uses the same canonical resolver as `send()`.

Resolver policy:

- per-kind route selection precedence: `trusted` first, `inproc` second
- `self` excluded
- `sendable_kinds` includes only kinds with viable selected route
- `selected_route_by_kind` exposes chosen source (`Trusted`, `Inproc`, `TrustedAndInproc`)

Truthfulness invariant:

- For any peer/kind listed in `sendable_kinds`, `send(kind)` must not fail due to unresolved recipient.
- Allowed failures are runtime-only (offline, timeout, IO, inbox full/closed).

---

## Validation and Error Policy

Dispatch-time validation (MCP/REST/RPC):

1. Must be object.
2. `kind` required and known.
3. Strict type checks.
4. Required/forbidden field matrix.
5. Unknown fields rejected.
6. Deterministic sorted error details.

No type coercion.

Canonical error payload:

```json
{
  "code": "invalid_command",
  "message": "Command validation failed",
  "details": [
    { "field": "status", "issue": "forbidden_field", "expected": "absent", "got": "string" }
  ]
}
```

---

## Surface Contracts

### Rust SDK

Add:

- `send`
- `stream`
- `send_and_stream`

Remove:

- public injector naming and APIs.

### MCP

Replace tools:

- remove `send_message`, `send_request`, `send_response`, `list_peers`
- add `send`, `peers`

### REST

Add:

- `POST /comms/send`
- `GET /comms/peers`
- `GET /comms/stream?...`

Remove:

- old ingress/event-push naming from public REST surface/docs.

### RPC

Add:

- `comms/send`
- `comms/peers`
- `comms/stream_open`
- `comms/stream_close`
- `comms/stream_event` notification

Remove:

- `event/push` handler and advertised method.

### Python SDK

Add:

- `send`
- `stream`
- `send_and_stream`

Remove:

- `push_event` and event/push path usage.

### TypeScript SDK

Add:

- `send`
- `stream`
- `sendAndStream`

Remove:

- `event/push` helper path usage.

---

## Internal Staging Scaffolding Rules

Temporary internal adapters allowed only during staged migration:

- legacy drain adapter
- legacy injector bridge
- legacy subscriber bridge

Constraints:

- module-private only
- no public exports
- no docs references
- no public tests asserting them
- deleted before merge-to-main cutover in same merge sequence

---

## TDD Milestones (final)

### M0 Inventory + policy gates

- Red: failing scan proves legacy names still present.
- Green: add CI gates and temporary internal allowlist mechanism.
- Refactor: baseline inventory report.

### M1 Canonical types + seam signatures

- Red: compile/tests fail for missing new types/signatures.
- Green: add types/errors/trait stubs.
- Refactor: naming normalization.

### M2 Additive seam + internal bridge

- Red: existing callers fail without bridge.
- Green: add new seam and internal-only adapters.
- Refactor: isolate bridge code with deletion markers.

### M3 Canonical resolver + truthfulness

- Red: mixed-source sendability tests fail.
- Green: shared resolver for `send` and `peers`.
- Refactor: route logging/error normalization.

### M4 Reservation FSM + concurrency

- Red: attach/close/timeout races fail.
- Green: implement deterministic state machine.
- Refactor: central cleanup path.

### M5 send_and_stream semantics

- Red: partial-failure mapping tests fail.
- Green: implement `SendAndStreamError` contract.
- Refactor: simplify reserve/send paths.

### M6 Rust SDK cutover

- Red: new API tests fail.
- Green: implement new SDK surface, remove public injector API.
- Refactor: helper ergonomics and defaults.

### M7 RPC 0.3.0 cutover

- Red: comms/* contract tests fail.
- Green: implement new methods + stream_id lifecycle.
- Refactor: remove `event/push` fully.

### M8 Contracts + codegen lockstep

- Red: generated types mismatch new wire schema.
- Green: update contracts and regenerate SDK artifacts.
- Refactor: reproducible generation manifest workflow.

### M9 Python + TS SDK cutover

- Red: wrappers/parsers fail against new RPC.
- Green: implement stream_id routing and new methods.
- Refactor: delete old event/push code paths.

### M10 MCP and built-in tools cutover

- Red: tool registry still exposes legacy names.
- Green: only `send` + `peers`.
- Refactor: usage instructions and tool docs cleanup.

### M11 REST cutover

- Red: new endpoint/SSE tests fail.
- Green: implement `/comms/*`.
- Refactor: remove legacy REST naming.

### M12 Legacy public surface eradication

- Refactor: remove internal scaffolding in same merge sequence.

### M13 Docs/skills parity + final hard-cut gate

- Red: stale-name/docs/skills matrix fails.
- Green: all docs/examples/skills updated.
- Refactor: final terminology pass.

---

## Mandatory CI Gates

- Contract version must equal `0.3.0`.
- RPC initialize method list must not include `event/push`.
- MCP tool list must be exactly `send`, `peers`.
- SDK exported APIs must not include legacy methods.
- Docs/skills stale-name gate must pass across:
  - `docs/**`
  - `.claude/skills/meerkat-platform/**`
  - SDK READMEs/examples/API refs

---

## Acceptance Criteria

- Zero legacy public surfaces/aliases remain.
- All surfaces expose only `send/stream/peers`.
- Stream lifecycle deterministic via `stream_id` and ownership.
- Peer truthfulness invariant holds for all advertised sendable_kinds.
- Internal scaffolding removed before merge-to-main cutover.
- Contract and generated SDK artifacts are synchronized on `0.3.0`.
- Full cross-surface test matrix green.

---

## Explicit Assumptions and Defaults

- Hard cut with no backward compatibility in public surface.
- Internal staging bridges are temporary and private only.
- RPC remains canonical wire for Python/TypeScript.
- REST stream transport remains SSE.
- MCP does not expose stream-control tools.
- `allow_self_session` default is false.
- Reservation TTL default is `30s`.
- `comms/stream_close` is idempotent.
