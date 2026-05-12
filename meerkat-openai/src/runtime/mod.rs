//! OpenAI provider runtime.

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
pub mod oauth;

use std::sync::Arc;

use async_trait::async_trait;

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_core::AuthError;
use meerkat_core::{AuthLease, AuthMetadata, Provider};

#[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
use meerkat_auth_core::resolver::interactive_login_error;
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::resolver::{
    ManagedStoreLifecycle, begin_managed_store_oauth_refresh_lifecycle,
    load_managed_store_tokens_with_lifecycle, managed_store_oauth_refresh_failure_coordinator,
    managed_store_oauth_refresh_failure_is_permanent, mark_managed_store_oauth_refresh_failed,
    publish_managed_store_tokens_lifecycle_and_save, refresh_allowed,
};
use meerkat_auth_core::resolver::{
    finalize_auth_metadata, resolve_external_authorizer, resolve_simple_secret,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::{
    auth_store::PersistedAuthMode, oauth_flow::validate_oauth_target_for_auth_mode,
};
use meerkat_llm_core::provider_runtime::binding::{
    NormalizedAuthMethod, NormalizedBackendKind, ResolvedConnection, StaticLease, ValidatedBinding,
};
use meerkat_llm_core::provider_runtime::errors::{
    ProviderAuthError, ProviderBindingError, ProviderClientError,
};
use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;
use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;
use meerkat_llm_core::{ImageGenerationExecutor, LlmClient};

pub use meerkat_core::provider_matrix::openai::{OpenAiAuthMethod, OpenAiBackendKind};

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn openai_oauth_refresh_failure_is_permanent(error: &oauth::OpenAiOAuthError) -> bool {
    match error {
        oauth::OpenAiOAuthError::InteractiveLoginRequired
        | oauth::OpenAiOAuthError::MissingRefreshToken => true,
        oauth::OpenAiOAuthError::Refresh(meerkat_auth_core::RefreshError::Refresh(message)) => {
            managed_store_oauth_refresh_failure_is_permanent(message)
        }
        oauth::OpenAiOAuthError::OAuth(error) => {
            managed_store_oauth_refresh_failure_is_permanent(&error.to_string())
        }
        _ => false,
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn openai_oauth_refresh_error(
    error: oauth::OpenAiOAuthError,
    authmachine_failure: String,
) -> ProviderAuthError {
    let detail = if authmachine_failure.is_empty() {
        error.to_string()
    } else {
        format!("{error}{authmachine_failure}")
    };
    if authmachine_failure.is_empty() {
        match error {
            oauth::OpenAiOAuthError::InteractiveLoginRequired
            | oauth::OpenAiOAuthError::MissingRefreshToken => {
                return ProviderAuthError::Auth(AuthError::UserReauthRequired);
            }
            _ => {}
        }
    }
    ProviderAuthError::Auth(AuthError::RefreshFailed(detail))
}

fn chatgpt_backend_base_url(configured: Option<&str>) -> String {
    let Some(raw) = configured.filter(|url| !url.trim().is_empty()) else {
        return OpenAiBackendKind::ChatGptBackend.default_base_url().into();
    };
    let trimmed = raw.trim_end_matches('/');
    if trimmed == "https://chatgpt.com/backend-api" {
        OpenAiBackendKind::ChatGptBackend.default_base_url().into()
    } else {
        trimmed.to_string()
    }
}

fn chatgpt_backend_extra_headers(connection: &ResolvedConnection) -> Vec<(String, String)> {
    // Pull account identity + fedramp from the resolved lease's AuthMetadata
    // so the ChatGPT backend can emit the wire headers Codex's
    // bearer_auth_provider.rs:23-38 requires.
    let (account_id, is_fedramp) = match connection.auth_lease.metadata().provider_metadata {
        Some(meerkat_core::ProviderAuthMetadata::OpenAi(ref metadata)) => {
            (metadata.account_id.clone(), metadata.is_fedramp)
        }
        _ => (connection.auth_lease.metadata().account_id.clone(), None),
    };

    let mut headers = Vec::new();
    if let Some(account_id) = account_id {
        headers.push((
            meerkat_core::provider_matrix::openai_auth::CHATGPT_ACCOUNT_HEADER.to_string(),
            account_id,
        ));
    }
    if matches!(is_fedramp, Some(true)) {
        headers.push((
            meerkat_core::provider_matrix::openai_auth::FEDRAMP_HEADER.to_string(),
            "true".to_string(),
        ));
    }
    headers
}

pub struct OpenAiProviderRuntime;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ProviderRuntime for OpenAiProviderRuntime {
    fn provider_id(&self) -> Provider {
        Provider::OpenAI
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        if binding.provider() != Provider::OpenAI {
            return Err(ProviderAuthError::Binding(
                ProviderBindingError::ProviderMismatch,
            ));
        }
        let auth_method = match binding.auth() {
            NormalizedAuthMethod::OpenAi(m) => m,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };
        let backend_kind = match binding.backend() {
            NormalizedBackendKind::OpenAi(k) => k,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };

        let source_label = format!("openai:{}", binding.auth_profile().id);
        let lease: Arc<dyn AuthLease> = match auth_method {
            OpenAiAuthMethod::ApiKey | OpenAiAuthMethod::StaticBearer => {
                let secret =
                    resolve_simple_secret(&binding.auth_profile().source, env, binding).await?;
                let metadata = finalize_auth_metadata(binding, AuthMetadata::default())?;
                Arc::new(StaticLease::inline_secret(
                    secret,
                    metadata,
                    None,
                    source_label.clone(),
                ))
            }
            OpenAiAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile().source, env, binding).await?
            }
            OpenAiAuthMethod::ManagedChatGptOauth | OpenAiAuthMethod::ExternalChatGptTokens => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
                {
                    let expected_mode = match auth_method {
                        OpenAiAuthMethod::ManagedChatGptOauth => PersistedAuthMode::ChatgptOauth,
                        OpenAiAuthMethod::ExternalChatGptTokens => {
                            PersistedAuthMode::ExternalTokens
                        }
                        _ => unreachable!("OAuth branch only handles OAuth auth methods"),
                    };
                    validate_oauth_target_for_auth_mode(
                        binding.auth_profile(),
                        Provider::OpenAI,
                        expected_mode,
                    )
                    .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?;
                    let mut managed =
                        load_managed_store_tokens_with_lifecycle(env, binding).await?;
                    let lifecycle = managed.lifecycle;
                    let persisted = managed.tokens.clone();

                    let effective_tokens = match auth_method {
                        OpenAiAuthMethod::ExternalChatGptTokens => {
                            if lifecycle == ManagedStoreLifecycle::RefreshRequired {
                                return Err(ProviderAuthError::Auth(AuthError::RefreshRequired));
                            }
                            persisted
                        }
                        OpenAiAuthMethod::ManagedChatGptOauth => {
                            if lifecycle == ManagedStoreLifecycle::Authorized
                                && persisted.primary_secret.is_some()
                                && !env.force_refresh
                            {
                                persisted
                            } else {
                                if !refresh_allowed(binding) {
                                    return Err(ProviderAuthError::Auth(
                                        AuthError::RefreshRequired,
                                    ));
                                }
                                let refresh_started = begin_managed_store_oauth_refresh_lifecycle(
                                    env,
                                    binding,
                                    &mut managed,
                                )?;
                                let coord = env.refresh_coord.clone().unwrap_or_else(|| {
                                    Arc::new(meerkat_auth_core::InMemoryCoordinator::new())
                                });
                                let coord = managed_store_oauth_refresh_failure_coordinator(
                                    coord,
                                    env.clone(),
                                    binding.clone(),
                                    refresh_started,
                                );
                                let endpoints =
                                    oauth::chatgpt_endpoints("http://127.0.0.1:0/callback");
                                let runtime = oauth::OpenAiOAuthRuntime::new(
                                    managed.store.clone(),
                                    coord,
                                    endpoints,
                                    managed.key.clone(),
                                );
                                let commit_env = env.clone();
                                let commit_binding = binding.clone();
                                let commit: oauth::TokenCommitFn = Box::new(move |tokens| {
                                    Box::pin(async move {
                                        publish_managed_store_tokens_lifecycle_and_save(
                                            &commit_env,
                                            &commit_binding,
                                            &managed,
                                            &tokens,
                                        )
                                        .await
                                        .map_err(|e| {
                                            meerkat_auth_core::RefreshError::Refresh(e.to_string())
                                        })
                                    })
                                });
                                let refreshed = runtime
                                    .refresh_tokens_with_commit(commit, env.force_refresh)
                                    .await;
                                refreshed.map_err(|e| {
                                    let permanent = openai_oauth_refresh_failure_is_permanent(&e);
                                    let failure = mark_managed_store_oauth_refresh_failed(
                                        env,
                                        binding,
                                        refresh_started,
                                        permanent,
                                    )
                                    .err()
                                    .map(|err| format!("; {err}"))
                                    .unwrap_or_default();
                                    openai_oauth_refresh_error(e, failure)
                                })?
                            }
                        }
                        _ => unreachable!("arm guarded by outer match"),
                    };

                    let access = effective_tokens
                        .primary_secret
                        .clone()
                        .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
                    let mut chatgpt_account_id = effective_tokens.account_id.clone();
                    let mut chatgpt_user_id: Option<String> = None;
                    let mut chatgpt_email: Option<String> = None;
                    let mut chatgpt_is_fedramp: Option<bool> = None;
                    let mut chatgpt_plan_type: Option<String> = None;
                    if let Some(id_token) = effective_tokens.id_token.as_deref()
                        && let Ok(claims) =
                            meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
                    {
                        let lifted = oauth::ChatGptIdClaims::lift_from_claims(&claims.raw);
                        if chatgpt_account_id.is_none() {
                            chatgpt_account_id = lifted.account_id;
                        }
                        chatgpt_user_id = lifted.user_id;
                        chatgpt_email = lifted.email;
                        chatgpt_is_fedramp = lifted.is_fedramp;
                        chatgpt_plan_type = lifted.plan_type;
                    }
                    let mut metadata = AuthMetadata::default();
                    if chatgpt_account_id.is_some()
                        || chatgpt_user_id.is_some()
                        || chatgpt_email.is_some()
                        || chatgpt_is_fedramp.is_some()
                        || chatgpt_plan_type.is_some()
                    {
                        metadata.account_id = chatgpt_account_id.clone();
                        metadata.plan = chatgpt_plan_type.clone();
                        metadata.provider_metadata =
                            Some(meerkat_core::ProviderAuthMetadata::OpenAi(
                                meerkat_core::OpenAiAuthMetadata {
                                    plan_type: chatgpt_plan_type,
                                    user_id: chatgpt_user_id,
                                    account_id: chatgpt_account_id,
                                    is_fedramp: chatgpt_is_fedramp,
                                    email: chatgpt_email,
                                },
                            ));
                    }
                    let metadata = finalize_auth_metadata(binding, metadata)?;
                    Arc::new(StaticLease::inline_secret(
                        access,
                        metadata,
                        effective_tokens.expires_at,
                        source_label.clone(),
                    ))
                }
                #[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
                {
                    return Err(interactive_login_error(binding));
                }
            }
        };

        Ok(ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(backend_kind),
            backend_profile: binding.backend_profile().clone(),
            auth_lease: lease,
        })
    }

    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        // ProviderRuntimeRegistry dispatches on Provider enum; non-OpenAI
        // arms are unreachable at runtime.
        let backend_kind = match connection.backend {
            NormalizedBackendKind::OpenAi(k) => k,
            other => unreachable!(
                "OpenAiProviderRuntime received non-OpenAi backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        // Authorizer-backed path (ExternalAuthorizer→DynamicAuthorizer
        // envelope). Wire through OpenAiClient.with_authorizer so the
        // request uses HttpAuthorizer::authorize for headers rather
        // than Authorization: Bearer <api_key>. Plan §6.11: read the
        // authorizer from the auth lease directly instead of the
        // (deleted side channel).
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = connection.resolved_authorizer() {
            let base_url = match backend_kind {
                OpenAiBackendKind::ChatGptBackend => {
                    chatgpt_backend_base_url(connection.backend_profile.base_url.as_deref())
                }
                OpenAiBackendKind::OpenAiApi => connection
                    .backend_profile
                    .base_url
                    .clone()
                    .unwrap_or_else(|| backend_kind.default_base_url().into()),
            };
            let mut client =
                crate::OpenAiClient::new_with_optional_api_key_and_base_url(None, base_url)
                    .with_authorizer(authorizer);
            if matches!(backend_kind, OpenAiBackendKind::ChatGptBackend) {
                client = client.with_chatgpt_backend_wire();
            }
            return Ok(Arc::new(client));
        }
        // No inline secret and no DynamicAuthorizer kind: the
        // lease was constructed empty. Surface as missing credential
        // material (or MissingFeature on wasm32 where authorizers
        // don't compile) so surfaces get a typed error.
        #[cfg(target_arch = "wasm32")]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::MissingFeature(
                "openai-authorizer-backed auth not available on wasm32",
            ))?;
        #[cfg(not(target_arch = "wasm32"))]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        match backend_kind {
            OpenAiBackendKind::OpenAiApi => {
                // S1-verified: OpenAiClient::new returns Self (infallible).
                let client = match &connection.backend_profile.base_url {
                    Some(url) => crate::OpenAiClient::new_with_base_url(secret, url.clone()),
                    None => crate::OpenAiClient::new(secret),
                };
                Ok(Arc::new(client))
            }
            OpenAiBackendKind::ChatGptBackend => {
                // ChatGPT backend: emit account_id + fedramp wire
                // headers per Codex bearer_auth_provider.rs:23-38.
                let base_url =
                    chatgpt_backend_base_url(connection.backend_profile.base_url.as_deref());
                let client = crate::OpenAiClient::new_with_base_url(secret, base_url)
                    .with_extra_headers(chatgpt_backend_extra_headers(&connection))
                    .with_chatgpt_backend_wire();
                Ok(Arc::new(client))
            }
        }
    }

    fn build_image_generation_executor(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Option<Arc<dyn ImageGenerationExecutor>>, ProviderClientError> {
        let backend_kind = match connection.backend {
            NormalizedBackendKind::OpenAi(k) => k,
            other => unreachable!(
                "OpenAiProviderRuntime received non-OpenAi backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = connection.resolved_authorizer() {
            let base_url = match backend_kind {
                OpenAiBackendKind::ChatGptBackend => {
                    chatgpt_backend_base_url(connection.backend_profile.base_url.as_deref())
                }
                OpenAiBackendKind::OpenAiApi => connection
                    .backend_profile
                    .base_url
                    .clone()
                    .unwrap_or_else(|| backend_kind.default_base_url().into()),
            };
            let mut client =
                crate::OpenAiClient::new_with_optional_api_key_and_base_url(None, base_url)
                    .with_authorizer(authorizer);
            if matches!(backend_kind, OpenAiBackendKind::ChatGptBackend) {
                client = client.with_chatgpt_backend_wire();
            }
            return Ok(Some(Arc::new(client)));
        }
        #[cfg(target_arch = "wasm32")]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::MissingFeature(
                "openai-authorizer-backed auth not available on wasm32",
            ))?;
        #[cfg(not(target_arch = "wasm32"))]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        let client = match backend_kind {
            OpenAiBackendKind::OpenAiApi => match &connection.backend_profile.base_url {
                Some(url) => crate::OpenAiClient::new_with_base_url(secret, url.clone()),
                None => crate::OpenAiClient::new(secret),
            },
            OpenAiBackendKind::ChatGptBackend => {
                let base_url =
                    chatgpt_backend_base_url(connection.backend_profile.base_url.as_deref());
                crate::OpenAiClient::new_with_base_url(secret, base_url)
                    .with_extra_headers(chatgpt_backend_extra_headers(&connection))
                    .with_chatgpt_backend_wire()
            }
        };
        Ok(Some(Arc::new(client)))
    }

    fn image_generation_profile(
        &self,
    ) -> Option<Arc<dyn meerkat_core::ImageGenerationProviderProfile>> {
        Some(Arc::new(crate::OpenAiImageGenerationProfile))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::{
        Json, Router, extract::State, http::HeaderMap, response::IntoResponse, routing::post,
    };
    use meerkat_core::{
        AuthMetadata, AuthProfile, BackendProfile, BindingPolicy, ImageOperationTerminalClass,
        OpenAiAuthMetadata, ProviderAuthMetadata,
    };
    use meerkat_llm_core::{
        ProviderImageGenerationRequest, provider_runtime::ProviderRuntimeCatalog,
    };
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    #[test]
    fn typed_catalog_contains_expected_combinations() {
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi),
            NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::ApiKey),
        ));
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend),
            NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::ManagedChatGptOauth),
        ));
    }

    #[test]
    fn provider_id_is_openai() {
        assert_eq!(OpenAiProviderRuntime.provider_id(), Provider::OpenAI);
    }

    fn backend(kind: &str) -> BackendProfile {
        BackendProfile {
            id: "b".into(),
            provider: Provider::OpenAI,
            backend_kind: kind.into(),
            base_url: None,
            options: serde_json::Value::Null,
        }
    }

    fn auth(method: &str) -> AuthProfile {
        AuthProfile {
            id: "a".into(),
            provider: Provider::OpenAI,
            auth_method: method.into(),
            source: meerkat_core::CredentialSourceSpec::InlineSecret {
                secret: "sk-x".into(),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        }
    }

    fn auth_binding() -> meerkat_core::AuthBindingRef {
        meerkat_core::AuthBindingRef {
            realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
            profile: None,
        }
    }

    fn resolved_chatgpt_connection(
        metadata: AuthMetadata,
        base_url: Option<String>,
    ) -> ResolvedConnection {
        let mut backend = backend("chatgpt_backend");
        backend.base_url = base_url;
        ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend),
            backend_profile: Arc::new(backend),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "oauth-access-token".into(),
                metadata,
                None,
                "openai:test",
            )),
        }
    }

    #[derive(Clone)]
    struct ImageHeaderState {
        seen: Arc<Mutex<Vec<HeaderMap>>>,
    }

    async fn image_header_stub(
        State(state): State<ImageHeaderState>,
        headers: HeaderMap,
        Json(_body): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        state.seen.lock().expect("seen headers").push(headers);
        let payload = [
            r#"data: {"type":"response.output_item.done","item":{"id":"ig_test","type":"image_generation_call","status":"completed","result":"data:image/png;base64,aGVsbG8="}}"#,
            r#"data: {"type":"response.completed","response":{"id":"resp_test","output":[]}}"#,
            "data: [DONE]",
            "",
        ]
        .join("\n");
        ([("content-type", "text/event-stream")], payload)
    }

    async fn spawn_image_header_stub() -> (
        String,
        Arc<Mutex<Vec<HeaderMap>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/responses", post(image_header_stub))
            .with_state(ImageHeaderState {
                seen: Arc::clone(&seen),
            });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test server");
        });
        (format!("http://{addr}"), seen, handle)
    }

    fn hosted_image_request() -> ProviderImageGenerationRequest {
        serde_json::from_value(serde_json::json!({
            "operation_id": "00000000-0000-0000-0000-000000000101",
            "model": "gpt-5.4",
            "generate_request": {
                "intent": {
                    "intent": "generate",
                    "prompt": {"content": "draw a small red square"},
                    "prompt_source": {
                        "source": "user_provided",
                        "message_id": "00000000-0000-0000-0000-000000000102"
                    },
                    "reference_images": []
                },
                "target": {"target": "auto"},
                "size": {"size": "square1024"},
                "quality": "low",
                "format": "png",
                "count": 1
            },
            "execution_plan": {
                "provider": "openai",
                "backend": "hosted_tool",
                "max_count": 1,
                "capabilities": {
                    "hosted_image_generation_tool": true,
                    "native_image_output": false,
                    "custom_tools": true,
                    "image_search_grounding": false,
                    "image_continuity_tokens": "unsupported"
                },
                "requires_scoped_override": false,
                "provider_plan": {
                    "tool_name": "image_generation",
                    "model": "gpt-image-2",
                    "output": {
                        "size": "square1024",
                        "quality": "low",
                        "output_format": "png"
                    }
                }
            },
            "projected_messages": []
        }))
        .expect("hosted image request")
    }

    #[test]
    fn typed_catalog_validate_accepts_allowed_combination() {
        let vb = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend("openai_api"),
            &auth("api_key"),
            &BindingPolicy::default(),
        )
        .expect("allowed combination");
        assert_eq!(vb.provider(), Provider::OpenAI);
    }

    #[test]
    fn typed_catalog_validate_rejects_unknown_backend_kind() {
        let err = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend("bogus_backend"),
            &auth("api_key"),
            &BindingPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ProviderBindingError::UnknownBackendKind(_)));
    }

    #[test]
    fn typed_catalog_validate_rejects_unsupported_combo() {
        // openai_api + managed_chatgpt_oauth is not a typed catalog edge.
        let err = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend("openai_api"),
            &auth("managed_chatgpt_oauth"),
            &BindingPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProviderBindingError::UnsupportedCombination { .. }
        ));
    }

    #[test]
    fn typed_catalog_validate_rejects_provider_mismatch() {
        let mut wrong = backend("openai_api");
        wrong.provider = Provider::Anthropic;
        let err = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &wrong,
            &auth("api_key"),
            &BindingPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ProviderBindingError::ProviderMismatch));
    }

    #[test]
    fn typed_catalog_validate_propagates_binding_policy() {
        // Dogma §16: policy declared on the binding must flow through
        // catalog validation, not default-injected at the provider seam.
        let policy = BindingPolicy {
            allow_auth_override: true,
            require_metadata_account: true,
            require_metadata_workspace: false,
        };
        let vb = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend("openai_api"),
            &auth("api_key"),
            &policy,
        )
        .expect("allowed combination");
        assert_eq!(vb.policy(), &policy);
    }

    #[test]
    fn chatgpt_backend_headers_use_openai_oauth_metadata() {
        let connection = resolved_chatgpt_connection(
            AuthMetadata {
                provider_metadata: Some(ProviderAuthMetadata::OpenAi(OpenAiAuthMetadata {
                    account_id: Some("acct_123".into()),
                    is_fedramp: Some(true),
                    ..Default::default()
                })),
                ..Default::default()
            },
            None,
        );

        let headers = chatgpt_backend_extra_headers(&connection);

        assert!(headers.contains(&(
            meerkat_core::provider_matrix::openai_auth::CHATGPT_ACCOUNT_HEADER.to_string(),
            "acct_123".to_string(),
        )));
        assert!(headers.contains(&(
            meerkat_core::provider_matrix::openai_auth::FEDRAMP_HEADER.to_string(),
            "true".to_string(),
        )));
    }

    #[test]
    fn chatgpt_backend_headers_fall_back_to_generic_account_metadata() {
        let connection = resolved_chatgpt_connection(
            AuthMetadata {
                account_id: Some("acct_generic".into()),
                ..Default::default()
            },
            None,
        );

        let headers = chatgpt_backend_extra_headers(&connection);

        assert_eq!(
            headers,
            vec![(
                meerkat_core::provider_matrix::openai_auth::CHATGPT_ACCOUNT_HEADER.to_string(),
                "acct_generic".to_string(),
            )]
        );
    }

    #[tokio::test]
    async fn chatgpt_backend_image_executor_sends_oauth_account_headers()
    -> Result<(), Box<dyn std::error::Error>> {
        let (base_url, seen, handle) = spawn_image_header_stub().await;
        let connection = resolved_chatgpt_connection(
            AuthMetadata {
                provider_metadata: Some(ProviderAuthMetadata::OpenAi(OpenAiAuthMetadata {
                    account_id: Some("acct_image".into()),
                    is_fedramp: Some(true),
                    ..Default::default()
                })),
                ..Default::default()
            },
            Some(base_url),
        );

        let executor = OpenAiProviderRuntime
            .build_image_generation_executor(connection)?
            .expect("chatgpt backend should provide image executor");
        let output = executor
            .execute_image_generation(hosted_image_request())
            .await?;

        assert!(matches!(
            output.terminal,
            ImageOperationTerminalClass::Generated
        ));
        let headers = seen.lock().expect("seen headers");
        let first = headers.first().expect("captured image request headers");
        assert_eq!(
            first
                .get(meerkat_core::provider_matrix::openai_auth::CHATGPT_ACCOUNT_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("acct_image")
        );
        assert_eq!(
            first
                .get(meerkat_core::provider_matrix::openai_auth::FEDRAMP_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );

        handle.abort();
        Ok(())
    }
}
