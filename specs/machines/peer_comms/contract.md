# PeerCommsMachine

Status: normative `0.5` machine contract, first formal-spec draft

## Purpose

`PeerCommsMachine` owns peer-oriented transport admission and normalization
before peer traffic enters runtime admission.

It is the authoritative owner of:

- peer trust/auth snapshotting
- peer-envelope normalization
- peer-input classification
- peer-lifecycle normalization
- typed `PeerInput` candidate submission toward runtime admission

It is **not** the owner of:

- runtime queue ordering
- delegated-work lifecycle completion
- transcript projection of peer traffic
- ordinary prompt/work admission
- external tool lifecycle

## Scope Boundary

This machine begins at raw peer transport ingress or peerless external event
ingress that is still semantically peer/comms-owned.

It ends at typed `PeerInput` candidate submission through the runtime-owned
admission surface.

Important `0.5` rule:

- peer normalization/classification happens here
- the classified-inbox mechanism is not the architectural goal
- the responsibility survives even as the old mechanism is deleted

## Authoritative State Model

For one runtime/session owner, the machine state is the tuple:

- `trusted_peers: Set<PeerId>`
- `raw_item_state: Map<RawItemId, PeerIngressState>`
- `raw_item_kind: Map<RawItemId, RawPeerKind>`
- `raw_item_peer: Map<RawItemId, Option<PeerId>>`
- `classified_as: Map<RawItemId, Option<PeerInputClass>>`
- `trusted_snapshot: Map<RawItemId, Bool>`
- `submission_queue: Seq<RawItemId>`

`PeerIngressState` is the closed state set:

- `Absent`
- `Received`
- `Dropped`
- `Submitted`
- `Delivered`

`RawPeerKind` is the closed normalization alphabet:

- `Message`
- `Request`
- `Response`
- `SilentRequest`
- `PeerLifecycleAdded`
- `PeerLifecycleRetired`
- `PlainEvent`
- `Ack`

`PeerInputClass` is the target classified output family:

- `ActionableMessage`
- `ActionableRequest`
- `Response`
- `SilentRequest`
- `PeerLifecycleAdded`
- `PeerLifecycleRetired`
- `PlainEvent`

Important `0.5` rule:

- delegated child completion is **not** a `PeerCommsMachine` class; it moves to
  `OpsLifecycleMachine`

## Input Alphabet

The closed external input/command alphabet for this machine is:

- `TrustPeer(peer_id)`
- `UntrustPeer(peer_id)`
- `ReceivePeerEnvelope(raw_item_id, peer_id, raw_kind)`
- `ReceivePeerlessPlainEvent(raw_item_id)`
- `DropInvalid(raw_item_id)`
- `SubmitTypedPeerInput(raw_item_id)`
- `DeliverSubmitted(raw_item_id)`

Notes:

- `ReceivePeerEnvelope` covers message/request/response/silent/lifecycle/ack
  envelopes from trusted transport peers
- `ReceivePeerlessPlainEvent` covers external event-listener ingress that is
  normalized here and then admitted as work elsewhere
- `SubmitTypedPeerInput` is where classification becomes a runtime-facing typed
  candidate

## Effect Family

The closed machine-boundary effect family is:

- `EmitAck(raw_item_id)`
- `SubmitPeerInputCandidate(raw_item_id, peer_input_class)`
- `DropPeerIngress(raw_item_id, reason)`
- `EmitPeerNotice(kind, detail)`

Architecture rules:

- acks are transport-side effects; they are never peer inputs
- classification is attached to the submitted peer candidate and is not
  recomputed later by downstream owners

## Transition Relation

### Trust store mutation

1. `TrustPeer(peer_id)`

State updates:

- add `peer_id` to `trusted_peers`

2. `UntrustPeer(peer_id)`

State updates:

- remove `peer_id` from `trusted_peers`

Important:

- this affects future ingress
- it does not retroactively invalidate already submitted peer inputs

### Raw ingress

3. `ReceivePeerEnvelope(raw_item_id, peer_id, raw_kind)`

Preconditions:

- `raw_item_state[raw_item_id] = Absent`

State updates:

- `Absent -> Received`
- set `raw_item_peer[raw_item_id] := peer_id`
- set `raw_item_kind[raw_item_id] := raw_kind`

4. `ReceivePeerlessPlainEvent(raw_item_id)`

Preconditions:

- `raw_item_state[raw_item_id] = Absent`

State updates:

- `Absent -> Received`
- set `raw_item_peer[raw_item_id] := None`
- set `raw_item_kind[raw_item_id] := PlainEvent`

### Drop vs submit

5. `DropInvalid(raw_item_id)`

Preconditions:

- `raw_item_state[raw_item_id] = Received`

State updates:

- `Received -> Dropped`

Effect:

- `DropPeerIngress(raw_item_id, reason)`

6. `SubmitTypedPeerInput(raw_item_id)`

Preconditions:

- `raw_item_state[raw_item_id] = Received`
- if `raw_item_peer[raw_item_id] = Some(peer_id)`, then `peer_id ∈ trusted_peers`
- `raw_item_kind[raw_item_id] != Ack`

State updates:

- classify the item deterministically into one `PeerInputClass`
- `Received -> Submitted`
- store `classified_as[raw_item_id]`
- store `trusted_snapshot[raw_item_id]`
- append `raw_item_id` to `submission_queue`

Effect:

- `SubmitPeerInputCandidate(raw_item_id, peer_input_class)`

Hard rules:

- `Ack` is never submitted as runtime peer input
- peerless ingress in this machine is limited to `PlainEvent`

7. `DeliverSubmitted(raw_item_id)`

Preconditions:

- `raw_item_state[raw_item_id] = Submitted`
- `raw_item_id = Head(submission_queue)`

State updates:

- `Submitted -> Delivered`
- remove `raw_item_id` from `submission_queue`

## Classification Rules

The target classification relation is:

- `Message -> ActionableMessage`
- `Request -> ActionableRequest`
- `Response -> Response`
- `SilentRequest -> SilentRequest`
- `PeerLifecycleAdded -> PeerLifecycleAdded`
- `PeerLifecycleRetired -> PeerLifecycleRetired`
- `PlainEvent -> PlainEvent`
- `Ack -> dropped, never submitted`

## Invariants

The machine must maintain:

1. trust snapshot stickiness

- once a raw item has been submitted, its admission-time trust snapshot is
  authoritative even if trust later changes

2. submitted items always have a class

- `Submitted` and `Delivered` imply one stored `PeerInputClass`

3. `Ack` never reaches runtime admission

- `Ack` may cause transport effects, but it is not a typed peer input

4. submission queue contains only submitted items

- nothing `Dropped` or `Absent` may appear in the submission queue

5. peerless items are plain events only

- other peer/comms items must carry peer identity

## Model-check candidates

Best candidates for formal checking:

- no untrusted peer item is submitted
- submission queue/state consistency
- classification remains stable after submission
- ack never appears in runtime-facing peer submission

