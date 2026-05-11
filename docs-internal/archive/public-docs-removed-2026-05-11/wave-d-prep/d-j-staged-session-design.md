# D-j design note — PendingSession lifecycle → canonical owner

Context: `meerkat-rpc/src/session_runtime.rs` holds an RPC-local
`PendingSession` + `PendingSessionPhase::{Staged, Promoting}` two-phase
lifecycle for sessions that have been created (ID returned) but not yet
materialized in the service. Ten branching sites read `.phase` inside
the RPC surface. The surface is currently the authority for "staged but
not materialized" — a Dogma #17 + #3 violation (transport skin owns
semantic state; duplicate authority path alongside `SessionService`).

The task contract offers two closures.

## Option A — `MeerkatMachine` grows a `Staged` phase

Shape: DSL adds a `Staged` variant to `MeerkatPhase` (or a parallel
per-session discriminant). RPC calls `machine.stage(session_id, build_config)`
then `machine.promote(session_id)` to advance to `Attached` / `Running`.

Problem: the thing a staged session carries is an `AgentBuildConfig`
bundle — `Arc<dyn AgentToolDispatcher>`, `Arc<dyn LlmClient>`, the
attached `McpRouterAdapter`, `SessionRuntimeBindings` from
`MeerkatMachine::prepare_bindings`, tokio channel senders, and a live
`Session` with the claimed ID already embedded. None of that is
representable as DSL state: the DSL is a pure value machine (TLC
verifiable), and the build bundle is full of non-`Serialize` Arcs and
live handles.

The honest shape of Option A is therefore:

- DSL owns a `Staged | Promoting | Attached` discriminant, plus the
  `SessionId` set.
- Shell holds a side map `SessionId → AgentBuildConfig` indexed by
  staged session — same `IndexMap<SessionId, _>` the RPC surface
  already has, renamed.

That is the same split-ownership anti-pattern the task targets,
relocated: DSL gates transitions, shell holds the real payload, the
two must stay in lock-step. The concurrency guard the
`Staged → Promoting` flip currently provides (`SESSION_BUSY` on
reentrant promotion) would cross the DSL-shell boundary on every
materialization — not free.

Option A would also cascade schema regen and a workspace codegen
rebuild for a change whose on-disk truth is still an `Arc`-heavy bundle
the DSL can't model.

## Option B — `SessionService` owns `stage / promote / abandon_staged`

Shape: the canonical `SessionService` trait grows three typed methods:

```rust
async fn stage_session(&self, req: StageSessionRequest) -> Result<SessionId, SessionError>;
async fn promote_session(&self, id: &SessionId, req: PromoteSessionRequest) -> Result<RunResult, SessionError>;
async fn abandon_staged(&self, id: &SessionId) -> Result<(), SessionError>;
```

`StageSessionRequest` carries the build data (model, prompt, system
prompt, labels, deferred prompt policy, `SessionBuildOptions`, optional
pre-allocated `SessionId`), exactly what the RPC surface currently
stores in `PendingSession`. `PromoteSessionRequest` carries the first
turn's prompt and overrides. `abandon_staged` is the no-turn exit path.

`PersistentSessionService` owns the staged map directly — a private
`DashMap<SessionId, StagedSlot>` next to its existing live-session
storage. Concurrency gate (`SESSION_BUSY` on concurrent promote) is
enforced inside the service, not by the transport. List/read/archive
see staged sessions via the same mechanism that exposes live sessions.

Why this is right for Meerkat:

- `SessionService` is **not** a projection layer. It is the canonical
  lifecycle authority for session existence: `create_session`,
  `archive`, `interrupt`. Adding a pre-materialization state alongside
  "live session" is extending the existing authority, not creating a
  new one.

- `MeerkatMachine` is a **per-session runtime-behavior machine**. Its
  own `Initializing → Idle → Attached → Running` lifecycle begins
  with `RegisterSession`. "Before `RegisterSession` has run" is
  pre-DSL-existence — conceptually outside the DSL's domain. The
  service layer is where "session is named but not yet materialized"
  lives naturally.

- The build bundle stays where it belongs (live Arc references in the
  service layer), with no DSL-shell shadow pair.

- The catching assertion from the task (`rg 'PendingSession' meerkat-rpc/src/`
  zero production hits; canonical owner holds the staged phase) is
  fully satisfied: after closure the RPC surface has no local staged
  storage and no `.phase`-branching code paths — every entry point
  flows through `SessionService::{stage_session,promote_session,abandon_staged}`
  or the existing `start_turn` / `archive`.

## Chosen path: B, refined for crate-dep constraint

First implementation pass surfaced a structural constraint. The
staged session carries an `AgentBuildConfig` (defined in the
`meerkat` facade crate at `meerkat::factory`). `PersistentSessionService`
lives in `meerkat-session`, which is an *upstream* dependency of
`meerkat`. `meerkat-session` therefore cannot import
`AgentBuildConfig` without inverting a crate dependency.

The honest landing point is one layer up, at the **canonical
agent-construction authority**: the `meerkat` facade itself.

### Refined shape

- `meerkat-core/src/service/mod.rs` — trait extension on
  `SessionService` (default `Unsupported`) for `stage_session /
  promote_session / abandon_staged`. Methods operate on an opaque
  build-handle keyed by `SessionId`; the trait remains object-safe.

- `meerkat/src/service_factory.rs` (facade) — new typed
  `StagedSessionRegistry` holding `DashMap<SessionId, StagedSlot>`.
  Each `StagedSlot` carries the fully-typed `AgentBuildConfig`,
  `SessionLlmIdentity`, labels, deferred prompt, timestamps, and the
  phase discriminant (`Staged | Promoting`). Registry exposes typed
  `stage / promote / abandon / append_system_context / snapshot`
  methods. The registry is the canonical owner; it is *not* a
  transport skin — the facade is the canonical agent-construction
  authority and already owns `AgentFactory`, `FactoryAgentBuilder`,
  and `build_ephemeral_service`.

- `meerkat-rpc/src/session_runtime.rs` — `PendingSession`,
  `PendingSessionPhase`, and the `pending:
  RwLock<IndexMap<SessionId, PendingSession>>` field are deleted.
  Every callsite is rewritten to call through the facade-owned
  registry handle.
  `restore_pending_from_promoting` becomes a single
  `registry.abandon_promote(session_id)` that restages the slot.

- Catching assertion: `rg 'PendingSession|PendingSessionPhase'
  meerkat-rpc/src/` returns zero production hits.

- Tests: the registry ships with unit tests in `meerkat/tests/` for
  stage → promote, stage → abandon, stage → promote concurrency
  gate, `append_system_context` mutation during Staged and
  Promoting, and list/read exposure. RPC integration tests
  continue to exercise the end-to-end path through
  `session/create` + `turn/start` / `turn/append_system_context`
  / `session/archive`.

### Why facade, not service

`SessionService` is crate-boundary bounded: it cannot hold
surface-constructed `AgentBuildConfig` without a crate-dep
inversion. The facade is the layer where agent construction is
already rooted; placing the staging authority there keeps the
transport (RPC) a pure skin and puts the canonical owner at the
construction authority, not at a skin. This still closes the
Dogma #17 + #3 violation the task targets: the transport surface
holds zero staged state after the change.

### Scope widen notes

Per the architectural-prerequisite rule, this widens scope to
`meerkat-core/src/service/mod.rs` (trait methods) and
`meerkat/src/service_factory.rs` (registry authority) rather than
splitting the work. No DSL schema change; no codegen regen; no
workspace codegen cascade. Cascade is limited to the two crates
above plus `meerkat-rpc` callsite rewrites.
