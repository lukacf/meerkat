# Meerkat Runtime Dogma

Status: Normative architecture doctrine
Scope: Current internal design and review work across runtime/control plane, comms, mob, shell helpers, public surfaces, machine schema, and generated authority
Supersedes: `docs/architecture/0.5/meerkat_0_5_dogma.md`

## Purpose

This document states the non-negotiable architectural rules for Meerkat's current runtime architecture.

It exists to prevent a repeat of the same failure mode:

- a formal core with too much semantic truth still living outside it

If a proposal violates this document, the default answer is not "maybe."
The default answer is "no" until the design is rewritten to fit.

## The Dogma

### 1. One semantic fact, one owner

Every semantically meaningful fact must have exactly one canonical owner.

Examples:

- lifecycle phase
- transition legality
- terminal outcome
- barrier membership
- async operation truth
- peer wiring truth
- surfaced result class

If the answer to "who owns this?" is "the machine, but also this helper/router/cache," the design is wrong.

### 2. Machines own semantics

If a fact changes what the system *is*, not just how it is delivered, that fact belongs in:

- a machine
- a composition
- a seam / handoff protocol

It must not live primarily in:

- helper wrappers
- adapters
- caches
- RPC handlers
- SDK shims
- background tasks

### 3. Shell owns mechanics, not meaning

Handwritten shell code may:

- execute IO
- hold handles
- buffer transport
- schedule tasks
- capture evidence
- rebuild projections

Handwritten shell code must not:

- invent lifecycle truth
- invent terminal classes
- decide barrier truth
- keep shadow transition tables
- infer semantic completion from side maps
- hide authoritative state in booleans, counters, or caches

### 4. One semantic condition, one canonical terminal path

The same semantic condition must not terminate differently depending on where it is detected.

Bad:

- loop-top timeout -> one terminal path
- in-call timeout -> another terminal path

Good:

- one typed semantic condition
- one canonical machine-owned terminalization rule

### 5. Typed truth, never folklore

If a distinction matters semantically, it must be typed.

Forbidden:

- JSON blobs whose `"kind"` field carries the real semantics
- parsing error messages to recover meaning
- string prefixes or naming conventions as primary identity
- "convention says this means unreachable / detached / completed"

### 6. App-facing APIs expose domain handles

Public control nouns are domain handles.

Examples:

- `job_id`
- `MemberRef`
- `member_id`
- `session_id` when a live bridge actually exists

Raw infrastructure identity is not a public noun.

If an app-facing caller must reason about a raw lifecycle ID, the API is leaking the wrong abstraction.

### 7. Raw infrastructure identity must be canonical

If raw infra identity escapes at all, it must be canonical.

That means:

- it resolves through the real owning registry
- it is not helper-local
- it is not "maybe canonical"
- it is not a local fallback disguised as shared truth

### 8. Optionality must not hide ownership uncertainty

`Option<T>` is not a valid substitute for "we have not established ownership."

If owner context is required, require it in the type.
If a path cannot supply it, that path cannot expose the contract.

### 9. Override-first at the composition seam

Policy is composed at the facade / factory seam.

Not in:

- provider adapters
- random helpers
- RPC handlers
- leaf surfaces

Overrides must be explicit, ordered, and semantically representable.

### 10. Inherit, disable, and set are different facts

If a seam needs to express:

- inherit lower-layer behavior
- explicitly disable behavior
- explicitly set behavior

then it requires a tri-state override type.

Plain `Option<T>` is forbidden when it collapses those facts.

### 11. Profiles are real owners, or they are not

If model/provider defaults come from `ModelProfile`, then `ModelProfile` owns operational defaults too.

Do not quietly widen a seam while pretending it is still just a capability catalog.
If ownership expands, the docs and code should say so plainly.

### 12. Dynamic policy must follow dynamic identity

If model, provider, session, or owner identity can change at runtime, any policy derived from that identity must be recomputed at that seam.

Bad:

- resolve policy once at build time
- hot-swap model later
- keep old policy silently

### 13. Derived projections are rebuildable, never authoritative

A projection is allowed only if:

- source truth is explicit
- rebuild trigger is explicit
- staleness policy is explicit
- semantic decisions do not depend on stale projection state

If stale projection can change behavior, it is not a projection.
It is shadow truth.

### 14. Local fallback state must never leak

Local registries, bookkeeping maps, receipts, and caches may exist internally.

They must not cross a boundary disguised as canonical truth.

### 15. Success means truthful completion

Do not classify an incomplete or failed execution as success just because the old path is convenient.

If the run did not complete truthfully, success-class terminalization is a lie.

### 16. Soft control and hard failure are different mechanisms

A soft steer is not a timeout failure.
A deadline escalation is not a terminal hard stop.
A policy warning is not a terminal outcome.

Do not collapse them.

### 17. Surfaces are skins, not authorities

CLI, REST, RPC, MCP, SDKs, and WASM are transport skins.

They may:

- format
- normalize
- adapt wire shapes

They must not:

- own semantic truth
- invent lifecycle meaning
- reinterpret terminal classes

### 18. Runtime conforms to machines

The runtime does not get to redefine the machine because an implementation path is awkward.

If the runtime needs semantic exception logic, either:

- the machine/composition model is incomplete, or
- the runtime is wrong

### 19. A feature must shrink ambiguity, not move it

If a proposal adds:

- a new type
- a new manifest field
- a new machine
- a new helper layer

but leaves the old ambiguity intact, it is architecture theater.

### 20. If we cannot say where a fact lives, the design is wrong

Every important behavior should reduce to one of:

- this machine owns it
- this composition owns it
- this protocol owns it
- this projection derives it
- this shell path executes mechanics only

If the answer is "sort of here and sort of there," the design is not ready.

## Shortcut Rejection List

Reject these shortcuts during review:

- stringly typed semantic classifiers
- helper-local lifecycle truth
- public APIs that leak raw infra IDs as user control handles
- local fallback registries that masquerade as canonical
