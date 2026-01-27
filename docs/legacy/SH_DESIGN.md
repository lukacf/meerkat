# Meerkat Shell Tool Design Specification

**Version:** 1.0
**Status:** Draft
**Author:** Claude + GPT 5.2 collaboration

---

## Overview

The Shell Tool provides command execution capabilities for Meerkat agents using Nushell as the backend. It supports both synchronous and asynchronous (background) execution, with completion notifications delivered via the Meerkat comms system.

### Design Principles

1. **Nushell Backend** - Leverages Nushell's structured output and Rust heritage
2. **Subprocess for v1** - Simpler implementation, allows future embedding
3. **Disabled by Default** - Security-first; requires explicit opt-in
4. **Full Environment Inheritance** - With explicit denylist for secrets
5. **Background Execution** - Long-running commands don't block the agent loop
6. **Comms Integration** - Job completion notifications via existing Router

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Agent Loop                                   │
│  ┌─────────┐    ┌─────────────┐    ┌──────────────┐                │
│  │CallingLlm│───▶│WaitingForOps│───▶│DrainingEvents│◀──┐            │
│  └─────────┘    └─────────────┘    └──────────────┘   │            │
│       ▲                                    │           │            │
│       └────────────────────────────────────┘           │            │
│                                                        │            │
│  Receives job completion events ───────────────────────┘            │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              │ EventPayload (shell_job_completed)
                              │
┌─────────────────────────────┴───────────────────────────────────────┐
│                         Comms Router                                 │
└─────────────────────────────┬───────────────────────────────────────┘
                              │
┌─────────────────────────────┴───────────────────────────────────────┐
│                        JobManager                                    │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │ jobs: HashMap<JobId, BackgroundJob>                             │ │
│  │ - spawn_job(command, timeout) -> JobId                          │ │
│  │ - get_status(JobId) -> Option<BackgroundJob>                    │ │
│  │ - cancel_job(JobId) -> Result<()>                               │ │
│  │ - list_jobs() -> Vec<JobSummary>                                │ │
│  └────────────────────────────────────────────────────────────────┘ │
│                              │                                       │
│                              │ spawns tokio task                     │
│                              ▼                                       │
│  ┌────────────────────────────────────────────────────────────────┐ │
│  │ Background Task                                                 │ │
│  │ - Executes `nu -c <command>` as subprocess                      │ │
│  │ - Captures stdout/stderr                                        │ │
│  │ - On completion: sends EventPayload via Router                  │ │
│  └────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Configuration

### Tool-Level Config

```toml
# .rkat/config.toml

[tools.shell]
# Must be explicitly enabled (default: false)
enabled = false

# Default timeout for commands in seconds (default: 30)
default_timeout_secs = 30

# Restrict working directory to project root (default: true)
restrict_to_project = true

# Shell to use (default: "nu" on all platforms)
# Falls back to /bin/sh if nu not found
shell = "nu"
```

### Global Config

```toml
# ~/.config/rkat/config.toml

[tools.shell]
enabled = true
default_timeout_secs = 60
```

### Config Precedence

1. Per-call parameters (highest)
2. Project config (`.rkat/config.toml`)
3. User config (`~/.config/rkat/config.toml`)
4. Defaults (lowest)

---

## Data Types

### JobId

```rust
/// Unique identifier for background jobs
/// Format: "job_" + ULID (26 chars lowercase)
/// Example: "job_01hx7z8k9m2n3p4q5r6s7t8u9v"
pub struct JobId(pub String);
```

### JobStatus

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum JobStatus {
    /// Job is currently executing
    Running {
        started_at_unix: u64,
    },

    /// Job completed successfully
    Completed {
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        duration_secs: f64,
    },

    /// Job failed to execute (spawn error, etc.)
    Failed {
        error: String,
        duration_secs: f64,
    },

    /// Job exceeded timeout
    TimedOut {
        stdout: String,  // Partial output captured before timeout
        stderr: String,
        duration_secs: f64,
    },

    /// Job was cancelled by user
    Cancelled {
        duration_secs: f64,
    },
}
```

### BackgroundJob

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundJob {
    pub id: JobId,
    pub command: String,
    pub working_dir: Option<String>,
    pub timeout_secs: u64,
    pub status: JobStatus,
}
```

### JobSummary

```rust
/// Lightweight job info for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: JobId,
    pub command: String,
    pub status: String,  // "running", "completed", "failed", "timed_out", "cancelled"
    pub started_at_unix: u64,
}
```

### ShellConfig

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    /// Whether shell tool is enabled (default: false)
    pub enabled: bool,

    /// Default timeout in seconds (default: 30)
    pub default_timeout_secs: u64,

    /// Restrict working_dir to project root (default: true)
    pub restrict_to_project: bool,

    /// Shell executable (default: "nu")
    pub shell: String,

    /// Explicit shell path override (default: None)
    pub shell_path: Option<PathBuf>,

    /// Project root directory
    pub project_root: PathBuf,

    /// Maximum number of completed jobs retained (default: 100)
    pub max_completed_jobs: usize,

    /// Completed job retention window in seconds (default: 300)
    pub completed_job_ttl_secs: u64,

    /// Maximum number of concurrent processes (default: 10)
    pub max_concurrent_processes: usize,
}
```

---

## Tools

### shell

Execute a shell command.

**Input Schema:**

```json
{
  "type": "object",
  "properties": {
    "command": {
      "type": "string",
      "description": "The command to execute (Nushell syntax)"
    },
    "working_dir": {
      "type": "string",
      "description": "Working directory (relative to project root)"
    },
    "timeout_secs": {
      "type": "integer",
      "description": "Timeout in seconds (uses config default if not specified)"
    },
    "background": {
      "type": "boolean",
      "description": "If true, run in background and return job ID immediately",
      "default": false
    }
  },
  "required": ["command"]
}
```

**Synchronous Output:**

```json
{
  "exit_code": 0,
  "stdout": "output here",
  "stderr": "",
  "timed_out": false,
  "duration_secs": 1.234
}
```

**Background Output (immediate return):**

```json
{
  "job_id": "job_01hx7z8k9m2n3p4q5r6s7t8u9v",
  "status": "running",
  "message": "Command 'cargo build' started in background"
}
```

### shell_job_status

Check status of a background job.

**Input Schema:**

```json
{
  "type": "object",
  "properties": {
    "job_id": {
      "type": "string",
      "description": "The job ID to check"
    }
  },
  "required": ["job_id"]
}
```

**Output:**

```json
{
  "id": "job_01hx7z8k9m2n3p4q5r6s7t8u9v",
  "command": "cargo build",
  "working_dir": "/project",
  "timeout_secs": 300,
  "status": {
    "status": "completed",
    "exit_code": 0,
    "stdout": "...",
    "stderr": "",
    "duration_secs": 45.2
  }
}
```

### shell_jobs

List all background jobs.

**Input Schema:**

```json
{
  "type": "object",
  "properties": {}
}
```

**Output:**

```json
[
  {
    "id": "job_01hx7z8k9m2n3p4q5r6s7t8u9v",
    "command": "cargo build",
    "status": "completed",
    "started_at_unix": 1706123456
  },
  {
    "id": "job_01hx7z9abcdefghijklmnopqr",
    "command": "npm test",
    "status": "running",
    "started_at_unix": 1706123500
  }
]
```

### shell_job_cancel

Cancel a running background job.

**Input Schema:**

```json
{
  "type": "object",
  "properties": {
    "job_id": {
      "type": "string",
      "description": "The job ID to cancel"
    }
  },
  "required": ["job_id"]
}
```

**Output (success):**

```json
{
  "job_id": "job_01hx7z8k9m2n3p4q5r6s7t8u9v",
  "status": "cancelled"
}
```

**Output (error):**

```json
{
  "error": "Job not found"
}
```

---

## Environment Handling

### Inheritance Model

The shell tool inherits the **full environment** from the parent process without modification.

### No Default Denylist

There is **no default denylist** for environment variables. The full environment is passed through as-is. Users who require environment filtering can configure it externally before launching Meerkat.

### PWD Override

The `PWD` environment variable is always set to the resolved working directory.

---

## Background Execution

### Flow

1. Agent calls `shell` with `background: true`
2. Tool returns immediately with `job_id` and `status: "running"`
3. JobManager spawns tokio task to execute command
4. Task runs `nu -c <command>` as subprocess
5. On completion (success, failure, timeout, or cancel):
   - JobManager updates job status
   - JobManager sends `EventPayload` via Router
6. Agent receives event in `DrainingEvents` phase
7. Event is formatted as system message for LLM context

### Completion Event

Sent via comms Router as `EventPayload`:

```json
{
  "event_type": "shell_job_completed",
  "data": {
    "job_id": "job_01hx7z8k9m2n3p4q5r6s7t8u9v",
    "command": "cargo build --release",
    "result": {
      "status": "completed",
      "exit_code": 0,
      "stdout": "Compiling...\nFinished release",
      "stderr": "",
      "duration_secs": 45.2
    }
  }
}
```

### Job Lifecycle

```
┌─────────┐     spawn      ┌─────────┐
│ Created │───────────────▶│ Running │
└─────────┘                └────┬────┘
                                │
           ┌────────────────────┼────────────────────┐
           │                    │                    │
           ▼                    ▼                    ▼
    ┌───────────┐        ┌───────────┐        ┌───────────┐
    │ Completed │        │ TimedOut  │        │ Cancelled │
    └───────────┘        └───────────┘        └───────────┘
           │
           ▼
    ┌───────────┐
    │  Failed   │ (spawn error)
    └───────────┘
```

### Cancellation

When `shell_job_cancel` is called:
1. JobManager sends SIGTERM to process group
2. Waits up to 5 seconds for graceful shutdown
3. Sends SIGKILL if process still running
4. Updates status to `Cancelled`
5. Sends completion event

---

## Security Considerations

### Disabled by Default

The shell tool is **disabled by default**. Users must explicitly enable it:

```toml
[tools.shell]
enabled = true
```

### Working Directory Restriction

When `restrict_to_project = true` (default):
- `working_dir` parameter must resolve to a path within project root
- Attempts to escape (e.g., `../../../etc`) return error

### No Sandbox (v1)

Version 1 does **not** include OS-level sandboxing (Seatbelt, Landlock). Security relies on:
- Disabled by default
- Environment denylist
- Working directory restriction
- User awareness

Future versions may add optional sandboxing.

### Command Validation

Minimal validation to catch obvious mistakes:

```rust
const BLOCKED_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf ~",
    "> /dev/sd",
    "dd if=",
];
```

This is **defense in depth**, not a security boundary.

---

## Error Handling

### ShellError

```rust
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("Shell not installed: {0}. Install from https://nushell.sh")]
    ShellNotInstalled(String),

    #[error("Command blocked: contains '{0}'")]
    BlockedCommand(String),

    #[error("Working directory '{0}' is outside project root")]
    WorkingDirEscape(String),

    #[error("Working directory not found: {0}")]
    WorkingDirNotFound(String),

    #[error("Job not found: {0}")]
    JobNotFound(String),

    #[error("Job is not running")]
    JobNotRunning,

    #[error("Background execution not configured")]
    BackgroundNotConfigured,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

### Error Response Format

All tool errors return:

```json
{
  "error": "error_code",
  "message": "Human readable description"
}
```

---

## Agent Loop Integration

### DrainingEvents Phase

The existing `DrainingEvents` phase is extended to receive shell job completion events:

```rust
LoopPhase::DrainingEvents => {
    // Check for incoming events (non-blocking)
    if let Some(ref rx) = comms_rx {
        while let Ok(envelope) = rx.try_recv() {
            if let MessagePayload::Event(event) = envelope.payload {
                if event.event_type == "shell_job_completed" {
                    // Inject as system message
                    let message = Message::system(json!({
                        "type": "background_job_completed",
                        "data": event.data
                    }));
                    state = state.add_message(message);
                }
            }
        }
    }
    state = state.drain_events();
}
```

### Non-Blocking

Job completion events are processed **non-blocking**. If no events are pending, the agent continues immediately.

---

## File Structure

```
meerkat-tools/src/builtin/
├── mod.rs                    # Add shell module exports
├── shell/
│   ├── mod.rs               # Shell module root
│   ├── config.rs            # ShellConfig
│   ├── tool.rs              # ShellTool (BuiltinTool impl)
│   ├── job_manager.rs       # JobManager
│   ├── job_status_tool.rs   # ShellJobStatusTool
│   ├── jobs_list_tool.rs    # ShellJobsListTool
│   ├── job_cancel_tool.rs   # ShellJobCancelTool
│   └── types.rs             # JobId, JobStatus, BackgroundJob, etc.
```

---

## Dependencies

### Required

- `tokio` - Async runtime, process spawning
- `ulid` - Job ID generation
- `serde` / `serde_json` - Serialization
- `thiserror` - Error types
- `tracing` - Logging

### Optional Runtime

- `nu` (Nushell) - Must be installed on system PATH

---

## Testing Strategy

### Unit Tests

- Config parsing and defaults
- Environment denylist filtering
- Job status transitions
- Working directory validation

### Integration Tests

- Synchronous command execution
- Background job spawn and completion
- Job cancellation
- Timeout handling
- Error cases (nu not installed, invalid working dir)

### E2E Tests

- Agent runs shell command and sees output
- Agent runs background job and receives completion event
- Multiple concurrent background jobs
- Job cancellation mid-execution

---

## Future Considerations

### v2: Nushell Embedding

Embed Nushell as a library (`nu-engine`) instead of subprocess:
- Eliminates startup overhead
- No external dependency
- Consistent nu version

### v2: Sandboxing

Optional OS-level sandboxing:
- macOS: Seatbelt via `sandbox-exec`
- Linux: Landlock + seccomp or bubblewrap

### v2: Streaming Output

Stream stdout/stderr incrementally for long-running commands:
- WebSocket or SSE for real-time output
- Useful for build commands, test runs

---

## Changelog

- **v1.0** (Draft): Initial specification
