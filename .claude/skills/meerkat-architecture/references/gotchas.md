# Regression Checklist and Gotchas

Load this reference as the first review lens when touching runtime, mob, comms, agent-construction, persistence, or tool-scope code. These are the architectural invariants that quietly re-break if not explicitly checked.

## Architectural invariants (must hold)

1. **Never bypass `AgentFactory::build_agent()`** — all agent construction goes through this pipeline.
2. **Never import implementations in business logic** — use traits from `meerkat-core`.
3. **No sub-agent system** — all multi-agent work goes through mobs. No `SubAgentManager`, no `agent_spawn`/`agent_fork`.
4. **Runtime conforms to catalog-generated machines** — runtime behavior matches verified machine schemas. Production bridge modules import/invoke catalog-owned DSL bodies; centralized `meerkat-machine-kernels` is the enforced design.
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
20. **Runtime owns detached wake** — background-op completion wakeups flow through `RuntimeCompletionFeed` / `EpochCursorState` + `ContinuationInput`, not surface code.
21. **No production-only machine semantics** — new state fields, transitions, effects, helpers, and invariants land in catalog DSL first. Production bridge code may convert/realize, not decide.
22. **Typed command parity only** — command classification uses typed manifests. String exception tables, wildcard "allowed" branches, and command-name folklore are dogma violations.
23. **Mob flow helper reducers are not machines** — `flow_run`, `flow_frame`, and `loop_iteration` are MobMachine-owned fail-closed projection reducers. They must not become independent semantic owners.
24. **`auth_binding` propagates everywhere a credential decision is made.** `SessionLlmIdentity.auth_binding` is persisted, hot-swap re-resolves through the same binding, and surfaces (RPC `session/create`, REST `POST /sessions`, MCP, SDKs, Web `runtime.createSession` / `mob.spawn`) must accept and forward it. Surface-level rejection of typed `auth_binding` variants is scope reduction, not pragmatism.
25. **`AuthLeaseHandle` is the credential consumer**, not raw env reads. New code that touches provider auth must obtain a lease through `ProviderRuntimeRegistry` (or, in the WASM external-resolver case, through the JS callback). `std::env::var("ANTHROPIC_API_KEY")` outside the registry is a dogma violation.
26. **Provider-native web search and tool capability is per-model.** `ModelProfile` carries flat capability fields (`supports_web_search`, vision, image_tool_results, realtime, …) that gate tool/feature visibility — when a tool unexpectedly disappears under a particular model, check the profile capability before assuming a bug. There is no nested `ModelProfile.tools` namespace.
27. **MCP provenance filtering on mob profiles.** Mob-profile-injected MCP servers are filtered by source provenance at agent build; review changes to MCP loading paths against the profile filter, not just the global `.rkat/mcp.toml`.
28. **Public wire schemas fail closed.** `meerkat-contracts` schema artifacts must be regenerated (`make regen-schemas`) and `make verify-schema-freshness` must pass — the SDKs are codegen targets, not handwritten clients. Adding a wire field without regenerating is a CI failure waiting to happen.
29. **Live channels replaced the realtime attachment plane.** The `session/realtime_attachment_status`, `realtime/open_info`, `realtime/status`, and `realtime/capabilities` methods have been removed. Live channels use the caller-initiated `live/*` method family. `ModelCapabilities.realtime` remains the capability gate.
30. **`meerkat-models` owns ALL provider model data.** `meerkat-core/src/model_profile/` holds only vocabulary types + `ModelCatalog` mechanics; core takes catalog data as an explicit parameter and must contain zero model-name literals (pinned by `xtask/tests/no_provider_data_in_core.rs`). Do not put new model rows in core; do not put new model code in the legacy `meerkat-client` shim.
31. **Web SDK auth model.** Browser-hosted flows use `authBinding` + `registerExternalAuthResolver` (resolves to typed `AuthCredential`s in the host page) — per-session `apiKey` fields were removed. New WASM-bridged auth wiring must go through the resolver path, not by re-introducing inline credentials.
32. **Auth tokens commit acquire-first.** A persisted token without the AuthMachine lifecycle acquisition marker is dead data: `publish_token_lifecycle_acquired` (meerkat-core/src/auth/lifecycle.rs) is the durable proof-of-acquisition, and a failed save must roll the acquired lease back so no half-state survives. `TokenStore` (meerkat-core/src/auth/token_store.rs) is the vault contract — do not write token files around it.
33. **Trust is PeerId-keyed.** `TrustStore` (meerkat-comms/src/trust.rs) keys entries on `PeerId` and rejects duplicates with a typed error. Recipient-side wiring trust is a MobMachine obligation (`pending_recipient_trust: Set<PeerId>`), not fire-and-forget shell work.
34. **Member revival is machine-authorized.** Post-discard revival flows observe → classify → realize through MobMachine (`member_revival_pending`, `MemberLiveMaterializationClassified`, `ResolveMemberRevival*`). `Broken` is terminal — the `not_broken` guard refuses retry and `member()` returns typed `MobError::MemberRestoreFailed`. Do not add shell-side retry loops.
35. **Image-gen routing follows session identity.** The planner resolves the image target from `SessionModelRoutingStatus.session_provider` (typed session provider identity); model-name inference (`infer_from_model`) was deleted from the planner path. Do not reintroduce provider-prefix sniffing in image routing.
36. **Model fallback is a core-applied switch, not a hidden client retry.** Generated recovery authority classifies the LLM failure; core decides pre-stream retry safety and applies identity, auth lease, request policy, token limits, and tool visibility before retry. The fallback client only selects a prebuilt candidate from the factory-resolved chain.
37. **Identity-first lease re-acquire is re-fencing, not idempotent ownership.** `LeaseProvider::acquire_leases` minting a NEW (strictly-higher) `FencingToken` when the *same* `runtime_instance` re-acquires an already-held identity is INTENTIONAL: it re-establishes ownership and fences a prior session's stale writes. `retire`/`respawn`/`reset` (and `restore_flow` on every reconcile) depend on the bump, and tests enforce it — `identity_first_runtime_retire_returns_advanced_fencing_token` asserts the returned token advances *and* that old-token writes are then rejected; `restore_flow_releases_active_lease_on_customizer_failure` asserts `restore_flow` refreshes (advances) the active lease before customizer work. Do **not** "fix" the per-acquire bump by making `acquire_leases` idempotent for the same holder — it looks like wasteful churn (continuity-record tokens climb each reconcile) but it is the fencing mechanism, and making it idempotent breaks both tests and the invariant. The orphan-on-retry story (`renew(N)`→Lost) is unreachable: `mark_lifecycle_in_progress` nulls `entry.lease` and the bundled provider is single-process. (Distinct, *real* bug — already fixed: MobKit's bundled single-process `LocalLeaseProvider` (`meerkat-mobkit/src/identity_first/local_lease.rs`) reset its in-memory counter to 1 on **restart**, presenting a stale token that aborted boot; fixed in meerkat-mobkit 0.7.9 by seeding the counter from the persisted continuity high-water. That is cross-restart monotonicity, NOT same-process idempotence — keep the two concerns separate.)

38. **Injected context is slot-derived typed truth with ONE lowering owner per mode.** Direct-path materialization in `run_inner` fires only when `typed_turn_appends` is empty; the runtime path carries a distinct `PromptInput` slot lowered into `ConversationAppendRole::InjectedContext` appends ordered BEFORE the user append (never chained after via `additional_appends`); staged promotion must carry it alongside the deferred prompt. Hosts can never mint `CompactionSummary` — rewrite ingress accepts only `{conversational, injected_context}` and rejects `compaction_summary` fail-closed.
39. **`indexable_content()` consults `transcript_role`; the store owns include/exclude.** New user-channel message classes extend `MemoryIndexExclusion` — never producer-side pre-filtering in `index_compaction_discards`, never content sniffing. The transcript-continuity save-guard keys on `is_compaction_summary()` only; new roles must not satisfy it.
40. **Hnsw memory store scoped-SQL discipline.** Every scoped op (lazy scope load, `drop_scope`, `enumerate_scoped`) runs the idempotent NULL-`session_id` heal first (old-binary writers must never become invisible); point ids come from the durable allocator table and are NEVER reused after `drop_scope`; a scope index for an existing scope must build from durable rows — an `or_insert_with(empty)` on the index path silently hides prior entries.
41. **`ExecutionPolicyGatedDispatcher` wraps OUTERMOST and forwards the ENTIRE dispatcher surface** — explicitly including `bind_mcp_server_lifecycle_handle` and `bind_external_tool_surface_handle`, as `FilteredToolDispatcher` does, so either wrapper preserves MCP DSL lifecycle mirroring. Policy deny is an ordinary `access_denied` tool error (run continues); hook denials remain the separate run-fatal channel (Rule 8: different mechanisms). `Inherit` resolves to the PARENT's effective policy from session metadata — resolving it to Unrestricted reintroduces the escape-by-spawn containment hole.
42. **Comms `content_taint` lives INSIDE the signed region and None ≠ Clean.** Absent keeps envelopes byte-identical (pin tests enforce it); present fails verification closed on pre-field receivers. Receivers must never coalesce a missing declaration into Clean. Taint is content-adjacent typed metadata — the moment any meerkat hop makes an admission/routing/rendering-class decision on it, it must fold into the ClassifyExternalEnvelope machine facts (R084), not stay shell-carried.
43. **The compaction curator substitutes summary CONTENT only.** Trigger stays `should_compact` under the machine-emitted CheckCompaction effect; curator failure is typed `CompactionFailureReason::CuratorFailed` with NO silent LLM fallback (one condition, one path); curated summaries carry the typed `CompactionSummary` role and therefore stay index-excluded; the indexing-gates-commit ordering is untouched.

## Gotchas to check on every non-trivial change

- **Prefabs are gone.** `MobDefinition` is the only creation input.
- **"Delegate" / "sub-agent" UX is mob-backed and session-owned.** Use canonical `owner_bridge_session_id`, `is_implicit`, and `destroy_session_mobs()` seams.
- **Persist `ToolCategoryOverride` intent**; never flatten `Inherit` into a resolved bool on save/resume paths.
- **Background-op wake is runtime-owned** via `RuntimeCompletionFeed` / `EpochCursorState` + `ContinuationInput`; surfaces must not spawn bespoke waker loops.
- **Only actionable peer inputs may carry `handling_mode`**; response progress/terminal traffic must fall back to kind-based policy.
- **`FlowEngine::execute_step_with_all_guards()` is the only canonical step path**; frame-step outcomes route back through MobMachine DSL.
- **Mob persistence is SQLite/WAL-backed.** Avoid lock-holding backends or split store state.
- **Agent mob tools and archive cleanup must share the same hydrated `MobMcpState`**; parallel shadow states are architectural bugs.
- **Input lifecycle facts are DSL-owned**: `input_phases`, `input_terminal_kind`, `input_attempt_counts`, `input_admission_seq`, etc. are authoritative in MeerkatMachine DSL. `InputStateSeed` (meerkat-runtime/src/input_state.rs) carries them alongside persisted `InputState` only at the store boundary; inside a running driver they are always read from the DSL, never from the seed.
- **Generated protocol files in `src/generated/`**: never hand-edit. Regenerate via `make machine-codegen` after any DSL change.
- **Drift check before commit**: `make machine-check-drift` must be clean; if it reports stale artifacts, `make machine-codegen` was forgotten.
- **Schema/alphabet parity before commit**: `runtime_schema_parity`, `runtime_alphabet_parity`, and render-contract audits must stay green; they are the ratchet for #38/#39.
- **TLC at CI bounds**: `make machine-verify` passing at small bounds does not prove the runtime is bug-free at production scale; treat it as strong evidence, not a certificate.

## Review lens — ask these before accepting a change

- What semantic facts are introduced or changed?
- Who is the one canonical owner of each fact?
- Is any shell/helper/cache still carrying parallel truth?
- Are app-facing APIs exposing domain handles rather than infra identity?
- If raw infra identity escapes, is it definitely canonical?
- If policy depends on dynamic identity, where is recomputation defined?
- Did the change reduce ambiguity, or merely relocate it?

Canonical doctrine: `docs/architecture/meerkat-dogma.md` (+ commentary in `docs/architecture/meerkat-dogma-commentary.md`).
Public machine-authority summary: `docs/reference/machine-authority.mdx`.
Historical internal doctrine archive (legacy rules #1–#20): `docs-internal/archive/public-docs-removed-2026-05-11/architecture/meerkat-runtime-dogma.md`.
