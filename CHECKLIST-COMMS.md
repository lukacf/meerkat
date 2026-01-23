# Meerkat Comms Implementation Checklist

**Purpose:** Task-oriented checklist for agentic AI execution of the inter-agent communication system.

**Specification:** See `DESIGN-COMMS.md` for authoritative requirements.

---

## Completed Phases (Excluded)

Phases 0-7 and the original Final Phase have been completed and approved. They covered:

- **Phase 0:** Representation Contracts (types, serialization, round-trips)
- **Phase 1:** Cryptographic Identity (keypair, signing, verification)
- **Phase 2:** Trust Management (trusted peers load/save/query)
- **Phase 3:** Transport Layer (UDS, TCP, framing)
- **Phase 4:** IO Task and Inbox (validation, acks, inbox queue)
- **Phase 5:** Router (send API with ack timeout)
- **Phase 6:** MCP Tools (send_message, send_request, send_response, list_peers)
- **Phase 7:** Agent Loop Integration (meerkat-comms-agent crate, CommsAgent wrapper)
- **Final Phase:** E2E Scenarios (library-level integration tests)

All library functionality is complete. The remaining phases integrate comms into meerkat-core so all interfaces get identical functionality.

---

## RCT Methodology Overview

This implementation follows the **RCT (Representation Contract Tests)** methodology:

1. **Gate 0 Must Be Green First** - Representation contracts must pass before behavior
2. **Representations First, Behavior Second, Internals Last**
3. **Integration Tests Over Unit Tests** - System proof trumps local correctness

---

## Task Format (Agent-Ready)

Each task is:
- **Atomic**: One deliverable per task
- **Observable**: Clear "done when" condition with test name or verifiable outcome
- **File-specific**: Explicit output location when applicable

Format: `- [ ] <action> (Done when: <observable condition>)`

---

## Design Clarification: Async Model

The comms system runs **parallel and async** to the LLM loop:
- **Ack = transport-level receipt** - IO task sends ack immediately when envelope is validated, NOT when LLM processes the message
- **Inbox drain = non-blocking** - Agent drains inbox at turn boundaries; never waits for new messages
- **Decoupled processing** - Sender gets ack immediately; recipient processes message later at turn boundary

This is already implemented correctly in `meerkat-comms`. These phases document it explicitly and test it.

---

## Phase 8: Core Comms Config
PHASE_8_APPROVED

**Goal:** Define portable comms configuration in meerkat-core with path interpolation and defaults.

**Dependencies:** Phase 7 (meerkat-comms-agent complete)

### Tasks - Config Struct (`meerkat-core/src/comms_config.rs`)

- [x] Create `meerkat-core/src/comms_config.rs` (Done when: file exists)
- [x] Add `mod comms_config;` to `meerkat-core/src/lib.rs` (Done when: `cargo check -p meerkat-core` exits 0)
- [x] Define `CoreCommsConfig` struct with fields: `enabled: bool`, `name: String`, `listen_uds: Option<PathBuf>`, `listen_tcp: Option<SocketAddr>`, `identity_dir: PathBuf`, `trusted_peers_path: PathBuf`, `ack_timeout_secs: u64`, `max_message_bytes: usize` (Done when: `test_core_comms_config_struct` passes)
- [x] Derive `Default` for `CoreCommsConfig` with spec defaults: enabled=false, name="meerkat", listen_uds=None, listen_tcp=None, identity_dir=".rkat/identity", trusted_peers_path=".rkat/trusted_peers.json", ack_timeout_secs=30, max_message_bytes=1048576 (Done when: `test_core_comms_config_defaults` passes)
- [x] Derive `Serialize`/`Deserialize` for `CoreCommsConfig` (Done when: `test_core_comms_config_serde` passes)
- [x] Implement `CoreCommsConfig::with_name(name: &str)` builder method (Done when: `test_core_comms_config_with_name` passes)
- [x] Implement path interpolation: `{name}` in paths replaced with config name (Done when: `test_core_comms_config_path_interpolation` passes - e.g., ".rkat/{name}/identity" → ".rkat/alice/identity")
- [x] Implement `CoreCommsConfig::resolve_paths(&self, base_dir: &Path) -> ResolvedCommsConfig` (Done when: `test_core_comms_config_resolve_paths` passes)

### Tasks - Conversion to Library Config

- [x] Define `ResolvedCommsConfig` struct with absolute paths (Done when: `test_resolved_comms_config_struct` passes)
- [x] Implement `ResolvedCommsConfig::to_comms_config(&self) -> meerkat_comms::CommsConfig` (Done when: `test_resolved_to_comms_config` passes)

> Note: Originally planned `to_comms_manager_config` but this would create a cyclic dependency (meerkat-comms-agent depends on meerkat-core). Instead, we provide `to_comms_config()` for the simple config conversion. The full `CommsManagerConfig` creation (with keypair/trusted_peers) happens in CommsRuntime (Phase 9).

### Phase 8 Gate Verification Commands

```bash
cargo test -p meerkat-core comms_config
cargo clippy -p meerkat-core -- -D warnings
```

### Phase 8 Gate Review

Spawn subagents in parallel with prompt "Review Phase 8 of meerkat-comms":
- `rct-guardian`
- `spec-auditor`

---

## Phase 9: CommsRuntime
PHASE_9_APPROVED

**Goal:** Create CommsRuntime in meerkat-core that manages the full comms lifecycle.

**Dependencies:** Phase 8 (CoreCommsConfig complete)

### Tasks - Runtime Struct (`meerkat-core/src/comms_runtime.rs`)

- [x] Create `meerkat-core/src/comms_runtime.rs` (Done when: file exists)
- [x] Add `mod comms_runtime;` to `meerkat-core/src/lib.rs` (Done when: `cargo check -p meerkat-core` exits 0)
- [x] Define `CommsRuntime` struct holding: `manager: CommsManager`, `listener_handles: Vec<JoinHandle<()>>`, `config: ResolvedCommsConfig` (Done when: `test_comms_runtime_struct` passes)
- [x] Implement `CommsRuntime::new(config: ResolvedCommsConfig) -> Result<Self>` that loads/generates keypair, loads trusted peers, creates CommsManager (Done when: `test_comms_runtime_new` passes)
- [x] Implement `CommsRuntime::start_listeners(&mut self) -> Result<()>` spawning UDS and/or TCP listeners based on config (Done when: `test_comms_runtime_start_listeners` passes)
- [x] Implement `CommsRuntime::drain_messages(&mut self) -> Vec<CommsMessage>` (Done when: `test_comms_runtime_drain_messages` passes)
- [x] Implement `CommsRuntime::router(&self) -> &Router` accessor (Done when: `test_comms_runtime_router` passes)
- [x] Implement `CommsRuntime::public_key(&self) -> PubKey` accessor (Done when: `test_comms_runtime_public_key` passes)
- [x] Implement `CommsRuntime::shutdown(&mut self)` stopping listeners cleanly (Done when: `test_comms_runtime_shutdown` passes)

### Tasks - Error Handling

- [x] Define `CommsRuntimeError` enum with variants: `IdentityError`, `TrustLoadError`, `ListenerError`, `AlreadyStarted` (Done when: `test_comms_runtime_error_variants` passes)
- [x] Implement `From<std::io::Error>` for `CommsRuntimeError` (Done when: `test_comms_runtime_error_from_io` passes)

### Phase 9 Gate Verification Commands

```bash
cargo test -p meerkat-core comms_runtime
cargo clippy -p meerkat-core -- -D warnings
```

### Phase 9 Gate Review

Spawn subagents in parallel with prompt "Review Phase 9 of meerkat-comms":
- `rct-guardian`
- `integration-sheriff`

---

## Phase 10: Agent Integration
PHASE_10_APPROVED

**Goal:** Wire CommsRuntime into AgentBuilder so agents automatically get comms tools and inbox processing.

**Dependencies:** Phase 9 (CommsRuntime complete)

### Tasks - AgentBuilder Extension (`meerkat-core/src/agent.rs`)

- [x] Add `comms_config: Option<CoreCommsConfig>` field to `AgentBuilder` (Done when: `test_agent_builder_has_comms_config` passes)
- [x] Implement `AgentBuilder::comms(mut self, config: CoreCommsConfig) -> Self` (Done when: `test_agent_builder_comms_method` passes)
- [x] Add `comms_runtime: Option<CommsRuntime>` field to `Agent` (Done when: `test_agent_has_comms_runtime` passes)
- [x] Wire CommsRuntime creation in `AgentBuilder::build()` when comms enabled (Done when: `test_agent_builder_creates_comms_runtime` passes)
- [x] Start listeners automatically on agent build when comms enabled (Done when: `test_agent_starts_comms_listeners` passes)

### Tasks - Tool Integration

> **Note:** Tool integration is handled by `CommsToolDispatcher` in `meerkat-comms-agent`, not in `meerkat-core`.
> The dispatcher wraps user tools and adds comms tools at composition time. See `meerkat-comms-agent/src/tool_dispatcher.rs`.

- [x] Create comms tools (send_message, send_request, send_response, list_peers) from CommsRuntime when enabled (Done: via `CommsToolDispatcher::new()` using `agent.comms().router()` and `agent.comms().trusted_peers()`)
- [x] Merge comms tools with user-provided tools in AgentBuilder (Done: via `CommsToolDispatcher::with_inner()`)
- [x] Verify agent without comms has no comms tools (Done when: `test_agent_no_comms_tools_when_disabled` passes)

### Tasks - Inbox Processing

- [x] Implement inbox drain at turn boundary in agent loop (Done when: `test_agent_drains_inbox_at_turn_boundary` passes)
- [x] Format drained messages as user message content for LLM (Done when: `test_agent_formats_inbox_for_llm` passes)
- [x] Verify empty inbox does not block agent loop (Done when: `test_agent_empty_inbox_nonblocking` passes)

### Tasks - Accessor Methods

- [x] Implement `Agent::comms(&self) -> Option<&CommsRuntime>` (Done when: `test_agent_comms_accessor` passes)
- [x] Implement `Agent::comms_mut(&mut self) -> Option<&mut CommsRuntime>` (Done when: `test_agent_comms_mut_accessor` passes)

### Tasks - Subagent Restrictions

- [x] Ensure subagents have comms DISABLED regardless of parent config (Done when: `test_subagent_has_no_comms` passes)
- [x] Document: subagents cannot have network exposure per security model (Done when: comment exists in code)

### Phase 10 Gate Verification Commands

```bash
cargo test -p meerkat-core agent
cargo test -p meerkat-core comms
cargo clippy -p meerkat-core -- -D warnings
```

### Phase 10 Gate Review

Spawn subagents in parallel with prompt "Review Phase 10 of meerkat-comms":
- `integration-sheriff`
- `spec-auditor`

---

## Phase 11: CLI Integration

**Goal:** Wire comms config loading into CLI so `rkat` can use comms.

**Dependencies:** Phase 10 (Agent integration complete)

### Tasks - Config Loading (`meerkat-cli/src/config.rs`)

- [x] Add `comms: Option<CoreCommsConfig>` field to CLI config struct (Done when: `test_cli_config_has_comms` passes)
- [x] Load `[comms]` section from `~/.config/rkat/config.toml` (Done when: `test_cli_loads_comms_from_toml` passes)
- [x] Load `[comms]` section from project `.rkat/config.toml` with override priority (Done when: `test_cli_project_config_overrides` passes)
- [x] Support `--comms-name` CLI flag to override name (Done when: `test_cli_comms_name_flag` passes)
- [x] Support `--comms-listen-tcp` CLI flag (Done when: `test_cli_comms_listen_tcp_flag` passes)
- [x] Support `--no-comms` CLI flag to disable (Done when: `test_cli_no_comms_flag` passes)

### Tasks - AgentBuilder Wiring

- [x] Pass comms config to AgentBuilder in CLI run command (Done when: CLI uses `AgentBuilder::comms()`)
- [x] Resolve paths relative to config file location (Done when: `test_cli_resolves_comms_paths` passes)

### Tasks - Status Display

- [x] Display comms status on startup when enabled (Done when: running `rkat` with comms shows "Comms: listening on ...")
- [x] Display peer ID on startup (Done when: running `rkat` with comms shows "Peer ID: ed25519:...")

### Phase 11 Gate Verification Commands

```bash
cargo test -p meerkat-cli config
cargo clippy -p meerkat-cli -- -D warnings
```

### Phase 11 Gate Review

Spawn subagents in parallel with prompt "Review Phase 11 of meerkat-comms":
- `spec-auditor`
- `integration-sheriff`

PHASE_11_APPROVED

---

## Phase 12: Real rkat E2E Tests

**Goal:** E2E tests that spawn actual `rkat` processes and verify inter-process communication.

**Dependencies:** Phase 11 (CLI integration complete)

### Tasks - Test Harness (`meerkat-cli/tests/e2e_rkat_comms.rs`)

- [x] Create `meerkat-cli/tests/e2e_rkat_comms.rs` (Done when: file exists)
- [x] Implement `RkatTestInstance` struct for managing a test rkat process (Done when: `test_rkat_instance_spawn_kill` passes)
- [x] Implement temp directory setup with identity and trusted_peers.json (Done when: `test_rkat_temp_setup` passes)
- [x] Implement mutual trust config generation for N test instances (Done when: `test_rkat_mutual_trust_setup` passes)
- [x] Implement `RkatTestInstance::wait_for_ready()` checking for startup message (Done when: `test_rkat_wait_for_ready` passes)

### Tasks - E2E Scenarios

- [x] E2E: Two rkat instances exchange Message over TCP (Done when: `test_e2e_rkat_tcp_message_exchange` passes)
- [x] E2E: Two rkat instances exchange Message over UDS (Done when: `test_e2e_rkat_uds_message_exchange` passes)
- [x] E2E: Full Request → Ack → Response flow with real rkat (Done when: `test_e2e_rkat_request_response_flow` passes)
- [x] E2E: Untrusted peer connection rejected (Done when: `test_e2e_rkat_untrusted_rejected` passes)
- [x] E2E: Three rkat peers coordinate (Done when: `test_e2e_rkat_three_peer_coordination` passes)

### Tasks - Async Model Verification

- [x] E2E: Ack returns immediately even if recipient LLM is busy (Done when: `test_e2e_rkat_ack_is_immediate` passes)
  - Use `RKAT_TEST_LLM_DELAY_MS` env var to simulate slow LLM
  - Verify sender gets ack in <100ms while recipient takes >1s to process
- [x] E2E: Sender does not block waiting for recipient processing (Done when: `test_e2e_rkat_sender_nonblocking` passes)

### Phase 12 Gate Verification Commands

```bash
cargo test -p meerkat-cli --test e2e_rkat_comms -- --ignored
cargo clippy -p meerkat-cli -- -D warnings
```

### Phase 12 Gate Review

Spawn subagents in parallel with prompt "Review Phase 12 of meerkat-comms":
- `integration-sheriff`
- `spec-auditor`

PHASE_12_APPROVED

---

## Phase 13: Documentation

**Goal:** Update DESIGN-COMMS.md with async model clarification and CLI config examples.

**Dependencies:** Phase 12 (all implementation complete)

### Tasks - DESIGN-COMMS.md Updates

- [x] Add "Async Model" section explaining ack vs processing distinction (Done when: section exists with clear explanation)
- [x] Update "Ack Rules" section to state: "Ack = envelope received and validated, NOT message processed by LLM" (Done when: clarification added)
- [x] Add "CLI Configuration" section with example config.toml (Done when: example shown)
- [x] Add "Interface Parity" section documenting that CLI/MCP/REST/SDK all use identical core (Done when: section exists)
- [x] Review all sections for accuracy against final implementation (Done when: no spec/impl mismatches)

### Phase 13 Gate Verification Commands

```bash
# Documentation review is manual
cat DESIGN-COMMS.md | head -200
```

### Phase 13 Gate Review (Final)

Spawn subagents in parallel with prompt "Review Phase 13 of meerkat-comms":
- `spec-auditor`
- `methodology-integrity`

PHASE_13_APPROVED

---

## Success Criteria

**Library phases (complete):**
1. [x] All existing meerkat workspace tests still pass
2. [x] Phase 0 RCT tests green (all round-trips pass)
3. [x] All E2E scenario tests pass (library level)
4. [x] No `todo!()`, `unimplemented!()`, or stub code in `meerkat-comms*/src/`
5. [x] Two Meerkats can exchange messages over UDS (library)
6. [x] Two Meerkats can exchange messages over TCP (library)
7. [x] Concurrent messages from multiple peers handled correctly
8. [x] 100% spec coverage verified by Spec Auditor
9. [x] `cargo clippy -p meerkat-comms -p meerkat-comms-mcp -p meerkat-comms-agent -- -D warnings` passes

**Core integration phases (complete):**
10. [x] CoreCommsConfig in meerkat-core with path interpolation (Phase 8)
11. [x] CommsRuntime in meerkat-core managing full lifecycle (Phase 9)
12. [x] AgentBuilder integration with automatic tool/inbox wiring (Phase 10)
13. [x] CLI loads comms config and passes to AgentBuilder (Phase 11)
14. [x] Real rkat E2E tests pass (actual processes communicating) (Phase 12)
15. [x] Async model documented (ack != processing) (Phase 13)

---

## Appendix: Gate Review Agents

Gate reviewers are defined in `.claude/agents/`:

| Agent | Purpose |
|-------|---------|
| `rct-guardian` | Representation contracts, serialization, round-trips |
| `integration-sheriff` | Cross-component wiring, data flow, resource lifecycle |
| `spec-auditor` | Requirements compliance, spec matching |
| `methodology-integrity` | Detect process gaming (final phase only) |

Each reviewer returns a structured verdict: `APPROVE` or `BLOCK`.
All listed reviewers must APPROVE before marking a phase complete.
