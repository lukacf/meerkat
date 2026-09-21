# Comms Model

Load this reference when working on peer trust, inter-agent messaging, comms drain lifecycle, or peer identity.

## Topology

- **InprocRegistry** — process-global peer discovery
- **CommsRuntime** — per-session, created by `AgentFactory::build_agent()` when `comms_name` is set
- **Wiring** — bidirectional trust. Each peer has `TrustedPeerSpec`
- **Unified trust state** — single `Arc<parking_lot::RwLock<TrustStore>>`. `TrustStore` (meerkat-comms/src/trust.rs) is `PeerId`-keyed (`BTreeMap<PeerId, TrustEntry>`); duplicate `PeerId` inserts are rejected with a typed `DuplicatePeerId` error, so name-keyed trust folklore is structurally impossible
- **Not all mob members are peers** — wiring rules control which members can communicate

## Peer classification

Envelope classification (trusted-vs-untrusted, content-shape detection, handling mode inference) runs through MeerkatMachine DSL via the `PeerCommsHandle.classify_external_envelope` / `classify_plain_event` methods. The trait lives in `meerkat-core/src/handles.rs`; the runtime impl routes to the DSL's classification signals. `ClassifyExternalEnvelope` carries the typed `from_peer_id: PeerId` (not just a display name), and the machine echoes the canonical peer id back in the classification result — shell code reads `ingress_fact.canonical_peer_id` from the machine-owned result rather than re-deriving identity from envelope strings.

Canonical `PeerId` lives in `meerkat-core/src/comms.rs`; the current ingress-kind
vocabulary is `PeerIngressKind` in `meerkat-core/src/interaction.rs`.
`meerkat-comms/src/peer_types.rs` contains the shell-side `ContentShape` and
`PeerIngressState` types, not independent state machine authorities. The trust
state on `ClassifiedInboxQueue` is the shared `Arc<RwLock<TrustStore>>`.
`PeerIngressState` is the queue's local projection of
`PeerIngressAuthorityPhase`, updated only from machine receive/dequeue
authority results through `PeerCommsHandle`. Queue storage, order, and locking
remain mechanical; the local phase copy does not choose lifecycle truth.

## Comms drain lifecycle

Background inbox consumption is a real lifecycle seam. Drain phase (`Inactive` / `Running` / `Stopped` / `ExitedRespawnable`) and drain mode (`PersistentHost` / `Timed` / `AttachedSession`) are MeerkatMachine DSL state (`drain_phase`, `drain_mode`). A local starting mechanic is not a DSL phase. Spawn/stop/exit transitions flow through the DSL via `CommsDrainHandle` (for cross-crate callers) or direct `dsl_apply` (for in-runtime callers).

Turn-boundary suppression is a local projection of that truth. Do not keep parallel drain-phase state in shell caches.

## Session identity claims

Session identity claims route through `SessionClaimHandle` (meerkat-core/src/handles.rs): `try_acquire(session_id)` atomically reserves the identity and returns an RAII `SessionClaim` whose `Drop` releases the slot (`release` is idempotent). Bare paths with no `MeerkatMachine` use the process-global `DefaultSessionClaimRegistry`. Dangling tasks holding claims can leak identity across in-process restarts — make sure teardown drops the claim.

## Key files

- `meerkat-comms/src/runtime/comms_runtime.rs` — `CommsRuntime`
- `meerkat-comms/src/trust.rs` — `TrustStore` (PeerId-keyed trust entries)
- `meerkat-core/src/comms.rs` — canonical `PeerId`
- `meerkat-core/src/interaction.rs` — `PeerIngressKind`, `PeerIngressAuthorityPhase`
- `meerkat-comms/src/peer_types.rs` — `ContentShape` and the non-authoritative `PeerIngressState` projection
- `meerkat-comms/src/classify.rs` — envelope classification (calls `PeerCommsHandle` for lifecycle transitions; consumes machine-echoed canonical peer id)
- `meerkat-comms/src/inbox.rs` — `ClassifiedInboxQueue`, shared trust store handle, shell-mechanics drain helpers
- `meerkat-comms/src/inproc.rs` — `InprocRegistry`
- `meerkat-runtime/src/handles/peer_comms.rs`, `comms_drain.rs` — runtime impls of the DSL handle traits
- `meerkat-core/src/handles.rs` — `PeerCommsHandle`, `CommsDrainHandle`, `SessionClaimHandle` trait definitions
