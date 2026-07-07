---
title: "Multi-Host Mobs Plan"
description: "Architecture and implementation plan for distributed multi-host mobs: identity-routed comms, mob-owned placement, cross-host observation, scoped control."
icon: "diagram-project"
---

# Multi-Host Mobs — Architecture & Implementation Plan (v4)

> Status: **adjudicated v4** (2026-07-07, at HEAD 0.7.22 / da467fca8). Every substrate claim below was re-verified against that HEAD (file:line anchors are from it).
> This document is the merged, authoritative plan. It supersedes the v1 sketch (2026-06-03), the v2.1 adversarial adjudication (2026-06-15), and the pasted "RMAT-aware extension plan" draft; the v3 "leaderless peer fabric" concept is a **separate track**, not this plan (see Adjudication history).
> Companion doctrine: `docs/architecture/meerkat-dogma.md`. Machine registry: `canonical_machine_schemas()` in `meerkat-machine-schema/src/catalog/mod.rs`.

## 1. Goal

Distributed mobs whose members run on multiple hosts with **the same UX as local**:

- Agents communicate by identity (`AgentIdentity`), never knowing whether a peer is local or remote.
- Users observe and control every member — including remote and remotely-spawned members — through the same console/API surfaces: list, status, history, live events, send, cancel, retire, wire.
- Mob membership/topology stays `MobMachine` authority. Comms delivery stays `meerkat-comms` authority. Session execution/history stays runtime/session authority. MobKit stays a projection layer.

Example target topology:

```text
Host A: A            (controlling host: runs the MobActor / MobMachine)
Host B: B1, B2, later B21, B22   (B2 spawns B21/B22 onto Host B)
Host C: C1, C2, C3

Edges:  A<->B1  A<->C1  A<->C2  B1<->B2  C1<->C2  C2<->C3
```

`send_message(B1)` works identically from A (cross-host) and from B2 (same host). The console, attached to Host A, sees and controls all eight identities.

## 2. Adjudication history (do not re-litigate silently)

| Round | Date | Outcome |
|---|---|---|
| v1 review | 2026-06-03 | Direction right; 5 proposed primitives duplicated machine-owned facts (`topology_epoch`, fences, roster); ~60-70% substrate exists; blocker = cross-host event/history. |
| v2.1 | 2026-06-15 | Adversarial re-map at 0.7.4. Deferred the one-host-listener consolidation (then designed as `HostTransportFrame` two-layer envelope) to v2 because it generated a cluster of majors; chose the shipped per-member external-TCP plane for v1. |
| v3 | 2026-06-16 | Concept fork: leaderless peer fabric (HomeBotNet/ESP32). Dissolves the control/data duality; **separate product track**, not this plan. |
| **v4 (this doc)** | 2026-07-07 | Draft plan corrected and rebuilt. **Deliberately overturns the v2.1 listener deferral** (D1) on new evidence: the identity coupling in the receive path is two seams, not a framing protocol — the deferral priced a design we are not building. Keeps v2.1's other calls (HostId, placement-in-admission, control scopes, event/history as the critical path). |

## 3. Constraints (v1, corrected wording)

- Realms are **not** distributed. A member's sessions/events/credentials persist in the member host's local realm. Cross-host reads are bridge-served projections, never remote store handles.
- One process hosts many members; **one TCP ingress port per host process** (not per agent, not per member).
- **No new authority owners.** No parallel roster, no second topology counter, no host-local fence rotation, no comms-owned route authority. Every new semantic fact folds into an existing machine or a catalog-generated scoped authority.
- **Protocols are separated; transport is shared.** Peer data (`MessageKind`) and supervisor control (`BridgeCommand` over the `supervisor.bridge` intent) already share the comms envelope, listener, and inbox — that stays. Separation is at the protocol/authority level: typed payloads, distinct admission authorities, one ingress. (The draft's "do not merge the planes" wording is retired; read literally it contradicts shipped reality — `meerkat-runtime/src/comms_drain.rs:1687` dispatches bridge commands out of the ordinary drain.)
- The **peer envelope does not change** in v1 (see D5-adjacent rationale in §7.1).
- MobKit consumes Meerkat surfaces; it grows no roster, directory, or authority.

## 4. Decisions

**D1 — Data plane: one host acceptor, demux on `envelope.to`.**
A host process runs one `TcpTransportListener`; inbound envelopes are routed to the addressed member by the existing `to: PubKey` field. Evidence that this is small: there is no connection-level identity handshake — the only identity coupling in the receive path is the per-envelope gate `envelope.to != keypair.public_key()` (`meerkat-comms/src/io_task.rs:70`) and the single `inbox_sender` parameter of `handle_connection` (io_task.rs:34-56). The acceptor replaces that pair with a registry lookup `PubKey → (member keypair for acks, member inbox)`. No new frame, no codec change, no envelope version. The v2.1 deferral priced the `HostTransportFrame` design (two-layer envelope, second routing owner); the demux design has neither. The registry is a **host-local projection installed and removed only by machine effects** (materialize/retire), never self-populated. `require_peer_auth` is unconditionally on for the acceptor — cross-host ingress without signature+trust verification is unrepresentable. The existing per-listener first-byte `{` sniff that branches into the JSON pairing handshake (`comms_runtime.rs:3195-3238`) is kept, but pairing binds the **host identity** (§7.2), not a member. Standalone external members (`rkat run --comms-listen-tcp`) remain supported as the degenerate one-identity acceptor. Outbound is unchanged (router dials per send; connection reuse is a v2 optimization, noted in §13).

**D2 — Observation: proxy-and-merge through the controlling host.**
The console has one endpoint (the controlling host's RPC/REST). The controlling host reads remote members' history/events over the supervisor bridge and merges them into the existing mob stream. Controlling→member-host connectivity exists by construction (the bridge already commands remote members). Direct observer→member-host streams are a v2 optimization. The v3 fabric answers this differently for its own product; that fork stays separate.

**D3 — Two auth planes, split; neither lands in AuthMachine.**
AuthMachine's charter is per-binding provider-credential lease lifecycle (`catalog/dsl/auth_machine.rs:1-26`; `meerkat-runtime/src/handles/auth_lease.rs:1-16` records the prior deliberate rejection of cross-domain absorption). Mob control authorization is a foreign fact there. The repo itself names the missing layer: `meerkat-rpc/src/secure_rpc.rs:3-4` — transport policy "does not decide who may perform RPC actions once connected; that belongs to auth/grants." So:
- **Plane (a), host↔mob:** a bind ceremony mirroring `BindMember` (`BridgeBindPayload`, `meerkat-contracts/src/wire/supervisor_bridge.rs:758-766`): one-time bootstrap token → durable host authority record fenced by the supervisor authority epoch. Possession-proof + Ed25519 identity; all-or-nothing per host. Comms bootstrap tokens are never principal auth.
- **Plane (b), principal→mob:** typed `ControlScope` grants owned by MobMachine (grant lifecycle = machine facts) and enforced through a **sealed resolved policy** at the controlling host's command dispatch — the `ToolExecutionPolicy`/`ExecutionPolicyGatedDispatcher` pattern (`meerkat-core/src/tool_execution_policy.rs`), including its fail-closed `UnresolvedInherit` discipline. Bridge commands are only ever sent by the already-authorized supervisor, so plane (b) enforcement lives entirely on the controlling host (MobCommand admission + operator surfaces), not on member hosts.

**D4 — Route install: obligations, not epoch re-timing.**
`topology_epoch` keeps advancing at membership/wiring **commit**, exactly as shipped (`mob_machine.rs:3354`, and in `WireMembersRunning` at :5783-5794). Cross-host install convergence is tracked as machine obligations generalizing `pending_recipient_trust: Set<PeerId>` (mob_machine.rs:159, with the existing `RecordPendingRecipientTrust`/`ResolvePendingRecipientTrust`/`RollbackPendingRecipientTrust` input trio at :789-791). Fail-closed is preserved by the receiver: an envelope from a peer whose trust entry is not (or no longer) installed is classified untrusted and rejected — delivery gating already works this way, per-member, with zero envelope support. "RouteInstallIncomplete" is a typed projection of outstanding obligations, not a new owner. Retry probes durably recorded state before trusting it — the pending-rotation precedent (`SupervisorPendingRotationRecord.accepted_peer_ids`, `meerkat-mob/src/store/mod.rs:165-178`).

**D5 — Event/history protocol: durable-cursor pages served from the owning host's EventStore; long-poll is the single primitive.**
Live session streams are tail-only broadcasts with no cursor resume (three unrelated sequence domains exist today: session-task counter, per-RPC-stream counter, durable store seq), and `session/history` is a snapshot read. The durable substrate already exists on the owning host: `EventStore` preserves full envelope identity with a store-assigned, fsync-gated `seq` and `read_from(session_id, from_seq)` (`meerkat-session/src/event_store.rs:120-186`). Therefore: remote reads are **pages** — `ReadMemberHistory` (transcript pages) and `PollMemberEvents` (event pages with optional bounded long-poll wait, the `CompletionFeed { watermark, list_since, wait_for_advance }` contract shape). There are **no** stream-open/close commands in v1: "streaming" is the controlling host running a poll pump per remote member; one canonical path, no push/poll divergence. Full replay parity requires persistent (event-store-backed) sessions on member hosts; ephemeral member hosts degrade to a bounded live buffer with typed `StaleCursor` on overrun — declared as a typed capability at bind, never a silent difference.

**D6 — The member host is a composition role, not a new runtime type.**
`MeerkatMachine` already is the per-process multi-session runtime (one instance hosts all local mob member sessions; `prepare_bindings` / `prepare_local_session_bindings`, `meerkat-runtime/src/meerkat_machine/runtime_control.rs:1653-1683`). The member host = MeerkatMachine + host comms acceptor (D1) + host bridge responder (§7.2-7.3) + registration client, packaged as a daemon subcommand `rkat mob host`. The background-driver template is `spawn_schedule_host` (`meerkat/src/surface/schedule_host.rs:943-983`). `RuntimeHostInfo` remains the wire projection of the process — and MUST stay anti-authority: `runtime_host_info_does_not_claim_topology_authority` (`meerkat-contracts/src/wire/host.rs:181-210`) pins that no authority vocabulary lands on it. Do not mint `MobHostRuntime`/`AgentHost` types.

## 5. Authority model

One row per semantic fact. "Projection" means rebuildable, never consulted for semantic decisions.

| Fact | Owner | Notes |
|---|---|---|
| Mob membership, member lifecycle | `MobMachine` (roster, spawn-exec ladder, kickoff, revival) | unchanged |
| Topology edges + freshness | `MobMachine.wiring_edges` + `topology_epoch` | unchanged; epoch bumps at commit |
| Runtime binding freshness | `MobMachine.runtime_fence_tokens` / `identity_runtime_fence_tokens` + `Generation` | unchanged; no host-local fence rotation |
| **Participating hosts** (new) | `MobMachine`: host set, keys, endpoints, authority epochs, bind phase | §6.1 |
| **Placement** (new) | `MobMachine`: `member_placement` + spawn-exec ladder rung | §6.1; decided inside the existing spawn admission path |
| **Materialization state** (new) | `MobMachine` (controlling side) + `MobHostBindingAuthority` (host side, catalog-generated scoped authority) | §6.3 |
| Identity ↔ peer key ↔ endpoint | `MobMachine` roster (`member_peer_endpoints`, `RegisterMemberPeer`) | pubkey returns in the materialize ack; private keys never cross hosts |
| **Cross-host wiring install** (new) | `MobMachine` obligations (generalize `pending_recipient_trust`) | D4 |
| Peer delivery | `meerkat-comms` (`Envelope`, TrustStore-as-route, receiver-side trust gating) | envelope unchanged in v1 |
| Supervisor/member control | `BridgeCommand` over comms `supervisor.bridge` intent; machine-adjudicated admission both sides | extended, §7 |
| Session execution/history/events | member host's runtime + session service + EventStore | bridge serves projections |
| **Remote event subscription** (new outcome) | `MobMachine` — third outcome on the existing `AuthorizeAgentEventSubscription`/`RejectAgentEventSubscription` seam (mob_machine.rs:1323-1324) | §7.4 |
| **Control scopes** (new) | `MobMachine` grant facts + sealed `ResolvedControlPolicy` at dispatch | D3(b), §8 |
| Reachability | **projection only** (v1), computed observer-locally on the controlling host | §7.5; lift into the machine only when a machine decision depends on it |

Phantom-authority note: `PeerDirectoryReachabilityAuthority` (named as owner in the prior draft) does not exist — zero hits at HEAD. Reachability is new work, and it is a projection.

## 6. Machine deltas (catalog first — Rule: no production-only semantics)

All DSL changes land in `meerkat-machine-schema/src/catalog/dsl/` first, then `make machine-codegen` / `machine-check-drift` / `machine-verify`, schema+alphabet parity, and effect dispositions for `seam-inventory`. TLC bounds for the new state must include ≥2 hosts × 3 members.

### 6.1 MobMachine — hosts and placement

New state (field-driven, per the DSL design principle — no phase enums):

```text
mob_hosts:                 Set<HostId>
host_public_keys:          Map<HostId, PeerSigningKey>
host_endpoints:            Map<HostId, PeerAddress>
host_authority_epochs:     Map<HostId, u64>
host_bind_phase:           Map<HostId, HostBindPhase>       // Requested | Bound; absence = unbound
host_capabilities:         Map<HostId, HostCapabilityFlags> // durable_event_replay, autonomous_members, hard_cancel, protocol range
member_placement:          Map<AgentIdentity, HostId>       // ABSENT = controlling host (local). Present = remote.
member_materialization_failures: Map<AgentIdentity, String> // mirrors member_restore_failures
```

`HostId` is a sealed newtype over the host's comms `PeerId` (identity-first; no second id space; sealed ctor validates pubkey derivation). It is distinct from `RuntimeHostInfo.host_id: String`, which stays a wire projection.

New inputs: `BeginHostBind`, `CommitHostBind { host_id, pubkey, endpoint, capabilities, epoch }`, `RevokeHost`, `HostRebound { host_id, epoch }`, `ResolveMemberMaterialization { agent_identity, outcome }`. Spawn-path extension: `ResolveSpawnMemberAdmission` (mob_machine.rs:661-673) gains placement validation (host bound + capability present, typed reject otherwise); `SpawnExecPhase` (:10433, currently `Opened | MembershipCommitted | Activated`, absence = settled) gains **`MaterializePending`** entered only for remote placements: `Opened → MaterializePending → MembershipCommitted → Activated`. `CommitSpawnMembership` for a remote member carries the materialization-ack facts (real member pubkey, endpoint, session binding), so roster truth is never published before the owning host has confirmed.

New effects (each with an explicit disposition for `seam-inventory`, following mob_machine.rs:1459-1477): `RequestHostBind` (routed), `RequestMemberMaterialization { agent_identity, generation, fence_token, spec_digest, host }` (routed), `RequestMemberRelease` (routed), `HostRegistered`/`HostRevoked` (external seam), `RouteInstallRequested { edge, host, a_endpoint, b_endpoint, epoch }` (routed).

New invariants: placement targets a bound host (`member_placement` values ⊆ `mob_hosts` with `host_bind_phase = Bound`); a remote member in `MembershipCommitted+` has a `member_peer_endpoints` entry whose signing key matches the materialization ack; `host_authority_epochs` monotonic per host.

### 6.2 MobMachine — wiring install obligations (D4)

```text
pending_route_installs: Set<RouteInstallObligation>   // { edge: WiringEdge, host: HostId }
```

Inputs mirror the recipient-trust trio: `RecordRouteInstall` (folded into the wire transitions when an edge endpoint is remote), `ResolveRouteInstall { edge, host }`, `RollbackRouteInstall`. The existing plural wiring inputs (`WireMembers`, `WireMembersWithTrust`, `UnwireMembers`, `WireExternalPeer` — mob_machine.rs:763-775; note: the singular `WireMember`/`UnwireMember` names are bridge commands, not DSL inputs) stay the only topology mutators; `topology_epoch` semantics unchanged. Edge usability is enforced by the receiver (uninstalled trust ⇒ untrusted ⇒ rejected), so a pending obligation is fail-closed by construction; the obligation set makes convergence observable, retryable, and reportable (`RouteInstallIncomplete` projection = outstanding set). Unwire produces symmetric removal obligations. Transport selection per edge is planner work in the wiring planning helper: both-endpoints-same-host ⇒ that host's inproc; cross-host ⇒ TCP to the host acceptor address. `handle_wire_members_batch`'s local-only rejection (actor.rs:11867) is removed as part of this.

### 6.3 MobHostBindingAuthority — new catalog-generated *scoped* authority (host side)

The member host needs machine-adjudicated admission for **host-addressed** commands (bind, materialize, release, trust install, status), and durable memory for restart/dedup. It has no MobMachine (that lives on the controlling host) and MeerkatMachine authorities are per-session, so this is a process-scoped fact set. Precedent: `session_persistence_version_authority` — a catalog DSL source generated into a production crate, registered for schema parity, **not** a canonical machine. `MobHostBindingAuthority` owns:

```text
supervisor binding:     peer_id, epoch, bind phase        (mirrors member-side SupervisorBridgeCommandAdmission facts)
materialized members:   Map<AgentIdentity, { generation, fence_token, session_id }>
dedup memory:           last-materialize tuple per identity (idempotent replay; stale tuple → typed reject)
```

Admission vocabulary reuses the member-side classes and adds fences: `NotBound | StaleSupervisor | SenderMismatch | InvalidBootstrapToken | StaleFence | Unsupported`. Persistence: one SQLite table in the host's realm (shape mirrors `mob_runtime_supervisors`), CAS-capable — this is what makes host restart rebind and materialize-retry dedup durable.

### 6.4 MeerkatMachine — member-side admission extension

Member-addressed bridge command admission is already machine-owned (`ResolveSupervisorBridgeCommandAdmission` → `SupervisorBridgeCommandAdmissionResolved`, realized at `meerkat-runtime/src/comms_drain.rs:645-739`). The new member-addressed commands (`ReadMemberHistory`, `PollMemberEvents`, `HardCancelMember` execution) extend that classification coverage — the typed command manifests (`runtime_alphabet_parity`) force this to be explicit. Today **nothing validates fence/topology-epoch member-side on delivery** (only supervisor identity+epoch); that stays true for delivery in v1 (supervisor-side `StaleFenceToken` on submit covers it), but the new host-addressed materialize/release path validates `(generation, fence_token)` in `MobHostBindingAuthority` — that check is new work, not reuse.

### 6.5 MobMachine — subscriptions and grants

- Event subscription: the adjudication seam exists with two outcomes — `AuthorizeAgentEventSubscription { agent_identity, session_id }` / `RejectAgentEventSubscription { reason: MemberNotFound | NoSessionBinding }` (mob_machine.rs:1323-1324). Add the **third outcome** `AuthorizeExternalAgentEventSubscription { agent_identity, host }` guarded on `member_placement`/`external_peer_edges`. Extend `AuthorizeAllAgentEventSubscription` to carry the external member set next to `session_bound_runtimes` — today the mob-wide stream **silently omits** external members (`AuthorizedMobEventRouter` filters to session-bound, `meerkat-mob/src/runtime/event_router.rs:48`, handle.rs:2582-2653); this plan kills that silent cap.
- Grants: `operator_grants: Map<PrincipalId, GrantRecord { scopes, expires_at_ms }>` with `GrantOperatorScopes` / `RevokeOperatorScopes` inputs. Expiry is data checked at the enforcement seam (shell reads the clock; the sealed policy compares — mirrors how `ToolExecutionPolicy` keeps policy resolution out of the machine while lifecycle stays machine-owned).

## 7. Wire & protocol deltas (meerkat-contracts; one versioned protocol)

`BridgeProtocolVersion` gains **V4**; `SUPPORTED = [V2, V3, V4]`; every new command requires V4 (per-command version + fail-closed decode already exist: `decode_bridge_command`, supervisor_bridge.rs:318-330). `BridgeCapabilities` (already exchanged on bind, :769-816) advertises: `durable_event_replay`, `autonomous_members`, `hard_cancel_member` (flips to true), protocol range. **The comms `Envelope` does not change**: it signs `(id, from, to, kind)` (`meerkat-comms/src/types.rs:157`); topology epoch already rides where it belongs — inside control payloads (`BridgeMobPeerOverlayHandoff.topology_epoch`, contracts :939) — and delivery staleness is receiver-side trust gating. If the envelope ever must evolve, the mechanism is the `content_taint` precedent (absence-preserving optional field inside the signed region, fail-closed on old receivers; types.rs:54-61), not a version integer.

New commands (same `BridgeCommand` enum, `deny_unknown_fields`, `#[non_exhaustive]`):

**Member-addressed** (admitted by MeerkatMachine, as today):
- `ReadMemberHistory { supervisor, epoch, protocol_version, from_index, limit }` → `BridgeReply::MemberHistoryPage` (mirrors `SessionHistoryPage`; transcript snapshot pages).
- `PollMemberEvents { supervisor, epoch, protocol_version, cursor: MemberEventCursor | Tail, max, wait_ms }` → `BridgeReply::MemberEventsPage { generation, events: Vec<EventEnvelope>, next_seq, watermark }`. Long-poll: `wait_ms` bounded well under the bridge transport budget. Served from `EventStore.read_from` when `durable_event_replay`, else from a bounded live buffer with typed `StaleCursor` on overrun.
- `HardCancelMember` — variant exists on the wire today but every receiver rejects it and nothing sends it (provisioner.rs:3297-3312; comms_drain catch-all). v1 implements the receiver arm through the machine-admitted hard-cancel path and a production sender, gated by the `Cancel` scope.

**Host-addressed** (new payload family carrying host authority epoch; admitted by `MobHostBindingAuthority`):
- `BindHost { supervisor, epoch, protocol_version, expected_host_peer_id, expected_address, bootstrap_token }` → capabilities reply (mirror of `BindMember`).
- `MaterializeMember { spec, agent_identity, generation, fence_token, budget_seed, launch_mode }` → `MemberMaterialized { member_pubkey, member_peer_id, advertised_address, session_id }`. **Idempotent** on `(agent_identity, generation, fence_token)`: replay returns the recorded result (the `BridgeDeliveryOutcome::Deduplicated` precedent); a lower tuple is a typed `StaleFence` reject.
- `ReleaseMember { agent_identity, generation, fence_token }` — retire/teardown on the owning host; also the reconciliation verb for orphans.
- `InstallPeerTrust { agent_identity, peer: TrustedPeerDescriptor }` / `RemovePeerTrust { ... }` — realize `RouteInstallRequested` on the owning host through the member's machine-gated `apply_trust_mutation` seam (never direct TrustStore writes).
- `HostStatus {}` → materialized-member inventory + health (feeds reconciliation and reachability projection).

New rejection causes extend `BridgeRejectionCause` (typed only — the `bridge-classifier` gate forbids `ResponseStatus` reinterpretation): `StaleFence`, `StaleCursor`, `Unavailable`, `ScopeDenied { required, presented }`.

Cursor type: `MemberEventCursor { generation: Generation, seq: u64 }` — `seq` is the owning host's durable `StoredEvent.seq` for the session bound at that generation; a generation bump (respawn) resets the seq domain and the page reply always carries the current generation so consumers restart cleanly.

Projections (contracts): `mob/member_status` gains `placement: Option<HostId-projection>`, reachability fields (§7.5); new `mob/host_status` and route-install status DTOs; grant DTOs. Blast radius gates: `make regen-schemas`, SDK codegen, `verify-version-parity`, `verify-rpc-surface-alignment` / `verify-rest-surface-alignment`.

### 7.1 Why the envelope stays untouched (recorded so it is not re-proposed)

The draft proposed signing `version + target host + mob/topology epoch` into the envelope. Rejected: (a) epoch-in-envelope couples the data plane to authority propagation — mid-rotation, honest senders with stale epochs get bounced; the property it buys ("stale topology rejects delivery") already holds receiver-side via trust classification (unwire ⇒ trust removed ⇒ untrusted ⇒ rejected); (b) `target host` has no consumer — there is no relay in v1 and the TCP connection terminates at the host, `to` selects the mailbox; (c) a `CommsProtocolVersion` integer is not the house evolution style and nothing changes that needs it.

### 7.2 Host bind ceremony (plane a)

1. Operator starts `rkat mob host` on Host B. The host loads/mints its host keypair (`Keypair::load_or_generate` in the host identity dir), binds the acceptor, and writes a **host binding descriptor** (the `--comms-binding-out` shape, host flavor: `{kind: "host", address, ed25519 public_key, bootstrap_token}`).
2. Operator (or automation) hands the descriptor to the controlling mob: `BeginHostBind` → routed `RequestHostBind` → bridge `BindHost` with the one-time bootstrap token, expected identity, expected address.
3. Host validates via `MobHostBindingAuthority` (token, sender, address — the `validate_bind_request` shape, comms_drain.rs:879-965), records the supervisor binding at the offered epoch, replies capabilities.
4. `CommitHostBind` records `{HostId, pubkey, endpoint, capabilities, epoch}` in MobMachine. Supervisor rotation fan-out (already fail-closed with pending-rotation memory) extends its recipient set to bound hosts.

Pairing on the host acceptor (the `{`-sniff JSON handshake) proves possession of the **host** secret and returns the **host** bootstrap token; members are never paired individually — member trust flows through machine-authorized `InstallPeerTrust`.

### 7.3 Placement and materialization

Placement input: `SpawnMemberSpec.placement: Option<HostId>`. Defaults: a member-initiated spawn (B2 spawns B21) defaults to the **requesting member's host**; operator/owner spawns default to the controlling host. Admission rejects unbound hosts or missing capabilities (e.g. `autonomous_members` for an `AutonomousHost`-mode profile) with typed causes.

B2-spawns-B21 walkthrough: delegation tool → MobCommand::Spawn → `ResolveSpawnMemberAdmission` (placement=Host B validated) → `BeginSpawnExec` (Opened) → ladder enters `MaterializePending`, machine emits `RequestMemberMaterialization` → bridge `MaterializeMember` to Host B → Host B: `prepare_bindings`, session created via its session service, member comms runtime minted (its own keypair, its own inbox), identity registered on the host acceptor, `MobHostBindingAuthority` records `(identity, generation, fence, session)` → `MemberMaterialized` ack carries pubkey/address/session → `ResolveMemberMaterialization` → `CommitSpawnMembership` publishes roster + `RegisterMemberPeer` publishes the endpoint → `CommitSpawnActivation` → wiring rules fire (§6.2) → B21 visible and controllable in the console.

Key custody: the member's private key is minted and stays on Host B; only the pubkey travels (the `BackendPeer.peer_id`-derives-from-real-key discipline, `meerkat-mob/src/backend.rs:34-53`, generalized). Budget: the spawn's budget split is computed on the controlling host and seeded in the materialize payload; enforcement is local to the member's session (aggregate mob budget is a non-goal, §12).

Runtime modes: host-materialized members support both `AutonomousHost` and `TurnDriven` (`MobRuntimeMode`, `meerkat-mob/src/runtime_mode.rs:7-13`) — the member host runs the loop. The forced-TurnDriven normalization (actor.rs:783-791) continues to apply only to legacy peer-only `RuntimeBinding::External` members. The empty `ExternalBackend` provisioner stub (provisioner.rs:2580) is **deleted**; `MultiBackendProvisioner` dispatches `RuntimeBinding` — `Session` (local), `External` (legacy peer-only, unchanged), new `HostMaterialized { host }`.

Failure/orphans: materialize timeout/failure → `member_materialization_failures` + the existing machine-authorized revival flow (observe → classify → realize; `Broken` terminal). Orphan reconciliation: on (re)bind, `HostStatus` reports the host's materialized set; the controlling machine issues `ReleaseMember` for entries it does not recognize at a current fence — the whole-mob-resume external rebind path (commit "external_tcp: own external members by MobMachine owner bridge session for resume restore" + `AuthorizeMemberPeerRebind`) is the recovery precedent.

### 7.4 Events, history, completion, cancel — closing the observation gap

The verified blocker family (nothing carries a remote member's session events or history across hosts today; `BridgeCommand` has no read/stream variant; the mob-wide merger is local-only; per-member subscribe hard-rejects peer-only members with machine-emitted `NoSessionBinding`; the delivery path **discards the completion handle** at comms_drain.rs:2701 and the supervisor degrades TurnCompleted to a dispatch-ack; `HardCancelMember` is reject-only):

- **History:** console `mob/member_history` (new RPC/REST/MCP surface, by `AgentIdentity`) → controlling host: local member ⇒ existing session read; remote ⇒ `ReadMemberHistory` proxy. Same page shape either way.
- **Events:** the controlling host runs a **poll pump** per remote member (schedule-driver-style loop) driving `PollMemberEvents` from the last cursor, wrapping results as `AttributedEvent { source, source_fence_token, role, envelope }` (the existing merge item, `meerkat-mob/src/event.rs:509-518`) and feeding `AuthorizedMobEventRouter` next to the local `SelectAll`. Dual cursor: the source `(generation, seq)` is preserved on each item; the merged stream's ingest order is the router's own. Console UX is unchanged (`mob/stream_event` push from the controlling host). Pump lifecycle follows the third subscription outcome (§6.5).
- **Completion:** falls out of events — terminal AgentEvents arrive through the pump; `wait_one`/`wait_all`/flow-step completion for remote members consume the merged stream (the `spawn_turn_completed_reply` degradation path is upgraded to it). `DeliverMemberInput` keeps replying at admission (unchanged; the reply deliberately carries no turn payload).
- **Hard cancel:** receiver arm lands (machine-admitted), capability flips, `Cancel` scope gates it at the controlling host.

Bridge client concurrency: `MobSupervisorBridge` currently serializes requests through a single-flight `request_lock` (supervisor_bridge.rs:41-48). Long-polls must not block lifecycle commands: correlation is already by envelope id, so v1 relaxes single-flight to per-request correlation (or a dedicated poller bridge instance per host) — named here because it looks optional and is not.

`bridge-classifier` gate mechanics: `BRIDGE_CLASSIFIER_FILES` (`xtask/src/bridge_classifier.rs:27-31`) is a hardcoded three-file allowlist. Every new file that consumes `BridgeReply` (the poll pump, the history proxy) MUST be added to it, or it silently escapes the no-`ResponseStatus` gate.

### 7.5 Reachability (projection)

Computed on the controlling host from typed outcomes it already sees: bridge request results (verified-ack vs `PeerOffline`/timeout — liveness today is exactly the per-send verified-ACK, `DEFAULT_ACK_TIMEOUT_SECS = 30`, router.rs:52) and pump progress. Surfaced per host and per member as `control_reachability` / `comms_reachability`: `Reachable | Stale | Unreachable | Unknown` (the wire enum exists at supervisor_bridge.rs:600) + `last_seen` + `freshness_reason`. Freshness is **observer-local monotonic** (receive-time + elapsed); never compare remote wall-clocks across hosts. Reachability never mutates membership. The existing `ExternalMemberReachability` projection (handle.rs:842) and `SupervisorReachability` transport classifier (mob supervisor_bridge.rs:58) are absorbed/fed, not duplicated.

## 8. Scoped control (plane b)

```rust
enum ControlScope { List, ReadHistory, SubscribeEvents, SendCommand, Cancel, Retire, WireTopology, AdminHost }
```

- Grants are MobMachine facts (§6.5), keyed by `PrincipalId` (comms-principal = PeerId; local operator contexts injected — Gotcha #19: operator authority is injected, not ambient; `MobToolAuthorityContext` + seal at `meerkat-core/src/service/mod.rs:491,511` is the vocabulary to extend).
- Enforcement: a sealed `ResolvedControlPolicy` checked at the controlling host's two chokepoints — MobCommand admission and the operator surfaces (RPC/REST/MCP mob handlers + agent-facing mob tools). Deny = typed `ScopeDenied { required, presented }`. Fail-closed: an unresolvable principal has no scopes.
- Defaults: the owning session (`owner_bridge_session_id`) holds implicit full scope — single-user CLI behavior is unchanged. Every other principal is default-deny until granted. Scope semantics honored end-to-end: `SendCommand` without `ReadHistory` can drive but not read; `SubscribeEvents` without `Cancel` can watch but not stop; `WireTopology` is distinct from `SendCommand`; `AdminHost` (bind/revoke hosts) is distinct from everything.
- Ordering rule: **no remote read/mutation surface ships before this lands** (phase 5 precedes phase 6).

## 9. Failure semantics (typed, enumerated)

| Failure | Behavior |
|---|---|
| Materialize timeout / host reject | `MaterializePending → member_materialization_failures[identity]`; spawn aborts via the ladder's abort path or retries idempotently on the same `(identity, generation, fence)`; revival classification governs re-attempts; `Broken` stays terminal. |
| Materialized on host, commit lost | Orphan: host holds it in `MobHostBindingAuthority`; next (re)bind's `HostStatus` reconciliation → `ReleaseMember` at stale fence. Roster never shows a member the machine didn't commit. |
| Partial route install | Obligation remains in `pending_route_installs`; edge unusable at the uninstalled end (receiver rejects untrusted); `RouteInstallIncomplete` projection reports per-host status; retry drains obligations idempotently. |
| Member host restart | Host rebinds (durable `MobHostBindingAuthority` record + `HostRebound` epoch bump); sessions recover from the host's realm-local stores; fences re-issued monotonically (re-fencing is intentional, not idempotent ownership); acceptor registry rebuilt from recovered materialized set; cursors survive (durable seq). |
| Controlling host restart | Existing mob resume + supervisor-authority recovery; hosts re-probed via durably recorded state before being trusted (pending-rotation discipline). |
| Partition | Observation degrades to typed `Unavailable`/`Stale`; deliveries surface `PeerOffline`; membership and topology unchanged; obligations retained. Fail closed, never fail quiet. |
| Stale supervisor / stale fence | Member-side `StaleSupervisor` (exists) and host-side `StaleFence` (new) typed rejects; rotation retry re-validates accepted peers from durable metadata. |
| Cursor overrun (ephemeral member host) | Typed `StaleCursor` + current watermark; consumer restarts from watermark; capability `durable_event_replay=false` was declared at bind, so the console can label the gap honestly. |

## 10. Phases (exit gates, no calendar)

Each phase exits only with its gates green. Machine phases additionally run: `make machine-codegen`, `make machine-check-drift`, `make machine-verify`, `runtime_schema_parity`, `runtime_alphabet_parity`, `xtask seam-inventory`, `xtask effect-authority`, `xtask ownership-ledger --check-drift`, `xtask rmat-audit --strict`. Contracts phases additionally run: `make regen-schemas`, `verify-schema-freshness`, `verify-version-parity`, `verify-sdk-codegen-freshness`, `verify-rpc-surface-alignment`, `verify-rest-surface-alignment`.

1. **Adjudication + catalog deltas.** This document is the phase-1 artifact. Land §6 in one catalog change-set (fields, inputs, effects+dispositions, invariants, `MobHostBindingAuthority` DSL, MeerkatMachine admission coverage) + §7 contracts (V4 commands, replies, causes, cursor, projections). TLC bounds: 2 hosts × 3 members.
2. **Host bind + acceptor + host role.** D1 demux in meerkat-comms (registry install/remove from machine effects only; mandatory peer auth; acks signed per-member; pairing branch host-scoped); §7.2 ceremony end-to-end; `rkat mob host` daemon (D6); host binding descriptor.
3. **Placement + materialization.** §7.3 end-to-end including B2→B21/B22 on Host B; `HostMaterialized` binding variant; `ExternalBackend` stub deleted; idempotent retry; orphan reconciliation; revival integration.
4. **Cross-host wiring install.** §6.2 obligations; placement-aware transport selection; `wire_members_batch` local-only rejection removed; unwire symmetry; fail-closed partial install with durable retry.
5. **Control scopes.** §8 grants + sealed policy at both chokepoints; `ScopeDenied`; owner-implicit-full default. **Must land before phase 6 ships any remote read.**
6. **Events / history / completion / cancel.** §7.4: DSL third outcome + all-subscription external fan-out; `ReadMemberHistory` + `PollMemberEvents` + pumps + router merge with dual cursors; completion consumption; `HardCancelMember` receiver + sender; bridge single-flight relaxed; `BRIDGE_CLASSIFIER_FILES` extended.
7. **Surfaces.** RPC/REST/MCP/SDKs: by-identity `member_history`, member event streams for remote members (existing `mob/stream_event` now covers them), host register/status, route-install status, grant management (`rkat mob grant/revoke`, host verbs). RPC stays the console↔controlling-host transport; `rkat-rpc --tcp` is never a host↔host control path (one control plane: the bridge).
8. **MobKit projection.** Remote/local members project into the existing console identity records; remote history/events consumed via the new Meerkat surfaces; console contracts stable; no MobKit-owned roster/directory (verified by review, not assumed).

## 11. Test matrix (lane-mapped; deterministic lanes are the CI ratchet)

Lane authority: `tests/integration/src/e2e_lanes.rs`. Known trap: e2e-smoke/e2e-system do **not** run in GitHub CI, and external-TCP real-TCP coverage has historically been an ignored smoke lane — so the deterministic lanes below are the regression gate, not the live ones.

**`unit`/`int`/`e2e-fast` (GitHub CI)** — two-hosts-in-one-process harness (two MeerkatMachines + two acceptors over loopback/in-memory transport):
- mixed A/B/C topology: identity-routed send local↔remote both directions; B1↔B2 same-host inproc selection
- many identities, one acceptor: demux by `to`, per-member ack signing, misaddressed reject, unregistered-identity reject
- mandatory peer auth on acceptor cannot be disabled; envelope byte-compat pin (no signed-region drift)
- host bind ceremony: token single-use, sender mismatch, address mismatch, capability capture, epoch record
- remote spawn ladder: B2→B21 full walkthrough; materialize retry deduplicates; stale-fence materialize rejected; orphan reconciliation releases at stale fence; revival classification on materialize failure; `Broken` refuses retry
- wiring: partial install leaves obligation + unusable edge fails closed; retry drains; unwire removes trust and subsequent delivery is rejected; batch wiring with mixed placements
- events: durable cursor resume across pump restart AND member respawn (generation bump resets seq domain); mob-wide stream **includes** remote members; completion for remote members via merged stream; long-poll does not block lifecycle commands; `StaleCursor` on ephemeral overrun
- history: remote page read == local page shape; scope matrix — each `ControlScope` grants exactly its verbs, `ScopeDenied` carries required/presented; work-only cannot read history; read-only cannot cancel
- rotation: bound hosts in fan-out; pending-rotation retry with a host that rebound to current authority
- TLC (`make machine-verify`) at 2×3 bounds green for all new state

**`e2e-system` (local `make ci` / nightly)** — real multi-process TCP: `rkat mob host` daemon lifecycle; host restart rebind with session recovery and fence advance; partition (kill link) → typed `Unavailable`, membership unchanged, obligations retained; controlling restart resume with host re-probe.

**`e2e-smoke` (live)** — kitchen-sink A/B/C with real providers.

## 12. Non-goals (v1) and v2 seeds

Out of scope, stated so they cannot creep silently: relay/multi-hop envelope routing; dynamic host discovery (registration is explicit); cross-host aggregate budget enforcement (per-session enforcement + later projection only); distributed realms; direct observer→member-host streams; leaderless-fabric semantics (separate track); outbound connection pooling (the router dials per send — `router.rs:575-643`; a known perf characteristic to measure in e2e-system, correctness-neutral). v2 seeds, in likely order: connection reuse, direct observer streams, richer placement policy (capability/label matching — note `RuntimeHostInfo.placement_labels` has zero writers today and an anti-authority pin; any real capability matching feeds MobMachine facts, not that projection).

## 13. Implementer gotchas (verified traps)

1. `BRIDGE_CLASSIFIER_FILES` is a hardcoded allowlist — extend it for every new bridge-reply consumer (§7.4).
2. `RuntimeHostInfo` must stay anti-authority (pinning test at wire/host.rs:181-210) — do not enrich it with host/placement authority facts.
3. DSL wiring inputs are **plural**; the singular names are bridge commands. Do not invent `WireMember` DSL inputs.
4. No member-side fence/epoch validation exists on delivery today — the host-side `StaleFence` checks in §6.3 are new work.
5. `ExternalBackend` (provisioner.rs:2580) is an empty stub — delete, don't extend.
6. `MobSupervisorBridge` single-flight `request_lock` will serialize long-polls against lifecycle commands unless relaxed (§7.4).
7. Trust rows are never seeded from config and persisted seeds are rejected at startup (comms_runtime.rs:1620-1632) — all trust flows through `apply_trust_mutation` under machine authority; the acceptor registry follows the same rule.
8. Three sequence domains exist (session-task seq, per-RPC-stream seq, durable store seq) — only the durable store seq is a cursor; never leak the others into the wire cursor.
9. Re-fencing on re-acquire is intentional (fencing mechanism, not churn) — do not "fix" monotonic fence bumps on host rebind.
10. Effects need typed dispositions (`seam-inventory`) and commands need typed classification (`runtime_alphabet_parity`) — budget for these in every machine delta, they are CI failures, not review notes.
