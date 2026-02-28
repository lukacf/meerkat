# Architecture Review: Proposed Meerkat Mob API Additions

**Date:** 2026-02-28
**Reviewer:** Claude (Architecture Review)
**Scope:** Feature requests for meerkat-mob 0.4.0 convenience, extension, and infrastructure APIs

---

## Tier 1: Convenience Accessors

### 1a. `Roster::session_id(meerkat_id)` — ACCEPT

Clean, well-scoped. The `MemberRef` enum match is genuinely boilerplate.

**Design note:** `BackendPeer` now also carries `session_id: Option<SessionId>` (for local session bridges). The implementation must decide whether `BackendPeer { session_id: Some(sid), .. }` returns `Some(&sid)` or `None`. The proposal says "returns None for backend peers" — this is the right default for mob-to-session bridging, but document the behavioral contract explicitly since callers may expect different semantics for bridged backend peers.

No WASM concern. No trait change. No new types.

### 1b. `MobHandle::subscribe_agent_events(meerkat_id)` — ACCEPT WITH RESERVATION

The problem is real: `MobHandle` does not hold a reference to the session service. It holds `command_tx`, `roster`, `task_board`, `definition`, `state`, `events`, `mcp_servers`, `flow_streams`. The session service lives exclusively in `SubagentBackend` (via `MobProvisioner`) inside the `MobActor`. The handle was deliberately kept thin — a channel-based command proxy with direct reads on shared `Arc` state.

**Recommended approach:** Store `Arc<dyn MobSessionService>` as an additional field on `MobHandle`. The handle already holds 7 `Arc` fields; an 8th doesn't change the character. Routing through the actor via `MobCommand` would add latency to a read-only operation. Acknowledge that this widens the handle's surface area and document it as a conscious choice.

**WASM note:** `EventStream` is `Pin<Box<dyn Stream<Item = EventEnvelope<AgentEvent>> + Send>>`. On WASM, the `+ Send` bound is dropped via cfg-gated async_trait. Verify that the WASM cfg gates propagate correctly through the new code path.

### 1c. `AttributedEvent` / source attribution — ACCEPT OPTION A, REJECT OPTION B

**Option B violates a core principle:** `AgentEvent` is a session-level primitive defined in `meerkat-core`. It has no concept of mobs, `MeerkatId`, or multi-agent identity. `meerkat-core` does not depend on `meerkat-mob`. Adding mob-level attribution to a core type creates an upward dependency. Reject entirely.

**Option A is correct.** The type belongs in `meerkat-mob`, not `meerkat-core`. Note that the existing `ScopedAgentEvent` in `meerkat-core` serves a different purpose (hierarchical scope path for nested sub-agent streaming). `AttributedEvent` is mob-specific attribution. Keep them separate.

**Design refinement:** Wrap `EventEnvelope<AgentEvent>` instead of raw `AgentEvent`:

```rust
pub struct AttributedEvent {
    pub source: MeerkatId,
    pub profile: ProfileName,
    pub envelope: EventEnvelope<AgentEvent>,
}
```

This preserves the full event metadata chain (`event_id`, `source_id`, `seq`, `mob_id`, `timestamp_ms`) without information loss.

---

## Tier 2: Multi-Agent Operations

### 2a. `send_or_spawn` — REJECT (prefer 3a SpawnPolicy)

The proposal acknowledges the race condition but solves it in the wrong place. `MobHandle` doesn't do locking — the actor serializes mutations. The correct fix is an atomic `SpawnAndSend` command variant in the actor protocol.

More importantly, `send_or_spawn` requires the caller to already know the correct profile for an unknown `MeerkatId`. This is application logic. The method handles one pattern (spawn-then-send) but doesn't address spawn-then-subscribe, spawn-then-wire, or other first-contact patterns.

Tier 3a (`SpawnPolicy`) solves the general case properly. `send_or_spawn` is a half-measure.

### 2b. `subscribe_all_agent_events` — CONDITIONAL ACCEPT (non-merged only)

Depends on 1b. The non-merged signature `Vec<(MeerkatId, EventStream)>` is clean and honest: "here are the streams, point-in-time, you merge them."

The merged-stream variant promises a single stream but can't promise it stays up-to-date (the proposal's own caveat). The merged variant is Tier 4a's job. Accept the non-merged version only.

### 2c. `respawn` — ACCEPT

Simple composition of retire + spawn. Should be implemented as an atomic actor command (`MobCommand::Respawn`) that retires and spawns within a single actor processing step. No roster gap.

**Verify:** comms trust removal from retire doesn't interfere with trust setup for the new spawn when they happen in the same actor step.

---

## Tier 3: Extension Points

### 3a. `SpawnPolicy` trait — ACCEPT WITH DESIGN CHANGES

The concept is sound. The proposed implementation needs redesign.

**Problem with proposal:** The per-`MeerkatId` lock inside `MobHandle` is solving a problem that doesn't exist. The actor already serializes all mutations. Concurrent `ExternalTurn` commands for the same unknown ID are serialized by the actor's command channel. The first triggers the spawn; the second sees the agent in the roster.

**Correct design:**
- Store `Option<Arc<dyn SpawnPolicy>>` on the `MobActor` (or `MobProvisioner`)
- When `MobCommand::ExternalTurn` targets an unknown `MeerkatId`, the actor calls `spawn_policy.resolve(target)`
- If it returns a spec, the actor spawns then delivers the message
- All within the actor's sequential processing — no per-ID locks needed

This eliminates `DashMap`, per-ID mutexes, and lock ordering concerns entirely.

**WASM note:** Add the standard dual cfg gate:
```rust
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
```

### 3b. `SessionAgentBuilder` lifecycle phases — REJECT

The current trait is minimal by design:

```rust
pub trait SessionAgentBuilder: Send + Sync {
    type Agent: SessionAgent + Send + 'static;
    async fn build_agent(&self, req: &CreateSessionRequest, event_tx: mpsc::Sender<AgentEvent>) -> Result<Self::Agent, SessionError>;
}
```

**Problems with the proposal:**

1. **Comms is already wired during construction.** In `AgentFactory::build_agent()`, the comms runtime is created, tools are composed with comms, and the agent is built with everything wired. The `OnceCell` pattern mentioned as a problem is an application-level composition ordering issue, not a framework deficiency.

2. **`prepare` introduces upward dependency.** The hook receives `MeerkatId` and `ProfileName` — mob-level concepts. `SessionAgentBuilder` is a session-level abstraction. This creates a conceptual dependency from session infrastructure to mob orchestration.

3. **Every implementation now must understand three phases.** Test mocks, the `FactoryAgentBuilder`, and any custom builders all grow in surface area for a lifecycle protocol that most don't need.

**Alternative:** If mob-specific pre/post-build hooks are needed, add them to `MobProvisioner` (which already knows about `MeerkatId` and `ProfileName`) rather than polluting the generic `SessionAgentBuilder` trait.

---

## Tier 4: Infrastructure Primitives

### 4a. Mob-level agent event bus — CONDITIONAL ACCEPT (with redesign)

The concept is valuable but the implementation needs separation from the actor.

**Problem:** Adding event routing to the actor couples message forwarding with mutation serialization. Every `AgentEvent` from every agent would flow through the actor, which exists for command processing.

**Recommended design:** Add a dedicated `MobEventRouter` task that:
- Subscribes to the session-level broadcast of each agent
- Tags and forwards to a mob-level `broadcast::Sender<AttributedEvent>`
- Listens to mob events (`MeerkatSpawned`/`MeerkatRetired`) to add/remove stream sources
- Runs independently of the actor

The `MobHandle` holds a `broadcast::Sender<AttributedEvent>` for subscriber access. This keeps the actor focused and avoids coupling event forwarding to mutation serialization.

**WASM note:** `tokio::sync::broadcast` is **NOT provided by `tokio_with_wasm`** — it only provides `mpsc`, `oneshot`, `watch`, `RwLock`, `Mutex`, `Semaphore`. The session-level broadcast in `EphemeralSessionService` already has this problem (the `BroadcastEventReceiver` type alias in `meerkat-session/src/lib.rs` uses native `tokio::sync::broadcast`). A mob-level broadcast would inherit the same WASM gap. On WASM, the event bus would need to use `mpsc` fan-out or a custom broadcast implementation. This isn't a blocker but it's a design constraint — the `broadcast::Receiver<AttributedEvent>` return type in the proposal won't compile on WASM without a platform-specific abstraction.

### 4c. Session store metadata — REJECT

**Reasons:**

1. **Application concern.** The motivating case (BigQuery store needs agent_type/owner_id) is application-specific. The application can wire metadata through its own `SessionStore` implementation.

2. **Stringly-typed.** `HashMap<String, String>` labels violate the project's "typed enums over `serde_json::Value`" principle. If metadata is important enough for a trait method, it's important enough for typed fields.

3. **`SessionBuildOptions` already exists** for passing build-time context. If mob-level metadata needs to reach the store, it can flow through `SessionBuildOptions` -> `CreateSessionRequest` -> builder -> store at construction time, not at every save.

4. **Trait pollution.** Adding `save_with_metadata` to `SessionStore` means all store implementations must acknowledge metadata's existence, even if the default ignores it.

---

## Cross-Cutting WASM Concerns

`meerkat-mob` is WASM-aware (`tokio_with_wasm` on WASM, `tokio` on native). Any new async traits need:

```rust
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
```

Return types involving streams need conditional `Send` bounds. The main risk areas are `SpawnPolicy` (3a) and `AttributedEvent` streams (4a).

**Critical finding:** `tokio::sync::broadcast` is NOT available in `tokio_with_wasm`. The existing session-level broadcast in `EphemeralSessionService` (ephemeral.rs:108) already has this gap. Any proposal using `broadcast::Sender/Receiver` (particularly 4a) must account for a WASM-compatible alternative or accept that the feature is native-only.

---

## Summary

| Item | Verdict | Rationale |
|------|---------|-----------|
| 1a `Roster::session_id()` | **Accept** | Pure convenience, no new concepts |
| 1b `subscribe_agent_events` | **Accept** | Add `session_service` Arc to MobHandle |
| 1c `AttributedEvent` | **Accept Option A** | Mob-level type wrapping EventEnvelope; reject Option B |
| 2a `send_or_spawn` | **Reject** | Half-measure; prefer 3a SpawnPolicy |
| 2b `subscribe_all_agent_events` | **Accept (non-merged)** | Point-in-time snapshot; merged is 4a's job |
| 2c `respawn` | **Accept** | Atomic actor command, clean composition |
| 3a `SpawnPolicy` | **Accept (redesigned)** | Move to actor, no per-ID locks needed |
| 3b `SessionAgentBuilder` phases | **Reject** | Pollutes session-level trait with mob concepts |
| 4a Mob event bus | **Accept (redesigned)** | Separate MobEventRouter task, not in actor |
| 4c Session store metadata | **Reject** | Application concern, stringly-typed |

The strongest additions are **1a**, **2c**, and **3a** (redesigned). They solve real problems at the right abstraction level without widening core trait surfaces or creating dependency inversions.
