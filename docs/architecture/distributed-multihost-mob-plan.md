# Distributed Mobs as a Leaderless Peer Fabric (v3)

> Grounded in HEAD **v0.7.4** (commit `f4882d6cc`). **Supersedes v2 / v2.1**, which
> framed distribution as a *supervised, hub-centric* extension (central `HostId`
> authority, control-by-identity routed through `BridgeCommand`). That framing is
> abandoned. The chosen model is **leaderless**: every device is a sovereign peer,
> there is no required authority, and self-organization is emergent agent behavior —
> the framework provides only a substrate.
>
> This document is **concept-first** (the architecture was worked out conceptually
> before any protocol detail) and marks every claim about current behavior with a
> `file:line` anchor verified against HEAD.

---

## 0. Goal & the two shapes

Drop a meerkat agent onto many devices; let them peer with each other as one mob;
let a human observe and talk to any of them. Two deployment shapes, **one model**:

- **HomeBotNet / edge control** — heterogeneous devices (computers, robots, phones, routers) with a human **console**. The console is a **privileged *observer/operator* peer** — never an authority. If it's offline, the mesh is unaffected.
- **ESP32 swarm** — tiny devices that self-organize with no console at all (moisture sensors broadcasting presence, sharing observations, collectively reasoning — "must be the sprinklers"). Pure leaderless.

Both are the same system at different settings. The only difference is whether a peer holding observe/operate capabilities happens to be present.

---

## 1. Philosophy — infrastructure, not application

The framework gives you **authenticated, capability-scoped messages between peers,
and per-agent memory.** Everything that looks like a "distributed algorithm" is
**emergent agent behavior driven by prompts**, not framework machinery:

- Discovery ("Hi, I'm Bob"), the neighbor/directory picture, membership, convergence, relaying/multi-hop, dedup, localization from RSSI, temporal correlation ("Lisa got wet 2 min later"), and consensus ("must be the sprinklers") are **all prose reasoning over messages**. The framework implements **no** CRDT, SWIM, consensus, leader-election, or world-model service.
- The shared world model lives in each agent's context/`meerkat-memory` and in the messages exchanged. **The agents *are* the distributed algorithm.**

The framework's only irreducible jobs are the things a prompt physically cannot do:
put authenticated bytes on a wire, verify a credential, and persist state.

---

## 2. Invariants (the concepts, settled)

1. **A runtime is always owned by the device it runs on.** You never own a remote
   runtime. "Controlling" a remote agent is always mediated: you send its agent a
   message and **its own runtime decides**. (Today: `MeerkatMachine.sessions` already
   hosts many sovereign sessions per process, `meerkat-runtime/src/meerkat_machine/mod.rs:2123`.)
2. **A realm is local to one runtime and is never distributed.** Realm = the boundary
   for session **persistence** + the comms **inproc namespace** (`Realm ID is the comms
   inproc namespace boundary`, `factory.rs:4429`) + **credential resolution**. One realm
   per edge device; **degenerate on ESP32** (one device = one realm = one agent ≈ the
   NVS holding the keypair + memory).
3. **A mob is an emergent overlay across runtimes/realms — orthogonal to realms.**
   `mob_id ≠ realm`. In the field there is **no framework roster and no single
   `MobMachine` authority.** The `meerkat-mob` `Roster` / sealed `RosterAuthority` /
   single-`MobMachine` apparatus (`roster.rs`, `runtime/roster_authority.rs`) stays for
   the **supervised / in-process** mob only (codemob, single-host orchestration). The
   field mob runs none of it.
4. **Roster ownership = nobody.** Each node's machine owns *its own view* of the
   neighborhood (typed, single-owner, dogma-clean per node). Global membership is
   intentionally ownerless and un-stored — convergent emergent belief.

---

## 3. The unified peer fabric (the core idea)

There is **one plane**, not two (no separate "comms" and "control" systems). Everything
is a **capability-scoped message between peers**, and the receiving **runtime dispatches**
each message to the agent or to itself.

**One identity.** Every participant — sensor, robot, console, phone app — is a peer
with one Ed25519/`PeerId` identity (`meerkat-comms/src/identity.rs:104-118`). There is
no separate identity space for "users."

**One credential = capabilities.** The pre-shared mob credential carries a **capability
set** `{peer, observe, drive, interrupt, operate, …}`. A sensor's grant = `peer`. A
console's grant = `peer + observe + drive + operate`. The dogma rule *"a comms credential
must never imply control authority"* is enforced **inside one credential by capability**,
not by maintaining two systems. Holding `peer` grants nothing on `observe`.

**One transport abstraction.** Signed messages over `inproc | uds | tcp` today
(`meerkat-comms/src/transport/`), RF later. One bus.

**One dispatcher, two handler classes.** A received message is routed by *kind*,
authorized by *capability*:

| Message kind | Capability | Handled by | Hits the LLM? |
|---|---|---|---|
| peer-content (`Lisa→Bob: "I'm wet"`) | `peer` | **agent** | yes (as `user`, §6) |
| operator/user turn (`Console→Bob: "status?"`) | `drive` | **agent** | yes (as `user`, §6) |
| subscribe-events / read-history | `observe` | **runtime** | **no** |
| interrupt / cancel | `interrupt` | **runtime** | no |
| operate (reboot-all, …) | `operate` | **runtime** + agent policy | no |

**The console is just a peer** holding `observe + drive (+ operate)`. "Chat with Bob"
= a held peer conversation: send `drive` turns, hold an `observe` event stream, pull
`read-history` on open. It looks like chat; underneath it is the one bus.

**Local is the inproc case.** Single-host meerkat (console → local agent via `rkat-rpc`)
is exactly "console-peer → agent-peer, all inproc." Distributed is the identical model
with network transports and an emergent peer set. The existing RPC/SessionService
operations (`turn/start`, `session/read`, `session/history`, `session/event`,
`turn/interrupt`) become the **runtime-side handlers** the dispatcher invokes; the RPC
server is their **inproc/local binding**. One system, one box to a lawn.

**Why this dissolves the old "control plane" problem:** `observe` is a **direct peer
request to the runtime that owns the session**, answered from that runtime's own event
stream. There is nothing to merge through a mob — which is exactly why the v2 cross-host
event-router blocker (`handle.rs:3710-3727` rejects peer-only members) simply **does not
arise** in this model: we never route observability through the roster.

---

## 4. The framework / emergent line

**Below the line (framework — irreducible):** authenticate a message under the mob
credential; check the sender's capability; deliver bytes over a transport (inproc/net/RF);
dispatch to agent-or-runtime; persist session state; serve `observe`/`history` mechanically.

**Above the line (emergent — prompt-only):** who-is-in (membership), the directory,
where-things-are (topology/RSSI), what-is-true (world model), when-to-relay, when-to-forget,
and how-much-to-trust a given peer. All of these are beliefs an LLM forms from message
content.

The framework **guarantees** admission soundness (no non-member admitted) and idempotent,
monotonic local trust installs. The framework **guarantees nothing** about convergence,
completeness, agreement, or liveness — *that is the agents' distributed algorithm.* A
partition is not a failure; it's "I can't hear Lisa right now," which the agent handles
in prose.

---

## 5. Trust & membership — asymmetric, self-admitting, leaderless

### 5.1 Credential: a CA-grant, not a shared secret
The "pre-shared mob identity" is **asymmetric**, decisively *not* a symmetric PSK:

- A **mob CA keypair**; the **private half never touches the mesh** (lives only at provisioning).
- Each device is flashed with: the **mob CA public key** + its **own per-node grant** = an Ed25519 signature over `(node_pubkey ‖ mob_id ‖ epoch)` by the CA, **plus its capability set**.
- "Proves it belongs" = present the grant; any peer verifies it against the CA public key with a pure `verify_group_membership()` (no I/O).

**Why asymmetric:** a moisture sensor *will* be physically captured. With a symmetric
PSK, one captured node can mint membership for the **whole mob, unrevocably**. With the
CA shape, a captured node leaks only **its own** impersonability. Same provisioning
ergonomics (you still pre-share material), vastly better blast radius.

### 5.2 Self-admission — the seam already exists
The per-node, no-central-authority trust install is **~90% built and dogma-clean**
(verified at HEAD):

```
AddDirectPeerEndpoint  (meerkat-runtime/src/meerkat_machine_types.rs:763)
  → CommsTrustReconciler::reconcile  (comms_trust_reconcile.rs:123; full-set diff at :128/:211)
  → apply_trust_mutation  under  GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection
        (meerkat-core/src/comms.rs:534, gated by is_meerkat_machine_trust_source at :857)
  → per-node TrustStore  (meerkat-comms/src/trust.rs — "THE owner of comms trust state")
```

Each node's `MeerkatMachine` already self-installs peers into **its own** `TrustStore`.
Today admission keys purely on a pre-enrolled trust row (absent ⇒ `SendError::PeerNotFound`,
`router.rs:545-552`); the leaderless model swaps central wiring for **group-credential
self-admission**.

### 5.3 Membership sync = emergent gossip over a typed gate
Nodes converge by re-broadcasting natural-language presence/directory in message bodies;
each agent folds what it hears into its own belief and installs heard peers via the seam
above. No SWIM/CRDT service. Each node honors a peer iff its grant verifies.

### 5.4 Admission must remain ONE authority (dogma trap)
Admission is *already* a single machine-owned authority (`io_task` sig-check →
machine `classify`). The new gossip path **must fold in as a variant of that one
authority**, never a parallel shell predicate — or it forks Invariant-1. Build it as
**one authority, three origins**: `PeerEnrolled | Dialed | Gossip`, where the
`GroupMembershipProof` is **required when `origin == Gossip`** (hearsay) and unnecessary
when you dialed a configured peer yourself.

---

## 6. Representation at the model boundary (the substrate reality)

The fabric is async and N-party; the LLM loop is synchronous with a **fixed, tiny role
vocabulary** imposed by the providers — verified in meerkat's own clients:

- **Anthropic** (`meerkat-anthropic/src/client.rs`): message roles are **only `user`/`assistant`** (`:465,591,652`); `system` is a separate param (`:456-666`); **tool results are `user`-role content blocks** (`:636-652`).
- **OpenAI** (`meerkat-openai/src/client.rs`): `system`/`user`/`assistant`/`tool` (`:736,749,781,323`) (`developer` supersedes `system` in newer models).
- **Gemini** (`meerkat-gemini/src/client.rs`): `user`/`model` + `systemInstruction` (`:366,440,515`).

**No provider supports arbitrary roles.** Therefore **every inbound party — operator and
every peer — collapses to the `user` role at the wire.** This is the substrate, universal
to *all* LLM apps (a plain chatbot already collapses everything non-assistant/non-tool/non-system
into `user`); the swarm just produces *more* `user`s. Provenance/authority can only live
**above the wire, in content** (and meerkat-internal typed fields), **never as a role.**

### 6.1 meerkat's richer internal model projects DOWN
meerkat's `Message` enum has five variants — `System`, `SystemNotice`, `User`,
`BlockAssistant`, `ToolResults` (`meerkat-core/src/types.rs:1135`) — a richer *internal*
model that flattens to the provider floor at the client. Peer messages are already
projected to `Message::User` via a **typed** projection (`format_peer_message_projection`,
`types.rs:1931`) with **no stringly `[from X]` prefix** (test `input.rs:1164`).

### 6.2 The one representation addition: typed provenance on `UserMessage`
`UserMessage` (`types.rs:2101`) today carries no typed sender/capability. Add a **typed
origin `{sender_peer, capability, kind}`** (the way it already has `transcript_role`),
which the projector renders **consistently into the user-role content**. This is **not**
a role escape (impossible) — it's so the agent is told, structurally and uniformly, *who*
is speaking and *with what authority*. The agent's **prompt** then decides deference
(operator-turn from an `operate`-capable peer = instruction; peer-content from a `peer`-only
sensor = a claim). Framework delivers provenance; judgment stays emergent.

### 6.3 Scheduling is orthogonal to representation
The async→sync adapter is the existing **input lifecycle**. `HandlingMode {Queue, Steer}`
(`types.rs:504-515`) + `WakeMode` (`input_state.rs`) are the typed *scheduling* axis:
`Steer` = inner-loop now, `Queue` = next outer turn, `Wake` = re-open a completed turn
(the "wait-and-hold"). The apparent mid-loop "weirdness" is a **provider constraint** —
you cannot splice a `user` message between an assistant `tool_calls` and its `ToolResults`,
so a `Steer` waits for the next legal boundary and then enters as a uniform `user` message.
**Representation stays uniform (`User` + typed origin); only insertion timing varies.**

### 6.4 `SystemNotice` for runtime facts
Runtime-originated context the agent should know but that isn't a party speaking ("peer
Lisa went unreachable", "you were interrupted") uses `Message::SystemNotice`
(`types.rs:2000`) — distinct from the agent's `System` prompt. `observe`/`history`/`interrupt`
traffic is runtime-handled and **never becomes a `Message` at all**.

### 6.5 Capability does three jobs (the consolidation paying off)
The same capability object (a) **authorizes** the message on the fabric, (b) **frames** it
for the LLM (typed origin → content), and (c) **informs deference** in the prompt. One
concept across transport-auth, representation-provenance, and trust-weighting — not three
subsystems. The injection/deference risk is the **universal untrusted-content problem**
(same as tool output, RAG/web, multi-user), not swarm-specific, not role-enforceable by
anyone; mitigated the standard way (delimited provenance + prompt convention + capability-gated
command authority).

---

## 7. What the framework must add (irreducible) vs reuse

**Reuse (already at HEAD):** `PeerId`/Ed25519 identity; `inproc|uds|tcp` transport; signed
`Envelope` + canonical-CBOR signing; per-node `TrustStore`; the self-admission seam
(`AddDirectPeerEndpoint → CommsTrustReconciler → apply_trust_mutation`); the comms inbox
classifier; `Message` enum incl. `SystemNotice`; `HandlingMode`/`WakeMode` + input
lifecycle; `SessionService` ops (`turn/`, `session/read|history|event`, `interrupt`) as the
runtime handlers; the CA-grant idea slots beside the existing keypair load path.

**Add (the irreducible new mechanism):**
1. **`GroupMembershipProof`** — ≤64-byte wire-opaque newtype on the trusted-peer descriptor (`meerkat-core/src/comms.rs`).
2. **`MobGroupCredential`** (CA public key + this node's grant + capabilities) loaded per node like the keypair; pure **`verify_group_membership()`**.
3. **`GeneratedCommsTrustAuthoritySourceKind::MeerkatMachineGossipAdmission`** variant (`comms.rs:533`), registered in `is_meerkat_machine_trust_source` (`:857`).
4. **`PeerEndpointOrigin{Dialed, Gossip}`** on `AddDirectPeerEndpoint`; guard requires a valid proof when `Gossip`.
5. **Capability-scoped dispatch** — the grant's capability set drives the inbox classifier's agent-vs-runtime routing + authorization (machine-owned, typed, not per-handler checks). This subsumes the earlier "control scope" concern into the one credential.
6. **A subscribe/stream message pattern on the bus** — `subscribe-events` opens a stream the runtime feeds back as messages (today event streaming is RPC-notification-only). **The one genuinely new wire pattern.**
7. **Typed origin `{sender_peer, capability, kind}` on `UserMessage`** (§6.2).
8. **`OperatorGrant`** — CA-signed `{target_capability, grantee_pubkey, sig}` carried inside the existing `MessageKind::Request.params`; framework `verify()`, agent decides obedience; mesh works if operator absent.
9. **A frozen cross-implementation wire spec + byte-exact test vectors** (§8).
10. **A field comms profile** — cap `max_message_bytes` far below the 1 MiB default (`transport/mod.rs`), per-egress rate/size caps, and **connection reuse** (today `router.rs:583` does `TcpStream::connect` per send — a fan-out storm risk on constrained links).

---

## 8. ESP32 / embedded reality

meerkat already runs on ESP32 (S3/P4) over **WiFi + known TCP IPs** — the current signed
`Envelope`-over-TCP path with static addressing. Constraints to honor:

- **The ESP32 is a *firmware twin*, not a compile target.** `meerkat-comms` is std/tokio/ciborium/ed25519-dalek (no `no_std` path), so embedded nodes are a **second implementation** of the wire. ⇒ the **frozen wire spec + test vectors (add #9)** is a real, load-bearing deliverable to prevent Rust↔firmware drift.
- **Future RF transports** (ESP-NOW ~250 B, LoRa ~50–250 B, BLE) are broadcast-native and tiny-MTU. The signed `Envelope` is ~144 B of fixed overhead before payload — fine on WiFi/TCP today, but a future RF egress needs the small-frame profile (#10) and possibly a compact frame. **RF is out of scope for v1; WiFi+TCP is the v1 transport.**
- **Realm is degenerate** on ESP32 (one device/agent/realm); the inproc-namespace facet is unused.

---

## 9. Risks & explicitly-accepted properties

- **No online revocation (accepted).** Leaderless ⇒ revoking a captured node means rotating the credential `epoch` and re-granting survivors (coarse, manual). The asymmetric CA shape contains blast radius; it does not provide live revocation. **This is a policy decision (§11).**
- **Emergent membership can degrade silently (accepted property, not a bug).** The framework gives no convergence signal; partitions / prompt variance / buggy or adversarial gossip can leave divergent or stale pictures. Mitigation is emergent (agents notice staleness and re-probe) + optional operator observability. State it; don't pretend otherwise.
- **Admission-authority fork** (the dogma trap, §5.4) — the most likely way this is built wrong.
- **Learned-address poisoning + no liveness** — addresses arrive authenticated only as "from a member," so any valid member can advertise false reachability; caught only emergently.
- **Connect-per-send storm** (`router.rs:583`) — needs the field profile's connection reuse + rate caps.
- **Wire/contract churn** — any descriptor/`UserMessage`/`Message` change locks the 6-file version-parity surface + SDK codegen; sequence accordingly.
- **Provider role floor (§6)** — provenance/trust is content+prompt+capability, never role-enforced. Inherent to LLMs.

---

## 10. What stays unchanged

The **supervised / in-process mob** (`meerkat-mob` `MobMachine`, `RosterAuthority`,
`mob_wire`, the spawn-exec ladder, supervisor bridge / external-TCP) is **not deleted and
not modified** by this work. It remains the right tool for single-host orchestration and
codemob-style spawning. The leaderless field fabric is an **additional** configuration that
shares the comms substrate and the per-node `MeerkatMachine`, not a replacement for it.

---

## 11. Open decisions (genuinely the owner's to make)

1. **Revocation policy.** Accept coarse epoch-rotation + re-flash (simplest, fully
   leaderless), or add short-lived epoch'd grants periodically re-issued by an
   operator-capable peer (finer, reintroduces a soft dependency on an operator being
   reachable)?
2. **Operator command surface.** Are fleet actions (`reboot-all`) delivered as a broadcast
   `operate` message over the bus, or fanned out as per-peer `operate` requests? (Broadcast
   is more leaderless-native; fan-out gives per-node acks.)
3. **Reachability for the `Stale` cue.** Does "is this peer reachable" derive from
   message-ACK timeouts, a lazy heartbeat, or is it left entirely to agent inference?
   (Affects how the console shows offline vs idle.)

---

## 12. Non-goals (v1)

Distributed realms; multi-realm-per-host; a central roster / `MobMachine` authority in the
field; RF transports (WiFi+TCP only in v1); NAT traversal / hole-punching; transport-level
encryption beyond signing (content is authenticated, not confidential — add later if devices
sit on untrusted links); CRDT/SWIM/consensus as framework services; global event ordering /
causal clocks; online revocation; multi-homing one identity on multiple devices.

---

## 13. Build path (concept-led; each step leaves the tree green)

1. **Credential & proof** — `MobGroupCredential`, `GroupMembershipProof`,
   `verify_group_membership()`; per-node load path beside the keypair.
2. **Gossip admission as one authority** — `MeerkatMachineGossipAdmission` source-kind +
   `PeerEndpointOrigin{Dialed,Gossip}` guard, folded into the existing single admission
   authority (run `machine-verify`/`rmat-audit`/drift gates).
3. **Capability-scoped dispatch** — capability set on the grant; inbox classifier routes
   agent-vs-runtime and authorizes by capability.
4. **Subscribe/stream + observe/history over the bus** — the new wire pattern; runtime
   serves it; `OperatorGrant` for `operate`.
5. **Typed `UserMessage` origin** — projector renders provenance into content consistently.
6. **Agent tools** — `merge_peer_directory` / `forget_peer`; presence/discovery prompt patterns.
7. **Field profile + frozen wire spec + test vectors**; harden the real-TCP lane.
8. **Console-as-peer** — MobKit holds `observe/drive/operate` and speaks the fabric; local
   `rkat-rpc` re-cast as the inproc binding of the same handlers.

---

## 14. One-line model

**One fabric of capability-scoped peer messages; each device's runtime is sovereign and
routes every message to its agent or to itself; trust is a pre-shared CA-grant carrying
capabilities; membership and the world model are emergent; the console is just a peer; and
local is simply the inproc case.**
