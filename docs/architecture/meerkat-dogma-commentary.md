# Meerkat Dogma Commentary

Status: Draft companion commentary
Primary doctrine: `docs/architecture/meerkat-dogma.md`

## Purpose

The dogma states the law. This commentary explains how to apply it without
turning the law into theater.

The commentary is not softer doctrine. It is practical doctrine: examples,
repair shapes, review questions, and guidance for resolving tension between
rules. If the primary dogma says no, this document does not make it yes. It
explains how to redesign the work until the answer can become yes.

## How To Read This Book

Read every chapter through the same question:

> Where does the semantic fact live?

Most Meerkat architecture failures are variations on one sin: a fact has an
official owner and an unofficial owner. The unofficial owner is usually a
helper, surface, cache, provider adapter, SDK parser, recovery path, or
compatibility field introduced for convenience. The unofficial owner is almost
always easier to write. That is why it is dangerous.

This book uses these terms consistently:

- **Authority**: the named owner of semantic truth.
- **Shell**: code that performs mechanics around an authority.
- **Projection**: a rebuildable, read-only view of authority-owned truth.
- **Mirror**: a duplicate compatibility shape. Mirrors are temporary debt and
  require Highest Dogma Authority approval plus expiry.
- **Surface**: a transport, UX, SDK, host, or product skin over an authority.
- **Seam**: a typed boundary where ownership, policy, effects, or evidence pass
  between authorities or between authority and shell.

## Resolving Tension Between Rules

Dogma rules do not form a priority list where lower rules can be sacrificed.
When two rules appear to pull in different directions, the design is missing a
seam, owner, or type.

Use this order:

1. Name the semantic fact.
2. Name the current owner claimed by the design.
3. Name every other path that can create, alter, recover, infer, or surface that
   fact.
4. Decide which path is the authority. Delete, lower, or project the rest.
5. If multiple rules still pull, add a typed seam that preserves both truths
   instead of collapsing them.
6. If compatibility is demanded, require Highest Dogma Authority approval,
   expiry, drift tests, and proof that compatibility cannot affect behavior.
7. If the system cannot explain failure, staleness, or recovery, fail closed.

Common tensions have standard resolutions:

| Tension | Wrong Resolution | Correct Resolution |
| --- | --- | --- |
| UX wants a friendly shortcut; authority wants a typed handle | Let the surface synthesize the handle | Let the surface accept friendly syntax, then resolve through the canonical owner |
| Provider has a native concept; runtime wants provider agnosticism | Branch on provider strings in shared runtime | Add a typed provider-extension seam or keep the concept inside the provider crate |
| Read performance wants denormalization; authority wants one source of truth | Let the index become fallback truth | Define the index as a projection with source, rebuild trigger, staleness rule, and non-authority boundary |
| Old clients need a field; dogma forbids mirrors | Keep the legacy field forever | Add a temporary compatibility mirror with approval, expiry, drift test, and no behavior influence |
| A task failed after emitting partial evidence; caller expects success | Return success and log the failure | Model or surface the partial failure truth through the canonical terminal path |
| A shell can observe something the machine cannot | Let the shell update semantic state | Convert the observation into a typed input or declare the machine incomplete |

## Chapter 1: Authority Is Singular

### Thesis

A Meerkat fact has one throne.

Every semantically meaningful fact must have exactly one canonical owner. Not
one preferred owner and several helpful replicas. Not one runtime owner and a
surface override. Not one machine truth and one recovery truth. One owner.

This rule is first because every other rule depends on it. Generated machines
cannot own canonical change if helpers also own transition legality. Surfaces
cannot be thin if they reconstruct lifecycle. Provider seams cannot hold if
shared runtime branches on provider folklore. Stores cannot be mechanical if
recovery metadata becomes terminality truth.

The question is not where the fact is convenient to compute. The question is:
who has the right to say what is true?

If that answer is unclear, the design must fail closed.

### What The Rule Forbids

Rule 1 forbids shared authority over meaning.

It forbids helper-local lifecycle truth: a runtime task, waiter, cache, or loop
deciding that a session is running, cancelled, failed, detached, completed, or
recoverable when the owning machine has not said so.

It forbids surface authority: CLI, REST, RPC, MCP, SDKs, WASM bindings, browser
hosts, or examples inventing session status, defaults, result classes,
admission rules, auth behavior, provider selection, or terminal outcomes.

It forbids store authority drift: event stores, projectors, snapshots, indexes,
receipts, and recovery paths inferring canonical state from stale projection
rows, convenience summaries, or legacy metadata.

It forbids provider authority leaks: provider mechanics deciding shared runtime
semantics through native field names, stop reasons, error strings, stream chunk
shapes, or request conventions.

It forbids policy split-brain: profiles, catalogs, provider adapters, CLI flags,
SDK defaults, auth helpers, and runtime builders each deciding pieces of model
capability, auth freshness, trust, tool permission, retry behavior, or defaults.

It forbids mob ambiguity: supervisors, rosters, bridges, flows, peer transports,
and message routers each maintaining their own truth about membership,
delegation, handoff, peer identity, task ownership, or barrier completion.

It forbids tool ambiguity: tool dispatch, builtin tools, skills, hooks, memory,
and policy checks each deciding admission or capability without lowering into
one typed owner.

Most of all, it forbids "the same fact, but optimized over here."
Optimization may create projections. It may not create a second sovereign.

### Violation Examples

#### Provider Stream As Terminal Authority

A runtime loop marks a run completed because the provider stream ended cleanly,
while the machine still has pending tool operations.

The stream observed evidence. It did not own terminality.

Correct shape: the provider path lowers stream completion into typed evidence.
The machine or terminal handoff protocol decides whether the run completed,
failed, remained pending, or needs cleanup.

#### SessionService As Parallel Lifecycle

`SessionService` keeps an internal status enum that can disagree with
`MeerkatMachine`.

The service may host, persist, and route sessions. It must not become a
parallel lifecycle authority.

Correct shape: the service projects machine-owned lifecycle or submits typed
inputs to the machine. It does not decide lifecycle phase itself.

#### Projector As Recovery Authority

A projector writes `last_status = "failed"` and recovery later trusts that
field to decide whether a session is terminal.

The projector created a read model. Recovery turned it into authority.

Correct shape: recovery uses declared machine snapshots, transition journals,
events, or migrators. The projector remains rebuildable and non-authoritative.

#### SDK As Default Owner

An SDK sees a missing model field and silently chooses a default provider.

A user-facing convenience fabricated model policy.

Correct shape: defaults come from the owning profile, catalog, facade, realm
config, or explicit public contract. SDKs expose absence, mismatch, or server
truth. They do not invent it.

#### Mob Transport As Roster Owner

A mob bridge considers a worker detached because its transport disconnected,
while the roster still considers the worker assigned.

Transport liveness is evidence. Assignment truth belongs to the roster,
composition, or handoff protocol.

Correct shape: the transport submits typed disconnect evidence. The mob
authority decides whether the member is detached, reconnecting, retired,
failed, or still assigned.

### Correct Solution Shapes

Start by naming the fact. Do not name the module first.

Examples:

- session lifecycle phase
- tool admission
- provider capability
- mob worker membership
- realm trust
- terminal result class
- async operation truth
- public session identity

Then name the owner.

If the fact is transition legality, lifecycle, terminality, or canonical state,
the owner is usually a generated machine or machine composition.

If the fact is model capability, provider identity, default behavior, or
provider policy, the owner is a typed catalog, profile, registry, or
provider-extension seam.

If the fact is auth freshness, realm trust, or credential binding, the owner is
the auth machine, realm binding, or token-store contract. The token store
persists credentials. It does not decide trust.

If the fact is persistence durability or replay semantics, the owner is a
declared store contract. A store may own append order, snapshot version, replay
boundary, and migration version. It must not own lifecycle meaning unless that
meaning is explicitly part of the canonical contract.

If the fact is public wire shape, the owner is the public contract generated
from or constrained by the authority path.

Once the owner is named, force all other code into one of three roles:

- input adapter
- effect executor
- projection

Input adapters classify raw material and lower it into typed inputs. Provider
chunks, CLI flags, RPC payloads, SDK calls, hook returns, and tool results
belong here.

Effect executors perform IO after authority permits it. They call providers,
run tools, persist events, send messages, schedule tasks, and emit observations.

Projections reshape owned truth for query, display, indexing, compatibility, or
interop. They declare source truth, rebuild trigger, staleness policy, and
boundary. They do not decide behavior.

### Review Questions

- What exact fact is being asserted?
- Who is allowed to change it?
- Can two modules answer this question differently?
- Is this a machine fact, profile fact, catalog fact, store-contract fact, auth
  fact, mob-composition fact, or public-contract fact?
- Is any helper, cache, waiter, task, callback, surface, SDK, or test fixture
  maintaining a local copy that can affect behavior?
- Does recovery rebuild authority from canonical events, snapshots, or
  migrators, or from projection folklore?
- Does a provider-native field leak into shared runtime meaning?
- Does a surface lower into the shared runtime path, or reconstruct semantics
  because that was easier?
- Can stale data change admission, defaults, terminality, identity, trust,
  routing, or policy?
- If the owner disappeared, would another component quietly pretend to know the
  truth?

### Tension Notes

Rule 1 and Rule 2: naming the owner is not enough. If the owner is a machine,
canonical change must go through generated machine authority. Handwritten
reducers are not implementation detail. They are rival authority.

Rule 1 and Rule 3: stores and projections can look authoritative because they
are durable or queryable. Durability is not sovereignty. A store owns
persistence guarantees; the machine owns meaning.

Rule 1 and Rule 4: ownership must be typed. A convention, string prefix,
optional field, path shape, or provider-native name is not an owner.

Rule 1 and Rule 5: surfaces may be friendly, but they must not become clever.
User syntax can differ from internal machinery only if lowering returns to the
canonical owner.

Rule 1 and Rule 6: provider-specific crates may own provider-native
translation. They do not own Meerkat semantics. Shared runtime receives typed
facts, not provider folklore.

Rule 1 and Rule 7: terminality is where split authority becomes most dangerous.
A timeout, dropped stream, cancelled task, provider error, or failed tool must
lower into one terminal authority.

Rule 1 and Rule 8: generated artifacts must constrain the real authority path.
A generated schema that describes one truth while production follows another is
documentation theater.

### Maxims

- One fact, one owner.
- Evidence is not authority.
- A projection that decides behavior is a usurper.
- Convenience defaults are policy in disguise.
- If two paths can disagree, the architecture is already lying.

## Chapter 2: Generated Machines Own Canonical Change

### Thesis

Canonical machine state changes only through generated machine authority.

This rule is the operational form of "machines own semantics." It is not enough
for a machine model to exist, or for production code to vaguely resemble it. If
handwritten code can decide the same transition, mutate the same state, or
classify the same terminal outcome, the machine is decoration.

Generated authority owns the semantic core:

- what states exist
- which inputs are valid
- which transitions are legal
- what canonical state mutation occurs
- which semantic effects are emitted
- which terminal class follows from machine truth

Handwritten code is still necessary. It receives raw inputs, waits on tasks,
performs IO, schedules work, persists snapshots, routes messages, and executes
effects. But it must stop at the border of meaning. Once a fact changes what
the system is, what transition is legal, what obligation exists, or what result
the caller may trust, it must cross into the machine as a typed input, typed
composition route, or typed handoff protocol.

The runtime conforms to the generated machine. The generated machine must never
be bent to match an awkward handwritten runtime path.

### What The Rule Forbids

Rule 2 forbids any handwritten path that becomes a second reducer.

A reducer is any code that looks at current semantic state plus an event and
decides the next semantic state. It does not have to be named `reduce`. It may
be a helper, actor method, RPC handler, recovery routine, test harness,
callback, retry loop, status mapper, or provider adapter. If it decides
canonical state, transition legality, terminality, or semantic effects, it is a
reducer.

Forbidden patterns include:

- directly setting canonical machine fields from handwritten code
- matching on lifecycle state and manually advancing to another lifecycle state
- maintaining a shadow transition table in an actor, handler, or helper
- duplicating machine states in local enums, booleans, maps, or status structs
- deciding that a transition is legal because a shell-local condition says so
- emitting semantic effects that the owning transition did not produce
- classifying terminal results through ad hoc branches instead of generated or
  machine-derived classifiers
- recovering state from projection fields, compatibility metadata, or local
  cache truth
- making a test pass by forcing machine state instead of driving the machine
  through legal inputs

The decisive test:

> If removing the handwritten shell would remove semantic truth, generated
> authority is incomplete.

### Shell Classification And Typed Inputs

The shell is allowed to decide when to call the machine. It may receive raw
evidence and classify it into a typed machine input.

Legitimate shell classification:

- a JSON-RPC request arrives; the handler maps it to a typed runtime command
- a task exits; the owner records raw completion evidence and submits typed
  feedback
- a provider stream yields a chunk; the adapter maps it to typed observation
  data
- a timer fires; the scheduler routes a typed timeout input
- a store loads a snapshot; recovery submits declared replay, restore, attach,
  or migrator inputs

The permission ends there. The classified output must become a typed machine
input, typed composition route, or typed handoff protocol event.

If the shell has to say "this is kind of like completed, except from recovery"
or "this provider error should probably close the run", the model is missing a
type. Add the input. Add the route. Add the protocol. Do not encode missing
semantics in shell folklore.

A proper typed input carries the facts the machine needs to judge legality:
domain identity, owner identity, correlation IDs, generation or fence tokens
when relevant, payload evidence, and the semantic category being asserted.

### Handoff Protocols

Many semantic transitions do not finish inside one synchronous function call. A
machine may authorize work, emit an effect, and depend on an owner to realize
that effect asynchronously. That seam is dangerous because the shell is tempted
to decide what happened later.

A handoff protocol closes the seam.

Correct shape:

1. The machine transition emits an effect.
2. The effect declares a handoff protocol when later owner feedback is
   semantically meaningful.
3. The composition names the realizing owner, allowed feedback inputs,
   correlation fields, and closure policy.
4. Handwritten owner code performs the IO or async work.
5. Owner feedback returns through generated protocol helpers.
6. The generated machine or composition path classifies the result and advances
   canonical state.

The shell may report evidence: task exited, channel closed, provider returned
error, tool completed, peer acknowledged, timeout elapsed. The shell may not
invent the semantic class: completed, failed, cancelled, abandoned, consumed,
detached, terminal, retryable, or authoritative.

### Violation Examples

#### Direct Lifecycle Mutation

A runtime task exits and handwritten code sets `phase = Completed`, emits a
success event, and clears waiters.

The shell observed a task fact, then invented lifecycle and terminal truth.

Correct shape: the shell submits typed completion evidence through generated
input or handoff feedback. The generated transition decides whether the session
completed, failed, cancelled, or remains obligated.

#### Shadow Transition Helper

An actor has `advance_flow_run()` with a handwritten `match` over `Pending`,
`Running`, `Waiting`, `Succeeded`, and `Failed`, while the catalog also defines
flow lifecycle.

The helper is a parallel reducer, even if its branches look reasonable.

Correct shape: convert each meaningful stimulus into generated machine input:
dependency satisfied, branch failed, quorum reached, retry exhausted, or owner
feedback received. Local maps become projections or private mechanics updated
from generated transitions.

#### String Terminal Classifier

An RPC handler maps internal status strings to public result classes: `"done"`
becomes success, `"stopped"` becomes cancelled, `"error"` becomes failure.

Terminal classification escaped the machine and became surface folklore.

Correct shape: use generated terminal classifiers or machine-derived typed
result classes. The surface may format the result; it must not decide it.

#### Recovery From Projection Truth

Recovery reads a denormalized `last_status` field from a read model and seeds
canonical state as running, detached, or completed.

A projection became authority.

Correct shape: recover from declared snapshot, journal, replay, or migrator
contracts. For transient runtime facts, observe the live fact and submit typed
attach, detach, or rebind input through generated authority.

### Correct Solution Shapes

Add a machine input when the shell is classifying a real semantic event that
the machine cannot currently express.

Add a handoff protocol when a machine-owned effect crosses into async owner
work and later feedback matters.

Promote cross-machine truth into a composition when no single machine owns the
full semantic fact. Acceptance, execution, consumption, barrier satisfaction,
and terminal result alignment often live at composition seams.

Convert local state into a projection when it exists for speed, lookup,
formatting, or transport. A projection must name source truth, rebuild trigger,
staleness policy, and use boundary.

Move provider-specific mechanics behind provider seams. Native request and
response forms are boundary facts, not shared runtime semantics.

Generate the classifier when handwritten code maps machine outcomes to
terminal classes. Exhaustiveness matters. Default arms and "unknown means
success" are architectural sins.

Strengthen tests around the authority path: illegal inputs fail closed,
handoff feedback cannot bypass the protocol, and recovery reaches the same
truth as live execution.

### Review Questions

- What canonical fact changes here?
- Which generated machine, composition, or handoff protocol owns that change?
- Is handwritten code mutating canonical state directly?
- Is this function deciding transition legality, or merely routing typed
  evidence?
- Does any local enum, boolean, map, status field, or cache duplicate machine
  state?
- Does terminal classification come from generated or machine-derived code?
- If this is async work, where is the handoff protocol and generated feedback
  helper?
- If the shell classifies raw input, what typed input does it lower into?
- If no typed input exists, why is the model considered complete?
- Could recovery replay this path without hidden shell-local truth?
- Would deleting this handwritten helper delete meaning, or only mechanics?

### Tension Notes

Rule 2 and Rule 1: for machine facts, singular authority means generated
authority. If there is tension between a convenient helper and the machine, the
helper loses.

Rule 2 and Rule 3: shell mechanics and projections are allowed only while they
remain mechanical. The shell observes and routes; the machine decides.

Rule 2 and Rule 4: a generated machine input is only authoritative if it
carries typed truth. String classifiers and optional owner context smuggle
handwritten semantics back into the path.

Rule 2 and Rule 7: terminal classifiers must be generated or machine-derived.
Fault evidence may come from shell code, but fault meaning belongs to the
machine or protocol.

Rule 2 and Rule 8: if the generated machine does not govern the production
transition, fix production or fix generation. Do not preserve a hand-authored
mirror and call it compatibility.

### Maxims

- The shell may bring evidence; only the machine may pronounce meaning.
- If no typed input exists, the model is incomplete.
- A handwritten reducer is a second source of truth wearing a helper's cloak.
- Async work may leave the machine; semantic closure must return through
  protocol.
- Generated authority is not documentation. It is the law.

## Chapter 3: Shells, Stores, and Projections Are Mechanical

### Thesis

Shells, stores, and projections are allowed to move truth, preserve truth, and
display truth. They are not allowed to create truth.

Rule 3 exists because many architectural failures enter through "just a cache",
"just recovery", "just a read model", "just local state", or "just
compatibility metadata." These components feel harmless because they are not
the central machine. That is exactly why they are dangerous. They sit near
authority, often run during exceptional paths, and can quietly become authority
when the real owner is absent, slow, stale, or inconvenient.

A mechanical component does not decide what happened. It carries evidence to
the owner that can decide.

### What The Rule Forbids

This rule forbids any shell, store, projection, cache, receipt table, local
registry, waiter map, recovery helper, or compatibility field from becoming an
undeclared owner of semantic truth.

It forbids:

- deriving lifecycle phase, terminality, policy, trust, ownership, or recovery
  outcome from stored or projected convenience fields
- treating stale projection state as permission to continue, terminate, retry,
  recover, or accept input
- using local fallback state as a substitute for canonical session, runtime,
  provider, peer, or machine state
- letting read models make behavioral decisions instead of serving queries and
  presentation
- recovering canonical state from denormalized views, indexes, SDK shadows,
  compatibility metadata, or provider-shaped artifacts
- allowing shell code to patch up missing authority by inventing defaults,
  inferred ownership, or synthetic outcomes
- allowing compatibility metadata to outlive its expiry or influence behavior

The forbidden move is always the same: a mechanical component starts answering
semantic questions.

### Violation Examples

#### Projection Becomes Authority

A session list projection stores `status = "completed"` for fast UI display.
Later, REST checks that projected status to decide whether cancellation is
legal.

The projection may display completion. It may not decide transition legality.

Correct shape: REST sends a typed cancellation input and lets the authority
accept or reject it. The projection remains a read model.

#### Store Creates Recovery Truth

A recovery path loads the most recent snapshot, notices a terminal-looking
event, and marks the run complete without replaying through the declared
transition, snapshot, journal, or migrator contract.

The store preserved bytes, not meaning.

Correct shape: recovery restores authority through the contract that owns
recovery semantics, or fails closed.

#### Stale Read Model Changes Behavior

A worker checks an eventually consistent `active_sessions` index before
dispatching a tool call. If the session is absent, the worker drops the call as
"session gone."

Eventual consistency is acceptable for query latency, not semantic disposal.

Correct shape: the worker uses the canonical session/runtime owner to validate
dispatch. The index may help find candidates, but absence from the index cannot
prove semantic absence unless that exact boundary is owned and declared.

#### Local Fallback Registry Masquerades As Canonical

A surface keeps a local map from request IDs to session handles. When the
shared runtime cannot resolve a handle, the surface falls back to its local map
and continues.

The local map is correlation state. It cannot override canonical handle
authority.

Correct shape: the local map correlates in-flight requests. If canonical
resolution fails, the surface reports failure or re-enters the declared
authority path.

#### Compatibility Metadata Drives New Semantics

A legacy event field `legacy_terminal = true` exists for old clients. New
recovery code reads it to decide whether a run reached terminal state.

Compatibility metadata has crossed into authority.

Correct shape: legacy fields are temporary, read-only compatibility mirrors
with expiry and approval. New behavior reads canonical terminal fact from the
owning machine or event contract.

### Correct Solution Shapes

Every projection must declare:

- source truth
- rebuild trigger
- staleness policy
- allowed use boundary

Source truth must be a machine state, event contract, snapshot contract, typed
registry, profile, catalog, or other named authority. A projection without
named source truth is usually an undocumented second model.

Rebuild triggers may be event append, snapshot update, schema migration,
provider callback normalization, explicit invalidation, or periodic refresh.
The trigger must be tied to the authority path, not incidental shell activity.

Staleness policy may be strongly consistent, eventually consistent, best
effort, cached with TTL, invalidated by version, or stale for presentation
only. It must state what stale data may not do.

Use boundary may be UI display, list query, search, diagnostics, audit export,
provider interop, human summary, metrics, or compatibility response. Outside
that boundary, the projection is suspect.

Read models are legitimate permanent projections. They reshape canonical truth
for query, performance, display, search, diagnostics, or interop.

Compatibility mirrors are different. They preserve an old semantic shape
temporarily for legacy consumers. They require explicit Highest Dogma Authority
approval, expiry, drift tests, and prohibition against feeding behavior.

### Review Questions

- What semantic fact is this shell, store, or projection carrying?
- Who owns that fact before this component sees it?
- Can this component change behavior when local state is stale, missing, or
  wrong?
- Is recovery restoring machine-owned truth, or reconstructing meaning from
  residue?
- Is this read model used only for query/presentation, or does it decide
  transitions, defaults, admission, terminality, or policy?
- What is the projection source truth, rebuild trigger, staleness policy, and
  allowed use boundary?
- Is local fallback state private mechanics, or can it escape as canonical
  truth?
- Is compatibility metadata being read by new code to decide behavior?
- If the projection vanished, would the system lose performance and display
  quality, or semantic correctness?

### Tension Notes

Rule 3 and Rule 1: do not ban projections; prevent projections from owning
meaning.

Rule 3 and Rule 2: stores persist and restore machine-owned truth only through
declared machine, snapshot, journal, or migrator contracts. Storage durability
is not transition authority.

Rule 3 and Rule 5: surface-local request maps, SDK shadows, browser caches, and
CLI state must remain private mechanics.

Rule 3 and Rule 7: stale indexes, dropped waiters, missing receipts, and
recovery guesses must not become terminal outcomes.

Rule 3 and Rule 8: generated or not, an artifact constrains production truth
only if it is the authority path. Otherwise it is a projection, and projections
must stay in their lane.

### Maxims

- A projection may answer "what should I show?" It may not answer "what is
  true?"
- A store preserves authority; it does not become authority.
- Stale data may be visible. It must not be decisive.
- Local fallback state is private plumbing until a named authority owns it.
- Compatibility metadata is not memory. It is debt with an expiry date.

## Chapter 4: Truth Is Typed, Identity Is Canonical

### Thesis

A semantic fact is not real in Meerkat until it has a type, an owner, and a
canonical identity path.

Strings, paths, provider fields, JSON shapes, optional values, and naming
conventions may carry evidence. They must not carry law. If code needs to
distinguish completed from detached, trusted from merely known, reachable from
absent, OpenAI from Anthropic, skill A from another skill with the same name, or
member identity from runtime incarnation, that distinction belongs in a named
type.

Folklore scales badly. Once meaning lives in string prefixes, missing fields,
provider-native names, or "everyone knows this path means X", every surface and
helper starts carrying a private copy of the architecture.

Typed truth is the antidote. Canonical identity is the boundary.

### What The Rule Forbids

Rule 4 forbids using unowned representation as semantic authority.

Forbidden patterns include:

- parsing model, provider, realm, peer, skill, session, or operation meaning
  from string prefixes
- treating enum variant names, display labels, log messages, or error text as
  machine-readable truth
- letting provider-native request or response fields drive shared runtime
  behavior
- using path layout, file names, command names, or URL shape as lifecycle or
  ownership truth
- accepting generic JSON where `kind`, `type`, or an ad hoc field secretly
  determines behavior
- inferring trust, completion, detachment, reachability, or ownership from
  convention
- using `Option<T>` when the real question is which owner supplies the fact
- exposing raw infrastructure identifiers as public control handles when the
  caller needs a domain handle
- treating provenance as decorative audit text instead of typed evidence bound
  to an owner

The target is not strings themselves. Strings are fine for text. The violation
begins when a string becomes the place where architecture hides.

### Violation Examples

#### Provider Folklore In Shared Logic

Bad:

```rust
if model_name.starts_with("openai:") {
    enable_provider_web_search();
}
```

The shared runtime is guessing provider semantics from names.

Correct shape:

```rust
match model_profile.capabilities.web_search {
    WebSearchCapability::ProviderNative { policy } => enable(policy),
    WebSearchCapability::Unavailable => disable(),
}
```

The provider crate owns native interpretation. Shared code receives typed
facts.

#### Public Handles That Are Really Infra IDs

A public API exposes a raw session UUID as if it were enough to express resume
or archive behavior. If session meaning is realm-scoped, backend-scoped,
tenant-scoped, or authority-scoped, the handle is incomplete.

Correct shape:

```rust
struct SessionHandle {
    realm_id: RealmId,
    session_id: SessionId,
}
```

The handle names the domain operation and carries the owner context required to
resolve it.

#### Skill Identity By Name Alone

`load_skill("email-extractor")` is not canonical if multiple sources can
provide that name.

Correct shape:

```rust
struct SkillKey {
    source_uuid: SourceUuid,
    skill_name: SkillName,
}
```

The source identity and skill name together form the canonical key.

#### Optionality Hiding Ownership Uncertainty

Bad:

```rust
struct RunRequest {
    model: Option<String>,
    auth_binding: Option<AuthBindingRef>,
}
```

This collapses intentional inherit, accidental omission, realm default,
provider default, anonymous mode, missing auth, and explicit auth into absence.

Correct shape:

```rust
enum ModelSelection {
    InheritRealmDefault,
    Explicit(ModelId),
    ProviderDefault(ProviderId),
}

enum AuthSelection {
    InheritRealmBinding,
    Explicit(AuthBindingRef),
    AnonymousAllowed,
}
```

When absence has multiple meanings, absence is not a type.

#### JSON `kind` As The Real Contract

Bad:

```rust
match value["kind"].as_str() {
    Some("detached") => mark_detached(),
    Some("completed") => mark_completed(),
    _ => mark_unknown(),
}
```

The JSON blob is the real enum, but the code refuses to admit it.

Correct shape:

```rust
#[serde(tag = "kind", content = "data")]
enum SessionObservation {
    Detached { session: SessionId, reason: DetachReason },
    Completed { session: SessionId, outcome: CompletionOutcome },
    Failed { session: SessionId, error: RuntimeFailure },
}
```

Tagged serialization is fine. Untyped interpretation is not.

### Correct Solution Shapes

Use newtypes for semantic primitives. If two values must never be confused,
they need different types even when both wrap `Uuid`, `String`, or `usize`.

```rust
struct SessionId(Uuid);
struct OperationId(Uuid);
struct RealmId(String);
struct SkillName(String);
struct AgentIdentity(String);
struct Generation(u64);
```

A primitive tells the compiler how data is stored. A newtype tells the codebase
what the data means.

Use domain handles at boundaries. A public control surface should accept
handles that match the user operation, not the nearest internal row key. A
handle is wrong if receiving code must guess the missing realm, source, owner,
generation, profile, or authority.

Separate raw identity from canonical identity. A provider request id can be
evidence for a provider attempt. It is not a Meerkat operation id. A runtime
epoch can order hidden runtime execution. It is not mob member identity.

Model absence as policy when absence has meaning. Use `Option<T>` only when
absence has exactly one harmless meaning. Do not use it for inherit vs disable
vs explicit, known vs unknown, unresolved vs forbidden, or unauthenticated vs
anonymous allowed.

Treat provider-native data as evidence, not shared semantics. Provider-specific
fields may be preserved, audited, and lowered inside provider-owned code. They
must not become shared runtime conditionals.

### Review Questions

- What semantic distinction is this code making?
- Is that distinction represented as a type, or inferred from representation?
- Who owns the meaning of this identifier?
- Is the handle complete enough to resolve the owner without ambient context?
- Could two authorities produce the same visible name?
- Could the same raw id mean different things in different realms, generations,
  providers, or sources?
- Is an `Option<T>` hiding inherit, disable, unresolved, unknown, forbidden, or
  defaulted?
- Is provider-native data crossing into shared runtime behavior?
- Is provenance typed enough to support audit, replay, recovery, and drift
  detection?
- Would a new surface or SDK be forced to copy a convention to behave
  correctly?

### Tension Notes

Rule 4 and Rule 1: types do not create authority by themselves. A typed
duplicate of an unowned fact is still a duplicate. Name both the type and the
authority.

Rule 4 and Rule 2: adapters may classify raw input into typed observations.
They may not make canonical state changes because they have a nice enum.

Rule 4 and Rule 3: a projection should be typed, but typing does not let it
decide behavior.

Rule 4 and Rule 5: friendly names are allowed only as projections over
canonical handles. UX sugar must resolve through the owner.

Rule 4 and Rule 6: provider-native fields must become typed provider-owned
facts before shared code sees them.

Rule 4 and Rule 7: error strings are especially tempting folklore. Do not parse
terminality out of text.

Rule 4 and Rule 8: generated contracts must preserve domain types. Flattening
`SkillKey` into `"source/name"` or policy enums into nullable strings
reintroduces folklore at the boundary.

### Maxims

- If it matters, name it in the type system.
- A string may carry evidence; it must not carry law.
- A public handle must resolve its owner without guessing.
- `None` is not a policy.
- Provenance that cannot be checked is decoration.

## Chapter 5: Surfaces Are Thin Over The Shared Runtime

### Thesis

A Meerkat surface is a translation boundary. It is not a second runtime.

CLI, JSON-RPC, REST, MCP, SDKs, WASM, browser hosts, examples, and product
binaries may make Meerkat easier to call, read, embed, and ship. They may not
decide what Meerkat means. The same session, turn, model, tool, auth, mob,
comms, fault, and terminal semantics must survive every surface projection.

The surface may speak HTTP, JSON-RPC, MCP tools, CLI flags, Python objects,
TypeScript promises, Rust traits, or browser callbacks. Those are dialects. The
law beneath them is shared runtime authority or an explicitly declared
standalone substrate.

If a surface needs behavior that no shared runtime seam can express, add the
missing typed seam. Do not implement the behavior locally.

### What The Rule Forbids

Rule 5 forbids surface-owned semantic authority.

It forbids:

- surface-local session lifecycle state that competes with `SessionService` or
  `MeerkatMachine`
- separate create, resume, interrupt, cancellation, external-event, or commit
  semantics per surface
- CLI flags, REST handlers, RPC methods, MCP tools, SDK methods, or browser
  callbacks that infer terminality locally
- SDK defaults that fabricate required facts such as realm, model, auth,
  runtime mode, persistence, or capability truth
- MCP host tools that bypass typed public control planes and expose raw
  agent-side dispatchers
- REST or RPC adapters that call directly into agent execution when product
  path requires runtime admission
- examples that teach direct construction as ordinary public path when correct
  path is runtime-backed
- duplicated bootstrap, executor, materialization, config, auth, or provider
  setup when a shared helper exists
- standalone fallback behavior that silently emulates runtime-backed behavior
  without the runtime owner
- browser/WASM surfaces that market unavailable filesystem, TCP, stdio MCP,
  shell, or persistent realm behavior as present

A surface may classify its own syntax. It may parse `--resume`, route
`POST /sessions/{id}/turns`, receive `sessions/create`, expose `meerkat_resume`,
or offer `session.turn()` in an SDK. That classification must lower into typed
shared runtime operations. The surface parser may choose the door. It may not
become the house.

### Violation Examples

#### CLI As Hidden Runtime

`rkat resume` opens persistence, reconstructs a session, applies its own
`resumed` phase, and calls agent execution without the runtime-backed binding
path.

Correct shape: CLI syntax lowers into the canonical execution family. Friendly
commands are UX conveniences over shared session/runtime operations.

#### REST As Direct Execution Loop

`POST /sessions/{id}/external-events` directly wakes an agent task or drains an
inbox because that is simpler than entering runtime admission.

Correct shape: REST accepts HTTP, validates the request, and queues a typed
runtime-backed external event. It does not invent a second event loop.

#### SDK As Optimistic Mirror

A Python or TypeScript SDK creates a local `Session` object, defaults missing
status to `completed`, hides unknown events, or guesses capabilities from
package version.

Correct shape: SDK objects wrap canonical session identity and generated or
versioned contracts. Unknown events remain unknown, version mismatch is
explicit, and convenience methods project runtime facts.

#### WASM As Secret Product Emulation

The browser runtime uses in-memory state but presents itself as sharing durable
realms, filesystem config, stdio MCP servers, shell tools, or native
persistence.

Correct shape: WASM/browser mode is explicit: browser-safe substrate,
browser-safe providers and tools, in-memory or declared storage, and no hidden
native affordances.

### Correct Solution Shapes

Runtime-backed product surfaces should follow:

```text
surface syntax -> typed request -> shared surface helper -> MeerkatMachine bindings -> SessionService -> AgentFactory
```

The adapter owns protocol mechanics: HTTP status codes, JSON-RPC ids, MCP
content blocks, CLI output, SDK method names, browser event callbacks. The
shared runtime owns lifecycle, admission, runtime bindings, commit boundaries,
comms drain, routing, and terminal truth.

Standalone substrates should be explicit:

```text
declared standalone mode -> explicit RuntimeBuildMode::StandaloneEphemeral -> SessionService substrate -> AgentFactory
```

The mode must be visible in type, config, docs, and tests. It must reject or
omit runtime-owned capabilities it cannot honestly support.

When CLI, RPC, REST, MCP, SDK backend, or product binaries repeat bootstrap or
executor logic, move the behavior into a shared helper unless it is purely
protocol formatting.

User-facing syntax may be kind. It may collapse old commands, rename internal
nouns, provide shorthand, hide operator controls, or present friendly handles.
The lowering must be exact.

Capability-scoped surfaces may expose different subsets when built with
different features or deployed into constrained environments. The subset must
be explicit. Help text, `initialize`, capability responses, SDK feature flags,
and generated protocol artifacts must agree.

### Review Questions

- Does this surface enter the same runtime-backed path as its peers?
- If it is standalone, is that mode explicit and limited?
- Which shared helper does this bootstrap, executor, request-lowering, or
  response-projection logic use?
- Is any surface handler deciding lifecycle phase, terminality, retryability,
  auth truth, model capability, or persistence truth locally?
- Are CLI flags, HTTP routes, RPC methods, MCP tools, SDK methods, and WASM
  exports only syntax over typed operations?
- Are defaults owned by the correct profile, realm config, request contract, or
  runtime seam?
- Does cancellation mean the same thing across surfaces where the capability
  exists?
- Are unknown events, statuses, and version mismatches surfaced honestly?
- Do examples teach the public path, or canonize an escape hatch?
- Would deleting this surface leave runtime semantics intact?

### Tension Notes

Rule 5 and Rule 1: a surface may own presentation state, transport state, and
user interaction flow. It may not own runtime facts.

Rule 5 and Rule 2: a surface may parse raw input and classify protocol
operations. Meaningful classification must lower into typed machine input,
composition route, or service contract.

Rule 5 and Rule 3: surface caches, SDK objects, UI state, stream buffers, and
list views are projections. They may improve responsiveness. They may not
decide behavior when stale.

Rule 5 and Rule 4: friendly names are projections over canonical handles. If a
CLI alias, SDK object id, MCP parameter, browser key, or REST path segment
becomes authority, type it and resolve it through the owner.

Rule 5 and Rule 6: surfaces may expose provider selection and provider
parameters, but provider meaning belongs behind catalog, profile, provider, and
auth seams.

Rule 5 and Rule 7: HTTP disconnect, WebSocket close, JSON-RPC cancellation, MCP
tool failure, SDK abort, and browser unload must lower into explicit runtime or
standalone fault semantics.

Rule 5 and Rule 8: generated schemas, SDK types, OpenAPI, RPC catalogs, MCP
tools, and docs must describe the production path, not a parallel surface-local
interpretation.

### Maxims

- A surface may translate. It may not decide.
- Friendly syntax is holy only after typed lowering.
- Standalone is a mode, not a fallback.
- Shared behavior belongs in shared helpers.
- If CLI, REST, RPC, MCP, SDK, and WASM disagree, at least one is lying.

## Chapter 6: Providers and Policy Stay Behind Their Owning Seams

### Thesis

Meerkat is one runtime that can speak through many providers. The provider is a
backend coordinate, not a second architecture.

Provider-specific mechanics belong behind provider-owned seams. Shared runtime,
sessions, tools, memory, mobs, surfaces, SDKs, and public contracts may carry
typed provider identity and typed provider-extension payloads, but they must
not learn provider folklore. They must not parse native response shapes, infer
capabilities from model-name trivia, or decide policy from raw provider
strings.

Policy is not ambient. Capability truth, defaults, auth freshness, tool
availability, provider params, web-search behavior, and model-specific
operational defaults must flow from declared owners: `ModelProfile`, the model
catalog, realm auth bindings, `AuthMachine`, facade/factory composition, and
typed provider-extension seams.

Switching from Anthropic to OpenAI, Gemini, or a self-hosted alias must not
change Meerkat architecture. It should only change typed facts resolved by the
owning seams.

### What The Rule Forbids

Rule 6 forbids provider-specific branches in shared code unless the branch is
owned by a typed catalog, profile, or provider seam.

Forbidden patterns include:

- runtime code that checks `provider == "openai"` and changes semantic
  behavior directly
- surface code that parses `gpt-*`, `claude-*`, or `gemini-*` to infer
  capabilities
- provider-native request or response structs escaping into session history,
  tool logic, mob orchestration, memory, hooks, SDKs, or public contracts
- provider adapters deciding shared policy such as tool visibility, retry
  class, auth lifecycle, default timeout, or terminal outcome
- raw `provider_params` becoming authority in shared runtime
- public APIs that expose provider-native fields as if they were Meerkat
  concepts
- `Option<T>` policy overrides where `None` ambiguously means inherit, disable,
  unset, unknown, or use provider default
- auth freshness checks in CLI, RPC handlers, SDK defaults, or provider
  adapters
- resume or model hot-swap paths that keep stale policy from previous
  model/provider/auth identity
- exact self-hosted aliases being overridden by prefix inference
- hooks rewriting effective provider params after the owning factory/profile
  seam has composed them

Provider-specific crates may know provider-specific facts. Typed
provider-extension seams may carry irreducible provider details. The violation
begins when those details become untyped shared meaning.

### Violation Examples

#### Prefix Folklore As Capability Truth

Bad:

```rust
if model.starts_with("gpt-") {
    hide_tool("view_image");
}
```

This makes a model-name convention the owner of capability truth. It fails for
self-hosted aliases, renamed models, compatibility endpoints, and future
catalog entries.

Correct shape: resolve typed provider and model identity through the model
catalog. Read capabilities from `ModelProfile`. Unknown provider/model pairs
fail closed or require explicit catalog registration.

#### Provider Branch In A Surface

Bad:

```rust
if cli_provider == "gemini" {
    params["google_search"] = false;
} else {
    params["web_search"] = false;
}
```

The CLI has learned native provider parameter names. The surface is now a
policy composer and provider adapter.

Correct shape: the CLI records user intent, such as disabling provider-native
web search for this run. The facade/factory resolves effective
model/provider/profile and lowers the intent through the provider-owned
extension seam.

#### Provider Adapter Owns Policy

A provider crate checks session config, model profile, and runtime settings to
decide whether to inject web search.

The provider crate has become a policy composer.

Correct shape: the facade/factory composes effective provider-tool policy from
config, `ModelProfile`, session metadata, and explicit overrides. The provider
crate receives an already-composed request plan and performs native request
shaping.

#### Auth Failure Becomes Adapter Authority

A provider adapter sees HTTP 401 and marks a session as requiring reauth.

Provider transport error mutated session/auth truth. The adapter usurped
`AuthMachine`.

Correct shape: the adapter returns typed auth evidence, such as
`ProviderAuthError::Unauthorized`. The auth lease path routes evidence to the
binding-scoped `AuthMachine`.

#### Raw Provider Params In Shared Runtime

Shared runtime reads `provider_params["reasoning_effort"] == "high"` to
increase budget.

A provider-native JSON key is driving shared runtime behavior.

Correct shape: provider params are ingress or provider-extension payloads.
Shared runtime may carry them opaquely after validation, but must not interpret
native keys. If a value affects shared behavior, promote it into a typed
Meerkat concept with an owner.

#### Dynamic Identity With Static Policy

A session starts on Claude, derives tool visibility and web-search defaults,
then resumes with an OpenAI model while keeping old derived provider policy.

Correct shape: persist user intent and explicit overrides. Recompute derived
policy whenever model, provider, auth binding, session, realm, owner, or
catalog identity changes.

### Correct Solution Shapes

Provider crates own native mechanics:

- request serialization
- response parsing
- native streaming protocol
- provider retry affordances as evidence
- provider auth challenge interpretation as typed evidence
- provider-specific image/backend request shaping
- provider-native parameter validation at the provider extension seam

They do not own Meerkat session semantics, tool semantics, mob semantics,
memory semantics, terminality, or public contract meaning.

Once Meerkat uses a provider/model fact to make shared decisions, that fact
belongs in catalog/profile authority: vision, image tool results, inline video,
provider-native web search, timeout defaults, parameter schemas, and similar
facts.

Provider extensions must be typed seams. Good extension seams identify the
provider, validate against a provider-owned schema, reject unknown or
unsupported fields when the profile is closed, remain opaque to shared runtime
unless promoted into a shared type, and lower to native request fields only
inside the provider boundary.

`AuthMachine` owns auth lifecycle. Provider runtimes may consume resolved
leases. Provider adapters may return typed auth evidence. Refresh, expiry,
transient failure, permanent failure, reauth requirement, and release belong to
the binding-scoped auth machine.

Policy composition belongs at seams with full identity context: selected model,
resolved provider, `ModelProfile`, realm and auth binding, session identity,
explicit user overrides, config defaults, runtime mode, and provider-extension
payloads.

Overrides must preserve the difference between inherit, disable, set,
unsupported, and unavailable. Plain optionality is not policy.

### Review Questions

- What provider-specific fact is being used, and who owns it?
- Is this code inside a provider crate, typed provider-extension seam, or shared
  runtime?
- Is shared code branching on provider strings, model prefixes, native field
  names, or native response shapes?
- Does `ModelProfile` or the catalog already own this capability/default?
- If this is new capability/default, should catalog/profile ownership expand?
- Are provider params validated before use?
- Are provider params opaque to shared runtime after validation?
- Does this override distinguish inherit, disable, and set?
- What happens if model/provider/auth binding changes during resume, hot-swap,
  or mob member construction?
- Is derived policy recomputed from new identity?
- Does `AuthMachine` own auth lifecycle, or is a surface/provider/helper
  caching auth truth?
- Does exact self-hosted alias resolution happen before prefix inference?
- Is provider-native data stored as canonical Meerkat state?
- If a provider-specific concept must cross a boundary, is there a typed seam
  and drift test?

### Tension Notes

Rule 6 and Rule 1: provider mechanics may belong to provider crates, but shared
capability and policy facts belong to catalog/profile/auth/factory seams. Do
not let both own the same fact.

Rule 6 and Rule 2: auth lifecycle is not a provider callback. Provider
callbacks produce evidence. `AuthMachine` owns auth state change.

Rule 6 and Rule 3: provider-native forms are projections at the boundary. They
may be useful for IO, diagnostics, or interop, but must not seed canonical
session truth during recovery.

Rule 6 and Rule 4: provider strings, model prefixes, and JSON keys are not
enough. Use typed provider identity, model identity, `AuthBindingRef`,
`ModelProfile`, validated extension payloads, and typed policy overrides.

Rule 6 and Rule 5: surfaces may expose friendly provider syntax. They must
lower it into shared runtime seams.

Rule 6 and Rule 7: provider failures become typed Meerkat faults. Transport
timeout, rate limit, auth failure, malformed response, dropped stream, and
provider cancellation are not interchangeable.

Rule 6 and Rule 8: provider crates are authority boundaries. Dependencies from
shared runtime or surfaces into provider-specific internals are dogma alarms
unless explicitly part of a typed seam.

### Maxims

- Provider names are coordinates, not law.
- Policy follows identity or it lies.
- Raw provider params may enter through the gate; they may not govern the
  house.
- Auth failure is `AuthMachine` evidence, not adapter authority.
- If a provider branch lives outside its seam, prove the owner or delete the
  branch.

## Chapter 7: Terminality and Faults Are Explicit

### Thesis

Terminality is semantic truth. Faults are semantic evidence. They are not
incidental control-flow details.

A Meerkat run may be driven by loops, futures, provider streams, transports,
stores, projections, callbacks, and recovery code, but those paths must not
invent their own endings. The same condition must lower to the same terminal
authority no matter where it is detected.

The forbidden lie is not only "everything succeeded." It is also "nothing
happened", "the caller does not need to know", "the test path is close enough",
and "we logged it, therefore we handled it."

### What The Rule Forbids

Rule 7 forbids parallel terminal paths. A timeout caught in a provider adapter,
agent loop, and SDK transport must not produce three different result classes.

It forbids false success. A run that was cancelled, partially streamed,
abandoned by a dropped receiver, recovered from stale state, or completed
without verified authority must not be reported as successful completion.

It forbids turning hard failure into soft control. A warning may steer
behavior; it may not stand in for terminal outcome. A retry delay may schedule
the next attempt; it may not mean progress. A dropped channel may end delivery;
it may not mean the event was observed.

It forbids fault handling by side effect. Logging, metrics, best-effort cleanup,
swallowed panics, defaulted wire fields, and silent reconnection are evidence
at most. They are not semantic outcomes unless the owning machine, protocol, or
contract says they are.

It forbids hidden divergence between production and tests. Tests may use
smaller budgets and fake providers, but they must preserve the same terminal
semantics, ownership, schema behavior, cancellation obligations, and
backpressure policy as production.

### Violation Examples

#### Cancellation Cuts Through A Transition

A session loop races the main run future against interrupt handling. The
interrupt arm wins and the run future is dropped while a transition is between
two awaited writes. The caller receives `interrupted`, but machine state now
contains half of the abandoned transition.

Correct shape: state-mutating async paths are cancel-safe, guarded by rollback,
or wrapped in explicit transition state that can resume, compensate, or
terminally fail.

#### Partial Provider Output As Success

A provider stream yields content chunks, then the connection dies. The adapter
returns partial text as a normal assistant message because the old path expects
a response.

Correct shape: partial output is evidence. It is truthful success only if the
authority explicitly models partial terminal success.

#### Lagged Event Channel As Transport Annoyance

A REST or RPC event channel lags, drops events, logs the lag, and continues. If
observers rely on those events to reconstruct lifecycle or terminal outcome,
the drop is semantic.

Correct shape: dropped observations have declared consequence: reject, replay,
resync, degrade, or mark the observer stale.

#### Schema Default As Recovery

A schema reader accepts malformed tool arguments by defaulting them to `null`.

The system has replaced one semantic fact with another.

Correct shape: missing args, malformed args, and empty args are different typed
outcomes when they matter.

#### Panic Recovery Without State Repair

A panic is caught and converted into internal error after transition code has
already mutated state.

Correct shape: panic recovery restores pre-state, marks a partial-transition
state, or crashes loudly before corrupt state is observed.

### Correct Solution Shapes

Unify terminal paths. Define terminal outcome at the machine, composition,
handoff protocol, or public contract that owns the run.

Make success carry proof. A success result means the canonical authority
reached a successful terminal state and required effects, observations, or
persistence obligations were satisfied.

Separate soft and hard control. Use typed steering for warnings, hints, retry
recommendations, degraded modes, and budget pressure. Use typed terminal
outcomes for cancellation, timeout, rejection, exhaustion, corruption,
authorization failure, and unrecoverable drift.

Make cancellation a first-class obligation. Any state-mutating async path that
can be dropped must be cancel-safe, guarded by rollback, or wrapped in explicit
transition state.

Define backpressure where the queue is introduced. Bounded queues need named
capacity, overflow policy, and semantic consequence: block, reject, drop
newest, drop oldest, compact, or degrade. Unbounded queues need extraordinary
justification.

Put timing into profiles and contracts. Deadlines, retry budgets, stale leases,
idle timeouts, keepalive intervals, and provider request limits must be owned
timing policy, not magic durations in leaves.

Model external drift. Providers, auth systems, stores, leases, response caches,
sockets, and remote sessions can change outside Meerkat. The next interaction
must not pretend internal state remained fresh.

Treat schema skew as terminal or migration event. Version mismatch, unknown
fields, malformed payloads, missing required facts, and stale persisted data
must either migrate through declared migrators or fail closed.

Specify cascade routes. Every seam error in a composition needs a failure
route: retry, back off, compensate, fail dependents, isolate member, degrade
surface, or terminate parent.

Keep test modes semantically isomorphic. Tests may accelerate clocks and fake
IO, but they must preserve outcome classes, queue behavior, schema strictness,
cancellation model, and failure routes.

### Review Questions

- What is the one canonical terminal outcome for this condition?
- Can the same condition be detected in another task, callback, transport,
  SDK, provider, recovery path, or test harness?
- Does success prove completion, or merely absence of local error?
- What happens if cancellation occurs between every pair of awaits?
- What happens when the receiver is slow, gone, or lagged?
- Where are deadlines and retry budgets owned?
- What external state can drift while Meerkat is idle?
- What happens to old persisted data and new wire payloads when schemas
  diverge?
- Can adversarial input force excessive memory, ambiguous identity, malformed
  defaults, or panic paths?
- If this fails inside a composition, who else fails, retries, compensates, or
  continues?
- Do tests exercise the same terminal and fault semantics as production?

### Tension Notes

Rule 7 and Rule 1: terminality belongs to the same authority that owns
lifecycle truth. If a helper knows the run is over but the machine does not,
the helper is attempting to become authority.

Rule 7 and Rule 2: fault handling may require new machine inputs or states. Do
not patch around a missing timeout, cancellation, or partial-output transition
in handwritten code.

Rule 7 and Rule 3: stores and projections may report replay failure, drift, or
staleness. They must not invent recovered truth.

Rule 7 and Rule 4: faults need typed identity. Closed, gone, lagged, cancelled,
expired, rejected, and failed are not interchangeable strings.

Rule 7 and Rule 5: surfaces may choose how to display a fault. They may not
reclassify it.

Rule 7 and Rule 6: provider-native failures must be translated at the provider
seam into typed Meerkat outcomes.

Rule 7 and Rule 8: public contracts must expose real production fault
semantics. A schema that omits cancellation, partial completion, backpressure
failure, or version rejection is lying.

### Maxims

- One condition, one ending.
- Success must prove itself.
- A logged fault is not a handled fault.
- Cancellation is a transition, not an interruption.
- Tests may be faster than production; they may not be a different world.

## Chapter 8: Contracts, Crates, and Generation Are Ratchets

### Thesis

Meerkat declared architecture must tighten reality, not decorate it.

A public contract, crate boundary, generated file, schema, SDK type, formal
spec, or governance ledger is not a comment. It is an architectural ratchet:
once introduced, it must make drift harder, ambiguity smaller, and production
behavior more constrained than before.

A generated artifact that does not govern the production path is theater. A
crate boundary that does not preserve ownership is camouflage. A compatibility
field with no expiry is rot with a friendly name.

The question is never merely whether a contract exists. The question is: does
this contract bind the production authority path, and does CI prove it cannot
drift?

### What The Rule Forbids

Rule 8 forbids hand-authored semantic mirrors.

A semantic mirror is any duplicated representation of a meaningful fact that
can drift from its owner: a second status enum, a legacy wire field that still
feeds behavior, a handwritten SDK type that decides defaults differently from
the server, a documentation table used as a catalog, or a generated schema that
describes a path production does not actually use.

Rule 8 forbids generated artifacts that merely describe intent. Generated
artifacts must constrain production. If OpenAPI, RPC catalogs, SDK types,
machine specs, generated kernels, formal models, or governance ledgers can go
stale without failing a test, they are stage props.

Compatibility mirrors are forbidden unless all of these are true:

- the mirror is temporary
- the mirror is read-only
- the mirror is derived from a named authority
- the mirror has a declared owner
- the mirror has a freshness rule
- the mirror has an enforced drift test
- the mirror has an explicit expiry condition
- the mirror has explicit Highest Dogma Authority approval
- the mirror cannot feed decisions, defaults, recovery, terminality,
  acceptance, or public authority

Compatibility mirrors are not routine engineering tools. They are rare,
approved debt with a fuse.

Rule 8 also forbids crate-boundary laundering. Moving code into another crate
does not make it legitimate. If shared runtime branches on provider folklore,
the provider seam has leaked. If a surface owns request lifecycle truth, the
surface has become authority. If `meerkat-core` gains filesystem or network
authority, the core contract has been corrupted. If a facade quietly creates a
new semantic owner, composition has become concealment.

Finally, it forbids false progress. Adding a type, schema, manifest, generated
file, SDK helper, compatibility flag, or governance row while leaving old
ambiguity alive is not improvement. It widens the blast radius.

### Violation Examples

#### Generated Contract That Does Not Govern Production

The repo commits RPC schemas, SDK types, or OpenAPI output, but the server
accepts methods through a separate handwritten router that can diverge.

The generated artifact is not a ratchet.

Correct shape: production router, public schema, SDK types, and docs derive
from the same authoritative catalog or mechanically checked projections. CI
regenerates artifacts and fails on diff.

#### Catalog Machine Verified, Production Machine Forked

The catalog DSL defines a canonical machine and feeds schema generation and
model checking, while production carries a separate `machine!` body for the
same conceptual machine.

Formal verification covers one body; production executes another.

Correct shape: one authoritative machine body per canonical machine. Production
modules are generated from the catalog or expand catalog-owned source. Drift
checks compare schema and generated output, not just names.

#### Legacy Field Without Expiry

A public response keeps `old_status` for backwards compatibility. New code
still reads it, and there is no breaking-release gate, contract version, date,
owner, or removal test.

The field became permanent authority by inertia.

Correct shape: if the old field must remain temporarily, it is derived
read-only from canonical status, approved by the Highest Dogma Authority,
documented with expiry, covered by drift test, and forbidden from feeding
behavior.

#### Contract Crate Becomes Behavior Crate

`meerkat-contracts` starts computing runtime policy, selecting provider
defaults, validating machine transitions, or embedding provider behavior
because wire types were nearby.

The crate boundary inverted.

Correct shape: `meerkat-contracts` defines stable wire vocabulary, schema
surfaces, error codes, and versioning. Runtime policy lives in runtime, facade,
profile, auth, or composition seams.

#### Ambiguity Moved Instead Of Removed

A bug says terminal states are ambiguous. The fix adds `TerminalStatusV2` while
leaving `status`, `phase`, and `completed: bool` active. Different surfaces pick
different fields.

The change increased ambiguity.

Correct shape: introduce the new canonical type with a migration plan that
retires or mechanically derives old forms. If old wire fields must exist
temporarily, they are expiry-bound mirrors and cannot affect behavior.

### Correct Solution Shapes

Use authority-derived generation:

- catalog DSL generates machine schemas, formal specs, and production machine
  modules
- typed RPC catalog generates method schemas, SDK wrappers, and server
  registration checks
- OpenAPI is emitted from the same REST route contract production registers
- SDK types are generated from `meerkat-contracts` wire types or checked schema
- governance ledgers are generated from typed registries and fail when
  ownership rows are missing or stale

The lineage is:

```text
authority source -> generated artifact -> production check -> drift failure
```

Protect generated output with deterministic rerun-and-diff gates: run the
generator in CI, check worktree diff, fail on hand-edited generated files, fail
on missing generated headers, and fail on schema or catalog drift.

Make compatibility debt ugly on purpose:

```text
legacy_status:
  source: MachineTerminalClass
  owner: meerkat-contracts public compatibility
  reason: clients before contract version N
  expiry: remove at contract version N+1
  approval: Highest Dogma Authority, YYYY-MM-DD
  drift test: legacy_status == derive_legacy_status(machine_terminal_class)
  behavior: forbidden as input to server decisions
```

If that feels heavy, good. The weight is the point.

Treat projections differently from mirrors. A session list index, provider
native request body, or CLI display shape may be permanent. These are not
compatibility mirrors if they are mechanical projections and cannot override
authority.

Use crate ownership as enforcement:

- `meerkat-core` owns shared contracts and trait vocabulary; it must not gain
  network or filesystem authority.
- `meerkat-contracts` owns public wire shapes, schemas, error codes, and
  contract versioning; it must not become runtime policy.
- `meerkat-runtime` owns the runtime control plane and `MeerkatMachine` path.
- `meerkat-session` owns session service implementations; it must not become
  runtime authority.
- provider crates own provider-native mechanics; they must not export provider
  folklore as shared truth.
- surface crates own transport and UX; they must not own meaning.
- facade crates compose; they must not hide new authorities.

Public contracts are promises. Every public method, field, schema, error code,
handle, event, or SDK type must answer: who owns this fact, how is it generated
or checked, what versioning rule governs change, what clients may rely on it,
what drift test prevents divergence, and what happens when the authority
changes.

### Review Questions

- What production authority path does this contract describe?
- Is the artifact generated from that path, or hand-authored beside it?
- Can production behavior drift without failing CI?
- What rerun-and-diff or schema parity gate protects this artifact?
- If this is hand-authored, why is it not a semantic mirror?
- Is this a projection, compatibility mirror, or new authority?
- What is the source truth for this projection?
- Can stale projection data affect behavior?
- Does every compatibility mirror have Highest Dogma Authority approval?
- Does every compatibility mirror have date, contract version, or
  breaking-release expiry?
- Does any legacy field feed decisions, defaults, recovery, terminality,
  acceptance, or public authority?
- Does this crate own the concern being added to it?
- Are public SDKs and schemas derived from the same contract production uses?
- Does this change remove ambiguity, or add another representation beside the
  old one?
- If a future maintainer edits the wrong place, what test fails?

### Tension Notes

Rule 8 and Rule 1: contracts, crate boundaries, generated artifacts, and
ledgers must preserve singular ownership.

Rule 8 and Rule 2: it is not enough to generate specs from a machine catalog if
production executes a separate handwritten body.

Rule 8 and Rule 3: permanent read models are projections. Compatibility mirrors
are external-client debt with approval and expiry.

Rule 8 and Rule 4: a generated bad contract is still bad. Generation ratchets
the chosen model; it does not sanctify the wrong abstraction.

Rule 8 and Rule 5: generated surface schemas must describe shared
runtime-backed behavior, not parallel surface-local interpretation.

Rule 8 and Rule 6: when provider portability is impossible, use a typed
provider-extension seam. Do not smuggle native fields into shared logic.

Rule 8 and Rule 7: public error codes, terminal statuses, cancellation results,
retry classes, and recovery outcomes must be generated from or checked against
the authority that owns fault semantics.

### Maxims

- If it does not constrain production, it is not a contract.
- A generated artifact without a drift gate is theater.
- A permanent duplicate is either a projection or a violation.
- Compatibility mirrors require a fuse and the Highest Dogma Authority's
  blessing.
- Every new artifact must reduce ambiguity, or it has failed.
