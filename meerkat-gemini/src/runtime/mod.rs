//! Google provider runtime (Gemini API, Vertex AI, Code Assist).

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
pub mod oauth;

use std::sync::Arc;

use async_trait::async_trait;

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_core::AuthError;
#[cfg(all(not(target_arch = "wasm32"), feature = "adc"))]
use meerkat_core::HttpAuthorizer;
use meerkat_core::{AuthLease, AuthMetadata, Provider};
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use serde::Deserialize;

#[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
use meerkat_auth_core::resolver::interactive_login_error;
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::resolver::{
    OAuthLoginCredentialAdmission, load_managed_store_tokens_with_lifecycle,
    prepare_managed_store_oauth_refresh_under_lock, resolve_oauth_login_credential_disposition,
};
use meerkat_auth_core::resolver::{
    finalize_auth_metadata, resolve_external_authorizer, resolve_simple_secret,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::{
    auth_store::PersistedAuthMode, oauth_flow::validate_oauth_target_for_auth_mode,
};
#[cfg(all(not(target_arch = "wasm32"), any(feature = "adc", feature = "copilot")))]
use meerkat_llm_core::provider_runtime::binding::DynamicLease;
use meerkat_llm_core::provider_runtime::binding::{
    NormalizedAuthMethod, NormalizedBackendKind, ResolvedConnection, ResolvedTextTarget,
    StaticLease, ValidatedBinding,
};
use meerkat_llm_core::provider_runtime::errors::{
    ProviderAuthError, ProviderBindingError, ProviderClientError,
};
use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;
use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;
use meerkat_llm_core::{ImageGenerationExecutor, LlmClient};

pub use meerkat_core::provider_matrix::google::{GoogleAuthMethod, GoogleBackendKind};

fn configured_or_default_base_url(
    backend_kind: GoogleBackendKind,
    backend_profile: &meerkat_core::BackendProfile,
) -> Option<String> {
    backend_profile
        .base_url
        .clone()
        .filter(|u| !u.is_empty())
        .or_else(|| {
            let default = backend_kind.default_base_url();
            (!default.is_empty()).then(|| default.to_string())
        })
}

#[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
struct GeminiCopilotChatClient {
    inner: Arc<dyn LlmClient>,
}

#[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
impl GeminiCopilotChatClient {
    fn unsupported(field: &str) -> meerkat_llm_core::LlmError {
        meerkat_llm_core::LlmError::InvalidRequest {
            message: format!(
                "Gemini Copilot Chat Completions does not support provider parameter '{field}'"
            ),
        }
    }

    fn lower_request(
        &self,
        request: &meerkat_llm_core::LlmRequest,
    ) -> Result<meerkat_llm_core::LlmRequest, meerkat_llm_core::LlmError> {
        use meerkat_core::lifecycle::run_primitive::{
            GeminiThinkingLevel, OpenAiProviderTag, ProviderTag, ReasoningEffort,
        };

        let Some(provider_params) = request.provider_params.as_ref() else {
            return Ok(request.clone());
        };
        let ProviderTag::Gemini(tag) = provider_params else {
            return Err(Self::unsupported("non_gemini_provider_tag"));
        };
        let unsupported = [
            (tag.thinking_budget.is_some(), "thinking_budget"),
            (tag.top_k.is_some(), "top_k"),
            (tag.top_p.is_some(), "top_p"),
            (tag.google_search.is_some(), "google_search"),
            (tag.candidate_count.is_some(), "candidate_count"),
            (tag.cached_content_name.is_some(), "cached_content_name"),
        ]
        .into_iter()
        .find_map(|(present, field)| present.then_some(field));
        if let Some(field) = unsupported {
            return Err(Self::unsupported(field));
        }
        let nested_level = if let Some(thinking) = tag.thinking.as_ref() {
            if thinking.include_thoughts.is_some() {
                return Err(Self::unsupported("thinking.include_thoughts"));
            }
            if thinking.thinking_budget.is_some() {
                return Err(Self::unsupported("thinking.thinking_budget"));
            }
            thinking.thinking_level
        } else {
            None
        };
        if let (Some(nested), Some(flat)) = (nested_level, tag.thinking_level)
            && nested != flat
        {
            return Err(meerkat_llm_core::LlmError::InvalidRequest {
                message: "Gemini Copilot thinking levels disagree between typed and flat fields"
                    .to_string(),
            });
        }
        let reasoning_effort = match nested_level.or(tag.thinking_level) {
            Some(GeminiThinkingLevel::Minimal) => {
                return Err(Self::unsupported("thinking_level=minimal"));
            }
            Some(GeminiThinkingLevel::Low) => Some(ReasoningEffort::Low),
            Some(GeminiThinkingLevel::Medium) => Some(ReasoningEffort::Medium),
            Some(GeminiThinkingLevel::High) => Some(ReasoningEffort::High),
            None => None,
        };
        let mut lowered = request.clone();
        lowered.provider_params = Some(ProviderTag::OpenAi(OpenAiProviderTag {
            reasoning_effort,
            structured_output: tag.structured_output.clone(),
            supports_reasoning_override: reasoning_effort.map(|_| true),
            ..Default::default()
        }));
        Ok(lowered)
    }
}

#[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
#[async_trait]
impl LlmClient for GeminiCopilotChatClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_llm_core::LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn request_pressure(
        &self,
        request: &meerkat_llm_core::LlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, meerkat_llm_core::LlmError> {
        self.inner.request_pressure(&self.lower_request(request)?)
    }

    fn authored_cache_breakpoints(
        &self,
        request: &meerkat_llm_core::LlmRequest,
        canonical_messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, meerkat_llm_core::LlmError> {
        self.inner
            .authored_cache_breakpoints(&self.lower_request(request)?, canonical_messages)
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_llm_core::LlmRequest,
    ) -> meerkat_llm_core::LlmStream<'a> {
        Box::pin(async_stream::try_stream! {
            let lowered = self.lower_request(request)?;
            let mut stream = self.inner.stream(&lowered);
            while let Some(event) = futures::StreamExt::next(&mut stream).await {
                yield event?;
            }
        })
    }

    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
        self.inner.health_check().await
    }

    fn compile_schema(
        &self,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<meerkat_core::CompiledSchema, meerkat_core::SchemaError> {
        self.inner.compile_schema(output_schema)
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeAssistTier {
    id: Option<String>,
    is_default: Option<bool>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadCodeAssistResponse {
    current_tier: Option<CodeAssistTier>,
    allowed_tiers: Option<Vec<CodeAssistTier>>,
    ineligible_tiers: Option<Vec<serde_json::Value>>,
    cloudaicompanion_project: Option<String>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeAssistProjectRef {
    id: Option<String>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeAssistOnboardResponse {
    cloudaicompanion_project: Option<CodeAssistProjectRef>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeAssistOperationResponse {
    name: Option<String>,
    done: Option<bool>,
    response: Option<CodeAssistOnboardResponse>,
}

fn code_assist_project_id_from_metadata(metadata: &AuthMetadata) -> Option<String> {
    match &metadata.provider_metadata {
        Some(meerkat_core::ProviderAuthMetadata::Google(google)) => google.project_id.clone(),
        _ => None,
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn code_assist_method_url(base_url: &str, method: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    if base_url.ends_with("/v1internal") {
        format!("{base_url}:{method}")
    } else {
        format!("{base_url}/v1internal:{method}")
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn code_assist_operation_url(base_url: &str, name: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    if base_url.ends_with("/v1internal") {
        format!("{base_url}/{name}")
    } else {
        format!("{base_url}/v1internal/{name}")
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn code_assist_metadata(project_id: Option<&str>) -> serde_json::Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "ideType".to_string(),
        serde_json::Value::String("IDE_UNSPECIFIED".into()),
    );
    metadata.insert(
        "platform".to_string(),
        serde_json::Value::String("PLATFORM_UNSPECIFIED".into()),
    );
    metadata.insert(
        "pluginType".to_string(),
        serde_json::Value::String("GEMINI".into()),
    );
    if let Some(project_id) = project_id {
        metadata.insert(
            "duetProject".to_string(),
            serde_json::Value::String(project_id.to_string()),
        );
    }
    serde_json::Value::Object(metadata)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
async fn post_code_assist_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    access_token: &str,
    url: String,
    body: serde_json::Value,
) -> Result<T, ProviderAuthError> {
    let response = http
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {access_token}"))
        .json(&body)
        .send()
        .await
        .map_err(|err| ProviderAuthError::SourceResolutionFailed(err.to_string()))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "Google Code Assist setup request failed: status={} body={}",
            status.as_u16(),
            text,
        )));
    }
    serde_json::from_str(&text).map_err(|err| {
        ProviderAuthError::SourceResolutionFailed(format!(
            "Google Code Assist setup response was not valid JSON: {err}"
        ))
    })
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
async fn get_code_assist_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    access_token: &str,
    url: String,
) -> Result<T, ProviderAuthError> {
    let response = http
        .get(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|err| ProviderAuthError::SourceResolutionFailed(err.to_string()))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "Google Code Assist operation request failed: status={} body={}",
            status.as_u16(),
            text,
        )));
    }
    serde_json::from_str(&text).map_err(|err| {
        ProviderAuthError::SourceResolutionFailed(format!(
            "Google Code Assist operation response was not valid JSON: {err}"
        ))
    })
}

/// Typed result of resolving the Google Code Assist managed project and tier.
///
/// This is the single typed owner of the loadCodeAssist/onboardUser policy
/// decision: the runtime materializes both the resolved `project_id` and the
/// `tier` it actually onboarded into [`meerkat_core::GoogleAuthMetadata`] from
/// this one owner. The tier is no longer selected-then-discarded and re-derived
/// from a separate stringly binding option. `tier` is `None` when the project
/// came from an explicit hint and onboarding never ran (no tier was selected).
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
struct CodeAssistProjectResolution {
    project_id: String,
    tier: Option<String>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
async fn resolve_code_assist_user_project(
    access_token: &str,
    binding: &ValidatedBinding,
    env: &ResolverEnvironment,
    base_url: &str,
) -> Result<CodeAssistProjectResolution, ProviderAuthError> {
    let project_hint = backend_option_string(binding, "project_id")
        .or_else(|| (env.env_lookup)("GOOGLE_CLOUD_PROJECT"))
        .or_else(|| (env.env_lookup)("GCLOUD_PROJECT"))
        .or_else(|| (env.env_lookup)("CLOUDSDK_CORE_PROJECT"));
    if let Some(project_id) = project_hint.clone() {
        // Explicit project hint: no onboarding runs, so no tier is selected.
        return Ok(CodeAssistProjectResolution {
            project_id,
            tier: None,
        });
    }
    let http = reqwest::Client::new();
    let mut load_body = serde_json::Map::new();
    if let Some(project_id) = project_hint.as_deref() {
        load_body.insert(
            "cloudaicompanionProject".to_string(),
            serde_json::Value::String(project_id.to_string()),
        );
    }
    load_body.insert(
        "metadata".to_string(),
        code_assist_metadata(project_hint.as_deref()),
    );
    let load: LoadCodeAssistResponse = post_code_assist_json(
        &http,
        access_token,
        code_assist_method_url(base_url, "loadCodeAssist"),
        serde_json::Value::Object(load_body),
    )
    .await?;
    if let Some(project_id) = load
        .cloudaicompanion_project
        .clone()
        .or(project_hint.clone())
    {
        // loadCodeAssist returned a managed project: it also reports the
        // account's current tier; surface it as the resolved tier of record.
        let tier = load.current_tier.as_ref().and_then(|t| t.id.clone());
        return Ok(CodeAssistProjectResolution { project_id, tier });
    }
    if load.current_tier.is_some() {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "Google Code Assist did not return a managed project; set backend option project_id or GOOGLE_CLOUD_PROJECT".into(),
        ));
    }
    let tier = load
        .allowed_tiers
        .as_deref()
        .and_then(|tiers| {
            tiers
                .iter()
                .find(|tier| tier.is_default.unwrap_or(false))
                .or_else(|| tiers.first())
        })
        .ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "Google Code Assist account is not eligible for onboarding: {:?}",
                load.ineligible_tiers
            ))
        })?;
    let tier_id = tier.id.as_deref().unwrap_or("standard-tier");
    let mut onboard_body = serde_json::Map::new();
    onboard_body.insert(
        "tierId".to_string(),
        serde_json::Value::String(tier_id.to_string()),
    );
    if tier_id != "free-tier"
        && let Some(project_id) = project_hint.as_deref()
    {
        onboard_body.insert(
            "cloudaicompanionProject".to_string(),
            serde_json::Value::String(project_id.to_string()),
        );
    }
    onboard_body.insert(
        "metadata".to_string(),
        code_assist_metadata(if tier_id == "free-tier" {
            None
        } else {
            project_hint.as_deref()
        }),
    );
    let mut operation: CodeAssistOperationResponse = post_code_assist_json(
        &http,
        access_token,
        code_assist_method_url(base_url, "onboardUser"),
        serde_json::Value::Object(onboard_body),
    )
    .await?;
    for _ in 0..12 {
        if operation.done.unwrap_or(false) {
            break;
        }
        let Some(name) = operation.name.clone() else {
            break;
        };
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        operation = get_code_assist_json(
            &http,
            access_token,
            code_assist_operation_url(base_url, &name),
        )
        .await?;
    }
    let project_id = operation
        .response
        .and_then(|response| response.cloudaicompanion_project)
        .and_then(|project| project.id)
        .or(project_hint)
        .ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(
                "Google Code Assist onboarding did not return a managed project".into(),
            )
        })?;
    // The tier we just onboarded is the resolved tier of record — return it
    // alongside the project rather than discarding it.
    Ok(CodeAssistProjectResolution {
        project_id,
        tier: Some(tier_id.to_string()),
    })
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn google_code_assist_oauth_refresh_error(
    error: oauth::GoogleCodeAssistOAuthError,
    authmachine_failure: String,
) -> ProviderAuthError {
    let detail = if authmachine_failure.is_empty() {
        error.to_string()
    } else {
        format!("{error}{authmachine_failure}")
    };
    if authmachine_failure.is_empty() {
        match error {
            oauth::GoogleCodeAssistOAuthError::InteractiveLoginRequired
            | oauth::GoogleCodeAssistOAuthError::MissingRefreshToken => {
                return ProviderAuthError::Auth(AuthError::UserReauthRequired);
            }
            _ => {}
        }
    }
    ProviderAuthError::Auth(AuthError::RefreshFailed(detail))
}

#[derive(Default)]
pub struct GoogleProviderRuntime {
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    copilot: Option<Arc<meerkat_copilot::CopilotRuntime>>,
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    copilot_chat_completions: Option<Arc<dyn meerkat_copilot::CopilotChatCompletionsClientFactory>>,
}

#[allow(non_upper_case_globals)]
pub const GoogleProviderRuntime: GoogleProviderRuntime = GoogleProviderRuntime {
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    copilot: None,
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    copilot_chat_completions: None,
};

impl GoogleProviderRuntime {
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    pub fn with_copilot(
        copilot: Arc<meerkat_copilot::CopilotRuntime>,
        chat_completions: Arc<dyn meerkat_copilot::CopilotChatCompletionsClientFactory>,
    ) -> Self {
        Self {
            copilot: Some(copilot),
            copilot_chat_completions: Some(chat_completions),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ProviderRuntime for GoogleProviderRuntime {
    fn provider_id(&self) -> Provider {
        Provider::Gemini
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        if binding.provider() != Provider::Gemini {
            return Err(ProviderAuthError::Binding(
                ProviderBindingError::ProviderMismatch,
            ));
        }
        let auth_method = match binding.auth() {
            NormalizedAuthMethod::Google(m) => m,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };
        let backend_kind = match binding.backend() {
            NormalizedBackendKind::Google(k) => k,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };

        let source_label = format!("google:{}", binding.auth_profile().id);
        let lease: Arc<dyn AuthLease> = match auth_method {
            GoogleAuthMethod::ApiKey
            | GoogleAuthMethod::BearerApiKey
            | GoogleAuthMethod::ApiKeyExpress => {
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
            GoogleAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile().source, env, binding).await?
            }
            GoogleAuthMethod::GitHubCopilotOauth => {
                #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
                {
                    let runtime = self.copilot.as_ref().ok_or_else(|| {
                        ProviderAuthError::SourceResolutionFailed(
                            "Gemini Copilot backend is not composed with CopilotRuntime"
                                .to_string(),
                        )
                    })?;
                    let resolved = runtime.resolve(binding, env).await?;
                    Arc::new(DynamicLease::from_authorizer(
                        resolved.authorizer(),
                        resolved.metadata().clone(),
                        meerkat_copilot::GITHUB_COPILOT_AUTHORIZER_LABEL,
                    ))
                }
                #[cfg(not(all(feature = "copilot", not(target_arch = "wasm32"))))]
                {
                    return Err(ProviderAuthError::SourceResolutionFailed(
                        "Gemini Copilot backend is not compiled".to_string(),
                    ));
                }
            }
            GoogleAuthMethod::Adc => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "adc"))]
                {
                    let mut authorizer =
                        meerkat_auth_core::authorizers::GoogleAuthAuthorizer::with_env_lookup(
                            meerkat_auth_core::authorizers::GoogleAuthChain::Default,
                            env.env_lookup.clone(),
                        );
                    if let Some(handle) = env.auth_lease_handle.clone() {
                        authorizer = authorizer.with_auth_lease_observer(
                            handle,
                            meerkat_core::handles::LeaseKey::from_auth_binding(
                                binding.auth_binding_ref(),
                            ),
                        );
                    }
                    let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(authorizer);
                    let metadata = finalize_auth_metadata(
                        binding,
                        AuthMetadata {
                            provider_metadata: Some(meerkat_core::ProviderAuthMetadata::Google(
                                meerkat_core::GoogleAuthMetadata {
                                    project_id: backend_option_string(binding, "project_id"),
                                    region: backend_option_string(binding, "region"),
                                    ..Default::default()
                                },
                            )),
                            ..Default::default()
                        },
                    )?;
                    Arc::new(DynamicLease::from_authorizer(
                        authorizer,
                        metadata,
                        source_label.clone(),
                    ))
                }
                #[cfg(any(target_arch = "wasm32", not(feature = "adc")))]
                {
                    return Err(ProviderAuthError::SourceResolutionFailed(
                        "adc requires the gemini `adc` feature on non-wasm32".into(),
                    ));
                }
            }
            GoogleAuthMethod::ComputeAdc => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "adc"))]
                {
                    let mut authorizer =
                        meerkat_auth_core::authorizers::GoogleAuthAuthorizer::with_env_lookup(
                            meerkat_auth_core::authorizers::GoogleAuthChain::ComputeOnly,
                            env.env_lookup.clone(),
                        );
                    if let Some(handle) = env.auth_lease_handle.clone() {
                        authorizer = authorizer.with_auth_lease_observer(
                            handle,
                            meerkat_core::handles::LeaseKey::from_auth_binding(
                                binding.auth_binding_ref(),
                            ),
                        );
                    }
                    let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(authorizer);
                    let metadata = finalize_auth_metadata(
                        binding,
                        AuthMetadata {
                            provider_metadata: Some(meerkat_core::ProviderAuthMetadata::Google(
                                meerkat_core::GoogleAuthMetadata {
                                    project_id: backend_option_string(binding, "project_id"),
                                    region: backend_option_string(binding, "region"),
                                    ..Default::default()
                                },
                            )),
                            ..Default::default()
                        },
                    )?;
                    Arc::new(DynamicLease::from_authorizer(
                        authorizer,
                        metadata,
                        source_label.clone(),
                    ))
                }
                #[cfg(any(target_arch = "wasm32", not(feature = "adc")))]
                {
                    return Err(ProviderAuthError::SourceResolutionFailed(
                        "compute_adc requires the gemini `adc` feature on non-wasm32".into(),
                    ));
                }
            }
            GoogleAuthMethod::GoogleOauth => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
                {
                    validate_oauth_target_for_auth_mode(
                        binding.auth_profile(),
                        Provider::Gemini,
                        PersistedAuthMode::GoogleOauth,
                    )
                    .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?;
                    let mut managed =
                        load_managed_store_tokens_with_lifecycle(env, binding).await?;
                    let persisted = managed.tokens.clone();
                    // Cached-vs-refresh disposition owned by the per-binding
                    // AuthMachine: feed the pure observations and mirror the
                    // verdict (see anthropic runtime for the full contract).
                    let effective_tokens = match resolve_oauth_login_credential_disposition(
                        env,
                        binding,
                        persisted.primary_secret.is_some(),
                    )? {
                        OAuthLoginCredentialAdmission::UseCached => persisted,
                        OAuthLoginCredentialAdmission::BeginRefresh => {
                            managed.release_prelock_lifecycle_guard();
                            let persistence = env
                                .provider_auth_persistence()
                                .cloned()
                                .ok_or_else(|| {
                                ProviderAuthError::SourceResolutionFailed(
                                    "managed_store OAuth requires provider-auth persistence authority"
                                        .into(),
                                )
                            })?;
                            let endpoints =
                                oauth::code_assist_endpoints("http://127.0.0.1:0/callback");
                            let runtime = oauth::GoogleCodeAssistOAuthRuntime::new(
                                persistence,
                                endpoints,
                                managed.key.clone(),
                            );
                            let prepare_env = env.clone();
                            let prepare_binding = binding.clone();
                            let prepare: oauth::TokenPrepareFn =
                                Box::new(move |locked_baseline, mode| {
                                    Box::pin(async move {
                                        prepare_managed_store_oauth_refresh_under_lock(
                                            &prepare_env,
                                            &prepare_binding,
                                            managed,
                                            locked_baseline,
                                            mode,
                                        )
                                        .await
                                        .map_err(|error| {
                                            meerkat_auth_core::RefreshError::Refresh(
                                                error.to_string(),
                                            )
                                        })
                                    })
                                });
                            runtime
                                .refresh_tokens_with_locked_preparation(prepare, env.force_refresh)
                                .await
                                .map_err(|error| {
                                    google_code_assist_oauth_refresh_error(error, String::new())
                                })?
                        }
                    };
                    let access = effective_tokens
                        .primary_secret
                        .clone()
                        .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
                    let code_assist_resolution =
                        if matches!(backend_kind, GoogleBackendKind::GoogleCodeAssist) {
                            let base_url = configured_or_default_base_url(
                                backend_kind,
                                binding.backend_profile(),
                            )
                            .ok_or_else(|| {
                                ProviderAuthError::SourceResolutionFailed(
                                    "google_code_assist backend requires BackendProfile.base_url"
                                        .to_string(),
                                )
                            })?;
                            Some(
                                resolve_code_assist_user_project(&access, binding, env, &base_url)
                                    .await?,
                            )
                        } else {
                            None
                        };
                    let code_assist_project_id = code_assist_resolution
                        .as_ref()
                        .map(|r| r.project_id.clone());
                    // The resolved tier of record comes from the single typed
                    // resolution owner (the onboarded/reported tier), falling
                    // back to the explicit binding option only when onboarding
                    // never ran and reported no tier.
                    let code_assist_tier = code_assist_resolution
                        .as_ref()
                        .and_then(|r| r.tier.clone())
                        .or_else(|| backend_option_string(binding, "tier"));
                    let mut google_email: Option<String> = None;
                    let mut google_user_id: Option<String> = None;
                    // Plan §4b.12: lift OIDC claims into AuthMetadata.
                    if let Some(id_token) = effective_tokens.id_token.as_deref()
                        && let Ok(claims) =
                            meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
                    {
                        let lifted = oauth::GoogleIdClaims::lift_from_claims(&claims.raw);
                        google_email = lifted.email;
                        google_user_id = lifted.user_id;
                    }
                    let mut metadata = AuthMetadata::default();
                    if google_email.is_some()
                        || google_user_id.is_some()
                        || code_assist_project_id.is_some()
                    {
                        metadata.account_id = google_user_id.or_else(|| google_email.clone());
                        metadata.provider_metadata =
                            Some(meerkat_core::ProviderAuthMetadata::Google(
                                meerkat_core::GoogleAuthMetadata {
                                    account_email: google_email,
                                    project_id: code_assist_project_id
                                        .or_else(|| backend_option_string(binding, "project_id")),
                                    region: backend_option_string(binding, "region"),
                                    code_assist_tier,
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
            provider: Provider::Gemini,
            backend: NormalizedBackendKind::Google(backend_kind),
            backend_profile: binding.backend_profile().clone(),
            credential_identity: binding.credential_identity().clone(),
            auth_lease: lease,
        })
    }

    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        // ProviderRuntimeRegistry dispatches on Provider enum; non-Google
        // arms are unreachable at runtime.
        let backend_kind = match connection.backend {
            NormalizedBackendKind::Google(k) => k,
            other => unreachable!(
                "GoogleProviderRuntime received non-Google backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        // Authorizer-backed path (Vertex ADC, Code Assist GoogleOauth/
        // ComputeAdc, ExternalAuthorizer-dynamic). Must run before the
        // simpler secret-extraction branch because the authorizer
        // needs backend-specific wiring (Vertex vs Code Assist base
        // URLs). Plan §6.11: read the authorizer from the auth lease
        // directly.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = connection.resolved_authorizer() {
            if matches!(backend_kind, GoogleBackendKind::Copilot) {
                return Err(ProviderClientError::MissingFeature("copilot-text-target"));
            }
            let base_url =
                configured_or_default_base_url(backend_kind, &connection.backend_profile)
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "Google authorizer-backed backends require \
                             BackendProfile.base_url"
                                .to_string(),
                        )
                    })?;
            let mut client = crate::GeminiClient::new_with_base_url(String::new(), base_url)
                .with_authorizer(authorizer)
                .with_google_backend_kind(backend_kind);
            if matches!(backend_kind, GoogleBackendKind::GoogleCodeAssist) {
                client = client.with_code_assist_wire().with_code_assist_project_id(
                    code_assist_project_id_from_metadata(connection.auth_lease.metadata()),
                );
            }
            return Ok(Arc::new(client));
        }
        #[cfg(target_arch = "wasm32")]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::MissingFeature(
                "google-authorizer-backed auth not available on wasm32",
            ))?;
        #[cfg(not(target_arch = "wasm32"))]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        match backend_kind {
            GoogleBackendKind::GoogleGenAi => {
                // S1-verified: GeminiClient::new returns Self (infallible).
                let client = match &connection.backend_profile.base_url {
                    Some(url) => crate::GeminiClient::new_with_base_url(secret, url.clone()),
                    None => crate::GeminiClient::new(secret),
                };
                Ok(Arc::new(client.with_google_backend_kind(backend_kind)))
            }
            GoogleBackendKind::VertexAi => {
                // VertexAi `api_key_express` + `bearer_api_key` use the
                // Vertex-region URL (per BackendProfile.base_url) with
                // the same generative-language wire as GoogleGenAi. ADC
                // + ExternalAuthorizer paths arrive via the Authorizer
                // (deleted shim path). For a raw secret, we
                // treat it as the api_key_express path.
                let base_url = connection
                    .backend_profile
                    .base_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "vertex_ai backend requires BackendProfile.base_url \
                             (e.g. https://<region>-aiplatform.googleapis.com)"
                                .to_string(),
                        )
                    })?;
                let client = crate::GeminiClient::new_with_base_url(secret, base_url)
                    .with_google_backend_kind(backend_kind);
                Ok(Arc::new(client))
            }
            GoogleBackendKind::GoogleCodeAssist => {
                // Code Assist with a pre-resolved bearer secret (e.g.
                // ExternalAuthorizer→Secret subpath, where the host
                // resolved an OAuth access token via its own flow).
                // Wire as GeminiClient with StaticBearerAuthorizer
                // pointed at the Code Assist base URL. Requires
                // BackendProfile.base_url (Code Assist endpoint varies
                // by tier — production is
                // https://cloudcode-pa.googleapis.com).
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let base_url =
                        configured_or_default_base_url(backend_kind, &connection.backend_profile)
                            .ok_or_else(|| {
                            ProviderClientError::InvalidBaseUrl(
                                "google_code_assist backend requires \
                                     BackendProfile.base_url (e.g. \
                                     https://cloudcode-pa.googleapis.com)"
                                    .to_string(),
                            )
                        })?;
                    let authorizer: std::sync::Arc<dyn meerkat_core::HttpAuthorizer> =
                        std::sync::Arc::new(
                            meerkat_auth_core::authorizers::StaticBearerAuthorizer::new(
                                secret,
                                "code-assist-bearer",
                            ),
                        );
                    let client = crate::GeminiClient::new_with_base_url(String::new(), base_url)
                        .with_authorizer(authorizer)
                        .with_google_backend_kind(backend_kind)
                        .with_code_assist_wire()
                        .with_code_assist_project_id(code_assist_project_id_from_metadata(
                            connection.auth_lease.metadata(),
                        ));
                    Ok(Arc::new(client))
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = secret;
                    Err(ProviderClientError::MissingFeature(
                        "google_code_assist backend not available on wasm32",
                    ))
                }
            }
            GoogleBackendKind::Copilot => {
                Err(ProviderClientError::MissingFeature("copilot-text-target"))
            }
        }
    }

    fn build_text_client(
        &self,
        target: ResolvedTextTarget,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        if !matches!(
            target.connection().backend,
            NormalizedBackendKind::Google(GoogleBackendKind::Copilot)
        ) {
            let (_, _, connection) = target.into_parts();
            return self.build_client(connection);
        }
        #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
        {
            let runtime = self.copilot.as_ref().ok_or_else(|| {
                ProviderClientError::ClientInit(
                    "Gemini Copilot backend is not composed with CopilotRuntime".to_string(),
                )
            })?;
            let (identity, profile, connection) = target.into_parts();
            let chat_factory = self
                .copilot_chat_completions
                .as_ref()
                .ok_or_else(|| {
                    ProviderClientError::ClientInit(
                        "Gemini Copilot backend is not composed with a Chat Completions client factory"
                            .to_string(),
                    )
                })?
                .clone();
            let model = identity.model.clone();
            let supports_temperature = profile.profile().supports_temperature;
            let supports_image_tool_results = profile.profile().image_tool_results;
            let factory: meerkat_copilot::CopilotRouteClientFactory =
                Arc::new(move |route, connection| {
                    let endpoint = match route.access {
                        meerkat_copilot::CopilotModelAccess::Available { endpoint } => endpoint,
                        meerkat_copilot::CopilotModelAccess::Unknown => {
                            meerkat_copilot::CopilotEndpoint::optimistic_for_provider(
                                Provider::Gemini,
                            )
                            .ok_or_else(|| {
                                ProviderClientError::ClientInit(
                                    "Copilot has no optimistic Gemini route".to_string(),
                                )
                            })?
                        }
                        meerkat_copilot::CopilotModelAccess::Unavailable => {
                            return Err(ProviderClientError::ClientInit(
                                route.unavailable_message(
                                    Provider::Gemini,
                                    &model,
                                    meerkat_copilot::CopilotEndpoint::ChatCompletions,
                                ),
                            ));
                        }
                    };
                    if endpoint != meerkat_copilot::CopilotEndpoint::ChatCompletions {
                        return Err(ProviderClientError::ClientInit(route.unavailable_message(
                            Provider::Gemini,
                            &model,
                            meerkat_copilot::CopilotEndpoint::ChatCompletions,
                        )));
                    }
                    let authorizer = connection
                        .resolved_authorizer()
                        .ok_or(ProviderClientError::NoCredentialMaterial)?;
                    let authorizer = route.bind_authorizer(authorizer);
                    let client = chat_factory.build(
                        meerkat_copilot::CopilotChatCompletionsClientSpec::new(
                            Provider::Gemini,
                            model.clone(),
                            route.api_base.clone(),
                            authorizer,
                            supports_temperature,
                            true,
                            true,
                            supports_image_tool_results,
                        ),
                    )?;
                    Ok(Arc::new(GeminiCopilotChatClient { inner: client }))
                });
            meerkat_copilot::routed_client(
                Arc::clone(runtime),
                connection,
                Provider::Gemini,
                identity.model,
                factory,
            )
        }
        #[cfg(not(all(feature = "copilot", not(target_arch = "wasm32"))))]
        {
            let _ = target;
            Err(ProviderClientError::MissingFeature("copilot"))
        }
    }

    fn build_image_generation_executor(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Option<Arc<dyn ImageGenerationExecutor>>, ProviderClientError> {
        let backend_kind = match connection.backend {
            NormalizedBackendKind::Google(k) => k,
            other => unreachable!(
                "GoogleProviderRuntime received non-Google backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        if matches!(backend_kind, GoogleBackendKind::Copilot) {
            return Ok(None);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = connection.resolved_authorizer() {
            let base_url =
                configured_or_default_base_url(backend_kind, &connection.backend_profile)
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "Google authorizer-backed backends require BackendProfile.base_url"
                                .to_string(),
                        )
                    })?;
            let mut client = crate::GeminiClient::new_with_base_url(String::new(), base_url)
                .with_authorizer(authorizer)
                .with_google_backend_kind(backend_kind);
            if matches!(backend_kind, GoogleBackendKind::GoogleCodeAssist) {
                client = client.with_code_assist_wire().with_code_assist_project_id(
                    code_assist_project_id_from_metadata(connection.auth_lease.metadata()),
                );
            }
            return Ok(Some(Arc::new(client)));
        }
        #[cfg(target_arch = "wasm32")]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::MissingFeature(
                "google-authorizer-backed auth not available on wasm32",
            ))?;
        #[cfg(not(target_arch = "wasm32"))]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        let client = match backend_kind {
            GoogleBackendKind::GoogleGenAi => match &connection.backend_profile.base_url {
                Some(url) => crate::GeminiClient::new_with_base_url(secret, url.clone()),
                None => crate::GeminiClient::new(secret),
            },
            GoogleBackendKind::VertexAi | GoogleBackendKind::GoogleCodeAssist => {
                let base_url =
                    configured_or_default_base_url(backend_kind, &connection.backend_profile)
                        .ok_or_else(|| {
                            ProviderClientError::InvalidBaseUrl(
                                "google image executor backend requires BackendProfile.base_url"
                                    .to_string(),
                            )
                        })?;
                let client = crate::GeminiClient::new_with_base_url(secret, base_url)
                    .with_google_backend_kind(backend_kind);
                if matches!(backend_kind, GoogleBackendKind::GoogleCodeAssist) {
                    client.with_code_assist_wire().with_code_assist_project_id(
                        code_assist_project_id_from_metadata(connection.auth_lease.metadata()),
                    )
                } else {
                    client
                }
            }
            GoogleBackendKind::Copilot => return Ok(None),
        };
        Ok(Some(Arc::new(
            client.with_google_backend_kind(backend_kind),
        )))
    }

    fn image_generation_profile(
        &self,
    ) -> Option<Arc<dyn meerkat_core::ImageGenerationProviderProfile>> {
        Some(Arc::new(crate::GeminiImageGenerationProfile))
    }
}

#[cfg(any(
    all(not(target_arch = "wasm32"), feature = "adc"),
    all(not(target_arch = "wasm32"), feature = "oauth")
))]
fn backend_option_string(binding: &ValidatedBinding, key: &str) -> Option<String> {
    binding
        .backend_profile()
        .options
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    use std::sync::Mutex;

    use meerkat_core::{BackendProfile, HttpAuthorizationRequest, HttpAuthorizer};
    use meerkat_llm_core::provider_runtime::ProviderRuntimeCatalog;

    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    struct CapturingCopilotChatClient {
        seen_provider_params:
            Arc<Mutex<Option<meerkat_core::lifecycle::run_primitive::ProviderTag>>>,
    }

    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    #[async_trait]
    impl LlmClient for CapturingCopilotChatClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_llm_core::LlmError> {
            Ok(messages.to_vec())
        }

        fn request_pressure(
            &self,
            request: &meerkat_llm_core::LlmRequest,
        ) -> Result<Option<meerkat_core::ProviderRequestPressure>, meerkat_llm_core::LlmError>
        {
            *self.seen_provider_params.lock().expect("capture lock") =
                request.provider_params.clone();
            Ok(None)
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_llm_core::LlmRequest,
        ) -> meerkat_llm_core::LlmStream<'a> {
            Box::pin(futures::stream::empty())
        }

        fn provider(&self) -> Provider {
            Provider::Gemini
        }

        async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
            Ok(())
        }
    }

    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    #[test]
    fn copilot_chat_lowers_gemini_structured_output_for_response_format() {
        use meerkat_core::lifecycle::run_primitive::{
            GeminiProviderTag, GeminiThinkingLevel, ProviderTag, ReasoningEffort,
        };

        let seen = Arc::new(Mutex::new(None));
        let client = GeminiCopilotChatClient {
            inner: Arc::new(CapturingCopilotChatClient {
                seen_provider_params: Arc::clone(&seen),
            }),
        };
        let output_schema = meerkat_core::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"]
        }))
        .expect("valid output schema");
        let mut request = meerkat_llm_core::LlmRequest::new("gemini-test", Vec::new());
        request.provider_params = Some(ProviderTag::Gemini(GeminiProviderTag {
            thinking_level: Some(GeminiThinkingLevel::High),
            structured_output: Some(output_schema),
            ..Default::default()
        }));

        client
            .request_pressure(&request)
            .expect("Gemini Copilot request lowers");

        let captured = seen.lock().expect("capture lock").clone();
        let Some(ProviderTag::OpenAi(tag)) = captured else {
            panic!("Gemini Copilot must lower into the Chat Completions wire tag");
        };
        assert!(tag.structured_output.is_some());
        assert_eq!(tag.reasoning_effort, Some(ReasoningEffort::High));
    }

    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    #[test]
    fn copilot_chat_rejects_unrepresentable_gemini_parameters() {
        use meerkat_core::lifecycle::run_primitive::{GeminiProviderTag, ProviderTag};

        let client = GeminiCopilotChatClient {
            inner: Arc::new(CapturingCopilotChatClient {
                seen_provider_params: Arc::new(Mutex::new(None)),
            }),
        };
        let mut request = meerkat_llm_core::LlmRequest::new("gemini-test", Vec::new());
        request.provider_params = Some(ProviderTag::Gemini(GeminiProviderTag {
            top_p: Some(0.5),
            ..Default::default()
        }));

        assert!(matches!(
            client.request_pressure(&request),
            Err(meerkat_llm_core::LlmError::InvalidRequest { message })
                if message.contains("top_p")
        ));
    }

    #[test]
    fn typed_catalog_covers_three_backends() {
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::Google(GoogleBackendKind::GoogleGenAi),
            NormalizedAuthMethod::Google(GoogleAuthMethod::ApiKey),
        ));
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::Google(GoogleBackendKind::VertexAi),
            NormalizedAuthMethod::Google(GoogleAuthMethod::Adc),
        ));
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::Google(GoogleBackendKind::GoogleCodeAssist),
            NormalizedAuthMethod::Google(GoogleAuthMethod::GoogleOauth),
        ));
    }

    #[test]
    fn provider_id_is_gemini() {
        assert_eq!(GoogleProviderRuntime.provider_id(), Provider::Gemini);
    }

    #[test]
    fn code_assist_missing_base_url_resolves_to_cloudcode_default() {
        let backend = BackendProfile {
            id: "google_code_assist".into(),
            provider: Provider::Gemini,
            backend_kind: GoogleBackendKind::GoogleCodeAssist.as_str().into(),
            base_url: None,
            options: serde_json::Value::Null,
            server: None,
        };

        assert_eq!(
            configured_or_default_base_url(GoogleBackendKind::GoogleCodeAssist, &backend)
                .as_deref(),
            Some("https://cloudcode-pa.googleapis.com")
        );
        assert!(
            configured_or_default_base_url(GoogleBackendKind::VertexAi, &backend).is_none(),
            "Vertex remains region-specific and must still be configured"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct NoopAuthorizer;

    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait::async_trait]
    impl HttpAuthorizer for NoopAuthorizer {
        async fn authorize(
            &self,
            _req: &mut HttpAuthorizationRequest<'_>,
        ) -> Result<(), AuthError> {
            Ok(())
        }

        fn label(&self) -> &'static str {
            "noop"
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn code_assist_dynamic_authorizer_builds_client_without_configured_base_url() {
        let backend = Arc::new(BackendProfile {
            id: "google_code_assist".into(),
            provider: Provider::Gemini,
            backend_kind: GoogleBackendKind::GoogleCodeAssist.as_str().into(),
            base_url: None,
            options: serde_json::Value::Null,
            server: None,
        });
        let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(NoopAuthorizer);
        let lease: Arc<dyn AuthLease> = Arc::new(
            meerkat_llm_core::provider_runtime::binding::DynamicLease::from_authorizer(
                authorizer,
                AuthMetadata::default(),
                "test-google-oauth",
            ),
        );
        let connection = ResolvedConnection {
            provider: Provider::Gemini,
            backend: NormalizedBackendKind::Google(GoogleBackendKind::GoogleCodeAssist),
            backend_profile: backend,
            credential_identity: meerkat_core::AuthCredentialIdentity::from_auth_binding(
                &meerkat_core::AuthBindingRef {
                    realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
                    binding: meerkat_core::BindingId::parse("google").expect("valid binding"),
                    profile: None,
                    origin: meerkat_core::BindingOrigin::Configured,
                },
            ),
            auth_lease: lease,
        };

        GoogleProviderRuntime
            .build_client(connection)
            .expect("Google OAuth Code Assist should use the default cloudcode base URL");
    }
}
