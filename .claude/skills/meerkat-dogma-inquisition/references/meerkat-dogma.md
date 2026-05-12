# Meerkat Dogma

Status: Draft primary architecture doctrine
Scope: All Meerkat runtime, surface, provider, SDK, protocol, storage, auth, mob, scheduling, WASM, and governance work
Companion: A longer commentary document will explain examples, tensions, and repair patterns

## Purpose

This document states the rules. It does not negotiate with convenience.

Meerkat exists to expose one coherent agent runtime through many surfaces. Its
architecture fails when semantic truth escapes into helpers, transports,
provider adapters, caches, stores, SDKs, test harnesses, or folklore.

If a design violates this document, the default answer is no. Rewrite the design
until the owner, seam, contract, and failure mode are explicit.

## Interpretation

Every rule below is a veto.

When two rules appear to pull in different directions, do not choose the easier
violation. Stop, name the semantic fact, name the owner, and add the missing
typed seam. Until the owner is clear, fail closed.

Compatibility is not authority. Compatibility mirrors are temporary debt. They
may exist only as read-only projections of canonical truth, with an explicit
expiry condition such as a date, contract version, or named breaking-release
gate, and only if they cannot change semantic behavior. Introducing one
requires explicit approval from the Highest Dogma Authority. Treat approval as
a rare exception, not a reviewer's local judgment call.

Permanent read models, presentation shapes, and interop adapters are not
mirrors. They are projections. They obey the projection rules.

## The Dogma

### 1. Authority Is Singular

Every semantically meaningful fact must have exactly one canonical owner.

This includes lifecycle phase, transition legality, terminal outcome, admission,
barrier membership, trust, identity, capability, policy, defaults, durability,
schema version, async operation truth, peer wiring truth, and surfaced result
class.

If the answer to "who owns this?" is "the machine, but also this helper",
"the profile, but also this adapter", "the runtime, but also this surface", or
"the store, but also this cache", the design is wrong.

If a fact changes what the system is, what transition is legal, what result is
true, or what policy applies, it must live in a named authority:

- a machine
- a composition
- a handoff protocol
- a typed catalog/profile/registry
- a declared store contract
- a public wire contract generated from the owning model

Unnamed ownership is not ownership.

### 2. Generated Machines Own Canonical Change

Canonical machine state must change only through generated machine authority.

Handwritten code must not:

- mutate canonical machine state directly
- implement a parallel reducer
- keep a shadow transition table
- decide transition legality
- emit semantic effects outside the owning transition
- invent terminal classification outside generated or machine-derived paths

Handwritten code may observe facts, classify raw inputs, hold handles, perform
IO, schedule tasks, execute effects, persist snapshots, and route evidence.

That permission ends at meaning. Any meaningful classification must lower into
a typed machine input, typed composition route, or typed handoff protocol. If no
such input exists, the model is incomplete. The runtime conforms to the machine;
the machine does not conform to an awkward runtime path.

### 3. Shells, Stores, and Projections Are Mechanical

Shell code owns mechanics, not truth.

Stores persist authority. They do not create it. Recovery replays or restores
machine-owned truth through declared transition, snapshot, journal, or migrator
contracts. A store must not seed lifecycle, terminality, policy, or ownership
from projection folklore.

Projections are allowed only when all of these are explicit:

- source truth
- rebuild trigger
- staleness policy
- boundary where the projection may be used

If stale projection state can change behavior, it is not a projection. It is a
second source of truth.

Local fallback state must never cross a boundary disguised as canonical truth.
Local registries, receipts, counters, caches, waiters, indexes, and side maps
may exist only as private mechanics unless a named authority owns their meaning.

### 4. Truth Is Typed, Identity Is Canonical

If a distinction matters semantically, it must be typed.

Forbidden as semantic authority:

- string prefixes
- enum names parsed from text
- provider-native field names in shared logic
- path naming conventions
- JSON blobs whose `kind` field carries the real meaning
- error-message parsing
- optional fields that hide missing ownership
- "convention says this means completed, detached, trusted, or unreachable"

Public control nouns must be domain handles. Raw infrastructure identity is not
an app-facing API unless that identity is canonical, owner-resolved, and the
right abstraction for the caller.

`Option<T>` is not a substitute for ownership. If owner context is required,
require it in the type. If a path cannot supply it, that path cannot expose the
contract.

### 5. Surfaces Are Thin Over The Shared Runtime

CLI, REST, RPC, MCP, SDKs, WASM, browser hosts, examples, and product binaries
are skins over shared runtime or explicit standalone substrates.

Surfaces may:

- format UX
- normalize transport details
- adapt wire shapes
- choose presentation
- expose friendly names

Surfaces must not:

- own semantic truth
- reconstruct runtime lifecycle
- invent handles, phases, defaults, or terminal classes
- maintain separate bootstrap semantics
- bypass shared surface/runtime helpers when a shared seam exists
- silently emulate runtime-backed behavior in standalone mode

Runtime-backed surfaces must lower into the shared runtime path. Standalone,
ephemeral, embedded, and browser paths must be explicit modes with explicit
limits, not secret substitutes for the product runtime.

User-facing syntax may be kinder than internal machinery. Its lowering must
still route through the canonical owner.

### 6. Providers and Policy Stay Behind Their Owning Seams

Meerkat is provider-agnostic at the session, runtime, tool, memory, mob, and
surface model level.

Provider-native transport, request, response, retry, streaming, realtime, and
auth mechanics belong in provider-specific crates or typed provider-extension
seams. Shared runtime code must not branch on provider folklore, provider-native
string fields, native response shapes, or prefix inference unless a typed
provider/catalog/profile contract owns that distinction.

Provider identity, model identity, capability truth, auth freshness, defaults,
and provider-specific policy must flow through their owning seams:

- model catalog and `ModelProfile`
- provider identity types
- realm auth binding and `AuthMachine`
- facade/factory composition
- typed provider-extension payloads where true portability is impossible

Policy is composed at the facade, factory, profile, auth, or composition seam.
Not in provider adapters, random helpers, RPC handlers, CLI flags, SDK defaults,
or leaf surfaces.

Inherit, disable, and set are different facts. If a seam needs all three, use a
tri-state type. Plain optionality is forbidden when it collapses policy meaning.

Dynamic policy must follow dynamic identity. If model, provider, auth binding,
session, realm, or owner identity changes, derived policy must be recomputed at
the owning seam.

### 7. Terminality and Faults Are Explicit

The same semantic condition must have one canonical terminal path.

Do not terminate differently because a condition was detected in a different
loop, task, provider callback, SDK parser, transport, or recovery path.

Success means truthful completion. Do not report success after a failed,
abandoned, partial, unauthoritative, or unverified execution because the old path
expects success.

Soft control and hard failure are different mechanisms. A steer is not a
timeout. A warning is not a terminal outcome. A retry delay is not completion. A
dropped receiver is not successful delivery. A logged error is not handled
failure.

Faults that affect semantics must be modeled or fail closed. This includes:

- cancellation while a transition is in progress
- resource shutdown and leaked task ownership
- deadline and timing semantics
- queue capacity, backpressure, and dropped observations
- external state drift
- schema/version skew
- adversarial or oversized input
- panic recovery
- cascading failure across seams
- test-mode behavior that differs from production semantics

If the system cannot explain what happened, who owns the consequence, and what
the caller may trust, the design is not complete.

### 8. Contracts, Crates, and Generation Are Ratchets

Public contracts must be derived from the authority path. Legacy compatibility
fields and shapes may exist only as temporary compatibility mirrors with an
expiry condition.

Generated schemas, SDK types, machine specs, formal models, OpenAPI, RPC
catalogs, event contracts, and governance ledgers must cover the production path
they claim to describe. A generated artifact that does not constrain production
behavior is theater.

Hand-authored semantic mirrors are forbidden.

A hand-authored compatibility mirror may exist only as a temporary, read-only
projection of a named authority, with a declared owner, freshness rule, enforced
drift test, explicit expiry condition, and explicit Highest Dogma Authority
approval. It must not feed decisions, defaults, recovery, terminality,
acceptance, or public authority.

If it has no expiry, make it a proper projection, generate it, or delete it.
If it can change behavior, it is a second source of truth.

Crate boundaries are authority boundaries:

- `meerkat-core` owns shared agent/runtime contracts and must stay free of
  network and filesystem authority.
- `meerkat-contracts` owns public wire shapes, error codes, schema emission, and
  contract versioning.
- `meerkat-runtime` owns the runtime control plane and `MeerkatMachine` path.
- `meerkat-session` owns session service implementations, not runtime authority.
- provider crates own provider-native mechanics, not shared semantics.
- surface crates own transport and UX, not meaning.
- facade crates compose; they do not hide new authorities.

Every feature must shrink ambiguity. Adding a type, manifest field, machine,
helper, SDK parser, compatibility flag, or generated file while leaving the old
semantic ambiguity alive is not progress.

## Shortcut Rejection List

Reject these during review:

- stringly typed semantic classifiers
- provider-specific logic leaking into shared runtime or surfaces
- helper-local lifecycle truth
- surface-owned request, session, auth, model, or terminal semantics
- SDK defaults that fabricate required facts
- public APIs that expose raw infra identity as a user control handle
- local fallback registries masquerading as canonical owners
- projections that make behavior decisions
- stores that recover from stale or compatibility metadata as authority
- success responses after hidden failure
- unbounded queues or dropped observations with semantic consequences
- generated contracts that do not govern production behavior
- compatibility mirrors introduced without Highest Dogma Authority approval
- compatibility mirrors without date or version expiry
- compatibility paths that can still affect meaning

## Final Test

For every important behavior, answer:

- What fact is being asserted?
- Who owns it?
- How is it typed?
- Which generated authority, contract, profile, registry, store contract, or
  handoff protocol carries it?
- What happens on failure?
- Which surfaces merely project it?

If any answer is "sort of", the design is wrong.
