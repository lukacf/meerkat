# MeerkatMachine State Scope + Session Identity Audit

> **Current status (0.6.5):** Historical wave-c audit input. The state-scope
> classifications remain useful background, but the pre-0.6.5 realtime
> attachment fields described here were superseded at the public boundary by the
> live-adapter MVP and caller-initiated `live/*` channels.

Wave-C preparation. This audit enumerates every field in the MeerkatMachine
DSL state schema, classifies each by actual lifetime scope, and then
formalizes the session identity contract that MeerkatMachine presupposes.

Sources of truth consulted:

- DSL: `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs` (3059 lines)
- Generated authority (mirrored): `meerkat-runtime/src/meerkat_machine/dsl.rs`
- Runtime shell: `meerkat-runtime/src/meerkat_machine/mod.rs`
- Session core: `meerkat-core/src/session.rs`, `meerkat-core/src/session_store.rs`
- Event log: `meerkat-session/src/event_store.rs`
- Comms trust: `meerkat-comms/src/trust.rs`
- MCP router: `meerkat-mcp/src/router.rs`
- Ops lifecycle: `meerkat-runtime/src/ops_lifecycle.rs`

MeerkatMachine is introduced as the "session-scoped execution kernel for
the Meerkat runtime" — see
`meerkat-runtime/src/meerkat_machine/mod.rs:401-407`. The top-level shell
is a `HashMap<SessionId, RuntimeSessionEntry>` (same file, lines 406-427),
so the shell level is cleanly session-keyed. The DSL state schema is per
session (each `RuntimeSessionEntry` owns its own
`Arc<Mutex<MeerkatMachineAuthority>>` at line 158). This audit is about
whether every field inside that per-session authority actually has a
session-sized lifetime.

## Section 1: State Field Enumeration + Scope Classification

All fields are taken from the `state { ... }` block of the DSL at
`meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:9-180`.

### 1.1 Session identity + lifecycle scaffolding

| Field (line) | Type | Classification |
|---|---|---|
| `lifecycle_phase` (10) | `MeerkatPhase` | session-scoped |
| `session_id` (11) | `Option<SessionId>` | session-scoped (identity) |
| `active_runtime_id` (12) | `Option<AgentRuntimeId>` | session-scoped |
| `active_fence_token` (13) | `Option<FenceToken>` | session-scoped |
| `current_run_id` (14) | `Option<RunId>` | session-scoped (per-turn, lives inside session) |
| `pre_run_phase` (15) | `Option<String>` | session-scoped (pre-run bookkeeping) |
| `silent_intent_overrides` (16) | `Set<String>` | session-scoped |

All seven are unambiguous: they describe "this session" and die with it.
`session_id` is stored as `Option` because the `Initializing` phase
precedes registration; once set (via `RegisterSession`) it is the
identity fact.

### 1.2 Realtime attachment authority (lines 17-24, 76, 87-88, 96)

| Field | Type | Classification |
|---|---|---|
| `realtime_intent_present` | `bool` | session-scoped |
| `realtime_binding_state` | `Enum<RealtimeBindingState>` | session-scoped |
| `realtime_binding_authority_epoch` | `Option<u64>` | session-scoped |
| `realtime_reattach_required` | `bool` | session-scoped |
| `realtime_next_authority_epoch` | `u64` | session-scoped (monotonic within session) |
| `realtime_product_turn_phase` | `Enum<RealtimeProductTurnPhase>` | session-scoped (per-turn, lives in session) |
| `realtime_projection_freshness` | `Enum<RealtimeProjectionFreshness>` | session-scoped |
| `realtime_projection_frontier_ms` | `u64` | session-scoped (watermark relative to this session's ingress) |
| `realtime_reconnect_policy` | `Enum<RealtimeReconnectPolicy>` | session-scoped |

All realtime state is per provider-session binding. Realtime binding
lifetime is strictly bounded by the product session; detach/reattach
cycles are recorded via authority-epoch monotony, not by crossing
session boundaries. Clean classification.

A potentially confusing sub-case: the provider's WebSocket connection
(`meerkat-rpc/src/realtime_ws.rs`) lives in the shell, not in the DSL.
The DSL only owns the *authority* of "which binding-epoch is canonical";
the transport itself is shell-owned mechanics. That's the right split
and is already correctly projected — see U-C / dogma-round-2 notes in
the DSL comments at lines 78-96.

### 1.3 Live-topology reconfigure (line 27)

| Field | Type | Classification |
|---|---|---|
| `live_topology_phase` | `Enum<LiveTopologyPhase>` | session-scoped |

Gates realtime publishes during an LLM-identity swap of *this* session.
Per-session.

### 1.4 MCP server connection states (line 32)

| Field | Type | Classification |
|---|---|---|
| `mcp_server_states` | `Map<McpServerId, Enum<McpServerState>>` | **projection of wider authority** |

The MCP *configuration* (which servers exist, what transport, what args)
is user-scoped / project-scoped: it lives in `.rkat/mcp.toml` per project
or `~/.rkat/mcp.toml` per user. The *connection instance* the runtime
owns, however, is per-session. See:

- `meerkat-mcp/src/router.rs:886-912` — each `McpRouter` owns its own
  `servers: HashMap<String, ServerEntry>` and its own `pending_*`
  infrastructure.
- `meerkat/src/factory.rs:2092-2120` — every `build_agent()` call
  assembles a fresh dispatcher stack including a router adapter and
  binds an MCP lifecycle handle that mirrors into *this session's*
  `mcp_server_states`.

So `mcp_server_states` is the DSL projection of the per-session router's
handshake state. Two sessions speaking to the same logical MCP server run
independent connections and therefore independent
`mcp_server_states` entries. Classification stands as "projection of
wider authority" only in the sense that the *configuration* is wider
than the session; the *connection* is session-local and the field is
correctly scoped. No scope mismatch — but see §3 for the subtle
implication about shared server reconnect coordination.

### 1.5 Peer interaction lifecycle (lines 41-43)

| Field | Type | Classification |
|---|---|---|
| `pending_peer_requests` | `Map<PeerCorrelationId, Enum<OutboundPeerRequestState>>` | session-scoped |
| `inbound_peer_requests` | `Map<PeerCorrelationId, Enum<InboundPeerRequestState>>` | session-scoped |

`PeerCorrelationId` is generated per outbound/inbound correlation; these
maps are cleared on the session's terminal phase via
`PeerInteractionCleanup`. Session-scoped.

### 1.6 Session-context advancement (line 53)

| Field | Type | Classification |
|---|---|---|
| `last_session_context_updated_at_ms` | `u64` | session-scoped (watermark of *this* session's mutations) |

### 1.7 Interaction stream registry projection (lines 66-67)

| Field | Type | Classification |
|---|---|---|
| `reserved_interaction_streams` | `Set<PeerCorrelationId>` | session-scoped |
| `attached_interaction_streams` | `Set<PeerCorrelationId>` | session-scoped |

### 1.8 Peer-ingress transport capability ownership (lines 110-112)

| Field | Type | Classification |
|---|---|---|
| `peer_ingress_owner_kind` | `Enum<PeerIngressOwnerKind>` | session-scoped, but see note |
| `peer_ingress_comms_runtime_id` | `Option<CommsRuntimeId>` | session-scoped |
| `peer_ingress_mob_id` | `Option<MobId>` | **weak cross-machine projection** |

`peer_ingress_mob_id` references a `MobId` whose canonical authority is
in `MobMachine`. MeerkatMachine holds a *reference* to "this session is
ingress-owned by mob X"; the mob's existence is not gated by this
reference. If MobMachine destroys the mob, MeerkatMachine currently has
no structural invariant that this id will be cleared. Whether this is
a scope mismatch or a legitimate foreign-key-with-orphan-tolerance is
discussed in §3.

### 1.9 Supervisor-bridge authorization (lines 130-134)

| Field | Type | Classification |
|---|---|---|
| `supervisor_binding_kind` | `Enum<SupervisorBindingKind>` | **projection of wider authority** (router trust) |
| `supervisor_bound_name` | `Option<String>` | projection |
| `supervisor_bound_peer_id` | `Option<String>` | projection |
| `supervisor_bound_address` | `Option<String>` | projection |
| `supervisor_bound_epoch` | `Option<u64>` | projection |

The DSL comment (lines 114-134) is explicit: the authoritative
authorization fact is in MeerkatMachine now, but the *companion trust
edge* still lives in the router-owned `TrustedPeers`
(`meerkat-comms/src/trust.rs:232`). Mutations must step-lock: the DSL
mutator accepts first, then the shell calls `add_trusted_peer` /
`remove_trusted_peer` on the router trust store. The DSL owns the
authorization *discriminant*; the router owns the *trust edge*. Two
linked truths. This is already flagged in the DSL as a "companion
field" consistency invariant (`supervisor_binding_consistency`).

Scope note: the supervisor bridge is a per-session concept (each session
can be authorized by a different supervisor); these fields are correctly
session-scoped. But the fact that a *second* store holds a companion
truth weakens the single-owner invariant — see §3.

### 1.10 Track-B (R5) peer projection (lines 175-179)

| Field | Type | Classification |
|---|---|---|
| `local_endpoint` | `Option<PeerEndpoint>` | session-scoped |
| `direct_peer_endpoints` | `Set<PeerEndpoint>` | session-scoped |
| `mob_overlay_peer_endpoints` | `Set<PeerEndpoint>` | **projection of MobMachine.wiring_edges + member_session_bindings** |
| `peer_projection_epoch` | `u64` | session-scoped watermark |
| `mob_overlay_epoch` | `u64` | session-scoped watermark threaded from MobMachine.topology_epoch |

The DSL comment (lines 136-180) already documents this as a projection:
the identity-level wiring graph is owned by MobMachine, projected here
onto endpoint sets by the `RecomputeMobPeerOverlay` composition driver.
The overlay set is explicitly a derived projection (dogma #11). Correct
pattern — canonical authority is in MobMachine, MeerkatMachine holds a
derived read-side view. The split is formalized by the separate epoch
namespaces so overlay staleness cannot be confused with direct
mutation.

### 1.11 Summary of classifications

Out of 34 fields in `state { ... }`:

- **Session-scoped (proper home):** 25 fields
- **Projection of wider authority (canonical elsewhere):** 9 fields
  - `mcp_server_states` (configuration lives in user/project mcp.toml;
    the connection instance is per-session — correctly split)
  - `supervisor_binding_kind` + 4 companion `supervisor_bound_*` fields
    (authority is DSL, companion trust edge is router)
  - `peer_ingress_mob_id` (id-reference across machine boundary)
  - `mob_overlay_peer_endpoints` + `mob_overlay_epoch` (projection of
    MobMachine.wiring graph)
- **Genuine scope mismatch (field should move out):** 0

No field in MeerkatMachine state is misplaced in the sense that its
*natural* lifecycle is wider than the session. Every field either dies
with the session or is a projection of something wider whose *copy* in
MeerkatMachine is properly per-session.

## Section 2: Session Identity Contract + Append-Only Verification

### 2.1 Invariants (what "same session" means)

The canonical `Session` type is `meerkat-core/src/session.rs:32-47`:

```rust
pub struct Session {
    version: u32,
    id: SessionId,
    pub(crate) messages: Arc<Vec<Message>>,
    created_at: SystemTime,
    updated_at: SystemTime,
    metadata: serde_json::Map<String, serde_json::Value>,
    usage: Usage,
}
```

`SessionId` is a UUID newtype (`meerkat-core/src/types.rs:736`). It is
the stable identity fact. Within a single "same session" the contract
is:

- **Invariant:** `id` (SessionId), `created_at`
- **Monotonic:** `updated_at`, `usage` (cumulative), `messages.len()` —
  message history is append-only by convention in the reducer
- **Mutable:** `metadata` (arbitrary key/value), model/provider (via
  events), tool set (via session tool visibility), skills (via
  metadata-recorded manifests)

All of those mutations are, by design, recorded as events in the
append-only log before being projected into the session. That is the
event-sourcing contract: the Session value is a fold over the event
stream.

### 2.2 Append-only verification — EventStore trait

`meerkat-session/src/event_store.rs:30-49`:

```rust
pub trait EventStore: Send + Sync {
    async fn append(&self, session_id: &SessionId, events: &[AgentEvent])
        -> Result<u64, EventStoreError>;
    async fn read_from(&self, session_id: &SessionId, from_seq: u64)
        -> Result<Vec<StoredEvent>, EventStoreError>;
    async fn last_seq(&self, session_id: &SessionId)
        -> Result<u64, EventStoreError>;
}
```

The `EventStore` trait has **no** `replace`, `truncate`, `delete`,
`rewrite`, or bulk-overwrite method. It is genuinely append-only at the
type level: the contract does not expose a way to invalidate an already
appended event. This is a strong contract. A custom implementation
could, of course, violate it from inside the implementation body, but
*callers* cannot express destructive intent through the trait.

### 2.3 Append-only gap — SessionStore trait

`meerkat-core/src/session_store.rs:60-77`:

```rust
pub trait SessionStore: Send + Sync {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError>;
    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError>;
    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError>;
    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError>;
    async fn exists(&self, id: &SessionId) -> Result<bool, SessionStoreError>;
}
```

This is the **weak** part of the contract. `save(&Session)` takes the
full Session value and stores it; there is nothing in the trait that
prevents a caller from calling `save()` with a Session whose `messages`
vector is shorter than what was previously stored. The trait contract
does not express "subsequent `save()` calls for a given SessionId must
monotonically extend `messages`."

In practice the callers (EphemeralSessionService,
PersistentSessionService, the projector) respect this by convention,
and `.rkat/sessions/` is explicitly documented as derived projection
output. But at the trait level, `SessionStore::save` is
truncate-and-replace semantics — identical to writing any file.

**This is a convention, not a type-level contract.** A custom
`SessionStore` implementation could replace the full session state and
the type system would not object. The "same session" identity contract
is therefore weaker at the SessionStore boundary than at the EventStore
boundary.

### 2.4 The `fork_at` operation — a legitimate truncation

`meerkat-core/src/session.rs:798-814` exposes `Session::fork_at(index)`,
which returns a *new* Session with a truncated message history:

```rust
pub fn fork_at(&self, index: usize) -> Self {
    let truncated = self.messages[..index.min(self.messages.len())].to_vec();
    Session { messages: Arc::new(truncated), ..self.clone() }
}
```

This is not a violation of the contract because `fork_at` returns a
new Session (new implicit identity assigned by the caller if they
persist it under a new id). Within one `SessionId`, the trait-level
append-only contract holds at the EventStore level.

### 2.5 Operations NOT allowed within a same-SessionId

Reviewing the public API:

- Replacing the event log for a `SessionId`: **not expressible**
  through `EventStore` (good)
- Overwriting the Session snapshot for a `SessionId` with a shorter
  history: **expressible** through `SessionStore::save` (weak contract)
- Rotating the SessionId of an existing event log: **not expressible**
  (no method takes two SessionIds)
- Merging two SessionIds: **not expressible** (good)

The practically meaningful weak point is (2): a careless
`SessionStore::save` implementation or caller can overwrite the
materialized session snapshot with a shorter history, de-syncing from
the event log. This is mitigated by the doc that `.rkat/sessions/` is
derived projection output — it is by convention reconstructable from
the event log — but the trait does not encode that.

## Section 3: Findings Table

| # | File:line | Concern | Current scope | Proper scope | Remediation | Urgency |
|---|---|---|---|---|---|---|
| F1 | `meerkat-core/src/session_store.rs:62` | `SessionStore::save(&Session)` allows truncate-and-replace; no type-level append-only guarantee | trait-wide destructive replace | append-or-extend only | Option A: split trait into `SessionSnapshotStore::write_snapshot` (derived, caller-asserted idempotent) + explicit `AppendOnlySessionStore` with `append_messages()`. Option B: formalize the doc (snapshot = projection) and route all persistence through the EventStore only, making SessionStore a test-only convenience. | 0.7 |
| F2 | `meerkat-runtime/src/meerkat_machine/dsl.rs` (supervisor_bound_*) | Supervisor authorization has companion trust edge in router `TrustedPeers` | split ownership with step-lock | single owner | **Closed in C-F2 via option (b)**: step-lock formalised as a generated handoff obligation pair in `supervisor_trust_bundle` composition — `supervisor_trust_publish` (producer effect `PublishSupervisorTrustEdge` → feedback `SupervisorTrustEdgePublished` / `SupervisorTrustEdgePublishFailed`) and `supervisor_trust_revoke` (producer effect `RevokeSupervisorTrustEdge` → feedback `SupervisorTrustEdgeRevoked` / `SupervisorTrustEdgeRevokeFailed`), both with `ClosurePolicy::AckRequired`. Compat bridge: `meerkat-machine-schema/src/compat/supervisor_trust_bridge.rs`; composition: `supervisor_trust_bundle_composition` in `catalog/compositions.rs`; seam-inventory entry: `## Declared Handoff Obligation Pairs`. Option (a) rejected as out-of-scope: `AuthMachine` is per-binding auth-lease lifecycle; supervisor-binding trust is a per-session fact and already lives on `MeerkatMachine`. | closed (C-F2) |
| F3 | `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:112` | `peer_ingress_mob_id` references MobId with no structural "mob-exists" invariant in MeerkatMachine | cross-machine id reference | foreign-key w/ cleanup discipline | **Closed in C-F3**: added `mob_destroying_session_ingress` handoff obligation pair on the `mob_destroy_session_ingress_bundle` compat composition — producer effect `RequestSessionIngressDetachForMobDestroy { mob_id, agent_runtime_id }` → realising actor `mob_destroy_session_ingress_owner` → feedback `SessionIngressDetachedForMobDestroy` (success) or `SessionIngressDetachFailedForMobDestroy` (failure). `closure_policy: AckRequired`. Compat bridge: `meerkat-machine-schema/src/compat/mob_destroy_session_ingress_bridge.rs`; composition: `mob_destroy_session_ingress_bundle_composition` in `catalog/compositions.rs`. `xtask seam-inventory` now carries a `## Destroy-obligation Pairing (C-F3)` section that walks every canonical routed `Request*Destroy*` effect and flags any without a paired ingress-detach protocol — zero debt with the pair in place; flags 1+ debt if the pair is removed. | closed (C-F3) |
| F4 | `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs:177-179` | `mob_overlay_peer_endpoints` + `mob_overlay_epoch` are derived projections; monotony is guarded by the per-overlay epoch but not re-derivable from MobMachine state alone | derived projection w/ authoritative shadow | pure projection | The pattern is already documented and the two-epoch-namespace split is correct. No remediation required — this is a "how we classify projections" documentation point. | none |
| F5 | `meerkat-runtime/src/meerkat_machine/mod.rs:417` | `comms_drain_slots: RwLock<HashMap<SessionId, CommsDrainSlot>>` lives parallel to `sessions` — two per-session maps | single-owner violation by composition | unified session entry | Move `CommsDrainSlot` into `RuntimeSessionEntry` so "session exists" is one HashMap insertion; eliminates the class of bugs where the two maps go out of sync. | 0.7 |
| F6 | `meerkat-mcp/src/router.rs:886-911` | MCP router is per-session by construction, but `mcp.toml` config is user/project scoped — reconnect coordination across sessions is not represented | configuration read at session build time; no wider authority over reconnect | shared reconnect authority optional | Not urgent; this is by design (each session's MCP bindings are independent). If we want cross-session reconnect coordination later, introduce a `UserMcpAuthority` outside Meerkat/MobMachine. Flagged for 0.8+. | 0.8+ |
| F7 | `meerkat-core/src/session.rs:38` | `messages: Arc<Vec<Message>>` has no type-level append-only witness; any code holding `&mut Session` can call `.fork_at(0)` and replace | convention-driven | typed witness | Introduce `AppendOnlyMessages` newtype exposing only `.push()`/`.extend()`, not arbitrary `&mut Vec`. `fork_at` becomes an explicit fork, not a truncation of the same session. | 0.7 |

## Section 4: Wave-C Implications

Three observations frame wave-c decisions:

1. **No state field needs to *leave* MeerkatMachine.** The audit did not
   find any field whose natural lifecycle is wider than a session and
   that MeerkatMachine is the canonical holder of. Every "wider" field
   is already understood as a projection, and the projection-ownership
   is already explicit in DSL comments.

2. **The weakest part of the session-identity contract is
   `SessionStore::save`.** The EventStore is genuinely append-only at
   the trait level; the SessionStore is not. Wave-C should decide
   whether to harden this (F1, F7) or accept it as a deliberate
   test/fixture affordance and document it loudly.

3. **Two "split-authority" patterns remain live:** supervisor trust
   (F2) and mob ingress (F3). Both already have
   invariant-style mitigations inside the DSL, but neither is
   protocolized the way formal seam closure handled the four named
   handoffs. These are the natural wave-c candidates for adding
   obligation pairs.

Secondary wave-c items: F5 (collapse `comms_drain_slots` into
`RuntimeSessionEntry`) is a pure tidying that removes one class of
"two maps desync" bug; F6 is deferred to 0.8+ because shared-MCP
reconnect coordination is a product decision, not a correctness
blocker.

The "MeerkatMachine is strictly per-session" framing from Luka's
context survives intact: the kernel holds per-session state, projects
wider authority as needed, and does not pretend to be a long-lived
agent identity. Long-lived identity is MobMachine + a future
AgentIdentity construct, as stated.
