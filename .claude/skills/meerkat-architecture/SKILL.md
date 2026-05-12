---
name: meerkat-architecture
description: "Internal architecture guide for the Meerkat agent platform. This skill should be used when understanding crate ownership, trait contracts, the agent construction pipeline, session service lifecycle, runtime control plane, mob orchestration internals, machine authority, comms wiring, or making cross-cutting architectural changes. Oriented toward AI agents and developers working on meerkat internals, not end users."
---

# Meerkat Internal Architecture

Meerkat is a library-first agent runtime. The execution pipeline is shared across all surfaces. Understanding crate ownership, trait contracts, the DSL-authoritative machine system, and the runtime control plane is essential for making changes that don't break architectural invariants.

This file is a lean navigator. Load the specific reference under `references/` when working on that domain.

## Core Principles

1. **Infrastructure, not application** — the agent loop is a composable primitive with no opinions about prompts, tools, or output.
2. **Trait contracts own the architecture** — `meerkat-core` defines contracts; implementations live in satellite crates.
3. **Surfaces are skins, not authorities** — CLI, REST, RPC, MCP, WASM route through shared substrate and factory seams, but runtime-backed surfaces own runtime semantics.
4. **Composition over configuration** — optional components are `Option<Arc<dyn Trait>>`, not feature-flagged defaults.
5. **Runtime conforms to catalog-generated machines** — the runtime follows the verified DSL model, not the other way around. Production bridge modules import/invoke catalog-owned DSL bodies; they do not author competing machine semantics.
6. **Mob is the only multi-agent runtime path** — no separate sub-agent substrate. User-facing "delegate"/"sub-agent" flows compile to mob members, often inside session-owned implicit mobs.
7. **Override-first resource injection** — `AgentBuildConfig` overrides take precedence over factory/config/filesystem resolution.
8. **Seams are formal** — async owner handoffs, wait barriers, and surfaced terminal classes are modeled in the DSL or composition protocols, not left to shell convention.
9. **Parity gates are dogmatic** — production/catalog schema drift, command alphabet drift, string whitelists, and handwritten production machine bodies are CI failures, not review notes.

## Build And Test Architecture

Make is the developer-facing command surface. Cargo is still the default
backend; BuildBuddy/Bazel is an optional acceleration backend selected with the
single opt-in switch `MEERKAT_BUILDBUDDY=1`.

Architectural split:

- `Makefile` owns the stable local verbs: `make build`, `make check`,
  `make lint`, `make test`, `make test-unit`, `make test-int`, `make e2e-fast`,
  `make e2e-system`, `make e2e-live`, and `make e2e-smoke`.
- `scripts/run-build-backend-lane` is the backend switch. It runs
  `scripts/repo-cargo` by default and delegates to `scripts/buildbuddy-dev`
  only when BuildBuddy is explicitly enabled.
- `scripts/buildbuddy-dev` is the local BuildBuddy facade. Use it or the
  explicit `make buildbuddy-*` targets for local RBE lanes; avoid raw `bb`
  except when debugging the wrapper.
- `scripts/buildbuddy-bazel-poc` is the lower-level Bazel launcher and
  compatibility layer. It may temporarily rebase stale absolute local path
  dependencies in `MODULE.bazel.lock`, runs local lanes with
  `--lockfile_mode=error` against checked-in Bazel metadata, and restores the
  checked-in lockfile plus vendored generated BUILD bytes after Bazel exits.
  Persistent BUILD regeneration and module-lock updates are explicit maintenance
  steps, not normal local BuildBuddy lane behavior.
- `.github/workflows/ci.yml` selects exactly one backend. Cargo and BuildBuddy
  reusable workflows remain separate so CI lanes stay visible and comparable.

Test lane authority belongs in Rust, not shell glue. The canonical e2e lane
catalog is `tests/integration/src/e2e_lanes.rs`; scripts and Bazel targets
should route to that taxonomy rather than inventing parallel classifications.

For same-checkout multi-agent work, set distinct `RUST_LANE_ID` values when you
want stable warm local output roots. Separate Git worktrees are isolated by path
hash for both Cargo and BuildBuddy output roots.

## Runtime Dogma (first review lens)

Public doctrine summary: `docs/reference/machine-authority.mdx`.
Historical internal doctrine archive: `docs-internal/archive/public-docs-removed-2026-05-11/architecture/meerkat-runtime-dogma.md`.

Short version:

1. One semantic fact, one owner.
2. Machines own semantics.
3. Shell owns mechanics, not meaning.
4. One semantic condition, one terminal path.
5. Typed truth, never string folklore.
6. App-facing APIs expose domain handles (`job_id`, `AgentIdentity`, `AgentRuntimeId`, etc.), not raw infra IDs.
7. Raw infra identity must be canonical.
8. `Option<T>` must not hide ownership uncertainty.
9. Inherit / disable / set are different facts (tri-state override types).
10. Dynamic policy follows dynamic identity.
11. Derived projections are rebuildable, never authoritative.
12. Surfaces are skins, not authorities.

### Live audio/video adapter vocabulary (public noun)

Live (audio/video) channels are exposed through the caller-initiated
`live/*` surface. Capability detection still uses `ModelCapabilities.realtime`
to decide whether a model can back a live channel; channel lifecycle
is **caller-initiated** through the `live/*` JSON-RPC methods (and
their typed SDK wrappers). The previous capability-driven attachment
plane (`session/realtime_attachment_status`,
`mob/member_status.realtime_attachment_status`, `realtime/open_info`,
`RealtimeAttachmentStatus`, `MeerkatMachine::reconfigure_live_topology`
and the realtime-binding DSL state) has been removed.

Public vocabulary:

- `ModelCapabilities.realtime` — capability bit that gates whether
  `live/open` succeeds for a session.
- `live/open` — open a live channel on a session; returns a typed
  `LiveOpenResult` carrying transport bootstrap (e.g. WS URL for
  `rkat-rpc --live-ws`'s `/live/ws` listener), `WireLiveChannelCapabilities`,
  and `WireLiveContinuityMode`.
- `live/status`, `live/refresh`, `live/send_input`, `live/commit_input`,
  `live/interrupt`, `live/truncate`, `live/close` — channel lifecycle.
- The `--live-ws <addr>` flag on `rkat-rpc` enables the WebSocket
  listener. Without it the `live/*` methods are not registered (router
  arms gated on `live_ws_state.is_some()`).

Wire types live in `meerkat-contracts/src/wire/live.rs`. Adapter
internals live in `meerkat-core::live_adapter` (`LiveAdapterStatus`,
`LiveChannelCapabilities`, `LiveContinuityMode`,
`LiveTransportBootstrap`, `LiveAdapterObservation`, etc.). Provider
implementations currently sit in `meerkat-openai::live`.

The DSL realtime-binding plane and `reconfigure_live_topology`
orchestration were deleted, not renamed; the live-adapter implementation
lives in `meerkat-live`. For a deeper internal reference,
`meerkat-live/src/host.rs`, `meerkat-rpc/src/handlers/live.rs`, and
`meerkat-contracts/src/wire/live.rs` are the authoritative surface;
`docs/guides/realtime.mdx` is the user-facing Live Channels companion.

## The 6-machine target

Exactly six canonical machines, each with a DSL source in `meerkat-machine-schema/src/catalog/dsl/`:

- **MeerkatMachine** — session-scoped execution kernel. Owns session lifecycle, input admission, ops lifecycle, turn execution, tool surface state, drain lifecycle, peer comms classification.
- **MobMachine** — mob-scoped orchestration. Owns mob lifecycle, member lifecycle, kickoff, wiring, roster, flow/frame/loop execution. (Per-member realtime intent and the realtime-binding plane were removed — live channels are caller-initiated via `live/*`, gated on each member's session-level `ModelCapabilities.realtime`.)
- **ScheduleLifecycleMachine** — scheduler triggers and schedule lifecycle.
- **OccurrenceLifecycleMachine** — occurrence dispatch and delivery.
- **AuthMachine** — auth/session authorization state that must remain machine-owned.
- **WorkGraphLifecycleMachine** — realm-scoped durable commitment graph authority. Owns work item lifecycle, revision/CAS legality, dependency readiness, claim leases, terminal state, topology legality, and evidence revision handling.

Plus five composition protocols at the seams: `meerkat_mob_seam`, `schedule_bundle`, `schedule_runtime_bundle`, `schedule_mob_bundle`, `auth_lease_bundle`.

**Primary semantic authority lives in the catalog-generated machines.** Production modules are bridge shells around catalog-owned DSL bodies and crate-local bridging types. Handwritten `*_authority.rs` helpers that still exist are adapter mechanics, projections, planners, or sealed mutators, not competing semantic owners.

Phase 1 of the machine-authority convergence is closed:

- Catalog DSL is the source for production machine bodies and generated kernels.
- `runtime_schema_parity` asserts catalog/production schema equality for all canonical machines.
- `runtime_alphabet_parity` uses typed command classification manifests; string whitelists are forbidden.
- `flow_run`, `flow_frame`, and `loop_iteration` are MobMachine-owned fail-closed projection reducers. They are support modules for `MobRun` projection shape, not canonical machines.

Detailed architecture, DSL ↔ schema ↔ kernel ↔ TLA+ flow, the field-driven design principle, signals vs inputs, and the cross-crate handle trait pattern: **load `references/machine-system.md`**.

## Identity-first mob model

Stable per-member identity is separate from per-runtime binding:

- **`AgentIdentity`** — assigned at spawn, persists across respawns and runtime-binding changes. Keys all public mob APIs (`mob/member_status`, delegate targets, wiring, etc.).
- **`AgentRuntimeId`** — per-runtime binding detail. Rotates on respawn. DSL guards keyed on `{agent_runtime_id, fence_token}` use this for binding-level rotation safety.
- **`FenceToken`** — monotonic epoch counter for runtime bindings. DSL guards enforce `fence_token` ordering.
- **`Generation`** — mob-member generation counter; increments on respawn.

When adding state or effects keyed on member identity, choose
`AgentIdentity` if the fact survives respawn (wiring preferences,
durable per-member configuration), `AgentRuntimeId` if it's
per-binding (ops registry membership, adapter ownership for a running channel).

## Crate Ownership

| Crate | Owns | Key Trait |
|-------|------|-----------|
| `meerkat-models` | Compatibility shim that re-exports `meerkat_core::model_profile` | — |
| `meerkat-core` | Agent loop, core types, session-store contract, ALL trait contracts, DSL handle traits | `AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`, `SessionStore`, `SessionService`, `CommsRuntime`, `HookEngine`, `OpsLifecycleRegistry`, `TurnStateHandle`, `CommsDrainHandle`, `ExternalToolSurfaceHandle`, `PeerCommsHandle`, `SessionAdmissionHandle`, `ModelRoutingHandle`, `AuthLeaseHandle`, `McpServerLifecycleHandle`, `PeerInteractionHandle`, `SessionContextHandle`, `SessionClaimHandle`, `InteractionStreamHandle` |
| `meerkat-contracts` | Wire types, catalogs, stable error codes, generated surface schemas, **supervisor bridge protocol (`BridgeCommand`, `BridgeReply`, `BridgePeerSpec`, `BridgeSupervisorPayload`)** | — |
| `meerkat-client` | Compatibility client shim that re-exports provider surfaces | Compatibility exports only |
| `meerkat-auth-core` | Shared auth primitives, token stores, OAuth helpers, cloud authorizers | — |
| `meerkat-providers` | Compatibility provider-runtime/auth shim surface | — |
| `meerkat-anthropic` / `meerkat-openai` / `meerkat-gemini` | Provider-specific client/runtime implementations | Implements `AgentLlmClient` via provider-specific crates |
| `meerkat-store` | Session-store implementations and adapters (SQLite, Jsonl, Memory) | Implements `SessionStore` |
| `meerkat-tools` | Tool registry, builtins, shell, session-scoped task store | Implements `AgentToolDispatcher` |
| `meerkat-mcp` | MCP client, protocol transport, router (routes to `ExternalToolSurfaceHandle`) | — |
| `meerkat-session` | Session orchestration (Ephemeral, Persistent), turn admission slot (shell) | Implements `SessionService` |
| `meerkat-runtime` | Runtime control plane, policy engine, completion-feed wake, DSL handle impls | `RuntimeControlPlane`, `RuntimeDriver`, `MeerkatMachine` |
| `meerkat-comms` | Inter-agent messaging (inproc, TCP, UDS, Ed25519), peer identity claims, pure peer data types | Implements `CommsRuntime` |
| `meerkat-hooks` | Hook runtimes (in-process, command, HTTP) | Implements `HookEngine` |
| `meerkat-skills` | Skill loading (filesystem, git, HTTP, embedded) | Implements `SkillEngine` |
| `meerkat-memory` | Semantic memory stores and retrieval | Implements `MemoryStore` |
| `meerkat-mob` | Multi-agent orchestration, member provisioning, flow runtime, **identity-first binding model, supervisor bridge** | `MobSessionService`, `MobProvisioner`, `MobMemberRuntimeBridge` |
| `meerkat-mob-pack` | Mobpack archive format, signing, trust policies, validation | — |
| `meerkat-mob-mcp` | MCP/operator mob surface plus agent-facing delegation tool surface | `MobMcpState`, `AgentMobToolSurfaceFactory` |
| `meerkat-live` | Live channel host and WebSocket transport glue for `live/*` methods | `LiveAdapterHost`, `LiveProjectionSink` |
| `meerkat-schedule` | Scheduler subsystem; `Schedule::apply` / `Occurrence::apply` on domain types | `ScheduleService`, `ScheduleDriver`, `ScheduleStore` |
| `meerkat-web-runtime` | WASM browser deployment (wasm_bindgen exports) | — |
| `meerkat-machine-schema` | Rust-native machine/composition catalog DSL — the formal authority | — |
| `meerkat-machine-kernels` | Generated kernel interpreter for all machines/compositions | `GeneratedMachineKernel` |
| `meerkat-machine-codegen` | TLA+ model generation, TLC verification, drift detection | — |
| `meerkat` (facade) | `AgentFactory`, `FactoryAgentBuilder`, persistence helpers, re-exports | Wires everything together |

**Rule: `meerkat-core` has zero I/O dependencies.** All I/O happens in satellite crates.

For detailed crate-by-crate reference: load `references/crate_map.md`.

## Reference Navigator

Load these as needed. SKILL.md alone is intentionally minimal — everything else lives in `references/` for progressive disclosure.

- **`references/machine-system.md`** — load when touching DSL sources, catalog schemas, generated kernels, TLC verification, production bridge modules, parity gates, command classification, authority cutover, handle trait design, or any "where does this semantic state live" question. Covers the DSL → MachineSchema → kernel → TLA+ → runtime flow, the catalog/production parity ratchets, the field-driven design principle, signals vs inputs, and the `HandleDslAuthority` cross-crate pattern.
- **`references/runtime-control-plane.md`** — load when working on `MeerkatMachine`, runtime drivers, session registration, policy resolution, `RuntimeBuildMode` / `SessionRuntimeBindings`, `OpsLifecycleRegistry`, session service lifecycle, persistence pairing, detached-op wake, or test harness ownership.
- **`references/agent-construction.md`** — load when touching `AgentFactory::build_agent()`, agent builder, multimodal content types, or runtime tool scoping.
- **`references/mob-orchestration.md`** — load when working on mobs: creation, launch modes, spawn policies, delegation tools, lifecycle control, provisioning, wiring, flow/frame execution, mob persistence, or `MobActor` decomposition.
- **`references/comms-model.md`** — load when working on peer trust, inter-agent messaging, comms drain lifecycle, envelope classification, or session identity claims.
- **`references/gotchas.md`** — load as the first review lens for non-trivial changes. Regression checklist of architectural invariants that quietly re-break.
- **`references/crate_map.md`** — detailed crate-by-crate reference.

## Key files (quick index)

For comprehensive file lists, see the matching reference. This is a minimal pointer index for the most common landmarks.

- `meerkat-machine-schema/src/catalog/dsl/` — DSL sources (truth for all 6 canonical machines)
- `meerkat-machine-schema/src/catalog/mod.rs` — `canonical_machine_schemas()` registry
- `meerkat-machine-kernels/src/runtime.rs` — `GeneratedMachineKernel` interpreter
- `meerkat-runtime/src/meerkat_machine/` — `MeerkatMachine`, session management, dispatch paths, DSL adapter
- `meerkat-runtime/src/handles/` — runtime impls of DSL handle traits
- `meerkat-core/src/handles.rs` — DSL handle trait definitions
- `meerkat-core/src/runtime_epoch.rs` — `SessionRuntimeBindings`, `RuntimeBuildMode`
- `meerkat-live/src/host.rs`, `meerkat-live/src/transport.rs` — live channel host and WebSocket transport
- `meerkat-rpc/src/handlers/live.rs` — `live/*` JSON-RPC handlers
- `meerkat-core/src/agent.rs`, `meerkat-core/src/agent/*.rs` — agent loop
- `meerkat/src/factory.rs` — `AgentFactory::build_agent()` (pipeline)
- `meerkat-session/src/{ephemeral,persistent}.rs` — session services
- `meerkat-mob/src/runtime/actor.rs` — `MobActor`
- `meerkat-mob/src/backend.rs`, `meerkat-mob/src/ids.rs` — identity-first binding model
- `meerkat-mob/src/runtime/supervisor_bridge.rs` — supervisor bridge transport
- `meerkat-mob/src/runtime/local_bridge.rs` — in-process MeerkatMachine bridge
- `meerkat-mob-mcp/src/agent_tools.rs` — agent-facing delegation/orchestration tools
- `meerkat-contracts/src/wire/supervisor_bridge.rs` — bridge protocol types
- `docs/reference/machine-authority.mdx` — public machine-authority summary
- `docs/reference/build-and-ci.mdx` — public BuildBuddy/Cargo/CI guide
- `docs-internal/archive/public-docs-removed-2026-05-11/architecture/meerkat-runtime-dogma.md` — historical internal doctrine archive
- `docs-internal/archive/public-docs-removed-2026-05-11/architecture/identity-first-live-voice-proposal.md` — historical live + identity design notes
- `tests/integration/src/e2e_lanes.rs` — authoritative e2e lane catalog
- `scripts/build-backend-env`, `scripts/run-build-backend-lane`, `scripts/buildbuddy-dev` — local build backend switch and BuildBuddy facade
