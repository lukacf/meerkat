//! A stdio MCP server fixture for tests that need a real server process: one
//! that settles on stdin EOF, one that only a SIGKILL ends, and one whose
//! stdout outlives it in a process outside its group.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The fixture: records its pid, answers `initialize`, `tools/list` (one
/// `echo` tool), `tools/call` and `ping`, and exits on stdin EOF. Flags in
/// its first argument, joined by `+`, change that: `stubborn` ignores stdin
/// EOF and SIGTERM until it is killed; `escape` adds a child in a session of
/// its own that keeps the server's stdout open and records its pid next to
/// the server's (`<pid path>.escaped`); `silent` never answers `initialize`;
/// `hang_call` records each `tools/call` it receives (`<pid path>.call`) and
/// never answers it.
const FIXTURE: &str = r#"
import json, os, signal, sys, time

flags, pid_path = set(sys.argv[1].split("+")), sys.argv[2]


def record(path, pid):
    with open(path + ".tmp", "w") as handle:
        handle.write(str(pid))
    os.replace(path + ".tmp", path)


if "stubborn" in flags:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
if "escape" in flags and os.fork() == 0:
    os.setsid()
    record(pid_path + ".escaped", os.getpid())
    while True:
        time.sleep(3600)
record(pid_path, os.getpid())
tool = {"name": "echo", "description": "Echo.", "inputSchema": {"type": "object"}}
while True:
    line = sys.stdin.readline()
    if not line:
        break
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if "id" not in message:
        continue
    method = message.get("method")
    if method == "initialize" and "silent" in flags:
        continue
    if method == "tools/call" and "hang_call" in flags:
        # The call reached the server: record that, then never answer it.
        record(pid_path + ".call", os.getpid())
        continue
    if method == "initialize":
        version = message.get("params", {}).get("protocolVersion", "2025-06-18")
        result = {
            "protocolVersion": version,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "settlement-fixture", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": [tool]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "{\"ok\":true}"}]}
    elif method == "ping":
        result = {}
    else:
        error = {"code": -32601, "message": "unsupported"}
        reply = {"jsonrpc": "2.0", "id": message["id"], "error": error}
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()
        continue
    reply = {"jsonrpc": "2.0", "id": message["id"], "result": result}
    sys.stdout.write(json.dumps(reply) + "\n")
    sys.stdout.flush()
if "stubborn" in flags:
    while True:
        time.sleep(3600)
"#;

/// How a fixture server behaves and ends.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FixtureMode {
    /// Exits on stdin EOF.
    Cooperative,
    /// Only a SIGKILL ends it.
    Stubborn,
    /// Stubborn, and a process outside its group keeps its stdout open.
    Escaping,
    /// Stubborn, and never answers `tools/call`.
    HangCall,
    /// Stubborn, and never answers `initialize`.
    Silent,
    /// Silent and escaping.
    SilentEscaping,
}

impl FixtureMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cooperative => "cooperative",
            Self::Stubborn => "stubborn",
            Self::Escaping => "stubborn+escape",
            Self::HangCall => "stubborn+hang_call",
            Self::Silent => "stubborn+silent",
            Self::SilentEscaping => "stubborn+silent+escape",
        }
    }
}

/// One fixture server's files, in a temporary directory it owns. Dropping it
/// kills every recorded fixture process still running, so a failing test
/// leaves no orphan behind.
pub(crate) struct McpFixture {
    pub(crate) dir: tempfile::TempDir,
    script: PathBuf,
}

impl McpFixture {
    #[allow(clippy::expect_used)]
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("fixture dir");
        let script = dir.path().join("mcp_fixture.py");
        std::fs::write(&script, FIXTURE).expect("write fixture");
        Self { dir, script }
    }

    pub(crate) fn pid_path(&self) -> PathBuf {
        self.dir.path().join("server.pid")
    }

    pub(crate) fn escaped_pid_path(&self) -> PathBuf {
        self.dir.path().join("server.pid.escaped")
    }

    /// Written by a `HangCall` server when a `tools/call` reaches it.
    pub(crate) fn call_marker_path(&self) -> PathBuf {
        self.dir.path().join("server.pid.call")
    }

    /// `(command, args)` running the fixture in `mode`.
    pub(crate) fn command(&self, mode: FixtureMode) -> (String, Vec<String>) {
        (
            "python3".to_string(),
            vec![
                self.script.display().to_string(),
                mode.as_str().to_string(),
                self.pid_path().display().to_string(),
            ],
        )
    }

    /// A stdio server config named `name` running the fixture in `mode`.
    pub(crate) fn server(&self, name: &str, mode: FixtureMode) -> meerkat_core::McpServerConfig {
        let (command, args) = self.command(mode);
        meerkat_core::McpServerConfig::stdio(name, command, args, std::collections::HashMap::new())
    }
}

impl Drop for McpFixture {
    fn drop(&mut self) {
        for path in [self.pid_path(), self.escaped_pid_path()] {
            if let Some(pid) = recorded_pid(&path)
                && matches!(process_state(pid), ProcessState::Live(_))
                && let Some(pid) = i32::try_from(pid)
                    .ok()
                    .and_then(rustix::process::Pid::from_raw)
            {
                // A signal, not a spawned `kill`: every spawn briefly gives
                // its child copies of this test process's descriptors, file
                // locks included, which concurrent tests can trip over.
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessState {
    /// No such process: it exited and was reaped.
    Gone,
    /// Exited, not reaped yet.
    Zombie,
    /// Still running, in this `/proc` state.
    Live(char),
}

pub(crate) fn process_state(pid: u32) -> ProcessState {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return ProcessState::Gone;
    };
    // The state is the first field after the parenthesized command name.
    match stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.trim_start().chars().next())
    {
        Some('Z') => ProcessState::Zombie,
        Some(state) => ProcessState::Live(state),
        None => ProcessState::Gone,
    }
}

pub(crate) fn recorded_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The pid recorded at `path`, once it is, polling until `limit`.
pub(crate) async fn wait_for_recorded_pid(path: &Path, limit: Duration) -> Option<u32> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(pid) = recorded_pid(path) {
            return Some(pid);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

/// A mob definition whose `worker` profile owns one fixture server in `mode`.
#[allow(clippy::expect_used)]
fn worker_definition(fixture: &McpFixture, mode: FixtureMode) -> meerkat_mob::MobDefinition {
    let (command, args) = fixture.command(mode);
    meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "member-mcp-custody-{id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true

[[profiles.worker.tools.mcp_servers]]
name = "settlement"
command = {command:?}
args = {args:?}
"#,
        id = uuid::Uuid::new_v4().simple(),
    ))
    .expect("definition")
}

/// A runtime whose `worker` owns one fixture server in `mode`, composed under
/// `custody`, with one member spawned and its server running. Returns the
/// runtime and the server's pid.
#[allow(clippy::expect_used)]
pub(crate) async fn runtime_with_member_server(
    fixture: &McpFixture,
    mode: FixtureMode,
    custody: &std::sync::Arc<crate::member_mcp_custody::MemberMcpCustody>,
) -> (crate::UnifiedRuntime, u32) {
    let session_path = fixture.dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("sessions");
    let factory = custody.install(meerkat::AgentFactory::new(&session_path).comms(true));
    let service = std::sync::Arc::new(meerkat::build_ephemeral_service(
        factory,
        meerkat::Config::default(),
        4,
    ));
    let spec = crate::MobBootstrapSpec::new(
        worker_definition(fixture, mode),
        meerkat_mob::MobStorage::in_memory(),
        service,
    )
    .with_member_mcp_custody(std::sync::Arc::clone(custody))
    .with_options(crate::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(std::sync::Arc::new(meerkat_client::TestClient::default())),
    });
    let runtime = Box::pin(
        crate::UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: "member-mcp-custody".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(5))
            .drain_timeout(Duration::from_millis(200))
            .build(),
    )
    .await
    .expect("runtime");
    runtime
        .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "worker-1".to_string(),
            Some("You are worker-1.".to_string().into()),
            None,
            None,
        ))
        .await
        .expect("spawn member");
    let pid = wait_for_recorded_pid(&fixture.pid_path(), Duration::from_mins(1))
        .await
        .expect("the member's server started");
    (runtime, pid)
}
