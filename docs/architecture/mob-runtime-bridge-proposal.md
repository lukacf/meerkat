# Mob Runtime Bridge: Type-Enforced Machine Boundary

## Problem

The MobMachine manages members by calling MeerkatMachine methods directly through
`Arc<MeerkatMachine>`. The provisioner and actor hold a raw reference and call
`retire_runtime()`, `unregister_session()`, `interrupt_current_run()`, and 7 other
methods as direct Rust method calls.

This violates the runtime dogma in two ways:

1. **The inter-machine boundary is unformalized.** The composition schema declares
   `RequestRuntimeRetire` as a routed effect, but the realization is a direct method
   call. There's no protocol machine, no typed handoff, no acknowledgment states.
   The RMAT audits within-machine authority but not between-machine boundary crossings.

2. **Remote members are impossible.** The mob can only manage members whose
   `MeerkatMachine` is in the same process. `RuntimeBinding::External { peer_id, address }`
   exists in the roster but has no lifecycle management path — you can wire comms to
   an external peer, but you can't retire, interrupt, or destroy them through the mob.

Both problems have the same root cause: the mob holds `Arc<MeerkatMachine>` instead
of a typed protocol handle.

## Solution

Replace `Arc<MeerkatMachine>` in the mob with `Arc<dyn MobRuntimeBridge>` — a trait
that the mob crate defines and the runtime crate implements. The mob can only call
methods on the trait. The compiler enforces the boundary.

### The trait

Defined in `meerkat-mob`, not in `meerkat-runtime`. The mob owns its dependency
contract.

```rust
/// Protocol boundary between MobMachine and the MeerkatMachine instances
/// it supervises. All member lifecycle operations go through this trait.
///
/// Local members: implemented by a wrapper around MeerkatMachine.
/// Remote members: implemented by a comms-based protocol client.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobRuntimeBridge: Send + Sync {
    // --- Lifecycle ---

    /// Pre-register session bindings before session creation.
    async fn prepare_session_bindings(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionRuntimeBindings, MobError>;

    /// Attach a CoreExecutor to a registered session.
    async fn attach_executor(
        &self,
        session_id: &SessionId,
        executor: Arc<dyn CoreExecutor>,
    ) -> Result<(), MobError>;

    /// Retire a member's runtime (drain queued work, archive session).
    async fn retire_session(
        &self,
        session_id: &SessionId,
    ) -> Result<RetireReport, MobError>;

    /// Unregister a session from the runtime (teardown without archival).
    async fn unregister_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), MobError>;

    /// Interrupt the member's in-flight run.
    async fn interrupt_run(
        &self,
        session_id: &SessionId,
    ) -> Result<(), MobError>;

    // --- Input delivery ---

    /// Deliver an input to the member's session and receive a completion handle.
    async fn deliver_input(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<CompletionHandle>), MobError>;

    // --- Observation ---

    /// Check whether a session is registered in the runtime.
    async fn is_session_registered(
        &self,
        session_id: &SessionId,
    ) -> bool;

    /// Check whether a session has an attached executor.
    async fn has_executor(
        &self,
        session_id: &SessionId,
    ) -> bool;

    // --- Comms ---

    /// Conditionally spawn a comms drain for the session.
    async fn ensure_comms_drain(
        &self,
        session_id: &SessionId,
        keep_alive: bool,
        comms_runtime: Arc<dyn CommsRuntime>,
    ) -> Result<(), MobError>;

    /// Abort in-flight comms drain for the session.
    async fn abort_comms_drain(
        &self,
        session_id: &SessionId,
    ) -> Result<(), MobError>;
}
```

### Local implementation

Lives in `meerkat-runtime` (or a bridge module). Wraps `Arc<MeerkatMachine>` and
forwards calls — identical behavior to today:

```rust
pub struct LocalMobRuntimeBridge {
    machine: Arc<MeerkatMachine>,
}

#[async_trait]
impl MobRuntimeBridge for LocalMobRuntimeBridge {
    async fn retire_session(&self, session_id: &SessionId) -> Result<RetireReport, MobError> {
        self.machine
            .retire_runtime(session_id)
            .await
            .map_err(|e| MobError::Internal(e.to_string()))
    }

    async fn unregister_session(&self, session_id: &SessionId) -> Result<(), MobError> {
        self.machine.unregister_session(session_id).await;
        Ok(())
    }

    // ... 8 more methods, each a one-line forward
}
```

### Remote implementation (future)

Would live in a comms-bridge module. Sends lifecycle commands over comms:

```rust
pub struct RemoteMobRuntimeBridge {
    peer_id: PeerId,
    comms: Arc<dyn CommsRuntime>,
}

#[async_trait]
impl MobRuntimeBridge for RemoteMobRuntimeBridge {
    async fn retire_session(&self, session_id: &SessionId) -> Result<RetireReport, MobError> {
        // Send lifecycle command, wait for typed ack
        let response = self.comms
            .send_request(&self.peer_id, LifecycleCommand::Retire { session_id })
            .await?;
        response.into_retire_report()
    }
    // ...
}
```

On the receiving end, the remote MeerkatMachine processes this through its existing
ingress path:

1. Comms message arrives at the member's session
2. Inbox drain classifies it as a lifecycle command (new classification category)
3. Authorization check: is the sender a trusted supervisor?
4. Routes to the appropriate machine input (`Retire`, `Destroy`, etc.)
5. Machine processes it, shell sends ack back over comms

No new machines, no new compositions. Just a new ingress classification category
in the existing drain/classify/admit pipeline.

### Per-member bridge construction

The roster already carries `RuntimeBinding` per member — `Session` (local) or
`External { peer_id, address }` (remote). Bridge construction happens at spawn
time based on the binding:

```rust
fn bridge_for_binding(
    binding: &RuntimeBinding,
    machine: &Arc<MeerkatMachine>,
    comms: &Option<Arc<dyn CommsRuntime>>,
) -> Arc<dyn MobRuntimeBridge> {
    match binding {
        RuntimeBinding::Session => Arc::new(LocalMobRuntimeBridge {
            machine: machine.clone(),
        }),
        RuntimeBinding::External { peer_id, address } => Arc::new(RemoteMobRuntimeBridge {
            peer_id: peer_id.parse().unwrap(),
            comms: comms.clone().expect("comms required for external members"),
        }),
    }
}
```

The provisioner/actor stores the bridge handle per member. All lifecycle calls
go through the bridge. The mob never touches `MeerkatMachine` directly.

## Migration

### What changes

| Component | Change |
|-----------|--------|
| `meerkat-mob/src/runtime/bridge.rs` | New file: trait definition |
| `meerkat-mob/src/runtime/provisioner.rs` | `Option<Arc<MeerkatMachine>>` → `Option<Arc<dyn MobRuntimeBridge>>` (38 call sites) |
| `meerkat-mob/src/runtime/actor.rs` | Same type change (3 call sites) |
| `meerkat-mob/src/build.rs` | Accept `dyn MobRuntimeBridge` in builder |
| `meerkat-runtime/src/mob_adapter.rs` | Becomes `impl MobRuntimeBridge for LocalMobRuntimeBridge` |
| `meerkat-mob/Cargo.toml` | Remove `meerkat-runtime` dependency (or downgrade to dev-only) |

### What doesn't change

- `MeerkatMachine` public API — unchanged
- `MobMachineCommand` surface — unchanged
- Schema / TLA+ — unchanged
- All surfaces (CLI, RPC, REST, MCP, SDKs) — unchanged
- Session service — unchanged
- Comms infrastructure — unchanged
- Tests — behavioral equivalence, no new test failures

### Dependency inversion

Today: `meerkat-mob` depends on `meerkat-runtime` (to get `Arc<MeerkatMachine>`).

After: `meerkat-mob` defines the trait. `meerkat-runtime` implements it.
The dependency direction reverses at the type level — the mob owns its contract,
the runtime fulfills it. Construction happens at the surface/factory layer that
has access to both crates.

This is textbook dependency inversion and it matches the existing pattern:
`meerkat-core` defines `AgentLlmClient`, `meerkat-client` implements it.

## Boundary enforcement

### Compile-time

The mob crate no longer imports `MeerkatMachine`. It can't call methods that
aren't on `MobRuntimeBridge`. Adding a new cross-boundary call requires adding
it to the trait — which is a conscious, reviewable change, not an accidental
method call on a leaked reference.

### RMAT extension

Add a new RMAT rule: **no direct `MeerkatMachine` method calls from mob crate
source files.** If `meerkat-mob` doesn't depend on `meerkat-runtime` at all
(only on the trait), this is enforced by the compiler. If there's still a
transitive dependency, the RMAT grep catches any direct usage.

### Schema alignment

The 10 trait methods map to the composition schema's declared cross-boundary
interactions. The trait surface IS the composition protocol — just expressed as
a Rust trait instead of only a schema declaration. Future work could generate
the trait from the composition schema, closing that loop too.

## What this enables

1. **Remote mob members** — implement `RemoteMobRuntimeBridge`, wire it through
   comms. The mob doesn't change.

2. **Testing** — mock the bridge for mob tests. Today mob tests need a real
   `MeerkatMachine` and session service. With the trait, they can use a mock
   that returns canned responses. Simpler, faster, more targeted tests.

3. **Supervision protocol** — the trait IS the supervision protocol. Adding
   health checks, heartbeats, or liveness probes means adding methods to the
   trait. The mob calls `bridge.health_check()` and gets back a typed result.
   Local impl returns instantly; remote impl pings over comms.

4. **Multi-process mobs** — a mob can have a mix of local and remote members.
   Each member has its own bridge impl. The mob treats them uniformly.

5. **Formal protocol verification** — the trait methods map to composition
   protocol messages. If we later formalize the protocol as a TLA+ module,
   the trait is the Rust realization. One source (trait) generates both
   the protocol spec and the implementation interface.

## Relationship to other proposals

- **Machine simplification**: independent. The bridge works with the current
  machine surface or a simplified one. Simplification changes what methods
  the trait exposes; the bridge pattern doesn't change.

- **Machine DSL**: independent. The DSL generates the machine internals.
  The bridge wraps the machine externals. They don't overlap.

- **Live voice integration**: the bridge pattern applies there too. If a
  live voice session is managed by a mob (voice agent as a member), the
  bridge handles its lifecycle the same way. The mob doesn't care that
  the member is a voice session.

## Complexity estimate

| Component | Lines | Risk |
|-----------|-------|------|
| Trait definition | ~80 | Low — 10 methods, clear signatures |
| Local implementation | ~100 | Low — one-line forwards |
| Provisioner migration | ~200 changed | Low — mechanical type change |
| Actor migration | ~30 changed | Low — 3 call sites |
| Builder/factory wiring | ~50 changed | Low |
| Tests (mock bridge) | ~150 new | Low — enables simpler mob tests |
| **Total** | ~600 | **Low overall — no behavioral change** |

Net effect: ~600 lines of change, zero behavioral difference, compile-time
enforcement of the inter-machine boundary, and the foundation for remote
member management.
