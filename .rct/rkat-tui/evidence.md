# RCT Evidence Log — rkat-tui

Evidence is appended after each phase gate: exact commands, base commit,
timestamp, and output excerpts.

## Phase 0 (P0a skeleton) — partial

- Base commit: `c89d1ccc6e` (uncommitted working tree)
- Date: 2026-11-05
- `make check-rust-release-config`: `Rust release config is valid (15 publish levels)`, crate-level tests `OK`.
- `node scripts/generate-bazel-rust-builds.mjs --check`: clean after generation.
- Not yet run (blocked): `./scripts/repo-cargo check/clippy/run -p meerkat-tui`
  (host disk full), `make buildbuddy-lock-update` (GitHub downloads returned
  504), so `make verify-bazel-locks` still reports the stale MODULE.bazel.lock.

## Phase 0 (P0b verbatim ports) — 2026-11-05

- Base commit: `c89d1ccc6e` (working tree before the P0 commit)
- Lane: `RUST_LANE_ID=tui-p0b-logic` (ports built on `tui-p0b-ui` and `tui-p0b-logic`)
- `./scripts/repo-cargo fmt -p meerkat-tui -- --check`: exit 0
- `./scripts/repo-cargo clippy -p meerkat-tui --all-targets --all-features -- -D warnings`: exit 0
- `./scripts/repo-cargo test -p meerkat-tui`: lib 349 passed / 3 ignored (live-API);
  approval_policy 15, file_tools 22, memory_tools 2, render_snapshot 11; exit 0
- `./scripts/repo-cargo run -p meerkat-tui --bin rkat-tui -- --help </dev/null`: exit 0
- `make check-rust-release-config`: OK (6 crate-level tests)
- Left for P2: `ApprovalEngine` wiring, `LlmClassifier` on `meerkat-llm-core`.
- `make verify-bazel-locks`: all five gates pass after `make buildbuddy-lock-update` and BUILD regeneration.

## Phase 1 (session on the runtime path) — 2026-11-05

- Base commit: `8a99d61a38` (P0), uncommitted working tree
- Lane: `RUST_LANE_ID=tui-p1`
- `./scripts/repo-cargo fmt -p meerkat-tui`: clean
- `./scripts/repo-cargo clippy -p meerkat-tui --all-targets --all-features -- -D warnings`: exit 0
- `./scripts/repo-cargo nextest run -p meerkat-tui`: 429 run, 429 passed, 3 skipped (live-API);
  lib 368, approval_policy 15, file_tools 22, memory_tools 2, render_snapshot 11, integrator 8
- `nextest run -p meerkat-tui --test integrator` repeated 5x: 8/8 each time (~1.3 s)
- integrator (real `PersistentSessionService` + `MeerkatMachine`, scripted client):
  turn completes (P1-T001); `Queue` prompt behind a running turn and a `Steer`
  applied at a `read_file` tool boundary (`boundary_append_applied`, one run,
  steer after the tool result) (P1-T002); interrupt returns the steer as
  undelivered and it never reaches the model; `/new` + `/resume <id>`
  in-process, `last` resolution, `-c` and `--resume <prefix>` across
  lifetimes, workspace copied to a new path still resumes (P1-T003)
- `./scripts/repo-cargo run -p meerkat-tui --bin rkat-tui -- --help </dev/null`: exit 0
- `./scripts/repo-cargo run -p meerkat-tui --bin rkat-tui -- --realm-backend memory -w <dir> </dev/null`:
  exit 2 in 0.3 s, "stdin and stdout must be a terminal…" (no bootstrap)
- `node scripts/generate-bazel-rust-builds.mjs`: `crates/meerkat-tui/BUILD.bazel` regenerated;
  the root `BUILD.bazel` also needs `//crates/meerkat-tui:integrator_test` in its
  test suite (left to the pre-commit hook; outside this phase's edit scope)

## Phase 2 (approval gate) — 2026-11-05

- Base commit: `99364f1e82` (P1), uncommitted working tree
- Lane: `RUST_LANE_ID=tui-port`
- `./scripts/repo-cargo fmt -p meerkat-tui -- --check`: exit 0
- `./scripts/repo-cargo clippy -p meerkat-tui --all-targets --all-features -- -D warnings`: exit 0
- `./scripts/repo-cargo nextest run -p meerkat-tui`: 477 run, 477 passed, 6 skipped (live-API);
  lib+bin 398, approval_policy 15, approval_service 6, classifier_llm 15, file_tools 22,
  integrator 8, memory_tools 2, render_snapshot 11 (e2e_classifier: 3 ignored, live keys)
- `nextest run -p meerkat-tui --test approval_service --test integrator` repeated 5x: 14/14 each time (~1.4 s)
- `./scripts/repo-cargo run -p meerkat-tui --bin rkat-tui -- --help </dev/null`: exit 0
- `node scripts/generate-bazel-rust-builds.mjs`: `crates/meerkat-tui/BUILD.bazel` and the root
  `BUILD.bazel` test suite gained `approval_service_test` and `classifier_llm_test`; `--check` clean
- Gate: `approval::admission::ApprovalGate` implements `meerkat_core::ToolDispatchAdmission`;
  `SessionHandle::{create,resume}` install it via `SessionBuildOptions.tool_dispatch_admission`
  (composed after any preceding admission). Deny = `ToolError::PolicyDenied { ToolConsequenceDenial
  { code: "auto_mode", message: deny_message } }`: the model reads
  `{"error":"policy_denied","message":"Tool call denied (auto_mode): Blocked by auto-mode safety policy: …"}`,
  the UI sees `ToolExecutionCompleted { is_error: true }` plus `Msg::Approval` with the reason, and the run continues.
- approval_service (real `PersistentSessionService` + `MeerkatMachine`, scripted LLM, shell builtin on):
  tier-1 `read_file` runs unreviewed; a classifier-blocked `shell touch <outside>` never runs and the
  next model request carries the reason (P2-T001); a scripted `ActionClassifier` allow runs a
  `write_file` outside the workspace (P2-T002); a classifier error escalates, answered through
  `Effect::RespondEscalation`, ledger record `approved`; an unanswered escalation times out as a
  denial and closes the dialog; `--resume` re-seeds the classifier transcript from the authoritative session.
- classifier_llm (shared scripted `LlmClient`): stage-1 parse/normalize, stage-2 verdicts,
  malformed verdict ⇒ block, bounded tool loop (6 calls / 8 round trips, final tool-less call),
  tools rejected ⇒ tool-less stage 2, 16 KiB result cap, timeouts, typed `ProviderTag` knobs, usage events.
- Gap recorded: no `SessionAgentBuilder` wrapper — `build_runtime_backed_service_*` take a concrete
  `FactoryAgentBuilder`, and `CustodyEvidenceSource`, `arm_default_runtime_delivery` and the
  reconfigure-service impl are crate-private/specialised, so a wrapped builder would lose process
  custody, durable job delivery and hot swap. Upstream ask: a generic composition or a
  `FactoryAgentBuilder::default_tool_dispatch_admission` slot.

## Phase 3 (confinement + custody) — 2026-11-05

- Base commit: `cc4d0f57db` (P2), uncommitted working tree
- Lane: `RUST_LANE_ID=tui-port`
- `bootstrap::confinement`: `--confinement` lowered into an `ExecutionConfinement` built by the TUI
  (`workspace`: rw workspace + temp dir + Cargo home + user cache dir, ro rustup home, `/usr/{local,share,include}`,
  `/opt`, `/etc/{ssl,pki,ca-certificates}` and the files `/etc/{resolv.conf,hosts,nsswitch.conf,passwd,group,gitconfig}`,
  network `unrestricted`; `strict`: same filesystem, network `denied`; `off`: `ShellConfinement::TrustedHost`), checked once
  with `meerkat_sandbox::CompiledConfinement::compile`, installed with `AgentFactory::with_shell_confinement`. A refusal is a
  start-up `TuiError::Confinement` (exit 2) carrying the upstream `ConfinementRefusal` text and naming `--no-confinement`.
  A realm `[confinement]` is ignored with a start-up notice; the mode is announced as an Info notice
  (`confinement: workspace (network allowed)`).
- Why not upstream's `workspace` preset (`HostConfinement::resolve`): its extra grants are subtrees only, and the Linux
  backend refuses grants overlapping each other or the baseline (`/etc` contains `/etc/ld.so.cache`, `/usr` contains
  `/usr/bin`), missing paths and paths through symlinks, so the preset cannot grant name resolution files. The TUI's
  `grants()` keeps each extra path only when existing, canonical and disjoint. Verified on this host: with the detected
  paths the compile now fails only at the namespace probe (`BackendUnavailable`), no longer `UnsupportedRequirement`.
- Only the shell is confined (hooks and MCP stdio stay trusted host): confining every kind triggers
  `HostConfinement::admit_host_inputs`, which refuses the TUI's default in-workspace realm state on Linux (no deny lists).
  macOS adds `deny_write <ws>/.rkat` like the upstream preset.
- Custody: already wired by `build_runtime_backed_service_with_default_reconfigure_host` (it calls
  `set_interrupted_tool_evidence_source(CustodyEvidenceSource)` when the factory has a `runtime_root`; the TUI sets it to
  the realm root, and `bind_shell_process_custody` opens `<realm>/process_custody` per session). Proven by
  `tests/confinement.rs::an_interrupted_shell_call_leaves_custody_records_and_no_process` (interrupt during `exec sleep 60`:
  the process is gone, custody records exist under the session's scope).
- Tiers unchanged; `approval/policy.rs` module docs list which Tier-3 shell risks each mode contains.
- Tests: `bootstrap::confinement::tests` (mode → requirement table, grant filtering, realm override notice, relative root),
  `bootstrap::tests::each_confinement_mode_reaches_the_factory_shell` (`AgentFactory::shell_confinement()` equals the
  lowered requirement per mode); `tests/confinement.rs` workspace write containment and strict network denial
  (`bash -c 'exec 3<>/dev/tcp/127.0.0.1/<port>'` against a test listener) — **skipped on this host** with the printed
  reason: Ubuntu's `kernel.apparmor_restrict_unprivileged_userns=1` makes the Linux backend `BackendUnavailable`
  (`unshare -Ur true` fails the same way). Not `#[ignore]`d: they run wherever the backend works.
- Gap: on this host (and stock Ubuntu 24.04+) the default `workspace` mode refuses to start; users need
  `--no-confinement` or an AppArmor profile allowing user namespaces for `rkat-tui`. Also, where `/etc/resolv.conf` is a
  symlink (systemd-resolved), it cannot be granted, so DNS fails inside the Linux sandbox (cargo fetches by name fail)
  even in `workspace` mode; upstream would need a resolver grant.

## Phase 4 (models, endpoints, cost) — 2026-11-05

- `endpoints.rs` full port (registry, priority, `parse_model_ref`, bare-id lookup, `plan_startup`, `restore_choice`,
  sidecar `<realm>/tui_endpoints/<session id>.toml` with per-model cost) on the typed config:
  built-ins via `resolve_auth_binding_or_default_for_provider` (realm binding on the TUI realm's chain, else env default;
  `RKAT_` prefixes and Gemini's `GOOGLE_API_KEY` fallback honoured); keys from token-store/OAuth bindings are not
  readable, so those endpoints exist but cannot be listed (picker warning).
- Dialect paths (all through an in-memory layer the factory reads via a TUI-owned `MemoryConfigStore` passed to
  `FactoryAgentBuilder::new_with_config_store`; never written to a realm document):
  built-in `anthropic`/`openai`/`gemini` → default resolution, no binding; every configured `[endpoints.<name>]`
  (OpenAI dialect incl. LiteLLM/vLLM, Anthropic- and Gemini-dialect gateways) → backend/auth/binding in the synthetic
  realm `rkat-tui-endpoints` (dialect provider, the dialect's API-key backend kind, `base_url`, `Env{api_key_env}` or
  `InlineSecret`), selected by an explicit `AuthBindingRef` on the session. The realm is on no session's chain, so
  built-in default resolution is unchanged (tested).
  `self_hosted.servers/models` not used: `SelfHostedTransport` is only `openai_compatible` (api_style
  `chat_completions|responses`), and each model must be a global pre-declared alias pinned to one server, which fits
  neither run-time model lists nor one id on two proxies; Anthropic/Gemini have no self-hosted transport.
- Models only an endpoint knows get a `[models.<id>]` (`CustomModelConfig`) in the same layer, from the listing
  (context window, max output). Nothing added to the `meerkat-models` catalog; the catalog is consulted only for
  "is there already a profile" and as the gauge's second source.
- `/model` picker fed only by live lists (`models::ModelCache`, parallel fetch via an injectable `ModelLister`, 10 s per
  request, fetched once per process in the background or at start-up for bare-id lookups, `/model refresh`).
  Switch: `Runtime::ensure_profile` → `SessionHandle::switch_model` → `MeerkatMachine::reconfigure_session_llm_identity`
  (`SessionServiceRuntimeExt`, `SessionLlmReconfigureRequest{model, provider, auth_binding: Set|Clear}`), between turns.
- Gap: an agent captures its model registry at build (`effective_model_registry`), so a model whose profile was added
  after the build is refused by the hot swap ("absent from the agent's effective model registry"). The TUI rebuilds the
  session (retire + resume) first when its profile generation is older (`Runtime::profile_generation`). Resuming a
  session switched to such a model re-adds its profile before the build, else the tool-visibility restore guard
  (`capability_base_filter_matches_current_surface`) rejects it. Upstream ask: a registry refresh on reconfigure.
- Cost: UI counts `TurnCompleted` usage under `TurnUsage::accounting().model` (fallback: status model), classifier
  usage under the classifier model; snapshots saved to the sidecar; resume loads exact totals (derived/approximate only
  for sessions with no sidecar). Prices: `[pricing]` overrides > LiteLLM `/model/info` (from the listings) > community
  file (7-day cache in the global config dir, background refresh, `--no-pricing-fetch`). Gauge: endpoint window >
  registry window > `DEFAULT_CONTEXT_WINDOW` (128k, shown `~`).
- Tests: `endpoints::tests` (ported tables: parse, lookup/tie-breaks, discover, startup plans, restore, sidecar choice +
  cost round-trips; new: lowering resolves each binding to its `base_url`/key source with built-in resolution unchanged,
  `build_llm_client_for_identity` builds each dialect's client for a configured endpoint, profiles only for unknown
  models), `bootstrap::tests::startup_model_planning`, `credentials::tests`, UI cost arithmetic
  (`session_cost_arithmetic_from_turns_and_the_classifier`, accounted-model attribution), `pricing` cache TTL tests
  (existing), `tests/models.rs`: picker → bare-id switch to a gateway-only model → next request's `model` is
  `my-alias`, session metadata carries the gateway binding, sidecar + cost survive quit/resume, switch back to
  `anthropic:claude-opus-4-8` clears the binding and the next request uses it; unknown endpoint leaves the model.
- `./scripts/repo-cargo fmt -p meerkat-tui -- --check`: clean; `clippy -p meerkat-tui --all-targets --all-features -- -D warnings`: exit 0
- `./scripts/repo-cargo nextest run -p meerkat-tui`: 496 run, 496 passed, 6 skipped (live-API);
  lib+bin 412, approval_policy 15, approval_service 6, classifier_llm 15, confinement 3 (2 skip internally, see P3),
  file_tools 22, integrator 8, memory_tools 2, models 2, render_snapshot 11
- `--test models --test confinement --test integrator --test approval_service` repeated 5x: 19/19 each time (~2 s)
- `rkat-tui --help </dev/null`: exit 0; `rkat-tui --realm-backend memory --no-confinement -w <dir> </dev/null`: exit 2 in
  <0.01 s ("stdin and stdout must be a terminal"); in a pty with a fake key: default mode exits 2 with the confinement
  refusal; `--no-confinement` runs, `/help`, `/model`, `/cost`, `/quit` → exit 0, no panic.
- `node scripts/generate-bazel-rust-builds.mjs`: `confinement_test` and `models_test` added to
  `crates/meerkat-tui/BUILD.bazel` and the root `BUILD.bazel` test suite; `--check` clean.
- `Cargo.toml`: `meerkat-sandbox` left the machete `ignored` list (used by `bootstrap::confinement::check`).

## P5 — Mobs (helpers as mob members)

- Hosting (`src/mobs/`, feature `mob`, default on): `MobMcpState::new_with_runtime_adapter(service, Some(machine), Owner)`
  over the TUI's own `PersistentSessionService` + `MeerkatMachine`, `.with_persistent_storage_root(Some(<realm>))`
  (durable mob state under the realm, as `rkat` does; cheap: one SQLite profile store), `.with_workgraph_service`
  (realm-scoped `WorkGraphService` from the persistence bundle; without it a member build that resolves WorkGraph on
  fails "WorkGraph tools enabled without a host-issued namespace grant"), `.with_default_llm_client(llm_override)`,
  `.with_child_spawn_member_customizer(HelperCustomizer)`. `AgentMobToolSurfaceFactory` goes into the builder's
  `default_mob_tools` slot (taken before the builder is consumed), so members get mob tools too (nesting per upstream
  defaults). `AgentFactory::mob(cfg!(feature = "mob"))`; every session build (create and resume) sets
  `apply_generated_create_only_mob_operator_access(Enable)` (resumed metadata alone never re-enables it).
  Member usage is persisted in the TUI's cost sidecar, so finished helpers stay accounted on resume regardless of mob
  storage.
- Gate inheritance (no fake): `SpawnMemberCustomizer::customize_spawn` (asked for every member of every mob the state
  creates or restores, fresh spawn / restore / explicit resume) sets `spec.tool_dispatch_admission =
  ApprovalGate::for_helper(engine, owner, label).after(previous)`. Owner = the session whose implicit mob
  (`implicit-<session id>`) the helper runs in; a helper of a helper inherits its owner via the registry (helper
  session → tag, bound by the follower); other mobs (`mob_create`) → most recently opened TUI session; no session →
  spawn refused (fail closed). `ApprovalEngine::decide_as(Caller::helper(owner, label), …)`: the owner's transcript
  (user words only; the helper's task appears solely as the parent's `delegate` action, never as `[user]`), the
  owner's allow cache and Stop, the shared denial counters; `EscalationPrompt.agent` → modal line `Helper: <label>`;
  `ApprovalEvent::Decision.agent` → notices `[label] tool`.
- Policy: `delegate`/`fork_off` are `ActionKind::Delegation` → Tier 1 (delegating does nothing by itself; every helper
  call passes the gate). Read-only mob queries (`mob_list*`, `mob_check_member`, `mob_profile_get/list/list_sources`)
  joined `READ_ONLY_BUILTINS`; mutating `mob_*` stay Tier 3.
- Model/binding: a `delegate` helper runs on its delegator's model (upstream implicit-mob profile); the customizer adds
  the owner's `auth_binding` when the helper's model equals the owner's (an explicit `model_override`/inline profile
  model keeps default resolution). Sessions register model+binding with the registry on open and on `/model` switch.
- Helpers get the TUI's file tools (`spec.external_tools`, fresh `FileToolDispatcher`, no diff channel) and
  `HELPER_PERSONA` (`spec.additional_instructions`). `InheritParent` tooling needs provenance witnesses for the parent's
  external tools: file tools and notes now carry `ToolProvenance(Callback, "rkat-tui.files" / "rkat-tui.notes")`.
- Observation: the customizer announces each spawn; `follow::watch` subscribes via
  `handle.subscribe_agent_events_from(identity, SessionEventCursor::Earliest)` (retrying while the mob/member is not
  registered), binds the helper session (`resolve_bridge_session_id`, envelope source), forwards events scoped as
  `[Primary{owner}, MobMember{label}…]`, and ends when the member leaves the roster (a retired member's session stream
  does not end by itself; roster checked every 200 ms). Runner forwards `Msg::Scoped` and
  `StatusUpdate::Helpers(Vec<HelperStatus>)`; status bar shows `N helper(s)`; transcript blocks `[label · model] tool`
  with depth = number of member frames.
- Cost: no `usage_total` exists for delegate helpers (that projection is flow runs only, `meerkat-contracts` wire
  `MobRun`); per-helper usage comes from the helpers' own `TurnCompleted` (`TurnUsage` accounting model) →
  `SessionCost::record_helper` (adds to `per_model` — footer total — and `helpers[label][model]`); `/cost` prints
  "of which helpers:" lines; sidecar `[helper_cost.<label>."<model>"]` (serde-default, old sidecars load).
- Interrupt: `SessionHandle::interrupt` → `Helpers::cancel_for(owner)`: `force_cancel_member` then `retire` for each
  live helper of the session (the abandoned `delegate` call would otherwise leave the member seated), then the existing
  run stop; `engine.stop(owner)` also denies helpers' reviewed calls.
- Gaps / upstream asks:
  1. `MobDefinition::implicit(session, model)` carries no provider/self-hosted id and `validate` accepts only catalogued
     or `[models.<id>]`-in-definition models, so a session on an endpoint-only model id (e.g. `proxy:my-alias`) cannot
     `delegate` ("unknown_model: profiles.delegate model … neither catalogued …", returned to the model as the tool
     error). Ask: let the implicit mob inherit the delegator's provider / custom model profile.
  2. `fork_off` requires the caller to be a durable mob member; the TUI's primary session is not one and delegate
     helpers are `durable: false`, so only `delegate` (and the `mob_*` tools) are usable from the TUI.
  3. Hook inheritance for members: not used (the TUI gates via admission, hooks only observe); not verified.
  4. A retired member's session event stream stays open; the follower polls the roster. Ask: end member subscriptions
     on retire.
- Tests: `tests/mobs.rs` (real service, scripted client): `a_delegated_helper_runs_behind_the_sessions_gate` (delegate
  → helper one turn on the session's model with helper persona and file tools; helper `read_file` allowed; helper
  `shell` Tier 3 denied by the classifier with the reason in the helper's tool result; classifier saw `USER-ASK`, no
  `[user]` line with the task; denial counted on the parent's counters; notice `[scout] shell`; parent received
  `HELPER-DONE`; `Helpers` 1 → 0; scoped events depth 1; App cost `helpers.scout` = 20 in / 10 out, rolled into
  `per_model`, breakdown line; sidecar stores it), `an_interrupt_leaves_no_running_helper` (helper hangs, Esc → turn
  interrupted, `Helpers([])`, `helpers.live()` empty, implicit mob roster empty),
  `a_configured_endpoints_helper_uses_the_same_binding` (`--model proxy:claude-sonnet-4-6`: helper request model and
  helper session metadata model + `rkat-tui-endpoints/proxy` binding). Unit: registry owners/nesting/live, scoped path,
  transcript nested labels with model, cost roll-up/breakdown, policy delegation tier, personas, render snapshot shows
  `Helper: researcher`.
- `./scripts/repo-cargo fmt -p meerkat-tui -- --check`: clean; `clippy -p meerkat-tui --all-targets --all-features -- -D
  warnings`: exit 0 (lint enforcement probed with a deliberate `unwrap`).
- `./scripts/repo-cargo nextest run -p meerkat-tui`: 504 run, 504 passed, 6 skipped; lib+bin 417, approval_policy 15,
  approval_service 6, classifier_llm 15, confinement 3, file_tools 22, integrator 8, memory_tools 2, mobs 3, models 2,
  render_snapshot 11. `--test mobs --test approval_service --test integrator --test models` ×5: 19/19 each.
- `rkat-tui --help`: exit 0; pty smoke `--no-confinement` (fake key, DSR answered) `/help`, `/cost`, `/quit` → exit 0, no
  panic.
- `node scripts/generate-bazel-rust-builds.mjs`: `mobs_test` added to `crates/meerkat-tui/BUILD.bazel` and the root
  `BUILD.bazel`; `--check` clean. `Cargo.toml`: `meerkat-mob`, `meerkat-mob-mcp` left the machete `ignored` list.

## P6 — Context, memory, compaction policy, docs (2026-11-05)

- Base commit: `568b33af83` (P5), uncommitted working tree; lane `RUST_LANE_ID=tui-port`.
- Instruction files (`src/bootstrap/instructions.rs`): `InstructionSources { workspace, global, home }` discovers
  user-global `AGENTS.md` → ancestor `AGENTS.md`/`.rkat/AGENTS.md` from the git root (nearest `.git` dir or file; the
  walk stops below `$HOME`, so a dotfiles repo in home never adopts `~/AGENTS.md`) down to the workspace's parent →
  workspace `AGENTS.local.md`/`.rkat/AGENTS.local.md`; per file `normalize_agents_md_content` (upstream's
  `AGENTS_MD_MAX_BYTES` cap, truncation marker), total 96 KiB, empty/unreadable skipped; rendered
  `# Instructions (from <path>)` into `SessionBuildOptions.additional_instructions` after `CODING_PERSONA`, notes index
  last. Re-discovered by `Template::options` on every create/resume. The workspace `AGENTS.md` stays native (factory
  context root). Upstream's `find_project_agents_md_in` is no longer public (prompt assembly's
  `load_project_agents_md_in` is private), so the TUI mirrors its two candidates. `apply_system_md` sets
  `config.agent.system_prompt_file` on the factory's effective config when the realm sets neither prompt field.
- `/remember` / `#text` (`src/runner/context.rs`): `append_remember_line` on `remember_target(workspace)` (header on
  create), then `SessionHandle::append_system_context` → `SessionServiceControlExt::append_system_context`
  (`AppendSystemContextRequest::from_text("# Remembered instruction\n\n<line>")`, source `rkat-tui`, idempotency key
  `rkat-tui:remember:<hash of line>` per session); Info notice "… fully applied (as part of AGENTS.md) from the next
  session", Warn notice when the live append is refused (file still written).
- Native memory: `bootstrap::memory_probe` opens `HnswMemoryStore` at `<store_path>/memory` once (skipped for the
  memory backend); success → `AgentFactory::memory(true)` (`memory_search` composed by the factory, compaction indexes
  discarded history); failure → memory off + start-up notice (`Runtime::notices`). `HnswMemoryStore` is SQLite-backed
  now (no single-writer lock), so a second process no longer fails the open; the probe still catches a damaged or
  unwritable index, which would otherwise fail every build closed (`CapabilityUnavailable`).
- Read-only tiers: `READ_ONLY_BUILTINS` gains `blob_inspect`, `peers`; drops the stale v0.4 names `agent_status`,
  `agent_list` (no upstream tool has them; keeping them would make a same-named MCP tool Tier 1). New
  `REVIEWED_BUILTINS` names the acting builtins (shell family, `apply_patch`, `blob_save_file`, `blob_load_file`,
  `skill_invoke_function`, `web_search`, `brain_swap`, `generate_image`, comms sends, `council`,
  `conclude_objective`). `approval::policy::tests::upstream_builtins_are_all_classified` pins every name of upstream's
  `SHELL/COMMS/UTILITY/SKILL/WEB_SEARCH/BRAIN_SWAP_TOOL_NAMES` and `image_generation::TOOL_NAMES` into exactly one list
  and their tiers; `tests/semantic_recall.rs::every_offered_builtin_is_classified_and_memory_search_is_tier_one` checks
  every tool a real session offers the model (found `council` and `conclude_objective`, now classified).
- MCP (`src/bootstrap/mcp.rs`): `McpConfig::effective_servers_from_roots(config.tools.mcp_servers, workspace,
  user_config_root)` loaded once at start-up → `Runtime::mcp_servers` → every build's `SessionBuildOptions::mcp_servers`
  (factory session-owned router; `[MCP_PENDING]` native); `--wait-for-mcp` → `FactoryAgentBuilder::wait_for_mcp`.
  Project-scoped servers named in a start-up notice. No `HostConfinement` trust predicate (the TUI confines only the
  shell, P3), so a project `mcp.toml` is trusted like `rkat run` without confinement; the approval policy's protected
  `.rkat/` paths keep the model from editing it. MCP tools: `ActionKind::Other` → classifier unless a rule decides
  (`tests/approval_policy.rs::mcp_tools_reach_the_classifier_unless_a_rule_decides`).
- Compaction (`src/ui/compaction.rs`, `src/runner/context.rs`): mechanism = per-build
  `SessionBuildOptions::auto_compact_threshold_override`. `warn` → `NonZeroU64::MAX` (size trigger off; upstream still
  compacts on byte-cap/context-refusal recovery), gauge warns at `compact_warn_percent` once and every turn from
  `compact_auto_percent`; `auto` → `compact_auto_percent × window` (window: endpoint list > registry > 128k default, the
  gauge's number). The handle remembers its threshold; `ensure_threshold` rebuilds the idle session (retire + resume)
  after a model switch, a resume onto another model or a model-list fetch that moves it. `/compact` reports the policy;
  `/compact now` (idle, after ≥1 turn) retires and resumes the session with threshold `NonZeroU64::MIN` for one turn
  (the reconfigure host has no threshold field, so no live change), `Msg::CompactArmed`; `finish_turn` rebuilds it on the
  policy threshold after that turn. Not "half the context": a rebuilt agent restarts with `last_input_tokens = 0`
  (observed `CompactionStarted { input_tokens: 0 }`), so only a history/forecast threshold below the transcript fires.
  Gauge: `CompactionCompleted.estimated_history_tokens_after` (fallback `summary_tokens`), warning re-armed.
- Gaps / upstream asks:
  1. **On-demand compaction API** (ask): no `SessionService`/runtime call compacts now; `/compact now` costs two
     rebuilds and, with a one-token threshold, a long tool loop in the armed turn compacts again after
     `min_turns_between_compactions` (3) boundaries. Ask: `compact_session(id)` or a one-shot "compact before the next
     model call" flag on the runtime.
  2. `auto_compact_threshold_override` is not live-reconfigurable and is dropped by `session_recovery` (recovered builds
     set it `None`), so a session the runtime rebuilds on its own falls back to upstream's model-aware default.
  3. With a `DurableStaged` memory store (Hnsw), `CompactionCompleted` is published only when the runtime finalizes the
     memory projection at the session's next run (`finalize_committed_compaction_projections` via
     `default_event_tx`), and after a retire-and-resume between the two it never reached the session event stream in the
     tests. The compaction itself, the summary and the indexing work (asserted on the requests and `memory_search`).
  4. A resumed session keeps its persisted system prompt (factory: bare resume does not reassemble), so edited instruction
     files apply to new sessions; `/remember` covers the live session through system context.
  5. The admission gate sees only the tool name and arguments (`ToolDispatchContext` carries no provenance), so an MCP
     tool that shares a read-only builtin's name would be Tier 1. Ask: provenance in the dispatch context.
  6. `MODULE.bazel.lock` records the hash of `crates/meerkat-tui/Cargo.toml` (stale since P0; P6 changed the manifest
     again): `make buildbuddy-lock-update` needed before CI (outside this phase's edit scope).
- Machete: `meerkat-memory` now used (probe); `meerkat-hooks` and `meerkat-session` were never used (hooks only observe;
  compaction is native) and are removed from `[dependencies]`; the `[package.metadata.cargo-machete]` section is gone.
  Side effects: `Cargo.lock` (meerkat-tui's dependency list) and the generated `crates/meerkat-session/BUILD.bazel`
  visibility (drops `//crates/meerkat-tui`) change.
- Tests: `tests/instructions.rs` (SYSTEM.md replaces the base prompt; order SYSTEM.md < workspace AGENTS.md < persona <
  global < ancestor (via a `.git` file) < local; 32 KiB truncation marker; `/remember` twice → two file lines, one system
  message, next session's prompt carries the line from AGENTS.md), `tests/compact.rs` (auto: native compaction at the
  threshold, summary call, compacted request, `CompactionCompleted.estimated_history_tokens_after < 60k` on the next run;
  warn: no compaction past the threshold, the policy's gauge warns, `/compact now` arms → next turn compacts → disarmed,
  no second compaction), `tests/semantic_recall.rs` (offered tools classified; a fact compacted away is returned by
  `memory_search`), `tests/approval_policy.rs` MCP test; unit: instructions (order, `.rkat` local, truncation, total
  cap, `.git` file, home never root, empty skipped, SYSTEM.md rules, remember target/append), mcp loader, compaction
  policy/gauge/describe, UI `/compact` + `/compact now` flows, remember key.
- `./scripts/repo-cargo fmt -p meerkat-tui -- --check`: clean; `clippy -p meerkat-tui --all-targets --all-features -- -D
  warnings`: exit 0.
- `./scripts/repo-cargo nextest run -p meerkat-tui`: 522 run, 522 passed, 6 skipped; lib+bin 428, approval_policy 16,
  approval_service 6, classifier_llm 15, compact 2, confinement 3, file_tools 22, instructions 2, integrator 8,
  memory_tools 2, mobs 3, models 2, render_snapshot 11, semantic_recall 2. `--test compact --test instructions --test
  semantic_recall --test integrator --test mobs` ×5: 17/17 each.
- `rkat-tui --help`: exit 0. PTY smoke (`--no-confinement`, fake key, 160×60): `/help`, `#note from hash`, `/remember x`,
  `/compact`, `/compact now` ("nothing to compact yet"), `/quit` → exit 0, no panic, workspace `AGENTS.md` =
  `# AGENTS.md\n\n- note from hash\n- x\n`.
- `node scripts/generate-bazel-rust-builds.mjs`: `compact_test`, `instructions_test`, `semantic_recall_test` added to
  `crates/meerkat-tui/BUILD.bazel` and the root `BUILD.bazel`; `--check` clean. `make docs-check`: OK (covers the
  Mintlify docs only; the README is not under `docs/`). `make check-published-crates-standalone-guard`: meerkat-tui OK
  (no `standalone-tests` metadata: plain `cargo test`; README prints the ignored live-lane commands).
- README: `crates/meerkat-tui/README.md` rewritten for the port. CHANGELOG `[Unreleased]` `### Added` entry completed.
