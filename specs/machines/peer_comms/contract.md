# PeerCommsMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-comms` / `machines::peer_comms`

## State
- Phase enum: `Absent | Received | Dropped | Delivered`
- `trusted_peers`: `Set<PeerId>`
- `raw_item_peer`: `Map<RawItemId, PeerId>`
- `raw_item_kind`: `Map<RawItemId, RawPeerKind>`
- `classified_as`: `Map<RawItemId, PeerInputClass>`
- `trusted_snapshot`: `Map<RawItemId, Bool>`
- `submission_queue`: `Seq<RawItemId>`

## Inputs
- `TrustPeer`(peer_id: PeerId)
- `ReceivePeerEnvelope`(raw_item_id: RawItemId, peer_id: PeerId, raw_kind: RawPeerKind)
- `SubmitTypedPeerInput`(raw_item_id: RawItemId)

## Effects
- `SubmitPeerInputCandidate`(raw_item_id: RawItemId, peer_input_class: PeerInputClass)

## Helpers
- `ClassFor`(raw_kind: RawPeerKind) -> `PeerInputClass`

## Invariants
- `queued_items_are_classified`

## Transitions
### `TrustPeer`
- From: `Absent`, `Received`
- On: `TrustPeer`(peer_id)
- To: `Absent`

### `ReceiveTrustedPeerEnvelope`
- From: `Absent`, `Received`
- On: `ReceivePeerEnvelope`(raw_item_id, peer_id, raw_kind)
- Guards:
  - `peer_is_trusted`
- To: `Received`

### `DropUntrustedPeerEnvelope`
- From: `Absent`, `Received`
- On: `ReceivePeerEnvelope`(raw_item_id, peer_id, raw_kind)
- Guards:
  - `peer_is_not_trusted`
- To: `Dropped`

### `SubmitTypedPeerInputDelivered`
- From: `Received`
- On: `SubmitTypedPeerInput`(raw_item_id)
- Guards:
  - `item_was_queued`
  - `item_was_classified`
  - `delivery_drains_queue`
- Emits: `SubmitPeerInputCandidate`
- To: `Delivered`

### `SubmitTypedPeerInputContinue`
- From: `Received`
- On: `SubmitTypedPeerInput`(raw_item_id)
- Guards:
  - `item_was_queued`
  - `item_was_classified`
  - `delivery_leaves_more_work`
- Emits: `SubmitPeerInputCandidate`
- To: `Received`

## Coverage
### Code Anchors
- `meerkat-comms/src/classify.rs` — peer classification precursor
- `meerkat-comms/src/inbox.rs` — peer inbox and request/reservation registry precursor
- `meerkat-comms/src/runtime/comms_runtime.rs` — runtime comms owner precursor

### Scenarios
- `trust-normalize-submit` — trusted peer envelope is normalized and submitted exactly once
- `untrusted-drop` — untrusted or invalid peer work is dropped before runtime admission
- `request-response-correlation` — reservation/request state remains consistent across peer traffic
