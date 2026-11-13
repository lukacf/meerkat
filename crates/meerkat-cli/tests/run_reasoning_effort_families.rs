//! `rkat run --reasoning-effort` on the three direct provider routes, through
//! the built binary.
//!
//! Each row runs the shipped `rkat` against a scripted server on a loopback
//! socket that answers the OpenAI Responses, Anthropic Messages and Gemini
//! GenerateContent APIs. The realm's own config file points one direct
//! provider backend at it (`base_url`) and gives it a placeholder key, so no
//! request leaves the machine and no key of the host is read. The server
//! records every request, so a row can say which provider field carried the
//! level, and that a refused level never reached the provider.
//!
//! One cell per provider family:
//!
//! - a level the model's catalog row lists completes with a workspace tool
//!   call, a schema and streaming; every provider request carries the level
//!   in the route's own field; the run result reports what was requested
//!   and sent;
//! - a level outside the catalog row, and a model no catalog row is written
//!   for, are refused before any provider request: exit status 2 and one
//!   failure document.
//!
//! The models are picked from the catalog by the test; no model id is part
//! of what the rows pin.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::disallowed_methods,
    clippy::panic
)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use meerkat_core::model_profile::capabilities::EffortLevel;
use meerkat_core::{Provider, ReasoningLevelFacts};
use serde_json::{Value, json};

const DEADLINE: Duration = Duration::from_mins(3);
const REALM: &str = "reasoning-effort-families";
const BINDING: &str = "direct";
/// A model id no catalog row is written for.
const UNCATALOGUED: &str = "fixture-reasoner-1";
const STREAMED_ANSWER: [&str; 2] = ["the-file-", "is-written"];
const STRUCTURED: &str = r#"{"answer":"ok"}"#;
const PATCH: &str =
    "*** Begin Patch\n*** Add File: effect.txt\n+written by the tool\n*** End Patch";

const LADDER: [EffortLevel; 7] = [
    EffortLevel::None,
    EffortLevel::Minimal,
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
    EffortLevel::Xhigh,
    EffortLevel::Max,
];

fn rkat_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        return path.into();
    }
    let mut path = std::env::current_exe().expect("current test executable");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(if cfg!(windows) { "rkat.exe" } else { "rkat" });
    path
}

// ---------------------------------------------------------------------------
// The provider families
// ---------------------------------------------------------------------------

/// One direct provider route the binary can be pointed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    /// OpenAI API, Responses.
    OpenAi,
    /// Anthropic API, Messages.
    Claude,
    /// Google GenAI, GenerateContent.
    Gemini,
}

impl Family {
    fn provider(self) -> Provider {
        match self {
            Self::OpenAi => Provider::OpenAI,
            Self::Claude => Provider::Anthropic,
            Self::Gemini => Provider::Gemini,
        }
    }

    /// The provider as config and reports spell it.
    fn provider_name(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Claude => "anthropic",
            Self::Gemini => "gemini",
        }
    }

    /// The backend kind of the provider's direct route.
    fn backend(self) -> &'static str {
        match self {
            Self::OpenAi => "openai_api",
            Self::Claude => "anthropic_api",
            Self::Gemini => "google_genai",
        }
    }

    fn api(self) -> Api {
        match self {
            Self::OpenAi => Api::Responses,
            Self::Claude => Api::Messages,
            Self::Gemini => Api::GenerateContent,
        }
    }

    /// The route as a refusal and a report name it.
    fn route(self) -> Value {
        json!({"backend": self.backend(), "api": self.api().name()})
    }

    /// The wire path the route writes a level to.
    fn field(self) -> &'static str {
        match self {
            Self::OpenAi => "reasoning.effort",
            Self::Claude => "output_config.effort",
            Self::Gemini => "generationConfig.thinkingConfig.thinkingLevel",
        }
    }

    /// The level a request body carries in the route's own field.
    fn carried_level(self, body: &Value) -> Option<&str> {
        match self {
            Self::OpenAi => body["reasoning"]["effort"].as_str(),
            Self::Claude => body["output_config"]["effort"].as_str(),
            Self::Gemini => body["generationConfig"]["thinkingConfig"]["thinkingLevel"].as_str(),
        }
    }

    /// What the catalog row of `model` states on the provider's direct route.
    fn catalog_levels(self, model: &str) -> Option<Vec<EffortLevel>> {
        let provider = self.provider();
        match meerkat_core::supported_reasoning_levels(
            provider,
            meerkat_core::direct_route_observation(provider).as_ref(),
            meerkat_models::canonical().capabilities_for(provider, model),
        ) {
            ReasoningLevelFacts::Stated { levels, .. } => Some(levels),
            _ => None,
        }
    }

    /// A catalog model of the provider whose row states some levels and not
    /// every level, with those levels as the direct route can express them.
    /// A row that leaves out an active level (`low` or above) is preferred,
    /// so the refused level is one a caller would plausibly ask for; the
    /// provider's default model is looked at first.
    fn catalog_model(self) -> (String, Vec<EffortLevel>) {
        let provider = self.provider();
        let rows: Vec<(String, Vec<EffortLevel>)> = meerkat_models::default_model(provider)
            .into_iter()
            .chain(meerkat_models::allowed_models(provider))
            .filter_map(|model| {
                self.catalog_levels(model)
                    .filter(|levels| levels.len() >= 2 && levels.len() < LADDER.len())
                    .map(|levels| (model.to_string(), levels))
            })
            .collect();
        let leaves_out_an_active_level = |levels: &[EffortLevel]| {
            LADDER
                .iter()
                .any(|level| *level >= EffortLevel::Low && !levels.contains(level))
        };
        rows.iter()
            .find(|(_, levels)| leaves_out_an_active_level(levels))
            .or_else(|| rows.first())
            .cloned()
            .unwrap_or_else(|| panic!("the catalog has a {self:?} model that states levels"))
    }
}

/// A level of `supported`. Not `medium` where the set has another active
/// level: `medium` is what some clients send by themselves, and the rows
/// must tell the session's level from that.
fn supported_level(supported: &[EffortLevel]) -> EffortLevel {
    [EffortLevel::High, EffortLevel::Low]
        .into_iter()
        .find(|level| supported.contains(level))
        .or_else(|| {
            supported
                .iter()
                .copied()
                .find(|level| *level >= EffortLevel::Low)
        })
        .or_else(|| supported.first().copied())
        .expect("the row states levels")
}

/// A level outside `supported`: the lowest active one where there is one.
fn unsupported_level(supported: &[EffortLevel]) -> EffortLevel {
    let outside = |level: &EffortLevel| !supported.contains(level);
    LADDER
        .into_iter()
        .find(|level| *level >= EffortLevel::Low && outside(level))
        .or_else(|| LADDER.into_iter().find(outside))
        .expect("a level outside the set")
}

fn wire(levels: &[EffortLevel]) -> Vec<&'static str> {
    levels.iter().map(|level| level.as_wire_str()).collect()
}

// ---------------------------------------------------------------------------
// The loopback provider: one scripted server for the three provider APIs
// ---------------------------------------------------------------------------

/// A provider API the server answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Api {
    Responses,
    Messages,
    GenerateContent,
}

impl Api {
    /// The API as a route names it.
    fn name(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::Messages => "messages",
            Self::GenerateContent => "generate_content",
        }
    }

    /// The provider API a request path belongs to.
    fn of_path(path: &str) -> Option<Self> {
        if path.ends_with("/responses") {
            Some(Self::Responses)
        } else if path.ends_with("/messages") {
            Some(Self::Messages)
        } else if path.contains(":streamGenerateContent") {
            Some(Self::GenerateContent)
        } else {
            None
        }
    }

    /// The newest item of the conversation a request body carries.
    fn newest_item(self, body: &Value) -> &Value {
        let conversation = match self {
            Self::Responses => &body["input"],
            Self::Messages => &body["messages"],
            Self::GenerateContent => &body["contents"],
        };
        conversation
            .as_array()
            .and_then(|items| items.last())
            .unwrap_or(&Value::Null)
    }

    /// How the API writes a tool result into the conversation.
    fn tool_result_marker(self) -> &'static str {
        match self {
            Self::Responses => "function_call_output",
            Self::Messages => "tool_result",
            Self::GenerateContent => "functionResponse",
        }
    }
}

/// What the server answers one request with.
#[derive(Clone, Copy, Debug)]
enum Reply {
    /// One call of `apply_patch` that adds `effect.txt`.
    ToolCall,
    /// [`STREAMED_ANSWER`], streamed in two parts.
    Text,
    /// [`STRUCTURED`], the reply to a structured-output request.
    Json,
}

/// The scripted model: asked for JSON it answers the structured output; after
/// a tool result it streams its answer; offered `apply_patch` it asks it to
/// add `effect.txt` to the working directory; otherwise it answers.
fn scripted_reply(api: Api, body: &Value) -> Reply {
    let newest = api.newest_item(body).to_string();
    if newest.contains("valid JSON") {
        Reply::Json
    } else if newest.contains(api.tool_result_marker()) {
        Reply::Text
    } else if body["tools"].to_string().contains("apply_patch") {
        Reply::ToolCall
    } else {
        Reply::Text
    }
}

/// `data:` lines, each followed by `separator`.
fn sse(separator: &str, data: &[Value]) -> String {
    let mut body = String::new();
    for value in data {
        body.push_str("data: ");
        body.push_str(&value.to_string());
        body.push_str(separator);
    }
    body
}

/// The text parts a reply streams.
fn parts(reply: Reply) -> Vec<&'static str> {
    match reply {
        Reply::ToolCall => Vec::new(),
        Reply::Text => STREAMED_ANSWER.to_vec(),
        Reply::Json => vec![STRUCTURED],
    }
}

/// One scripted reply in the stream format of `api`.
fn render(api: Api, reply: Reply) -> String {
    let parts = parts(reply);
    let arguments = json!({ "patch": PATCH });
    match api {
        Api::Responses => {
            let usage = json!({"input_tokens": 10, "output_tokens": 3, "total_tokens": 13});
            let mut data: Vec<Value> = parts
                .iter()
                .map(|part| json!({"type": "response.output_text.delta", "delta": part}))
                .collect();
            let output = match reply {
                Reply::ToolCall => json!([{
                    "type": "function_call", "id": "fc_1", "call_id": "call_1",
                    "name": "apply_patch", "arguments": arguments.to_string(),
                    "status": "completed"
                }]),
                Reply::Text | Reply::Json => json!([]),
            };
            data.push(json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1", "status": "completed", "output": output, "usage": usage
                }
            }));
            let mut body = sse("\n\n", &data);
            body.push_str("data: [DONE]\n\n");
            body
        }
        Api::Messages => {
            let mut data = vec![json!({
                "type": "message_start",
                "message": {"usage": {"input_tokens": 40, "output_tokens": 0}}
            })];
            let stop_reason = match reply {
                Reply::ToolCall => {
                    data.push(json!({
                        "type": "content_block_start",
                        "content_block": {
                            "type": "tool_use", "id": "toolu_1", "name": "apply_patch"
                        }
                    }));
                    data.push(json!({
                        "type": "content_block_delta",
                        "delta": {
                            "type": "input_json_delta", "partial_json": arguments.to_string()
                        }
                    }));
                    "tool_use"
                }
                Reply::Text | Reply::Json => {
                    data.push(json!({
                        "type": "content_block_start",
                        "content_block": {"type": "text", "text": ""}
                    }));
                    for part in &parts {
                        data.push(json!({
                            "type": "content_block_delta",
                            "delta": {"type": "text_delta", "text": part}
                        }));
                    }
                    "end_turn"
                }
            };
            data.push(json!({"type": "content_block_stop"}));
            data.push(json!({
                "type": "message_delta",
                "usage": {"output_tokens": 3},
                "delta": {"stop_reason": stop_reason}
            }));
            data.push(json!({"type": "message_stop"}));
            sse("\n", &data)
        }
        Api::GenerateContent => {
            let usage = json!({"promptTokenCount": 12, "candidatesTokenCount": 4});
            let chunk = |part: Value, last: bool| {
                let mut candidate = json!({"content": {"role": "model", "parts": [part]}});
                if last {
                    candidate["finishReason"] = json!("STOP");
                }
                json!({"candidates": [candidate], "usageMetadata": usage})
            };
            let data: Vec<Value> = match reply {
                Reply::ToolCall => vec![chunk(
                    json!({"functionCall": {"name": "apply_patch", "args": arguments}}),
                    true,
                )],
                Reply::Text | Reply::Json => parts
                    .iter()
                    .enumerate()
                    .map(|(index, part)| chunk(json!({"text": part}), index + 1 == parts.len()))
                    .collect(),
            };
            sse("\n", &data)
        }
    }
}

/// One request the server was sent.
#[derive(Clone, Debug)]
struct Recorded {
    /// The request path, without its query.
    path: String,
    /// The JSON body; `null` when the request carried none.
    body: Value,
}

/// Read one whole HTTP request: its path and its JSON body.
fn read_request(socket: &mut TcpStream) -> Option<Recorded> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let target = head.lines().next()?.split_whitespace().nth(1)?;
    let path = target.split('?').next().unwrap_or(target).to_string();
    let content_length = head
        .to_ascii_lowercase()
        .lines()
        .find_map(|line| line.strip_prefix("content-length:").map(str::to_string))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
    let body = serde_json::from_slice(&buffer[header_end..header_end + content_length])
        .unwrap_or(Value::Null);
    Some(Recorded { path, body })
}

/// The loopback provider: its port and every request it was sent, on any
/// path.
struct Loopback {
    port: u16,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Loopback {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback server");
        let port = listener.local_addr().expect("loopback address").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        std::thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(mut socket) = socket else {
                    return;
                };
                let _ = socket.set_read_timeout(Some(Duration::from_secs(30)));
                let Some(request) = read_request(&mut socket) else {
                    continue;
                };
                let response = match Api::of_path(&request.path) {
                    Some(api) => {
                        let body = render(api, scripted_reply(api, &request.body));
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                    None => {
                        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                            .to_string()
                    }
                };
                recorded.lock().unwrap().push(request);
                let _ = socket.write_all(response.as_bytes());
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
        });
        Self { port, requests }
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

// ---------------------------------------------------------------------------
// The realm and the binary
// ---------------------------------------------------------------------------

struct Roots {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    state: PathBuf,
    user: PathBuf,
}

/// Fresh roots whose realm config sends the family's direct route to the
/// loopback server with a placeholder key, and registers a model of the
/// family's provider that no catalog row is written for.
fn roots(family: Family, port: u16) -> Roots {
    let temp = tempfile::TempDir::new().unwrap();
    let home = temp.path().to_path_buf();
    let project = temp.path().join("project");
    let state = temp.path().join("state");
    let user = temp.path().join("user");
    let realm = state.join(REALM);
    for dir in [&project, &state, &user, &project.join(".rkat"), &realm] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let provider = family.provider_name();
    let backend = family.backend();
    let key = format!("{}-{}", "placeholder", "key");
    std::fs::write(
        realm.join("config.toml"),
        format!(
            r#"[models."{UNCATALOGUED}"]
provider = "{provider}"
context_window = 200000
max_output_tokens = 8192

[realm."{REALM}"]
default_binding = "{BINDING}"

[realm."{REALM}".backend.{BINDING}]
provider = "{provider}"
backend_kind = "{backend}"
base_url = "http://127.0.0.1:{port}"

[realm."{REALM}".auth.{BINDING}]
provider = "{provider}"
auth_method = "api_key"
source = {{ kind = "inline_secret", secret = "{key}" }}

[realm."{REALM}".binding.{BINDING}]
backend_profile = "{BINDING}"
auth_profile = "{BINDING}"
"#
        ),
    )
    .unwrap();
    Roots {
        _temp: temp,
        home,
        project,
        state,
        user,
    }
}

struct Finished {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

fn drain(mut stream: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        let _ = sender.send(bytes);
    });
    receiver
}

fn collect(receiver: mpsc::Receiver<Vec<u8>>) -> String {
    String::from_utf8_lossy(
        &receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_default(),
    )
    .into_owned()
}

/// Run `rkat <root flags> <args>` in the family's realm with no provider key
/// in its environment and stdin closed; panics at the deadline. Every proxy
/// variable names a closed loopback port: a request to any address but the
/// loopback server fails at once instead of leaving the machine.
fn rkat(roots: &Roots, args: &[&str]) -> Finished {
    let mut command = Command::new(rkat_binary());
    for name in [
        "RKAT_ANTHROPIC_API_KEY",
        "ANTHROPIC_API_KEY",
        "RKAT_OPENAI_API_KEY",
        "OPENAI_API_KEY",
        "RKAT_GEMINI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "XAI_API_KEY",
        "RKAT_TEST_CLIENT",
        "RUST_LOG",
        "RUST_BACKTRACE",
    ] {
        command.env_remove(name);
    }
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env(name, "http://127.0.0.1:9");
    }
    command
        .current_dir(&roots.project)
        .env("HOME", &roots.home)
        .env("XDG_CONFIG_HOME", roots.home.join("config"))
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .arg("--realm")
        .arg(REALM)
        .arg("--context-root")
        .arg(&roots.project)
        .arg("--state-root")
        .arg(&roots.state)
        .arg("--user-config-root")
        .arg(&roots.user)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn rkat");
    let stdout = drain(child.stdout.take().expect("piped stdout"));
    let stderr = drain(child.stderr.take().expect("piped stderr"));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll rkat") {
            break status;
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "rkat {args:?} did not finish within {DEADLINE:?}; stdout:\n{}\nstderr:\n{}",
                collect(stdout),
                collect(stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Finished {
        status: status.code(),
        stdout: collect(stdout),
        stderr: collect(stderr),
    }
}

/// `rkat run <extra> -m <model> --auth-binding <the realm's binding> <prompt>`.
fn run_turn(roots: &Roots, model: &str, extra: &[&str]) -> Finished {
    let binding = format!("{REALM}:{BINDING}");
    let mut args = vec!["run"];
    args.extend_from_slice(extra);
    args.extend_from_slice(&[
        "-m",
        model,
        "--auth-binding",
        &binding,
        "Write the file, then answer.",
    ]);
    rkat(roots, &args)
}

/// The whole of stdout as exactly one JSON document.
fn only_document(finished: &Finished) -> Value {
    serde_json::from_str(finished.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "stdout is not exactly one JSON document ({error}); stdout:\n{}\nstderr:\n{}",
            finished.stdout, finished.stderr
        )
    })
}

/// The pretty-printed JSON document that ends stdout, and the text before it.
fn text_then_document(finished: &Finished) -> (String, Value) {
    let stdout = finished.stdout.trim_end();
    let start = if stdout.starts_with("{\n") {
        0
    } else {
        stdout
            .rfind("\n{\n")
            .map(|index| index + 1)
            .unwrap_or_else(|| {
                panic!(
                    "stdout ends with no JSON document; stdout:\n{}\nstderr:\n{}",
                    finished.stdout, finished.stderr
                )
            })
    };
    let document = serde_json::from_str(&stdout[start..]).unwrap_or_else(|error| {
        panic!(
            "the last stdout document is not JSON ({error}); stdout:\n{}",
            finished.stdout
        )
    });
    (stdout[..start].to_string(), document)
}

fn assert_ok(finished: &Finished, what: &str) {
    assert_eq!(
        finished.status,
        Some(0),
        "{what} succeeds; stdout:\n{}\nstderr:\n{}",
        finished.stdout,
        finished.stderr
    );
}

/// Every request went to the family's API and named `model`.
fn assert_on_the_route(family: Family, model: &str, requests: &[Recorded]) {
    for request in requests {
        assert_eq!(
            Api::of_path(&request.path),
            Some(family.api()),
            "{family:?}: every request is on the route's API: {}",
            request.path
        );
        if family == Family::Gemini {
            assert!(
                request.path.contains(model),
                "{family:?}: the request names the model: {}",
                request.path
            );
        } else {
            assert_eq!(request.body["model"], model, "{family:?}: {}", request.body);
        }
    }
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// A level the catalog row lists completes with a workspace tool call, a
/// schema and streaming; every provider request carries the level in the
/// route's own field; the run result reports what was requested and sent.
fn supported_run_reports_requested_and_sent(family: Family) {
    let provider = Loopback::start();
    let roots = roots(family, provider.port);
    let (model, supported) = family.catalog_model();
    let level = supported_level(&supported).as_wire_str();
    eprintln!(
        "cell {family:?}: route {} model {model} level {level} (the row lists {:?})",
        family.route(),
        wire(&supported)
    );
    let schema = roots.home.join("schema.json");
    std::fs::write(
        &schema,
        r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#,
    )
    .unwrap();

    let finished = run_turn(
        &roots,
        &model,
        &[
            "--reasoning-effort",
            level,
            "--tools",
            "workspace",
            "--schema",
            schema.to_str().unwrap(),
            "--stream",
            "--output",
            "json",
        ],
    );
    assert_ok(&finished, "a run at a level the catalog row lists");

    let requests = provider.requests();
    eprintln!(
        "cell {family:?}: request paths {:?}",
        requests
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        requests.len(),
        3,
        "{family:?}: the tool call, the answer and the extraction: {requests:#?}"
    );
    assert_on_the_route(family, &model, &requests);
    for request in &requests {
        assert_eq!(
            family.carried_level(&request.body),
            Some(level),
            "{family:?}: every provider request carries the level in the route's field: {}",
            request.body
        );
    }
    assert_eq!(
        std::fs::read_to_string(roots.project.join("effect.txt"))
            .ok()
            .as_deref(),
        Some("written by the tool\n"),
        "{family:?}: the tool call's effect exists in the working directory"
    );

    let (streamed, result) = text_then_document(&finished);
    assert!(
        streamed.contains(&STREAMED_ANSWER.concat()),
        "{family:?}: streamed text precedes the result: {:?}",
        finished.stdout
    );
    assert_eq!(
        result["structured_output"],
        json!({"answer": "ok"}),
        "{family:?}"
    );
    assert_eq!(
        result["reasoning_effort"],
        json!({
            "requested": level,
            "sent": [{
                "level": level,
                "provider": family.provider_name(),
                "model": model,
                "route": family.route(),
                "fields": [family.field()],
                "facts": "catalog",
                "decided_by": "request",
                "requests": 3
            }]
        }),
        "{family:?}: the result reports the requested level and what was sent: {result}"
    );

    // A run without the flag reports nothing about a level. The Responses
    // client of a model that takes reasoning sends an effort of its own;
    // that default is the client's and is left alone here.
    let before = provider.request_count();
    let plain = run_turn(&roots, &model, &["--output", "json"]);
    assert_ok(&plain, "a run without a level");
    let requests = provider.requests();
    assert!(requests.len() > before, "{family:?}");
    assert_on_the_route(family, &model, &requests[before..]);
    if family != Family::OpenAi {
        for request in &requests[before..] {
            assert_eq!(
                family.carried_level(&request.body),
                None,
                "{family:?}: {}",
                request.body
            );
        }
    }
    let (_, result) = text_then_document(&plain);
    assert!(
        result.get("reasoning_effort").is_none(),
        "{family:?}: {result}"
    );
}

/// The refusal a refused run reports, after checking that the run sent
/// nothing, exited 2 and printed one failure document whose typed parts
/// say a reasoning effort level was refused.
fn refusal_of(family: Family, provider: &Loopback, finished: &Finished) -> Value {
    assert_eq!(
        provider.request_count(),
        0,
        "{family:?}: a refused run reaches no provider: {:#?}\nstdout:\n{}\nstderr:\n{}",
        provider.requests(),
        finished.stdout,
        finished.stderr
    );
    assert_eq!(
        finished.status,
        Some(2),
        "{family:?}: a refused level exits 2; stdout:\n{}\nstderr:\n{}",
        finished.stdout,
        finished.stderr
    );
    let document = only_document(finished);
    let object = document
        .as_object()
        .expect("the failure document is an object");
    assert_eq!(
        object.keys().collect::<Vec<_>>(),
        ["error"],
        "{family:?}: one failure document and nothing else: {document}"
    );
    let error = &document["error"];
    assert_eq!(error["code"], "INVALID_PARAMS", "{family:?}: {document}");
    assert_eq!(
        error["details"]["kind"], "reasoning_effort_refused",
        "{family:?}: {document}"
    );
    error["details"]["reasoning_effort_refusal"].clone()
}

/// A level outside the catalog row, and a model no catalog row is written
/// for, are refused before any provider request: exit status 2 and one
/// failure document. No session is created.
fn refusals_exit_2_before_any_provider_request(family: Family) {
    let provider = Loopback::start();
    let roots = roots(family, provider.port);
    let (model, supported) = family.catalog_model();
    let sessions_before = rkat(&roots, &["session", "list"]).stdout;

    // Unsupported: a level the catalog row of the model does not list.
    let level = unsupported_level(&supported).as_wire_str();
    eprintln!(
        "cell {family:?}: route {} model {model} refused level {level} (the row lists {:?}); \
         model without a row {UNCATALOGUED}",
        family.route(),
        wire(&supported)
    );
    let refused = run_turn(
        &roots,
        &model,
        &["--reasoning-effort", level, "--output", "json"],
    );
    let refusal = refusal_of(family, &provider, &refused);
    assert_eq!(refusal["reason"], "unsupported_level", "{family:?}");
    assert_eq!(refusal["requested"], level, "{family:?}");
    assert_eq!(refusal["route"], family.route(), "{family:?}");

    // Unknown: no catalog row is written for the model.
    let level = supported_level(&supported).as_wire_str();
    let refused = run_turn(
        &roots,
        UNCATALOGUED,
        &[
            "--reasoning-effort",
            level,
            "--tools",
            "workspace",
            "--stream",
            "--output",
            "json",
        ],
    );
    let refusal = refusal_of(family, &provider, &refused);
    assert_eq!(refusal["reason"], "supported_levels_unknown", "{family:?}");
    assert_eq!(refusal["requested"], level, "{family:?}");
    assert_eq!(refusal["route"], family.route(), "{family:?}");

    assert_eq!(
        rkat(&roots, &["session", "list"]).stdout,
        sessions_before,
        "{family:?}: a refused run creates no session"
    );
    assert!(
        !roots.project.join("effect.txt").exists(),
        "{family:?}: a refused run calls no tool"
    );

    // Control: the same route answers the same models without the flag, so
    // the recorder would have seen a request of a refused run.
    for model in [model.as_str(), UNCATALOGUED] {
        let before = provider.request_count();
        let control = run_turn(&roots, model, &["--output", "json"]);
        assert_ok(&control, "a control run without the flag");
        let requests = provider.requests();
        assert!(
            requests.len() > before,
            "{family:?}: the control run of {model} reaches the provider"
        );
        assert_on_the_route(family, model, &requests[before..]);
    }
}

#[test]
fn openai_supported_run_reports_requested_and_sent() {
    supported_run_reports_requested_and_sent(Family::OpenAi);
}

#[test]
fn claude_supported_run_reports_requested_and_sent() {
    supported_run_reports_requested_and_sent(Family::Claude);
}

#[test]
fn gemini_supported_run_reports_requested_and_sent() {
    supported_run_reports_requested_and_sent(Family::Gemini);
}

#[test]
fn openai_refusals_exit_2_before_any_provider_request() {
    refusals_exit_2_before_any_provider_request(Family::OpenAi);
}

#[test]
fn claude_refusals_exit_2_before_any_provider_request() {
    refusals_exit_2_before_any_provider_request(Family::Claude);
}

#[test]
fn gemini_refusals_exit_2_before_any_provider_request() {
    refusals_exit_2_before_any_provider_request(Family::Gemini);
}
