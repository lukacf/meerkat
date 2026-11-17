//! What the in-process file tools of a gateway's sessions do with the
//! gateway's state directory.
//!
//! The rows of `member_file_tools_host_paths.rs` state the paths a gateway
//! declares to its members' writing tools. These rows state three more
//! things about the same declaration:
//!
//! - the reading tools (`blob_load_file`, `view_image`) of a member answer a
//!   path under the state directory with Meerkat's refusal for a host's own
//!   location, and read an ordinary file under the root as before;
//! - a session that a schedule's fire creates on demand, which no mob
//!   builds, has the same file tools on `meta_gateway` (a write and a read
//!   under the state directory are refused, an ordinary file is written),
//!   and is offered no file tool on `rpc_gateway`;
//! - a launch that roots the tools at the state directory itself or inside
//!   it is refused at its start.
//!
//! Each row runs the shipped binary with one member, whose profile has the
//! builtin tools and no shell, on a loopback model that makes the tool
//! calls its turn names, one per request. The model answers both the chat
//! completions form and the responses form of a request; a row whose member
//! calls `view_image` routes its model through the responses form, the one
//! a self-hosted model is offered that tool on. A row compares what became
//! of each call with what the row states: outcomes only, never what a file
//! holds.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

const MODEL: &str = "host-state-test";
/// The tool that writes a file from a patch.
const PATCH_TOOL: &str = "apply_patch";
/// The tool that reads a file into the blob store.
const LOAD_TOOL: &str = "blob_load_file";
/// The tool that writes a file from a blob.
const SAVE_TOOL: &str = "blob_save_file";
/// The tool that reads an image file.
const VIEW_TOOL: &str = "view_image";
/// The tool that stores a schedule.
const SCHEDULE_TOOL: &str = "meerkat_schedule_create";
/// The in-process file tools.
const FILE_TOOLS: [&str; 4] = [PATCH_TOOL, LOAD_TOOL, SAVE_TOOL, VIEW_TOOL];
/// The one member of every row.
const MEMBER: &str = "agent:writer";
/// The line a file added by a session holds.
const LINE: &str = "written by a session";
/// The line the rows' own fixture files hold.
const FIXTURE_LINE: &str = "# fixture line";
/// The words of Meerkat's refusal of a write in a host's own location.
const WRITE_REFUSAL: &str = "the file tools do not write there";
/// The words of Meerkat's refusal of a read in a host's own location.
const READ_REFUSAL: &str = "the file tools do not read there";
/// The leading bytes of a PNG file: what an image-named fixture file holds.
const IMAGE_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01";
/// What stands for an image part of a tool's answer in a row's text.
const IMAGE_MARK: &str = "[image]";
const SECRET: &str = "host-state-signing-key-with-enough-bytes";
const PERSON_SUBJECT: &str = "host-state-person";
const PERSON_EMAIL: &str = "host.state@example.com";
const SETTLE: Duration = Duration::from_mins(2);

/// Poll `condition` until it holds or `deadline` runs out; `false` on
/// timeout.
fn eventually(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if condition() {
            return true;
        }
        if start.elapsed() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// At most the first 240 characters of a tool's answer.
fn clip(text: &str) -> String {
    text.chars().take(240).collect()
}

// ---------------------------------------------------------------------------
// The calls a row makes
// ---------------------------------------------------------------------------

/// One file tool call a session makes, by a path relative to its tools'
/// root.
#[derive(Debug, Clone)]
enum Step {
    /// `apply_patch` adds a new file.
    Add(String),
    /// `blob_load_file` reads a file.
    Load(String),
    /// `view_image` reads an image-named file.
    View(String),
}

impl Step {
    fn add(path: &str) -> Self {
        Self::Add(path.to_owned())
    }

    fn load(path: &str) -> Self {
        Self::Load(path.to_owned())
    }

    fn view(path: &str) -> Self {
        Self::View(path.to_owned())
    }

    fn path(&self) -> &str {
        match self {
            Self::Add(path) | Self::Load(path) | Self::View(path) => path,
        }
    }

    /// The two words that name the call in a turn's text: the model reads
    /// the verb and the path.
    fn words(&self) -> String {
        let verb = match self {
            Self::Add(_) => "ADD-FILE",
            Self::Load(_) => "LOAD-FILE",
            Self::View(_) => "VIEW-IMAGE",
        };
        format!("{verb} {}", self.path())
    }

    /// What became of the call: `"refused"` when the tool answered with
    /// Meerkat's refusal (and a file to add is not there), `"written"` or
    /// `"read"` when the tool did what was asked, and otherwise the
    /// beginning of the tool's answer.
    fn outcome(&self, root: &Path, answer: &str) -> String {
        match self {
            Self::Add(path) => {
                let done = std::fs::read_to_string(root.join(path))
                    .is_ok_and(|text| text == format!("{LINE}\n"));
                match (answer.contains(WRITE_REFUSAL), done) {
                    (true, false) => "refused".to_owned(),
                    (false, true) => "written".to_owned(),
                    _ => format!("unexpected (done: {done}): {}", clip(answer)),
                }
            }
            Self::Load(_) => {
                let loaded = serde_json::from_str::<Value>(answer)
                    .is_ok_and(|value| value["blob_id"].is_string());
                match (answer.contains(READ_REFUSAL), loaded) {
                    (true, false) => "refused".to_owned(),
                    (false, true) => "read".to_owned(),
                    _ => format!("unexpected: {}", clip(answer)),
                }
            }
            Self::View(_) => match (answer.contains(READ_REFUSAL), answer.contains(IMAGE_MARK)) {
                (true, false) => "refused".to_owned(),
                (false, true) => "read".to_owned(),
                _ => format!("unexpected: {}", clip(answer)),
            },
        }
    }
}

/// The text of a turn tagged `tag` that makes `steps`, in order.
fn turn_content(tag: &str, steps: &[Step]) -> String {
    let words: Vec<String> = steps.iter().map(Step::words).collect();
    format!("TURN-{tag} {} now", words.join(" "))
}

/// The rows' table: each call with what the row states becomes of it.
type Expected = [(Step, &'static str)];

/// What became of every call of `expected`, given the tools' answers.
fn outcomes_of(expected: &Expected, root: &Path, answers: &[String]) -> Vec<String> {
    expected
        .iter()
        .zip(answers)
        .map(|((step, _), answer)| step.outcome(root, answer))
        .collect()
}

/// Compare what became of every call with what the row states, all at
/// once, so a failure shows the whole table.
fn assert_outcomes(expected: &Expected, found: &[String]) {
    let stated: Vec<String> = expected
        .iter()
        .map(|(step, outcome)| format!("{}: {outcome}", step.words()))
        .collect();
    let found: Vec<String> = expected
        .iter()
        .zip(found)
        .map(|((step, _), outcome)| format!("{}: {outcome}", step.words()))
        .collect();
    assert_eq!(
        found, stated,
        "what became of each file tool call (found, then stated)"
    );
}

// ---------------------------------------------------------------------------
// Loopback model
// ---------------------------------------------------------------------------

/// The messages of a request: `messages` of the chat completions form,
/// `input` of the responses form.
fn items(request: &Value) -> &[Value] {
    request["messages"]
        .as_array()
        .or_else(|| request["input"].as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// The text of one message or tool answer; an image part reads as
/// [`IMAGE_MARK`].
fn item_text(item: &Value) -> String {
    let body = if item["content"].is_null() {
        &item["output"]
    } else {
        &item["content"]
    };
    match body {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match part["text"].as_str() {
                Some(text) => Some(text.to_owned()),
                None => part["type"]
                    .as_str()
                    .is_some_and(|kind| kind.contains("image"))
                    .then(|| IMAGE_MARK.to_owned()),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Whether `item` is the answer of a tool call.
fn is_answer(item: &Value) -> bool {
    item["role"] == "tool" || item["type"] == "function_call_output"
}

/// The turn a request belongs to.
struct Turn {
    /// The tag of the newest `TURN-<tag>` a user message of the request
    /// carries.
    tag: String,
    /// The calls that message names: a verb and its argument each.
    script: Vec<(String, String)>,
    /// The answers of the turn's earlier calls, which follow that message.
    answers: Vec<String>,
}

fn turn_of(request: &Value) -> Option<Turn> {
    let items = items(request);
    let at = items
        .iter()
        .rposition(|item| item["role"] == "user" && item_text(item).contains("TURN-"))?;
    let text = item_text(&items[at]);
    let mut words = text[text.rfind("TURN-")?..].split_whitespace();
    let tag: String = words.next()?["TURN-".len()..]
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    if tag.is_empty() {
        return None;
    }
    let mut script = Vec::new();
    while let (Some(verb), Some(argument)) = (words.next(), words.next()) {
        if verb == "now" {
            break;
        }
        script.push((verb.to_owned(), argument.to_owned()));
    }
    let answers = items[at + 1..]
        .iter()
        .filter(|item| is_answer(item))
        .map(item_text)
        .collect();
    Some(Turn {
        tag,
        script,
        answers,
    })
}

/// The tool call a turn makes next, if any. `MAKE-SCHEDULE` stores a
/// schedule whose target is a session created on demand on the rows' model;
/// its argument is the turn that session runs, with `+` for each space.
fn next_call(turn: &Turn) -> Option<(&'static str, Value)> {
    let (verb, argument) = turn.script.get(turn.answers.len())?;
    match verb.as_str() {
        "ADD-FILE" => Some((
            PATCH_TOOL,
            json!({ "patch": format!(
                "*** Begin Patch\n*** Add File: {argument}\n+{LINE}\n*** End Patch"
            ) }),
        )),
        "LOAD-FILE" => Some((LOAD_TOOL, json!({ "path": argument }))),
        "VIEW-IMAGE" => Some((VIEW_TOOL, json!({ "path": argument }))),
        "MAKE-SCHEDULE" => Some((
            SCHEDULE_TOOL,
            json!({
                "name": "a session created on demand",
                "trigger": {
                    "type": "interval",
                    "start_at_utc": chrono::Utc::now().to_rfc3339(),
                    "every_seconds": 2
                },
                "target": {
                    "target_kind": "session",
                    "type": "materialize_on_demand_session",
                    "create": { "model": MODEL, "provider": "self_hosted" },
                    "action": {
                        "type": "prompt",
                        "prompt": format!("TURN-{} now", argument.replace('+', " "))
                    }
                },
                "missing_target_policy": "mark_misfired",
                "planning_horizon_occurrences": 1
            }),
        )),
        _ => None,
    }
}

/// The names of the tools a request offers the model, sorted.
fn offered_tools(request: &Value) -> Vec<String> {
    let mut names: Vec<String> = request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    tool["function"]["name"]
                        .as_str()
                        .or_else(|| tool["name"].as_str())
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// A streamed chat completions answer that is the text `text` and ends the
/// turn.
fn chat_text(text: &str) -> String {
    let delta = json!({ "choices": [{ "delta": { "content": text } }] });
    let done = json!({
        "choices": [{ "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 1 }
    });
    format!("data: {delta}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

/// A streamed chat completions answer that is one call of `tool`.
fn chat_call(tool: &str, arguments: &Value, id: &str) -> String {
    let call = json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": id,
                    "type": "function",
                    "function": { "name": tool, "arguments": arguments.to_string() }
                }]
            }
        }]
    });
    let done = json!({
        "choices": [{ "finish_reason": "tool_calls" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 1 }
    });
    format!("data: {call}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

/// A streamed responses answer whose one output item is `output`.
fn responses_reply(output: &Value) -> String {
    let usage = json!({ "input_tokens": 10, "output_tokens": 1 });
    let completed = json!({
        "type": "response.completed",
        "response": { "status": "completed", "output": [output], "usage": usage }
    });
    let done = json!({
        "type": "response.done",
        "response": { "status": "completed", "output": [], "usage": usage }
    });
    format!("data: {completed}\n\ndata: {done}\n\n")
}

/// A turn tagged `TURN-<tag>` makes the tool calls it names, one per
/// request, and ends with `ANSWER-<tag>` when the last one's answer comes
/// back. Any other request, and a turn whose next tool is not offered, is
/// answered with text. The answer has the form of the request.
async fn complete(
    axum::extract::State(requests): axum::extract::State<Arc<Mutex<Vec<Value>>>>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    let responses_form = request.get("input").is_some();
    let turn = turn_of(&request);
    let offered = offered_tools(&request);
    let call = turn
        .as_ref()
        .and_then(next_call)
        .filter(|(tool, _)| offered.iter().any(|name| name.as_str() == *tool));
    let tag = turn
        .as_ref()
        .map(|turn| turn.tag.clone())
        .unwrap_or_default();
    let step = turn.as_ref().map_or(0, |turn| turn.answers.len());
    requests.lock().unwrap().push(request);
    let id = format!("call-{tag}-{step}");
    let text = format!("ANSWER-{tag}");
    let payload = match (call, responses_form) {
        (Some((tool, arguments)), false) => chat_call(tool, &arguments, &id),
        (Some((tool, arguments)), true) => responses_reply(&json!({
            "type": "function_call",
            "id": format!("fc-{id}"),
            "call_id": id,
            "name": tool,
            "arguments": arguments.to_string(),
            "status": "completed"
        })),
        (None, false) => chat_text(&text),
        (None, true) => responses_reply(&json!({
            "type": "message",
            "content": [{ "type": "output_text", "text": text }]
        })),
    };
    ([("content-type", "text/event-stream")], payload)
}

async fn models() -> impl axum::response::IntoResponse {
    axum::Json(json!({ "data": [] }))
}

/// The loopback model and every request it answered.
struct Model {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Model {
    fn start() -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::clone(&requests);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("model runtime");
            runtime.block_on(async move {
                let app = axum::Router::new()
                    .route("/v1/chat/completions", axum::routing::post(complete))
                    .route("/chat/completions", axum::routing::post(complete))
                    .route("/v1/responses", axum::routing::post(complete))
                    .route("/responses", axum::routing::post(complete))
                    .route("/v1/models", axum::routing::get(models))
                    .route("/models", axum::routing::get(models))
                    .with_state(state);
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind the loopback model");
                tx.send(listener.local_addr().expect("model address"))
                    .expect("report the model address");
                axum::serve(listener, app)
                    .await
                    .expect("serve the loopback model");
            });
        });
        let address = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("loopback model address");
        Self {
            base_url: format!("http://{address}"),
            requests,
        }
    }

    /// The answers of the tool calls of the turn `tag`, in order, as the
    /// model received them: those of the request of that turn that holds
    /// the most.
    fn answers(&self, tag: &str) -> Vec<String> {
        let mut most = Vec::new();
        for request in self.requests.lock().unwrap().iter() {
            if let Some(turn) = turn_of(request)
                && turn.tag == tag
                && turn.answers.len() > most.len()
            {
                most = turn.answers;
            }
        }
        most
    }

    /// The answers of the first `count` tool calls of the turn `tag`, or
    /// `None` when they did not come back within the settle window.
    fn settled_answers(&self, tag: &str, count: usize) -> Option<Vec<String>> {
        eventually(SETTLE, || self.answers(tag).len() >= count);
        let mut answers = self.answers(tag);
        (answers.len() >= count).then(|| {
            answers.truncate(count);
            answers
        })
    }

    /// The first request of the turn `tag`: the tools it offers and its
    /// user messages.
    fn first_request(&self, tag: &str) -> Option<(Vec<String>, String)> {
        self.requests.lock().unwrap().iter().find_map(|request| {
            let turn = turn_of(request)?;
            (turn.tag == tag).then(|| {
                let users: Vec<String> = items(request)
                    .iter()
                    .filter(|item| item["role"] == "user")
                    .map(item_text)
                    .collect();
                (offered_tools(request), users.join("\n"))
            })
        })
    }

    /// What the model saw of the turn `tag`, for a failed wait.
    fn seen(&self, tag: &str) -> String {
        let count = self.requests.lock().unwrap().len();
        format!(
            "{count} model requests; the first of the turn offers {:?}; {} answers came back",
            self.first_request(tag).map(|(offered, _)| offered),
            self.answers(tag).len()
        )
    }
}

// ---------------------------------------------------------------------------
// Configuration files
// ---------------------------------------------------------------------------

/// A mob with one turn-driven profile on the loopback model: `writer` has
/// the builtin tools and no shell; with `schedule`, the schedule tools too.
fn mob_toml(mob_id: &str, schedule: bool) -> String {
    let schedule = if schedule { "schedule = true\n" } else { "" };
    format!(
        "[mob]\nid = \"{mob_id}\"\n\n\
         [profiles.writer]\nmodel = \"{MODEL}\"\nprovider = \"self_hosted\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.writer.tools]\ncomms = true\nbuiltins = true\n{schedule}"
    )
}

/// The configuration tables that route the sessions of a gateway with the
/// mob `mob_id` to the loopback model at `model_url`; with `responses`,
/// through the responses form, with image tool results.
fn model_tables(mob_id: &str, model_url: &str, responses: bool) -> String {
    let (style, images) = if responses {
        (
            "api_style = \"responses\"\n",
            "vision = true\nimage_tool_results = true\n",
        )
    } else {
        ("", "")
    };
    let mut tables = format!(
        "[self_hosted]\ndefault_model = \"{MODEL}\"\n\n[self_hosted.servers.loop]\n\
         base_url = \"{model_url}\"\n{style}\n[self_hosted.models.\"{MODEL}\"]\nserver = \"loop\"\n\
         remote_model = \"{MODEL}\"\ndisplay_name = \"Host state test model\"\n\
         family = \"{MODEL}\"\n{images}"
    );
    for realm in ["global".to_string(), format!("mob.{mob_id}")] {
        tables.push_str(&format!(
            "\n[realm.\"{realm}\"]\ndefault_binding = \"loop\"\n\n\
             [realm.\"{realm}\".backend.loop]\nprovider = \"self_hosted\"\n\
             backend_kind = \"self_hosted\"\n\n[realm.\"{realm}\".auth.loop]\n\
             provider = \"self_hosted\"\nauth_method = \"none\"\n\
             source = {{ kind = \"platform_default\" }}\n\n\
             [realm.\"{realm}\".binding.loop]\nbackend_profile = \"loop\"\n\
             auth_profile = \"loop\"\ndefault_model = \"{MODEL}\"\nprovider_default = true\n"
        ));
    }
    tables
}

fn write_bytes(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("a parent directory"))
        .expect("create the directory");
    std::fs::write(path, bytes).expect("write the file");
}

fn write_file(path: &Path, text: &str) {
    write_bytes(path, text.as_bytes());
}

fn remove_provider_keys(command: &mut Command) {
    // The sessions' model is the loopback one: no provider key reaches the
    // gateway, so nothing can call a real provider.
    for name in [
        "RKAT_ANTHROPIC_API_KEY",
        "ANTHROPIC_API_KEY",
        "RKAT_OPENAI_API_KEY",
        "OPENAI_API_KEY",
        "RKAT_GEMINI_API_KEY",
        "GEMINI_API_KEY",
        "RKAT_SELF_HOSTED_API_KEY",
    ] {
        command.env_remove(name);
    }
}

/// Collect a child's stderr lines.
fn collect_stderr(stderr: std::process::ChildStderr) -> Arc<Mutex<Vec<String>>> {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            let Ok(mut sink) = sink.lock() else { break };
            if sink.len() < 20_000 {
                sink.push(line);
            }
        }
    });
    lines
}

fn stderr_tail(lines: &Mutex<Vec<String>>) -> String {
    let Ok(lines) = lines.lock() else {
        return "[stderr unavailable]".to_string();
    };
    let start = lines.len().saturating_sub(80);
    lines[start..].join("\n")
}

// ---------------------------------------------------------------------------
// meta_gateway
// ---------------------------------------------------------------------------

/// A console token of the one person the rows sign in as.
fn person_token() -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({
            "iss": "http://127.0.0.1/meta-gateway",
            "aud": "persistent-gateway",
            "sub": PERSON_SUBJECT,
            "email": PERSON_EMAIL,
            "exp": chrono::Utc::now().timestamp() + 3600,
        }),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("sign the console token")
}

/// One `meta_gateway` over a workspace, its console signed in as one
/// person, with the member [`MEMBER`].
struct HttpGateway {
    child: Child,
    _stdin: ChildStdin,
    stderr: Arc<Mutex<Vec<String>>>,
    http_base_url: String,
    bearer: String,
}

impl HttpGateway {
    /// Start the gateway on `workspace` (its `config/mob.toml` is written
    /// by the row). A launch that names no root gets the workspace as its
    /// project root and `<workspace>/state` as its state directory.
    /// `launch` holds the init parameters the row adds or replaces.
    fn start(workspace: &Path, launch: &Value) -> Self {
        let (mut gateway, response) = Self::init(workspace, launch);
        assert!(
            response.get("error").is_none(),
            "init failed: {response}\n{}",
            stderr_tail(&gateway.stderr)
        );
        assert_eq!(response["result"]["launch_state"], "created", "{response}");
        gateway.http_base_url = response["result"]["http_base_url"]
            .as_str()
            .expect("HTTP base URL")
            .trim_end_matches('/')
            .to_owned();
        gateway
    }

    /// The message of the refusal a launch with `launch` gets at its init.
    /// The row fails when the gateway starts.
    fn refused_start(workspace: &Path, launch: &Value) -> String {
        let (gateway, response) = Self::init(workspace, launch);
        response["error"]["message"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "the start is refused: {response}\n{}",
                    stderr_tail(&gateway.stderr)
                )
            })
            .to_owned()
    }

    /// Spawn the gateway on `workspace`, send its init with `launch` and
    /// return the init's answer.
    fn init(workspace: &Path, launch: &Value) -> (Self, Value) {
        let mut params = json!({
            "workspace_root": workspace,
            "persistent_sessions": true,
            "identity_roster": [
                { "identity": MEMBER, "profile": "writer" },
            ],
            "auth_config": {
                "provider": "jwt",
                "shared_secret": SECRET,
                "accounts": "personal",
                "email_allowlist": [PERSON_EMAIL],
            },
        });
        for (key, value) in launch.as_object().expect("launch parameters") {
            params[key] = value.clone();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_meta_gateway"));
        command
            .current_dir(workspace)
            .env("XDG_STATE_HOME", workspace.join("xdg-state"))
            .env("XDG_CONFIG_HOME", workspace.join("xdg-config"))
            .env("RUST_LOG", "info")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        remove_provider_keys(&mut command);
        let mut child = command.spawn().expect("spawn meta_gateway");
        let mut stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let stderr = collect_stderr(child.stderr.take().expect("gateway stderr"));
        writeln!(
            stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "id": 1, "method": "meta/init", "params": params })
        )
        .expect("write init");
        stdin.flush().expect("flush init");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let gateway = Self {
            child,
            _stdin: stdin,
            stderr,
            http_base_url: String::new(),
            bearer: person_token(),
        };
        let line = rx
            .recv_timeout(Duration::from_mins(3))
            .unwrap_or_else(|_| panic!("no init answer\n{}", stderr_tail(&gateway.stderr)));
        let response: Value = serde_json::from_str(&line)
            .unwrap_or_else(|_| panic!("init answer {line:?}\n{}", stderr_tail(&gateway.stderr)));
        (gateway, response)
    }

    fn client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_mins(1))
            .build()
            .expect("HTTP client")
    }

    /// `POST {base}{path}` as the signed-in person: the status and the JSON
    /// body.
    fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let response = Self::client()
            .post(format!("{}{path}", self.http_base_url))
            .bearer_auth(&self.bearer)
            .json(body)
            .send()
            .expect("HTTP request");
        let status = response.status().as_u16();
        (status, response.json().unwrap_or(Value::Null))
    }

    /// Whether the console timeline of [`MEMBER`] holds the completed
    /// interaction of the turn `tag`.
    fn answered(&self, tag: &str) -> bool {
        let (status, answer) = self.post(
            "/console/rpc",
            &json!({
                "jsonrpc": "2.0", "id": 2, "method": "meta/console/query_timeline",
                "params": { "identity": MEMBER, "limit": 500 }
            }),
        );
        let answer_text = format!("ANSWER-{tag}");
        status == 200
            && answer["result"]["frames"].as_array().is_some_and(|frames| {
                frames.iter().any(|frame| {
                    frame["kind"] == "interaction_complete"
                        && frame["payload"].to_string().contains(&answer_text)
                })
            })
    }

    /// One console request to [`MEMBER`] with the text `content`, waited
    /// until it is answered: the answers of its `calls` tool calls as the
    /// model received them.
    fn turn(&self, model: &Model, tag: &str, content: &str, calls: usize) -> Vec<String> {
        let (status, sent) = self.post(
            "/console/send",
            &json!({
                "identity": MEMBER,
                "content": content,
                "origin": "console",
                "idempotency_key": format!("send-{tag}"),
                "handling_mode": "queue",
            }),
        );
        assert_eq!(
            status,
            200,
            "the request is accepted: {sent}\n{}",
            stderr_tail(&self.stderr)
        );
        let answers = model.settled_answers(tag, calls).unwrap_or_else(|| {
            panic!(
                "the turn {tag} returned too few tool answers to the model: {}\n{}",
                model.seen(tag),
                stderr_tail(&self.stderr)
            )
        });
        assert!(
            eventually(SETTLE, || self.answered(tag)),
            "the turn {tag} is answered\n{}",
            stderr_tail(&self.stderr)
        );
        answers
    }

    /// Make every call of `expected`, one turn each: what became of each.
    fn outcomes(&self, model: &Model, root: &Path, expected: &Expected) -> Vec<String> {
        expected
            .iter()
            .enumerate()
            .map(|(index, (step, _))| {
                let tag = format!("t{index}");
                let content = turn_content(&tag, std::slice::from_ref(step));
                step.outcome(root, &self.turn(model, &tag, &content, 1)[0])
            })
            .collect()
    }
}

impl Drop for HttpGateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            eprintln!("meta_gateway stderr:\n{}", stderr_tail(&self.stderr));
        }
    }
}

/// A workspace of `meta_gateway` rows with its mob definition and its
/// model tables written.
struct Workspace {
    dir: TempDir,
    model: Model,
}

impl Workspace {
    /// A workspace whose mob is `mob_id`; `schedule` and `responses` as in
    /// [`mob_toml`] and [`model_tables`]. The model tables are the
    /// workspace configuration file the gateway adopts when its launch
    /// names none.
    fn new(mob_id: &str, schedule: bool, responses: bool) -> Self {
        let dir = TempDir::new().expect("workspace");
        let model = Model::start();
        write_file(
            &dir.path().join("config").join("mob.toml"),
            &mob_toml(mob_id, schedule),
        );
        write_file(
            &dir.path().join(".rkat").join("config.toml"),
            &model_tables(mob_id, &model.base_url, responses),
        );
        Self { dir, model }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// A launch that names no root: the state directory lies under the
/// members' file tools' root. A member's `blob_load_file` does not read a
/// file in it and its `view_image` does not read an image-named file in
/// it; both read a file beside it.
#[test]
fn meta_gateway_members_reading_tools_do_not_read_the_state_directory() {
    let workspace = Workspace::new("host-state-http-reads", false, true);
    let gateway = HttpGateway::start(workspace.path(), &json!({}));
    let state = workspace.path().join("state");
    write_file(
        &state.join("fixture-note.txt"),
        &format!("{FIXTURE_LINE}\n"),
    );
    write_bytes(&state.join("chart.png"), IMAGE_BYTES);
    write_bytes(
        &workspace.path().join("notes").join("chart.png"),
        IMAGE_BYTES,
    );
    let expected = [
        (Step::add("notes/plan.md"), "written"),
        (Step::load("notes/plan.md"), "read"),
        (Step::view("notes/chart.png"), "read"),
        (Step::load("state/fixture-note.txt"), "refused"),
        (Step::view("state/chart.png"), "refused"),
    ];
    let found = gateway.outcomes(&workspace.model, workspace.path(), &expected);
    assert_outcomes(&expected, &found);
}

/// A member whose profile has the schedule tools stores a schedule whose
/// target is a session created on demand. The session its fire creates,
/// which no mob builds, has the gateway's file tools, and the gateway's
/// declaration holds for them: an `apply_patch` and a `blob_load_file`
/// under the state directory are refused, and an ordinary file under the
/// project root is written.
#[test]
fn meta_gateway_file_tools_of_a_session_a_schedule_creates_keep_out_of_the_state_directory() {
    let workspace = Workspace::new("host-state-http-fired", true, false);
    let gateway = HttpGateway::start(workspace.path(), &json!({}));
    write_file(
        &workspace.path().join("state").join("fixture-note.txt"),
        &format!("{FIXTURE_LINE}\n"),
    );
    let expected = [
        (Step::add("state/fired-note.txt"), "refused"),
        (Step::load("state/fixture-note.txt"), "refused"),
        (Step::add("notes/fired-plan.md"), "written"),
    ];
    let fired_turn = turn_content("f0", &expected.clone().map(|(step, _)| step));
    let fired_script = fired_turn["TURN-".len()..fired_turn.len() - " now".len()].replace(' ', "+");
    let stored = gateway.turn(
        &workspace.model,
        "s0",
        &format!("TURN-s0 MAKE-SCHEDULE {fired_script} now"),
        1,
    );
    assert!(
        stored[0].contains("schedule_id") || stored[0].contains("\"id\""),
        "the schedule is stored: {}",
        clip(&stored[0])
    );
    let answers = workspace
        .model
        .settled_answers("f0", expected.len())
        .unwrap_or_else(|| {
            panic!(
                "the session the fire creates returned too few tool answers to the model: {}\n{}",
                workspace.model.seen("f0"),
                stderr_tail(&gateway.stderr)
            )
        });
    let (offered, users) = workspace
        .model
        .first_request("f0")
        .expect("the first request of the session the fire creates");
    println!(
        "the session the fire creates is offered: {}",
        offered.join(", ")
    );
    assert!(
        !users.contains("TURN-s0"),
        "the fire's session is not the member's: its user messages do not hold the member's turn"
    );
    let found = outcomes_of(&expected, workspace.path(), &answers);
    assert_outcomes(&expected, &found);
}

/// A launch whose project root is the state directory itself, and one
/// whose project root lies inside the state directory, is refused at its
/// start: the members' file tools are rooted at the project root, the
/// declared state directory would contain that root and not be applied,
/// and the gateway starts only where the two are apart. The refusal names
/// the two parameters to change.
#[test]
fn meta_gateway_refuses_its_tools_root_at_or_inside_the_state_directory() {
    for inside in [false, true] {
        let workspace = Workspace::new("host-state-http-root", false, false);
        let state = workspace.path().join("state");
        let root = if inside { state.join("work") } else { state };
        std::fs::create_dir_all(&root).expect("create the project root");
        let message =
            HttpGateway::refused_start(workspace.path(), &json!({ "project_root": root }));
        assert!(
            message.contains("project_root") && message.contains("store_path"),
            "the refusal names the two parameters to change (inside: {inside}): {message}"
        );
    }
}

// ---------------------------------------------------------------------------
// rpc_gateway
// ---------------------------------------------------------------------------

/// One `rpc_gateway --persistent` with the member [`MEMBER`], driven over
/// its stdio.
struct StdioGateway {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
    next_id: u64,
}

impl StdioGateway {
    /// Start the gateway and initialize it with `mob_config`, a roster
    /// provider and `runtime_options`, over the state directory `state`.
    /// The process keeps its temporary files under `temp`.
    fn boot(mob_config: &str, state: &Path, temp: &Path, runtime_options: &Value) -> Self {
        std::fs::create_dir_all(temp).expect("create the temporary directory");
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            .env("TMPDIR", temp)
            .env("RUST_LOG", "info")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        remove_provider_keys(&mut command);
        let mut child = command.spawn().expect("spawn rpc_gateway --persistent");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let stderr = collect_stderr(child.stderr.take().expect("gateway stderr"));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut gateway = Self {
            child,
            stdin,
            lines: rx,
            stderr,
            next_id: 0,
        };
        let response = gateway.request(
            "meta/init",
            json!({
                "mob_config": mob_config,
                "has_roster_provider": true,
                "runtime_options": runtime_options,
                "persistent_state": state,
            }),
            Duration::from_mins(3),
        );
        assert!(
            response.get("error").is_none(),
            "init failed: {response}\n{}",
            stderr_tail(&gateway.stderr)
        );
        gateway
    }

    fn send(&mut self, value: &Value) {
        writeln!(self.stdin, "{value}").expect("write request");
        self.stdin.flush().expect("flush request");
    }

    /// The answer to one request; host callbacks that arrive meanwhile are
    /// answered.
    fn request(&mut self, method: &str, params: Value, deadline: Duration) -> Value {
        self.next_id += 1;
        let id = format!("row-{}", self.next_id);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let start = Instant::now();
        loop {
            let line = deadline
                .checked_sub(start.elapsed())
                .and_then(|remaining| self.lines.recv_timeout(remaining).ok())
                .unwrap_or_else(|| {
                    panic!(
                        "no response to {method} within {deadline:?}\n{}",
                        stderr_tail(&self.stderr)
                    )
                });
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("method").is_none()
                && message.get("id").and_then(Value::as_str) == Some(id.as_str())
            {
                return message;
            }
            self.answer_callback(&message);
        }
    }

    fn answer_callback(&mut self, message: &Value) {
        let (Some(method), Some(id)) = (
            message.get("method").and_then(Value::as_str),
            message.get("id").cloned(),
        ) else {
            return;
        };
        match method {
            "callback/roster_provider/roster" => {
                let roster = json!([{
                    "identity": MEMBER,
                    "profile": "writer",
                    "addressability": "addressable",
                    "display_name": null,
                    "labels": {},
                    "context": null,
                    "additional_instructions": []
                }]);
                self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": roster }));
            }
            other if other.starts_with("callback/") => {
                panic!("unexpected host callback {other}: the row provides only a roster")
            }
            _ => {}
        }
    }

    /// One tracked turn of [`MEMBER`] with the text `content`, waited until
    /// it completes: the answers of its `calls` tool calls as the model
    /// received them.
    fn turn(&mut self, model: &Model, tag: &str, content: &str, calls: usize) -> Vec<String> {
        let sent = self.request(
            "meta/send",
            json!({ "identity": MEMBER, "content": content, "track_turn": true }),
            Duration::from_mins(1),
        );
        let ticket = sent["result"]["turn"]["ticket"]
            .as_str()
            .unwrap_or_else(|| panic!("untracked turn: {sent}\n{}", stderr_tail(&self.stderr)))
            .to_owned();
        let waited = self.request(
            "meta/wait_for_turn",
            json!({ "identity": MEMBER, "ticket": ticket, "timeout_ms": 90_000 }),
            Duration::from_mins(2),
        );
        assert_eq!(
            waited["result"]["state"],
            json!("completed"),
            "the turn {tag} completes: {waited}\n{}",
            stderr_tail(&self.stderr)
        );
        model.settled_answers(tag, calls).unwrap_or_else(|| {
            panic!(
                "the turn {tag} returned too few tool answers to the model: {}\n{}",
                model.seen(tag),
                stderr_tail(&self.stderr)
            )
        })
    }

    /// Make every call of `expected`, one turn each: what became of each.
    fn outcomes(&mut self, model: &Model, root: &Path, expected: &Expected) -> Vec<String> {
        expected
            .iter()
            .enumerate()
            .map(|(index, (step, _))| {
                let tag = format!("t{index}");
                let content = turn_content(&tag, std::slice::from_ref(step));
                step.outcome(root, &self.turn(model, &tag, &content, 1)[0])
            })
            .collect()
    }
}

impl Drop for StdioGateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            eprintln!("rpc_gateway stderr:\n{}", stderr_tail(&self.stderr));
        }
    }
}

/// The directories of an `rpc_gateway` row and its gateway: `app` holds
/// the state directory, so it is the root of the member's file tools; the
/// configuration file the launch names and the process's temporary files
/// lie beside it.
struct StdioLaunch {
    scratch: TempDir,
    model: Model,
    gateway: StdioGateway,
}

impl StdioLaunch {
    /// `schedule` and `responses` as in [`mob_toml`] and [`model_tables`].
    fn new(mob_id: &str, schedule: bool, responses: bool) -> Self {
        let scratch = TempDir::new().expect("scratch");
        let model = Model::start();
        let outside = scratch.path().join("outside");
        let named = outside.join("gateway-config.toml");
        write_file(&named, &model_tables(mob_id, &model.base_url, responses));
        let app = scratch.path().join("app");
        std::fs::create_dir_all(&app).expect("create the app directory");
        let gateway = StdioGateway::boot(
            &mob_toml(mob_id, schedule),
            &app.join("state"),
            &outside.join("tmp"),
            &json!({
                "meerkat_config_path": named,
                "identity_bootstrap_mode": { "mode": "eager_materialize" },
            }),
        );
        Self {
            scratch,
            model,
            gateway,
        }
    }

    fn app(&self) -> PathBuf {
        self.scratch.path().join("app")
    }
}

/// A persistent launch: the member's file tools are rooted in the
/// directory that holds the state directory. A member's `blob_load_file`
/// does not read a file in the state directory and its `view_image` does
/// not read an image-named file in it; both read a file beside it.
#[test]
fn rpc_gateway_members_reading_tools_do_not_read_the_state_directory() {
    let mut launch = StdioLaunch::new("host-state-stdio-reads", false, true);
    let app = launch.app();
    let state = app.join("state");
    write_file(
        &state.join("fixture-note.txt"),
        &format!("{FIXTURE_LINE}\n"),
    );
    write_bytes(&state.join("chart.png"), IMAGE_BYTES);
    write_bytes(&app.join("notes").join("chart.png"), IMAGE_BYTES);
    let expected = [
        (Step::add("notes/plan.md"), "written"),
        (Step::load("notes/plan.md"), "read"),
        (Step::view("notes/chart.png"), "read"),
        (Step::load("state/fixture-note.txt"), "refused"),
        (Step::view("state/chart.png"), "refused"),
    ];
    let found = launch.gateway.outcomes(&launch.model, &app, &expected);
    assert_outcomes(&expected, &found);
}

/// A member whose profile has the schedule tools stores a schedule whose
/// target is a session created on demand. The session its fire creates,
/// which no mob builds, is offered none of the file tools: this gateway's
/// own sessions have no builtin tools, and a member has them by its
/// profile.
#[test]
fn rpc_gateway_offers_no_file_tool_to_a_session_a_schedule_creates() {
    let mut launch = StdioLaunch::new("host-state-stdio-fired", true, false);
    let stored = launch.gateway.turn(
        &launch.model,
        "s0",
        "TURN-s0 MAKE-SCHEDULE f0+ADD-FILE+notes/fired-plan.md now",
        1,
    );
    assert!(
        stored[0].contains("schedule_id") || stored[0].contains("\"id\""),
        "the schedule is stored: {}",
        clip(&stored[0])
    );
    assert!(
        eventually(SETTLE, || launch.model.first_request("f0").is_some()),
        "the schedule fires into a session created on demand: {}\n{}",
        launch.model.seen("f0"),
        stderr_tail(&launch.gateway.stderr)
    );
    let (offered, users) = launch
        .model
        .first_request("f0")
        .expect("the first request of the session the fire creates");
    println!(
        "the session the fire creates is offered: {}",
        offered.join(", ")
    );
    assert!(
        !users.contains("TURN-s0"),
        "the fire's session is not the member's: its user messages do not hold the member's turn"
    );
    let file_tools: Vec<&String> = offered
        .iter()
        .filter(|name| FILE_TOOLS.contains(&name.as_str()))
        .collect();
    assert!(
        file_tools.is_empty(),
        "the session the fire creates is offered no file tool: {file_tools:?}"
    );
    assert!(
        !launch.app().join("notes").join("fired-plan.md").exists(),
        "the session the fire creates wrote no file"
    );
}
