//! The paths a gateway declares to its members' file tools.
//!
//! A member's in-process file tools (`apply_patch`, `blob_save_file`) are
//! rooted in one directory: the project root on `meta_gateway` (the
//! workspace when the launch gives none), and the directory that holds the
//! state directory on `rpc_gateway` and on a library-built runtime. The
//! gateways and the library builder declare to those tools the paths they
//! keep their state and configuration in: the state directory, the gateway
//! home, the workspace's configuration directories, and the configuration
//! file and credentials root a launch names. The tools answer a call on such
//! a path with Meerkat's refusal for a host's own location and write
//! nothing; a `.rkat` directory is refused without a declaration, and every
//! other file under the root is written as before.
//!
//! Each row runs the shipped binary (or the library builder) with one
//! member, whose profile has the builtin tools and no shell, on a loopback
//! chat-completions model that makes the file tool call its turn names: an
//! `apply_patch` that adds a file or changes a line of one, or a
//! `blob_load_file` of an ordinary file followed by a `blob_save_file` of
//! that blob. A row compares what became of each call with what the row
//! states.

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

const MODEL: &str = "host-paths-test";
/// The tool that adds a file or changes one.
const PATCH_TOOL: &str = "apply_patch";
/// The tool that reads a file into a blob.
const LOAD_TOOL: &str = "blob_load_file";
/// The tool that writes a blob to a file.
const SAVE_TOOL: &str = "blob_save_file";
/// The one member of every row: its profile has the builtin tools and no
/// shell.
const MEMBER: &str = "agent:writer";
/// The line an added file holds.
const LINE: &str = "written by a member";
/// The last line of every file a row lets a member change.
const FIXTURE_LINE: &str = "# fixture line";
/// What a change makes of [`FIXTURE_LINE`].
const CHANGED_LINE: &str = "# changed by a member";
/// The ordinary file the blob tools copy: the second control call of every
/// row wrote it.
const COPY_SOURCE: &str = "notes/plan.md";
/// Meerkat's words for a path in the engine's own directory or in a
/// location the host declared.
const REFUSAL: &str = "the file tools do not write there";
const SECRET: &str = "host-paths-signing-key-with-enough-bytes";
const PERSON_SUBJECT: &str = "host-paths-person";
const PERSON_EMAIL: &str = "host.paths@example.com";
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

// ---------------------------------------------------------------------------
// The calls a row makes
// ---------------------------------------------------------------------------

/// One file tool call a member makes, by a path relative to its tools' root.
#[derive(Debug, Clone, Copy)]
enum Call {
    /// `apply_patch` adds a new file.
    Add(&'static str),
    /// `apply_patch` changes [`FIXTURE_LINE`] of an existing file.
    Change(&'static str),
    /// `blob_load_file` reads [`COPY_SOURCE`] and `blob_save_file` writes
    /// that blob to a new file.
    Copy(&'static str),
}

impl Call {
    fn path(self) -> &'static str {
        match self {
            Self::Add(path) | Self::Change(path) | Self::Copy(path) => path,
        }
    }

    /// The request a turn carries: the model reads the verb and the path
    /// after the turn's tag.
    fn content(self, tag: &str) -> String {
        match self {
            Self::Add(path) => format!("TURN-{tag} ADD-FILE {path} now"),
            Self::Change(path) => format!("TURN-{tag} CHANGE-FILE {path} now"),
            Self::Copy(path) => format!("TURN-{tag} COPY-FILE {path} now"),
        }
    }

    /// How many tool calls the turn makes: the last one's answer is the
    /// call's result.
    fn steps(self) -> usize {
        match self {
            Self::Add(_) | Self::Change(_) => 1,
            Self::Copy(_) => 2,
        }
    }

    /// What became of the call: `"refused"` when the tool answered with
    /// Meerkat's refusal and the file is as it was, `"written"` when the
    /// tool did what was asked, and otherwise the tool's answer.
    fn outcome(self, root: &Path, result: &str) -> String {
        let text = std::fs::read_to_string(root.join(self.path())).ok();
        let done = match self {
            Self::Add(_) | Self::Copy(_) => text.is_some_and(|text| text == format!("{LINE}\n")),
            Self::Change(_) => text.is_some_and(|text| text.contains(CHANGED_LINE)),
        };
        match (result.contains(REFUSAL), done) {
            (true, false) => "refused".to_owned(),
            (false, true) => "written".to_owned(),
            _ => format!("unexpected (done: {done}): {result}"),
        }
    }
}

/// The rows' table: each call with what the row states becomes of it.
type Expected = [(Call, &'static str)];

/// Compare what became of every call with what the row states, all at
/// once, so a failure shows the whole table.
fn assert_outcomes(expected: &Expected, found: &[String]) {
    let stated: Vec<String> = expected
        .iter()
        .map(|(call, outcome)| format!("{}: {outcome}", call.path()))
        .collect();
    let found: Vec<String> = expected
        .iter()
        .zip(found)
        .map(|((call, _), outcome)| format!("{}: {outcome}", call.path()))
        .collect();
    assert_eq!(
        found, stated,
        "what became of each file tool call (found, then stated)"
    );
}

// ---------------------------------------------------------------------------
// Loopback model
// ---------------------------------------------------------------------------

fn message_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The newest text among a request's messages that carries a `TURN-<tag>`,
/// from that tag on.
fn turn_text(request: &Value) -> Option<String> {
    request["messages"]
        .as_array()?
        .iter()
        .rev()
        .find_map(|message| {
            let text = message_text(message);
            let start = text.rfind("TURN-")?;
            Some(text[start..].to_owned())
        })
}

/// The turn a request belongs to: the tag of the newest `TURN-<tag>` its
/// messages carry.
fn turn_tag(request: &Value) -> Option<String> {
    let text = turn_text(request)?;
    let tag: String = text["TURN-".len()..]
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    (!tag.is_empty()).then_some(tag)
}

/// The tool call the turn of a request makes next, if any: the verb and
/// the path follow the turn's tag, and the answers of the turn's earlier
/// calls follow that message.
fn next_call(request: &Value) -> Option<(&'static str, Value)> {
    let messages = request["messages"].as_array()?;
    let turn = messages
        .iter()
        .rposition(|message| message_text(message).contains("TURN-"))?;
    let text = message_text(&messages[turn]);
    let mut words = text[text.rfind("TURN-")?..].split_whitespace().skip(1);
    let verb = words.next()?;
    let path: String = words
        .next()?
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || "/._-".contains(*character))
        .collect();
    let answers: Vec<String> = messages[turn + 1..]
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(message_text)
        .collect();
    match (verb, answers.len()) {
        ("ADD-FILE", 0) => Some((
            PATCH_TOOL,
            json!({ "patch": format!(
                "*** Begin Patch\n*** Add File: {path}\n+{LINE}\n*** End Patch"
            ) }),
        )),
        ("CHANGE-FILE", 0) => Some((
            PATCH_TOOL,
            json!({ "patch": format!(
                "*** Begin Patch\n*** Update File: {path}\n@@\n-{FIXTURE_LINE}\n+{CHANGED_LINE}\n*** End Patch"
            ) }),
        )),
        ("COPY-FILE", 0) => Some((LOAD_TOOL, json!({ "path": COPY_SOURCE }))),
        ("COPY-FILE", 1) => {
            let loaded: Value = serde_json::from_str(&answers[0]).ok()?;
            let blob_id = loaded["blob_id"].as_str()?;
            Some((SAVE_TOOL, json!({ "blob_id": blob_id, "path": path })))
        }
        _ => None,
    }
}

fn last_role(request: &Value) -> String {
    request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["role"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// The names of the tools a request offers the model.
fn offered_tools(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// A streamed answer that is the text `text` and ends the turn.
fn text_reply(text: &str) -> String {
    let delta = json!({ "choices": [{ "delta": { "content": text } }] });
    let done = json!({
        "choices": [{ "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 1 }
    });
    format!("data: {delta}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

/// A streamed answer that is one call of `tool` with `arguments`; `step`
/// tells the calls of one turn apart.
fn tool_call_reply(tool: &str, arguments: &Value, step: usize) -> String {
    let call = json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": format!("call-host-paths-{step}"),
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

/// A turn tagged `TURN-<tag>` makes the tool calls it names, one per
/// request, and ends with `ANSWER-<tag>` when the last one's answer comes
/// back. Any other request, and a turn whose tool is not offered, is
/// answered with text.
async fn chat_completions(
    axum::extract::State(requests): axum::extract::State<Arc<Mutex<Vec<Value>>>>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    let tag = turn_tag(&request).unwrap_or_default();
    let offered = offered_tools(&request);
    let call = next_call(&request)
        .filter(|(tool, _)| offered.iter().any(|offered| offered.as_str() == *tool));
    let step = requests.lock().unwrap().len();
    requests.lock().unwrap().push(request);
    let payload = match call {
        Some((tool, arguments)) => tool_call_reply(tool, &arguments, step),
        None => text_reply(&format!("ANSWER-{tag}")),
    };
    ([("content-type", "text/event-stream")], payload)
}

async fn models() -> impl axum::response::IntoResponse {
    axum::Json(json!({ "data": [] }))
}

/// The loopback chat-completions server and every request it answered.
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
                    .route(
                        "/v1/chat/completions",
                        axum::routing::post(chat_completions),
                    )
                    .route("/chat/completions", axum::routing::post(chat_completions))
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

    /// The results of the tool calls of the turn `tag`, as the model
    /// received them.
    fn tool_results(&self, tag: &str) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| {
                last_role(request) == "tool" && turn_tag(request).as_deref() == Some(tag)
            })
            .filter_map(|request| request["messages"].as_array()?.last().map(message_text))
            .collect()
    }

    /// The result of `call`, made in the turn `tag`: the answer of the
    /// turn's last tool call, or `None` when it did not come back within
    /// the settle window.
    fn tool_result(&self, tag: &str, call: Call) -> Option<String> {
        eventually(SETTLE, || self.tool_results(tag).len() >= call.steps());
        self.tool_results(tag).into_iter().nth(call.steps() - 1)
    }

    /// What the model saw of the turn `tag`, for a failed wait.
    fn seen(&self, tag: &str) -> String {
        let requests = self.requests.lock().unwrap();
        let of_turn: Vec<&Value> = requests
            .iter()
            .filter(|request| turn_tag(request).as_deref() == Some(tag))
            .collect();
        format!(
            "{} of {} model requests belong to the turn; the first offers {:?}",
            of_turn.len(),
            requests.len(),
            of_turn.first().map(|request| offered_tools(request))
        )
    }
}

// ---------------------------------------------------------------------------
// Configuration files
// ---------------------------------------------------------------------------

/// A mob with one turn-driven profile on the loopback model: `writer` has
/// the builtin tools and no shell.
fn mob_toml(mob_id: &str) -> String {
    format!(
        "[mob]\nid = \"{mob_id}\"\n\n\
         [profiles.writer]\nmodel = \"{MODEL}\"\nprovider = \"self_hosted\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.writer.tools]\ncomms = true\nbuiltins = true\n{FIXTURE_LINE}\n"
    )
}

/// [`mob_toml`], with a skill the `writer` profile reads from the file
/// `skill` at each of its builds.
fn mob_toml_with_skill(mob_id: &str, skill: &Path) -> String {
    format!(
        "[mob]\nid = \"{mob_id}\"\n\n\
         [profiles.writer]\nmodel = \"{MODEL}\"\nprovider = \"self_hosted\"\n\
         skills = [\"writer-role\"]\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.writer.tools]\ncomms = true\nbuiltins = true\n\n\
         [skills.writer-role]\nsource = \"path\"\npath = '{}'\n{FIXTURE_LINE}\n",
        skill.display()
    )
}

/// What the skill file of [`mob_toml_with_skill`] holds.
fn skill_text() -> String {
    format!("You write short notes.\n{FIXTURE_LINE}\n")
}

/// The configuration tables that route the members of `mob_id` to the
/// loopback model at `model_url`.
fn model_tables(mob_id: &str, model_url: &str) -> String {
    let mut tables = format!(
        "[self_hosted]\ndefault_model = \"{MODEL}\"\n\n[self_hosted.servers.loop]\n\
         base_url = \"{model_url}\"\n\n[self_hosted.models.\"{MODEL}\"]\nserver = \"loop\"\n\
         remote_model = \"{MODEL}\"\ndisplay_name = \"Host paths test model\"\n\
         family = \"{MODEL}\"\n"
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
    tables.push_str(&format!("{FIXTURE_LINE}\n"));
    tables
}

/// An access file that switches nothing on.
fn access_toml() -> String {
    format!("enabled = false\n{FIXTURE_LINE}\n")
}

fn write_file(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent directory"))
        .expect("create the directory");
    std::fs::write(path, text).expect("write the file");
}

fn remove_provider_keys(command: &mut Command) {
    // The member's model is the loopback one: no provider key reaches the
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
    /// by the row) with the layout a launch that names no root gets: the
    /// project root is the workspace and the state directory is
    /// `<workspace>/state`. The gateway home is `<workspace>/xdg-state`'s.
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

    /// One console request to [`MEMBER`], waited until it is answered: the
    /// result of its tool call as the model received it.
    fn turn(&self, model: &Model, tag: &str, call: Call) -> String {
        let (status, sent) = self.post(
            "/console/send",
            &json!({
                "identity": MEMBER,
                "content": call.content(tag),
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
        let result = model.tool_result(tag, call).unwrap_or_else(|| {
            panic!(
                "the turn {tag} returned no tool result to the model: {}\n{}",
                model.seen(tag),
                stderr_tail(&self.stderr)
            )
        });
        assert!(
            eventually(SETTLE, || self.answered(tag)),
            "the turn {tag} is answered\n{}",
            stderr_tail(&self.stderr)
        );
        result
    }

    /// Make every call of `expected`, one turn each: what became of each.
    fn outcomes(&self, model: &Model, root: &Path, expected: &Expected) -> Vec<String> {
        expected
            .iter()
            .enumerate()
            .map(|(index, (call, _))| {
                call.outcome(root, &self.turn(model, &format!("t{index}"), *call))
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

/// A workspace of `meta_gateway` rows with its mob definition written.
struct Workspace {
    dir: TempDir,
    mob_id: String,
    model: Model,
}

impl Workspace {
    fn new(mob_id: &str) -> Self {
        let dir = TempDir::new().expect("workspace");
        write_file(
            &dir.path().join("config").join("mob.toml"),
            &mob_toml(mob_id),
        );
        Self {
            dir,
            mob_id: mob_id.to_owned(),
            model: Model::start(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write the member's model tables to the workspace configuration file
    /// the gateway adopts when its launch names none.
    fn adopt_model_tables(&self) {
        write_file(
            &self.path().join(".rkat").join("config.toml"),
            &model_tables(&self.mob_id, &self.model.base_url),
        );
    }
}

/// The calls every row makes first: a `.rkat` directory under the tools'
/// root is refused with no declaration, and an ordinary file is written.
const CONTROLS: [(Call, &str); 2] = [
    (Call::Add(".rkat/member-note.txt"), "refused"),
    (Call::Add("notes/plan.md"), "written"),
];

/// A launch that names no root: the state directory, the gateway home, the
/// shell's process records, and the workspace's `config/` and `deployment/`
/// directories, its `definition.json` and its `manifest.toml` lie under the
/// members' file tools' root, and the gateway declares each. A member's
/// file tools add no file there and change none: not the mob definition,
/// not the access file, and no new file beside them, since the gateway
/// reads the files it finds in those directories at its next start.
#[test]
fn meta_gateway_declares_its_state_and_workspace_configuration_to_a_members_file_tools() {
    let workspace = Workspace::new("host-paths-http");
    workspace.adopt_model_tables();
    write_file(
        &workspace.path().join("config").join("access.toml"),
        &access_toml(),
    );
    let gateway = HttpGateway::start(workspace.path(), &json!({}));
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add("state/member-note.txt"), "refused"),
        (Call::Copy("state/member-copy.txt"), "refused"),
        (Call::Change("config/mob.toml"), "refused"),
        (Call::Change("config/access.toml"), "refused"),
        (Call::Add("config/gating.toml"), "refused"),
        (Call::Add("config/member-note.toml"), "refused"),
        (Call::Add("deployment/routing.toml"), "refused"),
        (Call::Add("definition.json"), "refused"),
        (Call::Add("manifest.toml"), "refused"),
        (
            Call::Add("xdg-state/meerkat-meta/member-note.txt"),
            "refused",
        ),
        (Call::Add("tool_process_custody/member-note.txt"), "refused"),
    ];
    let found = gateway.outcomes(&workspace.model, workspace.path(), &expected);
    assert_outcomes(&expected, &found);
}

/// A launch that names a configuration file and a provider credentials
/// root under the workspace: the gateway declares both. A member's
/// `apply_patch` does not change the file and adds none under the root.
#[test]
fn meta_gateway_declares_the_paths_its_launch_names_to_a_members_file_tools() {
    let workspace = Workspace::new("host-paths-http-named");
    let named = workspace.path().join("named").join("gateway-config.toml");
    write_file(
        &named,
        &model_tables(&workspace.mob_id, &workspace.model.base_url),
    );
    let gateway = HttpGateway::start(
        workspace.path(),
        &json!({
            "meerkat_config_path": named,
            "provider_credentials_root": workspace.path().join("credentials"),
        }),
    );
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add("credentials/member-note.txt"), "refused"),
        (Call::Change("named/gateway-config.toml"), "refused"),
    ];
    let found = gateway.outcomes(&workspace.model, workspace.path(), &expected);
    assert_outcomes(&expected, &found);
}

/// A definition that names a skill by a file path: a member's profile reads
/// that file at each of its builds, and the gateway declares it; it also
/// declares the workspace's `skills/` directory, which it counts among its
/// configuration. A member's `apply_patch` does not change the skill file
/// and adds no file under `skills/`; a new file beside the skill file is
/// written, since its directory is not the gateway's own.
#[test]
fn meta_gateway_declares_the_skill_files_its_definition_names_and_the_workspace_skills() {
    let workspace = Workspace::new("host-paths-http-skills");
    workspace.adopt_model_tables();
    let skill = workspace.path().join("roles").join("writer.md");
    write_file(&skill, &skill_text());
    write_file(
        &workspace.path().join("config").join("mob.toml"),
        &mob_toml_with_skill(&workspace.mob_id, &skill),
    );
    let gateway = HttpGateway::start(workspace.path(), &json!({}));
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Change("roles/writer.md"), "refused"),
        (Call::Add("skills/member-note.md"), "refused"),
        (Call::Add("roles/member-note.md"), "written"),
    ];
    let found = gateway.outcomes(&workspace.model, workspace.path(), &expected);
    assert_outcomes(&expected, &found);
}

/// The gateway counts its workspace's `hooks/` directory among its
/// configuration and declares it: a member's `apply_patch` adds no file
/// under it.
#[test]
fn meta_gateway_declares_the_workspace_hooks_directory() {
    let workspace = Workspace::new("host-paths-http-hooks");
    workspace.adopt_model_tables();
    let gateway = HttpGateway::start(workspace.path(), &json!({}));
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add("hooks/member-note.sh"), "refused"),
    ];
    let found = gateway.outcomes(&workspace.model, workspace.path(), &expected);
    assert_outcomes(&expected, &found);
}

/// The gateway counts its workspace's `mcp/` directory among its
/// configuration and declares it: a member's `apply_patch` adds no file
/// under it.
#[test]
fn meta_gateway_declares_the_workspace_mcp_directory() {
    let workspace = Workspace::new("host-paths-http-mcp");
    workspace.adopt_model_tables();
    let gateway = HttpGateway::start(workspace.path(), &json!({}));
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add("mcp/member-note.toml"), "refused"),
    ];
    let found = gateway.outcomes(&workspace.model, workspace.path(), &expected);
    assert_outcomes(&expected, &found);
}

/// Whether `message` names `root` as the directory the members' file tools
/// are rooted at and `state` as the directory that root is, or lies inside.
/// Both are compared by their physical location.
fn names_layout(message: &str, root: &Path, state: &Path) -> bool {
    let real = |path: &Path| {
        std::fs::canonicalize(path)
            .expect("an existing directory")
            .display()
            .to_string()
    };
    message.contains(&format!("rooted at {},", real(root)))
        && message.contains(&format!("{} or lies inside it", real(state)))
}

/// A launch whose project root lies inside its state directory is refused
/// at its start: the members' file tools are rooted at the project root,
/// and the gateway starts only where that root is apart from its state
/// directory. The refusal names both directories and the two parameters
/// to change.
#[test]
fn meta_gateway_refuses_a_project_root_inside_its_state_directory() {
    let workspace = Workspace::new("host-paths-http-root-in-state");
    workspace.adopt_model_tables();
    let store = workspace.path().join("store");
    let project = store.join("work");
    std::fs::create_dir_all(&project).expect("create the project root");
    let message = HttpGateway::refused_start(
        workspace.path(),
        &json!({ "store_path": store, "project_root": project }),
    );
    assert!(
        names_layout(&message, &project, &store)
            && message.contains("project_root")
            && message.contains("store_path"),
        "the refusal names both directories and what to change: {message}"
    );
}

/// A launch whose state directory is its workspace, or a directory above
/// it, is refused at its start: with no project root given the members'
/// file tools are rooted at the workspace, which then is the state
/// directory or lies inside it.
#[test]
fn meta_gateway_refuses_a_state_directory_at_or_above_its_workspace() {
    for above in [false, true] {
        let mob_id = "host-paths-http-state-at-workspace";
        let model = Model::start();
        let outer = TempDir::new().expect("outer directory");
        let workspace = outer.path().join("inner").join("workspace");
        write_file(
            &workspace.join("config").join("mob.toml"),
            &mob_toml(mob_id),
        );
        write_file(
            &workspace.join(".rkat").join("config.toml"),
            &model_tables(mob_id, &model.base_url),
        );
        let store = if above {
            outer.path().join("inner")
        } else {
            workspace.clone()
        };
        let message = HttpGateway::refused_start(&workspace, &json!({ "store_path": store }));
        assert!(
            names_layout(&message, &workspace, &store)
                && message.contains("project_root")
                && message.contains("store_path"),
            "the refusal names both directories and what to change \
             (state directory above the workspace: {above}): {message}"
        );
    }
}

/// A launch whose provider credentials root is its workspace is refused
/// at its start: with no project root given the members' file tools would
/// be rooted at the credentials root. The refusal names both directories
/// and what to change.
#[test]
fn meta_gateway_refuses_a_credentials_root_at_its_project_root() {
    let workspace = Workspace::new("host-paths-http-credentials-root");
    workspace.adopt_model_tables();
    let message = HttpGateway::refused_start(
        workspace.path(),
        &json!({ "provider_credentials_root": workspace.path() }),
    );
    assert!(
        names_layout(&message, workspace.path(), workspace.path())
            && message.contains("project_root")
            && message.contains("provider_credentials_root"),
        "the refusal names both directories and what to change: {message}"
    );
}

/// A launch without persistent sessions builds its members from a second
/// factory: it declares the same paths.
#[test]
fn meta_gateway_without_persistent_sessions_declares_the_same_paths() {
    let workspace = Workspace::new("host-paths-http-local");
    workspace.adopt_model_tables();
    let gateway = HttpGateway::start(workspace.path(), &json!({ "persistent_sessions": false }));
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add("state/member-note.txt"), "refused"),
        (Call::Change("config/mob.toml"), "refused"),
        (Call::Add("config/access.toml"), "refused"),
    ];
    let found = gateway.outcomes(&workspace.model, workspace.path(), &expected);
    assert_outcomes(&expected, &found);
}

// ---------------------------------------------------------------------------
// rpc_gateway
// ---------------------------------------------------------------------------

/// One `rpc_gateway --persistent` with the member [`MEMBER`], driven over
/// its stdio.
struct StdioGateway {
    child: Child,
    /// The gateway's stdin, until [`Self::shut_down`] closes it.
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
    next_id: u64,
}

impl StdioGateway {
    /// Start the gateway and initialize it with `mob_config`, a roster
    /// provider and `runtime_options`; with `state`, over that state
    /// directory. The process keeps its temporary files under `temp`.
    fn boot(mob_config: &str, state: Option<&Path>, temp: &Path, runtime_options: &Value) -> Self {
        Self::boot_at_home(mob_config, state, temp, None, runtime_options)
    }

    /// [`Self::boot`], with `home` as the process's home directory and no
    /// other per-user directory named: the per-user defaults lie under it.
    fn boot_at_home(
        mob_config: &str,
        state: Option<&Path>,
        temp: &Path,
        home: Option<&Path>,
        runtime_options: &Value,
    ) -> Self {
        let (gateway, response) = Self::init(mob_config, state, temp, home, runtime_options);
        assert!(
            response.get("error").is_none(),
            "init failed: {response}\n{}",
            stderr_tail(&gateway.stderr)
        );
        gateway
    }

    /// The message of the refusal a launch over `state` with
    /// `runtime_options` gets at its init. The row fails when the gateway
    /// starts.
    fn refused_boot(
        mob_config: &str,
        state: &Path,
        temp: &Path,
        runtime_options: &Value,
    ) -> String {
        let (gateway, response) = Self::init(mob_config, Some(state), temp, None, runtime_options);
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

    /// Spawn the gateway, send its init and return the init's answer.
    fn init(
        mob_config: &str,
        state: Option<&Path>,
        temp: &Path,
        home: Option<&Path>,
        runtime_options: &Value,
    ) -> (Self, Value) {
        std::fs::create_dir_all(temp).expect("create the temporary directory");
        let mut command = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"));
        command
            .arg("--persistent")
            .env("TMPDIR", temp)
            .env("RUST_LOG", "info")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(home) = home {
            std::fs::create_dir_all(home).expect("create the home directory");
            command.env("HOME", home);
            for name in [
                "XDG_CONFIG_HOME",
                "XDG_STATE_HOME",
                "XDG_DATA_HOME",
                "XDG_CACHE_HOME",
            ] {
                command.env_remove(name);
            }
        }
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
            stdin: Some(stdin),
            lines: rx,
            stderr,
            next_id: 0,
        };
        let mut params = json!({
            "mob_config": mob_config,
            "has_roster_provider": true,
            "runtime_options": runtime_options,
        });
        if let Some(state) = state {
            params["persistent_state"] = json!(state);
        }
        let response = gateway.request("meta/init", params, Duration::from_mins(3));
        (gateway, response)
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("an open gateway stdin");
        writeln!(stdin, "{value}").expect("write request");
        stdin.flush().expect("flush request");
    }

    /// Shut the gateway down as an SDK host does (the shutdown request,
    /// then its stdin closed once the gateway answered) and wait for its
    /// exit.
    fn shut_down(mut self) {
        let answer = self.request("meta/shutdown", json!({}), Duration::from_mins(2));
        assert!(
            answer.get("error").is_none(),
            "the shutdown is answered: {answer}\n{}",
            stderr_tail(&self.stderr)
        );
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_mins(2);
        while self
            .child
            .try_wait()
            .expect("gateway exit status")
            .is_none()
        {
            assert!(
                Instant::now() < deadline,
                "the gateway did not exit\n{}",
                stderr_tail(&self.stderr)
            );
            std::thread::sleep(Duration::from_millis(50));
        }
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

    /// One tracked turn of [`MEMBER`], waited until it completes: the
    /// result of its tool call as the model received it.
    fn turn(&mut self, model: &Model, tag: &str, call: Call) -> String {
        let sent = self.request(
            "meta/send",
            json!({
                "identity": MEMBER,
                "content": call.content(tag),
                "track_turn": true
            }),
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
        model.tool_result(tag, call).unwrap_or_else(|| {
            panic!(
                "the turn {tag} returned no tool result to the model: {}\n{}",
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
            .map(|(index, (call, _))| {
                call.outcome(root, &self.turn(model, &format!("t{index}"), *call))
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

/// The directories of an `rpc_gateway` row: `app` holds the state
/// directory, so it is the root of the member's file tools; `outside` is
/// beside it.
struct StdioLayout {
    scratch: TempDir,
}

impl StdioLayout {
    fn new() -> Self {
        let scratch = TempDir::new().expect("scratch");
        std::fs::create_dir_all(scratch.path().join("app")).expect("create the app directory");
        Self { scratch }
    }

    fn app(&self) -> PathBuf {
        self.scratch.path().join("app")
    }

    fn outside(&self) -> PathBuf {
        self.scratch.path().join("outside")
    }
}

/// A persistent launch: the member's file tools are rooted in the directory
/// that holds the state directory, and the gateway declares the state
/// directory. A member's `apply_patch` adds no file in it, by its own name
/// or through a link that leads into it, and `blob_save_file` writes none.
#[test]
fn rpc_gateway_declares_its_state_directory_to_a_members_file_tools() {
    let mob_id = "host-paths-stdio";
    let model = Model::start();
    let layout = StdioLayout::new();
    let named = layout.outside().join("gateway-config.toml");
    write_file(&named, &model_tables(mob_id, &model.base_url));
    let mut gateway = StdioGateway::boot(
        &mob_toml(mob_id),
        Some(&layout.app().join("state")),
        &layout.outside().join("tmp"),
        &json!({
            "meerkat_config_path": named,
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
        }),
    );
    std::os::unix::fs::symlink(layout.app().join("state"), layout.app().join("shortcut"))
        .expect("link to the state directory");
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add("state/member-note.txt"), "refused"),
        (Call::Add("shortcut/member-linked-note.txt"), "refused"),
        (Call::Copy("state/member-copy.txt"), "refused"),
    ];
    let found = gateway.outcomes(&model, &layout.app(), &expected);
    assert_outcomes(&expected, &found);
}

/// A persistent launch whose members' configuration is a native realm's:
/// the gateway declares the state directory and the realms root the launch
/// names. A member's `apply_patch` adds no file in either. The realm's
/// user document lies in the `.rkat` directory of the user configuration
/// root the launch names, which needs no declaration: no file is added
/// there and the document is not changed.
#[test]
fn rpc_gateway_with_a_native_realm_declares_its_state_directory_and_the_realms_root() {
    let mob_id = "host-paths-stdio-realm";
    let model = Model::start();
    let layout = StdioLayout::new();
    let user_root = layout.app().join("user");
    write_file(
        &user_root.join(".rkat").join("config.toml"),
        &model_tables(mob_id, &model.base_url),
    );
    let context_root = layout.outside().join("context");
    let realms_root = layout.app().join("realms");
    for directory in [&context_root, &realms_root] {
        std::fs::create_dir_all(directory).expect("create the realm door's directory");
    }
    let mut gateway = StdioGateway::boot(
        &mob_toml(mob_id),
        Some(&layout.app().join("state")),
        &layout.outside().join("tmp"),
        &json!({
            "native_realm": {
                "realm_id": "global",
                "context_root": context_root,
                "state_root": realms_root,
                "user_config_root": user_root,
            },
            "member_auth_binding": { "realm": "global", "binding": "loop" },
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
        }),
    );
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add("state/member-note.txt"), "refused"),
        (Call::Add("realms/member-note.txt"), "refused"),
        (Call::Add("user/.rkat/member-note.txt"), "refused"),
        (Call::Change("user/.rkat/config.toml"), "refused"),
    ];
    let found = gateway.outcomes(&model, &layout.app(), &expected);
    assert_outcomes(&expected, &found);
}

/// A persistent launch that names its configuration file, its access file,
/// a provider credentials root, a work graph directory and a session
/// identity root beside the state directory, in the directory the members'
/// file tools are rooted in: the gateway declares each of the first four by
/// its own path. A member's `apply_patch` changes neither file and adds
/// none under the directories.
#[test]
fn rpc_gateway_declares_the_paths_its_launch_names_to_a_members_file_tools() {
    let mob_id = "host-paths-stdio-named";
    let model = Model::start();
    let layout = StdioLayout::new();
    let named = layout.app().join("gateway-config.toml");
    write_file(&named, &model_tables(mob_id, &model.base_url));
    let access = layout.app().join("access.toml");
    write_file(&access, &access_toml());
    let mut gateway = StdioGateway::boot(
        &mob_toml(mob_id),
        Some(&layout.app().join("state")),
        &layout.outside().join("tmp"),
        &json!({
            "meerkat_config_path": named,
            "access_config_path": access,
            "provider_credentials_root": layout.app().join("credentials"),
            "workgraph": layout.app().join("workgraph"),
            "session_identity_config_root": layout.app().join("identity-root"),
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
        }),
    );
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Change("gateway-config.toml"), "refused"),
        (Call::Change("access.toml"), "refused"),
        (Call::Add("credentials/member-note.txt"), "refused"),
        (Call::Add("workgraph/member-note.txt"), "refused"),
        // What the gateway keeps under the session identity root lies in
        // its `.rkat` directory, which needs no declaration.
        (
            Call::Add("identity-root/.rkat/session_comms_identity/member-note.txt"),
            "refused",
        ),
    ];
    let found = gateway.outcomes(&model, &layout.app(), &expected);
    assert_outcomes(&expected, &found);
}

/// A persistent launch that names a gating file, a routing file and a
/// console file in the directory the members' file tools are rooted in: the
/// gateway declares each by its own path, and a member's `apply_patch`
/// changes none of them. Their directory is the tools' root and not the
/// gateway's own: a new file beside them is written.
#[test]
fn rpc_gateway_declares_the_gating_routing_and_console_files_its_launch_names() {
    let mob_id = "host-paths-stdio-files";
    let model = Model::start();
    let layout = StdioLayout::new();
    let named = layout.outside().join("gateway-config.toml");
    write_file(&named, &model_tables(mob_id, &model.base_url));
    let gating = layout.app().join("gating.toml");
    write_file(&gating, &format!("[actions]\n{FIXTURE_LINE}\n"));
    let routing = layout.app().join("routing.toml");
    write_file(&routing, &format!("routes = []\n{FIXTURE_LINE}\n"));
    let console = layout.app().join("console.toml");
    write_file(
        &console,
        &format!("[sidebar]\nvisible_controls = [\"roster\"]\n{FIXTURE_LINE}\n"),
    );
    let mut gateway = StdioGateway::boot(
        &mob_toml(mob_id),
        Some(&layout.app().join("state")),
        &layout.outside().join("tmp"),
        &json!({
            "meerkat_config_path": named,
            "gating_config_path": gating,
            "routing_config_path": routing,
            "console_config_path": console,
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
        }),
    );
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Change("gating.toml"), "refused"),
        (Call::Change("routing.toml"), "refused"),
        (Call::Change("console.toml"), "refused"),
        (Call::Add("member-note.toml"), "written"),
    ];
    let found = gateway.outcomes(&model, &layout.app(), &expected);
    assert_outcomes(&expected, &found);
}

/// A definition that names a skill by a file path, on a persistent launch:
/// the gateway declares the file. A member's `apply_patch` does not change
/// it; a new file beside it is written.
#[test]
fn rpc_gateway_declares_the_skill_files_its_definition_names() {
    let mob_id = "host-paths-stdio-skills";
    let model = Model::start();
    let layout = StdioLayout::new();
    let named = layout.outside().join("gateway-config.toml");
    write_file(&named, &model_tables(mob_id, &model.base_url));
    let skill = layout.app().join("roles").join("writer.md");
    write_file(&skill, &skill_text());
    let mut gateway = StdioGateway::boot(
        &mob_toml_with_skill(mob_id, &skill),
        Some(&layout.app().join("state")),
        &layout.outside().join("tmp"),
        &json!({
            "meerkat_config_path": named,
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
        }),
    );
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Change("roles/writer.md"), "refused"),
        (Call::Add("roles/member-note.md"), "written"),
    ];
    let found = gateway.outcomes(&model, &layout.app(), &expected);
    assert_outcomes(&expected, &found);
}

/// The gateway saves its access file by writing a sibling,
/// `<file name>.tmp`, and renaming it over the file: it declares that
/// sibling with the file. A member's `apply_patch` adds no file at the
/// sibling's path.
#[test]
fn rpc_gateway_declares_the_sibling_its_access_file_is_saved_through() {
    let mob_id = "host-paths-stdio-access-save";
    let model = Model::start();
    let layout = StdioLayout::new();
    let named = layout.outside().join("gateway-config.toml");
    write_file(&named, &model_tables(mob_id, &model.base_url));
    let access = layout.app().join("access.toml");
    write_file(&access, &access_toml());
    let mut gateway = StdioGateway::boot(
        &mob_toml(mob_id),
        Some(&layout.app().join("state")),
        &layout.outside().join("tmp"),
        &json!({
            "meerkat_config_path": named,
            "access_config_path": access,
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
        }),
    );
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add("access.toml.tmp"), "refused"),
    ];
    let found = gateway.outcomes(&model, &layout.app(), &expected);
    assert_outcomes(&expected, &found);
}

/// A launch whose state directory lies inside the directory of the native
/// realm it names is refused at its start: the members' file tools are
/// rooted in the directory that holds the state directory, and the gateway
/// starts only where that root is apart from the realms root. The refusal
/// names both directories and what to change.
#[test]
fn rpc_gateway_refuses_a_state_directory_inside_its_native_realms_directory() {
    let mob_id = "host-paths-stdio-state-in-realm";
    let model = Model::start();
    let layout = StdioLayout::new();
    let user_root = layout.outside().join("user");
    write_file(
        &user_root.join(".rkat").join("config.toml"),
        &model_tables(mob_id, &model.base_url),
    );
    let context_root = layout.outside().join("context");
    let realms_root = layout.app().join("realms");
    let realm_directory = realms_root.join("global");
    for directory in [&context_root, &realm_directory] {
        std::fs::create_dir_all(directory).expect("create the realm door's directory");
    }
    let message = StdioGateway::refused_boot(
        &mob_toml(mob_id),
        &realm_directory.join("state"),
        &layout.outside().join("tmp"),
        &json!({
            "native_realm": {
                "realm_id": "global",
                "context_root": context_root,
                "state_root": realms_root,
                "user_config_root": user_root,
            },
            "member_auth_binding": { "realm": "global", "binding": "loop" },
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
        }),
    );
    assert!(
        names_layout(&message, &realm_directory, &realms_root)
            && message.contains("persistent_state")
            && message.contains("native_realm"),
        "the refusal names both directories and what to change: {message}"
    );
}

/// A launch that names a native realm and keeps no state directory starts
/// also where the process's temporary directory lies inside the realms
/// root: its members' file tools are rooted in a workspace of the launch's
/// own, which holds no file of the realm. A member's file lies in that
/// workspace.
#[test]
fn rpc_gateway_without_a_state_directory_starts_with_its_temporary_directory_in_the_realms_root() {
    let mob_id = "host-paths-stdio-realm-temp";
    let model = Model::start();
    let layout = StdioLayout::new();
    let user_root = layout.outside().join("user");
    write_file(
        &user_root.join(".rkat").join("config.toml"),
        &model_tables(mob_id, &model.base_url),
    );
    let context_root = layout.outside().join("context");
    let realms_root = layout.app().join("realms");
    let temp = realms_root.join("tmp");
    for directory in [&context_root, &temp] {
        std::fs::create_dir_all(directory).expect("create the realm door's directory");
    }
    let mut gateway = StdioGateway::boot(
        &mob_toml(mob_id),
        None,
        &temp,
        &json!({
            "native_realm": {
                "realm_id": "global",
                "context_root": context_root,
                "state_root": realms_root,
                "user_config_root": user_root,
            },
            "member_auth_binding": { "realm": "global", "binding": "loop" },
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
        }),
    );
    let answer = gateway.turn(&model, "t0", Call::Add("member-note.txt"));
    let found = files_named(&temp, "member-note.txt");
    let in_the_workspace = found.len() == 1
        && found[0]
            .components()
            .nth(1)
            .is_some_and(|component| component.as_os_str() == "workspace");
    assert!(
        in_the_workspace,
        "the launch starts and the member's file lies in its own workspace: found {found:?}; \
         the tool answered: {answer}"
    );
}

/// The message of the refusal a persistent launch gets whose
/// `runtime_options` name `kept`, built from the directory that holds its
/// state directory, as a directory of its state; and the launch's layout,
/// whose app directory is that directory. The layout is returned so that
/// its directories exist while the row compares the message with them.
fn stdio_refusal_for(mob_id: &str, kept: impl Fn(&Path) -> Value) -> (String, StdioLayout) {
    let model = Model::start();
    let layout = StdioLayout::new();
    let named = layout.outside().join("gateway-config.toml");
    write_file(&named, &model_tables(mob_id, &model.base_url));
    let mut runtime_options = json!({
        "meerkat_config_path": named,
        "identity_bootstrap_mode": { "mode": "eager_materialize" },
    });
    for (key, value) in kept(&layout.app()).as_object().expect("the named options") {
        runtime_options[key] = value.clone();
    }
    let message = StdioGateway::refused_boot(
        &mob_toml(mob_id),
        &layout.app().join("state"),
        &layout.outside().join("tmp"),
        &runtime_options,
    );
    (message, layout)
}

/// A launch whose provider credentials root is the directory that holds
/// its state directory is refused at its start: the members' file tools
/// would be rooted at the credentials root. The refusal names both
/// directories and what to change.
#[test]
fn rpc_gateway_refuses_a_credentials_root_that_holds_its_state_directory() {
    let (message, layout) = stdio_refusal_for(
        "host-paths-stdio-credentials-root",
        |app| json!({ "provider_credentials_root": app }),
    );
    let root = layout.app();
    assert!(
        names_layout(&message, &root, &root)
            && message.contains("persistent_state")
            && message.contains("provider_credentials_root"),
        "the refusal names both directories and what to change: {message}"
    );
}

/// A launch whose durable work graph directory is the directory that holds
/// its state directory is refused at its start, in the same words.
#[test]
fn rpc_gateway_refuses_a_work_graph_directory_that_holds_its_state_directory() {
    let (message, layout) = stdio_refusal_for(
        "host-paths-stdio-workgraph-root",
        |app| json!({ "workgraph": app }),
    );
    let root = layout.app();
    assert!(
        names_layout(&message, &root, &root)
            && message.contains("persistent_state")
            && message.contains("workgraph"),
        "the refusal names both directories and what to change: {message}"
    );
}

/// A persistent launch whose home directory lies beside the state
/// directory, with no credentials root and no session identity root named:
/// the provider credential store and the members' session identities are
/// kept in their per-user default places under that home, and the gateway
/// declares both. A member's `apply_patch` adds no file in either.
#[cfg(target_os = "linux")]
#[test]
fn rpc_gateway_declares_the_per_user_default_stores_under_its_home() {
    let mob_id = "host-paths-stdio-home";
    let model = Model::start();
    let layout = StdioLayout::new();
    let named = layout.outside().join("gateway-config.toml");
    write_file(&named, &model_tables(mob_id, &model.base_url));
    let mut gateway = StdioGateway::boot_at_home(
        &mob_toml(mob_id),
        Some(&layout.app().join("state")),
        &layout.outside().join("tmp"),
        Some(&layout.app().join("home")),
        &json!({
            "meerkat_config_path": named,
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
        }),
    );
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (
            Call::Add("home/.config/meerkat/credentials/member-note.txt"),
            "refused",
        ),
        (
            Call::Add("home/.local/state/meerkat/session_comms_identity/member-note.txt"),
            "refused",
        ),
    ];
    let found = gateway.outcomes(&model, &layout.app(), &expected);
    assert_outcomes(&expected, &found);
}

/// Every file named `name` under `directory`, by its path below it.
fn files_named(directory: &Path, name: &str) -> Vec<PathBuf> {
    fn walk(directory: &Path, below: &Path, name: &str, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let below = below.join(entry.file_name());
            if path.is_dir() {
                walk(&path, &below, name, found);
            } else if entry.file_name() == name {
                found.push(below);
            }
        }
    }
    let mut found = Vec::new();
    walk(directory, Path::new(""), name, &mut found);
    found.sort();
    found
}

/// One `rpc_gateway` launch that keeps no state directory, with the
/// process's temporary directory set to the row's own, `temp`.
fn stdio_gateway_without_a_state_directory(
    mob_id: &str,
    model: &Model,
    temp: &Path,
) -> StdioGateway {
    let named = temp
        .parent()
        .expect("the row's scratch directory")
        .join("outside")
        .join("gateway-config.toml");
    write_file(&named, &model_tables(mob_id, &model.base_url));
    StdioGateway::boot(
        &mob_toml(mob_id),
        None,
        temp,
        &json!({
            "meerkat_config_path": named,
            "identity_bootstrap_mode": { "mode": "eager_materialize" },
        }),
    )
}

/// A launch that keeps no state directory makes a temporary directory of
/// its own for the run and roots its members' file tools in a workspace
/// inside it: a file a member adds by a relative path lies in that
/// workspace and nowhere else in the process's temporary directory.
#[test]
fn rpc_gateway_without_a_state_directory_roots_its_members_file_tools_in_its_own_workspace() {
    let model = Model::start();
    let layout = StdioLayout::new();
    let temp = layout.app();
    let mut gateway =
        stdio_gateway_without_a_state_directory("host-paths-stdio-own", &model, &temp);
    let answer = gateway.turn(&model, "t0", Call::Add("member-note.txt"));
    let found = files_named(&temp, "member-note.txt");
    let in_the_workspace = found.len() == 1
        && found[0].components().count() == 3
        && found[0]
            .components()
            .nth(1)
            .is_some_and(|component| component.as_os_str() == "workspace");
    assert!(
        in_the_workspace,
        "the member's file lies in the gateway's own workspace and nowhere else in the \
         temporary directory: found {found:?}; the tool answered: {answer}"
    );
}

/// The same launch: a path in the process's temporary directory outside
/// the gateway's own workspace is out of the file tools' reach. A member's
/// `apply_patch` that names such a path is refused and adds no file.
#[test]
fn rpc_gateway_without_a_state_directory_keeps_the_temporary_directory_out_of_reach() {
    let model = Model::start();
    let layout = StdioLayout::new();
    let temp = layout.app();
    let mut gateway =
        stdio_gateway_without_a_state_directory("host-paths-stdio-out", &model, &temp);
    let beside = temp.join("other").join("member-note.txt");
    let named: &'static str = Box::leak(beside.display().to_string().into_boxed_str());
    let answer = gateway.turn(&model, "t0", Call::Add(named));
    assert!(
        answer.contains("escapes the project root") && !beside.exists(),
        "a path beside the gateway's own directory is refused and not written: the file \
         exists: {}; the tool answered: {answer}",
        beside.exists()
    );
}

/// The same launch: the members' workspace goes with the gateway's own
/// temporary directory at its exit, so a file a member added by a relative
/// path does not outlive the gateway.
#[test]
fn rpc_gateway_without_a_state_directory_removes_its_members_workspace_at_its_exit() {
    let model = Model::start();
    let layout = StdioLayout::new();
    let temp = layout.app();
    let mut gateway =
        stdio_gateway_without_a_state_directory("host-paths-stdio-exit", &model, &temp);
    let answer = gateway.turn(&model, "t0", Call::Add("member-note.txt"));
    gateway.shut_down();
    let left = files_named(&temp, "member-note.txt");
    assert!(
        left.is_empty(),
        "the member's file goes with the gateway's own temporary directory at its exit: \
         left {left:?}; the tool answered: {answer}"
    );
}

/// The same launch keeps its agent store in a directory of the members'
/// workspace and declares it: a member's `apply_patch` adds no file in it,
/// a `.rkat` directory in the workspace is refused, and an ordinary file
/// is written there.
#[test]
fn rpc_gateway_without_a_state_directory_declares_its_store_in_the_members_workspace() {
    let model = Model::start();
    let layout = StdioLayout::new();
    let temp = layout.app();
    let mut gateway =
        stdio_gateway_without_a_state_directory("host-paths-stdio-local", &model, &temp);
    // The gateway's own temporary directory is the one directory of the
    // process's temporary directory that holds a workspace.
    let workspaces: Vec<PathBuf> = std::fs::read_dir(&temp)
        .expect("read the temporary directory")
        .flatten()
        .map(|entry| entry.path().join("workspace"))
        .filter(|workspace| workspace.is_dir())
        .collect();
    assert_eq!(
        workspaces.len(),
        1,
        "one members' workspace: {workspaces:?}"
    );
    let expected = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add(".gateway-store/member-note.txt"), "refused"),
    ];
    let found = gateway.outcomes(&model, &workspaces[0], &expected);
    assert_outcomes(&expected, &found);
}

// ---------------------------------------------------------------------------
// Library builder
// ---------------------------------------------------------------------------

/// What a builder row gives the runtime it builds, beside its state
/// directory.
#[derive(Debug, Clone, Copy, Default)]
struct BuilderExtras {
    /// An access file, `<app>/access.toml`.
    access_file: bool,
    /// A definition whose profile reads a skill from `<app>/roles/writer.md`.
    skill_file: bool,
}

/// What becomes of each call of `expected` on a runtime built with
/// `UnifiedRuntimeBuilder` whose state directory is `<app>/state`, with
/// `extras`. The member's file tools are rooted in `<app>`.
fn builder_outcomes(
    mob_id: &str,
    extras: BuilderExtras,
    expected: &'static Expected,
) -> Vec<String> {
    let mob_id = mob_id.to_owned();
    // The build and the member's turns run on threads with the hosts' stack
    // budget, not the test thread's.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .thread_stack_size(16 * 1024 * 1024)
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(async move {
                    let model = Model::start();
                    let scratch = TempDir::new().expect("scratch");
                    let app = scratch.path().join("app");
                    std::fs::create_dir_all(&app).expect("create the app directory");
                    let named = scratch.path().join("outside").join("runtime-config.toml");
                    write_file(&named, &model_tables(&mob_id, &model.base_url));
                    let config =
                        meerkat_meta::gateway_composition::load_gateway_host_config(&named)
                            .expect("the runtime's configuration");
                    let mob = if extras.skill_file {
                        let skill = app.join("roles").join("writer.md");
                        write_file(&skill, &skill_text());
                        mob_toml_with_skill(&mob_id, &skill)
                    } else {
                        mob_toml(&mob_id)
                    };
                    let definition =
                        meerkat_mob::MobDefinition::from_toml(&mob).expect("the mob definition");
                    let builder = meerkat_meta::UnifiedRuntime::builder()
                        .definition(definition)
                        .meerkat_config(config)
                        .persistent_state(app.join("state"));
                    let builder = if extras.access_file {
                        let access = app.join("access.toml");
                        write_file(&access, &access_toml());
                        builder
                            .access_control_file(access)
                            .expect("the access file")
                    } else {
                        builder
                    };
                    let runtime = Box::pin(builder.build()).await.expect("build the runtime");
                    runtime
                        .reconcile(vec![meerkat_mob::SpawnMemberSpec::from_wire(
                            "writer".to_string(),
                            "writer-1".to_string(),
                            None,
                            None,
                            None,
                        )])
                        .await
                        .expect("spawn the member");
                    let mut found = Vec::new();
                    for (index, (call, _)) in expected.iter().enumerate() {
                        let tag = format!("t{index}");
                        meerkat_meta::send_message_on_mob(
                            &runtime.mob_handle(),
                            "writer-1",
                            call.content(&tag),
                        )
                        .await
                        .expect("send to the member");
                        let start = Instant::now();
                        while model.tool_results(&tag).len() < call.steps() {
                            assert!(
                                start.elapsed() < SETTLE,
                                "the turn {tag} returned no tool result to the model: {}",
                                model.seen(&tag)
                            );
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        let result = model.tool_results(&tag).remove(call.steps() - 1);
                        found.push(call.outcome(&app, &result));
                    }
                    let _ = runtime.shutdown().await;
                    found
                })
        })
        .expect("spawn the row's thread")
        .join()
        .expect("the row's thread")
}

/// A library-built runtime with a state directory declares it: a member's
/// `apply_patch` adds no file in it.
#[test]
fn builder_declares_its_state_directory_to_a_members_file_tools() {
    static EXPECTED: [(Call, &str); 3] = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add("state/member-note.txt"), "refused"),
    ];
    let found = builder_outcomes("host-paths-builder", BuilderExtras::default(), &EXPECTED);
    assert_outcomes(&EXPECTED, &found);
}

/// A library-built runtime that is given an access file declares it: a
/// member's `apply_patch` does not change it.
#[test]
fn builder_declares_the_access_file_it_is_given() {
    static EXPECTED: [(Call, &str); 3] = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Change("access.toml"), "refused"),
    ];
    let found = builder_outcomes(
        "host-paths-builder-access",
        BuilderExtras {
            access_file: true,
            skill_file: false,
        },
        &EXPECTED,
    );
    assert_outcomes(&EXPECTED, &found);
}

/// A library-built runtime whose definition names a skill by a file path
/// declares the file: a member's `apply_patch` does not change it.
#[test]
fn builder_declares_the_skill_files_its_definition_names() {
    static EXPECTED: [(Call, &str); 3] = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Change("roles/writer.md"), "refused"),
    ];
    let found = builder_outcomes(
        "host-paths-builder-skills",
        BuilderExtras {
            access_file: false,
            skill_file: true,
        },
        &EXPECTED,
    );
    assert_outcomes(&EXPECTED, &found);
}

/// A library-built runtime that is given an access file declares the
/// sibling a save of that file is written through, `<file name>.tmp`: a
/// member's `apply_patch` adds no file at the sibling's path.
#[test]
fn builder_declares_the_sibling_its_access_file_is_saved_through() {
    static EXPECTED: [(Call, &str); 3] = [
        CONTROLS[0],
        CONTROLS[1],
        (Call::Add("access.toml.tmp"), "refused"),
    ];
    let found = builder_outcomes(
        "host-paths-builder-access-save",
        BuilderExtras {
            access_file: true,
            skill_file: false,
        },
        &EXPECTED,
    );
    assert_outcomes(&EXPECTED, &found);
}
