//! Deterministic replay of a recorded gpt-live-1 provider stream.
//!
//! A fixture is a scrubbed `provider-stream.jsonl` (see
//! `tests/integration/fixtures/gpt_live_replay/README.md`): every crossing of
//! the public Live adapter boundary of one green live run, in causal order.
//! [`Cassette`] serves it as a local public Live API (`POST
//! /v1/live/sessions`, the `.../attach` sideband websocket) that the broker
//! reaches through `ExperimentalGptLiveOpenAuthority::with_test_base_url`.
//!
//! Replay is causal, never timed. Each channel's tape is walked in recorded
//! order:
//! - a server frame is sent;
//! - a client event recorded before the next frame is awaited: the frame
//!   after it is served only once Meerkat sent an event of the same type and
//!   deterministic `event_id` (early arrival counts; payload bytes are not
//!   compared);
//! - a marker (a test-driven step such as `play_at:<fixture>` or
//!   `disconnect:graceful`) is awaited until the replaying test reaches it
//!   ([`Cassette::release`]);
//! - the receiver end closes the sideband, as the provider did.
//!
//! Client events recorded after the receiver end were sent on a dead socket;
//! they are not gated (the socket is gone), and the replaying test compares
//! them through its own recording instead.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, State as AxumState};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use futures::{SinkExt as _, StreamExt as _};
use meerkat::experimental_gpt_live::provider_recording::{Entry, Line};
use serde_json::Value;
use tokio::sync::watch;
use tokio::time::{Duration, timeout};

/// How long a replaying test waits for the tape to reach a step before it
/// reports where the replay is parked instead. A safety bound for a broken
/// replay, never an ordering device: every wait is on a typed tape state.
const STEP_BOUND: Duration = Duration::from_secs(60);

/// A client event's replay identity: its type and deterministic `event_id`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClientKey {
    pub kind: String,
    pub event_id: Option<String>,
}

impl ClientKey {
    pub fn of(event: &Value) -> Self {
        Self {
            kind: event["type"].as_str().unwrap_or_default().to_owned(),
            event_id: event["event_id"].as_str().map(str::to_owned),
        }
    }
}

impl std::fmt::Display for ClientKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.event_id {
            Some(id) => write!(f, "{}#{id}", self.kind),
            None => f.write_str(&self.kind),
        }
    }
}

#[derive(Clone, Debug)]
enum Step {
    Frame(Value),
    Await(ClientKey),
    Marker(String),
    End,
}

/// One recorded channel: its create exchange and its sideband tape.
#[derive(Clone, Debug)]
pub struct ChannelTape {
    pub ordinal: u32,
    pub create_request: Value,
    create_response: Value,
    steps: Vec<Step>,
    /// Every client event of the channel in recorded order, including those
    /// sent after the receiver end.
    pub client_events: Vec<ClientKey>,
}

/// A loaded fixture: its channels in the order they were created.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub channels: Vec<ChannelTape>,
    pub lines: Vec<Line>,
}

impl Fixture {
    /// Parse a fixture's JSONL text (embedded with `include_str!`).
    pub fn parse(text: &str) -> Result<Self, String> {
        let lines = text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(number, line)| {
                serde_json::from_str::<Line>(line)
                    .map_err(|error| format!("fixture line {}: {error}", number + 1))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut by_channel: BTreeMap<u32, Vec<&Line>> = BTreeMap::new();
        for line in &lines {
            by_channel
                .entry(line.channel_ordinal)
                .or_default()
                .push(line);
        }
        let mut channels = Vec::new();
        for (ordinal, lines) in by_channel {
            let mut create_request = None;
            let mut create_response = None;
            let mut steps = Vec::new();
            let mut client_events = Vec::new();
            let mut ended = false;
            for line in lines {
                match &line.entry {
                    Entry::CreateRequest { body } => create_request = Some(body.clone()),
                    Entry::CreateResponse { body } => create_response = Some(body.clone()),
                    Entry::ServerFrame { raw } if !ended => steps.push(Step::Frame(raw.clone())),
                    Entry::ServerFrame { .. } => {
                        return Err(format!("channel {ordinal}: a server frame after its end"));
                    }
                    Entry::ClientEvent { event } => {
                        let key = ClientKey::of(event);
                        client_events.push(key.clone());
                        if !ended {
                            steps.push(Step::Await(key));
                        }
                    }
                    Entry::Marker { step } if !ended => steps.push(Step::Marker(step.clone())),
                    Entry::Marker { .. } => {}
                    Entry::ReceiverEnd { .. } => {
                        steps.push(Step::End);
                        ended = true;
                    }
                }
            }
            if !ended {
                steps.push(Step::End);
            }
            channels.push(ChannelTape {
                ordinal,
                create_request: create_request
                    .ok_or_else(|| format!("channel {ordinal} has no create request"))?,
                create_response: create_response
                    .ok_or_else(|| format!("channel {ordinal} has no create response"))?,
                steps,
                client_events,
            });
        }
        // Channels are created in recorded order; ordinals follow it.
        Ok(Self { channels, lines })
    }

    /// The text of the recorded client events of `kind` on `ordinal`, in
    /// order (`content` of commentary/thinking appends).
    pub fn client_contents(&self, ordinal: u32, kind: &str) -> Vec<String> {
        self.lines
            .iter()
            .filter(|line| line.channel_ordinal == ordinal)
            .filter_map(|line| match &line.entry {
                Entry::ClientEvent { event } if event["type"] == kind => {
                    event["content"].as_str().map(str::to_owned)
                }
                _ => None,
            })
            .collect()
    }
}

/// Where one channel's sideband replay currently waits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parked {
    NotCreated,
    Created,
    Serving,
    Client(ClientKey),
    Marker(String),
    Ended,
}

#[derive(Debug, Default)]
struct TapeState {
    next_create: usize,
    sessions: BTreeMap<String, usize>,
    parked: BTreeMap<u32, Parked>,
    received: BTreeMap<u32, Vec<ClientKey>>,
    released: BTreeMap<u32, VecDeque<String>>,
    create_bodies: Vec<Value>,
    divergences: Vec<String>,
}

struct Shared {
    fixture: Fixture,
    state: Mutex<TapeState>,
    changed: watch::Sender<u64>,
}

impl Shared {
    fn update<T>(&self, change: impl FnOnce(&mut TapeState) -> T) -> T {
        let value = {
            let mut state = self.state.lock().expect("cassette state");
            change(&mut state)
        };
        self.changed.send_modify(|version| *version += 1);
        value
    }

    fn read<T>(&self, read: impl FnOnce(&TapeState) -> T) -> T {
        read(&self.state.lock().expect("cassette state"))
    }

    /// Wait until `ready` holds over the state; every state change wakes it.
    async fn wait_until(&self, ready: impl Fn(&TapeState) -> bool) {
        let mut changed = self.changed.subscribe();
        loop {
            if self.read(&ready) {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    }
}

/// A running replay server over one fixture.
pub struct Cassette {
    shared: Arc<Shared>,
    base_url: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Cassette {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Cassette {
    pub async fn start(fixture: Fixture) -> Result<Self, String> {
        let mut state = TapeState::default();
        for channel in &fixture.channels {
            state.parked.insert(channel.ordinal, Parked::NotCreated);
        }
        let (changed, _) = watch::channel(0);
        let shared = Arc::new(Shared {
            fixture,
            state: Mutex::new(state),
            changed,
        });
        let app = Router::new()
            .route("/v1/live/sessions", post(create_session))
            .route("/v1/live/sessions/{session_id}/attach", get(attach))
            .with_state(Arc::clone(&shared));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("bind replay listener: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("replay address: {error}"))?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            shared,
            base_url: format!("http://{address}/v1/"),
            server,
        })
    }

    /// The API root to hand `with_test_base_url`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Where `ordinal`'s replay waits now.
    pub fn parked(&self, ordinal: u32) -> Parked {
        self.shared.read(|state| {
            state
                .parked
                .get(&ordinal)
                .cloned()
                .unwrap_or(Parked::NotCreated)
        })
    }

    /// The replaying test reached `step` on channel `ordinal`: wait until the
    /// tape is parked at exactly that marker (every frame recorded before it
    /// has been served), then let it continue.
    pub async fn release(&self, ordinal: u32, step: &str) -> Result<(), String> {
        let want = Parked::Marker(step.to_owned());
        let reached = timeout(
            STEP_BOUND,
            self.shared.wait_until(|state| {
                state.parked.get(&ordinal) == Some(&want) || !state.divergences.is_empty()
            }),
        )
        .await;
        self.diverged()?;
        if reached.is_err() {
            return Err(format!(
                "channel {ordinal} never reached step {step:?}; the replay is parked at {:?}",
                self.parked(ordinal)
            ));
        }
        self.shared.update(|state| {
            state
                .released
                .entry(ordinal)
                .or_default()
                .push_back(step.to_owned());
        });
        Ok(())
    }

    /// Wait until channel `ordinal`'s tape has ended (the provider closed).
    pub async fn ended(&self, ordinal: u32) -> Result<(), String> {
        let reached = timeout(
            STEP_BOUND,
            self.shared.wait_until(|state| {
                state.parked.get(&ordinal) == Some(&Parked::Ended) || !state.divergences.is_empty()
            }),
        )
        .await;
        self.diverged()?;
        reached.map_err(|_| {
            format!(
                "channel {ordinal}'s tape never ended; the replay is parked at {:?}",
                self.parked(ordinal)
            )
        })
    }

    /// Every create body Meerkat sent, in order.
    pub fn create_bodies(&self) -> Vec<Value> {
        self.shared.read(|state| state.create_bodies.clone())
    }

    /// The first divergence from the recording, if any.
    pub fn diverged(&self) -> Result<(), String> {
        self.shared.read(|state| match state.divergences.first() {
            Some(divergence) => Err(divergence.clone()),
            None => Ok(()),
        })
    }
}

/// Expand a scrubbed `<silence-bytes:N>` audio payload to N zero bytes.
fn expand_silence(mut frame: Value) -> Value {
    let is_audio = frame["type"]
        .as_str()
        .is_some_and(|kind| kind.contains("audio"));
    if !is_audio {
        return frame;
    }
    for field in ["audio", "delta"] {
        let Some(text) = frame[field].as_str() else {
            continue;
        };
        if let Some(size) = text
            .strip_prefix("<silence-bytes:")
            .and_then(|rest| rest.strip_suffix('>'))
            .and_then(|size| size.parse::<usize>().ok())
        {
            frame[field] =
                Value::String(base64::engine::general_purpose::STANDARD.encode(vec![0_u8; size]));
        }
    }
    frame
}

async fn create_session(AxumState(shared): AxumState<Arc<Shared>>, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let created = shared.update(|state| {
        state.create_bodies.push(body);
        let index = state.next_create;
        let Some(tape) = shared.fixture.channels.get(index) else {
            state.divergences.push(format!(
                "Meerkat created channel #{} but the recording has only {}",
                index + 1,
                shared.fixture.channels.len()
            ));
            return None;
        };
        state.next_create += 1;
        if let Some(id) = tape.create_response["session"]["id"].as_str() {
            state.sessions.insert(id.to_owned(), index);
        }
        state.parked.insert(tape.ordinal, Parked::Created);
        Some(tape.create_response.clone())
    });
    match created {
        Some(response) => (
            StatusCode::CREATED,
            [("content-type", "application/json")],
            response.to_string(),
        )
            .into_response(),
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn attach(
    AxumState(shared): AxumState<Arc<Shared>>,
    AxumPath(session_id): AxumPath<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(index) = shared.read(|state| state.sessions.get(&session_id).copied()) else {
        shared.update(|state| {
            state
                .divergences
                .push(format!("attach for unknown session {session_id}"));
        });
        return StatusCode::NOT_FOUND.into_response();
    };
    upgrade.on_upgrade(move |socket| serve_tape(socket, shared, index))
}

async fn serve_tape(socket: WebSocket, shared: Arc<Shared>, index: usize) {
    let tape = shared.fixture.channels[index].clone();
    let ordinal = tape.ordinal;
    let (mut sink, mut stream) = socket.split();
    // Client events, read concurrently so early ones count.
    let reader_shared = Arc::clone(&shared);
    let expected = tape.client_events.clone();
    let reader = tokio::spawn(async move {
        while let Some(Ok(message)) = stream.next().await {
            let WsMessage::Text(text) = message else {
                continue;
            };
            let Ok(event) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let key = ClientKey::of(&event);
            reader_shared.update(|state| {
                if !expected.contains(&key) {
                    state.divergences.push(format!(
                        "channel {ordinal}: Meerkat sent {key}, which the recording never sent"
                    ));
                }
                state.received.entry(ordinal).or_default().push(key);
            });
        }
    });
    for step in &tape.steps {
        match step {
            Step::Frame(frame) => {
                shared.update(|state| {
                    state.parked.insert(ordinal, Parked::Serving);
                });
                let text = expand_silence(frame.clone()).to_string();
                if sink.send(WsMessage::Text(text.into())).await.is_err() {
                    shared.update(|state| {
                        state.divergences.push(format!(
                            "channel {ordinal}: Meerkat left the sideband before the recording's end"
                        ));
                    });
                    break;
                }
            }
            Step::Await(key) => {
                shared.update(|state| {
                    state.parked.insert(ordinal, Parked::Client(key.clone()));
                });
                let key = key.clone();
                shared
                    .wait_until(|state| {
                        state
                            .received
                            .get(&ordinal)
                            .is_some_and(|received| received.contains(&key))
                    })
                    .await;
            }
            Step::Marker(step) => {
                shared.update(|state| {
                    state.parked.insert(ordinal, Parked::Marker(step.clone()));
                });
                let step = step.clone();
                shared
                    .wait_until(|state| {
                        state
                            .released
                            .get(&ordinal)
                            .is_some_and(|released| released.contains(&step))
                    })
                    .await;
            }
            Step::End => break,
        }
    }
    let _ = sink.send(WsMessage::Close(None)).await;
    drop(sink);
    reader.abort();
    shared.update(|state| {
        state.parked.insert(ordinal, Parked::Ended);
    });
}

/// Rules a committed fixture must pass (the same ones as
/// `scripts/gpt-live-scrub-provider-stream check`): no credential-shaped
/// value, no SDP or host detail, and voice audio only as silence
/// placeholders. Returns `line: rule` findings without the matched text.
pub fn fixture_findings(text: &str) -> Vec<String> {
    let mut findings = Vec::new();
    for (number, raw) in text.lines().enumerate() {
        let number = number + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let Ok(line) = serde_json::from_str::<Value>(raw) else {
            findings.push(format!("{number}: not-json"));
            continue;
        };
        let mut strings = Vec::new();
        collect_strings(&line, None, &mut strings);
        for (key, value) in strings {
            for rule in value_rules(key.as_deref(), value) {
                findings.push(format!("{number}: {rule}"));
            }
        }
        let frame = &line["entry"]["raw"];
        if frame["type"]
            .as_str()
            .is_some_and(|kind| kind.contains("audio"))
        {
            for field in ["audio", "delta"] {
                if let Some(value) = frame[field].as_str()
                    && !is_silence_placeholder(value)
                {
                    findings.push(format!("{number}: voice-audio"));
                }
            }
        }
    }
    findings
}

const REDACTED_KEYS: &[&str] = &[
    "sdp",
    "client_secret",
    "authorization",
    "api_key",
    "apikey",
    "token",
    "access_token",
    "refresh_token",
    "ephemeral_key",
    "secret",
    "password",
    "cookie",
    "set-cookie",
];

fn collect_strings<'a>(
    value: &'a Value,
    key: Option<&'a str>,
    out: &mut Vec<(Option<String>, &'a str)>,
) {
    match value {
        Value::String(text) => out.push((key.map(str::to_ascii_lowercase), text)),
        Value::Array(items) => items
            .iter()
            .for_each(|item| collect_strings(item, key, out)),
        Value::Object(map) => {
            for (child_key, child) in map {
                if REDACTED_KEYS.contains(&child_key.to_ascii_lowercase().as_str())
                    && !child.is_null()
                    && !child
                        .as_str()
                        .is_some_and(|text| text.starts_with("<redacted-"))
                {
                    out.push((Some(child_key.to_ascii_lowercase()), "<unredacted-key>"));
                    continue;
                }
                collect_strings(child, Some(child_key), out);
            }
        }
        _ => {}
    }
}

fn is_silence_placeholder(value: &str) -> bool {
    value
        .strip_prefix("<silence-bytes:")
        .and_then(|rest| rest.strip_suffix('>'))
        .is_some_and(|size| !size.is_empty() && size.bytes().all(|byte| byte.is_ascii_digit()))
}

fn value_rules(key: Option<&str>, value: &str) -> Vec<&'static str> {
    let mut rules = Vec::new();
    if value == "<unredacted-key>" {
        rules.push(match key {
            Some("sdp") => "key:sdp",
            _ => "key:credential",
        });
        return rules;
    }
    let has = |needle: &str| value.contains(needle);
    let token_after = |prefix: &str, min: usize| {
        value.match_indices(prefix).any(|(at, _)| {
            value[at + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .count()
                >= min
        })
    };
    if token_after("sk-", 8) {
        rules.push("openai-key");
    }
    if token_after("ek_", 8) {
        rules.push("ephemeral-key");
    }
    if value
        .to_ascii_lowercase()
        .match_indices("bearer ")
        .any(|(at, _)| {
            value[at + 7..]
                .chars()
                .take_while(|c| !c.is_whitespace())
                .count()
                >= 8
        })
    {
        rules.push("bearer");
    }
    if has("a=ice-pwd:") || has("a=ice-ufrag:") {
        rules.push("sdp-ice-credential");
    }
    if has("a=candidate:") {
        rules.push("sdp-candidate");
    }
    if has("/home/") || has("/Users/") {
        rules.push("home-path");
    }
    rules
}
