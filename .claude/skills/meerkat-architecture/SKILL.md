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

## The 4-machine target

Exactly four canonical machines, each with a DSL source in `meerkat-machine-schema/src/catalog/dsl/`:

- **MeerkatMachine** — session-scoped execution kernel. Owns session lifecycle, input admission, ops lifecycle, turn execution, tool surface state, drain lifecycle, peer comms classification.
- **MobMachine** — mob-scoped orchestration. Owns mob lifecycle, member lifecycle, kickoff, wiring, roster, flow/frame/loop execution.
- **ScheduleLifecycleMachine** — scheduler triggers and schedule lifecycle.
- **OccurrenceLifecycleMachine** — occurrence dispatch and delivery.

Plus four composition protocols at the seams: `meerkat_mob_seam`, `schedule_bundle`, `schedule_runtime_bundle`, `schedule_mob_bundle`.

**Zero handwritten `*_authority.rs` state machines exist outside the catalog.** Shell helpers that retain `_authority.rs` naming are pure data projections (`roster`), planning helpers (`mob_wiring`), or DSL adapter plumbing (`dsl_authority`) — none contain `fn apply(input) -> Result<Transition, Error>` match tables.

Detailed architecture, DSL ↔ schema ↔ kernel ↔ TLA+ flow, the field-driven design principle, signals vs inputs, and the cross-crate handle trait pattern: **load `references/machine-system.md`**.

## Crate Ownership

| Crate | Owns | Key Trait |
|-------|------|-----------|
| `meerkat-models` | Model catalog, provider profiles, parameter schemas (leaf; no meerkat deps) | — |
| `meerkat-core` | Agent loop, core types, session-store contract, ALL trait contracts, DSL handle traits | `AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`, `SessionStore`, `SessionService`, `CommsRuntime`, `HookEngine`, `OpsLifecycleRegistry`, `TurnStateHandle`, `CommsDrainHandle`, `ExternalToolSurfaceHandle`, `PeerCommsHandle`, `SessionAdmissionHandle` |
| `meerkat-contracts` | Wire types, catalogs, stable error codes, generated surface schemas | — |
| `meerkat-client` | LLM providers (Anthropic, OpenAI, Gemini) | Implements `AgentLlmClient` |
| `meerkat-store` | Session-store implementations and adapters (SQLite, Jsonl, Memory) | Implements `SessionStore` |
| `meerkat-tools` | Tool registry, builtins, shell, session-scoped task store | Implements `AgentToolDispatcher` |
| `meerkat-mcp` | MCP client, protocol transport, router (routes to `ExternalToolSurfaceHandle`) | — |
| `meerkat-session` | Session orchestration (Ephemeral, Persistent), turn admission slot (shell) | Implements `SessionService` |
| `meerkat-runtime` | Runtime control plane, policy engine, detached wake, DSL handle impls | `RuntimeControlPlane`, `RuntimeDriver`, `MeerkatMachine` |
| `meerkat-comms` | Inter-agent messaging (inproc, TCP, UDS, Ed25519), peer identity claims, pure peer data types | Implements `CommsRuntime` |
| `meerkat-hooks` | Hook runtimes (in-process, command, HTTP) | Implements `HookEngine` |
| `meerkat-skills` | Skill loading (filesystem, git, HTTP, embedded) | Implements `SkillEngine` |
| `meerkat-memory` | Semantic memory stores and retrieval | Implements `MemoryStore` |
| `meerkat-mob` | Multi-agent orchestration, member provisioning, flow runtime | `MobSessionService`, `MobProvisioner` |
| `meerkat-mob-pack` | Mobpack archive format, signing, trust policies, validation | — |
| `meerkat-mob-mcp` | MCP/operator mob surface plus agent-facing delegation tool surface | `MobMcpState`, `AgentMobToolSurfaceFactory` |
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

- **`references/machine-system.md`** — load when touching DSL sources, catalog schemas, generated kernels, TLC verification, authority cutover, handle trait design, or any "where does this semantic state live" question. Covers the DSL → MachineSchema → kernel → TLA+ → runtime flow, the field-driven design principle, signals vs inputs, and the `HandleDslAuthority` cross-crate pattern.
- **`references/runtime-control-plane.md`** — load when working on `MeerkatMachine`, runtime drivers, session registration, policy resolution, `RuntimeBuildMode` / `SessionRuntimeBindings`, `OpsLifecycleRegistry`, session service lifecycle, persistence pairing, detached-op wake, or test harness ownership.
- **`references/agent-construction.md`** — load when touching `AgentFactory::build_agent()`, agent builder, multimodal content types, or runtime tool scoping.
- **`references/mob-orchestration.md`** — load when working on mobs: creation, launch modes, spawn policies, delegation tools, lifecycle control, provisioning, wiring, flow/frame execution, mob persistence, or `MobActor` decomposition.
- **`references/comms-model.md`** — load when working on peer trust, inter-agent messaging, comms drain lifecycle, envelope classification, or session identity claims.
- **`references/gotchas.md`** — load as the first review lens for non-trivial changes. Regression checklist of architectural invariants that quietly re-break.
- **`references/crate_map.md`** — detailed crate-by-crate reference.

## Key files (quick index)

For comprehensive file lists, see the matching reference. This is a minimal pointer index for the most common landmarks.

- `meerkat-machine-schema/src/catalog/dsl/` — DSL sources (truth for all 4 machines)
- `meerkat-machine-schema/src/catalog/mod.rs` — `canonical_machine_schemas()` registry
- `meerkat-machine-kernels/src/runtime.rs` — `GeneratedMachineKernel` interpreter
- `meerkat-runtime/src/meerkat_machine/` — `MeerkatMachine`, session management, dispatch paths, DSL adapter
- `meerkat-runtime/src/handles/` — runtime impls of DSL handle traits
- `meerkat-core/src/handles.rs` — DSL handle trait definitions
- `meerkat-core/src/runtime_epoch.rs` — `SessionRuntimeBindings`, `RuntimeBuildMode`
- `meerkat-core/src/agent.rs`, `meerkat-core/src/agent/*.rs` — agent loop
- `meerkat/src/factory.rs` — `AgentFactory::build_agent()` (pipeline)
- `meerkat-session/src/{ephemeral,persistent}.rs` — session services
- `meerkat-mob/src/runtime/actor.rs` — `MobActor`
- `meerkat-mob-mcp/src/agent_tools.rs` — agent-facing delegation/orchestration tools
- `docs/architecture/meerkat-runtime-dogma.md` — full dogma
- `tests/integration/src/e2e_lanes.rs` — authoritative e2e lane catalog
