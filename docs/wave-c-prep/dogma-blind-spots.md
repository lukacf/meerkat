# Dogma Catalog Blind Spots — Wave-C Prep

**Scope.** The 70-item catalog at `~/.codex/dogma-violations.md` is focused on
semantic-state-ownership: stringly typing, shadow state, duplicate authority,
name-keyed routing, surface-level mutation, folklore comments. These don't exhaust
the production-bug surface. This doc enumerates **classes of failure modes the
catalog cannot see**, with concrete sites and enforcement paths. Seeds issue #341.

Line citations: worktree `/Users/luka/src/meerkat/.claude/worktrees/wave-a-demo`,
branch `dogma/wave-a-demolition`.

---

## 1. Async cancellation races

**Def.** Futures dropped mid-transition leave state partially applied. A `select!`
arm wins over a half-written row; a spawned task runs on after its owner; an `.await`
between two state updates that the supervising future never reaches.

**Why catalog misses it.** Catalog reasons about field values, not about which future
owns the continuation. Cancel-safety is a property of the *code path*, not the machine.

**Sample sites.**
- `meerkat-session/src/ephemeral.rs:2548-2572` — top-level `select!` races `run_fut`
  vs `interrupt_wait` vs `agent_event_rx.recv()`. `run_fut` mutates agent state; on
  interrupt, dropped mid-turn. Blast radius = whole agent loop; no proof every inner
  `await` is cancel-safe.
- `meerkat-mob/src/runtime/actor.rs:3320` — `spawn_fut.catch_unwind().await` converts
  panics to `MobError::Internal`, but outer actor cancellation mid-spawn orphans
  `spawn_ticket` with no `SpawnProvisioned` reply.
- `meerkat-mob/src/runtime/actor.rs:6102` — flow-stream `select!`; on actor shutdown
  the completion signal is lost.
- `meerkat-rpc/src/realtime_ws.rs:2891` — realtime WS `select!` over DSL + audio +
  keepalive; comment at line 1789 explicitly flags "arms cannot carry state" —
  cancellation semantics load-bearing and undermodeled.
- `meerkat-anthropic/src/client.rs:966` — streaming `while let Some(chunk) = stream.next().await`;
  drop mid-SSE leaves partial buffer, retry replay hazard.

**Severity.** **High.** Exercised on every `^C`; partial-write paths uncovered.

**Enforcement.** Attribute `#[cancel_safe]` / `#[cancel_not_safe]` on state-mutating
`async fn`; xtask AST lint rejects `select!`/`spawn` arms that drop a
`#[cancel_not_safe]` future without `abort_handle` or rollback pairing. Cheaper
first step: `CancellationContext` newtype threaded through transitions with
Drop-guarded compensation.

---

## 2. Resource lifetime / leaks

**Def.** Who holds the last `Arc`, who calls `abort()`, when does the TCP socket
close. OS-level liveness, not logical state.

**Why catalog misses it.** Ownership graphs are orthogonal to authority writes.

**Sample sites.**
- `meerkat-cli/src/main.rs:140-142` — `TuiPipeline` holds three `Option<JoinHandle<_>>`;
  only one `.abort()` at 5452; other handles may leak.
- `meerkat-rest/src/lib.rs:2169-2195` — `spawn_event_forwarder` → `drain_event_forwarder`
  hand-off window; panic in between leaks the task.
- `meerkat-store/src/realm.rs:114-214` — realm lease heartbeat; abort only in optional
  `stop()`. Silent leak of SQLite-writing task.
- 130 `Arc<Mutex<...>>` / `Arc<RwLock<...>>` fields workspace-wide; none flagged for
  Drop semantics (DB handles, WAL flush).
- `meerkat-comms/src/inproc.rs:108+` — global `InprocRegistry` never unregisters
  dropped senders without explicit call; 17 `pub fn` with no ownership contract.

**Severity.** **Medium–High.** Long-lived RPC/REST: hours-scale memory growth.

**Enforcement.** Trait `ResourceOwned` with `async fn shutdown()`; xtask lint rejects
structs holding `JoinHandle` / `broadcast::Sender` / `Notify` without
`ResourceOwned` or `#[leak_ok = "reason"]`. Pair with `#[must_use]` on handles.

---

## 3. Timing / duration violations

**Def.** State machines carry no model of *how long* a transition takes. A
`WaitingForLlm` state with 300s is indistinguishable from one with 30s until a user
waits too long.

**Why catalog misses it.** DSL models order, not wall-clock.

**Sample sites.**
- `meerkat-anthropic/src/client.rs:19` — `DEFAULT_REQUEST_TIMEOUT = 300s` hardcoded,
  not in `Config`.
- `meerkat-anthropic/src/client.rs:1483-1484` — stub tests hardcode 5s/120s; divergent
  from production.
- `meerkat-store/src/realm.rs:89-91` — 30s stale + 20ms retry + 5s timeout; three magic
  numbers, no relational test.
- `meerkat-store/src/realm.rs:612,952,1043` — three copy-pasted 2s/10ms busy-wait loops,
  no shared budget.
- `meerkat-session/src/ephemeral.rs:45` — `EVENT_CHANNEL_CAPACITY = 256` for both mpsc
  and broadcast; `Lagged` timing failure indistinguishable from network stall.

**Severity.** **Medium.** User-visible flakiness, not data loss.

**Enforcement.** Central `TimingProfile` consumed at every `.timeout()`; lint rejects
literal `Duration::from_*` outside `timing.rs`. DSL gains `deadline_within` annotation
so TLC can express "WaitingForLlm ≤ 300s ⇒ eventual transition."

---

## 4. Serialization skew / schema drift

**Def.** Persisted shape changes but a consumer reads the old format. Machine state
looks right in memory; projector silently corrupts.

**Why catalog misses it.** Catalog enforces *current* schema; no model of prior-schema
payloads in flight.

**Sample sites.**
- `meerkat-session/src/event_store.rs:25` — `EVENT_SCHEMA_VERSION: u32 = 1`; never
  bumped; no v0→v1 test. A silent bump breaks every `.rkat/` session.
- `meerkat-mob/src/event.rs:218,529` — mob event v6 with `bail!` on mismatch; no
  migration. v5 unrecoverable on upgrade.
- `meerkat-mob/src/event.rs:921` — test writes `version+1` to assert rejection. Policy
  is "refuse unknown," not surfaced to users.
- `meerkat-contracts/src/wire/session.rs:362` — `unwrap_or(Value::Null)` silently
  downgrades malformed tool args to null — indistinguishable from legit "no args."
- `meerkat-rpc/src/handlers/session.rs:61-102` — 15 consecutive `#[serde(default)]`;
  stale clients accepted silently.

**Severity.** **High.** Data loss + silent-default on every wire.

**Enforcement.** Extend `schema-freshness` CI to every versioned struct. Every
`#[serde(default)]` must reference a default-const with justification doc. Per-
envelope proptest fuzzes v0..=vN payloads and asserts accept-and-upgrade-or-reject.

---

## 5. Backpressure and memory pressure

**Def.** Unbounded channels, unbounded Vecs, slow broadcast consumers. Transitions
succeed; process OOMs.

**Sample sites.**
- `meerkat-rest/src/lib.rs:2362,5771` + `meerkat-rpc/src/session_runtime.rs:3741` —
  three `mpsc::unbounded_channel()` in production paths.
- `meerkat-session/src/ephemeral.rs:1487` — broadcast(256); `Lagged` logged + continued
  at 2568, subscriber events dropped silently.
- `meerkat-session/src/ephemeral.rs:2316,2343` — unbounded `blocks.extend`; compaction
  is only backpressure and it's optional.
- `meerkat/src/service_factory.rs:778-1233` — 13 `mpsc::channel(8)` with immediate
  `_rx` drop (test-harness pattern; would deadlock in prod).

**Severity.** **High** for persistent surfaces.

**Enforcement.** Clippy `disallowed_methods` bans `unbounded_channel`; every
`channel(N)` must cite a `Capacity` const with rationale. `cargo xtask audit-queues`
enumerates every channel + documented backpressure strategy.

---

## 6. External state drift

**Def.** Machine says session is alive; provider evicted it (OAuth, quota,
response-store eviction). Internal invariants hold; world's don't.

**Why catalog misses it.** External drift unobservable until next call.

**Sample sites.**
- `meerkat-anthropic/src/client.rs:17` — pool idle 90s; `reqwest` doesn't notify the
  machine when it recycles connection; next request silently reconnects.
- `meerkat-core/src/error.rs:327-332,433` — `retry_after` parsed but no explicit
  `WaitingForRetryAfter` state; delay hidden in retry layer.
- `meerkat-store/src/realm.rs:89` — 30s stale; a paused remote loses lease without
  local reflection until refresh.
- `meerkat-openai/src/live.rs:2069` — `conversation_id: None` — future OpenAI changes
  silently start fresh context.

**Severity.** **Medium.** Reproducible on long-idle + provider rotation.

**Enforcement.** Every external-state transition declares
`liveness_probe() -> Result<Fresh, Stale>`; `Stale` is an explicit variant, not an error.

---

## 7. Security boundary holes

**Def.** Elided authz, `pub` on internals, TOCTOU between verify and trust lookup.

**Sample sites.**
- `meerkat-comms/src/io_task.rs:39,63` — `require_peer_auth: bool` is a parameter;
  caller must remember `true`. No compile-time guarantee prod path sets it.
- `meerkat-comms/src/inbox.rs:483 vs 505` — TOCTOU: 483 reads `trusted_peers` under
  queue lock; 505 re-reads separately. Trust revoked in between ⇒ enqueue-accept /
  dequeue-reject split.
- `meerkat-comms/src/inproc.rs:108+` — 17 `pub fn` on global `InprocRegistry`; any
  caller can `register_with_meta_in_namespace` impersonating another pubkey.
- `meerkat-contracts/src/wire/session.rs:362` — `unwrap_or(Value::Null)` turns
  adversarial malformed args into a no-args call, possibly bypassing tool validation.
- 6769 `.unwrap()`/`.expect()` outside tests — each a potential remote-DOS vector.

**Severity.** **High** for TOCTOU and unwrap count; **Medium** for ambient `pub`.

**Enforcement.** Promote `require_peer_auth` to type-state. Ban `unwrap`/`expect`
in non-test code via clippy workspace lint; `xtask bulk-fix` converts existing.
Token-gate `InprocRegistry` mutation to surface-construction context.

---

## 8. Byzantine / adversarial inputs passing spec validation

**Def.** Inputs the spec accepts as well-formed that violate a relied-on invariant:
overlong strings, pathological nesting, Unicode confusables, duplicate JSON keys.

**Sample sites.**
- `meerkat-session/src/ephemeral.rs` — `blocks.extend(...)` accepts arbitrary
  block-count per turn; runaway provider bloats memory.
- `meerkat-contracts/src/wire/session.rs:815+` — `WireContentBlock` round-trips
  arbitrary `Value`; no size limit on tool-arg raw values.
- `meerkat-comms/src/io_task.rs` — envelope size only transport-bounded; 1 GB signed
  envelope passes `verify()` before size check.
- Skill/tool/mob-member names — no Unicode normalization; name-keyed lookups vulnerable
  to NFKC/NFC confusables.

**Severity.** **Medium.** High in mob-federated deployments.

**Enforcement.** `BoundedString<MAX>` / `BoundedBytes<MAX>` newtypes in
`meerkat-contracts`; deserialization enforces. Add `xtask fuzz` targets.

---

## 9. Panic paths (transition body panics)

**Def.** Transition panics; `catch_unwind` catches it but machine state is past
pre-state, before post-state. Future transitions read corrupt state.

**Sample sites.**
- `meerkat-mob/src/runtime/actor.rs:3320` — `spawn.catch_unwind()` → `MobError::Internal`,
  but partial mutation inside spawn closure is preserved. No rollback.
- `meerkat-mob/src/runtime/actor_turn_executor.rs:269` — `reconcile.catch_unwind()`;
  same; reconcile may have half-applied to session.
- `meerkat-core/src/model_profile/capabilities.rs:222` +
  `schema_builder.rs:323,344` — 3 `panic!` on missing catalog entries; library-surface
  code, panics on misconfigured runtime.

**Severity.** **Medium.** Rare; hard to reproduce mid-transition.

**Enforcement.** `TransitionGuard` wraps every transition body: snapshot pre-state,
run, on `catch_unwind` *restore* pre-state before propagating error.

---

## 10. Cascading failure without composition spec coverage

**Def.** Composition spec verifies declared routes; failure *propagation* isn't
declared. When X fails, does Y retry, back off, die, or silently void?

**Why catalog misses it.** Verifies routes, not failure exhaustiveness.

**Sample sites.**
- `meerkat-mob/src/runtime/flow_frame_engine.rs:314-337` — `0..=max_retries` then
  `bail!`; no spec for post-bail cascade.
- `meerkat-rpc/src/session_runtime.rs:3741` — unbounded lifecycle-channel drop
  triggers nothing; downstream assumes liveness.
- `meerkat-session/src/ephemeral.rs:2568` — `event_stream_open = false` on sink drop;
  event logged + ignored. No spec line says "lost events on sink drop acceptable."

**Severity.** **Medium.**

**Enforcement.** Extend composition spec: every `Err` arm at a seam declares a
`FailureRoute` (retry / give-up / escalate). TLC verifies the transitive fault graph
has no silent absorbers.

---

## 11. (New) Silent test-mode divergence

Found while scanning: several production code paths have test-only behavior that
changes semantics.

**Sample sites.**
- `meerkat-anthropic/src/client.rs:1483-1484` — test timeouts 5s/120s vs prod 30s/300s;
  tests pass, prod fails at 5s.
- `meerkat/src/service_factory.rs:778-1233` — 13 `_rx` drops; harness exercises a path
  production won't.

**Severity.** **Low-Medium.** Risk = false confidence from green tests.

**Enforcement.** Per-module `#![cfg_attr(test, production_divergence)]` attribute with
doc-rationale; flags reviewer attention.

---

## Overlap with wave-b

| Class | Wave-B coverage |
|---|---|
| 1. Async cancellation races | **None** |
| 2. Resource lifetime | **None** |
| 3. Timing violations | **None** |
| 4. Serialization skew | **Partial** — wave-b tightens wire typing (`ConnectionRef`, `SkillKey`) which reduces parse-silent-fail surface, but doesn't add versioning machinery |
| 5. Backpressure | **None** |
| 6. External state drift | **Partial** — `AuthMachine` per wave-b plan forces auth-lease liveness; doesn't cover provider-session eviction |
| 7. Security boundaries | **Partial** — typed newtypes close accidental-string-mixing; `require_peer_auth` type-state and the 6769 unwraps are untouched |
| 8. Byzantine input | **None** |
| 9. Panic paths | **None** |
| 10. Cascading failure | **None** |
| 11. Test-mode divergence | **None** |

Wave-b is an SOS (State Ownership Sanitization) wave. These classes need Wave-C+.

---

## Top-5 seeds for issue #341 (fault-explicit state machines)

Ranked by production-bug potential × fix tractability:

1. **`meerkat-session/src/ephemeral.rs:2548-2572`** — model the three-way `select!`
   (run/interrupt/event) as explicit states with a `CancellationObligation` on
   `run_fut`. Highest-value single seam: every session cancel goes through here.
2. **`meerkat-mob/src/runtime/actor.rs:3320` + `actor_turn_executor.rs:269`** — pair
   `catch_unwind` with `TransitionGuard::snapshot_and_restore`; teach the mob authority
   the "partial-transition" state.
3. **`meerkat-anthropic/src/client.rs:19` + the 300s/5s/120s cluster** — surface
   `TimingProfile` through config; TLC annotates `WaitingForLlm` with deadline.
4. **`meerkat-rest/src/lib.rs:2362,5771` + `meerkat-rpc/src/session_runtime.rs:3741`** —
   kill the 3 `unbounded_channel` sites; pick `Capacity` constants and document
   overflow policy (drop-oldest / reject / block).
5. **`meerkat-comms/src/io_task.rs:39,63`** — promote `require_peer_auth` from
   parameter to type-state; fixes category 7 TOCTOU + closes the ambient impersonation
   surface on `InprocRegistry`.

These five, executed rigorously, unblock the formal-fault-model pass. Remaining
classes (serialization, byzantine, test-mode) are tractable but lower-leverage for an
initial fault-explicit pass.
