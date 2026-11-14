//! Shared fixtures for the GPT Live browser-peer verticals.
//!
//! Scenario 96 (experimental `gpt-live-1-codex`, ChatGPT OAuth) and scenario
//! 97 (public `gpt-live-1`, OpenAI API key) drive the same RPC, Mob, and
//! browser-peer graph. Only the provider protocol seen on the `oai-events`
//! data channel differs, which [`BrowserPeerProtocol`] selects.
//!
//! This file is `#[path]`-included by both test targets, so every item is
//! public and unused items in one target are expected.
#![allow(dead_code, clippy::disallowed_methods)]

#[path = "gpt_live_evidence.rs"]
pub mod evidence;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use meerkat::experimental_gpt_live::{
    ExperimentalLiveCurrentConfigSource, ExperimentalLiveOpenAuthorityError,
    ExperimentalLiveSessionBindingAuthority, ExperimentalLiveSessionBindingAuthorization,
};
use meerkat_core::{
    ActingOnBehalfOf, AuthBindingRef, AuthBindingUseRequest, AuthGrant, Config, GrantAction,
    GrantScope, PrincipalKind, PrincipalRef,
};
use meerkat_mob_mcp::MobMcpState;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{Duration, Instant, sleep, timeout};

pub fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("MEERKAT_WORKSPACE_ROOT") {
        return PathBuf::from(root);
    }

    let current_dir = std::env::current_dir().expect("current directory");
    current_dir
        .ancestors()
        .find(|candidate| {
            candidate.join("Cargo.toml").is_file()
                && candidate.join("tests/live_smoke/browser").is_dir()
        })
        .expect("Meerkat workspace root")
        .to_path_buf()
}

/// Root for the scenario's temporary directory, honoring the Bazel-style
/// `TEST_TMPDIR` override.
pub fn test_tmp_root() -> std::io::Result<PathBuf> {
    let root = std::env::var_os("TEST_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&root)?;
    Ok(root)
}

#[derive(Clone)]
pub struct FixedConfigSource(pub Config);

#[async_trait]
impl ExperimentalLiveCurrentConfigSource for FixedConfigSource {
    async fn current_config(&self) -> Result<Config, meerkat_core::ConfigError> {
        Ok(self.0.clone())
    }
}

/// Session-bound binding authority: exactly one durable session may use
/// exactly one configured binding, authorized through an explicit grant.
pub struct ExplicitScenarioBindingAuthority {
    pub session_id: meerkat_core::SessionId,
    pub binding: AuthBindingRef,
    pub auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
    pub mobs: Arc<MobMcpState>,
    /// Human principal id recorded on the explicit grant.
    pub principal_id: &'static str,
}

#[async_trait]
impl ExperimentalLiveSessionBindingAuthority for ExplicitScenarioBindingAuthority {
    async fn validate_live_durable_source_availability(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
        if session_id != &self.session_id {
            return Err(ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable);
        }
        let owner = self
            .mobs
            .live_member_owner(session_id)
            .await
            .map_err(|_| ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable)?;
        owner
            .is_some()
            .then_some(())
            .ok_or(ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable)
    }

    async fn authorize_binding_use(
        &self,
        session_id: &meerkat_core::SessionId,
        selected: &AuthBindingRef,
    ) -> Result<ExperimentalLiveSessionBindingAuthorization, ExperimentalLiveOpenAuthorityError>
    {
        if session_id != &self.session_id || selected != &self.binding {
            return Err(ExperimentalLiveOpenAuthorityError::BindingUseDenied);
        }
        let principal = PrincipalRef::new(PrincipalKind::Human, self.principal_id)
            .map_err(|_| ExperimentalLiveOpenAuthorityError::AccessDenied)?;
        let durable_target = PrincipalRef::new(PrincipalKind::PersonalAgent, "voice-executor")
            .map_err(|_| ExperimentalLiveOpenAuthorityError::AccessDenied)?;
        let request =
            AuthBindingUseRequest::new(principal.clone(), durable_target.clone(), selected.clone());
        let grant = AuthGrant {
            principal: principal.clone(),
            scope: GrantScope::AuthBinding {
                realm_id: selected.realm.clone(),
                binding_id: selected.binding.clone(),
                profile_id: selected.profile.clone(),
            },
            actions: BTreeSet::from([GrantAction::UseAuthBinding]),
            acting_on_behalf_of: Some(ActingOnBehalfOf::new(principal, durable_target)),
        };
        let witness = meerkat_core::authorize_explicit_auth_binding_use(&request, &[grant])
            .into_result()
            .map_err(|_| ExperimentalLiveOpenAuthorityError::AccessDenied)?;
        Ok(
            ExperimentalLiveSessionBindingAuthorization::from_machine_authority(
                witness,
                self.auth_lease.clone(),
            ),
        )
    }
}

pub struct JsonlRpcClient {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next_id: i64,
    notifications: VecDeque<Value>,
}

impl JsonlRpcClient {
    pub fn new(stream: DuplexStream) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
            notifications: VecDeque::new(),
        }
    }

    pub async fn call_raw(
        &mut self,
        method: &str,
        params: Value,
        timeout_secs: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params});
        self.writer
            .write_all(request.to_string().as_bytes())
            .await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        loop {
            let mut line = String::new();
            let remaining = deadline.saturating_duration_since(Instant::now());
            let read = timeout(remaining, self.reader.read_line(&mut line))
                .await
                .map_err(|_| {
                    format!("timed out after {timeout_secs}s waiting for the RPC reply to {method}")
                })??;
            if read == 0 {
                return Err("RPC server closed".into());
            }
            let message: Value = serde_json::from_str(line.trim())?;
            if message["id"].as_i64() != Some(id) {
                if message["method"].is_string() {
                    self.notifications.push_back(message);
                }
                continue;
            }
            return Ok(message);
        }
    }

    fn take_queued_notification(&mut self, method: &str) -> Option<Value> {
        let index = self
            .notifications
            .iter()
            .position(|message| message["method"].as_str() == Some(method))?;
        Some(
            self.notifications
                .remove(index)
                .expect("indexed notification exists")["params"]
                .clone(),
        )
    }

    pub async fn wait_for_notification(
        &mut self,
        method: &str,
        timeout_secs: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        if let Some(params) = self.take_queued_notification(method) {
            return Ok(params);
        }
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        loop {
            let mut line = String::new();
            let remaining = deadline.saturating_duration_since(Instant::now());
            let read = match timeout(remaining, self.reader.read_line(&mut line)).await {
                Ok(read) => read?,
                Err(_) => {
                    let seen: Vec<String> = self
                        .notifications
                        .iter()
                        .filter_map(|message| message["method"].as_str().map(str::to_string))
                        .collect();
                    return Err(format!(
                        "timed out after {timeout_secs}s waiting for RPC notification {method}; \
                         other notifications queued meanwhile: {seen:?}"
                    )
                    .into());
                }
            };
            if read == 0 {
                return Err("RPC server closed while awaiting notification".into());
            }
            let message: Value = serde_json::from_str(line.trim())?;
            if message["method"].as_str() == Some(method) {
                return Ok(message["params"].clone());
            }
            if message["method"].is_string() {
                self.notifications.push_back(message);
            }
        }
    }

    /// Like [`Self::wait_for_notification`] but returns `None` instead of an
    /// error when nothing arrives within the wait window, so callers can
    /// interleave notification draining with other polling.
    pub async fn poll_notification(
        &mut self,
        method: &str,
        wait: Duration,
    ) -> Result<Option<Value>, Box<dyn std::error::Error>> {
        if let Some(params) = self.take_queued_notification(method) {
            return Ok(Some(params));
        }
        let deadline = Instant::now() + wait;
        loop {
            let mut line = String::new();
            let remaining = deadline.saturating_duration_since(Instant::now());
            match timeout(remaining, self.reader.read_line(&mut line)).await {
                Err(_) => return Ok(None),
                Ok(read) => {
                    if read? == 0 {
                        return Err("RPC server closed while polling notifications".into());
                    }
                }
            }
            let message: Value = serde_json::from_str(line.trim())?;
            if message["method"].as_str() == Some(method) {
                return Ok(Some(message["params"].clone()));
            }
            if message["method"].is_string() {
                self.notifications.push_back(message);
            }
        }
    }

    pub async fn call(
        &mut self,
        method: &str,
        params: Value,
        timeout_secs: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let response = self.call_raw(method, params, timeout_secs).await?;
        if !response["error"].is_null() {
            let code = response["error"]["code"].as_i64().unwrap_or_default();
            let message = response["error"]["message"]
                .as_str()
                .unwrap_or("unspecified RPC error");
            return Err(format!("RPC {method} failed with code {code}: {message}").into());
        }
        Ok(response["result"].clone())
    }

    /// The whole of a session's `session/history`: pages of
    /// [`SESSION_HISTORY_PAGE`] rows read by offset until a short page, so a
    /// check sees every row whatever the history's length. The returned value
    /// is the first page's result with `messages` holding every row in order.
    pub async fn session_history(
        &mut self,
        session_id: Value,
        timeout_secs: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let mut history = Value::Null;
        let mut messages = Vec::new();
        loop {
            let page = self
                .call(
                    "session/history",
                    serde_json::json!({
                        "session_id": session_id,
                        "offset": messages.len(),
                        "limit": SESSION_HISTORY_PAGE,
                    }),
                    timeout_secs,
                )
                .await?;
            let rows = page["messages"]
                .as_array()
                .ok_or("session/history page carries no messages array")?
                .clone();
            let short = rows.len() < SESSION_HISTORY_PAGE;
            messages.extend(rows);
            if history.is_null() {
                history = page;
            }
            if short {
                break;
            }
        }
        history["messages"] = Value::Array(messages);
        Ok(history)
    }
}

/// Rows per `session/history` page read by [`JsonlRpcClient::session_history`]
/// (the RPC's default page; its maximum is 1000).
pub const SESSION_HISTORY_PAGE: usize = 100;

/// Provider protocol observed by the browser peer on the `oai-events` data
/// channel. Selects the harness mode and the safe event classification used
/// in diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserPeerProtocol {
    /// Deprecated private ChatGPT-brokered protocol (`turn.*`, `delegation.*`).
    Experimental,
    /// Public OpenAI Live API (`session.*`).
    Public,
}

impl BrowserPeerProtocol {
    fn harness_flag(self) -> &'static str {
        match self {
            Self::Experimental => "experimental",
            Self::Public => "public",
        }
    }

    /// Classify one browser event into a fixed, payload-free vocabulary so
    /// diagnostics never render provider text.
    pub fn classify(self, event: &Value) -> &'static str {
        let kind = event.get("type").and_then(Value::as_str);
        match self {
            Self::Experimental => match kind {
                Some("session.started") => "session.started",
                Some("session.context.appended") => "session.context.appended",
                Some("input_transcript.added") => "input_transcript.added",
                Some("output_transcript.added") => "output_transcript.added",
                Some("turn.created") => "turn.created",
                Some("turn.delta") => "turn.delta",
                Some("turn.done") => "turn.done",
                Some("delegation.created") => "delegation.created",
                Some("delegation.context.appended") => "delegation.context.appended",
                _ => "unknown",
            },
            Self::Public => match kind {
                Some("session.started") => "session.started",
                Some("session.updated") => "session.updated",
                Some("session.closed") => "session.closed",
                Some("session.input_transcript.delta") => "session.input_transcript.delta",
                Some("session.output_transcript.delta") => "session.output_transcript.delta",
                Some("session.output_audio.delta") => "session.output_audio.delta",
                Some("session.input_audio.muted") => "session.input_audio.muted",
                Some("session.input_audio.unmuted") => "session.input_audio.unmuted",
                Some("session.delegation.created") => "session.delegation.created",
                Some("session.commentary.appended") => "session.commentary.appended",
                Some("session.instructions.appended") => "session.instructions.appended",
                Some("session.thinking.appended") => "session.thinking.appended",
                Some("session.usage.updated") => "session.usage.updated",
                Some("error") => "error",
                Some("info") => "info",
                _ => "unknown",
            },
        }
    }
}

/// Applies `live/assistant_playback_hint`s to one browser peer's assistant
/// playback: a fire-and-forget `playback_hint` command the peer answers with
/// no response line, so it never disturbs the test's request/response
/// commands.
#[derive(Clone)]
pub struct PlaybackHintSender {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
}

impl PlaybackHintSender {
    pub async fn send(&self, hint: &str) -> std::io::Result<()> {
        let line = format!("{}\n", json!({"type": "playback_hint", "hint": hint}));
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await
    }
}

/// The browser peer currently answering a channel's playback hints (#1638).
/// The RPC tee applies each `live/assistant_playback_hint` notification to
/// it the moment the server writes it, as a client's notification handler
/// would; a reopen swaps in the new peer.
#[derive(Clone, Default)]
pub struct PlaybackHintRelay {
    peer: Arc<std::sync::Mutex<Option<PlaybackHintSender>>>,
}

impl PlaybackHintRelay {
    pub fn attach(&self, peer: &BrowserPeer) {
        if let Ok(mut slot) = self.peer.lock() {
            *slot = Some(peer.playback_hint_sender());
        }
    }

    /// Apply one hint (`"duck"` or `"restore"`) to the attached peer, if any.
    pub async fn apply(&self, hint: &str) {
        let sender = self.peer.lock().ok().and_then(|slot| slot.clone());
        if let Some(sender) = sender {
            let _ = sender.send(hint).await;
        }
    }
}

/// A JSONL RPC duplex whose server-to-client lines pass through unchanged,
/// except that each `live/assistant_playback_hint` notification is also
/// applied to the relay's browser peer as soon as the server writes it. The
/// test's client reads notifications only when it calls, so without the tee
/// a hint would reach the peer long after the barge-in it is for.
pub fn playback_hint_tee(
    relay: &PlaybackHintRelay,
) -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    let (client_stream, client_mid) = tokio::io::duplex(1024 * 1024);
    let (server_stream, server_mid) = tokio::io::duplex(1024 * 1024);
    let (mut client_mid_read, mut client_mid_write) = tokio::io::split(client_mid);
    let (server_mid_read, mut server_mid_write) = tokio::io::split(server_mid);
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut client_mid_read, &mut server_mid_write).await;
    });
    let relay = relay.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(server_mid_read);
        loop {
            let mut line = String::new();
            match lines.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if line.contains("live/assistant_playback_hint")
                && let Ok(message) = serde_json::from_str::<Value>(&line)
                && message["method"] == "live/assistant_playback_hint"
                && let Some(hint) = message["params"]["hint"].as_str()
            {
                relay.apply(hint).await;
            }
            if client_mid_write.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    (client_stream, server_stream)
}

pub struct BrowserPeer {
    evidence: Option<(evidence::Journal, u32)>,
    child: Child,
    /// Shared with [`PlaybackHintSender`]s: the test's request/response
    /// commands and fire-and-forget playback hints each write whole lines
    /// under this lock.
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    stdout: BrowserOutput,
    next_id: u64,
    pub protocol: BrowserPeerProtocol,
    pub last_raw_messages: u64,
    pub last_parse_failures: u64,
}

enum BrowserOutput {
    Direct(BufReader<ChildStdout>),
    Recorded {
        responses: tokio::sync::mpsc::Receiver<Value>,
        task: tokio::task::AbortHandle,
        closing: Arc<std::sync::atomic::AtomicBool>,
    },
}

impl Drop for BrowserOutput {
    fn drop(&mut self) {
        if let Self::Recorded { task, .. } = self {
            task.abort();
        }
    }
}

impl Drop for BrowserPeer {
    fn drop(&mut self) {
        if let Some((journal, channel)) = &self.evidence {
            let _ = journal.channel(*channel, evidence::ChannelAction::BrowserDropping);
            if let Err(fault) = journal.finish(evidence::Outcome::CancelledOrPanicked) {
                eprintln!("{fault}; journal={}", journal.path().display());
            }
        }
    }
}

impl BrowserPeer {
    pub async fn start(protocol: BrowserPeerProtocol) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_inner(protocol, None).await
    }

    pub async fn start_recorded(
        protocol: BrowserPeerProtocol,
        journal: evidence::Journal,
        channel: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_inner(protocol, Some((journal, channel))).await
    }

    async fn start_inner(
        protocol: BrowserPeerProtocol,
        evidence: Option<(evidence::Journal, u32)>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let browser_root = workspace_root().join("tests/live_smoke/browser");
        let script = browser_root.join("harness/gpt-live-peer-e2e.mjs");
        let node = [
            std::env::var_os("MEERKAT_E2E_LINUX_NODE_BIN"),
            std::env::var_os("MEERKAT_E2E_DARWIN_NODE_BIN"),
        ]
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("node"));
        // The browser peer never needs provider credentials; keep every
        // scenario's secret out of the child environment.
        let mut child = Command::new(node)
            .current_dir(browser_root)
            .arg(script)
            .arg("--protocol")
            .arg(protocol.harness_flag())
            .args(evidence.as_ref().map(|_| "--capture-evidence"))
            .env_remove("MEERKAT_E2E_AUTH_OPENAI_OAUTH_TOKENS_JSON")
            .env_remove("OPENAI_API_KEY")
            .env_remove("OPENAI_API_KEY_OLD")
            .env_remove("RKAT_OPENAI_API_KEY")
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("missing peer stdin")?;
        let mut stdout = BufReader::new(child.stdout.take().ok_or("missing peer stdout")?);
        let stdout = if let Some((journal, channel)) = &evidence {
            let journal = journal.clone();
            let channel = *channel;
            let (responses, receiver) = tokio::sync::mpsc::channel(4);
            let closing = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let reader_closing = closing.clone();
            let task = tokio::spawn(async move {
                loop {
                    let mut line = String::new();
                    match stdout.read_line(&mut line).await {
                        Ok(0) | Err(_) => {
                            if !reader_closing.load(std::sync::atomic::Ordering::Acquire) {
                                let _ = journal.fail(evidence::Fault::BrowserReaderClosed);
                            }
                            break;
                        }
                        Ok(_) => {}
                    }
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        let _ = journal.fail(evidence::Fault::InvalidBrowserEvidence);
                        continue;
                    };
                    if let Some(record) = message.get("evidence") {
                        match serde_json::from_value(record.clone()) {
                            Ok(record) => {
                                let _ = journal.native(channel, record);
                            }
                            Err(_) => {
                                let _ = journal.fail(evidence::Fault::InvalidBrowserEvidence);
                            }
                        }
                    } else if responses.send(message).await.is_err() {
                        break;
                    }
                }
            });
            BrowserOutput::Recorded {
                responses: receiver,
                task: task.abort_handle(),
                closing,
            }
        } else {
            BrowserOutput::Direct(stdout)
        };
        Ok(Self {
            evidence,
            child,
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            stdout,
            next_id: 1,
            protocol,
            last_raw_messages: 0,
            last_parse_failures: 0,
        })
    }

    /// A sender that applies `live/assistant_playback_hint`s to this peer's
    /// assistant playback while the test keeps driving the peer.
    pub fn playback_hint_sender(&self) -> PlaybackHintSender {
        PlaybackHintSender {
            stdin: Arc::clone(&self.stdin),
        }
    }

    pub async fn call(&mut self, command: Value) -> Result<Value, Box<dyn std::error::Error>> {
        // A direct play is a test-driven step like a scheduled one: mark it
        // in the channel's provider stream before the browser runs it.
        if command["type"] == "play" {
            self.provider_step(&play_step(command["name"].as_str().unwrap_or("unknown")));
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut command = command;
        command["id"] = json!(id);
        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(format!("{command}\n").as_bytes()).await?;
            stdin.flush().await?;
        }
        let response: Value = match &mut self.stdout {
            BrowserOutput::Direct(stdout) => {
                let mut line = String::new();
                timeout(Duration::from_secs(120), stdout.read_line(&mut line)).await??;
                serde_json::from_str(line.trim())?
            }
            BrowserOutput::Recorded { responses, .. } => {
                timeout(Duration::from_secs(120), responses.recv())
                    .await?
                    .ok_or("browser evidence reader closed")?
            }
        };
        if response["id"].as_u64() != Some(id) {
            return Err("browser peer response id mismatch".into());
        }
        if let Some(error) = response["error"].as_str() {
            return Err(format!("browser peer failed: {error}").into());
        }
        Ok(response["result"].clone())
    }

    pub async fn snapshot(&mut self) -> Result<Value, Box<dyn std::error::Error>> {
        self.call(json!({"type":"snapshot"})).await
    }

    pub async fn events(&mut self) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
        let snapshot = self.snapshot().await?;
        self.last_raw_messages = snapshot["event_transport"]["rawMessages"]
            .as_u64()
            .unwrap_or(0);
        self.last_parse_failures = snapshot["event_transport"]["parseFailures"]
            .as_u64()
            .unwrap_or(0);
        Ok(snapshot["events"].as_array().cloned().unwrap_or_default())
    }

    /// Outbound audio RTP packets sent so far and the browser clock now,
    /// for the uplink health record.
    pub async fn uplink(&mut self) -> Result<(u64, u64), Box<dyn std::error::Error>> {
        let snapshot = self.snapshot().await?;
        Ok((
            snapshot["connection"]["packets_sent"].as_u64().unwrap_or(0),
            snapshot["now_ms"].as_u64().unwrap_or(0),
        ))
    }

    pub async fn audio_evidence(&mut self) -> Result<AudioEvidence, Box<dyn std::error::Error>> {
        let snapshot = self.snapshot().await?;
        Ok(serde_json::from_value(snapshot["audio"].clone())?)
    }

    /// One snapshot read for the assistant-quiet wait: the events so far and
    /// the decoded playback audio ([`PlaybackAudio`]).
    pub async fn quiet_probe(
        &mut self,
    ) -> Result<(Vec<Value>, PlaybackAudio), Box<dyn std::error::Error>> {
        let snapshot = self.snapshot().await?;
        self.last_raw_messages = snapshot["event_transport"]["rawMessages"]
            .as_u64()
            .unwrap_or(0);
        self.last_parse_failures = snapshot["event_transport"]["parseFailures"]
            .as_u64()
            .unwrap_or(0);
        let audio = PlaybackAudio::from_snapshot(&snapshot["audio"])?;
        Ok((
            snapshot["events"].as_array().cloned().unwrap_or_default(),
            audio,
        ))
    }

    /// Payload-free summary of `events` under this peer's protocol.
    pub fn event_summary(&self, events: &[Value]) -> String {
        browser_event_summary(events, self.protocol)
    }

    pub async fn close(mut self) {
        if self.evidence.is_some() {
            let _ = self.stop_evidence().await;
        }
        let _ = self.call(json!({"type":"close"})).await;
        let _ = self.child.kill().await;
    }

    pub async fn stop_evidence(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some((journal, channel)) = self.evidence.take() {
            let result = timeout(
                Duration::from_secs(5),
                self.call(json!({"type":"stop_evidence"})),
            )
            .await;
            if let BrowserOutput::Recorded { closing, .. } = &self.stdout {
                closing.store(true, std::sync::atomic::Ordering::Release);
            }
            journal.channel(channel, evidence::ChannelAction::BrowserDropping)?;
            match result {
                Ok(Ok(_)) => Ok(()),
                _ => {
                    journal.fail(evidence::Fault::BrowserReaderClosed)?;
                    Err("browser evidence flush failed".into())
                }
            }
        } else {
            Ok(())
        }
    }

    /// Schedule a fixture against a timeline anchor. Returns at once with the
    /// schedule id; the browser records `scheduled`, `anchor_fired`,
    /// `fixture_start`, and `fixture_end` timeline entries as it happens.
    pub async fn play_at(&mut self, spec: &PlayAt) -> Result<u64, Box<dyn std::error::Error>> {
        self.provider_step(&play_at_step(&spec.name));
        let mut command = serde_json::to_value(spec)?;
        command["type"] = json!("play_at");
        let result = self.call(command).await?;
        result["scheduled"]
            .as_u64()
            .ok_or_else(|| "browser peer returned no schedule id".into())
    }

    /// Ordered sequence: each item is armed only after the previous fixture
    /// finished playing, then waits for its own anchor.
    pub async fn queue(
        &mut self,
        items: &[PlayAt],
    ) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
        let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
        self.provider_step(&queue_step(&names));
        let result = self.call(json!({"type":"queue","items":items})).await?;
        Ok(serde_json::from_value(result["scheduled"].clone())?)
    }

    /// Inject exactly `ms` of digital silence into the microphone track.
    pub async fn silence(&mut self, ms: u64) -> Result<(), Box<dyn std::error::Error>> {
        self.call(json!({"type":"silence","ms":ms})).await?;
        Ok(())
    }

    /// Tear down the client transport. `Hard` destroys the RTCPeerConnection
    /// without any goodbye; `Graceful` stops the track and closes the data
    /// channel first.
    pub async fn disconnect(
        &mut self,
        mode: DisconnectMode,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let mode_name = serde_json::to_value(mode)?;
        self.provider_step(&format!(
            "disconnect:{}",
            mode_name.as_str().unwrap_or("unknown")
        ));
        self.call(json!({"type":"disconnect","mode":mode})).await
    }

    /// Mark a browser action in the channel's provider stream before it
    /// happens: provider frames it causes are recorded after the marker.
    fn provider_step(&self, step: &str) {
        if let Some((journal, channel)) = &self.evidence {
            journal.provider_step(*channel, step);
        }
    }

    pub async fn timeline(&mut self) -> Result<Vec<TimelineEntry>, Box<dyn std::error::Error>> {
        let result = self.call(json!({"type":"timeline"})).await?;
        Ok(serde_json::from_value(result["timeline"].clone())?)
    }

    /// Record this peer's timeline in its journal (`Record::Timeline`, the
    /// shape S103/S104 write): the input of the Turbo S lag rule, which pairs
    /// each `fixture_start` (`detail.speech_ms`) with the next `input_final`
    /// (`detail.t_ms`). A scenario records it whether its phases passed or
    /// failed, so a failing run can be classified as provider-degraded or
    /// valid. A peer without evidence records nothing.
    pub async fn record_timeline(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let Some((journal, channel)) = self.evidence.clone() else {
            return Ok(());
        };
        let entries = self.timeline().await?;
        journal.record(evidence::Record::Timeline { channel, entries })?;
        Ok(())
    }

    /// Close the in-progress assistant response, so its `response_end`
    /// (with its text and `flushed: true`) is recorded, and return the
    /// timeline. A response otherwise ends only on new user speech, a
    /// disconnect or evidence stop, so a scenario that checks the final
    /// response's text must flush it first.
    pub async fn flush_response_timeline(
        &mut self,
    ) -> Result<Vec<TimelineEntry>, Box<dyn std::error::Error>> {
        let result = self.call(json!({"type":"flush_response"})).await?;
        Ok(serde_json::from_value(result["timeline"].clone())?)
    }

    pub async fn energy(&mut self) -> Result<EnergyReport, Box<dyn std::error::Error>> {
        let result = self.call(json!({"type":"energy"})).await?;
        Ok(serde_json::from_value(result)?)
    }

    /// The peer's readout records (one per response) so far. Strict: a
    /// snapshot without well-formed records is an error, never "no readouts".
    pub async fn readouts(&mut self) -> Result<ReadoutSnapshot, Box<dyn std::error::Error>> {
        let snapshot = self.snapshot().await?;
        let readouts = snapshot
            .get("readouts")
            .cloned()
            .ok_or("the peer snapshot carries no readout records")?;
        Ok(serde_json::from_value(readouts)?)
    }

    /// Soft browser faults (overlap) observed so far.
    pub async fn faults(
        &mut self,
    ) -> Result<Vec<evidence::BrowserFault>, Box<dyn std::error::Error>> {
        let snapshot = self.snapshot().await?;
        Ok(serde_json::from_value(snapshot["faults"].clone())?)
    }

    /// Poll the timeline until `predicate` yields a value or `bound` expires.
    /// The failure names what was awaited and dumps the whole timeline.
    pub async fn wait_for_timeline<T>(
        &mut self,
        bound: Duration,
        waiting_for: &str,
        predicate: impl Fn(&[TimelineEntry]) -> Option<T>,
    ) -> Result<T, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + bound;
        loop {
            let timeline = self.timeline().await?;
            if let Some(value) = predicate(&timeline) {
                return Ok(value);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "timed out after {} ms waiting for {waiting_for}; timeline:\n{}",
                    bound.as_millis(),
                    format_timeline(&timeline)
                )
                .into());
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
}

/// One response's assistant output transcript, as the peer segmented it:
/// the output between consecutive response boundaries (a user transcript
/// delta, a commentary append, a delegation; output pauses do not split).
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadoutRecord {
    pub index: u64,
    /// The boundary event type that opened the response (`connect` for the
    /// first one).
    pub opened_by: String,
    pub opened_ms: Option<u64>,
    /// The boundary event type that closed it; `None` only for the response
    /// still open at snapshot time.
    pub closed_by: Option<String>,
    pub closed_ms: Option<u64>,
    /// Arrival of the response's latest non-empty output delta.
    pub last_output_ms: Option<u64>,
    pub text: String,
    /// Sentences of 3 or more normalized words this response spoke more than
    /// once (a measurement: a stutter inside one response).
    pub stutters: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadoutSnapshot {
    pub records: Vec<ReadoutRecord>,
    /// The peer stopped recording responses at its bound.
    pub overflow: bool,
}

/// Timeline anchor a scheduled fixture waits for.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Now,
    /// Next assistant energy onset (or immediately when `allow_active` and
    /// the assistant is already speaking at arm time).
    FirstAssistantAudio,
    /// Assistant energy below threshold for a continuous `quiet_ms`; by
    /// default the assistant must have spoken since arming.
    AssistantQuiet,
    /// Next input-transcript final (public protocol: transcript deltas quiet
    /// for 700 ms or the assistant starting to answer).
    InputFinal,
    /// Next data-channel event of `event_type`.
    Event,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PlayAt {
    pub name: String,
    pub anchor: Anchor,
    pub offset_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_speech: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlap_bound_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
}

impl PlayAt {
    pub fn new(name: &str, anchor: Anchor, offset_ms: u64) -> Self {
        Self {
            name: name.to_owned(),
            anchor,
            offset_ms,
            quiet_ms: None,
            require_speech: None,
            allow_active: None,
            overlap_bound_ms: None,
            event_type: None,
        }
    }
    pub fn quiet_ms(mut self, quiet_ms: u64) -> Self {
        self.quiet_ms = Some(quiet_ms);
        self
    }
    pub fn require_speech(mut self, require: bool) -> Self {
        self.require_speech = Some(require);
        self
    }
    pub fn allow_active(mut self, allow: bool) -> Self {
        self.allow_active = Some(allow);
        self
    }
    pub fn overlap_bound_ms(mut self, bound: u64) -> Self {
        self.overlap_bound_ms = Some(bound);
        self
    }
    pub fn event_type(mut self, event_type: &str) -> Self {
        self.event_type = Some(event_type.to_owned());
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisconnectMode {
    Hard,
    Graceful,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineKind {
    FixtureStart,
    FixtureEnd,
    FixtureFailed,
    AssistantAudioStart,
    AssistantAudioEnd,
    InputFinal,
    ResponseEnd,
    Scheduled,
    AnchorFired,
    Disconnect,
    DelegationCreated,
    /// The joined expected executor input recorded at a delegation.created
    /// arrival (executor-input rule).
    DelegationInput,
    CommentaryAppended,
    /// Any other provider event, by `type` in the detail.
    ProviderEvent,
    /// RTCPeerConnection reached `connected`.
    Connected,
    /// The `oai-events` data channel opened.
    DataChannelOpen,
    /// First outbound user audio RTP packet reported by the browser.
    FirstAudioPacketSent,
    /// First `session.input_transcript.delta` of the channel.
    FirstInputDelta,
    /// A `live/assistant_playback_hint` applied to the peer's assistant
    /// playback gate (`hint` in the detail, #1638).
    PlaybackHint,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct TimelineEntry {
    pub t_ms: u64,
    pub kind: TimelineKind,
    #[serde(default)]
    pub detail: Value,
}

impl TimelineEntry {
    pub fn detail_u64(&self, key: &str) -> Option<u64> {
        self.detail.get(key).and_then(Value::as_u64)
    }
    pub fn detail_f64(&self, key: &str) -> Option<f64> {
        self.detail.get(key).and_then(Value::as_f64)
    }
    pub fn detail_str(&self, key: &str) -> Option<&str> {
        self.detail.get(key).and_then(Value::as_str)
    }
    /// Scheduled fixture id (`id`) when this entry belongs to a fixture.
    pub fn schedule_id(&self) -> Option<u64> {
        self.detail_u64("id")
    }
}

/// The provider-stream step a `play_at` of fixture `name` records before it
/// is scheduled. Oracles that window the provider stream use the same names.
pub fn play_at_step(name: &str) -> String {
    format!("play_at:{name}")
}

/// The provider-stream step a `queue` of fixtures `names` records.
pub fn queue_step(names: &[&str]) -> String {
    format!("queue:{}", names.join(","))
}

/// The provider-stream step a direct play of fixture `name` records before
/// the browser plays it.
pub fn play_step(name: &str) -> String {
    format!("play:{name}")
}

/// The provider-stream step recorded before a typed turn is sent to the
/// session. Between two channels it is recorded on the channel the journal
/// still calls current, the closed one: after that channel's receiver end
/// when the recording has one, and as the channel's last line, before any
/// end, when it has none (a host close whose transport was already gone).
/// A replay treats a typed turn with no provider frame after it as holding
/// nothing in either position.
pub const TYPED_TURN_STEP: &str = "typed_turn";

/// The provider-stream step recorded before the harness completes the
/// playback of a published assistant output.
pub const PLAYBACK_COMPLETE_STEP: &str = "playback_complete";

/// The provider-stream step recorded before the harness truncates a
/// published assistant output.
pub const TRUNCATE_STEP: &str = "truncate";

pub fn format_timeline(timeline: &[TimelineEntry]) -> String {
    let mut out = String::new();
    for entry in timeline {
        let detail = entry.detail.to_string();
        let detail = match detail.char_indices().nth(240) {
            Some((cut, _)) => format!("{}...", &detail[..cut]),
            None => detail,
        };
        out.push_str(&format!(
            "  {:>8} ms  {:<22} {detail}\n",
            entry.t_ms,
            format!("{:?}", entry.kind)
        ));
    }
    out
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
pub struct EnergyWindow {
    pub t_ms: u64,
    pub rms: f32,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct EnergySummary {
    pub threshold: f32,
    pub window_ms: u64,
    pub windows: Vec<EnergyWindow>,
    pub assistant_active: bool,
    pub overlap_ms: u64,
    pub first_assistant_audio_ms: Vec<u64>,
}

/// One protocol-anchored user input final (see the browser harness).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct InputFinal {
    pub t_ms: u64,
    pub text: String,
    #[serde(default)]
    pub start_ms: Option<f64>,
    #[serde(default)]
    pub end_ms: Option<f64>,
    /// `delegation` (closed when session.delegation.created arrived) or
    /// `response` (closed when the response's first output delta arrived).
    #[serde(default)]
    pub closed_by: Option<String>,
    #[serde(default)]
    pub closed_at_ms: Option<u64>,
}

/// The expected executor input of one delegation: every user transcript
/// delta that arrived since the previous `session.delegation.created` (or
/// connect), regardless of assistant output in between.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct DelegationInput {
    pub t_ms: u64,
    pub text: String,
    #[serde(default)]
    pub deltas: usize,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct EnergyReport {
    pub energy: EnergySummary,
    pub input_finals: Vec<InputFinal>,
    /// The user utterance still open: deltas no response or delegation has
    /// closed. Public Live has no input item identity or input-completed
    /// event, so a final word that arrives after the reply began opens a new
    /// utterance; the runtime commits it as its own user row at close.
    #[serde(default)]
    pub input_open: String,
    #[serde(default)]
    pub delegation_inputs: Vec<DelegationInput>,
}

impl EnergyReport {
    /// Every user utterance the browser heard on this channel, in order: the
    /// closed input finals, then the utterance still open, if any. Read once
    /// the channel has settled, this is what the runtime commits as spoken
    /// user rows by the time the channel closes.
    pub fn heard_utterances(&self) -> Vec<String> {
        self.input_finals
            .iter()
            .map(|input| input.text.clone())
            .chain((!self.input_open.trim().is_empty()).then(|| self.input_open.clone()))
            .collect()
    }

    /// Evidence-sized copy of the windows: at most `max` (t_ms, rms) pairs,
    /// keeping the per-bucket maximum so speech onsets survive downsampling.
    pub fn downsampled_windows(&self, max: usize) -> Vec<(u32, f32)> {
        let windows = &self.energy.windows;
        if windows.is_empty() || max == 0 {
            return Vec::new();
        }
        let bucket = windows.len().div_ceil(max).max(1);
        windows
            .chunks(bucket)
            .map(|chunk| {
                let peak = chunk.iter().map(|w| w.rms).fold(0.0f32, f32::max);
                (u32::try_from(chunk[0].t_ms).unwrap_or(u32::MAX), peak)
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct AudioEvidence {
    pub decoded_non_silent_frames: u64,
    pub decoded_non_silent_seconds: f64,
    pub non_silent_frames: u64,
    pub total_audio_energy: Option<f64>,
    pub total_samples_received: Option<u64>,
    pub total_samples_duration: Option<f64>,
    pub bytes_received: u64,
    pub packets_received: u64,
}

impl AudioEvidence {
    /// S99 requires decoded native remote speech, not analyser-only evidence.
    pub fn has_decoded_speech_since(self, baseline: Self) -> bool {
        self.bytes_received > baseline.bytes_received
            && self.packets_received > baseline.packets_received
            && self.decoded_non_silent_frames > baseline.decoded_non_silent_frames
            && self.decoded_non_silent_seconds - baseline.decoded_non_silent_seconds >= 0.1
    }

    /// Require new remote RTP plus measured non-silent media, never protocol
    /// events or counters accumulated before this exchange.
    pub fn has_spoken_since(self, baseline: Self) -> bool {
        self.bytes_received > baseline.bytes_received
            && self.packets_received > baseline.packets_received
            && (self.decoded_non_silent_seconds - baseline.decoded_non_silent_seconds >= 0.1
                || self
                    .non_silent_frames
                    .saturating_sub(baseline.non_silent_frames)
                    >= 2)
    }
}

/// Decoded remote (assistant) audio as the assistant-quiet wait reads it
/// from a peer snapshot. The browser counts non-silent frames it decoded
/// from the raw remote track ahead of its playback gate, and remote RTP
/// arrives at playback pace, so the counter follows what the user hears.
/// Event arrival does not: a long readout's transcript and audio events end
/// seconds before its playback does (#1719).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackAudio {
    /// Non-silent decoded frames so far.
    Measured(u64),
    /// The browser decodes nothing: it has no `MediaStreamTrackProcessor`,
    /// or the track reader stopped on an error. The counter then stays put,
    /// which must never read as silence.
    Unmeasured,
}

impl PlaybackAudio {
    /// Read a peer snapshot's `audio` object. A missing or mistyped field is
    /// an error, never a default.
    pub fn from_snapshot(audio: &Value) -> Result<Self, String> {
        let field = |name: &str| {
            audio
                .get(name)
                .ok_or_else(|| format!("browser snapshot audio has no {name}"))
        };
        let supported = field("processor_supported")?
            .as_bool()
            .ok_or("browser snapshot audio processor_supported is not a boolean")?;
        let errors = field("processor_errors")?
            .as_u64()
            .ok_or("browser snapshot audio processor_errors is not a count")?;
        let frames = field("decoded_non_silent_frames")?
            .as_u64()
            .ok_or("browser snapshot audio decoded_non_silent_frames is not a count")?;
        Ok(if supported && errors == 0 {
            Self::Measured(frames)
        } else {
            Self::Unmeasured
        })
    }
}

/// What a scenario's assistant-quiet wait does without measured playback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackAudioPolicy {
    /// The wait fails ([`QuietPoll::Unmeasured`]): S99, whose exchanges
    /// already require decoded speech.
    Required,
    /// The wait runs on its other clauses and reports that playback was not
    /// measured: S97 and S98, which accept analyser-only speech.
    BestEffort,
}

/// The clause that last restarted the assistant-quiet window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuietRestart {
    /// A newly arrived assistant transcript or audio event.
    OutputEvent,
    /// More non-silent decoded playback audio.
    PlaybackAudio,
    /// A new owned context append on the thinking or instructions lane.
    ContextAppend,
}

/// How an assistant-quiet wait ended. The append counts are waiting
/// periods, not appends: every owned append seen before assistant output
/// follows joins one period (the bound runs from the newest of them), and
/// the journal counts append attempts per fragment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QuietReport {
    /// Waiting periods that assistant output ended within the bound.
    pub appends_answered: u32,
    /// Waiting periods the bound ended with no assistant output.
    pub appends_unanswered: u32,
    /// Playback was not measured, so the audio clause did not run
    /// ([`PlaybackAudioPolicy::BestEffort`] only).
    pub audio_unmeasured: bool,
}

impl QuietReport {
    /// The record of one wait that took `waited`: which clauses ended it,
    /// for the `GPT_LIVE_QUIET` line and S99's `quiet_wait` metric.
    pub fn describe(&self, waited: Duration) -> String {
        format!(
            "appends_answered={} appends_unanswered={} audio={} waited_ms={}",
            self.appends_answered,
            self.appends_unanswered,
            if self.audio_unmeasured {
                "unmeasured"
            } else {
                "measured"
            },
            waited.as_millis()
        )
    }
}

/// One poll of an assistant-quiet wait.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuietPoll {
    Waiting,
    Quiet(QuietReport),
    /// Playback is not measured and the policy requires it.
    Unmeasured,
    /// The wait ran past [`AssistantQuiet::CEILING`]; the clause that last
    /// restarted it, if any did.
    CeilingExceeded(Option<QuietRestart>),
}

/// What the assistant-quiet wait reads at each poll.
#[derive(Clone, Copy, Debug)]
pub struct QuietObservation<'a> {
    /// Every peer event so far.
    pub events: &'a [Value],
    pub audio: PlaybackAudio,
    /// Owned thinking and instructions append attempts so far, when the
    /// scenario keeps an evidence journal.
    pub owned_appends: Option<usize>,
}

/// The rule a spoken fixture waits for, so it never talks over the
/// assistant: [`Self::QUIET_FOR`] with no new assistant output event, no
/// new non-silent decoded playback audio and no new owned context append,
/// and every append seen during the wait followed by assistant output or
/// left unanswered for [`Self::APPEND_RESPONSE_BOUND`]. A result's cue goes
/// out about 1.6 s after a readout ends and its response starts later
/// still; the bound keeps the window open across that gap.
#[derive(Clone, Debug)]
pub struct AssistantQuiet {
    policy: PlaybackAudioPolicy,
    started: Instant,
    quiet_since: Instant,
    seen_events: usize,
    seen_decoded: Option<u64>,
    seen_appends: Option<usize>,
    awaiting_response_since: Option<Instant>,
    last_restart: Option<QuietRestart>,
    report: QuietReport,
}

impl AssistantQuiet {
    pub const QUIET_FOR: Duration = Duration::from_secs(3);
    pub const APPEND_RESPONSE_BOUND: Duration = Duration::from_secs(8);
    /// Longer than any readout the scenarios ask for (S99's spelled-out
    /// working directory runs about 28 s), so a clause that never stops
    /// restarting the window fails here instead of at the scenario deadline.
    pub const CEILING: Duration = Duration::from_secs(180);

    /// Start a wait at `first`: what it already holds restarts nothing.
    pub fn new(now: Instant, first: &QuietObservation<'_>, policy: PlaybackAudioPolicy) -> Self {
        Self {
            policy,
            started: now,
            quiet_since: now,
            seen_events: first.events.len(),
            seen_decoded: match first.audio {
                PlaybackAudio::Measured(frames) => Some(frames),
                PlaybackAudio::Unmeasured => None,
            },
            seen_appends: first.owned_appends,
            awaiting_response_since: None,
            last_restart: None,
            report: QuietReport::default(),
        }
    }

    pub fn observe(&mut self, now: Instant, seen: &QuietObservation<'_>) -> QuietPoll {
        // Output first: it answers appends seen at earlier polls. An append
        // first seen at this poll is answered by output at a later poll, so
        // output that predates it in this poll never counts as its answer.
        let mut output = false;
        match seen.audio {
            PlaybackAudio::Unmeasured => match self.policy {
                PlaybackAudioPolicy::Required => return QuietPoll::Unmeasured,
                PlaybackAudioPolicy::BestEffort => {
                    self.report.audio_unmeasured = true;
                    self.seen_decoded = None;
                }
            },
            PlaybackAudio::Measured(frames) => {
                if self
                    .seen_decoded
                    .is_some_and(|seen_frames| frames > seen_frames)
                {
                    self.restart(now, QuietRestart::PlaybackAudio);
                    output = true;
                }
                self.seen_decoded = Some(frames);
            }
        }
        let fresh = seen.events.get(self.seen_events..).unwrap_or_default();
        if fresh.iter().any(is_assistant_output_event) {
            self.restart(now, QuietRestart::OutputEvent);
            output = true;
        }
        self.seen_events = seen.events.len();
        if output && self.awaiting_response_since.take().is_some() {
            self.report.appends_answered += 1;
        }
        if let Some(appends) = seen.owned_appends {
            if self
                .seen_appends
                .is_some_and(|seen_appends| appends > seen_appends)
            {
                self.restart(now, QuietRestart::ContextAppend);
                // The bound runs from the newest append: a cue sent after
                // quiet context gets its own window to be answered in.
                self.awaiting_response_since = Some(now);
            }
            self.seen_appends = Some(appends);
        }
        if self.awaiting_response_since.is_some_and(|since| {
            now.saturating_duration_since(since) >= Self::APPEND_RESPONSE_BOUND
        }) {
            self.awaiting_response_since = None;
            self.report.appends_unanswered += 1;
        }
        if self.awaiting_response_since.is_none()
            && now.saturating_duration_since(self.quiet_since) >= Self::QUIET_FOR
        {
            return QuietPoll::Quiet(self.report);
        }
        if now.saturating_duration_since(self.started) >= Self::CEILING {
            return QuietPoll::CeilingExceeded(self.last_restart);
        }
        QuietPoll::Waiting
    }

    fn restart(&mut self, now: Instant, clause: QuietRestart) {
        self.quiet_since = now;
        self.last_restart = Some(clause);
    }
}

/// An assistant output event as the peer records it.
pub fn is_assistant_output_event(event: &Value) -> bool {
    event["type"] == "session.output_transcript.delta"
        || event["type"] == "session.output_audio.delta"
}

/// Whether the assistant was audible as a spoken question began: decoded
/// playback grew between the snapshot taken before the fixture played and
/// the one at the question's first input transcript delta.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuestionOnset {
    Silent,
    OverPlayback,
    Unmeasured,
}

impl QuestionOnset {
    pub fn classify(before_play: PlaybackAudio, at_first_input: PlaybackAudio) -> Self {
        match (before_play, at_first_input) {
            (PlaybackAudio::Measured(before), PlaybackAudio::Measured(at)) if at > before => {
                Self::OverPlayback
            }
            (PlaybackAudio::Measured(_), PlaybackAudio::Measured(_)) => Self::Silent,
            _ => Self::Unmeasured,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Silent => "false",
            Self::OverPlayback => "true",
            Self::Unmeasured => "unmeasured",
        }
    }
}

#[cfg(test)]
mod assistant_quiet_tests {
    use super::{
        AssistantQuiet, PlaybackAudio, PlaybackAudioPolicy, QuestionOnset, QuietObservation,
        QuietPoll, QuietReport, QuietRestart,
    };
    use serde_json::{Value, json};
    use tokio::time::{Duration, Instant};

    const POLL: Duration = Duration::from_millis(100);

    /// A scripted poll timeline: what the peer and journal report at each
    /// 100 ms poll, and when the wait first answers anything but Waiting.
    struct Script {
        start: Instant,
        events: Vec<Value>,
        decoded: u64,
        appends: usize,
        audio_measured: bool,
    }

    impl Script {
        fn new() -> Self {
            Self {
                start: Instant::now(),
                events: Vec::new(),
                decoded: 1_000,
                appends: 4,
                audio_measured: true,
            }
        }

        fn audio(&self) -> PlaybackAudio {
            if self.audio_measured {
                PlaybackAudio::Measured(self.decoded)
            } else {
                PlaybackAudio::Unmeasured
            }
        }

        /// Poll every 100 ms from t=0 until `until`, applying `step` before
        /// each poll; returns the first non-Waiting answer and its time.
        fn run(
            mut self,
            policy: PlaybackAudioPolicy,
            until: Duration,
            mut step: impl FnMut(Duration, &mut Script),
        ) -> Option<(Duration, QuietPoll)> {
            let first = QuietObservation {
                events: &self.events,
                audio: self.audio(),
                owned_appends: Some(self.appends),
            };
            let mut quiet = AssistantQuiet::new(self.start, &first, policy);
            let mut at = Duration::ZERO;
            while at <= until {
                step(at, &mut self);
                let seen = QuietObservation {
                    events: &self.events,
                    audio: self.audio(),
                    owned_appends: Some(self.appends),
                };
                let poll = quiet.observe(self.start + at, &seen);
                if poll != QuietPoll::Waiting {
                    return Some((at, poll));
                }
                at += POLL;
            }
            None
        }
    }

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    /// R3 (control): nothing moves after the wait starts, so it ends after
    /// exactly the quiet window, as today's rule does.
    #[test]
    fn quiet_after_three_silent_seconds_when_nothing_moves() {
        let outcome = Script::new().run(PlaybackAudioPolicy::Required, ms(20_000), |_, _| {});
        assert_eq!(
            outcome,
            Some((ms(3_000), QuietPoll::Quiet(QuietReport::default())))
        );
    }

    /// R1: the readout's events have all arrived, but its audio keeps
    /// playing for 8 s. The question must wait for playback to end, not for
    /// events to stop arriving (S99 run 39205621248 spoke over it).
    #[test]
    fn playback_that_outlasts_the_events_keeps_the_window_open() {
        let outcome = Script::new().run(PlaybackAudioPolicy::Required, ms(20_000), |at, script| {
            if at <= ms(8_000) {
                script.decoded += 960;
            }
        });
        assert_eq!(
            outcome,
            Some((ms(11_000), QuietPoll::Quiet(QuietReport::default())))
        );
    }

    /// R1 (events): an assistant transcript event still restarts the window.
    #[test]
    fn an_output_event_restarts_the_window() {
        let outcome = Script::new().run(PlaybackAudioPolicy::Required, ms(20_000), |at, script| {
            if at == ms(2_000) {
                script
                    .events
                    .push(json!({"type": "session.output_transcript.delta", "delta": "x"}));
            }
        });
        assert_eq!(
            outcome,
            Some((ms(5_000), QuietPoll::Quiet(QuietReport::default())))
        );
    }

    /// R2: the readout ends at t0, its deferred result cue goes out 1.6 s
    /// later, and the cue's response plays from t0+3.2 s to t0+6 s. Quiet
    /// comes 3 s after that response, not 3 s after the readout.
    #[test]
    fn a_cue_after_the_readout_and_its_response_keep_the_window_open() {
        let outcome = Script::new().run(PlaybackAudioPolicy::Required, ms(20_000), |at, script| {
            if at == ms(1_600) {
                script.appends += 1;
            }
            if at >= ms(3_200) && at <= ms(6_000) {
                script.decoded += 960;
            }
        });
        assert_eq!(
            outcome,
            Some((
                ms(9_000),
                QuietPoll::Quiet(QuietReport {
                    appends_answered: 1,
                    ..QuietReport::default()
                })
            ))
        );
    }

    /// R5 (L2): the cue's response starts 3.9 s after the cue, past the
    /// quiet window. The append keeps the window open until the response
    /// begins, and quiet comes 3 s after the response ends.
    #[test]
    fn a_response_later_than_the_quiet_window_still_holds_the_wait() {
        let outcome = Script::new().run(PlaybackAudioPolicy::Required, ms(20_000), |at, script| {
            if at == ms(1_600) {
                script.appends += 1;
            }
            if at >= ms(5_500) && at <= ms(7_000) {
                script.decoded += 960;
            }
        });
        assert_eq!(
            outcome,
            Some((
                ms(10_000),
                QuietPoll::Quiet(QuietReport {
                    appends_answered: 1,
                    ..QuietReport::default()
                })
            ))
        );
    }

    /// R5 (L2 bound): quiet context the model does not answer holds the wait
    /// for the bound only, and the report says it went unanswered.
    #[test]
    fn an_unanswered_append_is_waited_out_for_the_bound_only() {
        let outcome = Script::new().run(PlaybackAudioPolicy::Required, ms(20_000), |at, script| {
            if at == ms(1_000) {
                script.appends += 1;
            }
        });
        assert_eq!(
            outcome,
            Some((
                ms(9_000),
                QuietPoll::Quiet(QuietReport {
                    appends_unanswered: 1,
                    ..QuietReport::default()
                })
            ))
        );
    }

    /// L1: without measured playback, S99's wait fails typed instead of
    /// reading a still counter as silence.
    #[test]
    fn unmeasured_playback_fails_a_wait_that_requires_it() {
        let mut script = Script::new();
        script.audio_measured = false;
        let outcome = script.run(PlaybackAudioPolicy::Required, ms(20_000), |_, _| {});
        assert_eq!(outcome, Some((ms(0), QuietPoll::Unmeasured)));
    }

    /// L1: playback is judged on every poll, not once at the start. Frames
    /// grow, then the browser reports a processor error: from that poll the
    /// audio clause is unmeasured, so a wait that requires it fails with the
    /// typed error instead of going quiet 3 s after the last growth.
    #[test]
    fn a_processor_error_mid_wait_fails_a_wait_that_requires_it() {
        let outcome = Script::new().run(PlaybackAudioPolicy::Required, ms(20_000), |at, script| {
            if at <= ms(1_000) {
                script.decoded += 960;
            }
            let snapshot = json!({
                "processor_supported": true,
                "processor_errors": u64::from(at >= ms(1_500)),
                "decoded_non_silent_frames": script.decoded,
            });
            script.audio_measured = PlaybackAudio::from_snapshot(&snapshot)
                == Ok(PlaybackAudio::Measured(script.decoded));
        });
        assert_eq!(outcome, Some((ms(1_500), QuietPoll::Unmeasured)));
    }

    /// L1: S97 and S98 run on the other clauses and the report says so.
    #[test]
    fn unmeasured_playback_is_reported_by_a_best_effort_wait() {
        let mut script = Script::new();
        script.audio_measured = false;
        let outcome = script.run(PlaybackAudioPolicy::BestEffort, ms(20_000), |_, _| {});
        assert_eq!(
            outcome,
            Some((
                ms(3_000),
                QuietPoll::Quiet(QuietReport {
                    audio_unmeasured: true,
                    ..QuietReport::default()
                })
            ))
        );
    }

    /// L3: a clause that never stops (a remote noise floor above the
    /// non-silent threshold) fails at the ceiling and is named.
    #[test]
    fn a_clause_that_never_stops_fails_at_the_ceiling_and_is_named() {
        let outcome = Script::new().run(
            PlaybackAudioPolicy::Required,
            AssistantQuiet::CEILING + ms(1_000),
            |_, script| script.decoded += 960,
        );
        assert_eq!(
            outcome,
            Some((
                AssistantQuiet::CEILING,
                QuietPoll::CeilingExceeded(Some(QuietRestart::PlaybackAudio))
            ))
        );
    }

    #[test]
    fn snapshot_audio_is_measured_only_while_the_processor_decodes() {
        let audio = |supported: bool, errors: u64| {
            json!({
                "processor_supported": supported,
                "processor_errors": errors,
                "decoded_non_silent_frames": 4_800,
            })
        };
        assert_eq!(
            PlaybackAudio::from_snapshot(&audio(true, 0)),
            Ok(PlaybackAudio::Measured(4_800))
        );
        assert_eq!(
            PlaybackAudio::from_snapshot(&audio(false, 0)),
            Ok(PlaybackAudio::Unmeasured)
        );
        assert_eq!(
            PlaybackAudio::from_snapshot(&audio(true, 1)),
            Ok(PlaybackAudio::Unmeasured)
        );
        assert!(PlaybackAudio::from_snapshot(&json!({"processor_supported": true})).is_err());
    }

    /// R4: the onset compares the snapshot before the fixture played with
    /// the one at the question's first input delta.
    #[test]
    fn a_question_that_starts_over_playback_is_classified() {
        assert_eq!(
            QuestionOnset::classify(PlaybackAudio::Measured(100), PlaybackAudio::Measured(900)),
            QuestionOnset::OverPlayback
        );
        assert_eq!(
            QuestionOnset::classify(PlaybackAudio::Measured(900), PlaybackAudio::Measured(900)),
            QuestionOnset::Silent
        );
        assert_eq!(
            QuestionOnset::classify(PlaybackAudio::Unmeasured, PlaybackAudio::Measured(900)),
            QuestionOnset::Unmeasured
        );
    }

    /// The per-wait record (`GPT_LIVE_QUIET`, S99's `quiet_wait`) names how
    /// the wait ended, so a soak can count cues left unanswered.
    #[test]
    fn a_quiet_report_describes_how_its_wait_ended() {
        let report = QuietReport {
            appends_answered: 1,
            appends_unanswered: 2,
            audio_unmeasured: true,
        };
        assert_eq!(
            report.describe(ms(9_250)),
            "appends_answered=1 appends_unanswered=2 audio=unmeasured waited_ms=9250"
        );
        assert_eq!(
            QuietReport::default().describe(ms(3_000)),
            "appends_answered=0 appends_unanswered=0 audio=measured waited_ms=3000"
        );
    }
}

#[cfg(test)]
mod audio_evidence_tests {
    use super::AudioEvidence;

    #[test]
    fn silence_rtp_readiness_and_stale_audio_are_not_spoken_output() {
        let silence = AudioEvidence {
            bytes_received: 10_000,
            packets_received: 100,
            total_samples_received: Some(48_000),
            total_samples_duration: Some(1.0),
            ..AudioEvidence::default()
        };
        assert!(!silence.has_spoken_since(AudioEvidence::default()));
        let energy_only = AudioEvidence {
            total_audio_energy: Some(0.01),
            ..silence
        };
        assert!(!energy_only.has_spoken_since(AudioEvidence::default()));
        let spoken = AudioEvidence {
            decoded_non_silent_frames: 4_800,
            decoded_non_silent_seconds: 0.1,
            ..energy_only
        };
        assert!(spoken.has_spoken_since(AudioEvidence::default()));
        assert!(!spoken.has_spoken_since(spoken));
        let stale = AudioEvidence {
            bytes_received: spoken.bytes_received + 100,
            packets_received: spoken.packets_received + 1,
            total_samples_duration: Some(2.0),
            ..spoken
        };
        assert!(!stale.has_spoken_since(spoken));
    }

    #[test]
    fn decoded_audio_requires_remote_rtp_and_a_measured_duration() {
        let mut evidence = AudioEvidence {
            decoded_non_silent_frames: 4_800,
            decoded_non_silent_seconds: 0.1,
            ..AudioEvidence::default()
        };
        assert!(!evidence.has_spoken_since(AudioEvidence::default()));
        evidence.bytes_received = 5_000;
        evidence.packets_received = 10;
        assert!(evidence.has_spoken_since(AudioEvidence::default()));
        evidence.decoded_non_silent_seconds = 0.001;
        assert!(!evidence.has_spoken_since(AudioEvidence::default()));
    }

    #[test]
    fn missing_browser_audio_measurement_is_an_error_not_a_zero_default() {
        assert!(serde_json::from_value::<AudioEvidence>(serde_json::json!({})).is_err());
    }

    #[test]
    fn concurrent_bootstrap_requires_fresh_decoded_speech_not_analyser_or_transcript() {
        let analyser_only = AudioEvidence {
            bytes_received: 10_000,
            packets_received: 100,
            non_silent_frames: 100,
            total_audio_energy: Some(0.5),
            ..AudioEvidence::default()
        };
        assert!(analyser_only.has_spoken_since(AudioEvidence::default()));
        assert!(!analyser_only.has_decoded_speech_since(AudioEvidence::default()));
        let decoded = AudioEvidence {
            decoded_non_silent_frames: 4_800,
            decoded_non_silent_seconds: 0.1,
            ..analyser_only
        };
        assert!(decoded.has_decoded_speech_since(AudioEvidence::default()));
        assert!(!decoded.has_decoded_speech_since(decoded));
        assert!(
            !AudioEvidence {
                bytes_received: decoded.bytes_received + 1,
                packets_received: decoded.packets_received + 1,
                ..decoded
            }
            .has_decoded_speech_since(decoded)
        );
        assert!(
            !AudioEvidence {
                bytes_received: 0,
                packets_received: 0,
                ..decoded
            }
            .has_decoded_speech_since(AudioEvidence::default())
        );
    }
}

pub async fn wait_for_spoken_output(
    peer: &mut BrowserPeer,
    baseline: AudioEvidence,
    timeout_secs: u64,
) -> Result<AudioEvidence, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let snapshot = peer.snapshot().await?;
        let audio = &snapshot["audio"];
        let evidence: AudioEvidence = serde_json::from_value(audio.clone())?;
        let non_silent_frames = audio["non_silent_frames"].as_u64().unwrap_or(0);
        let decoded_non_silent_frames = audio["decoded_non_silent_frames"].as_u64().unwrap_or(0);
        let total_audio_energy = evidence.total_audio_energy;
        let total_samples_received = evidence.total_samples_received;
        if evidence.has_spoken_since(baseline) {
            return Ok(evidence);
        }
        if Instant::now() >= deadline {
            let sampled_frames = audio["sampled_frames"].as_u64().unwrap_or(0);
            let max_rms = audio["max_rms"].as_f64().unwrap_or(0.0);
            let decoded_frames = audio["decoded_frames"].as_u64().unwrap_or(0);
            let max_decoded_rms = audio["max_decoded_rms"].as_f64().unwrap_or(0.0);
            let processor_supported = audio["processor_supported"].as_bool().unwrap_or(false);
            let processor_errors = audio["processor_errors"].as_u64().unwrap_or(0);
            let bytes_received = audio["bytes_received"].as_u64().unwrap_or(0);
            let packets_received = audio["packets_received"].as_u64().unwrap_or(0);
            return Err(format!(
                "timed out waiting for spoken output; decoded_frames={decoded_frames}, decoded_non_silent_frames={decoded_non_silent_frames}, max_decoded_rms={max_decoded_rms:.6}, processor_supported={processor_supported}, processor_errors={processor_errors}, sampled_frames={sampled_frames}, non_silent_frames={non_silent_frames}, max_rms={max_rms:.6}, bytes_received={bytes_received}, packets_received={packets_received}, total_audio_energy={total_audio_energy:?}, total_samples_received={total_samples_received:?}"
            )
            .into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

pub async fn wait_for_events<F>(
    peer: &mut BrowserPeer,
    timeout_secs: u64,
    predicate: F,
) -> Result<Vec<Value>, Box<dyn std::error::Error>>
where
    F: Fn(&[Value]) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let events = peer.events().await?;
        if predicate(&events) {
            return Ok(events);
        }
        if Instant::now() >= deadline {
            let event_summary = peer.event_summary(&events);
            return Err(format!(
                "timed out waiting for provider events; raw_messages={}, parse_failures={}, {event_summary}",
                peer.last_raw_messages, peer.last_parse_failures
            )
            .into());
        }
        sleep(Duration::from_millis(250)).await;
    }
}

pub fn browser_event_summary(events: &[Value], protocol: BrowserPeerProtocol) -> String {
    let mut kind_counts = BTreeMap::<&'static str, usize>::new();
    let mut normalized_json_bytes = 0usize;
    for event in events {
        *kind_counts.entry(protocol.classify(event)).or_default() += 1;
        normalized_json_bytes = normalized_json_bytes
            .saturating_add(serde_json::to_vec(event).map_or(0, |encoded| encoded.len()));
    }
    format!(
        "observed {} events across {} safe classes with normalized_json_bytes={normalized_json_bytes}: {kind_counts:?}",
        events.len(),
        kind_counts.len()
    )
}

/// Payload-free view of the delegated executor's mob state, used both as a
/// completion signal and as the timeout diagnostic.
pub struct DelegatedExecutorDiagnostic {
    pub worker_identity: Option<String>,
    pub has_tool_result: bool,
    pub has_assistant_final: bool,
    summary: String,
}

impl fmt::Display for DelegatedExecutorDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.summary)
    }
}

impl DelegatedExecutorDiagnostic {
    fn absent(summary: String) -> Self {
        Self {
            worker_identity: None,
            has_tool_result: false,
            has_assistant_final: false,
            summary,
        }
    }
}

/// The member that executes a channel's delegated turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegatedWorker<'a> {
    /// A durable fork per delegation, named `live-delegation-<operation>`.
    Fork,
    /// The channel's own member executes each delegated turn in its session.
    ExistingMember(&'a str),
}

pub async fn delegated_executor_diagnostic(
    rpc: &mut JsonlRpcClient,
    mob_id: &str,
    worker: DelegatedWorker<'_>,
) -> DelegatedExecutorDiagnostic {
    let (identity, ownership) = match worker {
        DelegatedWorker::ExistingMember(identity) => (identity.to_string(), " (existing member)"),
        DelegatedWorker::Fork => {
            let roster = match rpc.call("mob/members", json!({"mob_id":mob_id}), 30).await {
                Ok(roster) => roster,
                Err(error) => {
                    return DelegatedExecutorDiagnostic::absent(format!("roster_error={error}"));
                }
            };
            let Some(worker) = roster["members"].as_array().and_then(|members| {
                members.iter().find(|member| {
                    member["agent_identity"]
                        .as_str()
                        .is_some_and(|id| id.starts_with("live-delegation-"))
                })
            }) else {
                return DelegatedExecutorDiagnostic::absent(
                    "fork worker=absent (no live-delegation-* member in the roster)".to_string(),
                );
            };
            let Some(identity) = worker["agent_identity"].as_str() else {
                return DelegatedExecutorDiagnostic::absent(
                    "fork worker=present identity=invalid".to_string(),
                );
            };
            (identity.to_string(), " (fork)")
        }
    };
    let status = match rpc
        .call(
            "mob/member_status",
            json!({"mob_id":mob_id,"agent_identity":identity}),
            30,
        )
        .await
    {
        Ok(status) => status,
        Err(error) => {
            return DelegatedExecutorDiagnostic {
                summary: format!("worker={identity}{ownership} status_error={error}"),
                worker_identity: Some(identity),
                has_tool_result: false,
                has_assistant_final: false,
            };
        }
    };
    let history = match rpc
        .call(
            "mob/member_history",
            json!({"mob_id":mob_id,"agent_identity":identity,"from_index":0,"limit":200}),
            30,
        )
        .await
    {
        Ok(history) => history,
        Err(error) => {
            return DelegatedExecutorDiagnostic {
                summary: format!("worker={identity}{ownership} history_error={error}"),
                worker_identity: Some(identity),
                has_tool_result: false,
                has_assistant_final: false,
            };
        }
    };
    let mut role_counts = BTreeMap::<String, usize>::new();
    let mut has_tool_result = false;
    let mut has_assistant_final = false;
    if let Some(messages) = history.pointer("/page/messages").and_then(Value::as_array) {
        for message in messages {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            *role_counts.entry(role.to_string()).or_default() += 1;
            has_tool_result |= role == "tool_results";
            has_assistant_final |= role == "assistant";
        }
    }
    let summary = format!(
        "worker={identity}{ownership} status={} is_final={} run_state={} in_flight={} last_progress={} health={} role_counts={role_counts:?} has_tool_result={has_tool_result} has_assistant_final={has_assistant_final}",
        status["status"].as_str().unwrap_or("<unknown>"),
        status["is_final"].as_bool().unwrap_or(false),
        status["progress"]["run_state"]
            .as_str()
            .unwrap_or("<unknown>"),
        status["progress"]["in_flight_work"].as_u64().unwrap_or(0),
        status["progress"]["last_progress_event"]
            .as_str()
            .unwrap_or("<unknown>"),
        status["progress"]["health"].as_str().unwrap_or("<unknown>"),
    );
    DelegatedExecutorDiagnostic {
        worker_identity: Some(identity),
        has_tool_result,
        has_assistant_final,
        summary,
    }
}

pub fn execution_identity(profile_id: &str) -> Value {
    json!({
        "version":"v1",
        "profile_id":profile_id
    })
}

#[cfg(test)]
mod browser_event_summary_tests {
    use super::{BrowserPeerProtocol, browser_event_summary};

    #[test]
    fn unknown_event_kinds_and_payloads_are_not_rendered() {
        let events = vec![
            serde_json::json!({
                "type": "FIXTURE_PRIVATE_UNKNOWN_KIND",
                "secret": "FIXTURE_PRIVATE_BROWSER_PAYLOAD"
            }),
            serde_json::json!({
                "type": "turn.done",
                "turn": { "transcript": "FIXTURE_PRIVATE_TRANSCRIPT" }
            }),
        ];

        let summary = browser_event_summary(&events, BrowserPeerProtocol::Experimental);
        assert!(summary.contains("unknown"));
        assert!(summary.contains("turn.done"));
        assert!(!summary.contains("FIXTURE_PRIVATE_UNKNOWN_KIND"));
        assert!(!summary.contains("FIXTURE_PRIVATE_BROWSER_PAYLOAD"));
        assert!(!summary.contains("FIXTURE_PRIVATE_TRANSCRIPT"));
    }

    #[test]
    fn public_protocol_classifies_only_public_live_kinds() {
        let events = vec![
            serde_json::json!({
                "type": "session.output_transcript.delta",
                "delta": "FIXTURE_PRIVATE_TRANSCRIPT"
            }),
            serde_json::json!({
                "type": "session.delegation.created",
                "delegation": { "id": "FIXTURE_PRIVATE_DELEGATION", "target": "client" }
            }),
            serde_json::json!({ "type": "turn.done" }),
        ];

        let summary = browser_event_summary(&events, BrowserPeerProtocol::Public);
        assert!(summary.contains("session.output_transcript.delta"));
        assert!(summary.contains("session.delegation.created"));
        assert!(summary.contains("unknown"));
        assert!(!summary.contains("turn.done"));
        assert!(!summary.contains("FIXTURE_PRIVATE_TRANSCRIPT"));
        assert!(!summary.contains("FIXTURE_PRIVATE_DELEGATION"));
    }
}
