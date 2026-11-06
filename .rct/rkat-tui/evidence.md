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
  symlink (systemd-resolved), it could not be granted, so DNS failed inside the Linux sandbox even in `workspace` mode.
  **Resolved on-branch** (2026-11-06): see the DNS entry below.

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
     error). Ask: let the implicit mob inherit the delegator's provider / custom model profile. **Resolved on-branch**
     (2026-11-06, engine change in `meerkat-mob-mcp`; narrowed for review the same day):
     `MobMcpState::with_implicit_mob_customizer` takes an `ImplicitMobCustomizer`, asked once with an `ImplicitMobContext` (owner bridge session, model) for
     `delegate_provider` before the create; the engine sets `profiles.delegate.provider` from a `Some` answer and the
     host never touches the definition (`implicit_mob::implicit_mob_definition`). Opt-in; no other surface installs
     one. The TUI's `mobs::endpoint_models::EndpointModels` answers the provider of the realm's `[models.<id>]` profile
     (read from `Runtime::effective`); the member build reads that profile from the realm config. Unit:
     `implicit_mob::tests::*`, `tests::implicit_mob_for_an_uncatalogued_model_is_refused_without_a_customizer`,
     `tests::implicit_mob_without_a_customizer_is_exactly_the_implicit_definition`,
     `tests::implicit_mob_customizer_provider_makes_an_uncatalogued_model_validate_and_is_reused`,
     `tests::implicit_mob_customizer_error_refuses_the_create`; TUI:
     `mobs::a_session_on_an_endpoint_only_model_delegates_on_it`. Found with it: the agent's mob tools capture the
     model at build (`AgentMobToolSurface.model`), so after a hot `/model` switch `delegate` helpers ran on the old
     model. The TUI now rebuilds the session after every switch
     (`RebuildReason::ModelSwitch`); `mobs::a_helper_delegated_after_a_model_switch_runs_on_the_new_model`. Ask: the
     mob tools read the session's current model.
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

## Review fixes — Agent A (approval, security, helpers)

### Why not the native operation review (A22, A23)

Checked concretely against `crates/meerkat-core/src/approval/{review,consent}.rs`,
`crates/meerkat/src/operation_reviewer.rs` and `crates/meerkat/src/governed_realm/stock/`:

- The pieces are public: `OperationReviewer` (review.rs:291), `BoundOperationReview::new`
  + `.with_human_consent(route, validity)` (review.rs:552-659), `HumanConsentRoute`
  (consent.rs:262-319), `AgentFactory::with_operation_review` (factory.rs:4718).
- But review only runs for a call carrying a `WorkAuthorizationContext`: the fenced dispatch
  in `agent.rs:3646-3716` builds a review binding only `if let Some(work) =
  context.work_authorization()`, else "Ungoverned work retains no review" (3678-3683). That
  context comes only from the native grant host (`NativeGrantWorkConfiguration`,
  `meerkat-runtime/src/meerkat_machine/local_authorization.rs:141-195`), which
  `open_realm_persistence_in` (what rkat-tui opens) never installs. In an ungoverned realm a
  bound review is accepted and never invoked.
- The tier (R1/R2/R3, authorization.rs:997-1004) is authorization truth from an
  `OperationPolicyOwner`; there is no host-local "tool call → tier" policy. Stock governance
  leaves builtins unmapped: `stock/policy.rs:52-60, 746-756` refuse shell / write_file /
  apply_patch as Unavailable (no `ToolOperationTarget` for builtins), and
  `StockReviewTarget` has no builtin selector (stock/review.rs:121-151).
- The consent route needs an `AuthenticatedApprover` (Human principal in a trust domain,
  consent.rs:126-145) and settles the call as `HumanConsentRequired` for a fresh attempt
  (review.rs:1372-1437); rkat-tui's dialog answers inside the waiting call.
- `ReviewCandidate` gives no transcript (review.rs:22-27) and `ModelOperationReviewer` is a
  single tool-less request (operation_reviewer.rs:127-133), so the classifier's reasoned
  stage with read-only project tools has no native host.

So rkat-tui keeps its gate on `ToolDispatchAdmission` (the seam that runs for every call in an
ungoverned realm) and records its escalations in the realm's canonical approval store.

Upstream asks:
- (Z-H1, D#1) Host-local review tier policy for non-governed realms: a `tool call →
  OperationReviewTier` policy installable on the factory/session build without
  `NativeGrantWorkConfiguration`, plus `ToolOperationTarget::BuiltinOperation` targets and a
  stock mapping / `StockReviewTarget` selector for builtins (shell, write_file, apply_patch).
- (D#19) A typed host reviewer so the classifier's own agent loop can go away: a core-hosted
  bounded multi-turn reviewer with host-supplied read-only tools and a sanctioned
  transcript/environment context source, and a synchronous in-call `HumanConsentRoute` mode
  for single-user hosts without a trust domain.
- (D#6) Typed authorship on host prompts: a prompt admitted through `PromptInput` should carry
  `TranscriptTurnInput::TypedText` (or `RenderClass::UserPrompt`) so a resumed transcript can
  tell the human's words from peer / runtime input without a host sidecar; rkat-tui demotes
  user-role messages without typed authorship to `[context]` lines.
- (D#5) The admission seam receives no tool provenance or mutation class; rkat-tui reads the
  session's published visible tool definitions per call. Ask: pass the resolved
  `ToolDef`/`ToolProvenance` and `tool_mutation_class` of the owning dispatcher into
  `ToolDispatchAdmission::await_dispatch_admission`.

## Review fixes — Agent B (runtime, endpoints, structure, docs)

Paths under `crates/meerkat-tui/`.

- B1 durable endpoints: `src/endpoints.rs` (`endpoint_section`, `endpoint_patch`, `project_endpoints`,
  `model_profile_entry`, `ensure_model_profile`, `EndpointError::DialectConflict`) writes the `[realm.rkat-tui-endpoints]`
  backend/auth/binding entries and `[models.<id>]` profiles into the realm's own `config.toml` through
  `FileConfigStore::patch` (only changed entries; an inline key makes the document 0600). The factory reads that document
  on every build through `src/bootstrap/realm_config.rs` (`RealmConfigStore`: document + env overrides + `SYSTEM.md`),
  replacing the in-memory synthetic realm. The endpoint-choice sidecar is gone: `endpoints::endpoint_of` derives the
  endpoint from the session's persisted binding/provider. Cost records moved to `src/runner/cost_store.rs`
  (`<realm>/tui_cost/<id>.toml`, `paths::write_atomic` 0600, corrupt file renamed `.corrupt-<unix s>`, legacy
  `tui_endpoints/` totals still read). `endpoints::load_cost` stays as a re-export for `tests/mobs.rs`.
- B2: `SessionHandle` keeps no model/provider/binding; `SessionHandle::identity` reads the authoritative metadata,
  the runner re-reads it after open/switch/rebuild. `ModelChoice.endpoint: EndpointName`,
  `Runner.current_endpoint: Option<EndpointName>`. Every build carries
  `tool_access_policy = DenyList{brain_swap, apply_patch}` (`session::TUI_OWNED_BUILTINS`).
- B3: prefix inference removed; `WireDialect::of_catalog_model` = `meerkat_models::infer_provider` (catalog-exact);
  bare ids outside the catalog resolve only via live lists (or the only endpoint). `Dialect` → `WireDialect`.
- B4: `bootstrap/confinement.rs` — a realm `[confinement]` shell requirement is the floor (lowered with
  `meerkat::HostConfinement::resolve`; `strict` narrows network), `--no-confinement` against it is
  `ConfinementError::BelowRealmFloor`. `tests/confinement.rs` probes `meerkat_sandbox::prepare` + a confined `/bin/true`
  and prints `SKIPPED:`; no output-text matching.
- B5: `ui::compaction::CompactionTrigger::{Inherit, Disable, At}`; window from the model profile only
  (`runner::models::window_of` over `Runtime.effective`), listings refresh the profile; `Msg::CompactRebuilt` /
  `COMPACT_REBUILT` ("rebuilt the session so its next model call compacts"); every retire+resume is
  `Runner::rebuild_session_workaround(RebuildReason)`; background fetches never rebuild.
- B6: runner — delivery only by `BoundaryAppendApplied` or the input's completion; runs tracked in a
  `HashSet<RunId>`; typed `ui::TurnFailure::{Failed, Interrupted, Unknown, Pending}` (interrupted only on `Cancelled`
  or metadata outcome `Cancelled`); `Completion::{Pending, TerminalAtAdmission (→ Unknown), AlreadyTerminal}`; waiters
  and interrupt tasks in a `JoinSet` aborted on switch/quit; the scoped channel is gone; transcript seeding and helper
  registration happen after a successful open; `SessionHandle::close` (→ `ApprovalEngine::forget_session`) on `/new`,
  `/resume`, quit; inputs recorded with `record_user_input`/`forget_user_input` by idempotency key after admission.
  A prompt during `/compact now` goes out as an ordinary `StartTurn` (Queue) served after the rebuild.
- B7: `ui::cost::CostConfidence::{Exact, Approximate, Unmeasured}`, `SessionCost::reconcile` against
  `Session::total_usage` on resume, `record_compaction` (estimated input + summary), `Msg::HelperUnmeasured`.
- B8: typed errors with sources: `EndpointError`, `EndpointSkip`, `AdmissionError`, `ConfinementError`,
  `PricingError` (sources), `ListModelsError::{Read, Decode, Utf8, Task}`, `CostStoreError`, `TuiError::ToolGateway`,
  `TuiError::McpConfig(EffectiveMcpServersError)`; `Provider`/`MissingApiKey`/`Mcp`/`InputRejected` string variants gone.
- B9: modules are crate-private, public under the `test-support` feature (`internal_modules!` in `src/lib.rs`), plus
  `meerkat_tui::harness`; public API `run`, `Args`, `TuiError` and its error types. Self dev-dependency with
  `test-support`, `[package.metadata.meerkat.standalone-tests] features = ["test-support"]`, README prints
  `cargo test --features test-support`.
- B10: price cache in `GlobalDirs::cache` (`--config-dir/cache`, `<user-root>/.cache/rkat-tui`, `$XDG_CACHE_HOME`,
  platform cache dir).
- B11: persona trimmed (no tool contracts / interrupt semantics); notes index last, via A's labelled untrusted
  `prompt_index`. README updated for the whole pass.
- B12: bidi controls replaced in `ui::markdown::sanitize`; model-list bodies through `http::read_capped`
  (`MAX_LIST_BYTES`); project `.rkat/mcp.toml` servers only when `ProjectMcpTrust` (A's `HostTrustRecord` digest via
  `/mcp trust`, or `--allow-project-mcp`), else upstream's trusting loader refuses them and a notice lists them.
- B13: keys are `SecretString` end to end (`ProviderCredentials`, `BuiltinCredentials`), headers marked sensitive.
- B14: resumed builds pass the session's persisted `build_state.additional_instructions` (the factory never re-renders
  a resumed prompt): persona once, edited files apply from the next new session (`tests/instructions.rs`).
- B15: `/remember` key = SHA-256 of the line (pinned in a test).
- B16: REQ-006 downgraded to PARTIAL (gap 3 below stays open).

Upstream asks (one line each):
- (D#13/D#12) A typed "no size-triggered compaction" for `auto_compact_threshold_override` (today `NonZeroU64::MAX`).
- (Z-H5, D#14) An on-demand compaction call and a live compaction-trigger update on a session, so
  `rebuild_session_workaround` can go away; `CompactionCompleted` delivered after a retire-and-resume (P6 gap 3).
- (Z-M9) A runtime/provider-owned "list models for this binding" seam so the TUI's live list client can go away.
- (D#15) A host-owned secret store write path for endpoint keys (`CredentialSourceSpec::ManagedStore` from a host) so an
  inline `api_key` need not be written into the realm document.
- (B7) `CompactionCompleted` reporting the summary call's usage (today the input is the history estimate).
- (B7, A) `HelperUpdate` has no "follow failed" variant yet: `Msg::HelperUnmeasured` exists on the UI side, but nothing
  emits it until `src/mobs/follow.rs` reports a follow failure (Agent A's file).

Verification (Agent B, lane `tui-port`, 2026-11-05, concurrent with Agent A's edits):
- `fmt -p meerkat-tui`: clean. `node scripts/generate-bazel-rust-builds.mjs`: regenerated (`test-support` feature on
  the test targets, self dev-dependency); `--check` clean. `make check-published-crates-standalone-guard`: meerkat-tui OK.
- `clippy -p meerkat-tui --all-targets --all-features -- -D warnings`: no finding in Agent B's files; the remaining
  findings are in Agent A's in-progress files (`approval/engine.rs` double_must_use, `memory/store.rs` redundant_clone,
  `tests/drift_pins.rs`, `tests/mobs.rs`).
- `nextest run -p meerkat-tui --lib --test integrator --test models --test compact --test instructions --test
  semantic_recall --test confinement --test render_snapshot`: 495 passed; failing: `approval::policy::tests::
  user_rules_take_precedence` (A), and `integrator::a_steered_message_is_applied_at_the_next_tool_boundary` +
  `semantic_recall::a_compacted_fact_is_found_with_memory_search`, both because `read_file` / `memory_search` now reach
  the classifier (A4 provenance lookup; the test fails the same with B's tool policy removed). The confinement cases
  printed `SKIPPED:` (no unprivileged user namespaces on this host); the realm-floor case ran and passed.
- `rkat-tui --help`: exit 0 (`--allow-project-mcp` listed).

Verification (Agent A, lane `tui-port`, 2026-11-05): `repo-cargo fmt -p meerkat-tui -- --check` clean;
`repo-cargo clippy -p meerkat-tui --all-targets --all-features -- -D warnings` clean;
`repo-cargo nextest run -p meerkat-tui`: 588 tests run, 588 passed, 6 skipped (no hangs);
`repo-cargo run -p meerkat-tui --bin rkat-tui -- --help </dev/null` prints usage.
The A4 regression (Tier-1 `read_file`/`memory_search` reaching the classifier) was the missing
`engine.set_tool_origins(ServiceToolOrigins::new(&service))` call in `bootstrap/mod.rs`, now wired.

## User follow-ups — confinement refusal, sandbox grants, config dir, env-only endpoint keys (2026-11-05)

- Educational refusal: `ConfinementError::Unenforceable` now carries `status: HostUsernsStatus` and `help`
  (`unenforceable_help`): what blocked it (read from `/proc/sys/user/max_user_namespaces`,
  `/proc/sys/kernel/unprivileged_userns_clone`, `/proc/sys/kernel/apparmor_restrict_unprivileged_userns`, in that
  order; else the backend's refusal; macOS Seatbelt / no backend elsewhere), how to fix it (sysctl + `/etc/sysctl.d/`
  drop-in; for AppArmor also a per-binary profile with `userns,` and `apparmor_parser -r`, with the trade-off), and how
  to run unconfined (`--no-confinement`/`--confinement=off`, or the realm config when a `[confinement]` floor applies).
  Fail closed, exit 2. Unit tests: `userns_status_is_read_from_the_proc_switches` (fake proc root),
  `the_refusal_explains_the_blocker_the_fix_and_the_way_out`.
- Linux baseline (read from `meerkat-sandbox/src/linux.rs`): dirs `/bin /sbin /usr/bin /usr/sbin /lib /lib64 /usr/lib
  /usr/lib64`, files `/etc/ld.so.cache /dev/null /dev/random /dev/urandom`; empty private mount view + chroot, no
  `/etc`, `/run`, `/proc`, `/sys`; grants bound at their own path; single regular files (nlink 1) are grantable as
  `Literal`; symlinked paths and overlaps refused; seccomp admits only `AF_INET`/`AF_INET6` sockets (no `AF_UNIX`).
- DNS: `/etc/{host.conf,gai.conf,services,protocols}` added as file grants. A symlinked `/etc/resolv.conf`
  (systemd-resolved) could not be granted, and granting its target's parent (`/run/systemd/resolve`) would not help
  (glibc reads only `/etc/resolv.conf`; nss-resolve needs `AF_UNIX`). Upstream ask: the Linux backend binding a
  symlink's target at the link's path. **Resolved on-branch** (2026-11-06, the branch's first engine change):
  `meerkat-sandbox`'s Linux `Mount::capture` accepts a read-only file grant whose final component is a symlink to a
  regular file (ancestors real directories) and binds the pinned canonical target at the link's path; `validate()`
  still re-resolves the link to the pinned target, so a retargeted link fails closed. Narrowed for review: the link
  is accepted only when the link and every directory from `/` to its parent are owned by root or the euid and none is
  group/other-writable unless sticky (`link_is_trusted`); `meerkat_sandbox::literal_file_grantable` exports the
  backend's rule and the TUI's `canonical` calls it. Unit:
  `linux::tests::a_final_component_link_grants_its_regular_file_target_read_only`,
  `linux::tests::a_final_component_link_in_a_shared_writable_directory_is_refused`,
  `linux::tests::a_final_component_link_owned_by_another_user_is_refused` (root only); positive lane:
  `linux_file_grant_through_a_final_symlink_binds_its_target_at_the_link`. The TUI now grants the
  link only (`canonical` admits a final file link on Linux; the target's directory is not granted); the
  `resolv_conf_link` notice is gone, and
  `workspace_mode_resolves_localhost_and_dns_names` no longer skips DNS for a link.
- Extra grants: `--confinement-read`/`--confinement-write <dir>` (`ExtraGrants`, validated by `grants_with`: absolute,
  existing, canonical, directory, off baseline/`/proc`/`/sys`/`/dev`, disjoint from the workspace and each other,
  not redundant; typed `ConfinementError::ExtraGrant`); refused under a realm floor. `ConfinementSection`
  (`extra_read`/`extra_write`) is defined but not yet a `tui.toml` key (needs a field in `approval/rules.rs`).
- Config dir: `~/.rkat/tui` (`<--user-config-root>/.rkat/tui`), `--config-dir` overrides; an existing legacy
  `~/.config/rkat-tui` / `$XDG_CONFIG_HOME/rkat-tui` is used while the new dir is missing, with a one-line notice
  (`legacy_config_notice`). Trust-record key unchanged (`the_trust_records_move_with_the_config_dir`).
- Endpoint keys: an inline `api_key` is `EndpointError::InlineApiKey` (exit 2, names `api_key_env`); the realm
  projection writes `CredentialSourceSpec::Env` only (a configured endpoint without a variable is refused); the 0600
  path and `Notice::EndpointPlainKey` are gone. Tests: `an_inline_api_key_is_a_typed_startup_error_naming_api_key_env`,
  `endpoints_are_projected_into_the_realm_document_once` (document holds the variable name only).
- Verification (lane `tui-port`): fmt clean; clippy `-p meerkat-tui --all-targets --all-features -D warnings` clean;
  nextest lib (`bootstrap::`, `endpoints::`, `cli::`) + `--test confinement` + `--test models`: 70 passed; confinement
  cases print `SKIPPED:` (AppArmor userns restriction on this host), DNS test included; `--help` exit 0; a real
  `rkat-tui --confinement=workspace` printed the AppArmor explanation and exited 2.

## Fork — the TUI's `fork` tool (2026-11-06)

- Goal: let the agent hand a side quest to a child that starts with its own conversation, behind the gate, while
  `fork_off` stays denied (`session::UNGATED_SPAWN_TOOLS`: its child seats through the engine's persisted-fork path
  without the TUI's spawn customizer, and it refuses a caller that is not a mob member).
- Probe, Route 1 (exact context: `PersistentSessionService::fork_durable_session` of the caller's session into a plain
  child): **dead**. A tool call runs inside the caller's turn; without a target the fork runs under
  `DurableForkSourceAdmission::Quiescent`, whose `quiescent_fork_mutation_guard` answers `SessionError::Busy` as soon as
  the source has an active runtime admission (at the committed prefix count too). The admission that forks a running
  caller, `CallerTurn`, exists only on a `DurableSessionForkTarget`, which carries a required `MobMemberBinding`: the
  child's metadata gets that binding and comms name (verified before commit) and its resume builds it as a member, so a
  plain child is not reachable through public API; `fork_durable_session_at_turn_boundary` waits for the turn to end
  (a deadlock from inside it). Not pursued further: the activated-instructions question
  (`ForkInstructionRefused`). Pinned by `tests/fork.rs::the_engine_refuses_a_durable_fork_of_a_running_session`
  (parent turn hanging in its model call; `fork_durable_session(id, None | Some(committed), None, None)` → `Busy`).
- Route 2 taken (text context): `fork::ForkToolDispatcher` (a TUI callback tool, provenance `rkat-tui.fork`, in the
  main session's gateway only; helpers get their tools from the helper host, so fork children cannot fork). It reads
  the caller's committed conversation with `load_committed_session_tolerating_tail` (the strict
  `load_authoritative_session` fails mid-turn with a transcript revision conflict against the running turn's tail), adds
  the running turn's request (not committed yet) from the approval engine's record of the user's newest words
  (`ApprovalEngine::latest_user_input`; `ToolDispatchContext::current_turn` is `None` for text turns), renders it
  (`fork::context`: user/assistant text, tool calls with truncated args, results clipped to 1 KiB, `/remember` system
  context, compaction summaries; the caller's system prompt left out; 48 KiB, newest kept; content cannot close the
  `<parent_conversation>` tag) and runs the child as a helper of the caller in its implicit mob through
  `DelegationExecutionService` (the path `delegate` takes) with member identity `fork-N`, the TUI's tool access policy
  and an override of the implicit `delegate` profile with builtins and shell on (that profile enables comms only; a
  `delegate` helper gets its parent's tools through `InheritParent`, which the host cannot resolve). Result: the
  child's bounded text (16 KiB) or an error result. (`MobMcpState::mob_spawn_helper` was tried first: from inside the
  caller's tool call the child never reached its first model call in the rig; not investigated further.)
- Everything else is the helper machinery: customizer gate for the parent as owner, `HELPER_PERSONA` + fork framing in
  the prompt (one task, report back, do not ask the user), file tools, curated shell env, follower (`[fork-1 · model]`,
  status bar, `/cost`), `Helpers::cancel_for` on interrupt.
- Policy: `ToolFamily::Fork` (provenance `rkat-tui.fork` only) and `fork` in `DELEGATION_TOOLS` → tier 1 like `delegate`;
  a `fork` from any other surface stays foreign.
- `mobs/follow.rs` follow failure: already emitted `HelperUpdate::Unmeasured` through `HelperRegistry::follow_failed`;
  unit test added (`a_helper_that_cannot_be_followed_is_reported_unmeasured`).
- Tests: `tests/fork.rs` — `a_fork_starts_with_the_parents_conversation_and_returns_its_answer`,
  `a_forks_calls_pass_the_parents_gate` (child `shell` denied by the classifier, counted on the parent, notice
  `[fork-1] shell`, `fork` itself never classified), `an_interrupt_leaves_no_running_fork`,
  `fork_off_stays_denied_and_a_fork_cannot_fork` (parent offers `fork`, not `fork_off`; child has `read_file`/`shell`,
  neither `fork` nor `fork_off`), the probe above; unit `fork_is_tiered_like_delegate_only_from_the_tuis_own_surface`,
  `fork::context` tests, the catalog family test.
- Upstream ask: a host hook to shape fork seats (the spawn customizer for persisted-fork children). It only matters once
  the main session is a durable mob member, which would make `fork_off` (exact context) usable behind the gate.

## Review fixes — 2026-11-06 (sealing, fork prompt, model switch)

- S1 hard links: `Shared::check_writable` refuses (Unix) a target with `nlink() > 1` (recoverable error naming the
  hard link); `commit_write` writes through `paths::write_atomic` with the new `FileMode::Preserve` (temp file renamed
  over the target, the old permission bits kept, a new file `0666` minus the umask), so a shared inode is never
  written. Tests: `tests/file_tools.rs` `write_and_edit_refuse_a_hard_linked_file` (`ln .git/config notes.txt`; both
  tools refuse, both names keep `[core]`), `writes_replace_the_file_and_keep_its_mode`; `paths` unit
  `atomic_write_never_writes_a_shared_inode`.
- S2 sealing outside the workspace: `ProjectRoot::with_sealed_roots` (canonicalized as far as they exist) and
  `paths::git_dirs` (the `.git` entry at the root and every ancestor: a directory, or a `gitdir:` file; plus the
  git dir's `commondir`; no subprocess). `bootstrap::scope::resolve_scope` seals the git dirs, the realm root and the
  user config dir. Tests: `paths` unit `sealed_roots_seal_state_outside_the_project`; `tests/approval_policy.rs`
  `sealed_roots_outside_the_workspace_are_sealed` (repo-subdir workspace, worktree `gitdir:` + `commondir`, realm,
  config dir; `write_file` and patch target Sealed, `read_file` not).
- A5 `.rkat` itself: `SEALED_PATTERNS`/`PROTECTED_PATTERNS` gain `.rkat` (`.rkat/**` does not match the base).
  Test: `sealed_paths_are_the_gate_state_and_vcs_internals_and_imply_protected` covers `.rkat`.
- S3 order-dependent patch check: `builtin_tier` checks every `FileWrite` target for a seal before the
  outside/protected loop. Tests: `policy` unit `a_later_sealed_patch_target_seals_the_patch`, `tests/approval_policy.rs`
  `a_sealed_patch_target_after_a_protected_or_outside_one_is_sealed` (`build.rs` before `.git/config`, `/tmp/x`
  before `.rkat/config.toml`, under `apply_patch *`).
- S4 forged roles in the fork prompt: `fork::context::entry` indents every continuation line (any line break,
  `\r`/VT/FF/NEL/U+2028/U+2029 included) with `  | `. Test: `content_lines_cannot_pose_as_entries`.
- S5 fork child tooling: `working_profile` takes `ParentTooling` (the caller session's persisted `SessionTooling`
  builtins/shell resolved against the TUI factory's defaults, both on) instead of forcing both on. Note: the realm
  `[tools] shell_enabled` is not read by the factory for TUI sessions (`build_factory` sets `.builtins(true)
  .shell(true)`), so the parent's effective tooling is its session overrides over those defaults. Test:
  `the_child_gets_the_parents_builtins_and_shell`.
- S6 proxies: README "Environment" says proxy URLs pass unchanged, credentials included.
- S7 + A6 state-dir error: `error::shell_quote` (`'\''` escaping) for the `mv`/`--state-root`/`--realm` hint, which
  is attached only when `error::suggests_move_aside` finds a schema/ledger/manifest incompatibility
  (`StoreError::{SchemaFromTheFuture, UnledgeredDomainObjects, ManifestFromTheFuture, CorruptLedger}` anywhere in
  the source chain). Tests: `a_state_dir_from_another_version_names_both_ways_out`,
  `other_open_failures_get_no_move_aside_advice` (governed realm, manifest lock timeout, permission error),
  `hint_paths_with_quotes_stay_one_shell_word`.
- C1 failed rebuild: `rebuild_session_workaround` returns `RebuildError` (`Retire`, `KeptPrevious` after a resume
  retried with the previous trigger, `Detached` when both resumes fail); `set_model` words the notice by kind
  (`model_switch_rebuild_notice`) and skips the trigger upkeep on a detached session. Tests (unit; the rig cannot
  inject a resume failure): `a_detached_session_is_reported_as_such_and_points_at_resume`,
  `a_session_still_running_only_keeps_its_helpers_on_the_old_model`.
- C2 repeated request: `render` skips the running turn's request only when the last conversational user message
  has its text and no assistant reply since ended a turn (each one asked for tools: the running turn's own
  committed tool rounds, which `the_conversation_renders_without_the_system_prompt` shows can be in the snapshot).
  Test: `a_repeated_request_after_an_answer_is_still_added`.
- C3 + A2 fork labels and errors: `fresh_label` takes the handle and lists `list_members_including_retiring`;
  `fork()` returns a `thiserror` `ForkError` rendered at the tool boundary; the handle is fetched once.
- C4 context entries: tool calls clipped to 1 KiB as a whole, assistant blocks in their own order. Test:
  `a_huge_tool_call_is_clipped_and_blocks_keep_their_order`.
- A1 delegate guards in `fork`: `admit_child_tool_policy()` before the implicit mob is ensured, and the handle must
  be the caller's implicit mob (`owner_bridge_session_lifecycle_authority`: implicit flag and bridge session ==
  caller). Test (unit): `only_the_callers_own_implicit_mob_is_acted_on`.
- A3 `fork::context` reuses `approval::transcript::truncate_text` (now `pub(crate)`).
- A4 helper policy merge: `deny_ungated_tools` merges the TUI denies into any policy (none/`Inherit` → the TUI
  list, deny list + names, allow list − names, constraints + a deny constraint, `ReadOnly` kept). Tests:
  `a_helper_without_a_policy_or_inheriting_gets_the_tui_deny_list`, `a_helpers_deny_list_gains_the_tui_names`,
  `a_helpers_allow_list_loses_the_tui_names`, `a_read_only_helper_stays_read_only_and_constraints_gain_a_deny`.
- C5 vacuous env test: `workspace_mode_passes_the_curated_environment` reruns itself with `RKAT_TUI_TEST_SECRET` set
  (unsafe `set_var` is denied) and asserts the confined shell does not see it.

## Dogfood follow-ups (2026-11-06, evening)

- `/model` picker vanished while a turn ran: an escalation prompt replaced the open picker and `advance_escalation` cleared the modal afterwards. Now a picker is parked while escalations show and restored once none remains; key repeats of Enter/Esc/Ctrl-C never act in modals (kitty reports repeats). Tests: `ui::tests::escalation_parks_the_model_picker_and_restores_it`, `parked_picker_returns_after_the_last_queued_escalation`, `stopping_all_escalations_restores_the_parked_picker`, `escalation_parks_the_resume_picker_and_restores_it`, `repeated_enter_and_esc_do_not_act_in_modals`.
- A prompt re-sent with the same text right after a failed turn was committed again (a failed run keeps its input; verified with a `RejectTools` probe): the runner now sends `(retrying the request above)` with a notice instead. Tests: `integrator::the_same_prompt_after_a_failed_turn_is_not_added_twice`, `a_different_prompt_after_a_failed_turn_is_sent_as_written`.
- `/compact now` notice reworded to say the compaction happens at the start of the next message; the read-before-edit refusal says a restart forgets earlier reads. Mouse copy: OSC 52 only (user's choice), written after the frame, clipboard + primary, honest notice.
- Known rare flake: `approval::engine::tests::parallel_escalations_share_one_timeout_including_the_queue` failed once while test binaries were compiling concurrently (heavy load), passed in 5×644 full runs and 6 lib runs afterwards. Hypothesis: with `start_paused`, the clock auto-advances past the escalation timeout while the ledger's `spawn_blocking` submit is still running on a starved thread. Not fixed; revisit if CI shows it.
