# meerkat-mob lib test cascade — triage (#31)

**Baseline**: `f12d100a6` (integration tip, `origin/dogma/wave-a-demolition`).

**Command**: `RUST_BACKTRACE=1 ./scripts/repo-cargo nextest run -p meerkat-mob --lib --no-fail-fast`

**Result**: `694 tests run: 459 passed, 235 failed, 14 skipped`

This document maps the 235 remaining failures to root causes. **No fixes are landed here**; the document is the decision surface for user-level scope call on whether #31 blocks `#15 D-GATE`.

---

## Bucket counts (by first-visible panic message)

| Count | Class | Bucket label |
|-------|-------|--------------|
| 173 | A | `Internal("actor task dropped")` — downstream symptom, real root in Class A1 |
| 17  | B | `CommsError(PeerNotFound("<test-mob>/..."))` — real-comms harness |
| 8   | F | projection/serialization snapshot shape mismatch |
| 7   | D | `WiringError("invalid peer_id: ...: ed25519:...")` — test fixture feeds ed25519 strings |
| 5   | C | `AdmissionDropped { reason: UntrustedSender }` — contracts tests |
| 2   | E | `MemberAlreadyExists` / `MemberNotFound` matcher — same actor-death symptom as A |
| 1   | G | `WiringError("scope-deferred")` fires from current-contract wire — bubbles downstream |
| 24  | Z | other (each singleton, most are `.expect(...)` messages wrapping Class A underneath) |

**237 unique panic lines across 235 tests** (two tests trip two panics). Tests are counted once in the "Bucket counts" table above.

## Class A1 — `flush_routed_effects` terminates actor on every spawn (ROOT ROOT CAUSE of ~190 / 235)

Every Class A, E, G, Z failure that surfaces `Internal("actor task dropped")` collapses to one root cause: the mob actor's `run` loop calls `flush_routed_effects` after each command handler, and the first `PrepareBindings` routed effect dispatched through the `Wired` composition binding fails because the `MeerkatConsumerSurface` can't parse the projected `agent_runtime_id` field as a `SessionId` UUID.

### Evidence

Single-test reproduction with in-place panic logging on the `flush_routed_effects` error branch:

```
!!! ACTOR CMD: Discriminant(0)     # MobCommand::Spawn for l-1
!!! ACTOR CMD: Discriminant(1)     # MobCommand::SpawnProvisioned completion
!!! ACTOR EXIT via flush_routed_effects error:
    internal error: consumer `meerkat` refused routed input `PrepareBindings`:
    routed agent_runtime_id `l-1:0` is not a valid session UUID:
    invalid character: expected an optional prefix of `urn:uuid:`
    followed by [0-9a-fA-F-], found `l` at 1
!!! ACTOR RUN() RETURNED Ok
```

After `l-1` spawn completes, the routed `PrepareBindings` effect fires; the consumer surface rejects; actor `run` returns; the `command_rx` sender-side lives on in the handle but nobody is draining — next `handle.spawn(w-1)` sees `command_tx.send(...).await` fail with `Err(SendError)`, surfaced as `Internal("actor task dropped")` (`meerkat-mob/src/runtime/handle.rs:927`).

### Structural trace

1. `MobHandle::spawn(..).await` sends `MobCommand::Spawn` into the actor's mpsc channel.
2. Actor handles Spawn, updates DSL authority via `MobMachineMutator::apply(...)`, pushes any routed effects into `self.pending_routed_effects` via `queue_routed_effects_from(...)` (`meerkat-mob/src/runtime/actor.rs:2044-2054`).
3. At the bottom of the loop body (`actor.rs:2749-2768`), actor calls `self.flush_routed_effects().await`.
4. `flush_routed_effects` (`actor.rs:769-787`) drains each queued `MobSeamEffect` through `super::composition::dispatch_routed_effect(&self.composition_binding, effect)`.
5. In `create_test_mob`, `MockSessionService::enable_runtime_adapter()` is called, which means `MobBuilder::with_session_service` seeds `self.runtime_adapter = Some(...)` (`builder.rs:220-224`). The `composition_binding` branch at `builder.rs:1428-1432` then picks `CompositionBinding::Wired(...)` via `wired_binding_from_runtime_adapter`.
6. `dispatch_routed_effect` (`composition.rs:318-335`) routes the `MobSeamEffect::PrepareBindings { agent_runtime_id: "l-1:0", fence_token, generation }` to the `CatalogCompositionDispatcher`, which invokes `MeerkatConsumerSurface::apply_routed_input`.
7. `MeerkatConsumerSurface::apply_routed_input` (`meerkat-runtime/src/meerkat_machine/composition.rs:179-227`) calls `self.resolve_session(&projected)`.
8. `resolve_session` (`composition.rs:103-129`) sees `pinned_session: None` and a projected `agent_runtime_id = "l-1:0"`; hits the `(None, Some(rt)) => SessionId::parse(&rt)` arm; `SessionId::parse` expects UUID or `urn:uuid:<uuid>`; rejects.
9. Rejection returns `Err(String)` back through the dispatcher as `DispatchRefusal::ConsumerRefused { reason }`, which `dispatch_refusal_to_mob_error` (`composition.rs:337-395`) lifts to `MobError::Internal(...)`.
10. `flush_routed_effects` propagates that `Err`. The actor's `run` loop at `actor.rs:2760-2767` terminates the task via bare `return;`.
11. `command_rx` drops. Next `handle.spawn(w-1)` fails at `command_tx.send().await.map_err(|_| Internal("actor task dropped"))`.

### The type mismatch

- **Producer side**: `MobMachine` DSL projects `agent_runtime_id` as `mob_dsl::AgentRuntimeId(String)` with the canonical display form `"{identity}:{generation}"` (`meerkat-mob/src/ids.rs:288-292`; shell domain `AgentRuntimeId::Display` is `"worker:0"` style).
- **Consumer side**: `MeerkatConsumerSurface::resolve_session` treats the projected field as a session UUID and parses via `SessionId::parse` (`meerkat-runtime/src/meerkat_machine/composition.rs:120-122`).
- These are two different identifier spaces: mob's `AgentRuntimeId` is `"identity:generation"`; `SessionId` is a UUID. The routed binding between them is broken by design on this code path — the mob produces the former, the consumer expects the latter, and there's no translation layer.

### Origin commit

`a52571dd1` (2026-04-23) "wave-c C-6c: consumer surface + dispatcher wiring + handle annotations" introduced:
- `MeerkatConsumerSurface` with `SessionId::parse(rt)` on `agent_runtime_id` (`meerkat-runtime/src/meerkat_machine/composition.rs`).
- `wired_binding_from_runtime_adapter` in mob builder path (`meerkat-mob/src/runtime/composition.rs`).

Prior to `a52571dd1`, the composition binding path was `Standalone` for test harness, which made `dispatch_routed_effect` return `Ok(None)` via the early-exit at `composition.rs:322-324` — no dispatch, no consumer parse, no actor exit. C-6c wiring on the same test code path closed the spine (good for production) but exposed the `AgentRuntimeId` vs `SessionId` type mismatch.

### Why it wasn't caught at C-6c merge

The C-6c merge test-coverage was at the unit level on `MeerkatConsumerSurface` (with `pinned_session: Some(sid)` — the pinned-session arm of `resolve_session`, which bypasses the UUID parse). Every actor-level mob spawn test uses `pinned_session: None` and sends `agent_runtime_id = "identity:generation"` — that combination is never tested in `meerkat-runtime/src/meerkat_machine/composition.rs:230-303`.

### Fix shape

Two possible shapes; user scope call required:

**Shape 1 — pin consumer session per mob member (minimal)**: The mob actor holds one `MeerkatMachine` per bridge session, each with a pinned session id. When constructing `MeerkatConsumerSurface` for a given member's routed effects, use `MeerkatConsumerSurface::with_pinned_session(session_id)` (already exists at `composition.rs:93-101`) so `resolve_session` takes the `(Some(pinned), _) => Ok(pinned.clone())` branch regardless of whether the projected field is a UUID or human-readable.

This requires either (a) per-effect `MeerkatConsumerSurface` construction with the member's session id, or (b) teaching the consumer surface to pin-on-first-use from the projected `agent_runtime_id` via a lookup map.

**Shape 2 — make `resolve_session` accept shell `AgentRuntimeId` format**: Change `MeerkatConsumerSurface::resolve_session` to treat `agent_runtime_id` as `"identity:generation"` and look up the bound session via the `MeerkatMachine` state (`member_session_bindings` map from W3-H γ). If no binding exists yet, defer the routed input rather than reject.

**Scope estimate**: design decision — not mechanical. Both shapes require deciding who owns the projection: the mob's producer (must emit session UUID) vs the meerkat consumer (must accept runtime-id shape). Each has architectural implications for how the `meerkat_mob_seam` composition is wired.

### Commit message preamble

```
fix(mob): translate routed agent_runtime_id to pinned session at consumer surface (#31 Class A1)

The mob MobMachine projects `agent_runtime_id` as the canonical display
form `"identity:generation"` (meerkat-mob/src/ids.rs:288-292). The
`MeerkatConsumerSurface::resolve_session` at
meerkat-runtime/src/meerkat_machine/composition.rs:120-122 parses the
projected field as a `SessionId` UUID when no session is pinned — which
is always the case in mob routing — and every `PrepareBindings` routed
effect is refused. The mob actor terminates on the first refusal via
`flush_routed_effects` error propagation at
meerkat-mob/src/runtime/actor.rs:2760-2767, which cascades to all
downstream tests as `Internal("actor task dropped")`.

Introduced by `a52571dd1` (wave-c C-6c). Coverage gap: C-6c unit tests
all use `pinned_session: Some(...)`, which takes the bypass arm.
```

### Tests unblocked by fixing A1

All 173 Class A failures plus 2 Class E, 1 Class G, and ~19 Class Z singletons that wrap `"actor task dropped"` inside `.expect(...)` messages — **~195 / 235 tests**.

---

## Class B — real-comms `PeerNotFound` (17 tests)

### Representative

```
thread 'runtime::tests::test_external_spawn_with_binding_uses_real_identity' panicked at
    meerkat-mob/src/runtime/tests.rs:18450:54:
    spawn with binding: CommsError(PeerNotFound(
        "test-mob-external-spawn-real-identity-<uuid>/worker/w-real"
    ))
```

### Observed message variants

All 17 failures match the same shape: `CommsError(PeerNotFound("test-mob-<test-name>-<random-uuid>/<role>/<id>"))`. Every test in the bucket builds a "real comms" test harness (`create_test_mob_with_real_comms` / `_with_runtime_backed_real_comms` — `tests.rs:16058`, `16397`) that spawns an external backend member whose `TrustedPeerDescriptor` has a peer name derived from the test-unique mob-id.

### Fix shape hypothesis

These tests use a real `CoreCommsRuntime` with a trust-addressed transport. The `PeerNotFound` error surfaces from the comms transport when a peer_name is not found in the local peer directory at send time. Two possible causes:

- **B.1 (most likely)** — These tests were passing before C-6c because the producer-side path didn't attempt to route through the meerkat consumer, so spawn fired trust registration; with C-6c wired the spawn's routed effect fires first, actor terminates before trust registration, comms directory never gets populated, peer not found at send. In other words: **B collapses into A1**; fixing A1 likely clears most of these.
- **B.2 (backup)** — The `peer_name` encoding for external members changed shape between producer and consumer after `a52571dd1`, and these tests assert the producer-side encoding. Needs verification after A1 is fixed.

### Scope estimate

Run after A1 is fixed; the remaining failures (if any) are narrow and likely 1-2 site fixes in external-backend test harness.

### Commit message preamble

```
fix(mob): [follow-up to A1 — likely zero code change]

Confirmed after A1 lands that external-backend `PeerNotFound` failures
resolve automatically: the `actor task dropped` cascade prevented
trust registration for the external peer, which surfaced downstream
as "peer not found" at first send. With the actor surviving the
routed PrepareBindings dispatch, trust registration lands and the
comms directory contains the expected external peer.
```

---

## Class C — `AdmissionDropped { reason: UntrustedSender }` (5 contract tests)

### Representative

```
thread 'tests::contracts::contract_mob_002_peer_request_response_round_trip' panicked at
    meerkat-mob/src/tests/contracts.rs:75:10:
    PeerRequest send should succeed: AdmissionDropped { reason: UntrustedSender }
```

### Trace

Every contract test in this bucket exercises `CoreCommsRuntime::send(sender, PeerRequest { to: receiver, ... })` after both directions of trust are added via `add_trusted_peer`. The send is dropped at admission because the sender's identity, as seen by the receiver, is considered untrusted at send time.

### Root cause hypothesis

Likely same class as #24 `PubKey::to_peer_id()` UUID-mismatch: `add_trusted_peer` stores a canonical peer-identity record on the receiver side. When `PeerRequest` arrives, the receiver cross-references the envelope's sender-claimed identity against its trusted-peer directory. If `PeerId` shape drifted (UUID vs `ed25519:` string) between trust-add and send-check sites, the lookup misses → `UntrustedSender`.

### Scope estimate

Unknown — requires running one contract test through `RUST_BACKTRACE=full` with tracing enabled on the admission path. Could be 1-site (lookup key derivation) or deeper (trust-record identity shape). Dependent on whether this interacts with A1.

### Commit message preamble

```
fix(mob/comms): align sender identity derivation between trust-add and admission check (#31 Class C)

Contract tests in tests::contracts trip `AdmissionDropped {
reason: UntrustedSender }` after both sides call `add_trusted_peer`
with matching specs. The admission path on the receiver derives the
envelope sender's `PeerId` via <TBD method>, while `add_trusted_peer`
stores the trusted-sender `PeerId` via <TBD method>. The two do not
agree after #24 changed `PubKey::to_peer_id` to emit typed UUIDs.
Align the two derivations on the same site.
```

---

## Class D — ed25519 test-fixture peer_id rejected (7 tests)

### Representative

```
thread 'runtime::provisioner::tests::validated_external_peer_spec_preserves_the_validated_peer_name'
    panicked at meerkat-mob/src/runtime/provisioner.rs:498:10:
    external peer spec should validate:
    WiringError("invalid external peer spec for 'mob/worker/member-1':
        invalid peer_id: invalid peer id \"ed25519:member-1\":
        invalid character: expected an optional prefix of `urn:uuid:`
        followed by [0-9a-fA-F-], found `:` at 8")
```

### Trace

Test fixtures call `TrustedPeerDescriptor::test_only_unsigned(name, "ed25519:member-1", addr)`. Post-#24, `PeerId::parse` only accepts UUID format, so the `peer_id` argument must be a UUID string. The `test_only_unsigned` constructor doesn't translate — it passes through — and validation in `TrustedPeerDescriptor` rejects.

### Relationship to #28

**This is exactly the D-obs-audit class #28 was opened to catalog.** Team-lead's note on the `test_only_unsigned_typed` helper proposal (message sent as part of #25 close) explicitly scoped this to #28 with the recommendation to migrate all `test_only_unsigned` call sites to a typed variant that takes a `PeerId` directly rather than a string.

### Fix shape (from #28 proposal)

Add `TrustedPeerDescriptor::test_only_unsigned_typed(name, peer_id: PeerId, addr)` lib-side. Migrate the 7 test sites to use the typed form with `PeerId::new_random()` or a canonical test UUID.

### Scope estimate

1 lib-code commit (adding typed helper in `meerkat-core/src/comms.rs`) + 1 test-migration commit touching 7 sites. Mechanical after the helper exists.

### Commit message preamble

```
feat(comms): add TrustedPeerDescriptor::test_only_unsigned_typed (#28 / #31 Class D)

Post-#24, `PeerId` is typed-UUID and `PeerId::parse` rejects the
legacy `ed25519:` string form. The existing
`test_only_unsigned(name, peer_id_str, addr)` helper preserves the
stringly-typed signature, which pushes string-form peer ids into
test fixtures that round-trip through parse and fail at runtime.

Add a typed variant that takes a `PeerId` directly, avoiding the
parse round-trip. Migrate the remaining 7 `ed25519:`-string call
sites to use `PeerId::new_random()` or a canonical test UUID.
```

---

## Class E — MemberAlreadyExists / MemberNotFound matcher (2 tests)

### Tests

- `runtime::tests::test_spawn_duplicate_meerkat_id_fails` — expects `Err(MobError::MemberAlreadyExists(_))` on second `spawn(same_id)`
- `runtime::tests::test_wire_unknown_meerkat_fails` — expects `Err(MobError::MemberNotFound(_))` on wire to unknown id

### Trace

Both fail on `matches!(result, Err(<specific variant>))`. The actual `result` is `Err(MobError::Internal("actor task dropped"))` from Class A1 — actor died on first spawn, second spawn / wire can't check uniqueness or existence because there's no actor to check.

### Scope

**Collapses into A1.** Zero code change for this class after A1 lands.

---

## Class F — projection/serialization snapshot shape (8 tests)

### Representative

```
thread 'runtime::handle::tests::helper_result_serializes_identity_native_runtime_fields'
    panicked at meerkat-mob/src/runtime/handle.rs:3640:9:
    assertion `left == right` failed

thread 'runtime::handle::tests::member_projection_types_omit_bridge_session_fields_in_serialized_output'
    panicked at meerkat-mob/src/runtime/handle.rs:3544:9:
    assertion failed: !snapshot_value["agent_runtime_id"].is_null()
```

### Trace

These are pure serialization shape tests — they build domain structs (no mob runtime), `serde_json::to_value(&value)`, and assert field layout. The assertions want:
- `agent_runtime_id` field present in serialized JSON output
- Specific equality between `receipt_value["agent_runtime_id"]` and `serde_json::to_value(&runtime_id)`
- `bridge_session_id` absent (this passes)

`AgentRuntimeId` at `meerkat-mob/src/ids.rs:263-286` is `struct { identity, generation }` with `#[derive(Serialize, Deserialize)]` — produces JSON `{"identity":"worker","generation":0}`.

The tests assert field layout that matches earlier behavior. The assertions may be passing on the presence side (`!is_null()`) and failing on the equality side — the equality check at `handle.rs:3608` and `3640` is `receipt_value["agent_runtime_id"] == serde_json::to_value(&runtime_id)`. If that fails, the serialized shape on the receipt side diverges from the runtime_id-standalone shape — most likely a serde rename or flatten difference between the `MemberRespawnReceipt`/`HelperResult` struct field and direct `AgentRuntimeId` serialization.

Would need `assert_eq!(left, right)` print-out with `--nocapture` to confirm exact divergence. Sample run didn't show the panic body's formatted-values.

### Scope estimate

Probably small — serde-attribute drift between two struct fields. 1-commit, 8-test-scope.

### Commit message preamble

```
fix(mob): align receipt/result serde shapes with canonical AgentRuntimeId (#31 Class F)

Serialization shape tests in handle.rs expect
`receipt_value["agent_runtime_id"]` to equal
`serde_json::to_value(&runtime_id)` byte-for-byte. The struct field
currently serializes under a different shape (<TBD: rename or flatten
attribute>). Align the field's serde annotations with the canonical
`AgentRuntimeId::Serialize` shape so the two sites round-trip
identically.
```

---

## Class G — `WiringError("scope-deferred")` from current-contract wire (1 test)

### Test

`runtime::tests::test_unwire_prunes_stale_local_trust_when_projection_is_already_absent`

### Trace

Test constructs a scenario where a *local* edge's local-side projection is already absent, and expects `unwire` to still prune stale comms trust. The failure message is `WiringError("external peer wiring requires supervisor-owned trust authority; scope-deferred beyond Wave D ...")`. That's the `PeerTarget::External(_)` rejection from `MobActor::handle_unwire` at `actor.rs:4248-4251`.

Which means the test harness is accidentally constructing `PeerTarget::External` instead of `PeerTarget::Local` somewhere. Likely test-data shape issue rather than semantic — a fixture built after the External rejection was added.

### Scope estimate

1 test-site fix, trivial.

---

## Class Z — singletons (24 tests)

The remaining 24 failures are each distinct but almost all wrap `Internal("actor task dropped")` inside an `.expect("...")` message that makes them look unique. E.g.:

```
spawn must surface wiring failure after rollback cleanup   → Err is A1
expected PreviousMemberCleanupAmbiguous, got Mob(Internal("actor task dropped"))  → Err is A1
valid trusted peer spec: "unknown peer address transport: https"  → not A1
```

The one that isn't A1-wrapped (`unknown peer address transport: https`) is a different class: test fixture uses `https://...` as peer address, and the comms runtime's address parser rejects unknown transports. Trivial — change the fixture or teach the parser.

### Scope estimate

After A1 lands, Class Z shrinks to ~3-5 actual tests; most will be single-site fixture or assertion fixes.

---

## D-GATE scope disposition (raised to user)

### Absolutely blocking D-GATE

- **Class A1** — it's the single fix that recovers ~195 / 235 failing tests. Without it, mob lib is compile-clean but runtime-unshippable: `cargo nextest run -p meerkat-mob --lib` reports 33% test failure. A release pipeline that passes the D-GATE compile axis while failing 235 tests is strictly worse than the pre-D-GATE compile-broken baseline (which at least honestly surfaces the state).

### Probably blocking D-GATE

- **Class D** (ed25519 peer_id) — 7 tests, already owned by #28, mechanical once typed helper exists. Low cost to land as part of #28 close.
- **Class F** (serde shape) — 8 tests, small scope, serde-attribute drift. Low cost.

### Possibly deferrable to post-Wave-D

- **Class B** (real-comms PeerNotFound) — may collapse into A1. Confirm after A1 lands.
- **Class C** (UntrustedSender contract tests) — 5 tests, unknown depth. If it's #24-class drift at a trust-check site, small fix; if deeper comms identity architecture, larger.
- **Class G** / **Z** singletons — trivial cleanup, can go with any of the above or after.

### My recommendation

D-GATE should block on A1 (which cascade-fixes most of the tree) and D (cheap #28 close). After those land, re-run nextest; if the tree is green or near-green, close D-GATE.

If A1 turns out to require architectural design (Shape 1 vs Shape 2), that is a genuine Wave-D-to-post-Wave-D scope conversation — but deferring 235 test failures silently is the wrong failure mode.

---

## Method notes

- All bucket counts derived from `/tmp/mob-nextest-full.log`, a full `--no-fail-fast` run at `f12d100a6` with `RUST_BACKTRACE=1`.
- Root-cause for Class A1 surfaced by in-place panic logging on `flush_routed_effects` error branch + actor startup panic/drop guard. Instrumentation removed before this document was committed.
- `git log -S` pointers for `a52571dd1` confirmed `MeerkatConsumerSurface`, `resolve_session`, `wired_binding_from_runtime_adapter`, and `is not a valid session UUID` were all introduced in that one commit.
- Contract tests (Class C) traced only by inspection, not RUST_BACKTRACE=full run — marked unknown-depth.
- No Wave-A `git log -S` root cited for A1 because A1 is a **wave-c** introduction (`a52571dd1` is post-Wave-A), not a Wave-A delete-without-restore. The #30 pattern doesn't fit A1 — it's a different failure mode: new-infrastructure-land with a coverage gap.
