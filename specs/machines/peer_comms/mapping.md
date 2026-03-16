# PeerCommsMachine Mapping Note

This note maps the normative `0.5` `PeerCommsMachine` contract onto current
`0.4` anchors.

## Rust anchors

- comms runtime/inbox ownership:
  - `meerkat-comms/src/runtime/comms_runtime.rs`
  - `meerkat-comms/src/inbox.rs`
- current normalization/classification:
  - `meerkat-comms/src/classify.rs`
  - `meerkat-comms/src/agent/types.rs`
- trust store and router:
  - `meerkat-comms/src/trust.rs`
  - `meerkat-comms/src/router.rs`
- current host/runtime bridge:
  - `meerkat-core/src/agent/comms_impl.rs`
  - `meerkat-core/src/agent/runner.rs`

## What is already aligned

- there is already one shared trusted-peer store
- current ingress already snapshots trust/classification before downstream
  conversion
- peer lifecycle notifications are already normalized into explicit classes
- acks are already transport-side and not surfaced into the core agent loop
- runtime-backed peer admission already exists through `RuntimeCommsInputSink`

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- transport socket/task mechanics
- cryptographic signature payloads
- request/response correlation payloads
- exact inbox channel implementation
- transcript/session mutation
- host-loop-specific batching behavior

Those are implementation details refining the same peer normalization contract.

## Intentional `0.5` shift

The formal contract describes the architectural responsibility, not the legacy
classified-inbox mechanism.

In `0.5`:

- the classified-inbox bypass path should die
- the responsibility survives as admission-time peer normalization
- typed `PeerInput` submission should happen through runtime admission rather
  than through a parallel direct-agent execution path

## Known `0.4` divergence

- classification and queueing are currently coupled to inbox implementation
- host-mode routing still exists outside the canonical runtime path
- `SubagentResult` currently leaks through comms/inbox machinery even though
  `0.5` moves that responsibility into `OpsLifecycleMachine`

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `PeerCommsMachine`

### Code Anchors
- `peer_classify`: `meerkat-comms/src/classify.rs` — peer classification precursor
- `peer_inbox`: `meerkat-comms/src/inbox.rs` — peer inbox and request/reservation registry precursor
- `peer_runtime`: `meerkat-comms/src/runtime/comms_runtime.rs` — runtime comms owner precursor

### Scenarios
- `trust-normalize-submit` — trusted peer envelope is normalized and submitted exactly once
- `untrusted-drop` — untrusted or invalid peer work is dropped before runtime admission
- `request-response-correlation` — reservation/request state remains consistent across peer traffic

### Transitions
- `TrustPeer`
  - anchors: `peer_classify`, `peer_inbox`, `peer_runtime`
  - scenarios: `trust-normalize-submit`, `untrusted-drop`
- `ReceiveTrustedPeerEnvelope`
  - anchors: `peer_classify`, `peer_inbox`, `peer_runtime`
  - scenarios: `trust-normalize-submit`, `untrusted-drop`
- `DropUntrustedPeerEnvelope`
  - anchors: `peer_classify`, `peer_inbox`, `peer_runtime`
  - scenarios: `trust-normalize-submit`, `untrusted-drop`
- `SubmitTypedPeerInputDelivered`
  - anchors: `peer_classify`, `peer_inbox`, `peer_runtime`
  - scenarios: `trust-normalize-submit`
- `SubmitTypedPeerInputContinue`
  - anchors: `peer_classify`, `peer_inbox`, `peer_runtime`
  - scenarios: `trust-normalize-submit`

### Effects
- `SubmitPeerInputCandidate`
  - anchors: `peer_classify`, `peer_inbox`, `peer_runtime`
  - scenarios: `trust-normalize-submit`

### Invariants
- `queued_items_are_classified`
  - anchors: `peer_classify`, `peer_inbox`, `peer_runtime`
  - scenarios: `trust-normalize-submit`, `untrusted-drop`


<!-- GENERATED_COVERAGE_END -->
