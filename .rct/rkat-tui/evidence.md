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
