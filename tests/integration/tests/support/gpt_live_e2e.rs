//! Shared fixtures for the GPT Live browser-peer verticals.
//!
//! Scenario 96 (experimental `gpt-live-1-codex`, ChatGPT OAuth) and scenario
//! 97 (public `gpt-live-1`, OpenAI API key) drive the same RPC, Mob, and
//! browser-peer graph. Only the provider protocol seen on the `oai-events`
//! data channel differs, which [`BrowserPeerProtocol`] selects.
//!
//! This file is `#[path]`-included by both test targets, so every item is
//! public and unused items in one target are expected.
#![allow(dead_code)]

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
}

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

pub struct BrowserPeer {
    evidence: Option<(evidence::Journal, u32)>,
    child: Child,
    stdin: ChildStdin,
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
            stdin,
            stdout,
            next_id: 1,
            protocol,
            last_raw_messages: 0,
            last_parse_failures: 0,
        })
    }

    pub async fn call(&mut self, command: Value) -> Result<Value, Box<dyn std::error::Error>> {
        let id = self.next_id;
        self.next_id += 1;
        let mut command = command;
        command["id"] = json!(id);
        self.stdin.write_all(command.to_string().as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
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

    pub async fn audio_evidence(&mut self) -> Result<AudioEvidence, Box<dyn std::error::Error>> {
        let snapshot = self.snapshot().await?;
        Ok(serde_json::from_value(snapshot["audio"].clone())?)
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

pub async fn delegated_executor_diagnostic(
    rpc: &mut JsonlRpcClient,
    mob_id: &str,
) -> DelegatedExecutorDiagnostic {
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
        return DelegatedExecutorDiagnostic::absent("worker=absent".to_string());
    };
    let Some(identity) = worker["agent_identity"].as_str() else {
        return DelegatedExecutorDiagnostic::absent("worker=present identity=invalid".to_string());
    };
    let identity = identity.to_string();
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
                summary: format!("worker={identity} status_error={error}"),
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
                summary: format!("worker={identity} history_error={error}"),
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
        "worker={identity} status={} is_final={} run_state={} in_flight={} last_progress={} health={} role_counts={role_counts:?} has_tool_result={has_tool_result} has_assistant_final={has_assistant_final}",
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
