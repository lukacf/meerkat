---
name: meerkat-meta
description: "Meerkat Meta (meerkat-meta) architecture, SDK patterns, console/runtime surfaces, gateways, release workflow, and Rust core internals. Use this skill whenever working on the meerkat-meta repository: adding SDK features (PyPI meerkat-meta, npm @rkat/meta-sdk), writing tests, debugging console/RPC/SSE behavior, running or changing the meta_gateway, rpc_gateway or meta_repair binaries, updating Meerkat dependencies, understanding Meta's relationship to Meerkat, releasing Meta, or authoring Meta examples."
---

# Meerkat Meta

Meta is the companion composition and product layer around the Meerkat multi-agent runtime. Meerkat owns agent and mob construction, member identity and lifecycle, spawning, wiring, comms, sessions, tools, and the core agent loop. Meta composes that authority with module supervision, routing and delivery, the identity-first runtime, REST/JSON-RPC/SSE gateways, console projections, operational policy, SDKs, and examples.

Apps own desired roster and product policy. Meerkat machines admit member and session transitions. Meta translates app intent into those authorities, supervises its own operational modules, and owns product-facing projection, streaming, inspection, and console composition.

## Current Baseline

Repository: `lukacf/meerkat-meta`. One workspace version covers every artifact:

| Artifact | Name |
| --- | --- |
| Rust crate (library `meerkat_meta`) | `meerkat-meta` |
| Rust crate for opaque extension documents (the `meerkat-meta` feature `extension-state`) | `meta-extension-state` |
| Python SDK (import `meerkat_meta`) | PyPI `meerkat-meta` |
| TypeScript SDK | npm `@rkat/meta-sdk` |
| Console/admin gateway | `meta_gateway` (release archive `meta-gateway`) |
| SDK stdin JSON-RPC gateway | `rpc_gateway` (release archive `meta-rpc-gateway`) |
| Operator repair of predecessor composition manifests | `meta_repair` (release archive `meta-repair`) |

`crates/meta-store-conformance` is a workspace-only conformance crate
(`publish = false`). `meta_flow_editor` is a development binary that CI builds;
it is not released.

The authoritative Meta release line is `[workspace.package].version` in the
root `Cargo.toml`; the 0.9 line pairs with Meerkat 0.9. `make verify-version-parity`
checks the Python and TypeScript SDK package versions, the TypeScript lockfile's
root versions, the root Bazel module, the primary `crates/meerkat-meta/BUILD.bazel`
version fields, and canonical Rust installation snippets. Separately, run
`node scripts/generate-bazel-rust-builds.mjs --check` to check generated
per-crate BUILD freshness.

For the current Meerkat dependency family, read the exact requirements in
`crates/meerkat-meta/Cargo.toml` and `crates/meta-store-conformance/Cargo.toml`, including
dev-dependencies, and the resolved versions in `Cargo.lock`. The family is
exact-pinned (`=X.Y.Z`). Verify these before release or dependency work rather
than relying on a duplicated version snapshot.

There should be no vendored `meerkat-comms` patch in this repo. Durable image forwarding belongs upstream in Meerkat; do not restore old vendoring.

Docs: Mintlify sources live in `docs/` (`docs/docs.json`). The `Publish docs`
workflow (`.github/workflows/publish-docs.yml`) asks the Meerkat docs site to
mirror them on every push to `main` that touches `docs/`; releases do not
publish documentation. The mirror is served under `https://docs.rkat.ai/mobkit/`
(the site path keeps the predecessor name), and the Meerkat repository's
`docs/mobkit/_source.json` records the mirrored Meta commit.

## 0.9 Rename And Upgrade Notes

0.9 renamed the product from MobKit to Meta as a clean break with no aliases;
historical CHANGELOG entries keep the old name. The operator checklist for an
upgrade (adoption, refused and newly honoured `mob.toml` keys, ignored keys,
empty access and gating paths, each with its exact error text) is
`docs/guides/upgrading-from-mobkit.mdx`. Current names:

- crate, Python distribution and repository `meerkat-meta`; Rust library and
  Python import `meerkat_meta`; npm `@rkat/meta-sdk`;
- `Meta`-prefixed public types (for example `MetaConfig`, `MetaBuilder`,
  `MetaStorageLayout`, `MetaTypedClient`), JSON-RPC methods and notifications
  under `meta/`, HTTP routes under `/meta/...`;
- `META_` environment variables. The gateways and the flow editor refuse to
  start while a variable the predecessor read is set;
- detached-job callbacks on Meerkat's `meta/jobs/*` methods, so Meta 0.9 needs
  Meerkat 0.9.

Existing installations carry over only through one explicit offline step.
Startup refuses typed (`StorageLayoutError::PredecessorState`,
`PredecessorGatewayHome`, `AdoptionIncomplete`, ...) instead of starting empty
beside predecessor state. Stop every process that uses the installation, then:

```bash
meta_gateway storage-adopt-predecessor --state-dir <dir>            # dry run, read-only
meta_gateway storage-adopt-predecessor --state-dir <dir> --apply    # optional --gateway-home / --predecessor-home
```

`--state-dir` is exactly the directory the refusal names (the
`persistent_state` of `rpc_gateway` and the SDK builders, or the store
directory of `meta_gateway`). Apply writes a `meta-adoption.json` journal and
resumes from it. An adopted installation keeps the meerkat-level realm id
`mobkit`; a fresh one uses `meta` (`DEFAULT_MEERKAT_LEVEL_REALM_ID`).

Other 0.9 contract changes to keep in mind:

- A mob stays publicly owned while the `UnifiedRuntime` or any handle, runtime
  or identity view exported from it lives. Dropping the last one without
  `shutdown()` hands the runtime to a retained teardown that closes admission
  once the mob is idle. `identity_runtime()`, `identity_first_context()` and
  `session_bridge()` return owned views (`PublicIdentityRuntime`,
  `PublicIdentityFirstContext`, `PublicSessionBridge`) that carry that owner.
- The memory backend config keeps only `MemoryBackendConfig::LocalJson`
  (`kind = "local_json"`); any other kind is refused, never reinterpreted.
- `MobDefinition::from_toml` warns about and ignores keys a profile does not
  define, such as `tools.mob_tasks` in configs written for earlier releases.
  The typed parse drops them before Meta records the composition on create or
  compares it on resume, so dropping (or adding) such a key never makes a
  persistent store `divergent`; a divergence refusal names only accepted
  fields. Pinned by `dropping_an_ignored_profile_key_resumes_the_store` and its
  siblings in `tests/mob_storage_persistence.rs`.
- Meerkat 0.9 refuses a profile `system_prompt`, `prompt` or `instructions`
  key, which MobKit 0.8.31 and earlier dropped silently. Every Meta mob-TOML
  ingress (`rpc_gateway` `mob_config`, `meta_gateway` `config/mob.toml`,
  `UnifiedRuntimeBuilder`'s TOML path) formats parse failures through
  `gateway_composition::describe_mob_definition_error`, which appends that
  removing the key keeps the old behaviour and shows the `[skills.<id>]`
  remedy. The other keys 0.9 newly refuses or newly honours are listed in the
  upgrade guide.
- An empty access or gating path no longer switches the feature off (the
  predecessor SDKs dropped it by truthiness). `rpc_gateway` refuses
  `runtime_options.access_config_path` or `gating_config_path` set to `""`, and
  the SDK builders' `accessControl("")` / `access_control("")` / `gating("")`
  throw. To keep the feature off, omit the key or send `null`; in the SDKs, do
  not call the builder method and keep nothing at `config/access.toml` /
  `config/gating.toml` under the working directory (conventional discovery).
- `AccessController::load_or_default` is gone: `AccessController::load` reads
  and validates an explicitly configured access file, and a missing or invalid
  file is a typed `AccessConfigError`.

## 0.9.4 Upgrade Notes

0.9.4 keeps sessions and identities only in Meerkat's own stores: Meerkat's
session store holds every transcript, and its mob store holds every identity's
binding. Meta keeps no continuity copy of a transcript and holds no identity
leases. The operator checklist, with each refusal and its remedy, is
`docs/guides/upgrading-to-0-9-4.mdx`; point to its sections instead of restating
them.

- Every 0.9.3 state directory needs one offline, one-way migration with 0.9.4's
  `meta_gateway storage-migrate-native --state-dir <dir>` (a read-only census),
  then `--apply`. 0.9.4 refuses to start on a directory that was not migrated
  (`NativeStorageMigrationRequired`, STORAGE_RESOLUTION `-32014`), and 0.9.3
  refuses a migrated one. The way back is a copy of the stopped directory taken
  before `--apply`.
- SDK-hosted continuity is removed: the builder options `continuity_store`,
  `lease_provider` and `scratch_dir` (TypeScript `continuityStore()`,
  `leaseProvider()`, `scratchDir()`), the lease-lost error, and
  `request_continuity_repair` with its `meta/request_continuity_repair` method
  (now `-32601`). A host that passed a continuity provider exports it with 0.9.3
  still installed (`docs/guides/continuity-export.mdx`, scripts in
  `tools/continuity-export/`) and imports the file with `storage-migrate-native
  --continuity-export <file>`.
- One runtime per state directory: a runtime takes an exclusive lock on
  `<state>/meta-runtime.lock` before it opens any store, and a second runtime on
  the same directory is refused (`StorageLayoutError::StateDirInUse`, `-32014` on
  the gateways).
- The first 0.9.4 boot finishes the migration from
  `<state>/storage-migrate-native.first-boot.json`. The bootstrap status lists
  pending rows as `first_boot_pending`. `reset`, `delete_identity` and the
  delivery verbs of a pending identity are refused (`first_boot_row_pending`,
  retryable); the refusal's `remedy` is `add_to_roster` when no roster lists
  the identity, or `wake_identity` when it is listed and its row waits to be
  carried out.
- `persistent_state` keeps durable mob storage at `<state>/event_log.sqlite3` by
  default, and the builder has no `session_store()`.
- A persistent composition refuses in-memory stores that its state directory
  would outlive: a declared in-memory runtime store (`rpc_gateway`
  `runtime_options.runtime_store` under `persistent_state`, `-32014`; the
  builder's `ephemeral_runtime_store(true)` with `persistent_state`,
  `ConflictingConfiguration`), and in-memory mob storage while a roster
  provider is registered. A test that wants process-local sessions omits
  `persistent_state`.
- `meta_repair` keeps only `--composition-manifests`. Its session transcript
  modes (`--db`, `--session`, `--all-sessions`, `--keep-newest`,
  `--truncate-tool-results`, `--drop-tool-results-older-than`) refuse with a
  pointer to `meta_gateway storage-migrate-native`: Meta keeps no transcript
  copy to repair, and Meerkat compaction manages the size of the transcript a
  member reads.
- `delete_identity` starts the identity over on a fresh session and keeps the
  old session archived in Meerkat, where a history read by its session id still
  serves it. The result does not name the old session: read the identity's
  status first to keep it.
- A per-agent event stream of an identity follows its session: a retire no
  longer ends it; a reset, a delete, a respawn that mints a session, a roster
  removal, or a mob reset or destroy does.
- Rust `SessionBridge` implementations change shape: `create_session` and
  `resume_session` take no runtime id (and `resume_session` no
  `SessionSnapshot`), `create_session` and `reset_member_to_successor` return
  `MemberRuntimeBinding { agent_runtime_id, session_id }`, the live-member
  reads default to `BridgeError::Unsupported`, and `BridgeError` and
  `ResumeRejectionKind` are `#[non_exhaustive]`. The CHANGELOG's Breaking list
  has each signature.

## Repo Map

| Path | Purpose |
| --- | --- |
| `crates/meerkat-meta/` | Rust crate, runtime, HTTP, JSON-RPC, gateway binaries, and embedded console bundle. |
| `crates/meerkat-meta/src/lib.rs` | Public exports and the fastest map of current modules. Read this before trusting notes. |
| `crates/meerkat-meta/src/bin/` | `meta_gateway`, `rpc_gateway`, `meta_repair`, the `meta_flow_editor` dev binary, and internal check/eval binaries. |
| `crates/meerkat-meta/src/mob_handle_runtime.rs` | `MobRuntime` / `RealMobRuntime`, mob bootstrap, member operations, session hooks, identity bridge points. |
| `crates/meerkat-meta/src/runtime/` | Runtime subsystems: bootstrap, routing/delivery, gating, scheduling, memory, metadata, event transport, cross-mob control, session stores, supervisor. |
| `crates/meerkat-meta/src/unified_runtime/` | Unified runtime builder, discovery, lifecycle, mob ops, event log, console/mob event projection, edge reconciliation, cross-mob wiring. |
| `crates/meerkat-meta/src/console_aggregator/` | Console timeline/log aggregation, replay, visibility policy, SQL/in-memory stores, send state machine. |
| `crates/meerkat-meta/src/http_console.rs` | Embedded console, REST console JSON, JSON-RPC, multipart upload/send, timeline stream, blob serving. |
| `crates/meerkat-meta/src/http_sse.rs` | Agent events, mob events, structural mob-events SSE. |
| `crates/meerkat-meta/src/rpc.rs` and `src/rpc/` | JSON-RPC dispatcher and method groups for console ingress, mob ops/events, routing/delivery, gating, memory, scheduling, session store, subscriptions. |
| `crates/meerkat-meta/src/identity_first/` | The identity-first runtime on Meerkat's member verbs: bridge, orchestrator, gateway bridges, contracts. |
| `crates/meerkat-meta/src/storage_migrate_native/` | The one-way 0.9.3 to 0.9.4 state migration behind `meta_gateway storage-migrate-native`. |
| `tools/continuity-export/` | Standalone scripts that export SDK-hosted continuity from a 0.9.3 host before the upgrade. |
| `crates/meerkat-meta/src/mob_announcements/` | Mob announcements: owner, SQLite store, batch delivery as Meerkat host continuations, member tools, delivery to members on peer gateways, RPC. |
| `crates/meerkat-meta/src/runtime/keyed_inject.rs` | The keyed inject plane that serves the signed `inject_keyed` / `inject_status` control verbs. |
| `crates/meerkat-meta/src/contact_directory.rs`, `src/auth/peer_keys.rs` | Cross-mob contacts and signed peer-key handling. |
| `crates/meta-extension-state/` | Provider-owned extension documents with atomic access control and retries (`ExtensionDocumentStore`). |
| `crates/meta-store-conformance/` | Workspace-only storage conformance harness for Meta store backends (per-trait chapters, extending Meerkat's `meerkat-store-conformance`). |
| `console/` | React console app source, tests, Vite/build entrypoints, generated `console/dist`. |
| `packages/console-core/` | Reusable console contracts, conversation/activity/sidebar/dock logic, formatting, rich content. |
| `packages/console-components/` | Reusable console UI components and CSS tokens/themes. |
| `crates/meerkat-meta/console-dist/` | Embedded console bundle copied from `console/dist` by `npm run build`, plus the MCP Apps sandbox proxy copied from its console sources (written back out by the gateways' `--write-mcp-apps-sandbox <dir>`). |
| `flow-editor/` | Flow editor app source. `npm --prefix flow-editor run build` also writes the bundle that `crates/meerkat-meta/flow-editor-dist/` embeds; `flow_editor_router` serves it at `/flow-editor` (the `meta_flow_editor` dev binary). |
| `sdk/python/` | Python SDK, gateway subprocess transport, identity-first models/providers, tests. |
| `sdk/typescript/` | TypeScript SDK, fetch/SSE client, runtime builders, tests. |
| `docs/` | Mintlify documentation sources (mirrored to the Meerkat docs site). |
| `examples/001-incident-command-center-pack/` | Self-contained Incident Commander example pack. Keep example-specific source here. |
| `examples/002-foresight-studio-pack/` | TypeScript SDK example with identity-first providers, callback tools, and a heavily customized console. |
| `installer/` | Meta Forge: the `meta-forge` Python package (`launchkit`), its web UI (`installer/web`), templates and release inputs. It releases on its own tags; see `installer/RELEASING.md`. |
| `scripts/repo-cargo` | Preferred Cargo wrapper for this repo. |

Do not put example-specific support code under `crates/meerkat-meta/src`. Same repo is fine; the core crate source tree is not.

## Console Surfaces

Current HTTP routes to remember:

- `GET /console`, `/console/assets/console-app.js`, `/console/assets/console-app.css` serve the embedded console.
- `GET /console/experience`, `/console/modules`, `/console/identities`, `/console/timeline` serve REST JSON.
- `GET /console/timeline/stream` streams projected console timeline frames.
- `GET /console/identity/{identity}/stream` is an identity-filtered timeline stream convenience route.
- `POST /console/send` submits console sends through the aggregator.
- `POST /console/rpc` is JSON-RPC.
- `POST /console/rpc/multipart` supports `meta/console/send` with image uploads and `meta/blob/upload`.
- `GET /blobs/{blob_id}` resolves console blob/image content.
- `GET /agents/{agent_id}/events`, `/mob/events`, and `/meta/mob_events/stream` are SSE observation surfaces.

Console projection is contract-boundary code. Prefer changing `console_aggregator`, `http_console`, `console/src/lib/adapters.ts`, or `packages/console-core` rather than leaking raw runtime text into React components.

Identity binding facts the console contract keeps:

- The identity routes on `/console/rpc` (`meta/status_identity`, `meta/inspect_identity`, `meta/retire`, `meta/respawn`, `meta/reset`, `meta/delete_identity`) and the stdio identity methods refuse a generated runtime id that is no longer its identity's binding, such as the predecessor's `rt:<identity>:<n>` after a reset. The refusal is `-32000` with `error.data.kind = "stale_identity_runtime_binding"`, naming the registered runtime id and session. A runtime id of an identity the identity runtime does not know keeps `-32602`.
- `/console/experience` takes an identity-first member's session from the identity runtime and never serves a fact walked under an earlier binding. A record it holds no observation for carries `"unobserved": {"reason": "refresh_pending" | "not_observed", "fields": [...]}` (`meerkat_meta::unobserved`; console-core `UnobservedFacts`), with fail-closed affordances. `health_overview.live_snapshot.running` is `true`, `false` or `null`; `null` means unobserved and is never defaulted to running.
- `meta/reset` of one identity, on the SDK gateway and the console, answers `cleanup_warning: null`, in the response and in the `identity_reset` lifecycle frame. The reset's successor transition is Meerkat's respawn, which retires the previous live member; the field stays for wire compatibility, as `meta/respawn` keeps its retired warnings.

Agent-tool spawns (`mob_spawn_member`, `delegate`, `spawn_member`/`spawn_many_members`) project into the console through `crates/meerkat-meta/src/console_spawn.rs`: the unified runtime installs a `ConsoleSpawnSink` on the mob runtime at bootstrap, and the `AutoWireParentMobToolDispatcher` (in `mob_handle_runtime.rs`) registers each spawned member's identity + labels (`spawned_by`, `via_tool`, spawn labels) and appends the `initial_message` as a deduped `spawn-kickoff:*` `user_input` console event. Live runtime events resolve to the same chat identity. No console sink → spawn behavior unchanged.

## Access Control (ABAC)

Optional attribute-based access control for console and SSE surfaces lives in `crates/meerkat-meta/src/access/` (model, engine, controller). Deny-by-default with deny-overrides when enabled; `admins` bypass rules; a disabled config (or absent controller) changes nothing.

Key facts:

- Opt-in via `config/access.toml` (meta_gateway conventional discovery), `UnifiedRuntimeBuilder::access_control_file`, `runtime.set_access_controller(...)`, or rpc_gateway `runtime_options.access_config_path` (TS SDK: `.accessControl(path)`, Python SDK: `.access_control(path)`; both auto-discover `config/access.toml`). Discovery skips `config/access.toml` only on NotFound/ENOENT (nothing there, including a missing or dangling parent); a directory, a dangling final link, ENOTDIR and other lookup errors go to the strict loader, which refuses startup when it cannot open, read, parse or validate the file (it does not check the file type, so a FIFO blocks startup instead).
- The closed action vocabulary lives in `access/model.rs::ACCESS_ACTIONS`. It covers agent view/send/lifecycle, scoped memory, gating, WorkGraph, mob observation, topology, mobpack author/deploy, runtime administration, and access administration. Do not maintain a shorter shadow list in surface code.
- Enforcement seams: experience filtering + affordance intersection in `runtime/console_ingress.rs` (`handle_console_rest_json_route_with_snapshot_and_access`), RPC gating + result filtering + `meta/access/*` admin methods in `http_console.rs` (`console_rpc_access_requirement`, `handle_access_admin_rpc`), SSE gating in `http_sse.rs` (`sse_access_context`).
- `mob.observe` gates the whole-mob event surfaces (`/mob/events`, `/meta/mob_events/stream`, `meta/mob_events/query`/`subscribe`) but does NOT override per-agent `agent.view`: events are filtered per source/`agent_identity`, mob-level (unattributed) events flow on `mob.observe` alone. "Observe all" = `mob.observe` + `agent.view` on `*`.
- Spawn-lineage inheritance: agent-spawned members carry a `spawned_by` console label (recorded by `src/console_spawn.rs`); agent checks walk the member plus its spawn ancestors (`evaluate_access_lineage`, depth 8, cycle-safe), so rules matching the parent also match its spawned members, with deny-overrides preserved across the chain. Lineage is runtime-derived ONLY: `spawned_by`/`via_tool` claims in caller-controlled labels (spawn specs, roster) are stripped at every enforcement seam (`sanitize_unverified_lineage_labels`) - only the in-memory spawn registry may assert them.
- `/blobs/{id}` is a capability surface (content-addressed `sha256:` ids, deduped across agents, no per-agent ACL) — authn + hash-unguessability, not an `agent.view` boundary.
- Live config: successful `AccessController` config mutations validate and advance the revision. They persist TOML only when a persistence path is configured through `load` (an existing, valid file; a missing or invalid one is an error) or `with_persist_path`; `AccessController::new` and `disabled` remain in-memory unless a path is attached. A configuration that names no admin grants administration to nobody. Per-request `AccessView` snapshots and cached agent label/role attributes from roster projections let label selectors work on identity-only surfaces.
- Console UI: `console/src/panels/AccessPanel.tsx`, nav kind `access` appears only when `experience.access.can_administer`. The panel re-reads the access configuration in the background without disabling edits: a background read changes the panel only when it lands. The first read after opening Access or changing connection or subject, a Refresh, and the read after a save still disable edits until they land, and a save still carries the draft's edit token, which the owner checks.
- Denials: JSON-RPC `-32030` with `data.kind = "access_denied"`, HTTP 403 on REST/SSE.
- Anti-lockout invariant: enabling requires non-empty `admins`; validation lives in `access/model.rs`.
- Tests: `crates/meerkat-meta/tests/access_control.rs`, unit tests in `src/access/*`, console helper tests in `console/src/panels/AccessPanel.test.ts`. Docs: `docs/concepts/access-control.mdx`.

## Console Configuration

The stock console can be shaped by `config/console.toml` in the conventional workspace layout. The runtime projects the parsed config through `GET /console/experience` as `console_config`, so embedders and the bundled React console share the same contract.

Key files:

- `crates/meerkat-meta/src/console_config.rs` defines the TOML-backed config schema, normalization, realm overlays, and loader helpers.
- `crates/meerkat-meta/src/config_convention.rs` discovers `config/console.toml`.
- `crates/meerkat-meta/src/bin/meta_gateway.rs` loads the conventional config and applies the TUX init `realm` overlay.
- `crates/meerkat-meta/src/bin/rpc_gateway.rs` accepts `runtime_options.console_config_path`.
- `sdk/typescript/src/builder.ts` exposes `Meta.builder().consoleConfig("config/console.toml")` for RPC-gateway embedders, plus `.consoleAuthRequired(false)` for explicit local unauthenticated console demos; the Python builder has `console_config(path)`.
- `crates/meerkat-meta/src/runtime/console_ingress.rs` projects `console_config` and uses `title` for the base panel title.
- `console/src/ConsoleApp.tsx`, `console/src/panels/Topbar.tsx`, and `console/src/panels/Sidebar.tsx` consume the config.

Current shape:

```toml
title = "Ops"

[brand]
label = "Ops Console"
logo_url = "/assets/ops.svg"
logo_alt = "Ops"

[appearance]
default_theme = "dark"
default_variant = "graphite"

[environment]
label = "prod"

[layout]
initial_preset = "two_columns"
initial_control = "roster"
initial_agent = "identity:ops-lead"
sidebar_collapsed = false

[rail]
visible = true
collapsed = false
active_preset_id = "critical"
empty_text = "No meaningful signals yet."

[[rail.filter_presets]]
id = "critical"
label = "Critical"
alert_levels = ["critical"]

[sidebar]
visible_controls = ["topology", "roster", "logs", "health"]
hidden_controls = []

[[sidebar.buttons]]
id = "ops-board"
label = "Ops Board"
href = "https://example.test/ops"
target = "_blank"
icon_name = "external-link"

[agent_list]
group_by = ["labels.console_group", "labels.group", "role"]
subgroup_by = ["labels.org", "labels.realm"]
section_order = ["Personal", "Initiatives", "Internal"]
fallback_group = "Agents"
fallback_subgroup = "Other"
default_pinned_agent_ids = ["identity:ops-lead"]
collapse_single_subgroup = true

[[agent_list.badges]]
id = "org"
label = "Org"
field = "labels.org"
tone = "info"

[[agent_list.sections]]
name = "Initiatives"
collapsed = false
empty_title = "No initiatives"
empty_text = "Create one in Linear."

[actions]
inspect_label = "Profile"
chat_label = "Open chat"
send_label = "Send to agent"
respawn_label = "Respawn"
retire_label = "Retire"
reset_label = "Reset"
show_reset = false

[realms.ops]
title = "Ops"

[realms.ops.agent_list]
subgroup_by = ["labels.org"]
```

Agent grouping selectors and badge fields support `labels.<key>`, `label:<key>`, raw label keys, and direct fields such as `group`, `subgroup`, `role`, `kind`, `identity`, `member_id`, and `agent_id`. For configured `group_by` and `subgroup_by` selectors, the detected ancestor chain is searched from the highest ancestor down to the child; the first available configured value wins. This keeps spawned/delegate rows with their host even when their own labels differ. Child values apply only when no earlier ancestor supplies a configured value; this inheritance rule does not apply to badges. Prefer mob member labels such as `console_group = "Initiatives"` and `org = "Payments"` for domain-specific grouping.

When extending console customization, keep it view-level. The config should decide presentation, ordering, visibility, labels, defaults, and links; it should not decide runtime authorization, routing, or whether an agent exists. Be cautious with arbitrary CSS/theme color injection until there is a token contract.

Current console UX decisions to preserve:

- The main sidebar is navigation. Clicking an agent opens chat; no hover-driven chat/info toggle.
- Roster/details belong in roster-style surfaces, not in a redundant Inspect mode.
- Approvals owns pending, auto, audit, and policies. Do not reintroduce a separate `Gates` nav item.
- Signals should show meaningful user requests, peer-to-peer communication, assistant replies, image events, and relevant tool activity. Group repeated events from one interaction when useful.
- Roster should list actual wired peers, not only counts.
- Suppress scaffold/noise prompts, raw `checksum_token`, raw tool metadata, and transport envelopes from user-facing chat.
- Preserve user messages and timeline order. Ordering bugs usually come from merging history, live frames, and optimistic entries with partial timestamps.
- Render `assistant_image` and `assistant_image_appended` as images and dedupe by blob/reference.

## Cross-Mob And Comms

Comms delivers work; observation chooses scope. Do not design APIs that mix send and observe into one operation.

Cross-mob support includes in-process wiring and signed TCP/UDS contacts. Real transports require real Ed25519 peer pubkeys. In-process descriptors can stay unsigned because the in-process router authorizes through the identity map. The signing and digest contexts changed with the 0.9 rename, so a 0.9 gateway cannot exchange signed control requests or host facts with a pre-0.9 gateway.

Useful files:

- `crates/meerkat-meta/src/unified_runtime/cross_mob.rs`
- `crates/meerkat-meta/src/runtime/cross_mob_control.rs`
- `crates/meerkat-meta/src/runtime/cross_mob_remote.rs`
- `crates/meerkat-meta/src/contact_directory.rs`
- `crates/meerkat-meta/src/auth/peer_keys.rs`
- `crates/meerkat-meta/tests/cross_mob_signed.rs`
- `crates/meerkat-meta/tests/cross_mob_tcp.rs`
- `crates/meerkat-meta/tests/cross_mob_uds.rs`

Image forwarding gotcha: the old failure was `image_ref_unavailable: current_turn image 0 did not resolve to a current-turn image`. Same-turn generate-and-send worked because the image was still in current-turn scope. Durable forwarding should use current upstream Meerkat behavior, not a vendored patch.

Keyed inject: the control verbs `inject_keyed` and `inject_status` deliver one keyed continuation to a member of the serving gateway's mob, once per key and scoped to the calling grant, and read it back.

- A grant must name them explicitly; `verbs = ["*"]` (`ControlVerb::member_plane()`) does not cover them.
- Requests and answers are signed. A request must be issued within `runtime_options.control_request_window_ms` (default 300000, five minutes) of the serving gateway's clock, and a delivery is committed by its deadline (its issue time plus the window) or refused as `stale_request`, with its typed cause (`outside_window`, `issued_before_start`, `deadline_passed`) in a signed field.
- Every gateway that binds a control listener seals what the plane serves once its runtime is composed; until then both verbs answer `keyed_inject_unavailable`.
- A gateway that is not identity-first, or that turns serving off (`runtime_options.serve_keyed_inject: false`, or `UnifiedRuntimeBuilder::keyed_inject_plane(KeyedInjectPlane::Disabled)`), answers `not_served` with the typed cause `not_identity_first` or `runtime_disabled` in a signed field. `meta_gateway` has no such switch: its listener serves keyed injects whenever its runtime is identity-first, with the default window (`DEFAULT_KEYED_INJECT_WINDOW_MS`), so the grants that name the verbs decide who may call them.
- Only an answer the caller's pinned key verifies is evidence. A signed refusal says that nothing was committed, and a mob announcement delivery then waits with a typed reason. An unsigned answer, such as an older gateway's `decode` error, is kept as text and never branched on: the delivery's outcome stays unknown until a status read settles it (marker `peer_unanswered`). Only a signed `member_departed` ends a subscription, and no gateway of this release answers it yet.
- `ControlRequest`, `ControlVerb` and `ControlResponse` are `#[non_exhaustive]`, so a match outside the crate needs a wildcard arm. `ControlVerb::all()` returns a slice.

See `docs/concepts/access-control.mdx` (Keyed inject) and `crates/meerkat-meta/src/runtime/keyed_inject.rs`.

## Mob Announcements

Mob announcements carry developments to subscribed members (`crates/meerkat-meta/src/mob_announcements/`; design in `docs/design/mob-announcements.md`, which the docs site does not publish).

- The host announces a development to a mob hosted on the gateway, once per mob and source key. Member subscriptions, keyed by `{mob_id, identity}`, take the topics they name from the mobs they name (the gateway's own mob by default).
- An announcement is captured for every matching subscription in its own transaction. Each subscriber receives what it is owed in bounded batches, and each batch is one Meerkat host continuation keyed by batch id (namespace `meta.mob_announcements`).
- A lost acknowledgment is reconciled from native status, never submitted twice. A delivery stranded on a retired incarnation moves to a successor batch that names its predecessor.
- A subscription ends, typed, only on a final fact: every mob it named was destroyed (`mobs_ended`); for a member of another mob hosted here, that mob ended (`member_mob_ended`, below); or a peer gateway answered, signed, that its member departed (`member_departed`). No gateway of this release records a final fact that a member left, so none answers `member_departed` yet.
- A member that is not registered, not served where its deliveries go, governed, without a live incarnation, or gone from this gateway ends nothing: its deliveries wait, so a member served again is delivered to again. Its owed batches expire after the retention window, and `subscriptions/cancel` ends a subscription whose member is not coming back. Retention removes ended subscriptions and the logs of destroyed mobs; what waited past retention for a delivery that never started expires and is reported to the member as a gap.
- Why deliveries wait: the owner keeps the latest classified wait per member as a `marker` (`{cause, evidence, since_ms}`), shown by `deliveries/observe`, `deliveries/reconcile` and `subscriptions/get`. `marker.cause.kind` names the state (for example `member_unknown`, `member_not_live`, `peer_unanswered`, `clock_skew`) and `marker.evidence` its class (`verified`, `unverified` or `local`). It is a projection: no transition reads it, and a restart rebuilds it from the next attempt. It is the latest attempt's, so it can outlive the wait it names (after a cancel, or once the batch settles) until the member's next attempt or a restart.
- A submission whose outcome is not learned in time settles `expired_unverified`, and a committed batch whose admission is not confirmed in time settles `committed_unconfirmed` with its receipt kept. Neither is submitted again: the member's next batch reports their announcements as `delivery_not_confirmed` or `admission_not_confirmed`, and what a later status read finds is kept on the batch as its `late` outcome.
- Methods: `meta/mob_announcements/{announce, get, subscriptions/{put,pause,resume,cancel,get,list}, deliveries/{list,observe,reconcile}}`. Typed errors are `-32044` to `-32048`; `docs/api/rpc.mdx` has the table.
- Enable it on a persistent, identity-first host. Every surface composes it through one `AnnouncementComposition`: `rpc_gateway` with `runtime_options.mob_announcements`, `meta_gateway` with its `mob_announcements` init param, and a Rust host with `UnifiedRuntimeBuilder::mob_announcements` (which needs `persistent_state()` and an identity-first runtime). Each refuses what it cannot compose before it builds anything; `AnnouncementConfig::validate` is the one check. Batch, retention and limit options are in `docs/reference/configuration.mdx`. Without it, nothing opens `mob_announcements.sqlite3`, and the methods answer `mob_announcements_unavailable` on every surface.
- The console serves the same methods on `/console/rpc`. Every one of them, the reads included, needs `runtime.admin`, and a read-only console refuses the ones that change state (`announce`, `subscriptions/{put,pause,resume,cancel}`, `deliveries/reconcile`).
- Member tools: a profile that lists the `mob_announcements` bundle in `tools.rust_bundles` gives its members `mob_announce` and `mob_subscription`. A member announces into its own mob as itself and manages its own subscriptions. It may name its own mob and the mobs above and below it among the mobs members created, never a sibling's (`unrelated_mob`). Each call is attributed to exactly one member or refused; limits come from `member_limits`, and a child mob member's own subscriptions also count against `child_member_limits`.
- The host can subscribe a member of any mob the gateway hosts, such as a child mob, by its plain roster id. Its subscriptions end with `member_mob_ended` when that mob is destroyed, created again, completed or reset. A mob id that names both a hosted mob and a peer is refused with `ambiguous_mob`. These subscriptions are bounded by `child_member_limits` (`child_member_limit`). `member_tools_in_child_mobs` is the ceiling: with it a child mob profile gets the member tools by naming `mob_announcements` in its own `tools.bundles`; without it a child create that names the bundle is refused.
- Members on peer gateways are reached through keyed inject (above).
- Every public enum of the Rust surface is `#[non_exhaustive]`.

## Identity-First Runtime

**Doctrine (DECIDED 2026-07-06, `docs/design/identity-first-doctrine.md`): Meta is dual-plane.** Durable members live on the identity plane (`IdentityRuntime`: identities reconciled from a roster, each embodied as one Meerkat member). Ephemeral workers live on the mob plane (`MobHandle`: `spawn`/`mob_spawn_member`/`delegate`, idle-retire reaping). The mob plane is NOT legacy - it is the worker plane and the substrate the identity bridge is built on. What is wrong is building DURABLE populations on the mob plane (member-per-user, long-lived coordinators via `ensure_member`): use an identity roster instead. Never route worker churn through identities: each identity is durable state (a binding in Meerkat's mob store) that every reconcile pays for.

Identity-first makes stable `AgentIdentity` strings the control-plane key and treats runtime member IDs as generated bindings. Prefer identity-scoped APIs for durable agents and member-scoped APIs for ephemeral workers at the mob layer.

Since 0.9.4 the identity runtime runs on Meerkat's member verbs, and Meerkat holds each identity's binding (runtime id, session, member generation):

- View states are `active`, `dormant` and `broken`; tolerate unknown values. The status and the inspection carry Meerkat's facts: `generation`, `session_id` and `agent_runtime_id`, and when set `convergence`, `wake_refusal` (with its remedy) and `replaced_session`.
- When Meerkat reports an identity's session gone, the identity wakes on a fresh session and the status names the session it replaced.
- Refusals are typed: an identity Meerkat cannot resume, history it cannot read yet, an identity a host adopted, and a wake held for an operator. The lease codes `-32003` and `-32005` are no longer answered.
- A saved `DeliveryScope` carries Meerkat's member generation and fence token. A scope saved by 0.9.3, or captured before a respawn, is refused as stale (`-32007`): capture a new one.

Gateway note: on an identity-first gateway (`meta_gateway` init
`identity_first: true`, or any runtime with the `console_identity_roster` slot
set), console member RPCs default to identity semantics. `ensure_member`
upserts the `MutableRosterProvider` and reconciles through `restore_flow`,
`retire_member` is an identity retire, and `respawn_member` is an identity
reset. Callers must select `plane: "worker"` explicitly for ephemeral
mob-plane workers.

Key files:

- `crates/meerkat-meta/src/identity_first/types.rs`
- `crates/meerkat-meta/src/identity_first/contracts.rs`
- `crates/meerkat-meta/src/identity_first/runtime.rs`
- `crates/meerkat-meta/src/identity_first/orchestrator.rs`
- `crates/meerkat-meta/src/identity_first/bridge.rs`
- `crates/meerkat-meta/src/identity_first/gateway_bridges.rs`
- `sdk/python/meerkat_meta/identity_first_models.py`
- `sdk/python/meerkat_meta/identity_first_providers.py`
- `sdk/typescript/src/runtime.ts`
- `sdk/typescript/src/models.ts`

Avoid manual runtime ID extraction for ordinary identity-scoped work. Do not assume a registered identity is an active member: read its status, because `dormant` and `broken` identities exist.

## Standing Instructions And Forks

- `meta/member_activate_instruction` appends one keyed, immutable instruction
  activation (a skill body, a persona, any namespace) to a member's current
  session; `meta/member_instruction_activations` reads them, and a query that
  names `namespace` and `key` also returns its `key_state`. Both are host
  verbs, never agent tools (`docs/api/rpc.mdx`).
- A durable fork (`fork_off`, `UnifiedRuntime::fork_member`, a live delegation
  fork, temporary council participants) copies those rows only as history.
  The child runs no turn until each inherited key is activated in the child
  itself; until then its first model request fails with
  `forked session must explicitly activate inherited instruction keys`. Mob
  operator authority is never copied. `meta/fork_helper` is not a durable
  fork: an autonomous-host helper gets the source's transcript as text,
  activation bodies included, under every policy.
- `runtime_options.fork_instruction_policy` decides for every fork of the
  gateway's mob, never per fork, and is applied again on every boot (the mob
  does not store it). Python `.fork_instruction_policy(...)`, TypeScript
  `.forkInstructionPolicy(...)`, Rust
  `UnifiedRuntimeBuilder::fork_instruction_policy` or
  `MobBootstrapSpec::with_fork_instruction_policy`:
  - `require_explicit_activation` (default): a host-driven `fork_member` is
    committed and the host activates each key on the child
    (`MobHandle::activate_member_instruction`). A fork that starts its own
    turn is refused before commit with `fork_instruction_refused` (data
    `fork_instruction_refusal` with `cause.kind` `policy_refuses` and the
    `keys`). No gateway method forks a member, so it never arrives as an RPC
    error: agents get it as a `fork_off` tool error, Rust hosts as
    `MobError::ForkInstructionRefused`.
  - `reactivate_inherited`: the fork writes the child's own activation of
    every inherited key, in every namespace, with the revision and body the
    source had in effect at the fork boundary. The child runs them without
    the host checking them again, and forks of the child carry them on.
  - `refuse`: every fork of an activated member is refused before commit.
- What a fork copies, the refusal causes, the exact rows
  `reactivate_inherited` writes and what it trusts:
  `docs/reference/configuration.mdx`, "Fork instruction policy".

## Embedding Hooks

Hooks an embedding host uses around the runtime:

- `UnifiedRuntime::take_bootstrap_completion` hands the host a one-use `RuntimeBootstrapCompleted` (`unified_runtime/bootstrap_completion.rs`). The runtime mints it once, on its bootstrap success path, never on a failure. `publish(f)` runs `f` once, only while the runtime has not begun to close and its mob still admits work; later it is refused with `BootstrapCompletionRefused::RuntimeClosing` or `AdmissionClosed`. On the stdio gateway the host gets it from `GatewayBootstrapReady::take_runtime_completion` inside its publication boundary. A runtime from a legacy constructor (`UnifiedRuntime::bootstrap` and the `bootstrap_with_*` family) has none until `activate_without_identity_context` returns `Ok`.
- `UnifiedRuntimeBuilder::operation_refusal_observer` installs Meerkat's operation refusal observer on every agent factory the builder owns (persistent, scratch and temp-dir launches). A builder given an already-built `mob_spec()` refuses it with `ConflictingConfiguration`, and the `MobBootstrapSpec` constructors take none.
- A gateway host answers `GatewayHostComposition::operation_refusal_observer`. The gateway asks it once per member factory, with the same construction it offers `compose_member_factory`, before any answer is installed: `Ok(None)` (the default) installs nothing and `Err` refuses init. An installed observer sees every agent its factory builds, including spawned helpers, forks, and the child mobs and councils agents create.
- A gateway host answers `GatewayHostComposition::operation_review` with Meerkat's `BoundOperationReview`. The gateway asks it with the same construction, before any answer is installed and before any session exists on its runtime. When the review routes R3 to human consent, the gateway first installs its consent owner-loss settlement on the runtime it has just built, then the review on the factory; a review without a consent route goes on the factory alone. `Ok(None)` (the default) installs nothing, and `Err` refuses init. A runtime that refuses the settlement fails init with `GatewayHostError::ConsentSettlementRefused`.

Delivery and ownership facts an embedding host meets:

- A scope-bound delivery that Meerkat refuses because the mob's admission is closed (`MobError::AdmissionClosed`, whatever closed it: an idle close, the last owner's release, a stand-down, a shutdown, a destroy or an actor exit), or because the member is closed for identity convergence, is `ScopedDeliveryError::Rejected` (wire kind `scoped_delivery_rejected`, `admission_possible: false`), not `Uncertain`: nothing was admitted, so there is nothing to recover.
- A `WeakPublicIdentityRuntime` (from `PublicIdentityRuntime::downgrade`) no longer upgrades after the runtime's `shutdown`, even while the identity runtime itself is still held: Meerkat 0.9.4 closes the mob's admission when it admits the Shutdown.

## SDK Gateway Transports

The Python and TypeScript SDKs run the gateway as a child process and read its stdout (`sdk/python/meerkat_meta/_transport.py`, `sdk/typescript/src/transport.ts`). A request waiting on a gateway that exits while a process it started still holds that stdout fails with `GatewayChildEndedError` about a second after the exit, instead of waiting out its timeout. It is a `TransportReaderFailedError`; the TypeScript error carries `exitCode` and `signal`, the Python one `exit_code`. A gateway whose stdout closes with its exit still fails its waiters with `TransportReaderFailedError`. The TypeScript transport then lets go of the held stdout, so it no longer keeps the host's Node.js process running.

## Incident Commander Example

The Incident Commander example is self-contained:

- `examples/001-incident-command-center-pack/incident_command_center.rs`
- `examples/001-incident-command-center-pack/server.rs`
- `examples/001-incident-command-center-pack/scenario.yaml`
- `examples/001-incident-command-center-pack/README.md`
- `examples/001-incident-command-center-pack/ts_smoke.ts`
- `examples/001-incident-command-center-pack/python_smoke.py`

Cargo example name: `incident_command_center`, declared in `crates/meerkat-meta/Cargo.toml` with path `../../examples/001-incident-command-center-pack/server.rs`.

Useful commands:

```bash
./scripts/repo-cargo check -p meerkat-meta --example incident_command_center
./scripts/repo-cargo test -p meerkat-meta --test incident_command_center_pack
./scripts/repo-cargo run -p meerkat-meta --example incident_command_center
```

Default listen address: `127.0.0.1:63210` (the scenario's `listen_addr`). Override with `INCIDENT_COMMAND_CENTER_LISTEN_ADDR`.

## Development Workflow

Run these commands from the repository root with the existing development
dependencies installed.

For frontend/console changes:

```bash
npm --prefix console run phase0:types --silent
npm --prefix console run phase1:targets --silent
npm --prefix console run build --silent
```

`npm --prefix console run build` updates `console/dist` and `crates/meerkat-meta/console-dist`. Include generated bundle changes when the Rust server should serve the new console.

For Rust changes:

```bash
./scripts/repo-cargo fmt --check
./scripts/repo-cargo check -p meerkat-meta
./scripts/repo-cargo test -p meerkat-meta
```

For SDK changes:

```bash
(cd sdk/python && python3 -m pytest)
npm --prefix sdk/typescript run validate
```

For UI behavior, use the Browser plugin against a live console after code changes.

Governance checks (`validate_traceability_statuses`, `validate_governance_contracts` and the `governance_check` binary) parse the traceability YAML with serde-saphyr under YAML 1.2 rules: `yes`, `on` and `off` stay text and `<<` is an ordinary key. A custom tag, a key given twice, a null where text is expected, a `...` document end marker, nesting deeper than 64 levels, or alias-heavy input (100 or more aliases that outnumber their anchors more than ten to one) is refused as a malformed row (`InvalidTraceabilityRow`).

## Release Workflow

Inspect a release first with the repository wrapper:

```bash
./scripts/repo-cargo release <version>
```

This dry run changes nothing. cargo-release runs the release hook
(`scripts/release-hook.sh`) in a dry run too, with `DRY_RUN=true`, and the hook
then only reports what an execute would bump and stage.

`--execute` creates the release commit and normally tags and pushes it. The
release hook moves Cargo, Python, TypeScript, Bazel-module, generated BUILD,
and install-documentation versions together. For a PR-reviewed cut, prepare
without a tag or push, verify generated Rust BUILD freshness, and run the release
dry-run before opening the PR:

```bash
./scripts/repo-cargo release <version> --execute --no-tag --no-push
node scripts/generate-bazel-rust-builds.mjs --check
make release-dry-run
```

With the repository prerequisites installed, the release hook regenerates the
workspace per-crate BUILD files and stages all tracked `BUILD.bazel` files,
including `crates/meta-store-conformance/BUILD.bazel`. Keep the generator `--check`
command as verification. Tag
the exact merged main commit only after the release PR and push-to-main CI on
that exact commit are green, and the candidate has been accepted as described
below.

When updating Meerkat dependencies, update the family's exact requirements in
both `crates/meerkat-meta/Cargo.toml` and `crates/meta-store-conformance/Cargo.toml`,
including applicable dev-dependencies, before running
`./scripts/repo-cargo update -p ...` for the affected packages. Verify that
`Cargo.lock` resolves the intended coherent upstream family, then run the
appropriate workspace checks through `./scripts/repo-cargo` or Make.

**Release protocol: tags validate only; they no longer build or publish.**
After merging the final release-version/dependency commit, use
`make release-candidate SOURCE_SHA=<full-sha> TIER=core` and
`make release-candidate SOURCE_SHA=<full-sha> TIER=trailing`. The dispatch must
snapshot that same main SHA. Each run builds/signs/packages one tier (core:
Linux x86_64 and aarch64, macOS arm64; trailing: macOS x86_64, Windows) beside
the push-to-main CI run of that SHA, refuses at collection unless that CI run is
already green, and emits an immutable run/attempt/tier/artifact/digest selection
backed by signed provenance and outer/inner executable hashes. Save the
selections outside the repo and obtain consumer acceptance against those bytes.

The release owner then tags the same source as `vX.Y.Z` and invokes
`make release-promote RELEASE_TAG=vX.Y.Z ARTIFACT_SELECTION=<core json-file>`.
`TIER` is for `release-candidate` only: `release-promote`,
`release-assets-recover` and `release-registry-retry` refuse it (unset it if
the shell still exports it from the candidate step).
Promotion accepts only the core selection and verifies and publishes the
original bytes without rebuilding or signing again. The trailing tier is added
afterwards, add-only, with
`make release-assets-recover RELEASE_TAG=vX.Y.Z ARTIFACT_SELECTION=<trailing json-file>`. Assets precede the Python and TypeScript SDKs, then the crates
(`meta-extension-state`, then `meerkat-meta`) and an exact registry readback.
A failed SDK registry upload does not stop the crates; the readback still fails
the run while any registry lacks the exact version.
Releases do not publish documentation (see Current Baseline).
`REGISTRY_DRY_RUN=true` suppresses all publication;
`PUBLISH_RELEASE_PACKAGES=false` selects asset-only promotion.

Use `make release-registry-retry RELEASE_TAG=vX.Y.Z` for registry-only retry
(optional `REGISTRY_DRY_RUN=true`), and `make release-assets-recover
RELEASE_TAG=vX.Y.Z ARTIFACT_SELECTION=<json-file>` to add missing validated
original assets. Never replace existing hashes, rebuild expired candidates,
re-push historical tags, or rerun historical release workflows. Recovery must
dispatch the current workflow on main with the old tag as data. Missing originals
or conflicting hashes fail closed. See CONTRIBUTING.md for receipt/recovery
contracts and the full event/mode matrix.

Verify all registries directly:

```bash
./scripts/repo-cargo search meerkat-meta --limit 1
./scripts/repo-cargo search meta-extension-state --limit 1
npm view @rkat/meta-sdk version
python -m pip index versions meerkat-meta
```

Registry versions are immutable. If a version already published from the wrong commit, roll forward to the next patch version.

Meta Forge (`installer/`) releases on its own `forge-v<version>` tags through `.github/workflows/forge-release.yml`, beside Meta's releases. A candidate builds each target's package from the public inputs pinned in `installer/release/forge-release-inputs.json` and binds them, with a green dispatched `forge.yml` run that tested the same Forge inputs, in an attested candidate manifest; publication releases those bytes as a prerelease that never becomes the latest release. `installer/RELEASING.md` has the steps.

## Tests To Remember

Rust:

- `crates/meerkat-meta/tests/unified_console.rs`
- `crates/meerkat-meta/tests/console_experience.rs`
- `crates/meerkat-meta/tests/console_route_auth.rs`
- `crates/meerkat-meta/tests/cross_mob_signed.rs`
- `crates/meerkat-meta/tests/cross_mob_tcp.rs`
- `crates/meerkat-meta/tests/cross_mob_uds.rs`
- `crates/meerkat-meta/tests/e2e_target_contracts.rs`
- `crates/meerkat-meta/tests/e2e_sdk_wire.rs`
- `crates/meerkat-meta/tests/identity_first_*.rs`
- `crates/meerkat-meta/tests/incident_command_center_pack.rs`
- `crates/meerkat-meta/tests/mob_events_*.rs`
- `crates/meerkat-meta/tests/routing_delivery.rs`
- `crates/meerkat-meta/tests/sse_auth_gate.rs`
- `crates/meerkat-meta/tests/operation_refusal_observer.rs`
- `crates/meerkat-meta/tests/mob_announcements_delivery_process.rs` and `mob_announcements_remote_process.rs` (process tests), plus the unit tests in `src/mob_announcements/`
- `catalogs_response_exposes_filesystem_skill_catalog` in `crates/meerkat-meta/src/mobpack.rs` pins this skill: the repo's `.claude/skills/meerkat-meta/` is the expected filesystem skill (id `meerkat-meta`). Rename both together.

Console:

- `console/src/lib/adapters.test.ts`
- `console/src/lib/network.test.ts`
- `console/src/panels/Sidebar.test.ts`
- `console/src/panels/LogsPanel.test.ts`
- `console/src/panels/SignalsRail.test.ts`
- `packages/console-core/src/*.test.ts`
- `packages/console-components/src/**/*.test.tsx`
- `console/browser-e2e.cjs`

SDK:

- `sdk/python/tests/`
- `sdk/typescript/tests/`

## Practical Rules

- Follow the code that exists now. Start with `rg` and `crates/meerkat-meta/src/lib.rs`.
- Keep examples out of core source.
- Keep signed cross-mob transport fail-closed. Missing pubkeys on real transports are errors.
- Keep raw transport, checksum, scaffold, reasoning, and tool-envelope details out of user-facing console rendering.
- Use structured parsers/contract adapters rather than ad hoc string slicing where the repo already has a boundary helper.
- After frontend changes, rebuild the embedded console bundle.
