---
title: "Delivery, Interaction, and Durable Operations"
description: "Architecture decision separating one-way comms delivery, conversational interaction, and machine-owned durable operations."
icon: "route"
---

# Delivery, Interaction, and Durable Operations

Status: Accepted. Supervisor rotation is the first concrete operation; a
reusable platform operation API is intentionally deferred.

## Context

Meerkat needs three different temporal contracts. Treating all three as raw
request/response couples correctness to a live reply route and makes timeout or
caller cancellation ambiguous.

- **Delivery** is a signed, routed, fire-and-forget envelope. This remains the
  primitive owned by `meerkat-comms`.
- **Interaction** is optional, short-lived request/response correlation for
  conversational behavior. Existing `PeerResponse` and `in_reply_to` handling
  remain a compatibility interaction projection.
- **Operation** is a durable machine-owned command with a stable ID, observable
  progress, and a terminal receipt. Its truth cannot depend on a live
  interaction.

## Decision

Supervisor rotation is a durable operation scoped to one member session.

```text
SubmitSupervisorRotation(operation_id, target)  // one-way delivery
                     ↓
       member machine persists and advances
                     ↓
ObserveSupervisorRotation(operation_id)         // read-only interaction
       → pending | completed | rejected
```

The member records the operation ID, full previous and target authority, and
the first pending phase before changing live trust. That same transition fences
the old epoch. A session-owned worker advances revoke, publish, and completion;
process recovery restarts a pending worker from durable state.

Submission has no semantic ACK. An exact retry with the same operation ID and
target observes or resumes the same operation. Reusing the ID for another
target is rejected. Observation may use the existing interaction transport,
but a timeout means only that no receipt was observed before the deadline: it
does not cancel, roll back, or obscure the durable operation.

Terminal receipts remain keyed by operation ID across later rotations. The
current bound supervisor may observe that history, so rotating authority again
does not make an earlier completion or rejection unreachable.

The mob supervisor persists the target and operation ID before the first
one-way submission. It observes completion as the new authority and commits
the local authority change only after every affected member reports completion.

No trust edge may be created solely to deliver an ACK to a revoked supervisor.
Optional completion notification is one-way and non-authoritative.

## Consequences

- `meerkat-comms` remains fundamentally one-way and does not become a general
  RPC substrate.
- Supervisor rotation survives sender timeout, handler cancellation, and cold
  restart without rollback-by-timeout.
- Existing synchronous-looking host APIs become submit-and-observe adapters;
  their result shape can remain compatible.
- Ordinary current-supervisor verification may continue using the existing
  interaction compatibility path; it is not rotation authority.
- A reusable `OperationId → OperationState → TerminalReceipt` platform surface
  is follow-up work after this concrete contract has proven its shape.
