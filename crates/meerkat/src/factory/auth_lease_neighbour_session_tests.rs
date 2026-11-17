//! A neighbour session's turn after another session of the same sign-in moved
//! to another credential.
//!
//! One host (one runtime-backed session service over one runtime machine)
//! serves two sessions of one ChatGPT sign-in: the real OpenAI runtime over a
//! configured ChatGPT-backend binding whose OAuth record sits in a managed
//! store, answered by a loopback Responses server. Session A then leaves that
//! credential for an Anthropic API-key binding, answered by a loopback
//! Messages server: by a model fallback after a rate limit with nothing
//! streamed, or by a live model switch. Nobody signed out and the stored
//! record is unchanged, so the neighbour session B is owed its next turn on
//! the sign-in, and a new session of the sign-in is owed its start.
//!
//! The same holds for an API-key credential: a session that runs on it as
//! its fallback, or has it as its fallback, keeps it when another session
//! leaves it. A sign-out or a removal still withdraws a credential from
//! every session, and a session leaves a credential without waiting for its
//! lifecycle.
//!
//! Each row records what the host returned under an `LN-RECORD` prefix.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use meerkat_core::auth::{PersistedAuthMode, PersistedTokens, TokenKey, TokenStore};
use meerkat_core::config::{ModelFallbackConfig, ModelFallbackTarget};
use meerkat_core::handles::{GeneratedAuthLeaseHandle, LeaseKey};
use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
};
use meerkat_core::{
    AuthBindingRef, AuthCredentialIdentity, BindingId, Config, Provider, RealmConfigSection,
    RealmId, Session, SessionId,
};
use meerkat_providers::auth_store::{
    EphemeralTokenStore, FileTokenStore, InMemoryCoordinator, ProviderAuthPersistence,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::{
    Input, MeerkatMachine, PromptInput, SessionLlmReconfigureRequest, SessionServiceRuntimeExt,
};

use crate::{
    AgentFactory, FactoryAgentBuilder, PersistenceBundle, PersistentSessionService, SessionStore,
};

/// The model both sessions start on, served by the sign-in.
const LOGIN_MODEL: &str = "gpt-5.4";
/// The model session A leaves for, served by the API-key binding.
const BACKUP_MODEL: &str = "claude-sonnet-4-5";
const LOGIN_BINDING: &str = "chatgpt_login";
const BACKUP_BINDING: &str = "default_anthropic";
/// A second API-key binding of the backup's provider: another credential
/// than [`BACKUP_BINDING`], for a session that leaves that one.
const SECOND_BINDING: &str = "anthropic_second";
const PROVIDER_ACCOUNT: &str = "chatgpt-provider-account";
const FIRST_SIGN_IN: &str = "sign-in-access-one";
const SECOND_SIGN_IN: &str = "sign-in-access-two";
const LOGIN_ANSWER: &str = "answered on the sign-in";
const BACKUP_ANSWER: &str = "answered on the backup";
/// A request whose text holds this is answered 429 by the sign-in's server.
const RATE_LIMITED_PROMPT: &str = "this request meets the rate limit";

/// One request as the sign-in's loopback server received it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LoginCall {
    authorization: String,
    model: String,
    rate_limited: bool,
}

#[derive(Default)]
struct Wire {
    login: StdMutex<Vec<LoginCall>>,
    backup: StdMutex<Vec<String>>,
}

impl Wire {
    fn login_calls(&self) -> Vec<LoginCall> {
        self.login
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn backup_calls(&self) -> usize {
        self.backup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// The requests a session that took one turn on the sign-in and then
    /// left it, and its neighbour's one turn, put on the wire.
    fn assert_after_a_left(&self, departure: Departure) {
        let calls = self.login_calls();
        let rate_limited = calls.iter().filter(|call| call.rate_limited).count();
        assert_eq!(
            (calls.len(), rate_limited, self.backup_calls()),
            match departure {
                Departure::Fallback => (3, 1, 1),
                Departure::LiveSwitch => (2, 0, 1),
            },
            "control: the requests each server received: {calls:?}"
        );
        assert!(
            calls
                .iter()
                .all(|call| call.authorization == format!("Bearer {FIRST_SIGN_IN}")),
            "control: every sign-in request carried the sign-in's token: {calls:?}"
        );
    }
}

/// The ChatGPT backend's Responses route: a 429 with nothing streamed for a
/// request that holds [`RATE_LIMITED_PROMPT`], a completed answer otherwise.
async fn login_responses(
    State(wire): State<Arc<Wire>>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::response::Response {
    let rate_limited = body.to_string().contains(RATE_LIMITED_PROMPT);
    wire.login
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(LoginCall {
            authorization: headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            model: body["model"].as_str().unwrap_or_default().to_string(),
            rate_limited,
        });
    if rate_limited {
        // The server's wait is stated: without it the retry policy waits its
        // 30 second floor before the fallback takes the retry.
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "1")],
            axum::Json(serde_json::json!({
                "error": {
                    "message": "Rate limit reached for requests",
                    "type": "requests",
                    "code": "rate_limit_exceeded",
                },
            })),
        )
            .into_response();
    }
    let event = serde_json::json!({
        "type": "response.completed",
        "response": {
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": LOGIN_ANSWER}],
            }],
            "usage": {"input_tokens": 1, "output_tokens": 1},
        },
    });
    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        format!("data: {event}\n\ndata: [DONE]\n\n"),
    )
        .into_response()
}

/// The Anthropic Messages route of the API-key binding.
async fn backup_messages(
    State(wire): State<Arc<Wire>>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::response::Response {
    wire.backup
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(body["model"].as_str().unwrap_or_default().to_string());
    let text_delta = serde_json::json!({
        "type": "content_block_delta",
        "delta": {"type": "text_delta", "text": BACKUP_ANSWER},
    });
    let payload = [
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":1,"output_tokens":0}}}"#
            .to_string(),
        r#"data: {"type":"content_block_start","content_block":{"type":"text","text":""}}"#
            .to_string(),
        format!("data: {text_delta}"),
        r#"data: {"type":"content_block_stop"}"#.to_string(),
        r#"data: {"type":"message_delta","usage":{"output_tokens":1},"delta":{"stop_reason":"end_turn"}}"#
            .to_string(),
        r#"data: {"type":"message_stop"}"#.to_string(),
        String::new(),
    ]
    .join("\n");
    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        payload,
    )
        .into_response()
}

/// The two loopback servers and what they received.
struct Servers {
    wire: Arc<Wire>,
    login_base: String,
    backup_base: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Servers {
    async fn start() -> Self {
        let wire = Arc::new(Wire::default());
        let login_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the sign-in's loopback server");
        let login_base = format!(
            "http://{}",
            login_listener.local_addr().expect("loopback address")
        );
        let backup_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the backup's loopback server");
        let backup_base = format!(
            "http://{}",
            backup_listener.local_addr().expect("loopback address")
        );
        let login_app = axum::Router::new()
            .route("/responses", axum::routing::post(login_responses))
            .with_state(Arc::clone(&wire));
        let backup_app = axum::Router::new()
            .route("/v1/messages", axum::routing::post(backup_messages))
            .with_state(Arc::clone(&wire));
        let tasks = vec![
            tokio::spawn(async move {
                let _ = axum::serve(login_listener, login_app).await;
            }),
            tokio::spawn(async move {
                let _ = axum::serve(backup_listener, backup_app).await;
            }),
        ];
        Self {
            wire,
            login_base,
            backup_base,
            tasks,
        }
    }
}

impl Drop for Servers {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn binding(realm: &str, binding: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(realm).expect("valid test realm"),
        binding: BindingId::parse(binding).expect("valid test binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

/// The host's config: the sign-in's ChatGPT-backend binding (a credential
/// account in the managed store) and two Anthropic API-key bindings, each on
/// its loopback server. The host configures no model fallback; a session
/// states its own.
fn host_config(realm: &str, account: &str, login_base: &str, backup_base: &str) -> Config {
    let mut section = RealmConfigSection::from_inline_api_keys(&[("anthropic", "test-backup-key")]);
    section
        .backend
        .get_mut(BACKUP_BINDING)
        .expect("the API-key backend is configured")
        .base_url = Some(backup_base.to_string());
    let second_backend = section.backend[BACKUP_BINDING].clone();
    let second_auth = section.auth[BACKUP_BINDING].clone();
    let mut second_binding = section.binding[BACKUP_BINDING].clone();
    second_binding.backend_profile = SECOND_BINDING.to_string();
    second_binding.auth_profile = SECOND_BINDING.to_string();
    second_binding.provider_default = false;
    section
        .backend
        .insert(SECOND_BINDING.to_string(), second_backend);
    section.auth.insert(SECOND_BINDING.to_string(), second_auth);
    section
        .binding
        .insert(SECOND_BINDING.to_string(), second_binding);
    section.backend.insert(
        LOGIN_BINDING.to_string(),
        meerkat_core::BackendProfileConfig {
            provider: Provider::OpenAI.as_str().to_string(),
            backend_kind: "chatgpt_backend".to_string(),
            base_url: Some(login_base.to_string()),
            options: serde_json::Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        LOGIN_BINDING.to_string(),
        meerkat_core::AuthProfileConfig {
            provider: Provider::OpenAI.as_str().to_string(),
            auth_method: "managed_chatgpt_oauth".to_string(),
            source: meerkat_core::CredentialSourceSpec::ManagedStore,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        LOGIN_BINDING.to_string(),
        meerkat_core::ProviderBindingConfig {
            backend_profile: LOGIN_BINDING.to_string(),
            auth_profile: LOGIN_BINDING.to_string(),
            credential_account: Some(
                meerkat_core::CredentialAccountId::parse(account).expect("valid account"),
            ),
            default_model: None,
            policy: Default::default(),
            provider_default: true,
        },
    );
    // An output budget both models serve: a fallback target that cannot
    // serve the session's budget is skipped (`OutputBudget`).
    let mut config = Config {
        max_tokens: Some(1024),
        ..Default::default()
    };
    config.realm.insert(realm.to_string(), section);
    config.retry.max_retries = 1;
    config.retry.initial_delay = std::time::Duration::ZERO;
    config.retry.max_delay = std::time::Duration::ZERO;
    config.retry.multiplier = 1.0;
    config
}

/// A session's own fallback: after one failed attempt, the Anthropic model on
/// the API-key binding, another credential than the sign-in.
fn fallback_to_the_backup(realm: &str) -> ModelFallbackConfig {
    let mut fallback = ModelFallbackConfig {
        enabled: Some(true),
        chain: vec![ModelFallbackTarget {
            model: BACKUP_MODEL.to_string(),
            provider: Some(Provider::Anthropic),
            auth_binding: Some(binding(realm, BACKUP_BINDING)),
        }],
        ..Default::default()
    };
    fallback.policy.cross_provider = true;
    fallback.policy.trigger_after_attempts = 1;
    fallback.policy.require_tool_parity = false;
    fallback
}

/// What a turn returned to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Turn {
    Answered(String),
    /// The runtime's outcome, as its debug form and as the JSON a surface
    /// serializes.
    Failed(String),
}

fn answered(text: &str) -> Turn {
    Turn::Answered(text.to_string())
}

/// The model and binding a session is created on.
#[derive(Debug, Clone, Copy)]
enum Route {
    /// [`LOGIN_MODEL`] on the sign-in.
    SignIn,
    /// [`BACKUP_MODEL`] on the first API-key binding.
    Backup,
}

impl Route {
    fn model(self) -> &'static str {
        match self {
            Self::SignIn => LOGIN_MODEL,
            Self::Backup => BACKUP_MODEL,
        }
    }

    fn provider(self) -> Provider {
        match self {
            Self::SignIn => Provider::OpenAI,
            Self::Backup => Provider::Anthropic,
        }
    }

    fn binding(self) -> &'static str {
        match self {
            Self::SignIn => LOGIN_BINDING,
            Self::Backup => BACKUP_BINDING,
        }
    }
}

/// One host: the runtime-backed service and its machine over one store
/// directory, and the credential store it resolves the sign-in from.
struct Host {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: Arc<MeerkatMachine>,
    owner: GeneratedAuthLeaseHandle,
    store: Arc<dyn TokenStore>,
    persistence: ProviderAuthPersistence,
    /// The root of a credential store on disk; `None` for one in memory.
    credential_root: Option<PathBuf>,
    /// The sign-in's credential.
    credential: AuthCredentialIdentity,
    /// The first API-key binding's credential.
    backup_credential: AuthCredentialIdentity,
    realm: String,
}

impl Host {
    /// Build the host of the row `label` over `directory`: a realm and a
    /// credential account of its own, since a credential account names one
    /// process-wide login lifecycle.
    fn over(
        directory: &Path,
        label: &str,
        login_base: &str,
        backup_base: &str,
        store: Arc<dyn TokenStore>,
        credential_root: Option<PathBuf>,
    ) -> Self {
        let realm = format!("neighbour_{label}");
        let account = format!("neighbour_{label}_sign_in");
        let config = host_config(&realm, &account, login_base, backup_base);
        let credential = config.realm[&realm].binding[LOGIN_BINDING]
            .credential_identity(&binding(&realm, LOGIN_BINDING));
        let backup_credential = config.realm[&realm].binding[BACKUP_BINDING]
            .credential_identity(&binding(&realm, BACKUP_BINDING));
        let persistence =
            ProviderAuthPersistence::new(Arc::clone(&store), Arc::new(InMemoryCoordinator::new()));
        let factory = AgentFactory::new(directory.join("factory-sessions"))
            .with_provider_auth_persistence(persistence.clone())
            .builtins(false);
        let builder = FactoryAgentBuilder::new(factory, config);
        let session_store: Arc<dyn SessionStore> =
            Arc::new(meerkat_store::JsonlStore::new(directory.join("jsonl")));
        let runtime_persistence = PersistenceBundle::new(
            session_store,
            Arc::new(
                meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(
                    directory.join("runtime.sqlite3"),
                )
                .expect("open the runtime store"),
            ),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("construct runtime authority");
        let (service, adapter) =
            crate::surface::build_runtime_backed_service_with_default_reconfigure_host(
                builder,
                8,
                runtime_persistence,
                directory.join("config_state.json"),
            )
            .expect("compose the runtime-backed service");
        let owner = adapter.generated_auth_lease_handle();
        Self {
            service,
            adapter,
            owner,
            store,
            persistence,
            credential_root,
            credential,
            backup_credential,
            realm,
        }
    }

    fn lease_key(&self) -> LeaseKey {
        LeaseKey::from_credential_identity(&self.credential)
    }

    /// A sign-in: an OAuth record published to the host's lifecycle and
    /// stored, as a login does.
    async fn sign_in(&self, access_token: &str) {
        let issued = chrono::Utc::now();
        let tokens = PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some(access_token.to_string()),
            refresh_token: Some(format!("{access_token}-refresh")),
            id_token: None,
            expires_at: Some(issued + chrono::Duration::hours(1)),
            last_refresh: Some(issued),
            scopes: Vec::new(),
            account_id: Some(PROVIDER_ACCOUNT.to_string()),
            metadata: serde_json::Value::Null,
        };
        let transition = meerkat_core::publish_token_lifecycle_acquired_for_identity(
            &self.owner,
            &self.credential,
            &tokens,
        )
        .expect("the host admits the sign-in");
        let key = TokenKey::from_credential_identity(&self.credential);
        let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &key,
            &tokens,
            &transition,
        )
        .expect("the record carries the lifecycle marker");
        let guard = match &self.credential_root {
            Some(root) => meerkat_core::AdmissionGuard::try_acquire(vec![
                meerkat_core::AdmissionFence::credentials(root, key.realm())
                    .expect("a credential fence"),
            ])
            .expect("the credential fence is free"),
            None => meerkat_core::AdmissionGuard::none(),
        };
        self.store
            .save(&guard, &key, &marked)
            .await
            .expect("store the sign-in");
    }

    /// The access token of the stored sign-in record.
    async fn stored_access_token(&self) -> Option<String> {
        self.store
            .load(&TokenKey::from_credential_identity(&self.credential))
            .await
            .expect("read the credential store")
            .and_then(|tokens| tokens.primary_secret)
    }

    async fn materialize(
        &self,
        session: Session,
        fresh: bool,
        route: Route,
        fallback: bool,
    ) -> Result<(), String> {
        let request = CreateSessionRequest {
            injected_context: Vec::new(),
            model: route.model().to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            // A resumed session keeps its persisted prompt, provider and
            // binding.
            system_prompt: if fresh {
                crate::SystemPromptOverride::Set("neighbour sessions of one sign-in".to_string())
            } else {
                crate::SystemPromptOverride::Inherit
            },
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                model_fallback: fallback.then(|| fallback_to_the_backup(&self.realm)),
                provider: fresh.then_some(route.provider()),
                realm_id: Some(RealmId::parse(&self.realm).expect("valid test realm")),
                auth_binding: fresh.then(|| binding(&self.realm, route.binding())),
                override_image_generation: meerkat_core::ToolCategoryOverride::Disable,
                ..Default::default()
            }),
            labels: None,
        };
        Box::pin(crate::surface::materialize_session(
            &self.service,
            &self.adapter,
            session,
            request,
            {
                let service = Arc::clone(&self.service);
                let adapter = Arc::clone(&self.adapter);
                move |session_id| {
                    crate::surface::default_persistent_executor(service, adapter, session_id)
                }
            },
        ))
        .await
        .map(|_| ())
        .map_err(|error| format!("{error} [{error:?}]"))
    }

    /// Create a session on the sign-in, with the fallback to the backup when
    /// `fallback`. A refusal is returned as the host would show it.
    async fn create_session(&self, fallback: bool) -> Result<SessionId, String> {
        self.create_session_on(Route::SignIn, fallback).await
    }

    /// Create a session on `route`, with the fallback to the backup when
    /// `fallback`.
    async fn create_session_on(&self, route: Route, fallback: bool) -> Result<SessionId, String> {
        let session = Session::new();
        let session_id = session.id().clone();
        self.materialize(session, true, route, fallback)
            .await
            .map(|()| session_id)
    }

    /// Attach the persisted session `session_id` again, as a host does for a
    /// session it finds in its store.
    async fn resume_session(&self, session_id: &SessionId) -> Result<(), String> {
        let session = self
            .service
            .load_authoritative_session(session_id)
            .await
            .expect("load the persisted session")
            .expect("the session stays durable");
        self.materialize(session, false, Route::SignIn, false).await
    }

    /// Switch `session` live to `model` of `provider` on the binding
    /// `binding_name`.
    async fn switch(
        &self,
        session: &SessionId,
        model: &str,
        provider: Provider,
        binding_name: &str,
    ) -> Result<(), String> {
        self.adapter
            .reconfigure_session_llm_identity(
                session,
                SessionLlmReconfigureRequest {
                    model: Some(model.to_string()),
                    provider: Some(provider.as_str().to_string()),
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: Some(TurnMetadataOverride::Set(binding(
                        &self.realm,
                        binding_name,
                    ))),
                    reasoning_effort: None,
                },
            )
            .await
            .map(|_| ())
            .map_err(|error| format!("{error} [{error:?}]"))
    }

    /// Withdraw `credential` as a sign-out and an auth profile removal do:
    /// the one coordinated clear of its stored record and its lifecycle.
    async fn withdraw(&self, credential: &AuthCredentialIdentity) -> Result<(), String> {
        meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
            self.persistence.clone(),
            self.owner.clone(),
            credential.clone(),
        )
        .await
        .map_err(|error| format!("{error} [{error:?}]"))
    }

    /// One turn of `session`, and what the host got back.
    async fn turn(&self, session: &SessionId, prompt: &str) -> Turn {
        let completion = match self
            .adapter
            .accept_input_with_completion(session, Input::Prompt(PromptInput::new(prompt, None)))
            .await
        {
            Ok((_, completion)) => completion.expect("a completion handle"),
            Err(error) => return Turn::Failed(format!("not accepted: {error} [{error:?}]")),
        };
        let outcome = tokio::time::timeout(std::time::Duration::from_mins(1), completion.wait())
            .await
            .expect("the turn settles in time")
            .expect("the completion resolves");
        match outcome {
            CompletionOutcome::Completed(run) => Turn::Answered(run.text.clone()),
            other => Turn::Failed(format!(
                "{other:?} | as JSON: {}",
                serde_json::to_string(&other).expect("the outcome serializes")
            )),
        }
    }

    /// Collect the model fallback events of `session` from now on, each in
    /// its debug form.
    async fn watch_model_fallback(&self, session: &SessionId) -> Arc<StdMutex<Vec<String>>> {
        use futures::StreamExt as _;

        let seen = Arc::new(StdMutex::new(Vec::new()));
        let mut events = self
            .service
            .subscribe_session_events(session)
            .await
            .expect("subscribe to the session's events");
        let sink = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let text = format!("{:?}", event.payload);
                if text.starts_with("ModelFallback") {
                    sink.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(text.chars().take(1200).collect());
                }
            }
        });
        seen
    }

    /// The live model and binding of `session`.
    async fn live_route(&self, session: &SessionId) -> (String, Option<AuthBindingRef>) {
        let identity = self
            .service
            .live_session_llm_identity(session)
            .await
            .expect("read the live identity");
        (identity.model, identity.auth_binding)
    }

    /// The host's status read of the sign-in: the call a status surface makes
    /// for a store-backed credential with no live lease. Returns what it
    /// answered, without the record.
    async fn status_read(&self) -> String {
        match meerkat_core::observe_marked_tokens_for_status_for_identity(
            self.store.as_ref(),
            &self.owner,
            &self.credential,
            PersistedAuthMode::ChatgptOauth,
            chrono::Utc::now(),
        )
        .await
        {
            Ok(meerkat_core::CredentialStatusRead::Answered(Some(_))) => {
                "answered with the stored record".to_string()
            }
            Ok(meerkat_core::CredentialStatusRead::Answered(None)) => {
                "answered with no record".to_string()
            }
            Ok(meerkat_core::CredentialStatusRead::OwnerBusy) => "owner busy".to_string(),
            Ok(_) => "unanswered".to_string(),
            Err(error) => format!("refused: {error}"),
        }
    }

    fn record_lease(&self, moment: &str) {
        let snapshot = self.owner.snapshot(&self.lease_key());
        eprintln!(
            "LN-RECORD: the sign-in's lease {moment}: phase={:?} credential_present={} generation={}",
            snapshot.phase, snapshot.credential_present, snapshot.generation
        );
    }

    fn record_backup_lease(&self, moment: &str) {
        let snapshot = self
            .owner
            .snapshot(&LeaseKey::from_credential_identity(&self.backup_credential));
        eprintln!(
            "LN-RECORD: the API-key binding's lease {moment}: phase={:?} credential_present={} generation={}",
            snapshot.phase, snapshot.credential_present, snapshot.generation
        );
    }

    /// A rate-limited turn of `session` (named `who` in the record), and the
    /// model fallback events it emitted.
    async fn rate_limited_turn(&self, session: &SessionId, who: &str) -> (Turn, Vec<String>) {
        let watched = self.watch_model_fallback(session).await;
        let turn = self.turn(session, RATE_LIMITED_PROMPT).await;
        // The session's events reach the watcher on a task of its own: wait
        // for the fallback's decision (a skip or a commit), not for a fixed
        // time. A turn that decided no fallback ends the wait at its bound.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let events = loop {
            let events = watched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let decided = events.iter().any(|event| {
                event.starts_with("ModelFallbackSkipped")
                    || event.starts_with("ModelFallbackCommitted")
            });
            if decided || tokio::time::Instant::now() >= deadline {
                break events;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        for event in &events {
            eprintln!("LN-RECORD: {who}'s model fallback event: {event}");
        }
        (turn, events)
    }
}

/// How session A leaves the sign-in's credential.
#[derive(Debug, Clone, Copy)]
enum Departure {
    /// A rate limit with nothing streamed, then its fallback.
    Fallback,
    /// A live model switch onto the API-key binding.
    LiveSwitch,
}

/// Sign in, start sessions A and B on the sign-in, let each take a first
/// turn, then let A leave for the backup credential and answer there.
/// Returns B. Every step is a control the rows require, whatever the host
/// does afterwards.
async fn a_leaves_the_sign_in(host: &Host, departure: Departure) -> SessionId {
    host.sign_in(FIRST_SIGN_IN).await;
    let a = host
        .create_session(true)
        .await
        .expect("session A starts on the sign-in");
    let b = host
        .create_session(false)
        .await
        .expect("session B starts on the sign-in");

    assert_eq!(
        host.turn(&a, "A's first turn").await,
        answered(LOGIN_ANSWER),
        "control: A answers on the sign-in"
    );
    assert_eq!(
        host.turn(&b, "B's first turn").await,
        answered(LOGIN_ANSWER),
        "control: B answers on the sign-in"
    );
    host.record_lease("before A leaves");

    match departure {
        Departure::Fallback => {
            let (turn, events) = host.rate_limited_turn(&a, "A").await;
            assert_eq!(
                turn,
                answered(BACKUP_ANSWER),
                "control: A's rate-limited turn is answered by its fallback; \
                 its model fallback events: {events:?}"
            );
        }
        Departure::LiveSwitch => {
            host.switch(&a, BACKUP_MODEL, Provider::Anthropic, BACKUP_BINDING)
                .await
                .expect("control: A switches to the backup model and binding");
            assert_eq!(
                host.turn(&a, "A's first turn after the switch").await,
                answered(BACKUP_ANSWER),
                "control: A answers on the backup after the switch"
            );
        }
    }
    assert_eq!(
        host.live_route(&a).await,
        (
            BACKUP_MODEL.to_string(),
            Some(binding(&host.realm, BACKUP_BINDING))
        ),
        "control: A is on the backup credential"
    );
    assert_eq!(
        host.live_route(&b).await,
        (
            LOGIN_MODEL.to_string(),
            Some(binding(&host.realm, LOGIN_BINDING))
        ),
        "control: B is still routed to the sign-in"
    );
    assert_eq!(
        host.stored_access_token().await.as_deref(),
        Some(FIRST_SIGN_IN),
        "control: the stored sign-in is unchanged"
    );
    host.record_lease(&format!("after A left ({departure:?})"));
    b
}

/// One host process's stage: its loopback servers, its store directory and
/// its credential store in memory.
struct Stage {
    servers: Servers,
    temp: tempfile::TempDir,
    store: Arc<dyn TokenStore>,
    label: &'static str,
}

impl Stage {
    async fn new(label: &'static str) -> Self {
        Self {
            servers: Servers::start().await,
            temp: tempfile::tempdir().expect("tempdir"),
            store: Arc::new(EphemeralTokenStore::new()),
            label,
        }
    }

    /// A host over the stage's store directory and credential store.
    fn host(&self) -> Host {
        Host::over(
            self.temp.path(),
            self.label,
            &self.servers.login_base,
            &self.servers.backup_base,
            Arc::clone(&self.store),
            None,
        )
    }

    fn login_requests(&self) -> usize {
        self.servers.wire.login_calls().len()
    }
}

/// The stage, its host and session B after A left the sign-in.
async fn neighbours_after_a_left(
    label: &'static str,
    departure: Departure,
) -> (Stage, Host, SessionId) {
    let stage = Stage::new(label).await;
    let host = stage.host();
    let b = a_leaves_the_sign_in(&host, departure).await;
    stage.servers.wire.assert_after_a_left(departure);
    (stage, host, b)
}

/// ROW 1. Session A falls back to a model on another credential after a rate
/// limit. Session B, on the same sign-in in the same host, takes its next
/// turn.
#[tokio::test]
async fn a_neighbour_sessions_turn_succeeds_after_another_sessions_cross_credential_fallback() {
    let (stage, host, b) = neighbours_after_a_left("fallback", Departure::Fallback).await;
    let sent_before = stage.login_requests();

    let turn = host.turn(&b, "B's second turn").await;
    eprintln!("LN-RECORD: B's turn after A's fallback returned: {turn:?}");
    eprintln!(
        "LN-RECORD: requests B's turn sent to the sign-in's server: {}",
        stage.login_requests() - sent_before
    );
    host.record_lease("after B's turn");
    assert_eq!(
        turn,
        answered(LOGIN_ANSWER),
        "B's sign-in was not withdrawn and its stored record is unchanged"
    );
}

/// ROW 2. After A's fallback, B takes three more turns and nothing else
/// happens in between: no sign-in, no restart.
#[tokio::test]
async fn a_neighbour_sessions_later_turns_succeed_without_a_new_sign_in() {
    let (_stage, host, b) = neighbours_after_a_left("later_turns", Departure::Fallback).await;

    let mut turns = Vec::new();
    for prompt in ["B's second turn", "B's third turn", "B's fourth turn"] {
        let turn = host.turn(&b, prompt).await;
        eprintln!("LN-RECORD: {prompt} with nothing done in between returned: {turn:?}");
        turns.push(turn);
    }
    host.record_lease("after B's three turns");
    assert_eq!(
        turns,
        vec![answered(LOGIN_ANSWER); 3],
        "each of B's turns is owed an answer on the unchanged sign-in"
    );
}

/// ROW 2, second half. After A's fallback and one more turn of B, the user
/// signs in again (a later record is published and stored); B then takes a
/// turn.
#[tokio::test]
async fn a_neighbour_sessions_turn_succeeds_after_a_new_sign_in() {
    let (stage, host, b) = neighbours_after_a_left("new_sign_in", Departure::Fallback).await;

    let before = host.turn(&b, "B's second turn").await;
    eprintln!("LN-RECORD: B's turn before the new sign-in returned: {before:?}");
    // A later publication than the first sign-in's, to the millisecond.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    host.sign_in(SECOND_SIGN_IN).await;
    host.record_lease("after the new sign-in");

    let after = host.turn(&b, "B's turn after the new sign-in").await;
    eprintln!("LN-RECORD: B's turn after the new sign-in returned: {after:?}");
    let last = stage.servers.wire.login_calls().last().cloned();
    eprintln!("LN-RECORD: the last request the sign-in's server received: {last:?}");
    assert_eq!(after, answered(LOGIN_ANSWER));
    assert_eq!(
        last.map(|call| call.authorization),
        Some(format!("Bearer {SECOND_SIGN_IN}")),
        "B's turn is sent with the new sign-in's token"
    );
}

/// After A's fallback, a new session of the same sign-in starts in the same
/// host and takes a turn.
#[tokio::test]
async fn a_new_session_of_the_sign_in_starts_after_another_sessions_fallback() {
    let (_stage, host, _b) = neighbours_after_a_left("new_session", Departure::Fallback).await;

    let created = host.create_session(false).await;
    eprintln!("LN-RECORD: a new session of the sign-in after A's fallback: {created:?}");
    let session = created.expect("the unchanged sign-in serves a new session");
    let turn = host.turn(&session, "the new session's first turn").await;
    eprintln!("LN-RECORD: the new session's first turn returned: {turn:?}");
    assert_eq!(turn, answered(LOGIN_ANSWER));
}

/// ROW 3. Session A is switched live to a model on another credential.
/// Session B, on the same sign-in in the same host, takes its next turn.
#[tokio::test]
async fn a_neighbour_sessions_turn_succeeds_after_another_sessions_live_switch_to_another_credential()
 {
    let (stage, host, b) = neighbours_after_a_left("live_switch", Departure::LiveSwitch).await;
    let sent_before = stage.login_requests();

    let turn = host.turn(&b, "B's second turn").await;
    eprintln!("LN-RECORD: B's turn after A's live switch returned: {turn:?}");
    eprintln!(
        "LN-RECORD: requests B's turn sent to the sign-in's server: {}",
        stage.login_requests() - sent_before
    );
    host.record_lease("after B's turn");
    assert_eq!(
        turn,
        answered(LOGIN_ANSWER),
        "B's sign-in was not withdrawn and its stored record is unchanged"
    );
}

/// ROW 4, inside one process. After A's fallback the host's service and
/// machine are dropped and built again over the same store directory, in
/// the same process; B is attached again from the store and takes a turn.
#[tokio::test]
async fn a_neighbour_sessions_turn_succeeds_after_the_runtime_is_rebuilt_in_the_same_process() {
    let (stage, host, b) = neighbours_after_a_left("rebuilt_runtime", Departure::Fallback).await;
    host.adapter
        .unregister_session(&b)
        .await
        .expect("tear down B's runtime attachment");
    drop(host);

    let rebuilt = stage.host();
    rebuilt.record_lease("in the rebuilt runtime, before B is attached");
    let resumed = rebuilt.resume_session(&b).await;
    eprintln!("LN-RECORD: attaching B in the rebuilt runtime: {resumed:?}");
    resumed.expect("the unchanged sign-in serves B's session again");
    let turn = rebuilt.turn(&b, "B's turn in the rebuilt runtime").await;
    eprintln!("LN-RECORD: B's turn in the rebuilt runtime returned: {turn:?}");
    rebuilt.record_lease("in the rebuilt runtime, after B's turn");
    assert_eq!(turn, answered(LOGIN_ANSWER));
}

/// A host with a sign-in, a session on the sign-in whose fallback is the
/// API-key binding, and session Y on that API-key binding. Both controls
/// are asserted. Returns the session with the fallback and Y.
async fn a_fallback_user_and_a_session_on_the_api_key(host: &Host) -> (SessionId, SessionId) {
    host.sign_in(FIRST_SIGN_IN).await;
    let with_fallback = host
        .create_session(true)
        .await
        .expect("the session with the fallback starts on the sign-in");
    let y = host
        .create_session_on(Route::Backup, false)
        .await
        .expect("session Y starts on the API-key binding");
    assert_eq!(
        host.turn(&y, "Y's first turn").await,
        answered(BACKUP_ANSWER),
        "control: Y answers on the API-key binding"
    );
    (with_fallback, y)
}

/// Session Y leaves the API-key binding for the second API-key binding, by a
/// live switch, and answers there.
async fn y_leaves_the_api_key(host: &Host, y: &SessionId) {
    host.record_backup_lease("before Y leaves");
    host.switch(y, BACKUP_MODEL, Provider::Anthropic, SECOND_BINDING)
        .await
        .expect("control: Y switches to the second API-key binding");
    assert_eq!(
        host.turn(y, "Y's turn after the switch").await,
        answered(BACKUP_ANSWER),
        "control: Y answers on the second API-key binding"
    );
    assert_eq!(
        host.live_route(y).await,
        (
            BACKUP_MODEL.to_string(),
            Some(binding(&host.realm, SECOND_BINDING))
        ),
        "control: Y is on the second API-key binding"
    );
    host.record_backup_lease("after Y left");
}

/// Session X runs on its fallback, the API-key binding. Session Y, on that
/// binding in the same host, leaves it for another credential. X takes its
/// next turns.
#[tokio::test]
async fn a_session_on_its_fallback_keeps_its_turns_after_another_session_leaves_that_credential() {
    let stage = Stage::new("running_on_fallback").await;
    let host = stage.host();
    let (x, y) = a_fallback_user_and_a_session_on_the_api_key(&host).await;
    let (fell_back, events) = host.rate_limited_turn(&x, "X").await;
    assert_eq!(
        fell_back,
        answered(BACKUP_ANSWER),
        "control: X's rate-limited turn is answered by its fallback; \
         its model fallback events: {events:?}"
    );
    assert_eq!(
        host.live_route(&x).await,
        (
            BACKUP_MODEL.to_string(),
            Some(binding(&host.realm, BACKUP_BINDING))
        ),
        "control: X runs on the API-key binding"
    );
    y_leaves_the_api_key(&host, &y).await;

    let mut turns = Vec::new();
    for prompt in ["X's next turn", "X's turn after that"] {
        let turn = host.turn(&x, prompt).await;
        eprintln!("LN-RECORD: {prompt}, after Y left X's credential, returned: {turn:?}");
        turns.push(turn);
    }
    host.record_backup_lease("after X's turns");
    assert_eq!(
        turns,
        vec![answered(BACKUP_ANSWER); 2],
        "nobody removed the API-key credential X runs on"
    );
}

/// Session Z is on the sign-in and has the API-key binding as its fallback.
/// Session Y, on that binding in the same host, leaves it for another
/// credential. Z's own model then answers a rate limit with nothing
/// streamed.
#[tokio::test]
async fn a_sessions_fallback_happens_after_another_session_left_the_fallbacks_credential() {
    let stage = Stage::new("owed_fallback").await;
    let host = stage.host();
    let (z, y) = a_fallback_user_and_a_session_on_the_api_key(&host).await;
    assert_eq!(
        host.turn(&z, "Z's first turn").await,
        answered(LOGIN_ANSWER),
        "control: Z answers on the sign-in"
    );
    y_leaves_the_api_key(&host, &y).await;

    let (turn, events) = host.rate_limited_turn(&z, "Z").await;
    eprintln!(
        "LN-RECORD: Z's rate-limited turn, after Y left its fallback's credential, returned: {turn:?}"
    );
    assert_eq!(
        turn,
        answered(BACKUP_ANSWER),
        "nobody removed the API-key credential Z falls back to; \
         its model fallback events: {events:?}"
    );
}

/// A sign-out withdraws the sign-in from every session of the host: two
/// running sessions lose their next turn and a new one is refused.
#[tokio::test]
async fn a_sign_out_withdraws_the_sign_in_from_every_session() {
    let stage = Stage::new("sign_out").await;
    let host = stage.host();
    host.sign_in(FIRST_SIGN_IN).await;
    let b = host.create_session(false).await.expect("session B starts");
    let c = host.create_session(false).await.expect("session C starts");
    for (session, name) in [(&b, "B"), (&c, "C")] {
        assert_eq!(
            host.turn(session, "a first turn").await,
            answered(LOGIN_ANSWER),
            "control: {name} answers on the sign-in"
        );
    }

    host.withdraw(&host.credential)
        .await
        .expect("the sign-out succeeds");
    host.record_lease("after the sign-out");
    assert_eq!(host.stored_access_token().await, None);
    for (session, name) in [(&b, "B"), (&c, "C")] {
        let turn = host.turn(session, "a turn after the sign-out").await;
        eprintln!("LN-RECORD: {name}'s turn after the sign-out returned: {turn:?}");
        assert!(
            matches!(&turn, Turn::Failed(text) if text.contains("auth lease absent")),
            "{name} must not be served by a signed-out credential: {turn:?}"
        );
    }
    let created = host.create_session(false).await;
    eprintln!("LN-RECORD: a new session after the sign-out: {created:?}");
    assert!(
        created.is_err(),
        "a signed-out credential serves no new session"
    );
}

/// After session A left the sign-in by its fallback, a sign-out still
/// withdraws the sign-in from every session: B loses its next turn and a new
/// session is refused.
#[tokio::test]
async fn a_sign_out_after_another_sessions_fallback_withdraws_the_sign_in_from_every_session() {
    let (_stage, host, b) = neighbours_after_a_left("sign_out_later", Departure::Fallback).await;

    let signed_out = host.withdraw(&host.credential).await;
    eprintln!("LN-RECORD: the sign-out after A's fallback: {signed_out:?}");
    signed_out.expect("the sign-out succeeds");
    host.record_lease("after the sign-out");
    assert_eq!(host.stored_access_token().await, None);
    let turn = host.turn(&b, "B's turn after the sign-out").await;
    eprintln!("LN-RECORD: B's turn after the sign-out returned: {turn:?}");
    assert!(
        matches!(&turn, Turn::Failed(text) if text.contains("auth lease absent")),
        "B must not be served by a signed-out credential: {turn:?}"
    );
    let created = host.create_session(false).await;
    eprintln!("LN-RECORD: a new session after the sign-out: {created:?}");
    assert!(
        created.is_err(),
        "a signed-out credential serves no new session"
    );
}

/// A removed credential is withdrawn from the session that runs on it as its
/// fallback: its next turn is held, with the text a caller reads for a
/// session whose fallback's credential is unavailable.
#[tokio::test]
async fn a_removed_credential_is_withdrawn_from_a_session_that_runs_on_it_as_its_fallback() {
    let stage = Stage::new("removed_under_fallback").await;
    let host = stage.host();
    host.sign_in(FIRST_SIGN_IN).await;
    let x = host.create_session(true).await.expect("session X starts");
    let (fell_back, events) = host.rate_limited_turn(&x, "X").await;
    assert_eq!(
        fell_back,
        answered(BACKUP_ANSWER),
        "control: X's rate-limited turn is answered by its fallback; \
         its model fallback events: {events:?}"
    );

    host.withdraw(&host.backup_credential)
        .await
        .expect("the removal succeeds");
    host.record_backup_lease("after the removal");
    let turn = host.turn(&x, "X's turn after the removal").await;
    eprintln!("LN-RECORD: X's turn after its fallback's credential was removed returned: {turn:?}");
    assert!(
        matches!(
            &turn,
            Turn::Failed(text)
                if text.contains("fallback-origin session requires explicit model reconfiguration")
                    && text.contains("AuthUnavailable")
        ),
        "X must not be served by a removed credential: {turn:?}"
    );
}

/// A removed credential is withdrawn from the session that has it as its
/// fallback: when its own model fails, the removed credential is skipped as
/// unavailable and the turn fails on its own model.
#[tokio::test]
async fn a_removed_credential_is_not_fallen_back_to() {
    let stage = Stage::new("removed_fallback_target").await;
    let host = stage.host();
    host.sign_in(FIRST_SIGN_IN).await;
    let z = host.create_session(true).await.expect("session Z starts");
    assert_eq!(
        host.turn(&z, "Z's first turn").await,
        answered(LOGIN_ANSWER),
        "control: Z answers on the sign-in"
    );

    host.withdraw(&host.backup_credential)
        .await
        .expect("the removal succeeds");
    host.record_backup_lease("after the removal");
    let (turn, events) = host.rate_limited_turn(&z, "Z").await;
    eprintln!(
        "LN-RECORD: Z's rate-limited turn after its fallback's credential was removed returned: {turn:?}"
    );
    assert!(
        matches!(&turn, Turn::Failed(text) if text.contains("Rate limited")),
        "Z's turn fails on its own model: {turn:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.starts_with("ModelFallbackSkipped")
                && event.contains("AuthUnavailable")),
        "Z's fallback to the removed credential is skipped as unavailable: {events:?}"
    );
}

/// A sign-out clears a sign-in that no session uses any more: its only
/// session left it for another credential before.
#[tokio::test]
async fn a_sign_out_clears_a_sign_in_no_session_uses_any_more() {
    let stage = Stage::new("sign_out_unused").await;
    let host = stage.host();
    host.sign_in(FIRST_SIGN_IN).await;
    let a = host.create_session(false).await.expect("session A starts");
    assert_eq!(
        host.turn(&a, "A's first turn").await,
        answered(LOGIN_ANSWER),
        "control: A answers on the sign-in"
    );
    host.switch(&a, BACKUP_MODEL, Provider::Anthropic, BACKUP_BINDING)
        .await
        .expect("control: A switches to the backup model and binding");
    assert_eq!(
        host.turn(&a, "A's first turn after the switch").await,
        answered(BACKUP_ANSWER),
        "control: A answers on the backup after the switch"
    );
    host.record_lease("after its only session left");

    let signed_out = host.withdraw(&host.credential).await;
    eprintln!("LN-RECORD: the sign-out of the unused sign-in: {signed_out:?}");
    signed_out.expect("the sign-out succeeds");
    host.record_lease("after the sign-out");
    assert_eq!(host.stored_access_token().await, None);
    assert!(
        !host.owner.snapshot(&host.lease_key()).credential_present,
        "the signed-out credential's lease is withdrawn"
    );
    let created = host.create_session(false).await;
    eprintln!("LN-RECORD: a new session after the sign-out: {created:?}");
    assert!(
        created.is_err(),
        "a signed-out credential serves no new session"
    );
}

/// A status read of the sign-in, B's next turn and a new session of the
/// sign-in, after the sign-in's lease came to read absent while its record
/// stayed stored.
async fn a_status_read_then_the_sign_ins_sessions(host: &Host, b: &SessionId) {
    assert_eq!(
        host.stored_access_token().await.as_deref(),
        Some(FIRST_SIGN_IN),
        "control: the sign-in's record is stored"
    );
    host.record_lease("before the status read");
    let read = host.status_read().await;
    eprintln!("LN-RECORD: the status read of the sign-in: {read}");
    host.record_lease("after the status read");

    let turn = host.turn(b, "B's turn after the status read").await;
    eprintln!("LN-RECORD: B's turn after the status read returned: {turn:?}");
    let created = host.create_session(false).await;
    eprintln!("LN-RECORD: a new session of the sign-in after the status read: {created:?}");
    assert_eq!(
        turn,
        answered(LOGIN_ANSWER),
        "the status read restores the stored sign-in for a running session"
    );
    created.expect("the status read restores the stored sign-in for a new session");
}

/// After session A left the sign-in by its fallback, the host reads the
/// sign-in's status; B then takes its next turn and a new session starts.
#[tokio::test]
async fn a_status_read_after_another_sessions_fallback_serves_the_sign_ins_sessions() {
    let (_stage, host, b) = neighbours_after_a_left("status_read_later", Departure::Fallback).await;
    a_status_read_then_the_sign_ins_sessions(&host, &b).await;
}

/// A status read restores a sign-in whose lease is released while its record
/// stays stored (the state a credential is left in when its release was not
/// followed by the removal of its record): B then takes its next turn and a
/// new session starts.
#[tokio::test]
async fn a_status_read_restores_a_released_sign_in_whose_record_is_stored() {
    let stage = Stage::new("status_read").await;
    let host = stage.host();
    host.sign_in(FIRST_SIGN_IN).await;
    let b = host.create_session(false).await.expect("session B starts");
    assert_eq!(
        host.turn(&b, "B's first turn").await,
        answered(LOGIN_ANSWER),
        "control: B answers on the sign-in"
    );
    meerkat_core::publish_token_lifecycle_released_for_identity(&host.owner, &host.credential)
        .expect("release the sign-in's lease, leaving its record stored");
    let turn = host.turn(&b, "B's turn after the release").await;
    assert!(
        matches!(&turn, Turn::Failed(text) if text.contains("auth lease absent")),
        "control: a released lease serves no turn: {turn:?}"
    );
    a_status_read_then_the_sign_ins_sessions(&host, &b).await;
}

/// A session is switched live away from the sign-in while another task of
/// the host holds the sign-in's credential lifecycle (as a status read, a
/// sign-in or another session's authorization does for a moment).
#[tokio::test]
async fn a_live_switch_succeeds_while_another_task_holds_the_sign_ins_lifecycle() {
    let stage = Stage::new("busy_lifecycle").await;
    let host = stage.host();
    host.sign_in(FIRST_SIGN_IN).await;
    let a = host.create_session(false).await.expect("session A starts");
    assert_eq!(
        host.turn(&a, "A's first turn").await,
        answered(LOGIN_ANSWER),
        "control: A answers on the sign-in"
    );

    let held = meerkat_core::acquire_auth_login_lifecycle_guard(&host.lease_key()).await;
    let switched = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        host.switch(&a, BACKUP_MODEL, Provider::Anthropic, BACKUP_BINDING),
    )
    .await;
    drop(held);
    eprintln!("LN-RECORD: A's live switch while the sign-in's lifecycle was held: {switched:?}");
    switched
        .expect("the switch does not wait for the sign-in's lifecycle")
        .expect("the switch leaves the sign-in without asking for its lifecycle");
    assert_eq!(
        host.turn(&a, "A's first turn after the switch").await,
        answered(BACKUP_ANSWER)
    );
    host.record_lease("after A's switch");
}

const RESTART_PHASE_ENV: &str = "MEERKAT_TEST_NEIGHBOUR_RESTART_PHASE";
const RESTART_DIR_ENV: &str = "MEERKAT_TEST_NEIGHBOUR_RESTART_DIR";
const RESTART_LOGIN_BASE_ENV: &str = "MEERKAT_TEST_NEIGHBOUR_RESTART_LOGIN_BASE";
const RESTART_BACKUP_BASE_ENV: &str = "MEERKAT_TEST_NEIGHBOUR_RESTART_BACKUP_BASE";
const RESTART_HOST_PROCESS: &str =
    "factory::auth_lease_neighbour_session_tests::restart_row_host_process";
const RESTART_SESSION_FILE: &str = "session-b";
const RESTART_TURN_FILE: &str = "turn-after-restart";

/// Run one host process of the restart row over `directory` and return what
/// it recorded.
#[allow(clippy::disallowed_methods)] // the test re-runs its own binary
async fn run_host_process(phase: &str, directory: &Path, servers: &Servers) -> Vec<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_mins(2),
        tokio::process::Command::new(std::env::current_exe().expect("this test binary"))
            .args(["--exact", RESTART_HOST_PROCESS, "--ignored", "--nocapture"])
            .env(RESTART_PHASE_ENV, phase)
            .env(RESTART_DIR_ENV, directory)
            .env(RESTART_LOGIN_BASE_ENV, &servers.login_base)
            .env(RESTART_BACKUP_BASE_ENV, &servers.backup_base)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("the host process ends in time")
    .expect("start the host process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("running 1 test") && output.status.success(),
        "host process ({phase}) failed: stdout={stdout}; stderr={stderr}"
    );
    stderr
        .lines()
        .filter(|line| line.contains("LN-RECORD"))
        .map(str::to_string)
        .collect()
}

/// ROW 4, across processes. One host process signs in, serves A and B, and
/// ends after A's fallback. A second host process is started over the same
/// store directory (runtime store, session store, credential files), attaches
/// B from the store and gives it a turn.
#[tokio::test]
async fn a_neighbour_sessions_turn_succeeds_after_the_host_process_restarts() {
    let servers = Servers::start().await;
    let temp = tempfile::tempdir().expect("tempdir");

    for line in run_host_process("before", temp.path(), &servers).await {
        eprintln!("first process: {line}");
    }
    servers.wire.assert_after_a_left(Departure::Fallback);
    let sent_before = servers.wire.login_calls().len();

    for line in run_host_process("after", temp.path(), &servers).await {
        eprintln!("second process: {line}");
    }
    let turn = std::fs::read_to_string(temp.path().join(RESTART_TURN_FILE))
        .expect("the second process recorded B's turn");
    eprintln!("LN-RECORD: B's turn in the restarted host process returned: {turn}");
    let sent = servers.wire.login_calls();
    eprintln!(
        "LN-RECORD: requests the restarted process sent to the sign-in's server: {:?}",
        &sent[sent_before..]
    );
    assert_eq!(turn, format!("{:?}", answered(LOGIN_ANSWER)));
}

/// One host process of the restart row. Only
/// [`a_neighbour_sessions_turn_succeeds_after_the_host_process_restarts`]
/// starts it.
#[tokio::test]
#[ignore = "subprocess entrypoint: one host process of the restart row"]
async fn restart_row_host_process() {
    let phase = std::env::var(RESTART_PHASE_ENV).expect("only the restart row starts this process");
    let directory = PathBuf::from(std::env::var(RESTART_DIR_ENV).expect("the store directory"));
    let login_base = std::env::var(RESTART_LOGIN_BASE_ENV).expect("the sign-in's server");
    let backup_base = std::env::var(RESTART_BACKUP_BASE_ENV).expect("the backup's server");
    let credential_root = directory.join("credentials");
    let host = Host::over(
        &directory,
        "process_restart",
        &login_base,
        &backup_base,
        Arc::new(FileTokenStore::new(&credential_root)),
        Some(credential_root),
    );
    match phase.as_str() {
        "before" => {
            let b = a_leaves_the_sign_in(&host, Departure::Fallback).await;
            std::fs::write(directory.join(RESTART_SESSION_FILE), b.to_string())
                .expect("record B's session id");
        }
        "after" => {
            let b = SessionId::parse(
                &std::fs::read_to_string(directory.join(RESTART_SESSION_FILE))
                    .expect("B's session id from the first process"),
            )
            .expect("a session id");
            assert_eq!(
                host.stored_access_token().await.as_deref(),
                Some(FIRST_SIGN_IN),
                "control: the first process's sign-in is in the credential files"
            );
            host.record_lease("in the new process, before B is attached");
            let resumed = host.resume_session(&b).await;
            eprintln!("LN-RECORD: attaching B in the new process: {resumed:?}");
            let turn = match resumed {
                Ok(()) => host.turn(&b, "B's turn in the new process").await,
                Err(refusal) => Turn::Failed(format!("B was not attached: {refusal}")),
            };
            host.record_lease("in the new process, after B's turn");
            std::fs::write(directory.join(RESTART_TURN_FILE), format!("{turn:?}"))
                .expect("record B's turn");
        }
        other => panic!("unknown phase {other}"),
    }
}
