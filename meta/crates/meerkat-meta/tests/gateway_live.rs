//! Gateway-level tests for the live (realtime) surface: opt-in gating,
//! method advertisement, identity-target resolution errors, and the mounted
//! WebSocket route. A full provider round-trip needs a realtime credential
//! and a live provider — out of scope here; the projection/token semantics
//! are unit-tested in `src/live_wiring.rs` and upstream. The
//! `cross_provider_open` module below drives the full open pipeline
//! in-process over meerkat's scripted realtime factory instead - no socket,
//! no credential - to pin the per-open provider selection semantics.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const MOB_CONFIG: &str = r#"
[mob]
id = "gateway-live-test"

[profiles.default]
model = "gpt-5.5"
external_addressable = true

[profiles.default.tools]
comms = true
"#;

struct Gateway {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Gateway {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines: rx,
        }
    }

    fn send(&mut self, value: Value) {
        writeln!(
            self.stdin,
            "{}",
            serde_json::to_string(&value).expect("request json")
        )
        .expect("write request");
        self.stdin.flush().expect("flush request");
    }

    fn wait_for_response(&mut self, id: &str, deadline: Duration) -> Value {
        let start = Instant::now();
        while start.elapsed() < deadline {
            let remaining = deadline.saturating_sub(start.elapsed());
            let Ok(line) = self.lines.recv_timeout(remaining) else {
                break;
            };
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("method").is_none()
                && message.get("id").and_then(Value::as_str) == Some(id)
            {
                return message;
            }
        }
        panic!("no response for id {id} within {deadline:?}");
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn init_params(state_dir: &tempfile::TempDir, live: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "runtime_options": { "live": live }
        }
    })
}

#[cfg(feature = "openai-live")]
fn openai_live_init_params(state_dir: &tempfile::TempDir) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "runtime_options": {
                "openai_live": {
                    "principal": "root",
                    "realm": "family",
                    "auth_binding": {
                        "realm": "family",
                        "binding": "openai-api-key"
                    },
                    "voice": "marin"
                }
            }
        }
    })
}

#[cfg(feature = "experimental-gpt-live")]
fn experimental_init_params(state_dir: &tempfile::TempDir) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "runtime_options": {
                "experimental_live": {
                    "principal": "root",
                    "realm": "family",
                    "factory_kind": "private-live",
                    "factory_version": "v1",
                    "gate0_qualification": "gate0-v1",
                    "auth_binding": {
                        "realm": "family",
                        "binding": "chatgpt-oauth"
                    },
                    "voice": "marin"
                }
            }
        }
    })
}

#[test]
fn live_methods_answer_unavailable_without_opt_in() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();
    gateway.send(init_params(&state_dir, json!(false)));
    let init = gateway.wait_for_response("init", Duration::from_mins(1));
    assert!(init["result"]["contract_version"].is_string(), "{init}");

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "open",
        "method": "meta/live/open",
        "params": { "identity": "worker-1" }
    }));
    let response = gateway.wait_for_response("open", Duration::from_secs(15));
    assert_eq!(response["error"]["code"], json!(-32050), "{response}");
    assert_eq!(response["error"]["data"]["kind"], json!("live_unavailable"));

    // The catalog does not advertise live methods either.
    gateway.send(json!({
        "jsonrpc": "2.0", "id": "caps", "method": "meta/capabilities", "params": {}
    }));
    let caps = gateway.wait_for_response("caps", Duration::from_secs(15));
    assert_eq!(
        caps["result"]["feature_capabilities"],
        json!(["flow_role_targets", "flow_run_idempotency_key"])
    );
    let methods = caps["result"]["methods"].as_array().expect("methods");
    assert!(!methods.iter().any(|m| m == "meta/live/open"), "{caps}");
}

#[cfg(feature = "openai-live")]
#[test]
fn openai_live_registration_is_app_opt_in_without_http_mount() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();
    gateway.send(openai_live_init_params(&state_dir));
    let init = gateway.wait_for_response("init", Duration::from_mins(1));
    assert!(init["result"]["contract_version"].is_string(), "{init}");
    let base = init["result"]["http_base_url"]
        .as_str()
        .expect("http_base_url");

    gateway.send(json!({
        "jsonrpc": "2.0", "id": "caps", "method": "meta/capabilities", "params": {}
    }));
    let caps = gateway.wait_for_response("caps", Duration::from_secs(15));
    assert_eq!(
        caps["result"]["feature_capabilities"],
        json!([
            "live.execution_identity.v1",
            "live.execution.client_context.v1",
            "flow_role_targets",
            "flow_run_idempotency_key"
        ]),
        "the public gpt-live-1 build must advertise only strict identity selection and client-context execution"
    );
    let methods = caps["result"]["methods"].as_array().expect("methods");
    assert!(
        methods.iter().any(|method| method == "meta/live/open"),
        "the explicit stdio registration must install the live handler: {caps}"
    );

    assert_eq!(
        ureq_get_status(&format!("{base}/live/ws")),
        404,
        "public stdio registration must not mount the HTTP live route"
    );
}

#[cfg(feature = "experimental-gpt-live")]
#[test]
fn experimental_live_registration_is_app_opt_in_without_http_mount() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();
    gateway.send(experimental_init_params(&state_dir));
    let init = gateway.wait_for_response("init", Duration::from_mins(1));
    assert!(init["result"]["contract_version"].is_string(), "{init}");
    let base = init["result"]["http_base_url"]
        .as_str()
        .expect("http_base_url");

    gateway.send(json!({
        "jsonrpc": "2.0", "id": "caps", "method": "meta/capabilities", "params": {}
    }));
    let caps = gateway.wait_for_response("caps", Duration::from_secs(15));
    assert_eq!(
        caps["result"]["feature_capabilities"],
        json!([
            "live.execution_identity.v1",
            "live.execution.client_context.v1",
            "flow_role_targets",
            "flow_run_idempotency_key"
        ]),
        "the qualified experimental build must advertise only strict identity selection and client-context execution"
    );
    let methods = caps["result"]["methods"].as_array().expect("methods");
    assert!(
        methods.iter().any(|method| method == "meta/live/open"),
        "the explicit stdio registration must install the live handler: {caps}"
    );

    assert_eq!(
        ureq_get_status(&format!("{base}/live/ws")),
        404,
        "experimental stdio registration must not mount the HTTP live route"
    );
}

#[test]
fn live_opt_in_advertises_methods_and_mounts_the_ws_route() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();
    gateway.send(init_params(&state_dir, json!(true)));
    let init = gateway.wait_for_response("init", Duration::from_mins(1));
    assert!(init["result"]["contract_version"].is_string(), "{init}");
    let base = init["result"]["http_base_url"]
        .as_str()
        .expect("http_base_url")
        .to_string();

    gateway.send(json!({
        "jsonrpc": "2.0", "id": "caps", "method": "meta/capabilities", "params": {}
    }));
    let caps = gateway.wait_for_response("caps", Duration::from_secs(15));
    assert_eq!(
        caps["result"]["feature_capabilities"],
        json!(["flow_role_targets", "flow_run_idempotency_key"]),
        "ordinary live attachment must not advertise experimental execution identity"
    );
    let methods = caps["result"]["methods"].as_array().expect("methods");
    for method in [
        "meta/live/open",
        "meta/live/status",
        "meta/live/close",
        "meta/live/refresh",
    ] {
        assert!(methods.iter().any(|m| m == method), "missing {method}");
    }

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "experimental-off",
        "method": "meta/live/open",
        "params": {
            "identity": "worker-1",
            "execution_identity": {"version": "v1", "profile_id": "example.device.open-room.v1"}
        }
    }));
    let unavailable = gateway.wait_for_response("experimental-off", Duration::from_secs(15));
    assert_eq!(unavailable["error"]["code"], json!(-32004));
    assert_eq!(
        unavailable["error"]["data"]["capability"],
        json!("live.execution_identity.v1")
    );

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "experimental-conflict",
        "method": "meta/live/open",
        "params": {
            "identity": "worker-1",
            "model": "legacy",
            "execution_identity": {"version": "v1", "profile_id": "example.device.open-room.v1"}
        }
    }));
    let conflict = gateway.wait_for_response("experimental-conflict", Duration::from_secs(15));
    assert_eq!(conflict["error"]["code"], json!(-32602));

    // Unresolvable member target → typed invalid params, not a hang.
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "open-missing",
        "method": "meta/live/open",
        "params": { "identity": "nobody-here" }
    }));
    let response = gateway.wait_for_response("open-missing", Duration::from_secs(15));
    assert_eq!(response["error"]["code"], json!(-32602), "{response}");

    // The WS route is mounted on the gateway's OWN listener: a tokenless
    // GET to /live/ws is rejected by the transport (any HTTP status proves
    // the route exists; an unmounted path would be the axum 404 fallback
    // with an empty body — the transport rejection carries one).
    let url = format!("{base}/live/ws");
    let response = ureq_get_status(&url);
    assert!(
        response != 404,
        "live ws route must be mounted (got 404 from {url})"
    );
}

/// Fix 5: `meta/live/truncate` is served (not method-not-found) and
/// answers TYPED errors - invalid params for an empty opaque output_id, and the
/// strict target rejection when no durable identity accompanies an active
/// receipt claim. A full truncate round-trip needs a live provider channel;
/// the command mapping is covered by the shared
/// `live_command_result_from_machine_authority` unit coverage.
#[cfg(feature = "openai-live")]
#[test]
fn live_truncate_answers_typed_errors_without_active_custody() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();
    gateway.send(init_params(&state_dir, json!(true)));
    let init = gateway.wait_for_response("init", Duration::from_mins(1));
    assert!(init["result"]["contract_version"].is_string(), "{init}");

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "truncate-empty-output",
        "method": "meta/live/truncate",
        "params": {
            "channel_id": "chan-1",
            "output_id": "",
            "audio_played_ms": 0
        }
    }));
    let response = gateway.wait_for_response("truncate-empty-output", Duration::from_secs(15));
    assert_eq!(response["error"]["code"], json!(-32602), "{response}");
    assert!(
        response["error"]["message"]
            .as_str()
            .expect("error message")
            .contains("output_id"),
        "{response}"
    );

    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "truncate-unbound",
        "method": "meta/live/truncate",
        "params": {
            "channel_id": "no-such-channel",
            "output_id": "opaque-output-1",
            "audio_played_ms": 1200,
            "activation_receipt": "not-a-real-activation-receipt"
        }
    }));
    let response = gateway.wait_for_response("truncate-unbound", Duration::from_secs(15));
    let error = response.get("error").expect("typed error, not a hang");
    assert!(
        error["message"]
            .as_str()
            .expect("error message")
            .contains("exactly one non-empty identity is required"),
        "strict truncate rejects receipt claims without durable identity authority: {response}"
    );
}

/// Fix 4 parse surface: `runtime_options.live.seed_max_chars` is accepted at
/// init (object form) — a bad value fails init loudly.
#[test]
fn live_object_form_accepts_seed_max_chars() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();
    gateway.send(init_params(&state_dir, json!({ "seed_max_chars": 200000 })));
    let init = gateway.wait_for_response("init", Duration::from_mins(1));
    assert!(init["result"]["contract_version"].is_string(), "{init}");
}

/// Fix 1 registration surface: `runtime_options.host_runnables` is accepted
/// at init on a persistent gateway (the schedule host composes the callback
/// runnables), and malformed values fail init loudly. The fire path
/// (callback/schedule_fire over the bridge) is unit-tested in
/// `schedule_wiring`; target-kind acceptance is pinned there too.
#[test]
fn init_accepts_host_runnables_and_rejects_duplicates() {
    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "runtime_options": { "host_runnables": ["digest", "backup.rotate"] }
        }
    }));
    let init = gateway.wait_for_response("init", Duration::from_mins(1));
    assert!(init["result"]["contract_version"].is_string(), "{init}");
    drop(gateway);

    let state_dir = tempfile::tempdir().expect("state dir");
    let mut gateway = Gateway::start();
    gateway.send(json!({
        "jsonrpc": "2.0",
        "id": "init",
        "method": "meta/init",
        "params": {
            "persistent_state": state_dir.path(),
            "mob_config": MOB_CONFIG,
            "runtime_options": { "host_runnables": ["digest", "digest"] }
        }
    }));
    let init = gateway.wait_for_response("init", Duration::from_mins(1));
    assert!(
        init["error"]["message"]
            .as_str()
            .expect("init error")
            .contains("duplicated"),
        "{init}"
    );
}

// ---------------------------------------------------------------------------
// In-process cross-provider open coverage (downstream-app regression): these tests
// call `handle_live_method` directly over a real PersistentSessionService +
// MeerkatMachine pair with meerkat's scripted realtime factory swapped in,
// so the exact channel identity handed to the provider lane is observable
// without a provider socket or credential (the binary harness above cannot
// mint a member session without live provider keys).
// ---------------------------------------------------------------------------
mod cross_provider_open {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use meerkat::test_fixtures::realtime::ScriptedRealtimeSessionFactory;
    use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
    use meerkat_client::TestClient;
    use meerkat_client::realtime_session::{
        RealtimeExternalSessionTarget, RealtimeSession, RealtimeSessionFactory,
        RealtimeSessionOpenConfig,
    };
    use meerkat_core::service::{
        CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
        SessionService as _,
    };
    use meerkat_core::types::SessionId;
    use meerkat_meta::live_wiring::{GatewayLiveContext, attach_live, handle_live_method};
    use meerkat_meta::rpc::JsonRpcResponse;
    use serde_json::{Value, json};

    static LIVE_OPEN_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct LiveOpenStack {
        ctx: GatewayLiveContext,
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        machine: Arc<meerkat_runtime::MeerkatMachine>,
        factory: Arc<ScriptedRealtimeSessionFactory>,
        /// The realtime seed of every open, in open order.
        seeds: Arc<SeedRecordingFactory>,
        session_id: SessionId,
        /// The member's state directory, where the run-start hook keeps its
        /// log and reads its refusal switch.
        state_path: std::path::PathBuf,
        _state: tempfile::TempDir,
    }

    /// The scripted factory, also recording the realtime seed each open
    /// hands the provider, so a row can read what the realtime model was
    /// given.
    struct SeedRecordingFactory {
        scripted: Arc<ScriptedRealtimeSessionFactory>,
        seeds: Mutex<Vec<Vec<meerkat_core::Message>>>,
    }

    impl SeedRecordingFactory {
        /// The user rows of each recorded seed, in open order.
        fn seeded_user_rows(&self) -> Vec<Vec<String>> {
            self.seeds
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|seed| user_row_texts(seed.as_slice()))
                .collect()
        }
    }

    #[async_trait]
    impl RealtimeSessionFactory for SeedRecordingFactory {
        fn capabilities(&self) -> meerkat_contracts::RealtimeCapabilities {
            self.scripted.capabilities()
        }

        fn supports_provider(&self, provider: meerkat_core::Provider) -> bool {
            self.scripted.supports_provider(provider)
        }

        async fn open_session(
            &self,
            open_config: &RealtimeSessionOpenConfig,
        ) -> Result<Box<dyn RealtimeSession>, meerkat_client::LlmError> {
            self.scripted.open_session(open_config).await
        }

        async fn attach_external_session(
            &self,
            target: &RealtimeExternalSessionTarget,
            open_config: &RealtimeSessionOpenConfig,
        ) -> Result<Box<dyn RealtimeSession>, meerkat_client::LlmError> {
            self.scripted
                .attach_external_session(target, open_config)
                .await
        }

        async fn open_live_adapter(
            &self,
            open_config: &RealtimeSessionOpenConfig,
        ) -> Result<Arc<dyn meerkat_core::live_adapter::LiveAdapter>, meerkat_client::LlmError>
        {
            self.seeds
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(open_config.seed_messages().to_vec());
            self.scripted.open_live_adapter(open_config).await
        }
    }

    /// The text of every user row in `messages`, in order.
    fn user_row_texts(messages: &[meerkat_core::Message]) -> Vec<String> {
        messages
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect()
    }

    /// What the fixture member session carries beyond the cross-provider
    /// shape.
    #[derive(Clone, Copy, Default)]
    struct MemberSetup {
        /// A create-time prompt staged for the session's first interaction
        /// (`InitialTurnPolicy::Defer` with `DeferredPromptPolicy::Stage`).
        staged_prompt: Option<&'static str>,
        /// The member's agent runs [`run_start_command_hook`].
        run_start_hook: bool,
    }

    /// A persistent service + machine pair carrying one Anthropic-identity
    /// member session (the downstream app's text-profile shape, realm-scoped auth
    /// binding included), with the live context's realtime factory swapped
    /// for the scripted fixture.
    async fn anthropic_member_stack() -> LiveOpenStack {
        anthropic_member_stack_with(MemberSetup::default()).await
    }

    /// [`anthropic_member_stack`] with the member session set up as `setup`
    /// says.
    async fn anthropic_member_stack_with(setup: MemberSetup) -> LiveOpenStack {
        let state = tempfile::tempdir().expect("state dir");
        let state_path = state.path().join("state");
        std::fs::create_dir_all(&state_path).expect("create state dir");
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
            meerkat_store::SqliteSessionStore::open(state_path.join("sessions.sqlite"))
                .expect("session store"),
        );
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(state_path.join("runtime.sqlite"))
                .expect("runtime store"),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let machine = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(
                Arc::clone(&runtime_store),
                Arc::clone(&blob_store),
            )
            .expect("acquire the gateway live fixture runtime machine"),
        );
        let agent_factory = AgentFactory::new(&state_path).builtins(false);
        let mut config = Config::default();
        if setup.run_start_hook {
            config
                .hooks
                .entries
                .push(run_start_command_hook(&state_path));
        }
        let mut builder = FactoryAgentBuilder::new(agent_factory.clone(), config);
        builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        builder.default_blob_store = Some(blob_store.clone());
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
        let service = Arc::new(PersistentSessionService::new(
            builder,
            16,
            session_store,
            Arc::clone(&runtime_store),
            blob_store,
        ));
        service
            .acquire_canonical_runtime_adapter(Some(Arc::clone(&machine)))
            .expect("acquire the gateway live fixture session runtime owner");

        // Runtime-backed create, the same shape every runtime surface uses:
        // pre-mint the session, prepare machine-owned bindings on the SAME
        // machine the live handlers consult, and pass SessionOwned - without
        // this the live lifecycle lease has no ops runtime to lease against.
        let session = meerkat_core::Session::new();
        let session_id = session.id().clone();
        let bindings = machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare runtime bindings");
        let created = service
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: meerkat_core::ContentInput::Text(
                    setup.staged_prompt.unwrap_or_default().to_string(),
                ),
                injected_context: Vec::new(),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: if setup.staged_prompt.is_some() {
                    DeferredPromptPolicy::Stage
                } else {
                    DeferredPromptPolicy::default()
                },
                build: Some(SessionBuildOptions {
                    provider: Some(meerkat_core::Provider::Anthropic),
                    auth_binding: Some(meerkat_core::AuthBindingRef {
                        realm: meerkat_core::RealmId::parse("mob.example").expect("realm id"),
                        binding: meerkat_core::BindingId::parse("anthropic-main")
                            .expect("binding id"),
                        profile: None,
                        origin: meerkat_core::BindingOrigin::Configured,
                    }),
                    resume_session: Some(session),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    ..SessionBuildOptions::default()
                }),
                labels: None,
            })
            .await
            .expect("create member session");
        assert_eq!(
            created.session_id, session_id,
            "create honors the pre-minted session"
        );

        // Production member hosts admit only mob-owned peer ingress. Record
        // the same ownership fact materialization installs before exercising
        // the member-host live path.
        let comms_name = format!("gateway-live-member-{session_id}");
        let comms: Arc<dyn meerkat_core::agent::CommsRuntime> = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only(&comms_name).expect("inproc comms runtime"),
        );
        machine
            .maybe_spawn_mob_comms_drain(
                &session_id,
                comms,
                meerkat_runtime::meerkat_machine::dsl::MobId::from("gateway-live-test"),
            )
            .await
            .expect("record mob-owned ingress for member session");

        let scripted = Arc::new(ScriptedRealtimeSessionFactory::new());
        let seeds = Arc::new(SeedRecordingFactory {
            scripted: Arc::clone(&scripted),
            seeds: Mutex::new(Vec::new()),
        });
        let mut ctx = attach_live(
            Arc::clone(&service),
            Arc::clone(&machine),
            &agent_factory,
            Config::default(),
            "ws://127.0.0.1:0".to_string(),
            None,
        );
        ctx.session_factory = Arc::clone(&seeds) as Arc<dyn RealtimeSessionFactory>;

        LiveOpenStack {
            ctx,
            service,
            machine,
            factory: scripted,
            seeds,
            session_id: created.session_id,
            state_path,
            _state: state,
        }
    }

    async fn open(stack: &LiveOpenStack, params: Value) -> JsonRpcResponse {
        handle_live_method(
            &stack.ctx,
            &stack.service,
            &stack.machine,
            meerkat_meta::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
            Some(stack.session_id.clone()),
            "meta/live/open",
            &params,
            json!("open-test"),
        )
        .await
    }

    #[cfg(not(feature = "openai-live"))]
    #[tokio::test]
    async fn default_live_projection_routes_text_and_spoken_identity_through_attached_host() {
        use meerkat_core::live_adapter::LiveAdapterObservation;
        use meerkat_core::{
            AssistantBlock, Message, StopReason, TranscriptSource, TurnUsage, Usage,
        };
        use meerkat_live::{LiveAdapterHostError, LiveChannelId, LiveProjectionError};

        let _open_guard = LIVE_OPEN_TEST_LOCK.lock().await;
        let stack = anthropic_member_stack().await;
        let opened = open(
            &stack,
            json!({"provider": "openai", "model": "gpt-realtime-2"}),
        )
        .await;
        assert!(
            opened.error.is_none(),
            "open default live stack: {opened:?}"
        );
        let result = opened.result.expect("live open result");
        let channel_id =
            LiveChannelId::new(result["channel_id"].as_str().expect("opened channel id"));

        let rejected = stack
            .ctx
            .host
            .apply_observation(
                &channel_id,
                &LiveAdapterObservation::AssistantTextDelta {
                    provider_item_id: Some("incomplete-item".into()),
                    previous_item_id: None,
                    content_index: Some(0),
                    response_id: None,
                    delta_id: Some("incomplete-delta".into()),
                    delta: "must not enter history".into(),
                },
            )
            .await;
        assert!(matches!(
            rejected,
            Err(LiveAdapterHostError::ProjectionError(
                LiveProjectionError::Rejected(_)
            ))
        ));

        let usage = || {
            TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                "gateway-projection-test",
                Usage::default(),
            )
        };
        for observation in [
            LiveAdapterObservation::AssistantTextDelta {
                provider_item_id: Some("display-item".into()),
                previous_item_id: None,
                content_index: Some(0),
                response_id: Some("display-response".into()),
                delta_id: Some("display-delta".into()),
                delta: "Visible response".into(),
            },
            LiveAdapterObservation::TurnCompleted {
                response_id: Some("display-response".into()),
                stop_reason: StopReason::EndTurn,
                usage: usage(),
            },
            LiveAdapterObservation::AssistantTranscriptDelta {
                provider_item_id: Some("spoken-item".into()),
                previous_item_id: Some("display-item".into()),
                content_index: Some(0),
                response_id: Some("spoken-response".into()),
                delta_id: Some("spoken-delta".into()),
                delta: "Spoken draft".into(),
            },
            LiveAdapterObservation::AssistantTranscriptFinal {
                provider_item_id: "spoken-item".into(),
                previous_item_id: Some("display-item".into()),
                content_index: Some(0),
                response_id: Some("spoken-response".into()),
                text: "Spoken response".into(),
                stop_reason: StopReason::EndTurn,
                usage: usage().into_inner(),
            },
            LiveAdapterObservation::TurnCompleted {
                response_id: Some("spoken-response".into()),
                stop_reason: StopReason::EndTurn,
                usage: usage(),
            },
        ] {
            stack
                .ctx
                .host
                .apply_observation(&channel_id, &observation)
                .await
                .expect("project through the production-selected sink");
        }

        let closed = handle_live_method(
            &stack.ctx,
            &stack.service,
            &stack.machine,
            meerkat_meta::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
            None,
            "meta/live/close",
            &json!({"channel_id": channel_id.as_str()}),
            json!("close-projection-test"),
        )
        .await;
        // Fully qualified: the only use is feature-gated, so a trait import
        // would be unused under other feature sets.
        let history = meerkat::SessionServiceHistoryExt::read_history(
            &*stack.service,
            &stack.session_id,
            meerkat_core::service::SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
        .expect("canonical history after live close");
        stack.service.shutdown().await;

        assert!(
            closed.error.is_none(),
            "close projected channel: {closed:?}"
        );
        assert!(!history.has_more);
        let rows: Vec<_> = history
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::BlockAssistant(row) => Some(row),
                _ => None,
            })
            .collect();
        assert_eq!(rows.len(), 2, "no rejected or duplicate display rows");
        assert!(
            matches!(rows[0].blocks.as_slice(), [AssistantBlock::Text { text, .. }] if text == "Visible response")
        );
        assert!(
            matches!(rows[1].blocks.as_slice(), [AssistantBlock::Transcript { text, source: TranscriptSource::Spoken, .. }] if text == "Spoken response")
        );
        for (row, item_id) in rows.iter().zip(["display-item", "spoken-item"]) {
            assert_eq!(row.stop_reason, Some(StopReason::EndTurn));
            assert!(
                row.assistant_message_id.is_none(),
                "realtime rows pair by provider item identity"
            );
            let origin = row
                .identity
                .realtime_origin
                .as_ref()
                .expect("default sink must retain realtime origin");
            assert_eq!(origin.provider_item_ids(), &[item_id.to_string()]);
            let wire = serde_json::to_value(origin).expect("origin wire");
            assert_eq!(wire["session_id"], json!(stack.session_id));
            assert_eq!(wire["channel_id"], json!(channel_id));
        }
    }

    /// (a) Downstream-app regression: an Anthropic-profile member opens a live
    /// channel with `provider = "openai"` + a realtime-capable model. The
    /// channel identity's (provider, model) pair must be re-paired BEFORE
    /// the B19 precheck and the inherited Anthropic auth binding cleared,
    /// so the open reaches the OpenAI realtime lane instead of dying as
    /// MODEL_NOT_REALTIME.
    ///
    /// The selection is CHANNEL-scoped: it mutates only the per-open
    /// `RealtimeSessionOpenConfig` projection, never the member's durable
    /// identity (`live_session_llm_identity` stays the Anthropic text
    /// identity, binding included, while the channel is live), and close
    /// reverts by construction - a model-only reopen after close hits the
    /// same typed MODEL_NOT_REALTIME rejection as a member that never
    /// opened cross-provider.
    #[tokio::test]
    async fn cross_provider_open_reaches_the_selected_providers_realtime_lane() {
        let _open_guard = LIVE_OPEN_TEST_LOCK.lock().await;
        let stack = anthropic_member_stack().await;
        let inherited = stack
            .service
            .live_session_llm_identity(&stack.session_id)
            .await
            .expect("member identity");
        assert_eq!(inherited.provider, meerkat_core::Provider::Anthropic);
        assert!(
            inherited.auth_binding.is_some(),
            "fixture must carry a binding to prove the open clears it"
        );

        let response = open(
            &stack,
            json!({
                "provider": "openai",
                "model": "gpt-realtime-2",
                "seed_max_chars": 200_000
            }),
        )
        .await;
        assert!(
            response.error.is_none(),
            "cross-provider open must pass the B19/B18 gates: {response:?}"
        );
        let result = response.result.expect("open result");
        let channel_id = result
            .get("channel_id")
            .and_then(Value::as_str)
            .expect("channel_id in open result")
            .to_string();

        let opens = stack.factory.opens();
        assert_eq!(opens.len(), 1, "exactly one provider-lane open");
        assert_eq!(opens[0].identity.provider, meerkat_core::Provider::OpenAI);
        assert_eq!(opens[0].identity.model, "gpt-realtime-2");
        assert_eq!(
            opens[0].identity.auth_binding, None,
            "the Anthropic binding must not ride into the OpenAI open"
        );

        // Channel-scoped, not session-scoped: while the cross-provider
        // channel is live, the member's DURABLE identity is untouched -
        // its next text turn still resolves the Anthropic identity with
        // its binding.
        let durable = stack
            .service
            .live_session_llm_identity(&stack.session_id)
            .await
            .expect("member identity while channel is live");
        assert_eq!(durable, inherited, "durable identity must not change");

        // Close reverts by construction: the override lived only on the
        // channel's open config, so after close a model-only reopen is
        // back to the typed cross-provider mismatch.
        let closed = handle_live_method(
            &stack.ctx,
            &stack.service,
            &stack.machine,
            meerkat_meta::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
            None,
            "meta/live/close",
            &json!({ "channel_id": channel_id }),
            json!("close-test"),
        )
        .await;
        assert!(closed.error.is_none(), "close must succeed: {closed:?}");

        let reopened = open(
            &stack,
            json!({"model": "gpt-realtime-2", "seed_max_chars": 200_000}),
        )
        .await;
        let error = reopened.error.expect("typed precheck rejection on reopen");
        assert_eq!(error.code, -32602, "{error:?}");
        assert_eq!(
            error.message,
            "model gpt-realtime-2 (provider anthropic) does not support realtime"
        );
    }

    /// (b) Omitted provider preserves today's behavior byte-for-byte: the
    /// cross-provider mismatch dies at the B19 precheck as the typed
    /// MODEL_NOT_REALTIME invalid-params rejection and never reaches the
    /// provider lane.
    #[tokio::test]
    async fn omitted_provider_keeps_the_typed_model_not_realtime_rejection() {
        let _open_guard = LIVE_OPEN_TEST_LOCK.lock().await;
        let stack = anthropic_member_stack().await;
        let response = open(
            &stack,
            json!({"model": "gpt-realtime-2", "seed_max_chars": 200_000}),
        )
        .await;
        let error = response.error.expect("typed precheck rejection");
        assert_eq!(error.code, -32602, "{error:?}");
        assert_eq!(
            error.message,
            "model gpt-realtime-2 (provider anthropic) does not support realtime"
        );
        assert_eq!(
            stack.factory.open_count(),
            0,
            "B19 must reject before the provider lane"
        );
    }

    /// (c) Strict vocabulary: an unrecognized provider string is a typed
    /// parameter error - never a silent fallthrough to the inherited
    /// provider - and `provider` without `model` is rejected because the
    /// pair is mutated together.
    #[tokio::test]
    async fn unknown_or_unpaired_provider_is_a_typed_parameter_error() {
        let _open_guard = LIVE_OPEN_TEST_LOCK.lock().await;
        let stack = anthropic_member_stack().await;

        let response = open(
            &stack,
            json!({"provider": "gpt", "model": "gpt-realtime-2"}),
        )
        .await;
        let error = response.error.expect("typed parameter error");
        assert_eq!(error.code, -32602, "{error:?}");
        assert!(
            error.message.contains("unknown provider 'gpt'"),
            "{error:?}"
        );

        let response = open(&stack, json!({"provider": "openai"})).await;
        let error = response.error.expect("typed parameter error");
        assert_eq!(error.code, -32602, "{error:?}");
        assert!(
            error.message.contains("requires an explicit `model`"),
            "{error:?}"
        );

        assert_eq!(stack.factory.open_count(), 0);
    }

    // -----------------------------------------------------------------------
    // A voice-first member's staged create-time prompt at `meta/live/open`.
    // Every row runs on both live paths: the ordinary gateway delivers the
    // prompt itself before it reads the seed, and an `openai-live` gateway's
    // open goes through Meerkat's own open projection, which delivers it.
    // -----------------------------------------------------------------------

    const STAGED: &str = "Remember the codeword ORBIT-7 for later.";
    const CODEWORD: &str = "ORBIT-7";
    /// The file in the member's state directory whose presence makes
    /// [`run_start_command_hook`] refuse the staged prompt.
    const REFUSE_STAGED: &str = "refuse-staged";

    /// The open every row makes: the member's text model is Anthropic, so the
    /// channel selects the OpenAI realtime lane.
    fn cross_provider_open_params() -> Value {
        json!({"provider": "openai", "model": "gpt-realtime-2"})
    }

    /// A foreground run-start command hook: it appends one line to
    /// `run-started.log` in `dir` for every `RunStarted` it is offered and,
    /// while [`REFUSE_STAGED`] exists in `dir`, denies a run whose input
    /// carries the staged prompt's codeword.
    ///
    /// Its capability is `Observe`. Meerkat rejects a mandatory `Guardrail`
    /// hook at `RunStarted` as an unsupported lifecycle gate, so every run of
    /// the member, and every staged-prompt delivery, would be refused before
    /// the hook ran. The engine honors a foreground hook's `Deny` whatever its
    /// capability.
    fn run_start_command_hook(dir: &std::path::Path) -> meerkat_core::HookEntryConfig {
        let script = r#"payload=$(cat)
printf 'run_started\n' >> "$1/run-started.log"
case "$payload" in
  *"$2"*)
    if [ -e "$1/$3" ]; then
      printf '%s' '{"decision":{"decision":"deny","hook_id":"staged-prompt-guard","reason_code":"policy_violation","message":"the staged prompt is refused"}}'
      exit 0
    fi
    ;;
esac
printf '%s' '{}'"#;
        meerkat_core::HookEntryConfig {
            id: meerkat_core::HookId::new("staged-prompt-guard"),
            point: meerkat_core::HookPoint::RunStarted,
            mode: meerkat_core::HookExecutionMode::Foreground,
            capability: meerkat_core::HookCapability::Observe,
            timeout_ms: Some(5_000),
            runtime: meerkat_core::HookAdapterConfig::command(
                "/bin/sh",
                vec![
                    "-c".into(),
                    script.into(),
                    "staged-prompt-guard".into(),
                    dir.display().to_string(),
                    CODEWORD.into(),
                    REFUSE_STAGED.into(),
                ],
                std::collections::HashMap::new(),
            ),
            ..Default::default()
        }
    }

    /// How many `RunStarted` invocations the member's run-start hook saw.
    fn run_start_hook_calls(stack: &LiveOpenStack) -> usize {
        std::fs::read_to_string(stack.state_path.join("run-started.log"))
            .map_or(0, |log| log.lines().count())
    }

    /// The text of every committed user row of the member's transcript.
    async fn committed_user_rows(stack: &LiveOpenStack) -> Vec<String> {
        let history = meerkat::SessionServiceHistoryExt::read_history(
            &*stack.service,
            &stack.session_id,
            meerkat_core::service::SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
        .expect("the member's committed history");
        user_row_texts(&history.messages)
    }

    /// Close the channel `opened` names.
    async fn close_opened(stack: &LiveOpenStack, opened: &JsonRpcResponse) -> JsonRpcResponse {
        let channel_id = opened
            .result
            .as_ref()
            .and_then(|result| result.get("channel_id"))
            .cloned()
            .unwrap_or(Value::Null);
        handle_live_method(
            &stack.ctx,
            &stack.service,
            &stack.machine,
            meerkat_meta::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
            None,
            "meta/live/close",
            &json!({ "channel_id": channel_id }),
            json!("close-staged-prompt-test"),
        )
        .await
    }

    /// The advisories an open result carries; `None` when it carries none.
    fn advisories(opened: &JsonRpcResponse) -> Option<Value> {
        opened
            .result
            .as_ref()
            .and_then(|result| result.get("advisories"))
            .cloned()
    }

    /// A voice-first member: its first live open delivers the staged
    /// create-time prompt once, through the run-start hook a turn carrying it
    /// would run, committed before the realtime seed is read, so the realtime
    /// model is given it. A second open finds nothing staged: it runs no hook
    /// and commits no second row, and its seed carries the prompt once, from
    /// the transcript. Neither open reports an advisory.
    #[tokio::test]
    async fn a_voice_first_open_delivers_the_staged_prompt_once() {
        let _open_guard = LIVE_OPEN_TEST_LOCK.lock().await;
        let stack = anthropic_member_stack_with(MemberSetup {
            staged_prompt: Some(STAGED),
            run_start_hook: true,
        })
        .await;

        let first = open(&stack, cross_provider_open_params()).await;
        let rows_after_first = committed_user_rows(&stack).await;
        let hook_calls_after_first = run_start_hook_calls(&stack);
        let closed = close_opened(&stack, &first).await;
        let second = open(&stack, cross_provider_open_params()).await;
        let rows = committed_user_rows(&stack).await;
        let hook_calls = run_start_hook_calls(&stack);
        let seeds = stack.seeds.seeded_user_rows();
        let _ = close_opened(&stack, &second).await;
        stack.service.shutdown().await;

        let staged = vec![STAGED.to_string()];
        assert!(
            first.error.is_none()
                && advisories(&first).is_none()
                && rows_after_first == staged
                && hook_calls_after_first == 1
                && closed.error.is_none()
                && second.error.is_none()
                && advisories(&second).is_none()
                && rows == staged
                && hook_calls == 1
                && seeds == [staged.clone(), staged],
            "the first open must deliver the staged prompt once, ahead of its seed, and a \
             second open must deliver nothing again: first={first:?} \
             rows_after_first={rows_after_first:?} \
             hook_calls_after_first={hook_calls_after_first} closed={closed:?} \
             second={second:?} rows={rows:?} hook_calls={hook_calls} seeds={seeds:?}"
        );
    }

    /// A voice-first member whose turn boundary a holder other than a turn
    /// has when the open arrives: the open delivers the staged prompt at the
    /// boundary, which its seed read waits for anyway, so it completes only
    /// once the boundary is free, and it still delivers the prompt once,
    /// ahead of its seed, with no advisory.
    #[tokio::test]
    async fn a_held_turn_boundary_delays_a_voice_first_open_which_still_delivers_once() {
        let _open_guard = LIVE_OPEN_TEST_LOCK.lock().await;
        let stack = anthropic_member_stack_with(MemberSetup {
            staged_prompt: Some(STAGED),
            run_start_hook: true,
        })
        .await;

        let boundary = stack
            .service
            .acquire_runtime_turn_finalization_guard(&stack.session_id)
            .await;
        let mut opening = std::pin::pin!(open(&stack, cross_provider_open_params()));
        let while_held =
            tokio::time::timeout(std::time::Duration::from_millis(250), opening.as_mut()).await;
        // Only the hook log is read while the boundary is held: the delivery
        // has not run its hook yet.
        let hook_calls_while_held = run_start_hook_calls(&stack);
        drop(boundary);
        let opened = tokio::time::timeout(std::time::Duration::from_secs(30), opening.as_mut())
            .await
            .expect("the open completes once the boundary is free");
        let rows = committed_user_rows(&stack).await;
        let hook_calls = run_start_hook_calls(&stack);
        let seeds = stack.seeds.seeded_user_rows();
        let _ = close_opened(&stack, &opened).await;
        stack.service.shutdown().await;

        let staged = vec![STAGED.to_string()];
        assert!(
            while_held.is_err()
                && hook_calls_while_held == 0
                && opened.error.is_none()
                && advisories(&opened).is_none()
                && rows == staged
                && hook_calls == 1
                && seeds == [staged],
            "a held boundary must delay the open, which must then deliver the staged prompt \
             once ahead of its seed: while_held={while_held:?} \
             hook_calls_while_held={hook_calls_while_held} opened={opened:?} rows={rows:?} \
             hook_calls={hook_calls} seeds={seeds:?}"
        );
    }

    /// A member with nothing staged: the open does no delivery work. The
    /// run-start hook never runs, nothing is committed, the seed carries no
    /// user row, and the open result carries no advisory.
    #[tokio::test]
    async fn an_open_with_nothing_staged_does_no_delivery_work() {
        let _open_guard = LIVE_OPEN_TEST_LOCK.lock().await;
        let stack = anthropic_member_stack_with(MemberSetup {
            staged_prompt: None,
            run_start_hook: true,
        })
        .await;

        let opened = open(&stack, cross_provider_open_params()).await;
        let rows = committed_user_rows(&stack).await;
        let hook_calls = run_start_hook_calls(&stack);
        let seeds = stack.seeds.seeded_user_rows();
        let _ = close_opened(&stack, &opened).await;
        stack.service.shutdown().await;

        assert!(
            opened.error.is_none()
                && advisories(&opened).is_none()
                && rows.is_empty()
                && hook_calls == 0
                && seeds == [Vec::<String>::new()],
            "an open with nothing staged must run no hook and commit nothing: \
             opened={opened:?} rows={rows:?} hook_calls={hook_calls} seeds={seeds:?}"
        );
    }

    /// A run-start hook that refuses the staged prompt at the open: the open
    /// goes ahead without it and its result carries the
    /// `staged_prompt_refused` advisory, nothing is committed, and the seed
    /// does not carry it. The prompt stays staged: once the hook admits it,
    /// the next open delivers it once and reports no advisory.
    #[tokio::test]
    async fn a_refused_staged_prompt_is_an_open_advisory_and_stays_staged() {
        let _open_guard = LIVE_OPEN_TEST_LOCK.lock().await;
        let stack = anthropic_member_stack_with(MemberSetup {
            staged_prompt: Some(STAGED),
            run_start_hook: true,
        })
        .await;
        let refusal_switch = stack.state_path.join(REFUSE_STAGED);
        std::fs::write(&refusal_switch, b"").expect("arm the staged-prompt refusal");

        let refused = open(&stack, cross_provider_open_params()).await;
        let rows_after_refusal = committed_user_rows(&stack).await;
        let closed = close_opened(&stack, &refused).await;
        std::fs::remove_file(&refusal_switch).expect("disarm the staged-prompt refusal");
        let admitted = open(&stack, cross_provider_open_params()).await;
        let rows = committed_user_rows(&stack).await;
        let hook_calls = run_start_hook_calls(&stack);
        let seeds = stack.seeds.seeded_user_rows();
        let _ = close_opened(&stack, &admitted).await;
        stack.service.shutdown().await;

        let staged = vec![STAGED.to_string()];
        assert!(
            refused.error.is_none()
                && advisories(&refused) == Some(json!(["staged_prompt_refused"]))
                && rows_after_refusal.is_empty()
                && closed.error.is_none()
                && admitted.error.is_none()
                && advisories(&admitted).is_none()
                && rows == staged
                && hook_calls == 2
                && seeds == [Vec::new(), staged],
            "a refused staged prompt must be the open's advisory and stay staged for the \
             next open, which delivers it once: refused={refused:?} \
             rows_after_refusal={rows_after_refusal:?} closed={closed:?} \
             admitted={admitted:?} rows={rows:?} hook_calls={hook_calls} seeds={seeds:?}"
        );
    }
}

/// Minimal blocking GET returning the HTTP status (no external HTTP dep —
/// hand-rolled over std TcpStream; the gateway listens on loopback).
fn ureq_get_status(url: &str) -> u16 {
    use std::io::{Read, Write as _};
    let rest = url.strip_prefix("http://").expect("http url");
    let (host, path) = rest.split_once('/').expect("path");
    let mut stream = std::net::TcpStream::connect(host).expect("connect");
    write!(
        stream,
        "GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .expect("write request");
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let head = String::from_utf8_lossy(&buf);
    head.split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("status code")
}
