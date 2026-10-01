# Owner composition: operational consequences

Status: implementation clarification following the four revision 7 protocol
reviews. This is not implementation or deployment acceptance. The frozen
revision 7 candidate remains unchanged at SHA-256
`751503730f93c57ac5a08446034d92ea153db52c3f5b1c2684e00832faac8515`.

## Witness availability is model availability

Witness unavailable: governed agents cannot invoke models. Every physical
model attempt discloses its exact protected context to a processor and needs
the independent acknowledgment described in revision 7 before release. A
previous attempt, an acknowledged turn start, a recently healthy witness or an
unchanged provider account does not authorize the next disclosure.

The witness is therefore an operational dependency for model invocation, like
the provider itself. A deployment must include this dependency in its service
objectives, outage projections and operator documentation. An unavailable
witness produces a bounded typed refusal or pending evidence state owned by
the actual operation; it does not select trusted-embedded behavior. This
consequence belongs in the ADR Consequences and governed deployment profile
when the implementation candidate updates those documents. It does not alter
the frozen revision 6 design evidence.

## Batching exact attempts

One witness round trip may acknowledge a batch of independently prepared
attempts. Each item retains its exact attempt identity, context and binding
digest, local entry, dependencies and protected recovery custody. Physical
release requires acknowledgment that includes that exact item. A batch-wide
success bit, another member's receipt, or a maximum sequence without a verified
matching chain cannot release it. Lost acknowledgment reconciles immutable
items; retrying the batch does not recreate effect handles.

No per-turn disclosure envelope is introduced in the first profile. A later
design may propose one with exact route/account/context, contributor changes,
count and duration limits, but it would need a new owner contract and review.
Calling the same provider again does not establish identical disclosure.

Acceptance measures model-anchor p95 with at least four concurrent members,
the percentage of real tool operations requiring synchronous anchoring, and a
12-iteration model/tool loop under both no packet loss and 1 percent packet
loss. These are additions to the existing minimum-host and total-runtime
budgets, not replacement microbenchmarks.

## Ingress acknowledgment during outage

An external webhook acknowledgment cannot imply admitted work before its exact
admission/dedup custody is independently preserved. Providers with short
acknowledgment deadlines may retry or stop delivery while the witness is down;
deployment documentation must describe that consequence.

A future adapter may explicitly acknowledge raw durable capture without
admission only under its own current ingress permission, protected storage
custody and truthful provider-facing contract. Such capture is not dedup
classification, work admission, payload hydration or permission to process.
Recovery still reconciles the affected ingress namespace before admission.
The initial implementation does not silently add this behavior to Slack or
other connectors.

## Unknown outcomes remain honest

Unknown destination outcomes are not retried automatically. An overlapping
resource may remain unavailable until its actual owner can reconcile it. A
state read is useful only where the destination contract proves the original
attempt's outcome; an empty lookup is not proof of non-entry. Notification
independence likewise requires a complete owner-declared dependency predicate.

A generic administrative acknowledgment cannot turn Unknown into non-entry or
release an overlapping resource fence. A future feature-specific authorized
acceptance of an unresolved outcome would retain Unknown, specify how late
effects are fenced or reconciled, and require its own reviewed transition.
No such transition is introduced by this addendum.
