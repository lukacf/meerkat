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
5. **Runtime conforms to machines** — the runtime follows the verified DSL model, not the other way around.
6. **Mob is the only multi-agent runtime path** — no separate sub-agent substrate. User-facing "delegate"/"sub-agent" flows compile to mob members, often inside session-owned implicit mobs.
7. **Override-first resource injection** — `AgentBuildConfig` overrides take precedence over factory/config/filesystem resolution.
8. **Seams are formal** — async owner handoffs, wait barriers, and surfaced terminal classes are modeled in the DSL or composition protocols, not left to shell convention.

## Runtime Dogma (first review lens)

Full doctrine: `docs/architecture/meerkat-runtime-dogma.md`.

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

### Realtime attachment vocabulary (public noun)

Public API surfaces describe `realtime`, not `voice`. Realtime
transport is capability-driven: `ModelCapabilities.realtime: bool` on
the session's resolved model decides attach/detach. There is no
caller-initiated attach/detach RPC. Converged terms:

- `ModelCapabilities.realtime` — the capability bit that drives transport
- `runtime/realtime_attachment_status` — single-session status projection
- `runtime/realtime_attachment_statuses` — batch status projection
- `mob/member_status.realtime_attachment_status` — per-member projection
- `realtime/open_info` / `realtime/status` / `realtime/capabilities` — product-layer bootstrap

The `realtime_attachment_status` enum is the typed form surfaces
present; audio-only backend behavior is one provider specialization,
not the public capability name. Prior `mob/realtime_attach(detach)` and
`attachMobMemberLive`/`detachMobMemberLive` surfaces have been deleted.

Load `references/realtime-attachment.md` when touching realtime
attachment state, the live-topology reconfigure flow, or the DSL
authority epoch model.

## The 5-machine target

Exactly five canonical machines, each with a DSL source in `meerkat-machine-schema/src/catalog/dsl/`:

- **MeerkatMachine** — session-scoped execution kernel. Owns session lifecycle, input admission, ops lifecycle, turn execution, tool surface state, drain lifecycle, peer comms classification, **realtime attachment authority, live-topology reconfigure phase**.
- **MobMachine** — mob-scoped orchestration. Owns mob lifecycle, member lifecycle, kickoff, wiring, roster, flow/frame/loop execution. (Per-member realtime intent was removed — realtime is driven by each member's session-level `ModelCapabilities.realtime`.)
- **ScheduleLifecycleMachine** — scheduler triggers and schedule lifecycle.
- **OccurrenceLifecycleMachine** — occurrence dispatch and delivery.
- **AuthMachine** — auth/session authorization state that must remain machine-owned.

Plus four composition protocols at the seams: `meerkat_mob_seam`, `schedule_bundle`, `schedule_runtime_bundle`, `schedule_mob_bundle`.

**Primary semantic authority should live in the catalog-generated machines.** Handwritten `*_authority.rs` helpers that still exist should be treated as transitional or adapter-level mechanics, not competing semantic owners.

Detailed architecture, DSL ↔ schema ↔ kernel ↔ TLA+ flow, the field-driven design principle, signals vs inputs, and the cross-crate handle trait pattern: **load `references/machine-system.md`**.

## Identity-first mob model

Stable per-member identity is separate from per-runtime binding:

- **`AgentIdentity`** — assigned at spawn, persists across respawns and runtime-binding changes. Keys all public mob APIs (`mob/member_status`, delegate targets, wiring, etc.).
- **`AgentRuntimeId`** — per-runtime binding detail. Rotates on respawn. DSL guards keyed on `{agent_runtime_id, fence_token}` use this for binding-level rotation safety.
- **`FenceToken`** — monotonic epoch counter for runtime bindings. DSL guards enforce `fence_token` ordering.
- **`Generation`** — mob-member generation counter; increments on respawn.

When adding state or effects keyed on member identity, choose
`AgentIdentity` if the fact survives respawn (realtime attachment preferences,
wiring preferences), `AgentRuntimeId` if it's per-binding (ops
registry membership, live bridge).

## Crate Ownership

| Crate | Owns | Key Trait |
|-------|------|-----------|
| `meerkat-models` | Compatibility shim that re-exports `meerkat_core::model_profile` | — |
| `meerkat-core` | Agent loop, core types, session-store contract, ALL trait contracts, DSL handle traits | `AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`, `SessionStore`, `SessionService`, `CommsRuntime`, `HookEngine`, `OpsLifecycleRegistry`, `TurnStateHandle`, `CommsDrainHandle`, `ExternalToolSurfaceHandle`, `PeerCommsHandle`, `SessionAdmissionHandle` |
| `meerkat-contracts` | Wire types, catalogs, stable error codes, generated surface schemas, **supervisor bridge protocol (`BridgeCommand`, `BridgeReply`, `BridgePeerSpec`, `BridgeSupervisorPayload`)** | — |
| `meerkat-client` | Compatibility client shim that re-exports provider surfaces | Compatibility exports only |
| `meerkat-auth-core` | Shared auth primitives, token stores, OAuth helpers, cloud authorizers | — |
| `meerkat-providers` | Compatibility provider-runtime/auth shim surface | — |
| `meerkat-anthropic` / `meerkat-openai` / `meerkat-gemini` | Provider-specific client/runtime implementations | Implements `AgentLlmClient` via provider-specific crates |
| `meerkat-store` | Session-store implementations and adapters (SQLite, Jsonl, Memory) | Implements `SessionStore` |
| `meerkat-tools` | Tool registry, builtins, shell, session-scoped task store | Implements `AgentToolDispatcher` |
| `meerkat-mcp` | MCP client, protocol transport, router (routes to `ExternalToolSurfaceHandle`) | — |
| `meerkat-session` | Session orchestration (Ephemeral, Persistent), turn admission slot (shell) | Implements `SessionService` |
| `meerkat-runtime` | Runtime control plane, policy engine, detached wake, DSL handle impls, **`reconfigure_live_topology` orchestration, realtime attachment public methods** | `RuntimeControlPlane`, `RuntimeDriver`, `MeerkatMachine` |
| `meerkat-comms` | Inter-agent messaging (inproc, TCP, UDS, Ed25519), peer identity claims, pure peer data types | Implements `CommsRuntime` |
| `meerkat-hooks` | Hook runtimes (in-process, command, HTTP) | Implements `HookEngine` |
| `meerkat-skills` | Skill loading (filesystem, git, HTTP, embedded) | Implements `SkillEngine` |
| `meerkat-memory` | Semantic memory stores and retrieval | Implements `MemoryStore` |
| `meerkat-mob` | Multi-agent orchestration, member provisioning, flow runtime, **identity-first binding model, supervisor bridge** | `MobSessionService`, `MobProvisioner`, `MobMemberRuntimeBridge` |
| `meerkat-mob-pack` | Mobpack archive format, signing, trust policies, validation | — |
| `meerkat-mob-mcp` | MCP/operator mob surface plus agent-facing delegation tool surface | `MobMcpState`, `AgentMobToolSurfaceFactory` |
| `meerkat-schedule` | Scheduler subsystem; `Schedule::apply` / `Occurrence::apply` on domain types | `ScheduleService`, `ScheduleDriver`, `ScheduleStore` |
| `meerkat-web-runtime` | WASM browser deployment (wasm_bindgen exports) | — |
| `meerkat-machine-schema` | Rust-native machine/composition catalog DSL — the formal authority | — |
| `meerkat-machine-kernels` | Generated kernel interpreter for all machines/compositions | `GeneratedMachineKernel` |
| `meerkat-machine-codegen` | TLA+ model generation, TLC verification, drift detection | — |
| `meerkat` (facade) | `AgentFactory`, `FactoryAgentBuilder`, persistence helpers, re-exports, **`SessionLlmReconfigureHost` wiring** | Wires everything together |

**Rule: `meerkat-core` has zero I/O dependencies.** All I/O happens in satellite crates.

For detailed crate-by-crate reference: load `references/crate_map.md`.

## Reference Navigator

Load these as needed. SKILL.md alone is intentionally minimal — everything else lives in `references/` for progressive disclosure.

- **`references/machine-system.md`** — load when touching DSL sources, catalog schemas, generated kernels, TLC verification, authority cutover, handle trait design, or any "where does this semantic state live" question. Covers the DSL → MachineSchema → kernel → TLA+ → runtime flow, the field-driven design principle, signals vs inputs, and the `HandleDslAuthority` cross-crate pattern.
- **`references/runtime-control-plane.md`** — load when working on `MeerkatMachine`, runtime drivers, session registration, policy resolution, `RuntimeBuildMode` / `SessionRuntimeBindings`, `OpsLifecycleRegistry`, session service lifecycle, persistence pairing, detached-op wake, or test harness ownership.
- **`references/realtime-attachment.md`** — load when working on realtime attachment state, the capability-driven transport policy (`ModelCapabilities.realtime`), the live-topology reconfigure flow, provider callback authority epochs, or the peer-response-terminal context append path. Covers the DSL state fields, the `RealtimeAttachmentSignalAuthority` token, and the five CoreExecutor entry points that route context-only staged primitives.
- **`references/agent-construction.md`** — load when touching `AgentFactory::build_agent()`, agent builder, multimodal content types, or runtime tool scoping.
- **`references/mob-orchestration.md`** — load when working on mobs: creation, launch modes, spawn policies, delegation tools, lifecycle control, provisioning, wiring, flow/frame execution, mob persistence, or `MobActor` decomposition.
- **`references/comms-model.md`** — load when working on peer trust, inter-agent messaging, comms drain lifecycle, envelope classification, or session identity claims.
- **`references/gotchas.md`** — load as the first review lens for non-trivial changes. Regression checklist of architectural invariants that quietly re-break.
- **`references/crate_map.md`** — detailed crate-by-crate reference.

## Key files (quick index)

For comprehensive file lists, see the matching reference. This is a minimal pointer index for the most common landmarks.

- `meerkat-machine-schema/src/catalog/dsl/` — DSL sources (truth for all 5 canonical machines)
- `meerkat-machine-schema/src/catalog/mod.rs` — `canonical_machine_schemas()` registry
- `meerkat-machine-kernels/src/runtime.rs` — `GeneratedMachineKernel` interpreter
- `meerkat-runtime/src/meerkat_machine/` — `MeerkatMachine`, session management, dispatch paths, DSL adapter
- `meerkat-runtime/src/meerkat_machine/llm_reconfigure.rs` — `reconfigure_live_topology` orchestration
- `meerkat-runtime/src/handles/` — runtime impls of DSL handle traits
- `meerkat-core/src/handles.rs` — DSL handle trait definitions
- `meerkat-core/src/runtime_epoch.rs` — `SessionRuntimeBindings`, `RuntimeBuildMode`
- `meerkat-core/src/agent.rs`, `meerkat-core/src/agent/*.rs` — agent loop
- `meerkat/src/factory.rs` — `AgentFactory::build_agent()` (pipeline)
- `meerkat-session/src/{ephemeral,persistent}.rs` — session services
- `meerkat-mob/src/runtime/actor.rs` — `MobActor`
- `meerkat-mob/src/backend.rs`, `meerkat-mob/src/ids.rs` — identity-first binding model
- `meerkat-mob/src/runtime/supervisor_bridge.rs` — supervisor bridge transport
- `meerkat-mob/src/runtime/local_bridge.rs` — in-process MeerkatMachine bridge
- `meerkat-mob-mcp/src/agent_tools.rs` — agent-facing delegation/orchestration tools
- `meerkat-contracts/src/wire/supervisor_bridge.rs` — bridge protocol types
- `docs/architecture/meerkat-runtime-dogma.md` — full dogma
- `docs/architecture/identity-first-live-voice-proposal.md` — realtime + identity-first design notes
- `docs/guides/realtime.mdx` — user-facing realtime voice guide (public vocabulary, state enum, reconfigure flow)
- `tests/integration/src/e2e_lanes.rs` — authoritative e2e lane catalog
