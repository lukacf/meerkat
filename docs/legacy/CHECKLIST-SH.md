# Meerkat Shell Tool Implementation Checklist

**Purpose:** Task-oriented checklist for agentic AI execution of the shell tool system.

**Specification:** See `SH_DESIGN.md` for authoritative requirements.

---

## RCT Methodology Overview

This implementation follows the **RCT (Representation Contract Tests)** methodology:

1. **Gate 0 Must Be Green First** - Representation contracts must pass before behavior
2. **Representations First, Behavior Second, Internals Last**
3. **Integration Tests Over Unit Tests** - System proof trumps local correctness

### Phase vs RCT Gate Distinction

- **Phase Gates**: Approval checkpoints for each implementation phase
- **RCT Gates**: Methodology gates (Gate 0 = representations, Gate 1-2 = behavior, Gate 3 = units)
- Phase 0 aligns with **RCT Gate 0** and MUST be green before proceeding
- Phases 1-6 align with **RCT Gates 1-2** (E2E red is OK in early phases)
- Phase 7 aligns with **RCT Gate 3** - all gates MUST be green

---

## Task Format (Agent-Ready)

Each task is:
- **Atomic**: One deliverable per task
- **Observable**: Clear "done when" condition with test name or verifiable outcome
- **File-specific**: Explicit output location when applicable

Format: `- [ ] <action> (Done when: <observable condition>)`

---

## Anti-Pattern Detection (For Reviewers)

At each gate review, reviewers MUST independently verify:

1. **Status Echoing**: Did the reviewer discover anything not already stated in the prompt? (Reviewer prompts must NOT include test results)
2. **XFAIL Abuse**: `grep -r "#\[ignore\]" src/` - Are feature tests marked ignore?
3. **Infinite Deferral**: Are core features in a "later version" list?
4. **False Completion**: `grep -r "todo!\|unimplemented!\|NotImplementedError" src/` - Stubs in "completed" code?
5. **Process Displacement**: More doc changes than code changes in this phase?

---

## Phase 0: Types & Representation Contracts (RCT Gate 0)
PHASE_0_APPROVED

**Goal:** Define core types with serialization round-trip tests.

**Dependencies:** None

**RCT Alignment:** Gate 0 - MUST be GREEN before proceeding to Phase 1.

### Tasks - Types (`meerkat-tools/src/builtin/shell/types.rs`)

- [x] Create `meerkat-tools/src/builtin/shell/` directory (Done when: directory exists)
- [x] Create `meerkat-tools/src/builtin/shell/types.rs` (Done when: file exists)
- [x] Define `JobId` newtype wrapping `String` with "job_" + ULID format (Done when: `test_job_id_format` passes)
- [x] Implement `JobId::new()` generating ULID-based ID (Done when: `test_job_id_new_unique` passes)
- [x] Derive `Serialize`/`Deserialize` for `JobId` (Done when: `test_job_id_serde_roundtrip` passes)
- [x] Define `JobStatus` enum with variants: `Running`, `Completed`, `Failed`, `TimedOut`, `Cancelled` (Done when: `test_job_status_variants` passes)
- [x] Derive `Serialize`/`Deserialize` for `JobStatus` with `#[serde(tag = "status", rename_all = "snake_case")]` (Done when: `test_job_status_serde_roundtrip` passes)
- [x] Verify `JobStatus::Completed` serializes with `exit_code`, `stdout`, `stderr`, `duration_secs` fields (Done when: `test_job_status_completed_fields` passes)
- [x] Define `BackgroundJob` struct with fields: `id`, `command`, `working_dir`, `timeout_secs`, `status` (Done when: `test_background_job_struct` passes)
- [x] Derive `Serialize`/`Deserialize` for `BackgroundJob` (Done when: `test_background_job_serde_roundtrip` passes)
- [x] Define `JobSummary` struct with fields: `id`, `command`, `status` (string), `started_at_unix` (Done when: `test_job_summary_struct` passes)
- [x] Derive `Serialize`/`Deserialize` for `JobSummary` (Done when: `test_job_summary_serde_roundtrip` passes)

### Tasks - Module Setup

- [x] Create `meerkat-tools/src/builtin/shell/mod.rs` with public exports (Done when: `cargo check -p meerkat-tools` exits 0)
- [x] Add `mod shell;` to `meerkat-tools/src/builtin/mod.rs` (Done when: `cargo check -p meerkat-tools` exits 0)

### Phase 0 Gate Verification Commands

```bash
# Run ALL tests in meerkat-tools (not filtered)
cargo test -p meerkat-tools
# Verify clippy
cargo clippy -p meerkat-tools -- -D warnings
# Verify round-trip tests exist and ran (check output for test names)
cargo test -p meerkat-tools 2>&1 | grep -E "test_.*roundtrip"
```

### Phase 0 Gate Review

Spawn subagent with prompt "Review Phase 0 of meerkat-shell":
- `rct-guardian`

**Reviewer scope:** Verify all serde round-trip tests exist and pass. Check that deserialized values equal originals.

---

## Phase 1: ShellConfig
PHASE_1_APPROVED

**Goal:** Define configuration struct with defaults and validation.

**Dependencies:** Phase 0 (types complete)

**RCT Alignment:** Still Gate 0 territory (representation). Must be green.

### Tasks - Config (`meerkat-tools/src/builtin/shell/config.rs`)

- [x] Create `meerkat-tools/src/builtin/shell/config.rs` (Done when: file exists)
- [x] Define `ShellConfig` struct with fields: `enabled: bool`, `default_timeout_secs: u64`, `restrict_to_project: bool`, `shell: String`, `project_root: PathBuf` (Done when: `test_shell_config_struct` passes)
- [x] Derive `Default` for `ShellConfig`: enabled=false, default_timeout_secs=30, restrict_to_project=true, shell="nu" (Done when: `test_shell_config_defaults` passes)
- [x] Derive `Serialize`/`Deserialize` for `ShellConfig` (Done when: `test_shell_config_serde_roundtrip` passes)
- [x] Implement `ShellConfig::with_project_root(path: PathBuf) -> Self` builder (Done when: `test_shell_config_with_project_root` passes)
- [x] Implement `ShellConfig::validate_working_dir(&self, dir: &Path) -> Result<PathBuf>` (Done when: `test_shell_config_validate_working_dir` passes)
- [x] Verify `validate_working_dir` rejects paths escaping project root (Done when: `test_shell_config_rejects_escape` passes)
- [x] Add `shell::config` to module exports (Done when: `cargo check -p meerkat-tools` exits 0)

### Phase 1 Gate Verification Commands

```bash
cargo test -p meerkat-tools
cargo clippy -p meerkat-tools -- -D warnings
```

### Phase 1 Gate Review

Spawn subagents in parallel with prompt "Review Phase 1 of meerkat-shell":
- `rct-guardian`
- `spec-auditor`

**Reviewer scope:**
- rct-guardian: Verify ShellConfig round-trip, Default values match spec
- spec-auditor: Verify defaults match SH_DESIGN.md exactly

---

## Phase 2: ShellTool (Synchronous)
PHASE_2_APPROVED

**Goal:** Implement synchronous shell command execution.

**Dependencies:** Phase 1 (config complete)

**RCT Alignment:** Gate 1-2 (behavior). E2E tests may be red.

### Tasks - Error Types

- [x] Define `ShellError` enum in `types.rs` with variants: `ShellNotInstalled`, `BlockedCommand`, `WorkingDirEscape`, `WorkingDirNotFound`, `JobNotFound`, `JobNotRunning`, `Io` (Done when: `test_shell_error_variants` passes) - NOTE: Defined in config.rs during Phase 1
- [x] Implement `std::error::Error` for `ShellError` using thiserror (Done when: `test_shell_error_display` passes)
- [x] Implement `From<std::io::Error>` for `ShellError` (Done when: `test_shell_error_from_io` passes)

### Tasks - Tool Implementation (`meerkat-tools/src/builtin/shell/tool.rs`)

- [x] Create `meerkat-tools/src/builtin/shell/tool.rs` (Done when: file exists)
- [x] Define `ShellTool` struct holding `config: ShellConfig` (Done when: `test_shell_tool_struct` passes)
- [x] Implement `ShellTool::new(config: ShellConfig) -> Self` (Done when: `test_shell_tool_new` passes)
- [x] Implement `BuiltinTool` trait for `ShellTool` (Done when: `test_shell_tool_implements_builtin` passes)
- [x] Return correct tool name "shell" (Done when: `test_shell_tool_name` passes)
- [x] Return correct input schema per SH_DESIGN.md (Done when: `test_shell_tool_schema` passes)
- [x] Implement synchronous command execution via subprocess (Done when: `test_shell_tool_sync_execute` passes)
- [x] Handle timeout with process kill (Done when: `test_shell_tool_timeout` passes)
- [x] Return structured output: `exit_code`, `stdout`, `stderr`, `timed_out`, `duration_secs` (Done when: `test_shell_tool_output_format` passes)
- [x] Verify full environment inheritance (Done when: `test_shell_tool_env_inheritance` passes)
- [x] Override PWD to working directory (Done when: `test_shell_tool_pwd_override` passes)

### Tasks - Shell Detection

- [x] Implement shell executable detection (`which nu`) (Done when: `test_shell_tool_detect_nu` passes)
- [x] Return clear error when shell not installed (Done when: `test_shell_tool_not_installed_error` passes)

### Phase 2 Gate Verification Commands

```bash
cargo test -p meerkat-tools
cargo clippy -p meerkat-tools -- -D warnings
```

### Phase 2 Gate Review

Spawn subagents in parallel with prompt "Review Phase 2 of meerkat-shell":
- `spec-auditor`
- `integration-sheriff`

**Reviewer scope:**
- spec-auditor: Tool schema matches SH_DESIGN.md, output format matches spec
- integration-sheriff: BuiltinTool trait implemented correctly, subprocess spawning works

**Red OK:** E2E tests for agent integration may fail (Phase 5-7 scope).

---

## Phase 3: JobManager
PHASE_3_APPROVED

**Goal:** Implement background job management with async execution.

**Dependencies:** Phase 2 (synchronous execution complete)

**RCT Alignment:** Gate 1-2 (behavior). E2E tests may be red.

### Tasks - JobManager (`meerkat-tools/src/builtin/shell/job_manager.rs`)

- [x] Create `meerkat-tools/src/builtin/shell/job_manager.rs` (Done when: file exists)
- [x] Define `JobManager` struct holding: `jobs: Arc<Mutex<HashMap<JobId, BackgroundJob>>>`, `config: ShellConfig` (Done when: `test_job_manager_struct` passes)
- [x] Implement `JobManager::new(config: ShellConfig) -> Self` (Done when: `test_job_manager_new` passes)
- [x] Implement `JobManager::spawn_job(&self, command: &str, working_dir: Option<&Path>, timeout_secs: u64) -> Result<JobId>` (Done when: `test_job_manager_spawn` passes)
- [x] Verify `spawn_job` returns immediately with JobId (Done when: `test_job_manager_spawn_immediate` passes)
- [x] Verify spawned job status is `Running` immediately after spawn (Done when: `test_job_manager_spawn_running` passes)
- [x] Implement `JobManager::get_status(&self, job_id: &JobId) -> Option<BackgroundJob>` (Done when: `test_job_manager_get_status` passes)
- [x] Implement `JobManager::list_jobs(&self) -> Vec<JobSummary>` (Done when: `test_job_manager_list_jobs` passes)
- [x] Implement `JobManager::cancel_job(&self, job_id: &JobId) -> Result<()>` (Done when: `test_job_manager_cancel` passes)
- [x] Verify cancel sends SIGTERM then SIGKILL (Done when: `test_job_manager_cancel_signal` passes)

### Tasks - Async Execution

- [x] Spawn tokio task for background execution (Done when: `test_job_manager_tokio_spawn` passes)
- [x] Update job status to `Completed` on success (Done when: `test_job_manager_completed_status` passes)
- [x] Update job status to `Failed` on spawn error (Done when: `test_job_manager_failed_status` passes)
- [x] Update job status to `TimedOut` on timeout (Done when: `test_job_manager_timeout_status` passes)
- [x] Update job status to `Cancelled` after cancel (Done when: `test_job_manager_cancelled_status` passes)

### Tasks - Event Notification

- [x] Add `event_tx: Option<mpsc::Sender<EventPayload>>` to JobManager (Done when: `test_job_manager_has_event_sender` passes)
- [x] Implement `JobManager::with_event_sender(self, tx: mpsc::Sender<EventPayload>) -> Self` (Done when: `test_job_manager_with_event_sender` passes)
- [x] Send `shell_job_completed` event on job completion (Done when: `test_job_manager_sends_completion_event` passes)
- [x] Verify event contains job_id, command, and result (Done when: `test_job_manager_event_payload` passes)

### Phase 3 Gate Verification Commands

```bash
cargo test -p meerkat-tools
cargo clippy -p meerkat-tools -- -D warnings
```

### Phase 3 Gate Review

Spawn subagents in parallel with prompt "Review Phase 3 of meerkat-shell":
- `rct-guardian`
- `integration-sheriff`

**Reviewer scope:**
- rct-guardian: JobStatus transitions are correct, event payload serializes correctly
- integration-sheriff: Tokio task spawning, Arc/Mutex usage, channel wiring

**Red OK:** E2E agent integration tests may fail.

---

## Phase 4: Job Management Tools
PHASE_4_APPROVED

**Goal:** Implement shell_job_status, shell_jobs, and shell_job_cancel tools.

**Dependencies:** Phase 3 (JobManager complete)

**RCT Alignment:** Gate 1-2 (behavior). E2E tests may be red.

### Tasks - shell_job_status (`meerkat-tools/src/builtin/shell/job_status_tool.rs`)

- [x] Create `meerkat-tools/src/builtin/shell/job_status_tool.rs` (Done when: file exists)
- [x] Define `ShellJobStatusTool` holding `Arc<JobManager>` (Done when: `test_shell_job_status_tool_struct` passes)
- [x] Implement `BuiltinTool` for `ShellJobStatusTool` (Done when: `test_shell_job_status_tool_builtin` passes)
- [x] Return tool name "shell_job_status" (Done when: `test_shell_job_status_tool_name` passes)
- [x] Return correct input schema requiring `job_id` (Done when: `test_shell_job_status_tool_schema` passes)
- [x] Return full `BackgroundJob` as JSON output (Done when: `test_shell_job_status_tool_output` passes)
- [x] Return error for unknown job_id (Done when: `test_shell_job_status_tool_not_found` passes)

### Tasks - shell_jobs (`meerkat-tools/src/builtin/shell/jobs_list_tool.rs`)

- [x] Create `meerkat-tools/src/builtin/shell/jobs_list_tool.rs` (Done when: file exists)
- [x] Define `ShellJobsListTool` holding `Arc<JobManager>` (Done when: `test_shell_jobs_list_tool_struct` passes)
- [x] Implement `BuiltinTool` for `ShellJobsListTool` (Done when: `test_shell_jobs_list_tool_builtin` passes)
- [x] Return tool name "shell_jobs" (Done when: `test_shell_jobs_list_tool_name` passes)
- [x] Return empty object input schema (Done when: `test_shell_jobs_list_tool_schema` passes)
- [x] Return array of `JobSummary` as JSON output (Done when: `test_shell_jobs_list_tool_output` passes)

### Tasks - shell_job_cancel (`meerkat-tools/src/builtin/shell/job_cancel_tool.rs`)

- [x] Create `meerkat-tools/src/builtin/shell/job_cancel_tool.rs` (Done when: file exists)
- [x] Define `ShellJobCancelTool` holding `Arc<JobManager>` (Done when: `test_shell_job_cancel_tool_struct` passes)
- [x] Implement `BuiltinTool` for `ShellJobCancelTool` (Done when: `test_shell_job_cancel_tool_builtin` passes)
- [x] Return tool name "shell_job_cancel" (Done when: `test_shell_job_cancel_tool_name` passes)
- [x] Return correct input schema requiring `job_id` (Done when: `test_shell_job_cancel_tool_schema` passes)
- [x] Return success JSON with job_id and status (Done when: `test_shell_job_cancel_tool_output` passes)
- [x] Return error for unknown job_id (Done when: `test_shell_job_cancel_tool_not_found` passes)
- [x] Return error for already-completed job (Done when: `test_shell_job_cancel_tool_not_running` passes)

### Tasks - Tool Registration

- [x] Update `shell/mod.rs` to export all tool types (Done when: `cargo check -p meerkat-tools` exits 0)
- [x] Create `ShellToolSet` struct holding all four tools (Done when: `test_shell_tool_set_struct` passes)
- [x] Implement `ShellToolSet::new(config: ShellConfig) -> Self` creating tools with shared JobManager (Done when: `test_shell_tool_set_new` passes)
- [x] Implement `ShellToolSet::tools(&self) -> Vec<&dyn BuiltinTool>` (Done when: `test_shell_tool_set_tools` passes)

### Phase 4 Gate Verification Commands

```bash
cargo test -p meerkat-tools
cargo clippy -p meerkat-tools -- -D warnings
```

### Phase 4 Gate Review

Spawn subagents in parallel with prompt "Review Phase 4 of meerkat-shell":
- `spec-auditor`
- `integration-sheriff`

**Reviewer scope:**
- spec-auditor: All four tools match SH_DESIGN.md schemas exactly
- integration-sheriff: Shared JobManager via Arc, tool dispatch routing

**Red OK:** E2E agent integration tests may fail.

---

## Phase 5: Agent Loop Integration
PHASE_5_APPROVED

**Goal:** Wire shell tools into agent loop with event processing.

**Dependencies:** Phase 4 (all tools complete)

**RCT Alignment:** Gate 2 (integration). E2E tests should start passing.

### Tasks - CompositeDispatcher Integration

- [x] Add `shell_tools: Option<ShellToolSet>` to `CompositeDispatcher` (Done when: `test_composite_has_shell_tools` passes)
- [x] Implement `CompositeDispatcher::with_shell(config: ShellConfig) -> Self` builder (Done when: `test_composite_with_shell` passes)
- [x] Wire shell tools into `available_tools()` when enabled (Done when: `test_composite_shell_tools_available` passes)
- [x] Route shell tool calls through ShellToolSet (Done when: `test_composite_dispatches_shell` passes)

### Tasks - Event Channel Setup

- [x] Add shell event receiver to Agent struct (Done when: `test_agent_has_shell_event_rx` passes)
- [x] Wire event sender to JobManager on agent build (Done when: `test_agent_wires_shell_events` passes)

### Tasks - DrainingEvents Phase

- [x] Check for shell job completion events in DrainingEvents (Done when: `test_agent_checks_shell_events` passes)
- [x] Format completion event as system message (Done when: `test_agent_formats_shell_event` passes)
- [x] Verify non-blocking event drain (Done when: `test_agent_shell_events_nonblocking` passes)

### Phase 5 Gate Verification Commands

```bash
cargo test -p meerkat-core
cargo test -p meerkat-tools
cargo clippy -p meerkat-core -p meerkat-tools -- -D warnings
```

### Phase 5 Gate Review

Spawn subagents in parallel with prompt "Review Phase 5 of meerkat-shell":
- `integration-sheriff`
- `spec-auditor`

**Reviewer scope:**
- integration-sheriff: Event channel wiring, DrainingEvents phase extension, non-blocking drain
- spec-auditor: Event payload format matches SH_DESIGN.md

---

## Phase 6: CLI Integration
PHASE_6_APPROVED

**Goal:** Wire shell config into CLI so `rkat` can use shell tools.

**Dependencies:** Phase 5 (agent integration complete)

**RCT Alignment:** Gate 2 (integration). All unit/integration tests should pass.

### Tasks - Config Loading

- [x] Add `shell: Option<ShellConfig>` to CLI config struct (Done when: `test_cli_config_has_shell` passes)
- [x] Load `[tools.shell]` section from config.toml (Done when: `test_cli_loads_shell_config` passes)
- [x] Support `--shell` CLI flag to enable (Done when: `test_cli_shell_flag` passes)
- [x] Support `--no-shell` CLI flag to disable (Done when: `test_cli_no_shell_flag` passes)

### Tasks - AgentBuilder Wiring

- [x] Pass shell config to CompositeDispatcher in CLI run command (Done when: CLI uses shell config)
- [x] Resolve project_root relative to config location (Done when: `test_cli_resolves_shell_project_root` passes)

### Tasks - Status Display

- [x] Display shell status on startup when enabled (Done when: running `rkat` with shell shows "Shell: enabled (nu)")
- [x] Display warning if nu not found (Done when: running `rkat` with shell but no nu shows warning)

### Phase 6 Gate Verification Commands

```bash
cargo test -p meerkat-cli
cargo clippy -p meerkat-cli -- -D warnings
```

### Phase 6 Gate Review

Spawn subagents in parallel with prompt "Review Phase 6 of meerkat-shell":
- `spec-auditor`
- `integration-sheriff`

**Reviewer scope:**
- spec-auditor: Config precedence matches SH_DESIGN.md (per-call > project > user > defaults)
- integration-sheriff: Config flows from TOML to AgentBuilder to CompositeDispatcher

---

## Phase 7: E2E Tests (Final Phase)
PHASE_7_APPROVED

**Goal:** End-to-end tests verifying full shell tool functionality.

**Dependencies:** Phase 6 (CLI integration complete)

**RCT Alignment:** Gate 3 (all gates GREEN). No red tests allowed.

### Tasks - Test Setup (`meerkat/tests/e2e_shell.rs`)

- [x] Create `meerkat/tests/e2e_shell.rs` (Done when: file exists)
- [x] Implement test helper to check if `nu` is installed (Done when: helper function exists)
- [x] Add `#[ignore]` attribute for tests requiring nu (Done when: tests can run without nu)

### Tasks - Synchronous Execution

- [x] E2E: Agent executes sync shell command and receives output (Done when: `test_e2e_shell_sync_execute` passes)
- [x] E2E: Agent handles command timeout (Done when: `test_e2e_shell_sync_timeout` passes)
- [x] E2E: Agent receives exit code from command (Done when: `test_e2e_shell_exit_code` passes)
- [x] E2E: Agent handles command with stderr output (Done when: `test_e2e_shell_stderr` passes)

### Tasks - Background Execution

- [x] E2E: Agent spawns background job and receives job_id (Done when: `test_e2e_shell_background_spawn` passes)
- [x] E2E: Agent receives completion event after background job finishes (Done when: `test_e2e_shell_background_completion` passes)
- [x] E2E: Agent can cancel running background job (Done when: `test_e2e_shell_background_cancel` passes)
- [x] E2E: Agent can list multiple concurrent jobs (Done when: `test_e2e_shell_multiple_jobs` passes)

### Tasks - Error Handling

- [x] E2E: Agent receives error when shell not installed (Done when: `test_e2e_shell_not_installed` passes)
- [x] E2E: Agent receives error for invalid working directory (Done when: `test_e2e_shell_invalid_workdir` passes)
- [x] E2E: Agent receives error when checking non-existent job (Done when: `test_e2e_shell_job_not_found` passes)

### Phase 7 Gate Verification Commands

```bash
# All workspace tests must pass
cargo test --workspace
# E2E tests (requires nu installed)
cargo test -p meerkat --test e2e_shell -- --ignored
# Final clippy check
cargo clippy --workspace -- -D warnings
# Verify no stubs remain
grep -r "todo!\|unimplemented!\|NotImplementedError" meerkat-tools/src/builtin/shell/ && echo "FAIL: stubs found" || echo "OK: no stubs"
```

### Phase 7 Gate Review (Final)

Spawn subagents in parallel with prompt "Review Phase 7 (FINAL) of meerkat-shell":
- `integration-sheriff`
- `methodology-integrity`

**Reviewer scope:**
- integration-sheriff: Full data flow from CLI to agent to tool to subprocess and back
- methodology-integrity: No XFAIL abuse, no infinite deferral, no false completion, no process displacement

**CRITICAL:** methodology-integrity MUST verify:
1. Zero `todo!()`, `unimplemented!()` in shell module
2. All SH_DESIGN.md features implemented (not deferred)
3. No tests marked `#[ignore]` for feature gaps (only for external dependencies like nu)

---

## Success Criteria

1. [ ] All existing meerkat workspace tests still pass
2. [ ] Phase 0 RCT tests green (all serde round-trips pass)
3. [ ] Synchronous shell command execution works
4. [ ] Background job execution with completion events works
5. [ ] Job status, listing, and cancellation work
6. [ ] Shell tools disabled by default
7. [ ] Full environment inherited (no denylist)
8. [ ] PWD correctly set to working directory
9. [ ] Working directory restriction enforced when enabled
10. [ ] E2E tests pass with nu installed
11. [ ] No `todo!()`, `unimplemented!()`, or stub code in shell module
12. [ ] `cargo clippy --workspace -- -D warnings` passes
13. [ ] 100% spec coverage verified by Spec Auditor

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

### Reviewer Independence Rule

**CRITICAL:** Reviewer prompts must NOT include:
- Test results or error logs
- Current state summaries
- Lists of what "should" pass

Reviewers must run verification commands themselves and discover state independently. If a reviewer's findings exactly match what was provided to it, the review is invalid.
