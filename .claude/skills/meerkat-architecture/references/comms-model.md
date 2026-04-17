# Comms Model

Load this reference when working on peer trust, inter-agent messaging, comms drain lifecycle, or peer identity.

## Topology

- **InprocRegistry** — process-global peer discovery
- **CommsRuntime** — per-session, created by `AgentFactory::build_agent()` when `comms_name` is set
- **Wiring** — bidirectional trust. Each peer has `TrustedPeerSpec`
- **Unified trust state** — single `Arc<parking_lot::RwLock<TrustedPeers>>`
- **Not all mob members are peers** — wiring rules control which members can communicate

## Peer classification

Envelope classification (trusted-vs-untrusted, content-shape detection, handling mode inference) runs through MeerkatMachine DSL via the `PeerCommsHandle.classify_external_envelope` / `classify_plain_event` methods. The trait lives in `meerkat-core/src/handles.rs`; the runtime impl routes to the DSL's classification signals.

Shell-side data types live in `meerkat-comms/src/peer_types.rs` (`PeerId`, `RawPeerKind`, `ContentShape`, `PeerIngressState`). These are pure data, not state machine authorities. Trust set (`trusted_peers: BTreeSet<PeerId>`) and per-peer phase (`PeerIngressState`) are shell mechanics on `ClassifiedInboxQueue` — concurrency plumbing, not lifecycle truth.

## Comms drain lifecycle

Background inbox consumption is a real lifecycle seam. Drain phase (`Inactive` / `Starting` / `Running` / `Stopped` / `ExitedRespawnable`) and drain mode (`PersistentHost` / `Timed` / `AttachedSession`) are MeerkatMachine DSL state (`drain_phase`, `drain_mode`). Spawn/stop/exit transitions flow through the DSL via `CommsDrainHandle` (for cross-crate callers) or direct `dsl_apply` (for in-runtime callers).

Turn-boundary suppression is a local projection of that truth. Do not keep parallel drain-phase state in shell caches.

## Session identity claims

Process-global claim tables must be released on teardown/recovery (`release_session_claim`, `clear_all_session_claims`) if you manage runtimes manually. Dangling tasks can leak identity across in-process restarts.

## Key files

- `meerkat-comms/src/runtime/comms_runtime.rs` — `CommsRuntime`
- `meerkat-comms/src/peer_types.rs` — pure data types (PeerId, RawPeerKind, ContentShape, PeerIngressState)
- `meerkat-comms/src/classify.rs` — envelope classification (calls `PeerCommsHandle` for lifecycle transitions)
- `meerkat-comms/src/inbox.rs` — `ClassifiedInboxQueue`, trust set, shell-mechanics drain helpers
- `meerkat-comms/src/registry/inproc.rs` — `InprocRegistry`
- `meerkat-runtime/src/handles/peer_comms.rs`, `comms_drain.rs` — runtime impls of the DSL handle traits
- `meerkat-core/src/handles.rs` — `PeerCommsHandle`, `CommsDrainHandle` trait definitions
