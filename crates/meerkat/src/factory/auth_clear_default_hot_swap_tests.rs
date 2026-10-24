//! A hot swap that clears the session's auth binding keeps the credential its
//! default resolution actually selected.
//!
//! `TurnMetadataOverride::Clear` removes the session's binding selector; the
//! hosted client is then built from the realm's default candidate, which may
//! resolve the very account the session already holds. The swap must carry
//! that selection into the identity it applies and persists and into the
//! request policy whose credential the runner rotates to. Otherwise the
//! rotation sees "account A -> none", releases A's lease, and the next turn on
//! the new client (which still uses A) finds no lease.
//!
//! Two provider layers run the same scenario through the real runtime loop,
//! including a cold recreation of the persisted session:
//!
//! * a fake `ProviderRuntime` over a configured managed-store Copilot binding
//!   with a credential account, whose client fails a call when the account's
//!   lease is absent;
//! * the real OpenAI runtime over a configured ChatGPT-backend binding with
//!   external tokens in a managed store, answering through a loopback
//!   Responses server that records the bearer and account it was sent. The
//!   scenario checks the runtime's native lease presence after the swap and
//!   every turn, and verifies the HTTP requests that follow.

use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use futures::future::LocalBoxFuture;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::handles::{GeneratedAuthLeaseHandle, LeaseKey};
use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
};
use meerkat_core::{
    AuthBindingRef, AuthCredentialIdentity, BindingId, Config, Provider, RealmConfigSection,
    RealmId, Session, SessionId, SessionLlmIdentity, StopReason,
};
use meerkat_llm_core::provider_runtime::{
    ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
    ResolvedConnection, ResolvedTextTarget, ResolverEnvironment, StaticLease, ValidatedBinding,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::{
    Input, MeerkatMachine, PromptInput, SessionLlmReconfigureRequest, SessionServiceRuntimeExt,
};

use crate::{
    AgentFactory, FactoryAgentBuilder, PersistenceBundle, PersistentSessionService, SessionStore,
};

const MODEL_A: &str = "gpt-5.4";
const MODEL_B: &str = "gpt-5.5";
const TEST_REALM: &str = "default";
const TEST_BINDING: &str = "default_openai";
/// The managed-store scenario's credential account. A credential account
/// names one process-wide lease and login lifecycle (realm and account), so
/// no other test of this lib may configure it.
const TEST_ACCOUNT: &str = "hot_swap_account_a";
const ANSWER: &str = "answered";

fn configured_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(TEST_REALM).expect("valid test realm"),
        binding: BindingId::parse(TEST_BINDING).expect("valid test binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

fn test_config() -> Config {
    let mut config = Config::default();
    let mut section = RealmConfigSection::default();
    section.backend.insert(
        TEST_BINDING.to_string(),
        meerkat_core::BackendProfileConfig {
            provider: Provider::OpenAI.as_str().to_string(),
            backend_kind: "copilot".to_string(),
            base_url: None,
            options: serde_json::Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        TEST_BINDING.to_string(),
        meerkat_core::AuthProfileConfig {
            provider: Provider::OpenAI.as_str().to_string(),
            auth_method: "github_copilot_oauth".to_string(),
            source: meerkat_core::CredentialSourceSpec::ManagedStore,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        TEST_BINDING.to_string(),
        meerkat_core::ProviderBindingConfig {
            backend_profile: TEST_BINDING.to_string(),
            auth_profile: TEST_BINDING.to_string(),
            credential_account: Some(
                meerkat_core::CredentialAccountId::parse(TEST_ACCOUNT).expect("valid account"),
            ),
            default_model: None,
            policy: Default::default(),
            provider_default: true,
        },
    );
    config.realm.insert(TEST_REALM.to_string(), section);
    config
}

/// The lease authority and credential the provider resolved with, as a
/// managed-store provider sees them.
#[derive(Default)]
struct ResolvedLeaseView {
    lease: StdMutex<Option<(GeneratedAuthLeaseHandle, AuthCredentialIdentity)>>,
}

impl ResolvedLeaseView {
    fn lease_present(&self) -> Result<(), String> {
        let lease = self
            .lease
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((handle, credential)) = lease.as_ref() else {
            return Err("no lifecycle-backed resolution happened".to_string());
        };
        let snapshot = handle.snapshot(&LeaseKey::from_credential_identity(credential));
        if snapshot.credential_present {
            Ok(())
        } else {
            Err(format!(
                "auth lease absent for {credential:?}: {snapshot:?}"
            ))
        }
    }
}

/// Answers every call, but only while the resolved credential's lease is
/// present: the stand-in for a managed-store provider's `LeaseAbsent`.
struct LeaseCheckingClient {
    lease: Arc<ResolvedLeaseView>,
}

#[async_trait]
impl LlmClient for LeaseCheckingClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let events = match self.lease.lease_present() {
            Ok(()) => vec![
                Ok(LlmEvent::TextDelta {
                    delta: ANSWER.to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        Provider::OpenAI,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::EndTurn),
                }),
            ],
            Err(message) => vec![Err(LlmError::InvalidRequest { message })],
        };
        Box::pin(futures::stream::iter(events))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct LeaseRecordingRuntime {
    lease: Arc<ResolvedLeaseView>,
}

#[async_trait]
impl ProviderRuntime for LeaseRecordingRuntime {
    fn provider_id(&self) -> Provider {
        Provider::OpenAI
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        if let Some(handle) = env.auth_lease_handle.clone() {
            *self
                .lease
                .lease
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((handle, binding.credential_identity().clone()));
        }
        Ok(ResolvedConnection {
            provider: Provider::OpenAI,
            backend: binding.backend(),
            backend_profile: Arc::clone(binding.backend_profile()),
            credential_identity: binding.credential_identity().clone(),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "unused-test-token".to_string(),
                meerkat_core::AuthMetadata::default(),
                Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                "auth-clear-default-test",
            )),
        })
    }

    fn build_client(
        &self,
        _connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Ok(Arc::new(LeaseCheckingClient {
            lease: Arc::clone(&self.lease),
        }))
    }

    fn build_text_client(
        &self,
        _target: ResolvedTextTarget,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Ok(Arc::new(LeaseCheckingClient {
            lease: Arc::clone(&self.lease),
        }))
    }
}

/// The scenario's view of one attached runtime.
struct Harness {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
    lease_present: Box<dyn Fn() -> Result<(), String> + Send + Sync>,
}

type Attach<'a> = Box<dyn Fn(Option<SessionId>) -> LocalBoxFuture<'a, Harness> + 'a>;

fn create_request(fresh: bool) -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: MODEL_A.to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        // A recreated session keeps its persisted prompt.
        system_prompt: if fresh {
            crate::SystemPromptOverride::Set("auth clear default".to_string())
        } else {
            crate::SystemPromptOverride::Inherit
        },
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions {
            provider: fresh.then_some(Provider::OpenAI),
            realm_id: Some(RealmId::parse(TEST_REALM).expect("valid test realm")),
            auth_binding: fresh.then(configured_binding),
            override_image_generation: meerkat_core::ToolCategoryOverride::Disable,
            ..Default::default()
        }),
        labels: None,
    }
}

fn build_service(
    temp: &tempfile::TempDir,
    factory: AgentFactory,
    config: Config,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<MeerkatMachine>,
) {
    let builder = FactoryAgentBuilder::new(factory, config);
    let session_store: Arc<dyn SessionStore> =
        Arc::new(meerkat_store::JsonlStore::new(temp.path().join("jsonl")));
    let persistence = PersistenceBundle::new(
        session_store,
        Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(
                temp.path().join("runtime.sqlite3"),
            )
            .expect("open the runtime store"),
        ),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    )
    .expect("construct runtime authority");
    crate::surface::build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        4,
        persistence,
        temp.path().join("config_state.json"),
    )
    .expect("compose the runtime-backed service")
}

async fn materialize(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: &Arc<MeerkatMachine>,
    resume: Option<SessionId>,
) -> SessionId {
    let fresh = resume.is_none();
    let session = match resume {
        Some(session_id) => service
            .load_authoritative_session(&session_id)
            .await
            .expect("load the persisted session")
            .expect("the session stays durable"),
        None => Session::new(),
    };
    let session_id = session.id().clone();
    Box::pin(crate::surface::materialize_session(
        service,
        adapter,
        session,
        create_request(fresh),
        {
            let service = Arc::clone(service);
            let adapter = Arc::clone(adapter);
            move |session_id| {
                crate::surface::default_persistent_executor(service, adapter, session_id)
            }
        },
    ))
    .await
    .expect("materialize the runtime-backed session");
    session_id
}

impl Harness {
    async fn run_turn(&self, prompt: &str) {
        let (_, completion) = self
            .adapter
            .accept_input_with_completion(
                &self.session_id,
                Input::Prompt(PromptInput::new(prompt, None)),
            )
            .await
            .expect("the runtime accepts the prompt");
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            completion.expect("a completion handle").wait(),
        )
        .await
        .expect("the turn settles in time")
        .expect("the completion resolves");
        assert!(
            matches!(outcome, CompletionOutcome::Completed(ref run) if run.text == ANSWER),
            "{prompt}: {outcome:?}"
        );
        (self.lease_present)().unwrap_or_else(|error| panic!("{prompt}: {error}"));
    }

    async fn durable_identity(&self) -> SessionLlmIdentity {
        self.service
            .load_authoritative_session(&self.session_id)
            .await
            .expect("load the session")
            .expect("the session is durable")
            .session_metadata()
            .expect("canonical metadata")
            .llm_identity()
    }
}

/// Two turns on the explicit binding, a `Clear` swap to model B, two turns,
/// then a cold recreation and two more turns. The lease is present after
/// every turn, and the persisted identity names the resolved binding.
async fn clear_swap_keeps_the_resolved_account(attach: Attach<'_>) {
    let harness = attach(None).await;
    harness.run_turn("first turn on the explicit binding").await;
    harness
        .run_turn("second turn on the explicit binding")
        .await;
    assert_eq!(
        harness.durable_identity().await.auth_binding,
        Some(configured_binding())
    );

    // Clear the selector: the realm's default candidate is the same
    // configured binding and account.
    let report = harness
        .adapter
        .reconfigure_session_llm_identity(
            &harness.session_id,
            SessionLlmReconfigureRequest {
                model: Some(MODEL_B.to_string()),
                provider: None,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: Some(TurnMetadataOverride::Clear),
            },
        )
        .await
        .expect("the cleared binding resolves the realm default");
    assert_eq!(
        report.new_identity,
        harness.durable_identity().await,
        "the report returns the identity the swap applied and persisted"
    );
    assert_eq!(report.new_identity.auth_binding, Some(configured_binding()));
    (harness.lease_present)().expect("the swap must not release the account its new client uses");

    harness.run_turn("first turn after the clear").await;
    harness.run_turn("second turn after the clear").await;
    let identity = harness.durable_identity().await;
    assert_eq!(identity.model, MODEL_B);
    assert_eq!(
        identity.auth_binding,
        Some(configured_binding()),
        "the persisted identity names the binding the default resolution selected"
    );

    // Cold recreation from the persisted session keeps the same route.
    let session_id = harness.session_id.clone();
    harness
        .adapter
        .unregister_session(&session_id)
        .await
        .expect("tear down the runtime attachment");
    drop(harness);
    let restarted = attach(Some(session_id.clone())).await;
    assert_eq!(restarted.session_id, session_id);
    restarted.run_turn("first turn after cold recreation").await;
    restarted
        .run_turn("second turn after cold recreation")
        .await;
    let identity = restarted.durable_identity().await;
    assert_eq!(identity.model, MODEL_B);
    assert_eq!(identity.auth_binding, Some(configured_binding()));
}

#[tokio::test]
async fn clearing_the_binding_to_the_same_default_account_keeps_its_lease() {
    let temp = tempfile::tempdir().expect("tempdir");
    let temp = &temp;
    clear_swap_keeps_the_resolved_account(Box::new(move |resume| {
        Box::pin(async move {
            let lease = Arc::new(ResolvedLeaseView::default());
            let mut factory = AgentFactory::new(temp.path().join("factory-sessions"))
                .without_provider_auth_persistence()
                .builtins(false);
            factory.provider_registry = Arc::new(ProviderRuntimeRegistry::empty().with_runtime(
                Arc::new(LeaseRecordingRuntime {
                    lease: Arc::clone(&lease),
                }),
            ));
            let (service, adapter) = build_service(temp, factory, test_config());
            let session_id = materialize(&service, &adapter, resume).await;
            Harness {
                service,
                adapter,
                session_id,
                lease_present: Box::new(move || lease.lease_present()),
            }
        })
    }))
    .await;
}

/// The same scenario on the real OpenAI runtime: the real resolver constructs
/// the managed external-token client for a ChatGPT-backend binding, answered
/// by a loopback Responses server. The scenario checks native lease presence
/// after the swap and verifies the subsequent HTTP requests.
#[cfg(feature = "openai")]
mod chatgpt_backend {
    use super::*;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use meerkat_core::auth::{PersistedAuthMode, PersistedTokens, TokenKey, TokenStore as _};
    use meerkat_providers::auth_store::{
        EphemeralTokenStore, InMemoryCoordinator, ProviderAuthPersistence,
    };

    const ACCESS_TOKEN: &str = "external-chatgpt-access";
    const PROVIDER_ACCOUNT: &str = "chatgpt-provider-account";
    /// This scenario's credential account. A credential account names one
    /// process-wide lease and login lifecycle (realm and account), and the
    /// managed-store scenario above holds `TEST_ACCOUNT`'s while both run at
    /// once in one test process: each scenario holds an account of its own.
    const CHATGPT_ACCOUNT: &str = "chatgpt_account_a";

    /// One model request as the loopback server received it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct WireCall {
        authorization: String,
        account: String,
        model: String,
    }

    async fn responses(
        State(wire): State<Arc<StdMutex<Vec<WireCall>>>>,
        headers: HeaderMap,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> axum::response::Response {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        wire.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(WireCall {
                authorization: header("authorization"),
                account: header("chatgpt-account-id"),
                model: body["model"].as_str().unwrap_or_default().to_string(),
            });
        let event = serde_json::json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": ANSWER}],
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

    fn chatgpt_config(base_url: &str) -> Config {
        let mut config = test_config();
        let section = config
            .realm
            .get_mut(TEST_REALM)
            .expect("the test realm is configured");
        let backend = section
            .backend
            .get_mut(TEST_BINDING)
            .expect("the test backend is configured");
        backend.backend_kind = "chatgpt_backend".to_string();
        backend.base_url = Some(base_url.to_string());
        section
            .auth
            .get_mut(TEST_BINDING)
            .expect("the test auth profile is configured")
            .auth_method = "external_chatgpt_tokens".to_string();
        section
            .binding
            .get_mut(TEST_BINDING)
            .expect("the test binding is configured")
            .credential_account =
            Some(meerkat_core::CredentialAccountId::parse(CHATGPT_ACCOUNT).expect("valid account"));
        config
    }

    fn credential(config: &Config) -> AuthCredentialIdentity {
        config.realm[TEST_REALM].binding[TEST_BINDING].credential_identity(&configured_binding())
    }

    /// Store the external tokens under the account's key, published to the
    /// runtime's lifecycle the way a login does.
    async fn seed_tokens(
        store: &EphemeralTokenStore,
        owner: &GeneratedAuthLeaseHandle,
        credential: &AuthCredentialIdentity,
    ) {
        let key = TokenKey::from_credential_identity(credential);
        let lease = LeaseKey::from_credential_identity(credential);
        let issued = chrono::Utc::now();
        let expires = issued + chrono::Duration::hours(1);
        let tokens = PersistedTokens {
            auth_mode: PersistedAuthMode::ExternalTokens,
            primary_secret: Some(ACCESS_TOKEN.to_string()),
            refresh_token: None,
            id_token: None,
            expires_at: Some(expires),
            last_refresh: Some(issued),
            scopes: Vec::new(),
            account_id: Some(PROVIDER_ACCOUNT.to_string()),
            metadata: serde_json::Value::Null,
        };
        let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease).await;
        let transition = owner
            .acquire_lease(&lease, expires.timestamp().max(0) as u64)
            .expect("the runtime admits the login lifecycle");
        let tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &key,
            &tokens,
            &transition,
        )
        .expect("the tokens carry the lifecycle marker");
        store.save(&key, &tokens).await.expect("store the tokens");
        drop(guard);
    }

    #[tokio::test]
    async fn clearing_the_binding_keeps_the_chatgpt_account_lease_on_the_real_runtime() {
        let wire = Arc::new(StdMutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the loopback server");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("loopback address")
        );
        let app = axum::Router::new()
            .route("/responses", axum::routing::post(responses))
            .with_state(Arc::clone(&wire));
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let temp = tempfile::tempdir().expect("tempdir");
        let temp = &temp;
        let config = chatgpt_config(&base_url);
        let credential = credential(&config);
        let store = Arc::new(EphemeralTokenStore::new());
        let attach_config = config.clone();
        let attach_credential = credential.clone();
        let attach_store = Arc::clone(&store);
        clear_swap_keeps_the_resolved_account(Box::new(move |resume| {
            let config = attach_config.clone();
            let credential = attach_credential.clone();
            let store = Arc::clone(&attach_store);
            Box::pin(async move {
                let persistence = ProviderAuthPersistence::new(
                    Arc::clone(&store) as Arc<dyn meerkat_core::auth::TokenStore>,
                    Arc::new(InMemoryCoordinator::new()),
                );
                let factory = AgentFactory::new(temp.path().join("factory-sessions"))
                    .with_provider_auth_persistence(persistence)
                    .builtins(false);
                let (service, adapter) = build_service(temp, factory, config);
                let owner = adapter.generated_auth_lease_handle();
                if resume.is_none() {
                    seed_tokens(&store, &owner, &credential).await;
                }
                let session_id = materialize(&service, &adapter, resume).await;
                let lease = LeaseKey::from_credential_identity(&credential);
                Harness {
                    service,
                    adapter,
                    session_id,
                    lease_present: Box::new(move || {
                        let snapshot = owner.snapshot(&lease);
                        if snapshot.credential_present {
                            Ok(())
                        } else {
                            Err(format!("auth lease absent for {lease}: {snapshot:?}"))
                        }
                    }),
                }
            })
        }))
        .await;
        server.abort();

        let calls = wire
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let models: Vec<&str> = calls.iter().map(|call| call.model.as_str()).collect();
        assert_eq!(
            models,
            [MODEL_A, MODEL_A, MODEL_B, MODEL_B, MODEL_B, MODEL_B],
            "every turn reached the wire with its exact model"
        );
        for call in &calls {
            assert_eq!(call.authorization, format!("Bearer {ACCESS_TOKEN}"));
            assert_eq!(call.account, PROVIDER_ACCOUNT);
        }
    }
}
