# Regression Checklist and Gotchas

Load this reference as the first review lens when touching runtime, mob, comms, agent-construction, persistence, or tool-scope code. These are the architectural invariants that quietly re-break if not explicitly checked.

## Architectural invariants (must hold)

1. **Never bypass `AgentFactory::build_agent()`** — all agent construction goes through this pipeline.
2. **Never import implementations in business logic** — use traits from `meerkat-core`.
3. **No sub-agent system** — all multi-agent work goes through mobs. No `SubAgentManager`, no `agent_spawn`/`agent_fork`.
4. **Runtime conforms to machines** — runtime behavior matches verified machine schemas. No owner-crate `machines/mod.rs` re-exports; centralized `meerkat-machine-kernels` is the enforced design.
5. **Shell may execute mechanics; it may not invent seam semantics** — evidence capture is allowed, but feedback mapping, barrier membership, and terminal class must be protocol-constrained or machine-derived.
6. **DSL is sole authority for absorbed domains** — every `*_authority.rs` file outside `meerkat-machine-kernels` is either a DSL adapter (`dsl_authority.rs`), a pure data projection (e.g., `roster`, `mob_member_lifecycle`), or a planning helper (e.g., `mob_wiring`, `mob_runtime_bridge`). None contain `fn apply(input) -> Result<Transition, Error>` match tables.
7. **No shadow semantic truth** — if a helper, cache, queue, or surface carries authoritative meaning beside the machine/composition/protocol owner, the design is wrong.
8. **Errors separate mechanism from policy** — `ToolError → AgentError → SessionError`.
9. **Wire types ≠ domain types** — `meerkat-contracts` owns wire format; `meerkat-core` owns domain types.
10. **Sessions are first-class, persistence is optional** — Ephemeral and Persistent share the same trait.
11. **No backward compatibility aliases** — clean cut. No serde aliases for old names, no `pub use` of deleted types under new names, no `#[deprecated]` markers.
12. **No `.unwrap()`/`.expect()`/`panic!()` in library code** — use `?` propagation or explicit error handling.
13. **Runtime-backed builds require bindings** — `RuntimeBuildMode::SessionOwned(bindings)` for runtime-backed surfaces; `StandaloneEphemeral` for WASM/tests/embedded. Factory never creates a competing registry for `SessionOwned`.
14. **One recovery seam for epoch state** — `recover_or_create_ops_state()` on `MeerkatMachine` is the single canonical recovery helper. Both `register_session()` and `ensure_session_with_executor()` use it.
15. **No raw infra IDs as app-facing control nouns** — use domain handles publicly (`AgentIdentity`, `MemberRef`, `job_id`, `session_id`) and keep canonical raw identity infra-only unless there is a very strong reason not to.
16. **Definition-only mob creation** — `MobDefinition` is the only creation input. Do not resurrect prefabs or hidden template injection.
17. **Persist intent, not resolved defaults** — use `ToolCategoryOverride` / `from_override()` when storing tooling policy.
18. **One canonical step path** — `execute_step_with_all_guards()` is shared by flat and frame execution; parallel executors are a regression factory.
19. **Operator authority is injected, not ambient** — mob support being enabled must not surface operator tools without runtime context.
20. **Runtime owns detached wake** — background-op completion wakeups flow through `DetachedWakeState` + `ContinuationInput`, not surface code.

## Gotchas to check on every non-trivial change

- **Prefabs are gone.** `MobDefinition` is the only creation input.
- **"Delegate" / "sub-agent" UX is mob-backed and session-owned.** Use canonical `owner_bridge_session_id`, `is_implicit`, and `destroy_session_mobs()` seams.
- **Persist `ToolCategoryOverride` intent**; never flatten `Inherit` into a resolved bool on save/resume paths.
- **Background-op wake is runtime-owned** via `DetachedWakeState` + `ContinuationInput`; surfaces must not spawn bespoke waker loops.
- **Only actionable peer inputs may carry `handling_mode`**; response progress/terminal traffic must fall back to kind-based policy.
- **`FlowEngine::execute_step_with_all_guards()` is the only canonical step path**; frame-step outcomes route back through MobMachine DSL.
- **Mob persistence is SQLite/WAL-backed.** Avoid lock-holding backends or split store state.
- **Agent mob tools and archive cleanup must share the same hydrated `MobMcpState`**; parallel shadow states are architectural bugs.
- **`input_terminal_outcomes` / `input_attempt_counts`**: future DSL structural upgrades pending; until then, shell owns these two InputState fields explicitly (annotated with doc comments).
- **Generated protocol files in `src/generated/`**: never hand-edit. Regenerate via `make machine-codegen` after any DSL change.
- **Drift check before commit**: `make machine-check-drift` must be clean; if it reports stale artifacts, `make machine-codegen` was forgotten.
- **TLC at CI bounds**: `make machine-verify` passing at small bounds does not prove the runtime is bug-free at production scale; treat it as strong evidence, not a certificate.

## Review lens — ask these before accepting a change

- What semantic facts are introduced or changed?
- Who is the one canonical owner of each fact?
- Is any shell/helper/cache still carrying parallel truth?
- Are app-facing APIs exposing domain handles rather than infra identity?
- If raw infra identity escapes, is it definitely canonical?
- If policy depends on dynamic identity, where is recomputation defined?
- Did the change reduce ambiguity, or merely relocate it?

Full dogma: `docs/architecture/meerkat-runtime-dogma.md`.
