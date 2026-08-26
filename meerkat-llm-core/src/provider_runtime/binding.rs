//! Normalized binding shapes, resolved-connection shim, and concrete
//! lease implementations used by Phase 2 provider runtimes.
//!
//! `NormalizedBackendKind` / `NormalizedAuthMethod` are typed sums over
//! per-provider enums declared in `providers/<p>/{backend,auth}.rs`.
//! `ResolvedConnection.shim_credential` is the **Phase 2-only** seam that
//! `build_client` reads to get the resolved secret. Phase 3 deletes this
//! field when `build_client` owns HTTP request assembly directly.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use meerkat_core::{
    AuthError, AuthLease, AuthMetadata, AuthRefreshReason, BackendProfile, HttpAuthorizer,
    ModelProfileWitness, Provider, ResolvedAuthKind, SessionLlmIdentity,
};

use meerkat_core::provider_matrix::anthropic::{AnthropicAuthMethod, AnthropicBackendKind};
use meerkat_core::provider_matrix::google::{GoogleAuthMethod, GoogleBackendKind};
use meerkat_core::provider_matrix::openai::{OpenAiAuthMethod, OpenAiBackendKind};
use meerkat_core::provider_matrix::self_hosted::{SelfHostedAuthMethod, SelfHostedBackendKind};

pub use crate::provider_runtime::catalog::ValidatedBinding;

/// Exact factory kind qualified by the validated GPT Live client-context POC.
pub const GPT_LIVE_CLIENT_CONTEXT_FACTORY_KIND: &str = "private-live";
/// Exact factory version qualified by the validated GPT Live client-context POC.
pub const GPT_LIVE_CLIENT_CONTEXT_FACTORY_VERSION: &str = "v1";
/// Version of the client-context Gate0 evidence contract compiled into this build.
pub const GPT_LIVE_CLIENT_CONTEXT_GATE0_VERSION: &str = "gate0-v1";
/// SHA-256 of the redacted validated client-context evidence artifact.
///
/// Source artifact:
/// `work/probe/minimum-matrix/client-managed-sideband-delegation-context-v1.json`
pub const GPT_LIVE_CLIENT_CONTEXT_PROTOCOL_DIGEST: &str =
    "145a6ffa26833083c606bb76b16fa2efc614a8619dedf74ca2d497758bbb3228";

/// Provider-tagged normalized backend kind. Each variant is produced by the
/// provider runtime catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NormalizedBackendKind {
    OpenAi(OpenAiBackendKind),
    Anthropic(AnthropicBackendKind),
    Google(GoogleBackendKind),
    SelfHosted(SelfHostedBackendKind),
}

impl NormalizedBackendKind {
    /// The canonical wire string for this backend kind, delegating to the
    /// per-provider matrix enum's `as_str`. The matrix owns the literal; this
    /// is the typed-to-string projection used when materializing config.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi(kind) => kind.as_str(),
            Self::Anthropic(kind) => kind.as_str(),
            Self::Google(kind) => kind.as_str(),
            Self::SelfHosted(kind) => kind.as_str(),
        }
    }

    /// The default API-key backend kind for a provider — the canonical backend
    /// used when no explicit backend hint is given (e.g. non-interactive API-key
    /// login). Total over Provider; `Other` has no default. Owned here (the typed
    /// matrix) so surfaces never hand-map a provider->backend-kind string with a
    /// fail-open arm.
    pub fn default_for_provider(provider: Provider) -> Option<Self> {
        match provider {
            Provider::Anthropic => Some(Self::Anthropic(AnthropicBackendKind::AnthropicApi)),
            Provider::OpenAI => Some(Self::OpenAi(OpenAiBackendKind::OpenAiApi)),
            Provider::Gemini => Some(Self::Google(GoogleBackendKind::GoogleGenAi)),
            Provider::SelfHosted => Some(Self::SelfHosted(SelfHostedBackendKind::SelfHosted)),
            Provider::Other => None,
        }
    }
}

/// The typed [`NormalizedBackendKind`] these OAuth login credentials target.
///
/// Lives here as a free function (not an inherent method on
/// [`meerkat_core::OAuthProviderIdentity`]) because the identity is owned by
/// `meerkat-core` while [`NormalizedBackendKind`] is owned here in
/// `meerkat-llm-core`: this is the lowest crate that can see both. Auth-core's
/// `oauth_provider_declaration` reads it from here rather than re-deriving the
/// mapping.
pub fn oauth_provider_backend_kind(
    id: meerkat_core::OAuthProviderIdentity,
) -> NormalizedBackendKind {
    use meerkat_core::OAuthProviderIdentity;
    match id {
        OAuthProviderIdentity::AnthropicClaudeAi
        | OAuthProviderIdentity::AnthropicConsoleApiKey => {
            NormalizedBackendKind::Anthropic(AnthropicBackendKind::AnthropicApi)
        }
        OAuthProviderIdentity::OpenAiChatGpt => {
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend)
        }
        OAuthProviderIdentity::GoogleCodeAssist => {
            NormalizedBackendKind::Google(GoogleBackendKind::GoogleCodeAssist)
        }
    }
}

/// Provider-tagged normalized auth method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NormalizedAuthMethod {
    OpenAi(OpenAiAuthMethod),
    Anthropic(AnthropicAuthMethod),
    Google(GoogleAuthMethod),
    SelfHosted(SelfHostedAuthMethod),
}

impl NormalizedAuthMethod {
    /// Build the provider-tagged normalized auth method from a resolved
    /// [`AuthProfile`](meerkat_core::AuthProfile), parsing the profile's
    /// declared `auth_method` string through the profile provider's typed
    /// matrix enum. Returns `None` for `Provider::Other` or an `auth_method`
    /// that is not a member of that provider's auth matrix.
    ///
    /// This is the single typed projection consumed by every surface
    /// (RPC/REST/CLI) that holds a resolved `AuthProfile`, replacing the
    /// per-surface inline `provider -> *AuthMethod::parse` copies.
    pub fn from_auth_profile(auth_profile: &meerkat_core::AuthProfile) -> Option<Self> {
        Self::parse_for_provider(auth_profile.provider, auth_profile.auth_method.as_str())
    }

    /// Parse a raw `auth_method` string through the given provider's typed
    /// matrix enum. Returns `None` for `Provider::Other` or a method that is
    /// not a member of that provider's auth matrix.
    ///
    /// This is the single auth-method-string ingress for every surface that
    /// holds a provider identity but no resolved `AuthProfile` (e.g. CLI
    /// non-interactive login).
    pub fn parse_for_provider(provider: Provider, raw: &str) -> Option<Self> {
        match provider {
            Provider::OpenAI => OpenAiAuthMethod::parse(raw).map(Self::OpenAi),
            Provider::Anthropic => AnthropicAuthMethod::parse(raw).map(Self::Anthropic),
            Provider::Gemini => GoogleAuthMethod::parse(raw).map(Self::Google),
            Provider::SelfHosted => SelfHostedAuthMethod::parse(raw).map(Self::SelfHosted),
            Provider::Other => None,
        }
    }

    /// The persisted credential mode this auth method stores in the
    /// `TokenStore`, or `None` for authorizer/ADC/SigV4-backed methods that
    /// hold no persisted secret.
    ///
    /// Delegates to the per-provider `*AuthMethod::persisted_auth_mode`, which
    /// is the typed owner of the auth-method -> persisted-mode mapping.
    pub fn persisted_auth_mode(self) -> Option<meerkat_core::auth::token_store::PersistedAuthMode> {
        match self {
            Self::OpenAi(method) => method.persisted_auth_mode(),
            Self::Anthropic(method) => method.persisted_auth_mode(),
            Self::Google(method) => method.persisted_auth_mode(),
            Self::SelfHosted(method) => method.persisted_auth_mode(),
        }
    }

    /// The canonical wire string for this auth method, delegating to the
    /// per-provider matrix enum's as_str (the matrix owns the literal).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi(m) => m.as_str(),
            Self::Anthropic(m) => m.as_str(),
            Self::Google(m) => m.as_str(),
            Self::SelfHosted(m) => m.as_str(),
        }
    }
}

// Plan §6.11 deleted the legacy marker enum. Credential material
// lives on `auth_lease` directly via the typed `ResolvedAuthKind`
// variants: `InlineSecret(Arc<String>)` for simple-secret flows
// (dogma §5 closure — replaces the prior `__secret__` magic-header
// convention), `DynamicAuthorizer` for authorizer-backed flows,
// `StaticHeaders` for multi-header wire-level envelopes, and `None`
// for authless transports. `build_client` reads
// `ResolvedConnection::resolved_secret()` /
// `resolved_authorizer()`.

/// A fully resolved connection carries the trait-object lease alongside
/// backend metadata.
#[derive(Clone)]
pub struct ResolvedConnection {
    pub provider: Provider,
    pub backend: NormalizedBackendKind,
    pub backend_profile: Arc<BackendProfile>,
    pub auth_lease: Arc<dyn AuthLease>,
}

/// Factory-resolved realtime construction target.
///
/// This binds one exact durable session LLM identity and its registry-minted
/// capability witness to the resolved provider connection. Provider runtimes
/// consume this target and may implement transport mechanics, but cannot
/// substitute a model or infer catalog policy from a model-name string.
#[derive(Clone)]
pub struct ResolvedRealtimeTarget {
    identity: SessionLlmIdentity,
    profile: ModelProfileWitness,
    connection: ResolvedConnection,
}

impl ResolvedRealtimeTarget {
    /// Construct a target only when all three pieces name the same provider
    /// and the witness names the exact session model.
    pub fn new(
        identity: SessionLlmIdentity,
        profile: ModelProfileWitness,
        connection: ResolvedConnection,
    ) -> Option<Self> {
        if !profile.matches_identity(&identity) || connection.provider != identity.provider {
            return None;
        }
        Some(Self {
            identity,
            profile,
            connection,
        })
    }

    pub fn identity(&self) -> &SessionLlmIdentity {
        &self.identity
    }

    pub fn profile(&self) -> &ModelProfileWitness {
        &self.profile
    }

    pub fn connection(&self) -> &ResolvedConnection {
        &self.connection
    }

    pub fn into_parts(self) -> (SessionLlmIdentity, ModelProfileWitness, ResolvedConnection) {
        (self.identity, self.profile, self.connection)
    }
}

impl std::fmt::Debug for ResolvedRealtimeTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedRealtimeTarget")
            .field("identity", &self.identity)
            .field("profile", &self.profile)
            .field("connection", &self.connection)
            .finish()
    }
}

/// Structured host policy validated by the lower provider admission owner.
/// It contains configuration facts only - never credentials or a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentalRealtimeQualificationPolicy {
    realm: meerkat_core::RealmId,
    factory_kind: String,
    factory_version: String,
    required_gate0_version: String,
    execution_mode: meerkat_core::LiveExecutionMode,
}

impl ExperimentalRealtimeQualificationPolicy {
    pub fn new(
        realm: meerkat_core::RealmId,
        factory_kind: impl Into<String>,
        factory_version: impl Into<String>,
        required_gate0_version: impl Into<String>,
        execution_mode: meerkat_core::LiveExecutionMode,
    ) -> Result<Self, ExperimentalRealtimeAdmissionError> {
        let policy = Self {
            realm,
            factory_kind: factory_kind.into(),
            factory_version: factory_version.into(),
            required_gate0_version: required_gate0_version.into(),
            execution_mode,
        };
        if !valid_experimental_component(&policy.factory_kind)
            || !valid_experimental_component(&policy.factory_version)
            || !valid_experimental_component(&policy.required_gate0_version)
        {
            return Err(ExperimentalRealtimeAdmissionError::InvalidPolicy);
        }
        Ok(policy)
    }

    pub fn realm(&self) -> &meerkat_core::RealmId {
        &self.realm
    }

    pub fn execution_mode(&self) -> meerkat_core::LiveExecutionMode {
        self.execution_mode
    }
}

fn valid_experimental_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[derive(Debug)]
struct ExperimentalRealtimeAdmissionAuthorityInner;

/// Lower-layer owner of compiled Gate0 qualification and exact target
/// admission. Provider crates accept only carriers minted by this owner.
#[derive(Clone)]
pub struct ExperimentalRealtimeAdmissionAuthority {
    inner: Arc<ExperimentalRealtimeAdmissionAuthorityInner>,
    policy: ExperimentalRealtimeQualificationPolicy,
    protocol_digest: String,
}

impl std::fmt::Debug for ExperimentalRealtimeAdmissionAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExperimentalRealtimeAdmissionAuthority")
            .field("policy", &self.policy)
            .field("protocol_digest", &self.protocol_digest)
            .field("authority", &"[OPAQUE]")
            .finish()
    }
}

impl ExperimentalRealtimeAdmissionAuthority {
    /// Construct only when operator policy exactly matches this compiled
    /// artifact's Gate0 witness.
    pub fn from_compiled_gate0_policy(
        policy: ExperimentalRealtimeQualificationPolicy,
    ) -> Result<Self, ExperimentalRealtimeAdmissionError> {
        if !cfg!(feature = "experimental-gpt-live") {
            return Err(ExperimentalRealtimeAdmissionError::FeatureNotCompiled);
        }
        if policy.factory_kind != GPT_LIVE_CLIENT_CONTEXT_FACTORY_KIND
            || policy.factory_version != GPT_LIVE_CLIENT_CONTEXT_FACTORY_VERSION
            || policy.required_gate0_version != GPT_LIVE_CLIENT_CONTEXT_GATE0_VERSION
            || policy.execution_mode != meerkat_core::LiveExecutionMode::ClientContext
        {
            return Err(ExperimentalRealtimeAdmissionError::Gate0PolicyMismatch);
        }
        Ok(Self {
            inner: Arc::new(ExperimentalRealtimeAdmissionAuthorityInner),
            policy,
            protocol_digest: GPT_LIVE_CLIENT_CONTEXT_PROTOCOL_DIGEST.to_string(),
        })
    }

    pub fn qualify(
        &self,
        realm: &meerkat_core::RealmId,
        factory_kind: &str,
        factory_version: &str,
        execution_mode: meerkat_core::LiveExecutionMode,
    ) -> Result<ExperimentalRealtimeQualificationWitness, ExperimentalRealtimeAdmissionError> {
        if realm != &self.policy.realm
            || factory_kind != self.policy.factory_kind
            || factory_version != self.policy.factory_version
            || execution_mode != self.policy.execution_mode
        {
            return Err(ExperimentalRealtimeAdmissionError::QualificationMismatch);
        }
        Ok(ExperimentalRealtimeQualificationWitness {
            authority: Arc::clone(&self.inner),
            realm: realm.clone(),
            factory_kind: factory_kind.to_string(),
            factory_version: factory_version.to_string(),
            execution_mode,
            protocol_digest: self.protocol_digest.clone(),
        })
    }

    pub fn admit_target(
        &self,
        qualification: ExperimentalRealtimeQualificationWitness,
        target: ResolvedRealtimeTarget,
        binding_use: meerkat_core::AuthBindingUseWitness,
    ) -> Result<AdmittedExperimentalRealtimeTarget, ExperimentalRealtimeAdmissionError> {
        if !Arc::ptr_eq(&self.inner, &qualification.authority)
            || qualification.realm != self.policy.realm
            || qualification.factory_kind != self.policy.factory_kind
            || qualification.factory_version != self.policy.factory_version
            || qualification.execution_mode != self.policy.execution_mode
            || qualification.protocol_digest != self.protocol_digest
        {
            return Err(ExperimentalRealtimeAdmissionError::QualificationMismatch);
        }
        if target.identity().auth_binding.as_ref() != Some(binding_use.auth_binding()) {
            return Err(ExperimentalRealtimeAdmissionError::BindingUseMismatch);
        }
        if !target.profile().matches_identity(target.identity())
            || target.profile().profile().release_stage
                != meerkat_core::ModelReleaseStage::Experimental
            || !target.profile().profile().realtime
        {
            return Err(ExperimentalRealtimeAdmissionError::TargetNotExperimentalRealtime);
        }
        Ok(AdmittedExperimentalRealtimeTarget {
            target,
            retention: ExperimentalRealtimeAdmissionRetention {
                qualification,
                binding_use,
            },
        })
    }
}

/// Side-effect-free compiled/operator/realm/factory qualification minted by
/// the lower admission authority.
pub struct ExperimentalRealtimeQualificationWitness {
    authority: Arc<ExperimentalRealtimeAdmissionAuthorityInner>,
    realm: meerkat_core::RealmId,
    factory_kind: String,
    factory_version: String,
    execution_mode: meerkat_core::LiveExecutionMode,
    protocol_digest: String,
}

impl ExperimentalRealtimeQualificationWitness {
    pub fn execution_mode(&self) -> meerkat_core::LiveExecutionMode {
        self.execution_mode
    }
}

impl std::fmt::Debug for ExperimentalRealtimeQualificationWitness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExperimentalRealtimeQualificationWitness")
            .field("realm", &self.realm)
            .field("factory_kind", &self.factory_kind)
            .field("factory_version", &self.factory_version)
            .field("execution_mode", &self.execution_mode)
            .field("protocol_digest", &self.protocol_digest)
            .field("authority", &"[OPAQUE]")
            .finish()
    }
}

/// Exact authority retained by the concrete provider factory for its entire
/// lifetime. It is deliberately non-serializable and exposes no raw target.
pub struct ExperimentalRealtimeAdmissionRetention {
    qualification: ExperimentalRealtimeQualificationWitness,
    binding_use: meerkat_core::AuthBindingUseWitness,
}

impl std::fmt::Debug for ExperimentalRealtimeAdmissionRetention {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExperimentalRealtimeAdmissionRetention")
            .field("qualification", &self.qualification)
            .field("auth_binding", self.binding_use.auth_binding())
            .finish()
    }
}

/// Only provider construction input for pre-release realtime factories.
pub struct AdmittedExperimentalRealtimeTarget {
    target: ResolvedRealtimeTarget,
    retention: ExperimentalRealtimeAdmissionRetention,
}

impl AdmittedExperimentalRealtimeTarget {
    pub fn identity(&self) -> &SessionLlmIdentity {
        self.target.identity()
    }

    pub fn into_parts(
        self,
    ) -> (
        ResolvedRealtimeTarget,
        ExperimentalRealtimeAdmissionRetention,
    ) {
        (self.target, self.retention)
    }
}

impl std::fmt::Debug for AdmittedExperimentalRealtimeTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmittedExperimentalRealtimeTarget")
            .field("identity", self.target.identity())
            .field("admission", &"[OPAQUE]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExperimentalRealtimeAdmissionError {
    #[error("experimental realtime support is not compiled")]
    FeatureNotCompiled,
    #[error("compiled experimental realtime Gate0 evidence is unavailable")]
    Gate0Unavailable,
    #[error("operator policy does not match compiled Gate0 evidence")]
    Gate0PolicyMismatch,
    #[error("experimental realtime policy is invalid")]
    InvalidPolicy,
    #[error("experimental realtime qualification does not match its owner")]
    QualificationMismatch,
    #[error("experimental realtime target does not match binding-use authority")]
    BindingUseMismatch,
    #[error("target is not exact experimental realtime registry evidence")]
    TargetNotExperimentalRealtime,
}

#[cfg(test)]
mod experimental_realtime_admission_tests {
    use super::*;
    use meerkat_core::{
        ActingOnBehalfOf, AuthBindingRef, AuthBindingUseRequest, AuthGrant, AuthMetadata,
        BindingId, BindingOrigin, Config, GrantAction, GrantScope, ModelRegistry, PrincipalKind,
        PrincipalRef, RealmId, authorize_explicit_auth_binding_use,
    };
    use std::collections::BTreeSet;

    const FACTORY_KIND: &str = "private-live";
    const FACTORY_VERSION: &str = "v1";

    type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

    fn realm(value: &str) -> TestResult<RealmId> {
        Ok(RealmId::parse(value)?)
    }

    fn binding(realm_name: &str, binding_name: &str) -> TestResult<AuthBindingRef> {
        Ok(AuthBindingRef {
            realm: realm(realm_name)?,
            binding: BindingId::parse(binding_name)?,
            profile: None,
            origin: BindingOrigin::Configured,
        })
    }

    fn authority(
        realm_name: &str,
        digest_byte: &str,
    ) -> TestResult<ExperimentalRealtimeAdmissionAuthority> {
        Ok(ExperimentalRealtimeAdmissionAuthority {
            inner: Arc::new(ExperimentalRealtimeAdmissionAuthorityInner),
            policy: ExperimentalRealtimeQualificationPolicy::new(
                realm(realm_name)?,
                FACTORY_KIND,
                FACTORY_VERSION,
                "gate0-v1",
                meerkat_core::LiveExecutionMode::ClientContext,
            )?,
            protocol_digest: digest_byte.repeat(32),
        })
    }

    fn binding_use_witness(
        auth_binding: AuthBindingRef,
    ) -> TestResult<meerkat_core::AuthBindingUseWitness> {
        let principal = PrincipalRef::new(PrincipalKind::Human, "alice")?;
        let target = PrincipalRef::new(PrincipalKind::PersonalAgent, "agent")?;
        let request =
            AuthBindingUseRequest::new(principal.clone(), target.clone(), auth_binding.clone());
        let grant = AuthGrant {
            principal: principal.clone(),
            scope: GrantScope::AuthBinding {
                realm_id: auth_binding.realm,
                binding_id: auth_binding.binding,
                profile_id: auth_binding.profile,
            },
            actions: BTreeSet::from([GrantAction::UseAuthBinding]),
            acting_on_behalf_of: Some(ActingOnBehalfOf::new(principal, target)),
        };
        Ok(authorize_explicit_auth_binding_use(&request, &[grant]).into_result()?)
    }

    fn target(model: &str, auth_binding: AuthBindingRef) -> TestResult<ResolvedRealtimeTarget> {
        let registry = ModelRegistry::from_config(&Config::default(), meerkat_models::canonical())?;
        let profile = registry
            .profile_witness_for_provider(Provider::OpenAI, model)
            .ok_or_else(|| std::io::Error::other("canonical model profile"))?;
        let identity = SessionLlmIdentity {
            model: model.to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(auth_binding),
        };
        let connection = ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend),
            backend_profile: Arc::new(BackendProfile {
                id: "test-chatgpt".to_string(),
                provider: Provider::OpenAI,
                backend_kind: OpenAiBackendKind::ChatGptBackend.as_str().to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            }),
            auth_lease: Arc::new(StaticLease::empty_lease(AuthMetadata::default(), "test")),
        };
        ResolvedRealtimeTarget::new(identity, profile, connection)
            .ok_or_else(|| std::io::Error::other("exact target").into())
    }

    #[test]
    fn qualification_rejects_wrong_realm_and_factory() -> TestResult<()> {
        let authority = authority("voice", "ab")?;
        assert!(matches!(
            authority.qualify(
                &realm("other")?,
                FACTORY_KIND,
                FACTORY_VERSION,
                meerkat_core::LiveExecutionMode::ClientContext,
            ),
            Err(ExperimentalRealtimeAdmissionError::QualificationMismatch)
        ));
        assert!(matches!(
            authority.qualify(
                &realm("voice")?,
                "other-live",
                FACTORY_VERSION,
                meerkat_core::LiveExecutionMode::ClientContext,
            ),
            Err(ExperimentalRealtimeAdmissionError::QualificationMismatch)
        ));
        assert!(matches!(
            authority.qualify(
                &realm("voice")?,
                FACTORY_KIND,
                "v2",
                meerkat_core::LiveExecutionMode::ClientContext,
            ),
            Err(ExperimentalRealtimeAdmissionError::QualificationMismatch)
        ));
        assert!(matches!(
            authority.qualify(
                &realm("voice")?,
                FACTORY_KIND,
                FACTORY_VERSION,
                meerkat_core::LiveExecutionMode::FunctionBridge,
            ),
            Err(ExperimentalRealtimeAdmissionError::QualificationMismatch)
        ));
        Ok(())
    }

    #[test]
    fn compiled_gate0_policy_qualifies_only_exact_client_context_evidence() -> TestResult<()> {
        let client_policy = ExperimentalRealtimeQualificationPolicy::new(
            realm("voice")?,
            GPT_LIVE_CLIENT_CONTEXT_FACTORY_KIND,
            GPT_LIVE_CLIENT_CONTEXT_FACTORY_VERSION,
            GPT_LIVE_CLIENT_CONTEXT_GATE0_VERSION,
            meerkat_core::LiveExecutionMode::ClientContext,
        )?;
        let client =
            ExperimentalRealtimeAdmissionAuthority::from_compiled_gate0_policy(client_policy);
        if cfg!(feature = "experimental-gpt-live") {
            let client = client?;
            assert_eq!(
                client.protocol_digest,
                GPT_LIVE_CLIENT_CONTEXT_PROTOCOL_DIGEST
            );
        } else {
            assert!(matches!(
                client,
                Err(ExperimentalRealtimeAdmissionError::FeatureNotCompiled)
            ));
        }

        let policy = ExperimentalRealtimeQualificationPolicy::new(
            realm("voice")?,
            GPT_LIVE_CLIENT_CONTEXT_FACTORY_KIND,
            GPT_LIVE_CLIENT_CONTEXT_FACTORY_VERSION,
            GPT_LIVE_CLIENT_CONTEXT_GATE0_VERSION,
            meerkat_core::LiveExecutionMode::FunctionBridge,
        )?;
        assert!(matches!(
            ExperimentalRealtimeAdmissionAuthority::from_compiled_gate0_policy(policy),
            Err(ExperimentalRealtimeAdmissionError::Gate0PolicyMismatch
                | ExperimentalRealtimeAdmissionError::FeatureNotCompiled)
        ));
        Ok(())
    }

    #[test]
    fn admission_rejects_foreign_authority_and_protocol_digest() -> TestResult<()> {
        let owner = authority("voice", "ab")?;
        let foreign = authority("voice", "ab")?;
        let binding = binding("voice", "chatgpt")?;
        let foreign_witness = foreign.qualify(
            &realm("voice")?,
            FACTORY_KIND,
            FACTORY_VERSION,
            meerkat_core::LiveExecutionMode::ClientContext,
        )?;
        assert!(matches!(
            owner.admit_target(
                foreign_witness,
                target("gpt-live-1-codex", binding.clone())?,
                binding_use_witness(binding.clone())?,
            ),
            Err(ExperimentalRealtimeAdmissionError::QualificationMismatch)
        ));

        let mut stale_digest = owner.qualify(
            &realm("voice")?,
            FACTORY_KIND,
            FACTORY_VERSION,
            meerkat_core::LiveExecutionMode::ClientContext,
        )?;
        stale_digest.protocol_digest = "cd".repeat(32);
        assert!(matches!(
            owner.admit_target(
                stale_digest,
                target("gpt-live-1-codex", binding.clone())?,
                binding_use_witness(binding)?,
            ),
            Err(ExperimentalRealtimeAdmissionError::QualificationMismatch)
        ));
        Ok(())
    }

    #[test]
    fn admission_rejects_binding_witness_mismatch() -> TestResult<()> {
        let owner = authority("voice", "ab")?;
        let qualification = owner.qualify(
            &realm("voice")?,
            FACTORY_KIND,
            FACTORY_VERSION,
            meerkat_core::LiveExecutionMode::ClientContext,
        )?;
        assert!(matches!(
            owner.admit_target(
                qualification,
                target("gpt-live-1-codex", binding("voice", "chatgpt")?)?,
                binding_use_witness(binding("voice", "other")?)?,
            ),
            Err(ExperimentalRealtimeAdmissionError::BindingUseMismatch)
        ));
        Ok(())
    }

    #[test]
    fn admission_rejects_stable_and_nonrealtime_targets() -> TestResult<()> {
        for model in ["gpt-realtime-2", "gpt-5.3-codex"] {
            let owner = authority("voice", "ab")?;
            let qualification = owner.qualify(
                &realm("voice")?,
                FACTORY_KIND,
                FACTORY_VERSION,
                meerkat_core::LiveExecutionMode::ClientContext,
            )?;
            let binding = binding("voice", "chatgpt")?;
            assert!(
                matches!(
                    owner.admit_target(
                        qualification,
                        target(model, binding.clone())?,
                        binding_use_witness(binding)?,
                    ),
                    Err(ExperimentalRealtimeAdmissionError::TargetNotExperimentalRealtime)
                ),
                "model {model} must not cross experimental realtime admission"
            );
        }
        Ok(())
    }

    #[test]
    fn admitted_carrier_retains_lower_authority_until_provider_consumes_it() -> TestResult<()> {
        let owner = authority("voice", "ab")?;
        let authority_liveness = Arc::downgrade(&owner.inner);
        let qualification = owner.qualify(
            &realm("voice")?,
            FACTORY_KIND,
            FACTORY_VERSION,
            meerkat_core::LiveExecutionMode::ClientContext,
        )?;
        let binding = binding("voice", "chatgpt")?;
        let admitted = owner.admit_target(
            qualification,
            target("gpt-live-1-codex", binding.clone())?,
            binding_use_witness(binding)?,
        )?;
        drop(owner);
        assert!(authority_liveness.upgrade().is_some());

        let (_target, retention) = admitted.into_parts();
        assert!(authority_liveness.upgrade().is_some());
        drop(retention);
        assert!(authority_liveness.upgrade().is_none());
        Ok(())
    }
}

impl std::fmt::Debug for ResolvedConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedConnection")
            .field("provider", &self.provider)
            .field("backend", &self.backend)
            .field("backend_profile_id", &self.backend_profile.id)
            .finish()
    }
}

impl ResolvedConnection {
    /// Extract the resolved inline secret (api key, bearer token, OAuth
    /// access token) from the auth lease. Returns `None` for
    /// authorizer-backed leases. Plan §6.11 + dogma §5 closure:
    /// reads the typed `InlineSecret` variant of `ResolvedAuthKind`
    /// (replaces the prior `__secret__` synthetic-header convention).
    pub fn resolved_secret(&self) -> Option<String> {
        match self.auth_lease.kind() {
            meerkat_core::ResolvedAuthKind::InlineSecret(secret) => Some((**secret).clone()),
            _ => None,
        }
    }

    /// Extract the resolved dynamic authorizer (AWS SigV4, Google Auth,
    /// Azure AD, ExternalAuthorizer-backed) from the auth lease. Returns
    /// `None` for non-authorizer leases. Plan §6.11.
    pub fn resolved_authorizer(&self) -> Option<Arc<dyn HttpAuthorizer>> {
        match self.auth_lease.kind() {
            meerkat_core::ResolvedAuthKind::DynamicAuthorizer(auth) => Some(auth.clone()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------
// Lease implementations
// ---------------------------------------------------------------------

/// Static lease holding pre-projected headers + metadata. Used for api_key
/// and static_bearer resolutions.
pub struct StaticLease {
    kind: ResolvedAuthKind,
    metadata: AuthMetadata,
    expires_at: Option<DateTime<Utc>>,
    source_label: String,
}

impl StaticLease {
    /// Construct a lease carrying pre-projected wire headers. Used by
    /// resolvers that know the full header set (future post-§6.12
    /// paths). For raw secrets (api keys / bearer tokens), callers
    /// should prefer [`StaticLease::inline_secret`].
    pub fn new(
        headers: Vec<(String, String)>,
        metadata: AuthMetadata,
        expires_at: Option<DateTime<Utc>>,
        source_label: impl Into<String>,
    ) -> Self {
        Self {
            kind: ResolvedAuthKind::StaticHeaders(headers),
            metadata,
            expires_at,
            source_label: source_label.into(),
        }
    }

    /// Construct a lease carrying a raw inline secret (api key, bearer
    /// token, OAuth access token). Plan §6.11 + dogma §5: the typed
    /// `ResolvedAuthKind::InlineSecret` variant replaces the earlier
    /// `StaticHeaders(vec![("__secret__", value)])` magic-string
    /// convention.
    pub fn inline_secret(
        secret: String,
        metadata: AuthMetadata,
        expires_at: Option<DateTime<Utc>>,
        source_label: impl Into<String>,
    ) -> Self {
        Self {
            kind: ResolvedAuthKind::InlineSecret(Arc::new(secret)),
            metadata,
            expires_at,
            source_label: source_label.into(),
        }
    }

    /// Construct a lease with no credential material (authorizer-backed
    /// flows where the runtime constructs the authorizer in
    /// `build_client`, not the resolver). Matches
    /// `ResolvedAuthKind::None`.
    pub fn empty_lease(metadata: AuthMetadata, source_label: impl Into<String>) -> Self {
        Self {
            kind: ResolvedAuthKind::None,
            metadata,
            expires_at: None,
            source_label: source_label.into(),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AuthLease for StaticLease {
    fn kind(&self) -> &ResolvedAuthKind {
        &self.kind
    }
    fn metadata(&self) -> &AuthMetadata {
        &self.metadata
    }
    fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_at
    }
    fn source_label(&self) -> &str {
        &self.source_label
    }
    async fn refresh(&self, _reason: AuthRefreshReason) -> Result<(), AuthError> {
        // StaticLease has no refresh semantics in Phase 2.
        Ok(())
    }
}

/// Dynamic lease wrapping a runtime authorizer. Phase 2 build_client does
/// not accept this shape — authorizer-backed flows use this directly
/// `build_client` returns DynamicAuthorizerNotYetSupportedInShimMode.
pub struct DynamicLease {
    authorizer: Arc<dyn HttpAuthorizer>,
    metadata: AuthMetadata,
    expires_at: Option<DateTime<Utc>>,
    source_label: String,
    kind: ResolvedAuthKind,
}

impl DynamicLease {
    pub fn new(
        authorizer: Arc<dyn HttpAuthorizer>,
        metadata: AuthMetadata,
        expires_at: Option<DateTime<Utc>>,
        source_label: impl Into<String>,
    ) -> Self {
        let kind = ResolvedAuthKind::DynamicAuthorizer(authorizer.clone());
        Self {
            authorizer,
            metadata,
            expires_at,
            source_label: source_label.into(),
            kind,
        }
    }

    /// Construct a dynamic lease whose freshness is projected from the
    /// underlying authorizer. This is for authorizer-backed flows such as
    /// Google ADC and Azure AD where the token is fetched lazily per request.
    pub fn from_authorizer(
        authorizer: Arc<dyn HttpAuthorizer>,
        metadata: AuthMetadata,
        source_label: impl Into<String>,
    ) -> Self {
        Self::new(authorizer, metadata, None, source_label)
    }

    pub fn authorizer(&self) -> &Arc<dyn HttpAuthorizer> {
        &self.authorizer
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AuthLease for DynamicLease {
    fn kind(&self) -> &ResolvedAuthKind {
        &self.kind
    }
    fn metadata(&self) -> &AuthMetadata {
        &self.metadata
    }
    fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_at.or_else(|| self.authorizer.expires_at())
    }
    fn source_label(&self) -> &str {
        &self.source_label
    }
    async fn refresh(&self, reason: AuthRefreshReason) -> Result<(), AuthError> {
        // A dynamic lease has no in-place refresh: the caller must re-resolve
        // the typed auth binding through the resolver. Surface that as the
        // typed `ResolveRequired`, not a generic `RefreshFailed` (no refresh
        // was attempted), so callers can branch on re-resolution explicitly.
        Err(AuthError::ResolveRequired(format!(
            "dynamic lease '{}' cannot refresh in place for reason {reason:?}; re-resolve the typed auth_binding",
            self.source_label
        )))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // Plan §6.11 deleted the §6.11 deletion artifact cleanup.

    #[tokio::test]
    async fn static_lease_satisfies_trait() {
        let lease: Arc<dyn AuthLease> = Arc::new(StaticLease::new(
            Vec::new(),
            AuthMetadata::default(),
            None,
            "test",
        ));
        assert!(matches!(lease.kind(), ResolvedAuthKind::StaticHeaders(_)));
        assert_eq!(lease.source_label(), "test");
        assert!(lease.refresh(AuthRefreshReason::Manual).await.is_ok());
    }

    #[tokio::test]
    async fn dynamic_lease_refresh_reports_unsupported_instead_of_success() {
        #[derive(Debug)]
        struct TestAuthorizer;

        #[async_trait::async_trait]
        impl HttpAuthorizer for TestAuthorizer {
            async fn authorize(
                &self,
                _req: &mut meerkat_core::auth::HttpAuthorizationRequest<'_>,
            ) -> Result<(), AuthError> {
                Ok(())
            }

            fn label(&self) -> &'static str {
                "test-authorizer"
            }
        }

        let lease: Arc<dyn AuthLease> = Arc::new(DynamicLease::new(
            Arc::new(TestAuthorizer),
            AuthMetadata::default(),
            None,
            "dynamic:test",
        ));
        let err = lease
            .refresh(AuthRefreshReason::Manual)
            .await
            .expect_err("dynamic refresh must not report success without work");
        assert!(matches!(err, AuthError::ResolveRequired(_)));
    }

    #[tokio::test]
    async fn dynamic_lease_projects_authorizer_freshness() {
        #[derive(Debug)]
        struct ExpiringAuthorizer {
            expires_at: DateTime<Utc>,
        }

        #[async_trait::async_trait]
        impl HttpAuthorizer for ExpiringAuthorizer {
            async fn authorize(
                &self,
                _req: &mut meerkat_core::auth::HttpAuthorizationRequest<'_>,
            ) -> Result<(), AuthError> {
                Ok(())
            }

            fn label(&self) -> &'static str {
                "expiring-authorizer"
            }

            fn expires_at(&self) -> Option<DateTime<Utc>> {
                Some(self.expires_at)
            }
        }

        let expires_at =
            chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 4, 28, 12, 0, 0).unwrap();
        let lease: Arc<dyn AuthLease> = Arc::new(DynamicLease::from_authorizer(
            Arc::new(ExpiringAuthorizer { expires_at }),
            AuthMetadata::default(),
            "dynamic:expiring",
        ));

        assert_eq!(lease.expires_at(), Some(expires_at));
    }

    #[test]
    fn default_backend_kind_per_provider_is_typed() {
        assert_eq!(
            NormalizedBackendKind::default_for_provider(Provider::Anthropic),
            Some(NormalizedBackendKind::Anthropic(
                AnthropicBackendKind::AnthropicApi
            ))
        );
        assert_eq!(
            NormalizedBackendKind::default_for_provider(Provider::OpenAI),
            Some(NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi))
        );
        assert_eq!(
            NormalizedBackendKind::default_for_provider(Provider::Gemini),
            Some(NormalizedBackendKind::Google(
                GoogleBackendKind::GoogleGenAi
            ))
        );
        assert_eq!(
            NormalizedBackendKind::default_for_provider(Provider::SelfHosted),
            Some(NormalizedBackendKind::SelfHosted(
                SelfHostedBackendKind::SelfHosted
            ))
        );
        assert_eq!(
            NormalizedBackendKind::default_for_provider(Provider::Other),
            None
        );
    }

    #[test]
    fn auth_method_as_str_delegates_to_matrix() {
        assert_eq!(
            NormalizedAuthMethod::Anthropic(AnthropicAuthMethod::ClaudeAiOauth).as_str(),
            "claude_ai_oauth"
        );
    }
}
