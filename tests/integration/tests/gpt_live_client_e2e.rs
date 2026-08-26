#![cfg(all(feature = "experimental-gpt-live-e2e", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use meerkat::experimental_gpt_live::{
    ExperimentalGptLiveOpenAuthority, ExperimentalGptLiveOpenAuthorityConfig,
    ExperimentalGptLiveWebrtcTransport, ExperimentalLiveCurrentConfigSource,
    ExperimentalLiveOpenAuthorityError, ExperimentalLiveSessionBindingAuthority,
    ExperimentalLiveSessionBindingAuthorization,
};
use meerkat_core::handles::LeaseKey;
use meerkat_core::{
    ActingOnBehalfOf, AuthBindingRef, AuthBindingUseRequest, AuthGrant, BackendProfileConfig,
    BindingId, BindingOrigin, BindingPolicy, BlobStore, Config, ConfigRuntime, ConfigStore,
    CredentialSourceSpec, GrantAction, GrantScope, MemoryConfigStore, PrincipalKind, PrincipalRef,
    ProviderBindingConfig, RealmConfigSection, RealmId,
};
use meerkat_mob_mcp::MobMcpState;
use meerkat_providers::auth_store::{
    FileTokenStore, InMemoryCoordinator, PersistedAuthMode, PersistedTokens,
    ProviderAuthPersistence, TokenKey, TokenStore,
};
use meerkat_rpc::router::NotificationSink;
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::{Duration, Instant, sleep, timeout};

const REALM: &str = "scenario-96-gpt-live-client";
const BINDING: &str = "openai_oauth";
const CLIENT_PROFILE: &str = "openai.gpt-live-1-codex.client-context.v1";
const FUNCTION_BRIDGE_PROFILE: &str = "openai.gpt-live-1-codex.function-bridge.v1";
const MIN_TTL_SECS: i64 = 5 * 60;

fn workspace_root() -> PathBuf {
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

fn auth_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(REALM).expect("valid realm"),
        binding: BindingId::parse(BINDING).expect("valid binding"),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

fn required_tokens() -> Result<PersistedTokens, Box<dyn std::error::Error>> {
    let raw = std::env::var("MEERKAT_E2E_AUTH_OPENAI_OAUTH_TOKENS_JSON")
        .map_err(|_| "MEERKAT_E2E_AUTH_OPENAI_OAUTH_TOKENS_JSON is required")?;
    let content = if let Some(path) = raw.strip_prefix('@') {
        std::fs::read_to_string(path)?
    } else if Path::new(&raw).exists() {
        std::fs::read_to_string(&raw)?
    } else {
        raw
    };
    let tokens: PersistedTokens = serde_json::from_str(&content)
        .map_err(|error| format!("invalid OpenAI OAuth token bundle: {error}"))?;
    let now: i64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .try_into()?;
    prepare_remote_tokens(tokens, now)
}

fn prepare_remote_tokens(
    mut tokens: PersistedTokens,
    now_epoch_secs: i64,
) -> Result<PersistedTokens, Box<dyn std::error::Error>> {
    if tokens.auth_mode != PersistedAuthMode::ChatgptOauth
        || tokens
            .primary_secret
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err("scenario 96 requires a complete chatgpt_oauth access-token bundle".into());
    }
    if tokens
        .account_id
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        let id_token = tokens
            .id_token
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or("scenario 96 OAuth bundle has neither account_id nor an id_token")?;
        let claims = meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
            .map_err(|_| "scenario 96 OAuth id_token payload is invalid")?;
        tokens.account_id = claims
            .raw
            .get("https://api.openai.com/auth")
            .and_then(|auth| auth.get("chatgpt_account_id"))
            .and_then(Value::as_str)
            .or(claims.chatgpt_account_id.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
    }
    if tokens.account_id.is_none() {
        return Err("scenario 96 OAuth bundle has no supported ChatGPT account-id claim".into());
    }
    let expires_at = tokens
        .expires_at
        .as_ref()
        .ok_or("scenario 96 OAuth bundle has no expires_at")?
        .timestamp();
    if expires_at <= now_epoch_secs + MIN_TTL_SECS {
        return Err("scenario 96 OAuth access token is expired or within five minutes of expiry; rotate the encrypted secret locally".into());
    }
    // The source bundle is immutable. The remote action receives no refresh
    // authority because it cannot write a rotated refresh token back.
    tokens.refresh_token = None;
    Ok(tokens)
}

fn scenario_config() -> Config {
    let executor_model =
        std::env::var("GPT_LIVE_E2E_EXECUTOR_MODEL").unwrap_or_else(|_| "gpt-5.6-sol".to_string());
    let mut section = RealmConfigSection {
        backend: BTreeMap::new(),
        auth: BTreeMap::new(),
        binding: BTreeMap::new(),
        default_binding: Some(BINDING.to_string()),
        parent: None,
    };
    section.backend.insert(
        "chatgpt_backend".to_string(),
        BackendProfileConfig {
            provider: "openai".to_string(),
            backend_kind: "chatgpt_backend".to_string(),
            base_url: None,
            options: Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        BINDING.to_string(),
        meerkat_core::AuthProfileConfig {
            provider: "openai".to_string(),
            auth_method: "managed_chatgpt_oauth".to_string(),
            source: CredentialSourceSpec::ManagedStore,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        BINDING.to_string(),
        ProviderBindingConfig {
            backend_profile: "chatgpt_backend".to_string(),
            auth_profile: BINDING.to_string(),
            default_model: Some(executor_model),
            policy: BindingPolicy::default(),
            provider_default: false,
        },
    );
    let mut config = Config::default();
    config.realm.insert(REALM.to_string(), section);
    config.model_fallback.enabled = false;
    config
}

#[derive(Clone)]
struct FixedConfigSource(Config);

#[async_trait]
impl ExperimentalLiveCurrentConfigSource for FixedConfigSource {
    async fn current_config(&self) -> Result<Config, meerkat_core::ConfigError> {
        Ok(self.0.clone())
    }
}

struct ExplicitScenarioBindingAuthority {
    session_id: meerkat_core::SessionId,
    binding: AuthBindingRef,
    auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
    mobs: Arc<MobMcpState>,
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
        let principal = PrincipalRef::new(PrincipalKind::Human, "scenario-96-operator")
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

struct JsonlRpcClient {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next_id: i64,
    notifications: VecDeque<Value>,
}

impl JsonlRpcClient {
    fn new(stream: DuplexStream) -> Self {
        let (reader, writer) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
            notifications: VecDeque::new(),
        }
    }

    async fn call_raw(
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
            if timeout(remaining, self.reader.read_line(&mut line)).await?? == 0 {
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

    async fn wait_for_notification(
        &mut self,
        method: &str,
        timeout_secs: u64,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        if let Some(index) = self
            .notifications
            .iter()
            .position(|message| message["method"].as_str() == Some(method))
        {
            return Ok(self
                .notifications
                .remove(index)
                .expect("indexed notification exists")["params"]
                .clone());
        }
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        loop {
            let mut line = String::new();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if timeout(remaining, self.reader.read_line(&mut line)).await?? == 0 {
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

    async fn call(
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

struct BrowserPeer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    last_raw_messages: u64,
    last_parse_failures: u64,
}

impl BrowserPeer {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
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
        let mut child = Command::new(node)
            .current_dir(browser_root)
            .arg(script)
            .env_remove("MEERKAT_E2E_AUTH_OPENAI_OAUTH_TOKENS_JSON")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("missing peer stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("missing peer stdout")?);
        Ok(Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            last_raw_messages: 0,
            last_parse_failures: 0,
        })
    }

    async fn call(&mut self, command: Value) -> Result<Value, Box<dyn std::error::Error>> {
        let id = self.next_id;
        self.next_id += 1;
        let mut command = command;
        command["id"] = json!(id);
        self.stdin.write_all(command.to_string().as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        let mut line = String::new();
        timeout(Duration::from_secs(120), self.stdout.read_line(&mut line)).await??;
        let response: Value = serde_json::from_str(line.trim())?;
        if response["id"].as_u64() != Some(id) {
            return Err("browser peer response id mismatch".into());
        }
        if let Some(error) = response["error"].as_str() {
            return Err(format!("browser peer failed: {error}").into());
        }
        Ok(response["result"].clone())
    }

    async fn snapshot(&mut self) -> Result<Value, Box<dyn std::error::Error>> {
        self.call(json!({"type":"snapshot"})).await
    }

    async fn events(&mut self) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
        let snapshot = self.snapshot().await?;
        self.last_raw_messages = snapshot["event_transport"]["rawMessages"]
            .as_u64()
            .unwrap_or(0);
        self.last_parse_failures = snapshot["event_transport"]["parseFailures"]
            .as_u64()
            .unwrap_or(0);
        Ok(snapshot["events"].as_array().cloned().unwrap_or_default())
    }

    async fn audio_evidence(&mut self) -> Result<AudioEvidence, Box<dyn std::error::Error>> {
        let snapshot = self.snapshot().await?;
        let audio = &snapshot["audio"];
        Ok(AudioEvidence {
            decoded_non_silent_frames: audio["decoded_non_silent_frames"].as_u64().unwrap_or(0),
            non_silent_frames: audio["non_silent_frames"].as_u64().unwrap_or(0),
            total_audio_energy: audio["total_audio_energy"].as_f64().unwrap_or(0.0),
            total_samples_received: audio["total_samples_received"].as_u64().unwrap_or(0),
        })
    }

    async fn close(mut self) {
        let _ = self.call(json!({"type":"close"})).await;
        let _ = self.child.kill().await;
    }
}

#[derive(Clone, Copy)]
struct AudioEvidence {
    decoded_non_silent_frames: u64,
    non_silent_frames: u64,
    total_audio_energy: f64,
    total_samples_received: u64,
}

async fn wait_for_spoken_output(
    peer: &mut BrowserPeer,
    baseline: AudioEvidence,
    timeout_secs: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    const REQUIRED_NEW_FRAMES: u64 = 2;
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let snapshot = peer.snapshot().await?;
        let audio = &snapshot["audio"];
        let non_silent_frames = audio["non_silent_frames"].as_u64().unwrap_or(0);
        let decoded_non_silent_frames = audio["decoded_non_silent_frames"].as_u64().unwrap_or(0);
        let total_audio_energy = audio["total_audio_energy"].as_f64().unwrap_or(0.0);
        let total_samples_received = audio["total_samples_received"].as_u64().unwrap_or(0);
        if decoded_non_silent_frames > baseline.decoded_non_silent_frames
            || non_silent_frames.saturating_sub(baseline.non_silent_frames) >= REQUIRED_NEW_FRAMES
            || (total_audio_energy > baseline.total_audio_energy
                && total_samples_received > baseline.total_samples_received)
        {
            return Ok(());
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
                "timed out waiting for spoken output; decoded_frames={decoded_frames}, decoded_non_silent_frames={decoded_non_silent_frames}, max_decoded_rms={max_decoded_rms:.6}, processor_supported={processor_supported}, processor_errors={processor_errors}, sampled_frames={sampled_frames}, non_silent_frames={non_silent_frames}, max_rms={max_rms:.6}, bytes_received={bytes_received}, packets_received={packets_received}, total_audio_energy={total_audio_energy:.6}, total_samples_received={total_samples_received}"
            )
            .into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_events<F>(
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
            let event_summary = browser_event_summary(&events);
            return Err(format!(
                "timed out waiting for provider events; raw_messages={}, parse_failures={}, {event_summary}",
                peer.last_raw_messages,
                peer.last_parse_failures
            )
            .into());
        }
        sleep(Duration::from_millis(250)).await;
    }
}

fn browser_event_kind_class(event: &Value) -> &'static str {
    match event.get("type").and_then(Value::as_str) {
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
    }
}

fn browser_event_summary(events: &[Value]) -> String {
    let mut kind_counts = std::collections::BTreeMap::<&'static str, usize>::new();
    let mut normalized_json_bytes = 0usize;
    for event in events {
        *kind_counts
            .entry(browser_event_kind_class(event))
            .or_default() += 1;
        normalized_json_bytes = normalized_json_bytes
            .saturating_add(serde_json::to_vec(event).map_or(0, |encoded| encoded.len()));
    }
    format!(
        "observed {} events across {} safe classes with normalized_json_bytes={normalized_json_bytes}: {kind_counts:?}",
        events.len(),
        kind_counts.len()
    )
}

#[cfg(test)]
mod browser_event_summary_tests {
    use super::browser_event_summary;

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

        let summary = browser_event_summary(&events);
        assert!(summary.contains("unknown"));
        assert!(summary.contains("turn.done"));
        assert!(!summary.contains("FIXTURE_PRIVATE_UNKNOWN_KIND"));
        assert!(!summary.contains("FIXTURE_PRIVATE_BROWSER_PAYLOAD"));
        assert!(!summary.contains("FIXTURE_PRIVATE_TRANSCRIPT"));
    }
}

async fn delegated_executor_diagnostic(rpc: &mut JsonlRpcClient, mob_id: &str) -> String {
    let roster = match rpc.call("mob/members", json!({"mob_id":mob_id}), 30).await {
        Ok(roster) => roster,
        Err(error) => return format!("roster_error={error}"),
    };
    let Some(worker) = roster["members"].as_array().and_then(|members| {
        members.iter().find(|member| {
            member["agent_identity"]
                .as_str()
                .is_some_and(|id| id.starts_with("live-delegation-"))
        })
    }) else {
        return "worker=absent".to_string();
    };
    let Some(identity) = worker["agent_identity"].as_str() else {
        return "worker=present identity=invalid".to_string();
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
        Err(error) => return format!("worker={identity} status_error={error}"),
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
        Err(error) => return format!("worker={identity} history_error={error}"),
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
    format!(
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
    )
}

fn execution_identity(profile_id: &str) -> Value {
    json!({
        "version":"v1",
        "profile_id":profile_id
    })
}

#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_scenario_96_gpt_live_client_context_vertical() -> Result<(), Box<dyn std::error::Error>>
{
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            "oai_rt_rs::experimental::gpt_live=debug,meerkat_openai::gpt_live=debug,meerkat::experimental_gpt_live=warn,meerkat_runtime::meerkat_machine::runtime_control=debug,meerkat_mob_mcp::live_delegation=debug,meerkat_mob::runtime::delegation=debug",
        )
        .with_test_writer()
        .try_init();
    let tokens = required_tokens()?;
    let test_tmp_root = std::env::var_os("TEST_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&test_tmp_root)?;
    let temp = tempfile::Builder::new()
        .prefix("gpt-live-client-e2e-")
        .tempdir_in(test_tmp_root)?;
    let config = scenario_config();
    let binding = auth_binding();
    let token_store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(
        temp.path().join("xdg/meerkat/credentials"),
    ));
    let operator = meerkat::ExperimentalLiveOperatorConfig::gpt_live_client_context();
    let factory_identity = operator.factory().clone();
    let factory = meerkat::AgentFactory::new(temp.path().join("sessions"))
        .runtime_root(temp.path().join("runtime"))
        .project_root(temp.path().join("project"))
        .context_root(temp.path().join("project"))
        .builtins(true)
        .shell(true)
        .mob(true)
        .with_provider_auth_persistence(ProviderAuthPersistence::new(
            Arc::clone(&token_store),
            Arc::new(InMemoryCoordinator::new()),
        ))
        .with_experimental_live_admission(operator, [binding.realm.clone()]);
    let live_factory_owner = factory.clone();
    tokio::fs::create_dir_all(temp.path().join("project")).await?;
    let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
        config.clone(),
        meerkat_models::canonical(),
    ));
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let persistence = meerkat::PersistenceBundle::new(
        session_store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::new()) as Arc<dyn BlobStore>,
    );
    let runtime = Arc::new(SessionRuntime::new_with_config_store(
        factory.clone(),
        config.clone(),
        Arc::clone(&config_store),
        16,
        persistence,
        NotificationSink::noop(),
    ));
    runtime.set_realm_context(
        Some(binding.realm.clone()),
        None,
        Some("memory".to_string()),
    );
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        temp.path().join("config-state.json"),
    )));
    let token_key = TokenKey::from_auth_binding(&binding);
    let transition = runtime.auth_lease_handle().acquire_lease(
        &LeaseKey::from_auth_binding(&binding),
        meerkat_core::persisted_token_expires_at_epoch_secs(&tokens),
    )?;
    let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
        &token_key,
        &tokens,
        &transition,
    )?;
    token_store.save(&token_key, &marked).await?;
    drop(tokens);

    let callback_rx = runtime.init_callback_channel();
    let mobs = meerkat_rpc::router::compose_rpc_mob_state(&runtime, &config_store, None);
    runtime.set_mob_state(Arc::clone(&mobs));
    let (client_stream, server_stream) = tokio::io::duplex(1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let mut rpc = JsonlRpcClient::new(client_stream);

    let projection = Arc::new(
        meerkat_rpc::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(&runtime)),
    );
    let live_host = Arc::new(meerkat_live::LiveAdapterHost::new(projection.clone()));
    let webrtc = Arc::new(meerkat_live::LiveWebrtcState::new(
        Arc::clone(&live_host),
        projection.clone(),
        projection.clone(),
    ));
    let realm_source = Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
        temp.path().join("realm-state"),
        temp.path().join("global-config.toml"),
        meerkat_models::canonical(),
    ));
    let live_factory = meerkat_rpc::live_wiring::build_per_open_realtime_session_factory(
        &factory,
        Arc::clone(&config_store),
        realm_source,
        binding.realm.clone(),
    );
    let mut server = RpcServer::new_with_skill_runtime_and_mob_state(
        BufReader::new(server_read),
        server_write,
        Arc::clone(&runtime),
        Arc::clone(&config_store),
        None,
        Arc::clone(&mobs),
        callback_rx,
    )
    .with_live_session_factory_opt(Some(live_factory))
    .with_live_webrtc(webrtc);

    let server_task = tokio::spawn(async move { server.run().await });
    rpc.call("initialize", json!({}), 60).await?;
    let mob_id = format!("gpt-live-client-e2e-{}", std::process::id());
    rpc.call(
        "mob/create",
        json!({"definition":{"id":mob_id,"profiles":{"executor":{
            "model":std::env::var("GPT_LIVE_E2E_EXECUTOR_MODEL").unwrap_or_else(|_| "gpt-5.6-sol".to_string()),
            "runtime_mode":"turn_driven","external_addressable":true,
            "tools":{"builtins":true,"shell":true,"comms":true}
        }}}}),
        60,
    )
    .await?;
    rpc.call(
        "mob/spawn",
        json!({"mob_id":mob_id,"profile":"executor","agent_identity":"voice-executor",
            "runtime_mode":"turn_driven",
            "auth_binding":{"realm":REALM,"binding":BINDING}}),
        60,
    )
    .await?;
    let status = rpc
        .call(
            "mob/member_status",
            json!({"mob_id":mob_id,"agent_identity":"voice-executor"}),
            60,
        )
        .await?;
    let session_id = meerkat_core::SessionId::parse(
        status["current_session_id"]
            .as_str()
            .ok_or("spawned executor has no durable session")?,
    )?;

    let experimental_transport = Arc::new(ExperimentalGptLiveWebrtcTransport::new());
    let open_authority = Arc::new(ExperimentalGptLiveOpenAuthority::new(
        ExperimentalGptLiveOpenAuthorityConfig {
            agent_factory: factory.clone(),
            config_source: Arc::new(FixedConfigSource(config)),
            binding_authority: Arc::new(ExplicitScenarioBindingAuthority {
                session_id: session_id.clone(),
                binding: binding.clone(),
                auth_lease: runtime.generated_auth_lease_handle(),
                mobs: Arc::clone(&mobs),
            }),
            execution_identity: meerkat_core::SessionLlmIdentity {
                model: "gpt-live-1-codex".to_string(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: Some(binding.clone()),
            },
            realm: binding.realm.clone(),
            factory_identity,
            transport: Arc::clone(&experimental_transport),
            voice: "cove".to_string(),
        },
    )?);
    // Rebuild the connection host with the exact authenticated authority. The
    // authority is a public production constructor; no Gate0 test witness is
    // minted by this fixture.
    drop(rpc);
    server_task.abort();
    let _ = server_task.await;
    let (client_stream, server_stream) = tokio::io::duplex(1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let mut rpc = JsonlRpcClient::new(client_stream);
    let callback_rx = runtime.init_callback_channel();
    let projection = Arc::new(
        meerkat_rpc::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(&runtime)),
    );
    let live_host = Arc::new(meerkat_live::LiveAdapterHost::new(projection.clone()));
    let webrtc = Arc::new(meerkat_live::LiveWebrtcState::new(
        live_host,
        projection.clone(),
        projection,
    ));
    let realm_source = Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
        temp.path().join("realm-state"),
        temp.path().join("global-config.toml"),
        meerkat_models::canonical(),
    ));
    let live_factory = meerkat_rpc::live_wiring::build_per_open_realtime_session_factory(
        &live_factory_owner,
        Arc::clone(&config_store),
        realm_source,
        binding.realm.clone(),
    );
    let mut server = RpcServer::new_with_skill_runtime_and_mob_state(
        BufReader::new(server_read),
        server_write,
        Arc::clone(&runtime),
        Arc::clone(&config_store),
        None,
        Arc::clone(&mobs),
        callback_rx,
    )
    .with_live_session_factory_opt(Some(live_factory))
    .with_live_webrtc(webrtc)
    .with_live_webrtc_answer_transport(experimental_transport)
    .with_experimental_live_open_authority(open_authority);
    let server_task = tokio::spawn(async move { server.run().await });
    rpc.call("initialize", json!({}), 60).await?;

    let rejected = rpc
        .call_raw(
            "live/open",
            json!({"session_id":session_id,"transport":"webrtc",
                "execution_identity":execution_identity(FUNCTION_BRIDGE_PROFILE)}),
            30,
        )
        .await?;
    assert!(
        !rejected["error"].is_null(),
        "FunctionBridge must fail closed"
    );
    let open = rpc
        .call(
            "live/open",
            json!({"session_id":session_id,"transport":"webrtc",
                "execution_identity":execution_identity(CLIENT_PROFILE)}),
            60,
        )
        .await?;

    let mut peer = BrowserPeer::start().await?;
    let offer = peer.call(json!({"type":"prepare"})).await?;
    let answer = rpc
        .call(
            open["transport"]["answer_method"]
                .as_str()
                .unwrap_or("live/webrtc/answer"),
            json!({"channel_id":open["channel_id"],"token":open["transport"]["token"],
                "offer_sdp":offer["offer_sdp"]}),
            90,
        )
        .await?;
    peer.call(json!({"type":"answer","answer_sdp":answer["answer_sdp"]}))
        .await?;

    peer.call(json!({"type":"arm_barge_in","name":"greeting"}))
        .await?;
    let greeting_audio_baseline = peer.audio_evidence().await?;
    let before = peer.events().await?.len();
    peer.call(json!({"type":"play","name":"greeting"})).await?;
    let interrupted_output = rpc
        .wait_for_notification("live/assistant_output_available", 30)
        .await?;
    assert_eq!(interrupted_output["channel_id"], open["channel_id"]);
    let truncated = rpc
        .call(
            "live/truncate",
            json!({
                "channel_id": interrupted_output["channel_id"],
                "output_id": interrupted_output["output_id"],
                "audio_played_ms": 0,
            }),
            30,
        )
        .await?;
    assert_eq!(
        truncated["status"], "truncated",
        "barge-in must retire the interrupted playback owner before another assistant output"
    );
    let greeting = wait_for_events(&mut peer, 90, |events| {
        let turn = &events[before..];
        turn.iter()
            .filter(|event| event["type"] == "turn.done" && event["turn"]["role"] == "user")
            .count()
            >= 2
            && turn
                .iter()
                .filter(|event| {
                    event["type"] == "turn.done" && event["turn"]["role"] == "assistant"
                })
                .count()
                >= 2
    })
    .await?;
    let snapshot = peer.snapshot().await?;
    assert_eq!(
        snapshot["barge_in"]["failures"].as_u64(),
        Some(0),
        "browser failed to start the armed barge-in fixture"
    );
    let barge_start = snapshot["barge_in"]["starts"]
        .as_array()
        .and_then(|starts| starts.first())
        .ok_or("browser did not start the armed barge-in fixture")?;
    let interrupted_assistant = &barge_start["assistant_turn_id"];
    let event_count_at_start: usize = barge_start["event_count_at_start"]
        .as_u64()
        .ok_or("browser barge-in evidence has no event count")?
        .try_into()?;
    let interrupted_start_index = greeting
        .iter()
        .position(|event| {
            event["type"] == "turn.created"
                && event["turn"]["role"] == "assistant"
                && &event["turn"]["id"] == interrupted_assistant
        })
        .ok_or("armed barge-in has no exact assistant start")?;
    let interrupted_done_index = greeting
        .iter()
        .position(|event| {
            event["type"] == "turn.done"
                && event["turn"]["role"] == "assistant"
                && &event["turn"]["id"] == interrupted_assistant
        })
        .ok_or("armed barge-in has no exact assistant terminal")?;
    assert_eq!(
        event_count_at_start,
        interrupted_start_index + 1,
        "the browser must start barge-in audio synchronously at the exact assistant-start boundary"
    );
    assert!(
        interrupted_done_index >= event_count_at_start,
        "barge-in audio must start before the interrupted assistant terminalizes"
    );
    assert!(
        greeting
            .iter()
            .enumerate()
            .any(|(index, event)| index >= event_count_at_start
                && event["type"] == "turn.created"
                && event["turn"]["role"] == "user"),
        "provider did not admit the user turn started by the armed barge-in audio"
    );
    wait_for_spoken_output(&mut peer, greeting_audio_baseline, 30).await?;
    let greeting_output = rpc
        .wait_for_notification("live/assistant_output_available", 30)
        .await?;
    assert_eq!(greeting_output["channel_id"], open["channel_id"]);
    rpc.call(
        "live/playback_complete",
        json!({
            "channel_id": greeting_output["channel_id"],
            "output_id": greeting_output["output_id"],
        }),
        30,
    )
    .await?;
    assert!(
        !greeting[before..]
            .iter()
            .any(|event| event["type"] == "delegation.created"),
        "simple greeting must remain in the same live conversation; {}",
        browser_event_summary(&greeting[before..])
    );

    let before = greeting.len();
    let delegation_audio_baseline = peer.audio_evidence().await?;
    peer.call(json!({"type":"play","name":"delegation"}))
        .await?;
    let joined = wait_for_events(&mut peer, 120, |events| {
        let events = &events[before..];
        let Some(delegation) = events
            .iter()
            .find(|event| event["type"] == "delegation.created")
        else {
            return false;
        };
        let turn_id = &delegation["item"]["user_bidi_turn_id"];
        events.iter().any(|event| {
            event["type"] == "turn.done"
                && event["turn"]["role"] == "user"
                && &event["turn"]["id"] == turn_id
        })
    })
    .await?;
    let delegation = joined[before..]
        .iter()
        .find(|event| event["type"] == "delegation.created")
        .expect("joined delegation");
    assert_eq!(delegation["item"]["target"], "client");
    let provider_delegation_ref = delegation["item"]["id"]
        .as_str()
        .ok_or("joined delegation has no provider item id")?
        .to_string();
    let joined_user_turn_id = delegation["item"]["user_bidi_turn_id"]
        .as_str()
        .ok_or("joined delegation has no provider user turn id")?
        .to_string();
    let acknowledgement_output = rpc
        .wait_for_notification("live/assistant_output_available", 30)
        .await?;
    assert_eq!(acknowledgement_output["channel_id"], open["channel_id"]);
    wait_for_spoken_output(&mut peer, delegation_audio_baseline, 30).await?;
    rpc.call(
        "live/playback_complete",
        json!({
            "channel_id": acknowledgement_output["channel_id"],
            "output_id": acknowledgement_output["output_id"],
        }),
        30,
    )
    .await?;
    let append_deadline = Instant::now() + Duration::from_secs(240);
    let acked = loop {
        let events = peer.events().await?;
        if events[before..].iter().any(|event| {
            event["type"] == "delegation.context.appended"
                && event["delegation_item_id"].as_str() == Some(provider_delegation_ref.as_str())
        }) {
            break events;
        }
        let now = Instant::now();
        if now >= append_deadline {
            let executor_diagnostic = timeout(
                Duration::from_secs(5),
                delegated_executor_diagnostic(&mut rpc, &mob_id),
            )
            .await
            .unwrap_or_else(|_| "diagnostic=timed_out".to_string());
            return Err(format!(
                "timed out waiting for delegation context append; {executor_diagnostic}"
            )
            .into());
        }
        sleep(Duration::from_millis(250)).await;
    };
    let ack_index = acked
        .iter()
        .position(|event| {
            event["type"] == "delegation.context.appended"
                && event["delegation_item_id"].as_str() == Some(provider_delegation_ref.as_str())
        })
        .expect("exact context append ack");
    let joined_turn_done_index = acked
        .iter()
        .position(|event| {
            event["type"] == "turn.done"
                && event["turn"]["role"] == "user"
                && event["turn"]["id"].as_str() == Some(joined_user_turn_id.as_str())
        })
        .expect("exact joined user turn.done");
    assert!(
        joined_turn_done_index < ack_index,
        "exact joined user turn.done at index {joined_turn_done_index} must precede the exact delegation.context.appended at index {ack_index}"
    );
    let provider_delegation_ref_digest = format!(
        "sha256:{:x}",
        Sha256::digest(provider_delegation_ref.as_bytes())
    );
    assert_eq!(provider_delegation_ref_digest.len(), "sha256:".len() + 64);
    let post_append_audio_baseline = peer.audio_evidence().await?;
    wait_for_events(&mut peer, 90, |events| {
        events[ack_index + 1..]
            .iter()
            .any(|event| event["type"] == "turn.done" && event["turn"]["role"] == "assistant")
    })
    .await?;
    wait_for_spoken_output(&mut peer, post_append_audio_baseline, 30).await?;
    let delegated_output = rpc
        .wait_for_notification("live/assistant_output_available", 30)
        .await?;
    assert_eq!(delegated_output["channel_id"], open["channel_id"]);
    rpc.call(
        "live/playback_complete",
        json!({
            "channel_id": delegated_output["channel_id"],
            "output_id": delegated_output["output_id"],
        }),
        30,
    )
    .await?;

    let events = rpc
        .call(
            "mob/events",
            json!({"mob_id":mob_id,"after_cursor":0,"limit":200,"strict":true}),
            30,
        )
        .await?;
    assert!(
        events["events"].as_array().is_some_and(|events| {
            events.iter().any(|event| {
                event.pointer("/kind/type").and_then(Value::as_str) == Some("member_spawned")
                    && event
                        .pointer("/kind/agent_identity")
                        .and_then(Value::as_str)
                        .is_some_and(|identity| identity.starts_with("live-delegation-"))
            })
        }),
        "durable delegated executor spawn did not materialize in canonical mob events"
    );

    rpc.call("live/close", json!({"channel_id":open["channel_id"]}), 30)
        .await?;
    peer.close().await;
    drop(rpc);
    server_task.abort();
    println!(
        "GPT_LIVE_CLIENT_E2E_OK delegation_ref_digest={provider_delegation_ref_digest} joined_turn_done_index={joined_turn_done_index} context_appended_index={ack_index}"
    );
    Ok(())
}

#[cfg(test)]
mod token_tests {
    use base64::Engine;

    use super::{PersistedTokens, prepare_remote_tokens};

    fn unsigned_jwt(payload: serde_json::Value) -> String {
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = engine.encode(br#"{"alg":"none"}"#);
        let payload = engine.encode(serde_json::to_vec(&payload).expect("encode JWT payload"));
        format!("{header}.{payload}.fixture-signature")
    }

    fn canonical_tokens(payload: serde_json::Value) -> PersistedTokens {
        serde_json::from_value(serde_json::json!({
            "auth_mode": "chatgpt_oauth",
            "primary_secret": "access-fixture",
            "refresh_token": "must-not-reach-remote",
            "id_token": unsigned_jwt(payload),
            "expires_at": 2_000,
            "scopes": [],
            "metadata": {}
        }))
        .expect("canonical token fixture")
    }

    #[test]
    fn remote_bundle_lifts_nested_account_id_and_strips_refresh_authority() {
        let tokens = canonical_tokens(serde_json::json!({
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct-nested"}
        }));
        let prepared = prepare_remote_tokens(tokens, 1_000).expect("prepare nested claim");
        assert_eq!(prepared.account_id.as_deref(), Some("acct-nested"));
        assert_eq!(prepared.refresh_token, None);
    }

    #[test]
    fn remote_bundle_lifts_top_level_account_id() {
        let tokens = canonical_tokens(serde_json::json!({
            "chatgpt_account_id": "acct-top-level"
        }));
        let prepared = prepare_remote_tokens(tokens, 1_000).expect("prepare top-level claim");
        assert_eq!(prepared.account_id.as_deref(), Some("acct-top-level"));
    }

    #[test]
    fn remote_bundle_fails_closed_without_supported_account_id() {
        let tokens = canonical_tokens(serde_json::json!({"sub": "user-only"}));
        let error = prepare_remote_tokens(tokens, 1_000).expect_err("missing account claim");
        assert!(
            error
                .to_string()
                .contains("no supported ChatGPT account-id claim"),
            "unexpected credential-maintenance error: {error}"
        );
    }
}
