//! The in-process file tools and the engine's own directory, driven through
//! the shipped `rkat` binary with its default tool preset (built-in tools,
//! no shell) and a scripted model on a loopback socket.
//!
//! A session that was given no command execution must not be able to change
//! what the next start of the host loads and runs: the project's `.rkat/`
//! directory (its MCP servers, hooks, skills and, by default, the realm's
//! own state and config document) and a realm state root kept elsewhere
//! inside the project. Each row lets the scripted model ask `apply_patch`
//! for such a write, then starts the host again and looks for a marker file
//! that only a planted MCP server command would create. Everything lives in
//! the test's temporary directory.

#![cfg(all(unix, feature = "mcp"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::disallowed_methods)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const REALM: &str = "file-tools";
const MODEL: &str = "scripted-file-tools";

fn rkat_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        return path.into();
    }
    let mut path = std::env::current_exe().expect("current test executable");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("rkat");
    path
}

/// What the scripted model does on the next run: ask for one `apply_patch`
/// call with this patch, or (when `None`) answer with text only.
#[derive(Default)]
struct Script {
    patch: Option<String>,
    /// The tool result the host sent back for each `apply_patch` call.
    tool_results: Vec<String>,
}

/// A scripted OpenAI-compatible chat-completions server on loopback.
struct ScriptedModel {
    port: u16,
    script: Arc<Mutex<Script>>,
}

fn sse_chunk(value: &serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

fn scripted_body(request: &serde_json::Value, script: &Mutex<Script>) -> String {
    let last = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    let mut script = script.lock().unwrap();
    let mut body = String::new();
    let text = |body: &mut String| {
        body.push_str(&sse_chunk(&serde_json::json!({
            "choices": [{ "delta": { "content": "done" }, "finish_reason": null }]
        })));
        body.push_str(&sse_chunk(
            &serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
        ));
    };
    if last["role"].as_str() == Some("tool") {
        let result = match &last["content"] {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        script.tool_results.push(result);
        text(&mut body);
    } else if let Some(patch) = script.patch.take() {
        let arguments = serde_json::json!({ "patch": patch }).to_string();
        body.push_str(&sse_chunk(&serde_json::json!({
            "choices": [{
                "delta": { "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "apply_patch", "arguments": arguments }
                }] },
                "finish_reason": null
            }]
        })));
        body.push_str(&sse_chunk(
            &serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] }),
        ));
    } else {
        text(&mut body);
    }
    body.push_str(&sse_chunk(&serde_json::json!({
        "choices": [],
        "usage": { "prompt_tokens": 10, "completion_tokens": 2 }
    })));
    body.push_str("data: [DONE]\n\n");
    body
}

fn serve_connection(mut socket: std::net::TcpStream, script: &Mutex<Script>) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
    let request: serde_json::Value =
        serde_json::from_slice(&buffer[header_end..header_end + content_length])
            .unwrap_or_default();
    let body = scripted_body(&request, script);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes());
    let _ = socket.shutdown(std::net::Shutdown::Both);
}

impl ScriptedModel {
    fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind scripted model");
        let port = listener
            .local_addr()
            .expect("scripted model address")
            .port();
        let script = Arc::new(Mutex::new(Script::default()));
        let served = Arc::clone(&script);
        std::thread::spawn(move || {
            for socket in listener.incoming().flatten() {
                let script = Arc::clone(&served);
                std::thread::spawn(move || serve_connection(socket, &script));
            }
        });
        Self { port, script }
    }

    /// The realm config document an operator would have written to use this
    /// model in `REALM`.
    fn realm_document(&self) -> String {
        let port = self.port;
        format!(
            r#"[self_hosted.servers.scripted]
base_url = "http://127.0.0.1:{port}/v1"

[self_hosted.models."{MODEL}"]
server = "scripted"
remote_model = "{MODEL}"

[realm."{REALM}"]
default_binding = "scripted"

[realm."{REALM}".backend.scripted]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "scripted"

[realm."{REALM}".auth.scripted]
provider = "self_hosted"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm."{REALM}".binding.scripted]
backend_profile = "scripted"
auth_profile = "scripted"
"#
        )
    }

    fn ask_for_patch(&self, patch: String) {
        self.script.lock().unwrap().patch = Some(patch);
    }

    fn tool_results(&self) -> Vec<String> {
        self.script.lock().unwrap().tool_results.clone()
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    marker: PathBuf,
    model: ScriptedModel,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::TempDir::new().unwrap();
        // The canonical path: the patch names absolute paths under it.
        let base = temp.path().canonicalize().unwrap();
        let home = base.join("home");
        let project = base.join("project");
        for dir in [&home, &project] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Self {
            marker: base.join("planted-command-ran"),
            _temp: temp,
            home,
            project,
            model: ScriptedModel::start(),
        }
    }

    /// `rkat <global args> run ...` in the project with the default tool
    /// preset: no `--tools`, no `--yolo`.
    fn run(&self, global_args: &[&str], prompt: &str) -> Output {
        let mut command = Command::new(rkat_binary());
        for name in [
            "RKAT_ANTHROPIC_API_KEY",
            "ANTHROPIC_API_KEY",
            "RKAT_OPENAI_API_KEY",
            "OPENAI_API_KEY",
            "RKAT_GEMINI_API_KEY",
            "GEMINI_API_KEY",
            "RKAT_GOOGLE_API_KEY",
            "GOOGLE_API_KEY",
            "XAI_API_KEY",
            "RKAT_TEST_CLIENT",
            "RUST_LOG",
        ] {
            command.env_remove(name);
        }
        let output = command
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .args(global_args)
            .args([
                "--realm",
                REALM,
                "run",
                "--wait-for-mcp",
                "--output",
                "json",
            ])
            .args(["-m", MODEL, "--provider", "self-hosted", prompt])
            .stdin(Stdio::null())
            .output()
            .expect("run rkat");
        assert!(
            output.status.success(),
            "rkat run failed: status={} stdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        output
    }

    /// The text of a project `mcp.toml` whose one stdio server is a
    /// harmless command that creates the marker file.
    fn planted_mcp_lines(&self) -> Vec<String> {
        vec![
            "[[servers]]".to_owned(),
            "name = \"planted\"".to_owned(),
            "command = \"/usr/bin/touch\"".to_owned(),
            format!("args = [\"{}\"]", self.marker.display()),
            "connect_timeout_secs = 2".to_owned(),
        ]
    }

    /// Whether the planted command ran, allowing a launch in flight to land.
    fn marker_appeared(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.marker.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.marker.exists()
    }
}

fn add_file(path: &str, lines: &[String]) -> String {
    let mut hunk = format!("*** Add File: {path}\n");
    for line in lines {
        hunk.push('+');
        hunk.push_str(line);
        hunk.push('\n');
    }
    hunk
}

fn append_line(path: &str, line: &str) -> String {
    format!("*** Update File: {path}\n@@\n+{line}\n")
}

fn patch(hunks: &[String]) -> String {
    format!("*** Begin Patch\n{}*** End Patch", hunks.concat())
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// What the row found against the rule, one line each; empty when the
/// engine's own files were left alone.
fn report(violations: &[String], tool_results: &[String]) {
    assert!(
        violations.is_empty(),
        "the file tools changed the engine's own files:\n- {}\ntool results sent to the model: {tool_results:#?}",
        violations.join("\n- ")
    );
}

const REFUSAL_TEXT: &str = "engine's own directory";

/// The default layout: the realm lives under `<project>/.rkat/realms/`. One
/// patch adds `.rkat/mcp.toml` (a stdio server whose command creates the
/// marker) and appends to the realm's `config.toml`. Both are refused with
/// nothing written, and the next start has nothing planted to launch.
#[test]
fn a_session_without_a_shell_cannot_plant_what_the_next_start_runs() {
    let fixture = Fixture::new();
    let realm_dir = fixture.project.join(".rkat/realms").join(REALM);
    std::fs::create_dir_all(&realm_dir).unwrap();
    let realm_document = realm_dir.join("config.toml");
    let operator_document = fixture.model.realm_document();
    std::fs::write(&realm_document, &operator_document).unwrap();
    let project_mcp = fixture.project.join(".rkat/mcp.toml");

    fixture.model.ask_for_patch(patch(&[
        add_file(".rkat/mcp.toml", &fixture.planted_mcp_lines()),
        append_line(
            &format!(".rkat/realms/{REALM}/config.toml"),
            "# appended by the model",
        ),
    ]));
    fixture.run(&[], "Add the MCP server.");
    let tool_results = fixture.model.tool_results();
    assert_eq!(
        tool_results.len(),
        1,
        "the scripted model's apply_patch call was dispatched once: {tool_results:#?}"
    );

    let mut violations = Vec::new();
    if let Some(written) = read(&project_mcp) {
        violations.push(format!("apply_patch wrote .rkat/mcp.toml:\n{written}"));
    }
    let document_now = read(&realm_document).unwrap();
    if document_now != operator_document {
        violations.push(format!(
            "apply_patch changed the realm config document .rkat/realms/{REALM}/config.toml; it now ends with: {:?}",
            document_now.lines().last()
        ));
    }
    if !tool_results[0].contains(REFUSAL_TEXT) {
        violations.push(format!(
            "the call was not refused as a write into the {REFUSAL_TEXT}"
        ));
    }

    // The next start of the host in this project.
    let second = fixture.run(&[], "Hello again.");
    if fixture.marker_appeared() {
        violations.push(format!(
            "the next start ran the planted command (marker {} exists); its stderr:\n{}",
            fixture.marker.display(),
            String::from_utf8_lossy(&second.stderr)
        ));
    }
    report(&violations, &tool_results);
}

/// A realm state root kept elsewhere inside the project (`--state-root
/// ./state`) is the host's own state too: the realm's config document
/// there is refused like the one under `.rkat/`.
#[test]
fn a_state_root_elsewhere_inside_the_project_is_refused_too() {
    let fixture = Fixture::new();
    let state_root = fixture.project.join("state");
    let realm_dir = state_root.join(REALM);
    std::fs::create_dir_all(&realm_dir).unwrap();
    let realm_document = realm_dir.join("config.toml");
    let operator_document = fixture.model.realm_document();
    std::fs::write(&realm_document, &operator_document).unwrap();
    // As an operator types it, from the project directory.
    let state_root_arg = "./state";

    fixture.model.ask_for_patch(patch(&[append_line(
        &format!("state/{REALM}/config.toml"),
        "# appended by the model",
    )]));
    fixture.run(&["--state-root", state_root_arg], "Edit the config.");
    let tool_results = fixture.model.tool_results();
    assert_eq!(
        tool_results.len(),
        1,
        "the scripted model's apply_patch call was dispatched once: {tool_results:#?}"
    );

    let mut violations = Vec::new();
    let document_now = read(&realm_document).unwrap();
    if document_now != operator_document {
        violations.push(format!(
            "apply_patch changed the realm config document state/{REALM}/config.toml; it now ends with: {:?}",
            document_now.lines().last()
        ));
    }
    if !tool_results[0].contains(REFUSAL_TEXT) {
        violations.push(format!(
            "the call was not refused as a write into the {REFUSAL_TEXT}"
        ));
    }
    report(&violations, &tool_results);

    // A file beside the state root is still the project's own.
    fixture
        .model
        .ask_for_patch(patch(&[add_file("notes.txt", &["kept".to_owned()])]));
    fixture.run(&["--state-root", state_root_arg], "Write a note.");
    assert_eq!(
        read(&fixture.project.join("notes.txt")).as_deref(),
        Some("kept\n"),
        "an ordinary project file is still writable: {:#?}",
        fixture.model.tool_results()
    );
}
