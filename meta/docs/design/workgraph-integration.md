# WorkGraph Integration (mobkit 0.7.30)

Full integration of meerkat 0.7.23's WorkGraph (goals, work items, attention
bindings, apply-time attention overlays) into Meta: runtime wiring, agent
tool surface, `meta/workgraph/*` JSON-RPC, Python + TypeScript SDK parity,
and a conversation-native console widget.

This is the historical 0.7.30 implementation plan and upstream survey, not a
current source-line inventory. For the current immutable namespace-grant
boundary, see the [wire contract qualification](workgraph-wire-contract.md#current-scope-qualification-meerkat-0840).

Decision (Luka, 2026-07-08): the console centerpiece is an INLINE, expandable,
interactive, live-updating WorkGraph card in the chat pane, rendered when an
agent's turn calls workgraph tools. A light workbench panel is secondary.

## Upstream surface (meerkat =0.7.23, verified)

- Service: `WorkGraphService::with_scope(store, realm_id, WorkNamespace::default())`
  (`meerkat/src/lib.rs:226`). Store: `SqliteWorkGraphStore::open(path)`
  (`meerkat-workgraph/src/store.rs:807`); canonical filename `workgraph.sqlite3`;
  `MemoryWorkGraphStore` for ephemeral. Service methods: create/create_goal/
  goal_status/goal_confirm/goal_request_close/attention_* (list/pause/resume/
  reassign/projection)/get/list/ready/snapshot/claim/release/update/
  escalate_policy (monotonic tighten only)/block/close/link/add_evidence/events
  (`meerkat-workgraph/src/service.rs:37-945`).
- Member tools: `FactoryAgentBuilder.default_workgraph_tools` slot, injected into
  every build (`meerkat/src/service_factory.rs:876-884`); fill via
  `meerkat::surface::set_default_workgraph_tools(&builder, Some(Arc::new(
  WorkGraphToolSurface::new(service))))` (`meerkat/src/surface/embedded.rs:20-27`).
  Per-profile gate `tools.workgraph` DEFAULT FALSE (`meerkat-mob/src/profile.rs:330`;
  agent-side `Config.tools.workgraph_enabled` default false,
  `meerkat-core/src/config.rs:2041`). Builtin skill `workgraph-workflow`
  auto-preloads for opted-in profiles. 15 tool names:
  `workgraph_create|get|list|ready|snapshot|events|claim|release|update|
  policy_escalate|block|close|link|add_evidence|attention_reassign`.
- Attention overlays: `MobBuilder::with_workgraph_service(Option<WorkGraphService>)`
  (`meerkat-mob/src/runtime/builder.rs:1952`) → provisioner injects
  `inject_workgraph_attention_turn_overlay` BEFORE `apply_runtime_turn`
  (`meerkat-mob/src/runtime/provisioner.rs:1837`). Overlay rides
  `req.runtime.turn_tool_overlay` + `turn_metadata.turn_tool_overlay`. VERIFIED:
  identity-first bridge submits (`bridge.rs:230` submit_work lane) and schedule
  internal deliveries (`schedule_wiring.rs:591`) both ride this executor;
  meta's `normalize_runtime_turn_request` does NOT strip the overlay
  (only handling_mode + render_metadata). Failure mode (HISTORICAL, ≤0.7.24):
  `MultipleActiveBindings` was a HARD per-turn error — meerkat 0.7.25 (ask 25)
  removed it (newest-binding-wins arbitration with a loud diagnostic) and
  moved binding uniqueness into the store; meta's admission demotes to
  defense-in-depth + alias unification (see workgraph-wire-contract.md).
- Nested mobs: `MobMcpState::with_workgraph_service` (`meerkat-mob-mcp/src/lib.rs:245`)
  — thread in meta's `install_agent_mob_tools` so delegate/spawned child mobs inherit.
- Schedule host: `spawn_runtime_backed_schedule_host_with_mobs(..., workgraph_service, owner_id)`
  arg 8 (currently `None` at `schedule_wiring.rs:758`).
- Authority witness: `AttentionContextProjection` (`types.rs:1521`) with
  `ProjectedAttentionAuthority` capability bits — SERVER-INJECTED; wire callers
  cannot forge it. `reassign_attention`/`escalate_policy` require it: meta RPC
  fetches `attention_projection(binding_id)` server-side then calls the mutation.
  `GoalConfirmRequest.principal` is `#[serde(skip)]` — host promotes via
  `with_trusted_principal` (console: authenticated principal).
- Owner-key lowering for identity-addressed attention:
  `meerkat_mob::lower_agent_identity_attention_target` → `mob/<mob_id>/agent/<identity>`.
- CAS: revision-checked mutations of existing work items or attention bindings
  require `expected_revision: u64`; conflicts are typed errors. `create`,
  `goal/create`, `link`, and `attention/prune` have no caller-supplied CAS token
  (their transactional invariants still apply). Item mutations and
  `goal/confirm`/`goal/request_close` use the work item's `revision`;
  `attention/pause`/`attention/resume`/`attention/reassign` use
  `attention.machine_state.revision`. Console cards must retain the relevant
  revisions.

## Rust core (Stage A)

1. New `crates/meerkat-meta/src/workgraph_wiring.rs`:
   `pub const WORKGRAPH_STORE_FILE: &str = "workgraph.sqlite3";`
   `attach_workgraph_tools(&FactoryAgentBuilder, state_dir, realm_id) -> Option<WorkGraphService>`
   = open sqlite store (warn + None on failure — boot-without, matches schedule),
   `with_scope(store, realm_id, default ns)`, `set_default_workgraph_tools`.
   Ephemeral constructor: `attach_workgraph_tools_ephemeral` with `MemoryWorkGraphStore`
   (meta_gateway default launch is ephemeral; feature should exist there too,
   tools stay profile-gated so nothing changes for existing consumers).
2. `MobBootstrapSpec.workgraph_service: Option<WorkGraphService>` +
   `with_workgraph_service`; `MobRuntime::bootstrap` forwards to
   `MobBuilder::with_workgraph_service` (`mob_handle_runtime.rs:~3016`);
   `install_agent_mob_tools` forwards to `MobMcpState::with_workgraph_service`
   (`mob_handle_runtime.rs:940`).
3. `UnifiedRuntime`: `workgraph_service: Option<WorkGraphService>` seeded once
   from the bootstrap spec + `workgraph_service()` accessor. There is
   deliberately NO late setter: the admission machinery (cross-process
   sidecar, tool-plane slots) freezes at `MobRuntime::bootstrap`, so
   spec-at-bootstrap is the only blessed wiring.
4. `spawn_schedule_host` gains `workgraph_service: Option<WorkGraphService>`
   param → upstream arg 8. Call sites: rpc_gateway.rs:4588, meta_gateway.rs:867,
   in-crate e2e harness.
5. rpc_gateway: construct in persistent branch (`state_path.join(WORKGRAPH_STORE_FILE)`,
   realm = `definition.id`) beside `attach_schedule_tools_with_identity_targets`
   (:3684); runtime_options allowlist gains `workgraph` (bool, default true;
   `false` disables construction entirely); thread to spec/schedule-host/runtime slot.
   Ephemeral branch: memory store (unless `workgraph:false`).
6. meta_gateway: hoist `load_definition` above `build_persistent_session_service`
   (needed for realm scoping); construct in `build_persistent_session_service`
   (extend `PersistentSessionServiceParts`); ephemeral launch gets memory-store
   service; thread same as rpc_gateway. No new InitParams field.

## JSON-RPC (Stage A, same pass)

New `src/rpc/workgraph_methods.rs` (gating_methods.rs template). Methods
(params → service call → serde of upstream typed results, snake_case):

Read: `meta/workgraph/snapshot`, `list`, `get`, `ready`, `events`,
`attention/list`, `goal/status`.
Mutate: `meta/workgraph/create`, `update`, `claim`, `release`, `close`,
`block`, `link`, `evidence/add`, `policy/escalate`, `goal/create`,
`goal/confirm`, `goal/request_close`, `attention/pause`, `attention/resume`,
`attention/reassign`.

- `goal/create` accepts `target: {kind:"session",session_id} | {kind:"identity",identity}` —
  identity lowered via `lower_agent_identity_attention_target` (+ mob id from runtime).
- `attention/reassign` + `policy/escalate`: server-side witness injection
  (fetch `attention_projection` internally; wire callers pass binding_id/
  expected_revision/target only).
- `goal/confirm` on console surface: promote authenticated principal via
  `with_trusted_principal`.
- "Backend not configured" error: typed code, memory-backend pattern (rpc.rs:358).
- Registered in ALL dispatch tables: unified rpc.rs (match + capabilities table)
  and console http_console.rs (match + grant-intersected table). Module-mode
  (rpc.rs:481) does NOT get workgraph (no runtime).
- Capabilities: `workgraph: true` in `meta/capabilities`.
- ABAC: new actions `workgraph.view` (reads) + `workgraph.manage` (mutations)
  in `access/model.rs` vocabulary; console_rpc_access_requirement maps the
  method groups; experience payload gains
  `workgraph: {available, can_view, can_manage}` (memory-section pattern).

## SDK parity (Stage B)

- Python: `MobHandle` group banner `# --- WorkGraph ---` in runtime.py; methods
  mirror RPC names (`workgraph_snapshot`, `workgraph_goal_create`, ...);
  frozen dataclasses in types.py (`WorkGraphItem`, `WorkGraphEdge`,
  `WorkGraphAttentionBinding`, `WorkGraphSnapshotResult`, `WorkGraphGoalResult`,
  `WorkGraphEventEntry`, ...) with tolerant `.get()` from_dict; builder:
  `workgraph(enabled=True/False)` → runtime_options. Exports in __init__.py.
  NOT in _client.py (operational subsystems are MobHandle-only).
- TypeScript: runtime.ts methods + models.ts interfaces + builder option;
  same naming (camelCase methods, snake_case wire).
- Tests: both SDKs' fake-gateway harnesses; e2e_sdk_wire.rs wire coverage.

## Console (Stage C)

Primary — inline WorkGraphCard (FlowRunCard is the component/type template but
has NO producer in meta; the reducer is new work; ChatPane is the real path):

1. console-core `conversation.ts`: `ConversationWorkGraphEntry`
   (kind:"workgraph"; rootId; title; status aggregate; item tree rows with
   status/priority/owner/revision; attention rows with mode/status/binding_id/
   revision; lastEventAt) added to `ConversationTimelineEntry` union +
   `conversationEntryText` case + index exports.
2. console-components `conversation/work-graph-card.tsx`: modeled on
   flow-run-card.tsx — header (goal title, status badge, progress
   completed/total), collapsible item tree (Parent edges = hierarchy),
   per-row status dot/priority/owner chips, attention section (mode/status),
   expandable detail, action buttons gated by presence of callbacks
   (claim/close/reassign/confirm), `data-work-graph-card` + `data-root-id` +
   `data-status` attrs, `cc-work-graph*` CSS mirroring `cc-flow-run*`
   (+ light theme overrides). Dispatch branch in conversation-message-view.tsx
   + prop threading through message-group/transcript/pane (contract parity).
3. adapters.ts (the load-bearing path): `WORKGRAPH_TOOL_NAMES` set; intercept
   in `mapFramesToTimelineEntries` BEFORE generic tool handling at :3405
   (also exclude from `buildToolBlocks`/dedupe maps); aggregate ALL
   workgraph tool frames (args + result JSON: WorkItem/Snapshot/Goal*/
   Attention* shapes) into one evolving entry per root (goal binding root or
   snapshot realm), positioned at first contributing frame; full-rebuild per
   pass (existing perf posture). Track `revision` per item for CAS actions.
4. ChatPane.tsx: `MsgKind` += "workgraph"; `flattenEntry` branch; render
   branch → `<WorkGraphCard>`; new optional callback props
   (onWorkGraphAction) threaded from ConsoleApp (onGatingDecision template)
   via new headless commands → `meta/workgraph/*`; affordance-gated
   (`experience.workgraph.can_manage`, consoleReadOnly).
5. Secondary panel: NavKind "workgraph" server-gated on
   `experience.workgraph.available/can_view` (memory-panel pattern);
   snapshot tree + attention list + events tail; refresh + operator actions.
6. Headless: CONSOLE_COMMAND_NAMES/SPECS + CONSOLE_RPC_METHODS entries.
7. Tests: work-graph-card.test.tsx (conversation-pane.test.tsx template),
   adapters.test.ts workgraph-sequence cases (line 1292 template),
   ChatPane.test.ts flatten/render, panel test; browser-e2e scenario;
   `npm run phase0:types && phase1:targets && build` (embedded bundle
   freshness is a CI gate).

## Verification (Stage D/E — goal gate)

- Rust: unit + integration tests (workgraph_wiring, RPC methods incl. ABAC,
  overlay e2e: goal_create on a member session → member turn carries overlay —
  assert via TestClient-captured prompt/overlay; schedule-delivery overlay;
  MultipleActiveBindings diagnosability), K-asks + schedule suites under
  hangmap profile.
- SDK tests both languages; e2e_sdk_wire.
- Console: vitest suites + browser e2e with seeded workgraph frames.
- Battery of adversarial reviews (multi-lens workflow: correctness, security/
  ABAC, API-contract parity, console UX, perf) — ALL must be green.
- `make ci` green → release 0.7.30 lockstep (crates + PyPI + npm, 12 assets).

## Operating model (doctrine, Luka 2026-07-08)

WorkGraphs are AGENT-operated; the console is a debug/inspection surface.
Graph surgery (reassign/claim/close/pause) is normally performed by agents
— transfers specifically by COORDINATE-mode bindings at a human's
conversational request (the downstream app: triage/gate agents are the natural
coordinate holders for cross-domain goals). Console mutations exist as
debug overrides, deniable wholesale via the `workgraph.manage` ABAC
action. The one human-NATIVE act is goal confirmation: PrincipalConfirmed
completion policies mandate an authenticated human sign-off by design.
Ask 23 (upstream) is correspondingly a break-glass recovery path for
bindings stuck on wedged/retired agents, not an operating feature.

## Non-goals (this pass)

- No meta-side workgraph identity-target rewriting dispatcher (upstream tools
  already speak session/owner targets; revisit if field pain appears).
- No WorkGraph REST routes beyond console RPC (SDKs ride the RPC gateway).
- No workgraph-driven scheduling semantics beyond upstream's overlay injection.
