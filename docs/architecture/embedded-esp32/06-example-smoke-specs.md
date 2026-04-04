# Phase D - Example Pattern And Smoke Specifications

This file defines the two required ESP examples as user-facing recommended patterns that also act as authoritative smoke entrypoints.

## Shared example rules

| Rule ID | Requirement | Basis |
| --- | --- | --- |
| EXAMPLE-RULE-001 | Both examples must follow the repo example naming and layout convention. | [examples/README.md#L202-L217](../../../examples/README.md#L202-L217) |
| EXAMPLE-RULE-002 | Both examples must provide an `examples.sh` entrypoint that rebuilds repo-local runtime artifacts before building or flashing the target example. | [examples/033-the-office-demo-sh/examples.sh#L1-L24](../../../examples/033-the-office-demo-sh/examples.sh#L1-L24) |
| EXAMPLE-RULE-003 | Both examples must be self-contained real-surface entrypoints, not shell wrappers around toy mocks. | [examples/034-codemob-mcp/src/main.rs#L26-L235](../../../examples/034-codemob-mcp/src/main.rs#L26-L235) |
| EXAMPLE-RULE-004 | Both examples must expose a user-facing `run` mode in addition to validation modes. | `REQ-010`, `REQ-011`, `CONTRACT-006` |
| EXAMPLE-RULE-005 | Both examples must support `build-only`, `host-sim`, and `hardware-smoke` modes. | `REQ-010`, `REQ-011`, `CHOKE-004` |
| EXAMPLE-RULE-006 | Both examples must emit stable machine-readable markers to serial output in their smoke modes. | `REQ-010`, `REQ-011`, `E2E-*` |
| EXAMPLE-RULE-007 | Internal Phase 0 probes live under `scripts/live_smoke/*`, not in the examples tree. | `DEC-006` |
| EXAMPLE-RULE-008 | Both examples must teach a reusable operator workflow in the README, not only smoke invocations. | `CONTRACT-006`, [08-field-learnings-from-m5dial.md](./08-field-learnings-from-m5dial.md) |
| EXAMPLE-RULE-009 | When the chosen stack supports low-friction redeploy such as OTA, the example must surface that workflow in its README and script ergonomics; otherwise the README must explain the limitation. | `REQ-010`, [08-field-learnings-from-m5dial.md](./08-field-learnings-from-m5dial.md) |
| EXAMPLE-RULE-010 | Example 037 must generate an operator-facing topology report or visualizer from the same inventory and telemetry used in run and smoke modes. | `REQ-011`, [08-field-learnings-from-m5dial.md](./08-field-learnings-from-m5dial.md) |
| EXAMPLE-RULE-011 | Example 037 must have a predeclared topology-only fallback if metric relative-position estimation is rejected on real hardware. | `REQ-011`, `CHOKE-005` |

## Example 036 - `examples/036-esp32-event-agent-sh`

### Purpose

The recommended single-node embedded pattern: a persistent event-driven field agent that stays alive, accepts out-of-band events, invokes local tools, streams provider output, resumes after restart, and remains practical to redeploy in the field.

### Inherited patterns

- 033 pattern: `examples.sh` rebuilds repo-local runtime artifacts before the example build.
- 034 pattern: the example owns a real surface entrypoint and long-running behavior rather than acting as a detached demo shell.

### Required directory contract

- `examples/036-esp32-event-agent-sh/README.md`
- `examples/036-esp32-event-agent-sh/examples.sh`
- one self-contained runtime application payload owned by the example
- one marker parser or smoke helper owned by the example or a shared smoke helper path
- one documented redeploy path, ideally OTA when supported by the chosen stack

### Real entrypoint

- `./examples/036-esp32-event-agent-sh/examples.sh --mode run`

### Setup

- Real ESP32-S3 board with PSRAM and USB serial access
- Wi-Fi credentials available to the example
- Provider API key available to the example
- Persistent storage initialized through the example
- Repo-local runtime crates built from the current branch before flashing
- A documented first-flash and redeploy workflow for the chosen board or stack

### Command contract

- `./examples/036-esp32-event-agent-sh/examples.sh --mode build-only`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode run`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode host-sim`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode hardware-smoke`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode restart-smoke`
- `./examples/036-esp32-event-agent-sh/examples.sh --mode negative`

### Expected markers

- `MKT:BOOT:OK`
- `MKT:WIFI:OK`
- `MKT:TIME:OK`
- `MKT:TLS:OK`
- `MKT:SESSION:CREATED`
- `MKT:PROVIDER:STREAM_START`
- `MKT:PROVIDER:STREAM_DONE`
- `MKT:RUNTIME:KEEPALIVE`
- `MKT:EVENT:INPUT_ADMITTED`
- `MKT:TOOL:ACK`
- `MKT:TOOL:REQUESTED`
- `MKT:TOOL:COMPLETED`
- `MKT:PERSISTENCE:OK`
- `MKT:SESSION:RESTORED`
- `MKT:SMOKE:PASS`

### Failure markers

- `MKT:WIFI:FAIL`
- `MKT:TIME:FAIL`
- `MKT:TLS:FAIL`
- `MKT:PROVIDER:FAIL`
- `MKT:RUNTIME:FAIL`
- `MKT:EVENT:FAIL`
- `MKT:TOOL:FAIL`
- `MKT:PERSISTENCE:FAIL`
- `MKT:RESTORE:FAIL`
- `MKT:OOM`
- `MKT:SMOKE:FAIL`

### Cleanup

- Reset only the state needed for deterministic reruns after artifacts are captured.
- Preserve the raw serial log, parsed marker report, and restart evidence under the phase artifact path.
- Preserve redeploy notes or OTA evidence when the chosen stack supports it.

### Proof obligations

| Proof ID | Obligation | Owning spec IDs |
| --- | --- | --- |
| EX036-PROOF-001 | The single-node event-agent pattern can create a session through the canonical path and complete one streamed provider turn. | `REQ-010`, `E2E-001`, `CONTRACT-001`, `CONTRACT-002` |
| EX036-PROOF-002 | Host-tool callbacks and fire-and-forget acknowledgement work without blocking the event-agent loop. | `REQ-006`, `E2E-002`, `CONTRACT-004` |
| EX036-PROOF-003 | Persistence and restore work across restart without a shadow session model. | `REQ-008`, `CONTRACT-005`, `E2E-003` |
| EX036-PROOF-004 | Deterministic negative controls prove that failures surface as explicit markers, not silent drift. | `REQ-009`, `E2E-004`, `CHOKE-004` |

## Example 037 - `examples/037-esp32-triangulation-swarm-sh`

### Purpose

The recommended multi-node embedded pattern: flash a small set of ESP32-S3 nodes, let them discover each other, exchange local telemetry such as Wi-Fi signal observations, self-organize into a relative-position estimate when the telemetry supports it, and otherwise fall back to a confidence-scored topology or adjacency artifact that is still useful to an operator.

### Inherited patterns

- 033 pattern: `examples.sh` rebuilds repo-local runtime artifacts before build or flash.
- 034 pattern: the example is the real runtime/surface entrypoint and retains long-running behavior.

### Required directory contract

- `examples/037-esp32-triangulation-swarm-sh/README.md`
- `examples/037-esp32-triangulation-swarm-sh/examples.sh`
- one self-contained runtime application payload owned by the example
- one node inventory format such as `nodes.toml` or equivalent
- one marker parser or smoke helper owned by the example or a shared smoke helper path
- one operator-facing topology report or visualizer generated from the same run data used by the example

### Real entrypoint

- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode run --inventory ./nodes.toml`

### Setup

- A small ESP32-S3 node set on the same Wi-Fi network, ideally at least three nodes
- Stable node identities and serial ports described by an inventory file
- Provider API key and Wi-Fi credentials available to the example
- Repo-local runtime crates built from the current branch before flashing
- A known fixture layout when running the positioning smoke variant
- A merged topology artifact path such as HTML, SVG, JSON report, or equivalent
- A documented threshold or verdict rule that decides between metric-position success and topology-only fallback

### Command contract

- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode build-only`
- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode run --inventory ./nodes.toml`
- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode host-sim --nodes 4`
- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode hardware-smoke --inventory ./nodes.toml`
- `./examples/037-esp32-triangulation-swarm-sh/examples.sh --mode negative --inventory ./nodes.toml`

### Expected markers

- `MKT:BOOT:OK`
- `MKT:SWARM:PEER_DISCOVERED`
- `MKT:SWARM:MESSAGE_OK`
- `MKT:SWARM:TELEMETRY_OK`
- `MKT:SWARM:MODE:METRIC` or `MKT:SWARM:MODE:TOPOLOGY`
- `MKT:SWARM:TOPOLOGY_ESTIMATE`
- `MKT:SWARM:CONVERGED`
- `MKT:SMOKE:PASS`

### Failure markers

- `MKT:SWARM:DISCOVERY_FAIL`
- `MKT:SWARM:MESSAGE_FAIL`
- `MKT:SWARM:TELEMETRY_FAIL`
- `MKT:SWARM:SOLVER_FAIL`
- `MKT:SWARM:DIVERGED`
- `MKT:OOM`
- `MKT:SMOKE:FAIL`

`MKT:SWARM:POSITION_UNSUPPORTED` is not a failure by itself when it is followed by the topology-only fallback path and a passing smoke result.

### Cleanup

- Reset only the swarm state needed for deterministic reruns after artifacts are captured.
- Preserve the node inventory used, the raw per-node logs, the merged marker report, and any fixture-layout comparison output.
- Preserve the operator-facing topology report or visualizer output generated by the run.

### Proof obligations

| Proof ID | Obligation | Owning spec IDs |
| --- | --- | --- |
| EX037-PROOF-001 | A flashed node set can discover peers and exchange telemetry over the chosen comms path. | `REQ-011`, `ASSUMP-010`, `E2E-005` |
| EX037-PROOF-002 | Available instrumentation is sufficient to close the example through either metric relative-position convergence or the predeclared topology-only fallback artifact in the target fixture. | `REQ-011`, `ASSUMP-011`, `E2E-005`, `CHOKE-005` |
| EX037-PROOF-003 | The swarm scenario still routes through the same canonical runtime and tool contracts instead of a special-purpose side channel. | `CONTRACT-001`, `CONTRACT-002`, `CONTRACT-004`, `CONTRACT-006` |
| EX037-PROOF-004 | Deterministic negative controls prove that discovery, telemetry, convergence, or fallback failures surface explicitly. | `REQ-009`, `E2E-004`, `CHOKE-004`, `CHOKE-005` |

## Example artifact requirements

Both examples must retain:

- raw serial logs
- parsed marker summaries
- the exact command line used
- board identity and environment notes
- a proof-to-spec mapping covering every `EX036-PROOF-*` or `EX037-PROOF-*` row
- redeploy notes or OTA evidence for Example 036 when supported by the stack
- topology report or visualizer output for Example 037
- the metric-versus-topology verdict used to close Example 037
