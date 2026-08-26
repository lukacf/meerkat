//! Facade-owned bridge from the experimental OpenAI adapter to Meerkat's
//! provider-neutral WebRTC broker contract.
//!
//! This module is an internal composition seam. It does not admit models,
//! resolve credentials, or grant signaling/context authority. Callers must
//! first consume the experimental live admission witness into the lower
//! opaque admitted target accepted by the OpenAI factory.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use meerkat_contracts::{
    LiveOpenResult, RealtimeAudioFormat, RealtimeCapabilities, RealtimeInputKind,
    RealtimeOutputKind, RealtimeTurningMode, WireLiveTransportBootstrap,
};
use meerkat_core::live_adapter::{
    LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterErrorCode,
    LiveAdapterObservation, LiveAdapterStatus, LiveChannelCapabilities,
};
use meerkat_core::{Provider, StopReason, TurnUsage, Usage};
use meerkat_live::{
    LiveSidebandAppendAttempt, LiveSidebandCommand, LiveSidebandCommandDelivery,
    LiveSidebandDelegationRef, LiveSidebandObservation, LiveSidebandObservationKind,
    LiveSidebandProviderCommand, LiveSidebandTranscriptItemRef, LiveSidebandTurnRef,
    LiveSidebandTurnRole, LiveWebrtcAdmittedOffer, LiveWebrtcAnswerAccepted,
    LiveWebrtcAnswerTransport, LiveWebrtcBindingRequest, LiveWebrtcError, ProviderWebrtcBinding,
    ProviderWebrtcBroker, ProviderWebrtcBrokerAnswer, ProviderWebrtcBrokerError,
    ProviderWebrtcOffer, ProviderWebrtcPendingBoundReadyResolver, ProviderWebrtcSidebandSession,
    ProviderWebrtcSignalingError,
};
use meerkat_llm_core::realtime_session::{
    RealtimeExternalSessionTarget, RealtimeSessionFactory, RealtimeSessionOpenConfig,
};
use meerkat_llm_core::{LlmError, RealtimeSession};
use meerkat_openai::{
    GptLiveAppendToken, GptLiveBrokerError, GptLiveBrokerFactory, GptLiveBrokerObservation,
    GptLiveBrokerOpenConfig, GptLiveBrokerSession, GptLiveBrokerTerminalClass,
    GptLiveDelegationRef, GptLiveResponsesSessionConfig, GptLiveTurnRef, GptLiveTurnRole,
};
use meerkat_runtime::live_execution::{
    LiveContextAppendAuthority, LiveDelegationResultDeliveryAuthority,
    LiveDelegationResultDeliveryObservation,
};
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::session_runtime::live_orchestration::RealtimeSessionOpenProjection;

/// Sanitized failure from the host-injected experimental open authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExperimentalLiveOpenAuthorityError {
    #[error("experimental live execution is unavailable")]
    Unavailable,
    #[error("experimental live execution access was denied")]
    AccessDenied,
    #[error("the authoritative durable target is unavailable")]
    DurableTargetUnavailable,
    #[error("the authoritative durable member is ineligible for experimental live execution")]
    MemberIneligible,
    #[error("the requested channel identity is invalid")]
    InvalidExecutionIdentity,
    #[error("experimental live binding use was denied")]
    BindingUseDenied,
    #[error("experimental live provider admission failed")]
    AdmissionFailed,
    #[error("experimental live channel binding failed")]
    ChannelBindingFailed,
}

/// One opaque prepared open returned by an authority provider.
///
/// The handler can bind it only to the exact successful shared live/open
/// result. Principal, grants, durable-target authority, auth binding, and
/// provider factory remain inside the host implementation and sealed pending
/// value.
#[async_trait]
pub trait ExperimentalLivePendingOpen: Send {
    /// Apply the exact admitted execution identity to the canonical durable
    /// projection before shared machine admission.
    fn apply_execution_identity(&self, projection: &mut RealtimeSessionOpenProjection);

    /// The one-use prepared factory consumed by the shared S7-S9 pipeline.
    fn session_factory(&self) -> &dyn RealtimeSessionFactory;

    /// Server-qualified execution profile bound to the admitted catalog
    /// target. Surfaces cannot select or override this value.
    fn execution_profile(&self) -> &meerkat_runtime::live_execution::LiveExecutionProfileSelection;

    async fn bind_opened(
        self: Box<Self>,
        opened: &LiveOpenResult,
    ) -> Result<(), ExperimentalLiveOpenAuthorityError>;
}

/// Host-injected authority for a strict experimental live/open request.
///
/// Nothing in this seam accepts caller-supplied authority. Implementations
/// derive the authenticated principal, authoritative durable target, realm
/// grants, and configured admission owner from host state before calling the
/// facade admission service.
#[async_trait]
pub trait ExperimentalLiveOpenAuthorityProvider: Send + Sync {
    /// Independently revalidate and project capability atoms for the same
    /// server-owned execution profile used by prepared opens. The default is
    /// fail-closed for test and alternate authorities that install no direct
    /// durable-member bridge executor.
    fn execution_feature_capabilities(
        &self,
    ) -> Result<Vec<&'static str>, ExperimentalLiveOpenAuthorityError> {
        Err(ExperimentalLiveOpenAuthorityError::Unavailable)
    }

    async fn prepare_open(
        &self,
        canonical_session_id: &meerkat_core::SessionId,
        execution_identity: &meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
    ) -> Result<Box<dyn ExperimentalLivePendingOpen>, ExperimentalLiveOpenAuthorityError>;

    /// Read the exact host-catalog profile bound to a current experimental
    /// channel. Replacement signaling uses this before closing the old
    /// transport so profile-owned conversation guidance cannot silently
    /// revert to another profile.
    async fn bound_execution_profile_id(
        &self,
        _channel_id: &meerkat_live::LiveChannelId,
        _canonical_session_id: &meerkat_core::SessionId,
    ) -> Result<String, ExperimentalLiveOpenAuthorityError> {
        Err(ExperimentalLiveOpenAuthorityError::Unavailable)
    }

    /// Exact cleanup for an open that was bound but could not be published,
    /// or for a later channel close. A stale session/channel pair is a no-op.
    async fn unbind_channel(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        canonical_session_id: &meerkat_core::SessionId,
    );

    /// Close the physical provider transport only when this exact authority
    /// owns the supplied binding. Ordinary channels return `NotBound` without
    /// probing a provider transport or exposing an owner map to the surface.
    async fn close_physical_if_bound(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        canonical_session_id: &meerkat_core::SessionId,
    ) -> Result<ExperimentalLivePhysicalClose, ExperimentalLiveOpenAuthorityError>;

    /// Retain one exact generated ambiguity-recovery carrier until the
    /// admitted replacement answer reaches seed acknowledgement. The carrier
    /// is never exposed to a surface or provider.
    async fn register_context_recovery_for_answer(
        &self,
        _recovery: meerkat_runtime::live_execution::LiveContextAmbiguityRecoveryAuthority,
    ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
        Err(ExperimentalLiveOpenAuthorityError::Unavailable)
    }

    /// Retain one exact generated delegation-result ambiguity recovery until
    /// its admitted replacement answer reaches seed acknowledgement. This is
    /// distinct from canonical context-cursor recovery and cannot replay the
    /// ambiguous result.
    async fn register_result_recovery_for_answer(
        &self,
        _recovery: meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
    ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
        Err(ExperimentalLiveOpenAuthorityError::Unavailable)
    }

    /// Optional typed provider control plane retained by the same authority
    /// that prepared and bound the open. Stock compositions expose none.
    fn control_plane(&self) -> Option<Arc<dyn ExperimentalGptLiveControlPlane>> {
        None
    }

    /// Stable, side-effect-free answer binder mechanically derived from this
    /// same open authority. Capability projection must remain absent when the
    /// authority cannot return a complete post-answer binder.
    fn bound_ready_binder_for(
        &self,
        _activator: Arc<dyn ExperimentalLiveBoundChannelActivator>,
        _live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
        _public_observation_publisher: Arc<dyn ExperimentalLivePublicObservationPublisher>,
    ) -> Option<Arc<dyn crate::surface::LiveWebrtcBoundReadyBinder>> {
        None
    }
}

/// Host-owned authorization seam for one exact durable session and selected
/// auth binding.
///
/// Implementations sit beside the authenticated surface and the durable
/// member/session authority. They must reject a session that is not the exact
/// currently authorized durable target. The returned witness is opaque and
/// fences the exact principal, durable target, and binding authorization used
/// by the provider admission below.
pub struct ExperimentalLiveSessionBindingAuthorization {
    binding_use: meerkat_core::AuthBindingUseWitness,
    auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
}

impl ExperimentalLiveSessionBindingAuthorization {
    /// Seal exact binding-use policy together with the generated AuthMachine
    /// lease that owns credential lifecycle for this durable session.
    pub fn from_machine_authority(
        binding_use: meerkat_core::AuthBindingUseWitness,
        auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
    ) -> Self {
        Self {
            binding_use,
            auth_lease,
        }
    }

    fn into_parts(
        self,
    ) -> (
        meerkat_core::AuthBindingUseWitness,
        meerkat_core::handles::GeneratedAuthLeaseHandle,
    ) {
        (self.binding_use, self.auth_lease)
    }
}

#[async_trait]
pub trait ExperimentalLiveSessionBindingAuthority: Send + Sync {
    /// Preflight the exact current durable transcript source before config,
    /// credential, admission, channel, or provider work. Implementations must
    /// delegate to the actor-owned source-availability seam and perform no
    /// effects here. Direct same-member bridge eligibility is a stricter,
    /// separate policy and must not be substituted for this durable-fork
    /// topology check.
    async fn validate_live_durable_source_availability(
        &self,
        _canonical_session_id: &meerkat_core::SessionId,
    ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
        Err(ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable)
    }

    async fn authorize_binding_use(
        &self,
        canonical_session_id: &meerkat_core::SessionId,
        selected_binding: &meerkat_core::AuthBindingRef,
    ) -> Result<ExperimentalLiveSessionBindingAuthorization, ExperimentalLiveOpenAuthorityError>;
}

/// Side-effect-free source of the host's current provider configuration.
///
/// This experimental seam is intentionally independent of the public
/// `openai-realtime` feature. Implementations should return an immutable
/// snapshot for one admission attempt.
#[async_trait]
pub trait ExperimentalLiveCurrentConfigSource: Send + Sync {
    async fn current_config(&self) -> Result<meerkat_core::Config, meerkat_core::ConfigError>;
}

/// Complete host composition for the shipping GPT Live open authority.
pub struct ExperimentalGptLiveOpenAuthorityConfig {
    pub agent_factory: crate::AgentFactory,
    pub config_source: Arc<dyn ExperimentalLiveCurrentConfigSource>,
    pub binding_authority: Arc<dyn ExperimentalLiveSessionBindingAuthority>,
    /// Host-owned fixed execution identity for the registered GPT Live
    /// profile. Callers can select the profile but cannot override any part of
    /// this identity or its configured auth binding.
    pub execution_identity: meerkat_core::SessionLlmIdentity,
    pub realm: meerkat_core::RealmId,
    pub factory_identity: crate::ExperimentalLiveFactoryIdentity,
    pub transport: Arc<ExperimentalGptLiveWebrtcTransport>,
    pub voice: String,
}

/// Concrete Meerkat-owned implementation of the strict experimental open
/// authority.
///
/// The embedding host supplies authenticated durable-session authorization,
/// the fixed execution identity, and its configured auth binding. This owner
/// performs every other step: strict profile selection, current config read,
/// side-effect-free target preparation, exact binding authorization,
/// credential materialization, lower-layer admission, and construction of one
/// opaque pending provider channel.
pub struct ExperimentalGptLiveOpenAuthority {
    agent_factory: crate::AgentFactory,
    config_source: Arc<dyn ExperimentalLiveCurrentConfigSource>,
    binding_authority: Arc<dyn ExperimentalLiveSessionBindingAuthority>,
    execution_identity: meerkat_core::SessionLlmIdentity,
    realm: meerkat_core::RealmId,
    factory_identity: crate::ExperimentalLiveFactoryIdentity,
    transport: Arc<ExperimentalGptLiveWebrtcTransport>,
    voice: String,
    #[cfg(feature = "test-realtime-fixtures")]
    test_endpoints: Option<(String, String)>,
    pending_context_recovery: Arc<
        Mutex<
            HashMap<
                meerkat_live::LiveChannelId,
                meerkat_runtime::live_execution::LiveContextAmbiguityRecoveryAuthority,
            >,
        >,
    >,
    pending_result_recovery: Arc<
        Mutex<
            HashMap<
                meerkat_live::LiveChannelId,
                meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
            >,
        >,
    >,
}

impl ExperimentalGptLiveOpenAuthority {
    pub fn new(
        config: ExperimentalGptLiveOpenAuthorityConfig,
    ) -> Result<Self, ExperimentalGptLiveOpenAuthorityError> {
        if config.voice.trim().is_empty() {
            return Err(ExperimentalGptLiveOpenAuthorityError::MissingVoice);
        }
        if config.execution_identity.provider != meerkat_core::Provider::OpenAI
            || config.execution_identity.model != "gpt-live-1-codex"
            || config.execution_identity.self_hosted_server_id.is_some()
            || config.execution_identity.provider_params.is_some()
            || !matches!(
                config.execution_identity.auth_binding.as_ref(),
                Some(binding)
                    if binding.origin == meerkat_core::BindingOrigin::Configured
                        && binding.realm == config.realm
            )
        {
            return Err(ExperimentalGptLiveOpenAuthorityError::InvalidExecutionIdentity);
        }
        Ok(Self {
            agent_factory: config.agent_factory,
            config_source: config.config_source,
            binding_authority: config.binding_authority,
            execution_identity: config.execution_identity,
            realm: config.realm,
            factory_identity: config.factory_identity,
            transport: config.transport,
            voice: config.voice,
            #[cfg(feature = "test-realtime-fixtures")]
            test_endpoints: None,
            pending_context_recovery: Arc::new(Mutex::new(HashMap::new())),
            pending_result_recovery: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Redirect only the already-admitted provider transport to local test
    /// endpoints. All identity, binding-use, auth-lease, and admission work
    /// remains on the concrete shipping authority path.
    #[cfg(feature = "test-realtime-fixtures")]
    #[doc(hidden)]
    #[must_use]
    pub fn with_test_endpoints(
        mut self,
        call_url: impl Into<String>,
        sideband_base_url: impl Into<String>,
    ) -> Self {
        self.test_endpoints = Some((call_url.into(), sideband_base_url.into()));
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExperimentalGptLiveOpenAuthorityError {
    #[error("experimental GPT Live open authority requires a non-empty voice")]
    MissingVoice,
    #[error("experimental GPT Live open authority requires the canonical host-owned identity")]
    InvalidExecutionIdentity,
}

#[async_trait]
impl ExperimentalLiveOpenAuthorityProvider for ExperimentalGptLiveOpenAuthority {
    fn execution_feature_capabilities(
        &self,
    ) -> Result<Vec<&'static str>, ExperimentalLiveOpenAuthorityError> {
        self.agent_factory
            .experimental_live_execution_feature_capabilities(
                &self.realm,
                &self.factory_identity,
                crate::GPT_LIVE_CLIENT_CONTEXT_PROFILE_ID,
            )
            .map_err(|_| ExperimentalLiveOpenAuthorityError::Unavailable)
    }

    async fn prepare_open(
        &self,
        canonical_session_id: &meerkat_core::SessionId,
        execution_identity: &meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
    ) -> Result<Box<dyn ExperimentalLivePendingOpen>, ExperimentalLiveOpenAuthorityError> {
        self.binding_authority
            .validate_live_durable_source_availability(canonical_session_id)
            .await?;
        let identity = self.execution_identity.clone();
        let config = self
            .config_source
            .current_config()
            .await
            .map_err(|_| ExperimentalLiveOpenAuthorityError::Unavailable)?;
        let preparation = self
            .agent_factory
            .prepare_experimental_live_admission_for_identity(
                &config,
                &self.realm,
                &identity,
                &self.factory_identity,
                &execution_identity.profile_id,
            )
            .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed)?;
        let authorization = self
            .binding_authority
            .authorize_binding_use(canonical_session_id, preparation.auth_binding())
            .await?;
        let (binding_use, auth_lease) = authorization.into_parts();
        let admission = self
            .agent_factory
            .complete_experimental_live_admission(preparation, binding_use, auth_lease)
            .await
            .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed)?;
        #[cfg(feature = "test-realtime-fixtures")]
        let pending = if let Some((call_url, sideband_base_url)) = &self.test_endpoints {
            ExperimentalGptLivePendingChannel::__from_admission_with_test_endpoints(
                &self.agent_factory,
                admission,
                &self.realm,
                &self.factory_identity,
                canonical_session_id.clone(),
                self.voice.clone(),
                call_url,
                sideband_base_url,
            )
        } else {
            ExperimentalGptLivePendingChannel::from_admission(
                &self.agent_factory,
                admission,
                &self.realm,
                &self.factory_identity,
                canonical_session_id.clone(),
                self.voice.clone(),
            )
        }
        .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed)?;
        #[cfg(not(feature = "test-realtime-fixtures"))]
        let pending = ExperimentalGptLivePendingChannel::from_admission(
            &self.agent_factory,
            admission,
            &self.realm,
            &self.factory_identity,
            canonical_session_id.clone(),
            self.voice.clone(),
        )
        .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed)?;
        Ok(Box::new(ExperimentalGptLivePreparedOpen::new(
            pending,
            Arc::clone(&self.transport),
        )))
    }

    async fn unbind_channel(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        canonical_session_id: &meerkat_core::SessionId,
    ) {
        self.pending_context_recovery
            .lock()
            .await
            .remove(channel_id);
        self.pending_result_recovery.lock().await.remove(channel_id);
        self.transport
            .unbind_channel(channel_id, canonical_session_id)
            .await;
    }

    async fn bound_execution_profile_id(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        canonical_session_id: &meerkat_core::SessionId,
    ) -> Result<String, ExperimentalLiveOpenAuthorityError> {
        self.transport
            .bound_execution_profile_id(channel_id, canonical_session_id)
            .await
            .ok_or(ExperimentalLiveOpenAuthorityError::ChannelBindingFailed)
    }

    async fn register_context_recovery_for_answer(
        &self,
        recovery: meerkat_runtime::live_execution::LiveContextAmbiguityRecoveryAuthority,
    ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
        let channel_id = recovery.replacement_channel_id().clone();
        let mut pending = self.pending_context_recovery.lock().await;
        if pending.contains_key(&channel_id) {
            return Err(ExperimentalLiveOpenAuthorityError::ChannelBindingFailed);
        }
        pending.insert(channel_id, recovery);
        Ok(())
    }

    async fn register_result_recovery_for_answer(
        &self,
        recovery: meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
    ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
        let channel_id = recovery.replacement_channel_id().clone();
        let mut pending = self.pending_result_recovery.lock().await;
        if pending.contains_key(&channel_id) {
            return Err(ExperimentalLiveOpenAuthorityError::ChannelBindingFailed);
        }
        pending.insert(channel_id, recovery);
        Ok(())
    }

    async fn close_physical_if_bound(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        canonical_session_id: &meerkat_core::SessionId,
    ) -> Result<ExperimentalLivePhysicalClose, ExperimentalLiveOpenAuthorityError> {
        self.transport
            .close_physical_if_bound(channel_id, canonical_session_id)
            .await
            .map_err(|_| ExperimentalLiveOpenAuthorityError::ChannelBindingFailed)
    }

    fn control_plane(&self) -> Option<Arc<dyn ExperimentalGptLiveControlPlane>> {
        Some(Arc::clone(&self.transport) as Arc<dyn ExperimentalGptLiveControlPlane>)
    }

    fn bound_ready_binder_for(
        &self,
        activator: Arc<dyn ExperimentalLiveBoundChannelActivator>,
        live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
        public_observation_publisher: Arc<dyn ExperimentalLivePublicObservationPublisher>,
    ) -> Option<Arc<dyn crate::surface::LiveWebrtcBoundReadyBinder>> {
        Some(Arc::new(ExperimentalGptLiveBoundReadyBinder {
            transport: Arc::clone(&self.transport),
            activator,
            live_adapter_host,
            public_observation_publisher,
            pending_context_recovery: Arc::clone(&self.pending_context_recovery),
            pending_result_recovery: Arc::clone(&self.pending_result_recovery),
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentalLivePhysicalClose {
    NotBound,
    Closed,
}

#[async_trait]
pub trait ExperimentalGptLiveControlPlane: Send + Sync {
    async fn active_binding(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<ProviderWebrtcBinding>;

    async fn next_observation(
        &self,
        binding: &ProviderWebrtcBinding,
    ) -> Result<Option<ExperimentalGptLiveControlObservation>, ProviderWebrtcBrokerError>;

    async fn append_session_context(
        &self,
        authority: LiveContextAppendAuthority,
        text: String,
    ) -> Result<ExperimentalGptLiveAppendDispatch, ExperimentalGptLiveBridgeError>;

    /// Client-context capability only. Responses function output uses an
    /// independently qualified call-bound settlement path and must never
    /// fall back to this prose context append.
    async fn release_delegation_context(
        &self,
        authority: LiveDelegationResultDeliveryAuthority,
        delegation: LiveSidebandDelegationRef,
        text: String,
    ) -> Result<ExperimentalGptLiveResultDeliveryDispatch, ExperimentalGptLiveBridgeError>;
}

#[async_trait]
pub trait ExperimentalLiveBoundChannelActivator: Send + Sync {
    /// Reserve the exact binding without spawning work or projecting facts.
    async fn prepare_bound_channel(
        &self,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
    ) -> Result<(), String>;

    /// Run the owned control loop after outer answer publication commits.
    async fn run_bound_channel(
        &self,
        binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
    );

    /// Apply one role-bearing provider lifecycle fact in serialized order.
    async fn observe_provider_lifecycle(
        &self,
        observation: &LiveSidebandObservation,
    ) -> Result<(), String>;

    /// Cancel and await the exact prepared/running binding idempotently.
    async fn deactivate_bound_channel(
        &self,
        binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> Result<(), String>;

    /// Retire one externally failed bound channel through its shared physical
    /// and generated semantic close owner. The default is sufficient only for
    /// nonshipping compositions that have no provider transport custody.
    async fn retire_bound_channel_after_pump_exit(
        &self,
        binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    ) -> Result<(), ExperimentalLivePumpRetirementError> {
        self.deactivate_bound_channel(binding)
            .await
            .map_err(ExperimentalLivePumpRetirementError::SemanticUncommitted)
    }

    /// Idempotent provider-neutral replacement bootstrap. It remains visible
    /// until the exact replacement answer activates successfully, so a lost
    /// pull response cannot strand the fresh channel.
    async fn pending_replacement_required(
        &self,
        _session_id: &meerkat_core::SessionId,
    ) -> Option<crate::surface::ExperimentalLiveReplacementRequired> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExperimentalLivePumpRetirementError {
    #[error("experimental live pump exit remains semantically uncommitted: {0}")]
    SemanticUncommitted(String),
}

/// One public-safe ephemeral observation emitted by the bound provider pump.
///
/// The exact provider binding is retained only for surface-side fence
/// validation. The client-visible payload contains no provider turn, response,
/// item, delta, or interaction identity.
#[derive(Clone)]
pub struct ExperimentalLivePublicObservation {
    binding: ProviderWebrtcBinding,
    output: meerkat_live::LiveAssistantOutputAddress,
}

impl ExperimentalLivePublicObservation {
    fn assistant_output_available(
        binding: ProviderWebrtcBinding,
        output: meerkat_live::LiveAssistantOutputAddress,
    ) -> Self {
        Self { binding, output }
    }

    /// Candidate-only projection seam for the non-shipping Gate0 transport.
    /// It carries the same private fence and public-safe output address as the
    /// shipping pump without admitting a provider target or capability.
    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[doc(hidden)]
    pub fn __gate0_harness(
        binding: ProviderWebrtcBinding,
        output: meerkat_live::LiveAssistantOutputAddress,
    ) -> Self {
        Self::assistant_output_available(binding, output)
    }

    #[must_use]
    pub fn binding(&self) -> &ProviderWebrtcBinding {
        &self.binding
    }

    #[must_use]
    pub fn output(&self) -> &meerkat_live::LiveAssistantOutputAddress {
        &self.output
    }

    #[must_use]
    pub fn into_output(self) -> meerkat_live::LiveAssistantOutputAddress {
        self.output
    }
}

impl fmt::Debug for ExperimentalLivePublicObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalLivePublicObservation")
            .field("binding", &"[REDACTED]")
            .field("output", &self.output)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExperimentalLivePublicObservationDeliveryError {
    #[error("experimental live public observation delivery was rejected")]
    Rejected,
    #[error("experimental live public observation delivery channel is closed")]
    Closed,
}

/// Surface-owned publication custody for ephemeral live control events.
/// Implementations return success only after the outer transport has written
/// the sanitized event. Queue closure, shutdown, drop, or write failure must
/// return an error so provider custody can retire the exact binding.
#[async_trait]
pub trait ExperimentalLivePublicObservationPublisher: Send + Sync {
    async fn publish(
        &self,
        observation: ExperimentalLivePublicObservation,
    ) -> Result<(), ExperimentalLivePublicObservationDeliveryError>;
}

struct ExperimentalGptLiveBoundReadyBinder {
    transport: Arc<ExperimentalGptLiveWebrtcTransport>,
    activator: Arc<dyn ExperimentalLiveBoundChannelActivator>,
    live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
    public_observation_publisher: Arc<dyn ExperimentalLivePublicObservationPublisher>,
    pending_context_recovery: Arc<
        Mutex<
            HashMap<
                meerkat_live::LiveChannelId,
                meerkat_runtime::live_execution::LiveContextAmbiguityRecoveryAuthority,
            >,
        >,
    >,
    pending_result_recovery: Arc<
        Mutex<
            HashMap<
                meerkat_live::LiveChannelId,
                meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
            >,
        >,
    >,
}

struct ExperimentalGptLiveBoundReadyCustody {
    runtime: Arc<meerkat_runtime::meerkat_machine::MeerkatMachine>,
    live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
    transport: Arc<ExperimentalGptLiveWebrtcTransport>,
    authority: meerkat_runtime::meerkat_machine::LiveWebrtcAnswerExecutionBindingAuthority,
    activator: Arc<dyn ExperimentalLiveBoundChannelActivator>,
}

#[async_trait]
impl crate::surface::LiveWebrtcBoundReadyBinder for ExperimentalGptLiveBoundReadyBinder {
    async fn bind_answer_ready(
        &self,
        runtime: Arc<meerkat_runtime::meerkat_machine::MeerkatMachine>,
        binding: &LiveWebrtcBindingRequest,
        receipt: meerkat_live::ProviderWebrtcBoundReadyReceipt,
        answer_observation_sequence: u64,
    ) -> Result<
        Box<dyn crate::surface::LiveWebrtcBoundReadyCustody>,
        crate::surface::LiveWebrtcBoundReadyBindFailure,
    > {
        let runtime_binding = binding.runtime_binding.ok_or_else(|| {
            crate::surface::LiveWebrtcBoundReadyBindFailure::before_binding(
                "experimental bound-ready answer omitted its runtime incarnation",
            )
        })?;
        let provider_binding = ProviderWebrtcBinding::new(
            binding.channel_id.clone(),
            binding.session_id.clone(),
            meerkat_live::LiveRuntimeBindingGeneration::new(runtime_binding.generation),
            meerkat_live::LiveRuntimeBindingFence::new(runtime_binding.fence),
        );
        if self
            .transport
            .active_binding(&binding.session_id)
            .await
            .as_ref()
            != Some(&provider_binding)
        {
            return Err(
                crate::surface::LiveWebrtcBoundReadyBindFailure::before_binding(
                    "bound-ready answer does not match the authority's active provider binding",
                ),
            );
        }
        let (context_recovery, result_recovery) = {
            let mut pending_context = self.pending_context_recovery.lock().await;
            let mut pending_result = self.pending_result_recovery.lock().await;
            if pending_context.contains_key(&binding.channel_id)
                && pending_result.contains_key(&binding.channel_id)
            {
                return Err(
                    crate::surface::LiveWebrtcBoundReadyBindFailure::before_binding(
                        "replacement answer has conflicting recovery custody",
                    ),
                );
            }
            (
                pending_context.remove(&binding.channel_id),
                pending_result.remove(&binding.channel_id),
            )
        };
        let authority = match (context_recovery, result_recovery) {
            (Some(recovery), None) => {
                runtime
                    .accept_live_context_recovery_webrtc_answer_and_bind_execution(
                        &provider_binding,
                        &receipt,
                        answer_observation_sequence,
                        &recovery,
                    )
                    .await
            }
            (None, Some(recovery)) => {
                runtime
                    .accept_live_delegation_result_recovery_webrtc_answer_and_bind_execution(
                        &provider_binding,
                        &receipt,
                        answer_observation_sequence,
                        &recovery,
                    )
                    .await
            }
            (None, None) => {
                runtime
                    .accept_live_webrtc_answer_and_bind_execution(
                        &provider_binding,
                        &receipt,
                        answer_observation_sequence,
                    )
                    .await
            }
            (Some(_), Some(_)) => {
                return Err(
                    crate::surface::LiveWebrtcBoundReadyBindFailure::before_binding(
                        "replacement answer has conflicting recovery custody",
                    ),
                );
            }
        }
        .map_err(|error| {
            crate::surface::LiveWebrtcBoundReadyBindFailure::before_binding(error.to_string())
        })?;
        let custody = Box::new(ExperimentalGptLiveBoundReadyCustody {
            runtime,
            live_adapter_host: Arc::clone(&self.live_adapter_host),
            transport: Arc::clone(&self.transport),
            authority,
            activator: Arc::clone(&self.activator),
        });
        if !custody.authority.answer().answered
            || !matches!(
                custody.authority.answer().status,
                meerkat_runtime::meerkat_machine::dsl::LiveWebrtcAnswerPublicStatus::Answered
            )
        {
            return Err(
                crate::surface::LiveWebrtcBoundReadyBindFailure::after_binding(
                    "atomic answer-and-bind authority returned a non-answered state",
                    custody,
                ),
            );
        }
        if let Err(error) = self
            .transport
            .prepare_bound_channel_activation(
                &provider_binding,
                answer_observation_sequence,
                Arc::clone(&custody.runtime),
                custody.authority.binding().clone(),
                Arc::clone(&self.activator),
                Arc::clone(&self.transport) as Arc<dyn ExperimentalGptLiveControlPlane>,
                Arc::clone(&self.live_adapter_host),
                Arc::clone(&self.public_observation_publisher),
            )
            .await
        {
            return Err(
                crate::surface::LiveWebrtcBoundReadyBindFailure::after_binding(error, custody),
            );
        }
        Ok(custody)
    }
}

#[async_trait]
impl crate::surface::LiveWebrtcBoundReadyCustody for ExperimentalGptLiveBoundReadyCustody {
    async fn commit(self: Box<Self>) -> Result<(), String> {
        let binding = self.authority.binding().clone();
        let activated = self
            .transport
            .commit_bound_channel_activation(
                binding.session_id(),
                binding.channel_id(),
                binding.generation(),
                binding.fence_token(),
            )
            .await;
        if !activated {
            return self.rollback().await.and_then(|()| {
                Err("provider activation did not start all bound tasks".to_string())
            });
        }
        let _ = self.authority.commit();
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> Result<(), String> {
        let binding = self.authority.binding().clone();
        let mut errors = Vec::new();
        if let Err(error) = self.activator.deactivate_bound_channel(&binding).await {
            errors.push(format!("bound channel deactivation failed: {error}"));
        }
        let observation = self
            .live_adapter_host
            .reserve_channel_close_observation(binding.channel_id())
            .await;
        match observation {
            Ok(observation) => {
                if let Err(error) = self
                    .live_adapter_host
                    .prepare_channel_physical_close(&observation)
                    .await
                {
                    errors.push(format!("live adapter close failed: {error}"));
                }
                match self
                    .runtime
                    .rollback_live_webrtc_answer_execution_binding(
                        self.authority.into_rollback(),
                        &observation,
                    )
                    .await
                {
                    Ok(authority) => {
                        if let Some(commit) = authority.channel_close_commit_authority() {
                            if let Err(error) = self
                                .live_adapter_host
                                .commit_channel_close_observation(&observation, commit)
                                .await
                            {
                                errors.push(format!("host close commit failed: {error}"));
                            }
                        } else {
                            errors.push(
                                "generated answer binding rollback omitted host close commit authority"
                                    .to_string(),
                            );
                        }
                    }
                    Err(error) => {
                        errors.push(format!("generated answer binding rollback failed: {error}"));
                    }
                }
            }
            Err(error) => {
                errors.push(format!("host close observation failed: {error}"));
            }
        }
        self.runtime
            .retire_live_assistant_output_handles(binding.session_id(), binding.channel_id());
        self.transport
            .retire_after_semantic_rollback(binding.channel_id(), binding.session_id())
            .await;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// Configuration failure before the admitted broker is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExperimentalGptLiveBridgeError {
    #[error("experimental GPT Live requires a non-empty voice")]
    MissingVoice,
    #[error("the resolved target cannot construct the experimental GPT Live broker")]
    TargetRejected,
    #[error("experimental GPT Live custody requires a WebRTC live/open result")]
    NonWebrtcOpen,
    #[error("the live channel already has experimental transport custody")]
    ChannelAlreadyBound,
    #[error("the durable session already has experimental transport custody")]
    SessionAlreadyBound,
    #[error("experimental GPT Live context text must not be empty")]
    EmptyContext,
    #[error("no exact active experimental GPT Live transport binding exists")]
    ActiveBindingUnavailable,
    #[error("generated live context authority rejected the provider projection")]
    ContextAuthorityRejected,
}

/// Typed terminal evidence returned to the semantic owner for generated
/// append resolution. The facade never decides the machine transition.
pub struct ExperimentalGptLiveAppendResolution {
    authority: LiveContextAppendAuthority,
    outcome: meerkat_core::LiveAppendDeliveryOutcome,
}

impl ExperimentalGptLiveAppendResolution {
    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[doc(hidden)]
    #[must_use]
    pub fn __gate0_harness(
        authority: LiveContextAppendAuthority,
        outcome: meerkat_core::LiveAppendDeliveryOutcome,
    ) -> Self {
        Self { authority, outcome }
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        LiveContextAppendAuthority,
        meerkat_core::LiveAppendDeliveryOutcome,
    ) {
        (self.authority, self.outcome)
    }
}

impl fmt::Debug for ExperimentalGptLiveAppendResolution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalGptLiveAppendResolution")
            .field("authority", &"[OPAQUE]")
            .field("outcome", &self.outcome)
            .finish()
    }
}

/// A send either awaits an exact provider acknowledgement or already has a
/// terminal rejected/ambiguous outcome that must be resolved by the machine.
#[derive(Debug)]
pub enum ExperimentalGptLiveAppendDispatch {
    AwaitingAcknowledgement(ExperimentalGptLiveAppendWaiter),
    Resolved(ExperimentalGptLiveAppendResolution),
}

/// Typed terminal provider evidence for one machine-authorized delegation
/// result delivery. This authority is distinct from canonical context append
/// custody and carries no SessionDocument cursor.
pub struct ExperimentalGptLiveResultDeliveryResolution {
    authority: LiveDelegationResultDeliveryAuthority,
    observation: LiveDelegationResultDeliveryObservation,
}

impl ExperimentalGptLiveResultDeliveryResolution {
    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[doc(hidden)]
    #[must_use]
    pub fn __gate0_harness(
        authority: LiveDelegationResultDeliveryAuthority,
        observation: LiveDelegationResultDeliveryObservation,
    ) -> Self {
        Self {
            authority,
            observation,
        }
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        LiveDelegationResultDeliveryAuthority,
        LiveDelegationResultDeliveryObservation,
    ) {
        (self.authority, self.observation)
    }
}

impl fmt::Debug for ExperimentalGptLiveResultDeliveryResolution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalGptLiveResultDeliveryResolution")
            .field("authority", &"[OPAQUE]")
            .field("observation", &self.observation)
            .finish()
    }
}

#[derive(Debug)]
pub enum ExperimentalGptLiveResultDeliveryDispatch {
    AwaitingAcknowledgement(ExperimentalGptLiveResultDeliveryWaiter),
    Resolved(ExperimentalGptLiveResultDeliveryResolution),
}

pub struct ExperimentalGptLiveResultDeliveryWaiter {
    resolution_rx: oneshot::Receiver<ExperimentalGptLiveResultDeliveryResolution>,
}

impl fmt::Debug for ExperimentalGptLiveResultDeliveryWaiter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExperimentalGptLiveResultDeliveryWaiter([OPAQUE])")
    }
}

impl ExperimentalGptLiveResultDeliveryWaiter {
    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[doc(hidden)]
    #[must_use]
    pub fn __gate0_harness(
        resolution_rx: oneshot::Receiver<ExperimentalGptLiveResultDeliveryResolution>,
    ) -> Self {
        Self { resolution_rx }
    }

    pub async fn resolve(
        self,
    ) -> Result<ExperimentalGptLiveResultDeliveryResolution, ExperimentalGptLiveBridgeError> {
        self.resolution_rx
            .await
            .map_err(|_| ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)
    }
}

pub struct ExperimentalGptLiveAppendWaiter {
    resolution_rx: oneshot::Receiver<ExperimentalGptLiveAppendResolution>,
}

impl fmt::Debug for ExperimentalGptLiveAppendWaiter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExperimentalGptLiveAppendWaiter([OPAQUE])")
    }
}

impl ExperimentalGptLiveAppendWaiter {
    #[cfg(feature = "experimental-gpt-live-gate0-harness")]
    #[doc(hidden)]
    #[must_use]
    pub fn __gate0_harness(
        resolution_rx: oneshot::Receiver<ExperimentalGptLiveAppendResolution>,
    ) -> Self {
        Self { resolution_rx }
    }

    pub async fn resolve(
        self,
    ) -> Result<ExperimentalGptLiveAppendResolution, ExperimentalGptLiveBridgeError> {
        self.resolution_rx
            .await
            .map_err(|_| ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)
    }
}

enum PendingExperimentalGptLiveDelivery {
    CanonicalAppend {
        authority: LiveContextAppendAuthority,
        resolution_tx: oneshot::Sender<ExperimentalGptLiveAppendResolution>,
    },
    DelegationResult {
        authority: LiveDelegationResultDeliveryAuthority,
        resolution_tx: oneshot::Sender<ExperimentalGptLiveResultDeliveryResolution>,
    },
}

impl PendingExperimentalGptLiveDelivery {
    fn channel_id(&self) -> &meerkat_live::LiveChannelId {
        match self {
            Self::CanonicalAppend { authority, .. } => authority.channel_id(),
            Self::DelegationResult { authority, .. } => {
                authority.operation().domain_correlation().channel_id()
            }
        }
    }
}

/// Full-duplex control observation. Transcript/turn observations remain
/// provider facts; append observations carry the exact generated authority
/// needed for semantic resolution.
#[derive(Debug)]
pub enum ExperimentalGptLiveControlObservation {
    Provider(LiveSidebandObservation),
    AppendResolved(ExperimentalGptLiveAppendResolution),
    ResultDeliveryResolved(ExperimentalGptLiveResultDeliveryResolution),
}

/// OpenAI-specific broker hidden behind Meerkat's provider-neutral trait.
struct ExperimentalGptLiveWebrtcBroker {
    factory: GptLiveBrokerFactory,
    voice: String,
    execution_mode: meerkat_core::LiveExecutionMode,
    responses: Option<GptLiveResponsesSessionConfig>,
    session_instructions: Option<String>,
    initial_seed: Arc<Mutex<Option<ExperimentalGptLiveInitialSeed>>>,
}

struct ExperimentalGptLiveInitialSeed {
    commentary: Option<String>,
    canonical_seed_cursor: u64,
    _projection_lease: meerkat_core::RealtimeOpenProjectionLease,
}

enum ExperimentalGptLiveSeedCustody {
    Pending(Option<ExperimentalGptLiveInitialSeed>),
    InFlight {
        canonical_seed_cursor: u64,
        task: JoinHandle<Result<(), GptLiveBrokerError>>,
    },
    Ready,
    Failed(ProviderWebrtcBrokerError),
}

struct ExperimentalGptLivePendingBoundReady {
    sideband: Arc<ExperimentalGptLiveSideband>,
}

#[async_trait]
impl ProviderWebrtcPendingBoundReadyResolver for ExperimentalGptLivePendingBoundReady {
    async fn resolve(self: Box<Self>) -> Result<u64, ProviderWebrtcBrokerError> {
        self.sideband.resolve_initial_seed().await
    }
}

#[async_trait]
trait ExperimentalGptLiveBrokerSession: Send + Sync {
    async fn await_ready_and_seed_session_context(
        &self,
        commentary: Option<String>,
    ) -> Result<(), GptLiveBrokerError>;

    async fn append_session_context(
        &self,
        text: String,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError>;

    async fn append_delegation_context(
        &self,
        delegation: &GptLiveDelegationRef,
        text: String,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError>;

    async fn next_observation(
        &self,
    ) -> Result<Option<GptLiveBrokerObservation>, GptLiveBrokerError>;

    async fn close(&self) -> Result<(), GptLiveBrokerError>;
}

#[async_trait]
impl ExperimentalGptLiveBrokerSession for GptLiveBrokerSession {
    async fn await_ready_and_seed_session_context(
        &self,
        commentary: Option<String>,
    ) -> Result<(), GptLiveBrokerError> {
        GptLiveBrokerSession::await_ready_and_seed_session_context(self, commentary).await
    }

    async fn append_session_context(
        &self,
        text: String,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        GptLiveBrokerSession::append_session_context(self, text).await
    }

    async fn append_delegation_context(
        &self,
        delegation: &GptLiveDelegationRef,
        text: String,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        GptLiveBrokerSession::append_delegation_context(self, delegation, text).await
    }

    async fn next_observation(
        &self,
    ) -> Result<Option<GptLiveBrokerObservation>, GptLiveBrokerError> {
        GptLiveBrokerSession::next_observation(self).await
    }

    async fn close(&self) -> Result<(), GptLiveBrokerError> {
        GptLiveBrokerSession::close(self).await
    }
}

impl fmt::Debug for ExperimentalGptLiveWebrtcBroker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalGptLiveWebrtcBroker")
            .field("factory", &"[OPAQUE]")
            .field("voice", &"[REDACTED]")
            .field("execution_mode", &self.execution_mode)
            .field("responses_qualified", &self.responses.is_some())
            .field(
                "session_instructions",
                &self
                    .session_instructions
                    .as_ref()
                    .map(|_| "[CATALOG-BOUND]"),
            )
            .finish()
    }
}

impl ExperimentalGptLiveWebrtcBroker {
    /// Wrap an already admitted provider factory.
    fn new(
        factory: GptLiveBrokerFactory,
        voice: impl Into<String>,
        execution_mode: meerkat_core::LiveExecutionMode,
        session_instructions: Option<String>,
        initial_seed: Arc<Mutex<Option<ExperimentalGptLiveInitialSeed>>>,
    ) -> Result<Self, ExperimentalGptLiveBridgeError> {
        let voice = voice.into();
        if voice.trim().is_empty() {
            return Err(ExperimentalGptLiveBridgeError::MissingVoice);
        }
        Ok(Self {
            factory,
            voice,
            execution_mode,
            // Gate0 has not promoted the exact raw inbound function event or
            // its catalog-bound Responses model. No shipping constructor can
            // populate this field in the unqualified tree.
            responses: None,
            session_instructions,
            initial_seed,
        })
    }

    fn open_config(
        execution_mode: meerkat_core::LiveExecutionMode,
        responses: Option<GptLiveResponsesSessionConfig>,
        offer_sdp: &str,
        voice: &str,
        session_instructions: Option<String>,
    ) -> Result<GptLiveBrokerOpenConfig, ProviderWebrtcBrokerError> {
        let config = GptLiveBrokerOpenConfig::new(offer_sdp, voice).map_err(map_broker_error)?;
        let mut config = match execution_mode {
            meerkat_core::LiveExecutionMode::ClientContext => config.with_client_delegation(),
            meerkat_core::LiveExecutionMode::FunctionBridge => {
                config.with_responses_session(responses.ok_or(ProviderWebrtcBrokerError::Rejected)?)
            }
        };
        if let Some(instructions) = session_instructions {
            config = config.with_session_instructions(instructions);
        }
        Ok(config)
    }
}

#[async_trait]
impl ProviderWebrtcBroker for ExperimentalGptLiveWebrtcBroker {
    async fn answer(
        &self,
        offer: ProviderWebrtcOffer,
    ) -> Result<ProviderWebrtcBrokerAnswer, ProviderWebrtcBrokerError> {
        // Mode selection is sealed by the admitted execution profile.
        // FunctionBridge remains Responses-only and rejects before provider IO
        // while no exact qualified Responses config exists. ClientContext is
        // an independent fixed provider mode and cannot acquire Responses
        // tools through this branch.
        let config = Self::open_config(
            self.execution_mode,
            self.responses.clone(),
            offer.offer_sdp(),
            &self.voice,
            self.session_instructions.clone(),
        )?;
        let binding = offer.binding().clone();
        let seed = self
            .initial_seed
            .lock()
            .await
            .take()
            .ok_or(ProviderWebrtcBrokerError::Rejected)?;
        let bootstrap = self.factory.open(config).await.map_err(map_broker_error)?;
        let (answer_sdp, session) = bootstrap.into_parts();
        let (synthetic_tx, synthetic_rx) = mpsc::channel(8);
        let sideband = Arc::new(ExperimentalGptLiveSideband {
            binding,
            session: Arc::new(session),
            seed_custody: Mutex::new(ExperimentalGptLiveSeedCustody::Pending(Some(seed))),
            seed_changed: Notify::new(),
            correlations: Mutex::new(SidebandCorrelations::default()),
            synthetic_tx,
            synthetic_rx: Mutex::new(synthetic_rx),
        });
        let resolver = Box::new(ExperimentalGptLivePendingBoundReady {
            sideband: Arc::clone(&sideband),
        });
        Ok(offer.into_pending_bound_ready_answer(answer_sdp, sideband, resolver))
    }
}

struct SidebandCommandEnvelope {
    command: LiveSidebandCommand,
    result: oneshot::Sender<Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError>>,
}

struct PreparedExperimentalGptLiveActivation {
    runtime: Arc<meerkat_runtime::meerkat_machine::MeerkatMachine>,
    runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
    activator: Arc<dyn ExperimentalLiveBoundChannelActivator>,
    control: Arc<dyn ExperimentalGptLiveControlPlane>,
    live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
    public_observation_publisher: Arc<dyn ExperimentalLivePublicObservationPublisher>,
}

struct ExperimentalGptLivePumpRetirement {
    activation: Arc<PreparedExperimentalGptLiveActivation>,
    attempt: u32,
}

struct ExperimentalGptLiveActivationGate {
    prepared: Mutex<Option<Arc<PreparedExperimentalGptLiveActivation>>>,
    committed: AtomicBool,
    cancelled: AtomicBool,
    started_tasks: AtomicU64,
    changed: Notify,
    started: Notify,
}

impl ExperimentalGptLiveActivationGate {
    fn new() -> Self {
        Self {
            prepared: Mutex::new(None),
            committed: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            started_tasks: AtomicU64::new(0),
            changed: Notify::new(),
            started: Notify::new(),
        }
    }

    async fn wait_for_commit(&self) -> Option<Arc<PreparedExperimentalGptLiveActivation>> {
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return None;
            }
            if self.committed.load(Ordering::Acquire) {
                let Some(prepared) = self.prepared.lock().await.as_ref().cloned() else {
                    continue;
                };
                return Some(prepared);
            }
            let changed = self.changed.notified();
            if self.cancelled.load(Ordering::Acquire) {
                return None;
            }
            if self.committed.load(Ordering::Acquire) && self.prepared.lock().await.is_some() {
                continue;
            }
            changed.await;
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.changed.notify_waiters();
        self.started.notify_waiters();
    }

    fn mark_started(&self) {
        self.started_tasks.fetch_add(1, Ordering::AcqRel);
        self.started.notify_waiters();
    }

    async fn wait_for_started_tasks(&self, expected: u64) -> bool {
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return false;
            }
            if self.started_tasks.load(Ordering::Acquire) >= expected {
                return true;
            }
            let started = self.started.notified();
            if self.started_tasks.load(Ordering::Acquire) >= expected {
                continue;
            }
            started.await;
        }
    }

    async fn cancelled(&self) {
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            let changed = self.changed.notified();
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            changed.await;
        }
    }
}

struct ActiveExperimentalGptLiveBinding {
    binding: ProviderWebrtcBinding,
    sideband: Arc<dyn ProviderWebrtcSidebandSession>,
    answer_observation_sequence: u64,
    command_tx: mpsc::Sender<SidebandCommandEnvelope>,
    observation_rx: Arc<
        Mutex<mpsc::Receiver<Result<Option<LiveSidebandObservation>, ProviderWebrtcBrokerError>>>,
    >,
    command_actor: JoinHandle<()>,
    observation_actor: JoinHandle<()>,
    control_actor: JoinHandle<()>,
    adapter_pump: JoinHandle<()>,
    activation_gate: Arc<ExperimentalGptLiveActivationGate>,
}

struct RegisteredExperimentalGptLiveChannel {
    session_id: meerkat_core::SessionId,
    broker: Arc<dyn ProviderWebrtcBroker>,
    adapter: Arc<ExperimentalGptLiveDeferredAdapter>,
    identity: meerkat_core::SessionLlmIdentity,
    execution_profile_id: String,
}

/// Opaque one-use provider custody prepared from one exact per-open admission.
/// It has no channel identity until the shared live/open pipeline succeeds.
pub struct ExperimentalGptLivePendingChannel {
    registration: RegisteredExperimentalGptLiveChannel,
    initial_seed: Arc<Mutex<Option<ExperimentalGptLiveInitialSeed>>>,
    adapter_taken: AtomicBool,
    execution_profile: meerkat_runtime::live_execution::LiveExecutionProfileSelection,
}

impl fmt::Debug for ExperimentalGptLivePendingChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalGptLivePendingChannel")
            .field("session_id", &"[REDACTED]")
            .field("broker", &"[OPAQUE]")
            .field("admission", &"[OPAQUE]")
            .finish()
    }
}

impl ExperimentalGptLivePendingChannel {
    /// Consume one exact admitted target into provider custody without opening
    /// any provider transport. Channel binding happens only after shared
    /// live/open succeeds.
    pub fn from_admission(
        admission_owner: &crate::AgentFactory,
        admission: crate::ExperimentalLiveAdmissionWitness,
        realm: &meerkat_core::RealmId,
        factory_identity: &crate::ExperimentalLiveFactoryIdentity,
        canonical_session_id: meerkat_core::SessionId,
        voice: impl Into<String>,
    ) -> Result<Self, ExperimentalGptLiveBridgeError> {
        let session_instructions = admission
            .gpt_live_session_instructions()
            .map(ToString::to_string);
        let execution_profile = admission.execution_profile().clone();
        let target = admission_owner
            .consume_experimental_live_admission(admission, realm, factory_identity)
            .map_err(|_| ExperimentalGptLiveBridgeError::TargetRejected)?;
        let identity = target.identity().clone();
        let provider_factory = GptLiveBrokerFactory::try_from_admitted_target(target)
            .map_err(|_| ExperimentalGptLiveBridgeError::TargetRejected)?;
        let initial_seed = Arc::new(Mutex::new(None));
        let broker = ExperimentalGptLiveWebrtcBroker::new(
            provider_factory,
            voice,
            execution_profile.mode(),
            session_instructions,
            Arc::clone(&initial_seed),
        )?;
        let adapter = Arc::new(ExperimentalGptLiveDeferredAdapter::new(identity.clone()));
        Ok(Self {
            registration: RegisteredExperimentalGptLiveChannel {
                session_id: canonical_session_id,
                broker: Arc::new(broker),
                adapter,
                identity,
                execution_profile_id: execution_profile.profile_id().to_string(),
            },
            initial_seed,
            adapter_taken: AtomicBool::new(false),
            execution_profile,
        })
    }

    /// Consume the same real admission while redirecting only the provider's
    /// private external endpoints to a deterministic local test server.
    #[cfg(feature = "test-realtime-fixtures")]
    #[doc(hidden)]
    #[allow(
        clippy::too_many_arguments,
        reason = "the test-only constructor mirrors the exact admitted provider boundary while replacing only its two endpoints"
    )]
    pub fn __from_admission_with_test_endpoints(
        admission_owner: &crate::AgentFactory,
        admission: crate::ExperimentalLiveAdmissionWitness,
        realm: &meerkat_core::RealmId,
        factory_identity: &crate::ExperimentalLiveFactoryIdentity,
        canonical_session_id: meerkat_core::SessionId,
        voice: impl Into<String>,
        call_url: &str,
        sideband_base_url: &str,
    ) -> Result<Self, ExperimentalGptLiveBridgeError> {
        let session_instructions = admission
            .gpt_live_session_instructions()
            .map(ToString::to_string);
        let execution_profile = admission.execution_profile().clone();
        let target = admission_owner
            .consume_experimental_live_admission(admission, realm, factory_identity)
            .map_err(|_| ExperimentalGptLiveBridgeError::TargetRejected)?;
        let identity = target.identity().clone();
        let provider_factory = GptLiveBrokerFactory::__try_from_admitted_target_with_endpoints(
            target,
            call_url,
            sideband_base_url,
        )
        .map_err(|_| ExperimentalGptLiveBridgeError::TargetRejected)?;
        let initial_seed = Arc::new(Mutex::new(None));
        let broker = ExperimentalGptLiveWebrtcBroker::new(
            provider_factory,
            voice,
            execution_profile.mode(),
            session_instructions,
            Arc::clone(&initial_seed),
        )?;
        let adapter = Arc::new(ExperimentalGptLiveDeferredAdapter::new(identity.clone()));
        Ok(Self {
            registration: RegisteredExperimentalGptLiveChannel {
                session_id: canonical_session_id,
                broker: Arc::new(broker),
                adapter,
                identity,
                execution_profile_id: execution_profile.profile_id().to_string(),
            },
            initial_seed,
            adapter_taken: AtomicBool::new(false),
            execution_profile,
        })
    }

    /// Project the exact admitted execution identity into the canonical
    /// session projection before the shared S5 machine admission. This does
    /// not alter durable session identity.
    pub fn apply_execution_identity(&self, projection: &mut RealtimeSessionOpenProjection) {
        projection.open_config.llm_identity = self.registration.identity.clone();
    }
}

#[async_trait]
impl RealtimeSessionFactory for ExperimentalGptLivePendingChannel {
    fn capabilities(&self) -> RealtimeCapabilities {
        experimental_gpt_live_realtime_capabilities()
    }

    fn supports_provider(&self, provider: Provider) -> bool {
        provider == self.registration.identity.provider
    }

    async fn open_session(
        &self,
        _open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError> {
        Err(experimental_factory_wrong_seam())
    }

    async fn attach_external_session(
        &self,
        _target: &RealtimeExternalSessionTarget,
        _open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError> {
        Err(experimental_factory_wrong_seam())
    }

    async fn open_live_adapter(
        &self,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Arc<dyn LiveAdapter>, LlmError> {
        if open_config.llm_identity != self.registration.identity {
            return Err(LlmError::InvalidConfig {
                message: "experimental live projection identity does not match admission"
                    .to_string(),
            });
        }
        self.adapter_taken
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| LlmError::InvalidRequest {
                message: "experimental live pending adapter was already consumed".to_string(),
            })?;
        let projection_lease =
            open_config
                .take_open_projection_lease()
                .ok_or_else(|| LlmError::InvalidRequest {
                    message: "experimental live canonical seed custody was already consumed"
                        .to_string(),
                })?;
        let commentary_messages = open_config
            .seed_messages()
            .iter()
            .filter(|message| {
                !matches!(
                    message,
                    meerkat_core::types::Message::System(_)
                        | meerkat_core::types::Message::SystemNotice(_)
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        // The live endpoint is a tool-less channel embodiment. Executor
        // system messages and notices may describe tool and callback
        // authority, so they must not be copied into the provider voice
        // session. Stable voice behavior belongs to the catalog-owned live
        // profile instructions; only canonical conversation messages cross
        // this context seam.
        let commentary = (!commentary_messages.is_empty())
            .then(|| {
                serde_json::to_string(&serde_json::json!({
                    "canonical_messages": commentary_messages,
                }))
            })
            .transpose()
            .map_err(|error| LlmError::InvalidConfig {
                message: format!("failed to encode canonical live seed: {error}"),
            })?;
        let seed = ExperimentalGptLiveInitialSeed {
            commentary,
            canonical_seed_cursor: open_config.canonical_message_cursor(),
            _projection_lease: projection_lease,
        };
        let mut slot = self.initial_seed.lock().await;
        if slot.is_some() {
            return Err(LlmError::InvalidRequest {
                message: "experimental live canonical seed was already staged".to_string(),
            });
        }
        *slot = Some(seed);
        Ok(Arc::clone(&self.registration.adapter) as Arc<dyn LiveAdapter>)
    }
}

fn experimental_factory_wrong_seam() -> LlmError {
    LlmError::InvalidRequest {
        message:
            "experimental GPT Live is available only through the admitted WebRTC live/open seam"
                .to_string(),
    }
}

fn require_context_text(text: impl Into<String>) -> Result<String, ExperimentalGptLiveBridgeError> {
    let text = text.into();
    (!text.trim().is_empty())
        .then_some(text)
        .ok_or(ExperimentalGptLiveBridgeError::EmptyContext)
}

fn experimental_gpt_live_realtime_capabilities() -> RealtimeCapabilities {
    RealtimeCapabilities {
        input_kinds: vec![RealtimeInputKind::Audio],
        output_kinds: vec![RealtimeOutputKind::Audio],
        turning_modes: vec![RealtimeTurningMode::ProviderManaged],
        interrupt_supported: true,
        transcript_supported: true,
        tool_lifecycle_events_supported: false,
        video_supported: false,
        audio_input_format: Some(RealtimeAudioFormat::pcm(24_000, 1)),
        audio_output_format: Some(RealtimeAudioFormat::pcm(24_000, 1)),
    }
}

struct ExperimentalGptLiveDeferredAdapter {
    identity: meerkat_core::SessionLlmIdentity,
    status: std::sync::Mutex<LiveAdapterStatus>,
    // The provider reader must never await the transcript consumer before it
    // can route a control observation. The adapter owns this in-process queue
    // and drains it under the live host; control remains on its independent,
    // bounded lane below.
    observation_tx: mpsc::UnboundedSender<LiveSidebandObservation>,
    observation_rx: Mutex<mpsc::UnboundedReceiver<LiveSidebandObservation>>,
    pending_local_observations: std::sync::Mutex<VecDeque<LiveAdapterObservation>>,
    pending_local_notify: Notify,
    playback_by_item: std::sync::Mutex<HashMap<String, PendingExperimentalGptLivePlayback>>,
    closed: AtomicBool,
    closed_notify: Notify,
}

struct PendingExperimentalGptLivePlayback {
    provider_turn_ref: String,
    response_id: String,
    stop_reason: StopReason,
    usage: TurnUsage,
    final_forwarded: bool,
    terminal_forwarded: bool,
}

impl ExperimentalGptLiveDeferredAdapter {
    fn new(identity: meerkat_core::SessionLlmIdentity) -> Self {
        let (observation_tx, observation_rx) = mpsc::unbounded_channel();
        Self {
            identity,
            status: std::sync::Mutex::new(LiveAdapterStatus::Opening),
            observation_tx,
            observation_rx: Mutex::new(observation_rx),
            pending_local_observations: std::sync::Mutex::new(VecDeque::new()),
            pending_local_notify: Notify::new(),
            playback_by_item: std::sync::Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            closed_notify: Notify::new(),
        }
    }

    fn push_observation(
        &self,
        observation: LiveSidebandObservation,
    ) -> Result<(), ProviderWebrtcBrokerError> {
        self.observation_tx
            .send(observation)
            .map_err(|_| ProviderWebrtcBrokerError::Unavailable)
    }

    fn replace_status(&self, status: LiveAdapterStatus) {
        if let Ok(mut current) = self.status.lock() {
            *current = status;
        }
    }

    fn current_status(&self) -> LiveAdapterStatus {
        self.status
            .lock()
            .map(|current| current.clone())
            .unwrap_or(LiveAdapterStatus::Closed)
    }

    fn close_stream(&self) {
        self.closed.store(true, Ordering::Release);
        if let Ok(mut playback) = self.playback_by_item.lock() {
            playback.clear();
        }
        self.replace_status(LiveAdapterStatus::Closed);
        self.closed_notify.notify_waiters();
    }

    fn queue_local_observation(&self, observation: LiveAdapterObservation) {
        if let Ok(mut pending) = self.pending_local_observations.lock() {
            pending.push_back(observation);
            self.pending_local_notify.notify_one();
        }
    }

    fn local_response_id(turn: &LiveSidebandTurnRef) -> String {
        format!("experimental-gpt-live-response:{}", turn.adapter_key())
    }

    fn local_item_id(turn: &LiveSidebandTurnRef) -> String {
        format!("experimental-gpt-live-item:{}", turn.adapter_key())
    }

    fn queue_playback_terminal(
        &self,
        item_id: String,
        playback: &PendingExperimentalGptLivePlayback,
        interaction_id: meerkat_core::InteractionId,
        content_index: u32,
        evidence: meerkat_core::LiveAssistantPlaybackEvidence,
    ) {
        self.queue_local_observation(LiveAdapterObservation::AssistantPlaybackTerminalObserved {
            interaction_id,
            provider_item_id: item_id,
            content_index,
            response_id: playback.response_id.clone(),
            evidence,
            stop_reason: playback.stop_reason,
            usage: playback.usage.clone(),
        });
    }

    fn lower_observation(
        &self,
        observation: LiveSidebandObservation,
    ) -> Option<LiveAdapterObservation> {
        match observation.into_kind() {
            LiveSidebandObservationKind::SessionReady => {
                self.replace_status(LiveAdapterStatus::Ready);
                Some(LiveAdapterObservation::Ready)
            }
            LiveSidebandObservationKind::TurnStarted {
                turn,
                role: LiveSidebandTurnRole::Assistant,
            } => {
                let response_id = Self::local_response_id(&turn);
                let item_id = Self::local_item_id(&turn);
                let provider_turn_ref = turn.adapter_key().to_string();
                let pending = PendingExperimentalGptLivePlayback {
                    provider_turn_ref: provider_turn_ref.clone(),
                    response_id: response_id.clone(),
                    stop_reason: StopReason::EndTurn,
                    usage: TurnUsage::host_declared(
                        self.identity.provider,
                        self.identity.model.clone(),
                        Usage::default(),
                    ),
                    final_forwarded: false,
                    terminal_forwarded: false,
                };
                let inserted = self
                    .playback_by_item
                    .lock()
                    .ok()
                    .and_then(|mut playback| playback.insert(item_id.clone(), pending))
                    .is_none();
                if !inserted {
                    return Some(LiveAdapterObservation::Error {
                        code: LiveAdapterErrorCode::ProviderError,
                        message: "experimental GPT Live duplicated an assistant output start"
                            .to_string(),
                    });
                }
                Some(LiveAdapterObservation::AssistantOutputStarted {
                    provider_turn_ref,
                    response_id,
                    provider_item_id: item_id,
                    content_index: 0,
                })
            }
            LiveSidebandObservationKind::TurnFinished {
                turn,
                role,
                transcript,
            } => match role {
                LiveSidebandTurnRole::User => Some(LiveAdapterObservation::UserTranscriptFinal {
                    provider_item_id: Some(format!(
                        "experimental-gpt-live-user-item:{}",
                        turn.adapter_key()
                    )),
                    previous_item_id: None,
                    content_index: Some(0),
                    text: transcript,
                }),
                LiveSidebandTurnRole::Assistant => {
                    let response_id = Self::local_response_id(&turn);
                    let item_id = Self::local_item_id(&turn);
                    let final_observed =
                        self.playback_by_item.lock().ok().and_then(|mut playback| {
                            let pending = playback.get_mut(&item_id)?;
                            if pending.provider_turn_ref != turn.adapter_key()
                                || pending.response_id != response_id
                                || pending.final_forwarded
                            {
                                return None;
                            }
                            pending.final_forwarded = true;
                            let terminal_forwarded = pending.terminal_forwarded;
                            if terminal_forwarded {
                                playback.remove(&item_id);
                            }
                            Some(())
                        });
                    let Some(()) = final_observed else {
                        return Some(LiveAdapterObservation::Error {
                            code: LiveAdapterErrorCode::ProviderError,
                            message: "experimental GPT Live assistant final has no exact started output handle"
                                .to_string(),
                        });
                    };
                    Some(LiveAdapterObservation::AssistantTranscriptFinal {
                        provider_item_id: item_id,
                        previous_item_id: None,
                        content_index: Some(0),
                        response_id: Some(response_id),
                        text: transcript,
                        stop_reason: StopReason::EndTurn,
                        usage: Usage::default(),
                    })
                }
                LiveSidebandTurnRole::Unknown => None,
            },
            LiveSidebandObservationKind::UnsupportedProviderEvent
            | LiveSidebandObservationKind::DelegationActionableInputUnsupported { .. } => {
                Some(LiveAdapterObservation::Error {
                    code: LiveAdapterErrorCode::ProviderError,
                    message: "experimental GPT Live emitted an unsupported actionable event"
                        .to_string(),
                })
            }
            LiveSidebandObservationKind::UserTranscriptFragment { .. }
            | LiveSidebandObservationKind::AssistantTranscriptFragment { .. }
            | LiveSidebandObservationKind::TurnStarted { .. }
            | LiveSidebandObservationKind::TurnSnapshotDelta { .. }
            | LiveSidebandObservationKind::DelegationRequested { .. }
            | LiveSidebandObservationKind::AppendAcknowledged { .. }
            | LiveSidebandObservationKind::AppendDeliveryAmbiguousTerminal { .. } => None,
        }
    }
}

#[async_trait]
impl LiveAdapter for ExperimentalGptLiveDeferredAdapter {
    async fn send_command(&self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
        match command {
            LiveAdapterCommand::Close => self.close().await,
            LiveAdapterCommand::TruncateAssistantOutput {
                interaction_id,
                item_id,
                content_index,
                audio_played_ms: _,
                reported_playback_prefix,
            } => {
                if content_index != 0 {
                    return Err(LiveAdapterError::ProviderError {
                        code: LiveAdapterErrorCode::InternalError,
                        message: "experimental GPT Live truncation content index is not bound"
                            .to_string(),
                    });
                }
                let mut pending_by_item =
                    self.playback_by_item
                        .lock()
                        .map_err(|_| LiveAdapterError::ProviderError {
                            code: LiveAdapterErrorCode::InternalError,
                            message: "experimental GPT Live playback custody is unavailable"
                                .to_string(),
                        })?;
                let pending = pending_by_item.get_mut(&item_id).ok_or_else(|| {
                    LiveAdapterError::ProviderError {
                        code: LiveAdapterErrorCode::InternalError,
                        message:
                            "experimental GPT Live truncation has no exact local response binding"
                                .to_string(),
                    }
                })?;
                if pending.terminal_forwarded {
                    return Err(LiveAdapterError::ProviderError {
                        code: LiveAdapterErrorCode::InternalError,
                        message: "experimental GPT Live playback terminal is already retained"
                            .to_string(),
                    });
                }
                pending.terminal_forwarded = true;
                let evidence = reported_playback_prefix.map_or(
                    meerkat_core::LiveAssistantPlaybackEvidence::Unmeasured,
                    meerkat_core::LiveAssistantPlaybackEvidence::ReportedPrefix,
                );
                let final_forwarded = pending.final_forwarded;
                self.queue_playback_terminal(
                    item_id.clone(),
                    pending,
                    interaction_id,
                    content_index,
                    evidence,
                );
                if final_forwarded {
                    pending_by_item.remove(&item_id);
                }
                drop(pending_by_item);
                // The browser WebRTC peer owns playback and provider-native
                // barge-in. This command carries only its playback report; no
                // unsupported private sideband truncate event is invented.
                Ok(())
            }
            LiveAdapterCommand::CompleteAssistantPlayback {
                interaction_id,
                item_id,
                content_index,
            } => {
                if content_index != 0 {
                    return Err(LiveAdapterError::ProviderError {
                        code: LiveAdapterErrorCode::InternalError,
                        message:
                            "experimental GPT Live playback completion content index is not bound"
                                .to_string(),
                    });
                }
                let mut pending_by_item =
                    self.playback_by_item
                        .lock()
                        .map_err(|_| LiveAdapterError::ProviderError {
                            code: LiveAdapterErrorCode::InternalError,
                            message: "experimental GPT Live playback custody is unavailable"
                                .to_string(),
                        })?;
                let pending = pending_by_item.get_mut(&item_id).ok_or_else(|| {
                    LiveAdapterError::ProviderError {
                        code: LiveAdapterErrorCode::InternalError,
                        message: "experimental GPT Live playback completion has no exact local response binding"
                            .to_string(),
                    }
                })?;
                if pending.terminal_forwarded {
                    return Err(LiveAdapterError::ProviderError {
                        code: LiveAdapterErrorCode::InternalError,
                        message: "experimental GPT Live playback terminal is already retained"
                            .to_string(),
                    });
                }
                pending.terminal_forwarded = true;
                let final_forwarded = pending.final_forwarded;
                self.queue_playback_terminal(
                    item_id.clone(),
                    pending,
                    interaction_id,
                    content_index,
                    meerkat_core::LiveAssistantPlaybackEvidence::PlaybackComplete,
                );
                if final_forwarded {
                    pending_by_item.remove(&item_id);
                }
                drop(pending_by_item);
                Ok(())
            }
            _ => Err(LiveAdapterError::NotReady {
                status: self.current_status(),
            }),
        }
    }

    async fn next_observation(&self) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
        loop {
            if let Some(observation) = self
                .pending_local_observations
                .lock()
                .ok()
                .and_then(|mut pending| pending.pop_front())
            {
                return Ok(Some(observation));
            }
            let observation = {
                let mut receiver = self.observation_rx.lock().await;
                if self.closed.load(Ordering::Acquire) {
                    receiver.try_recv().ok()
                } else {
                    tokio::select! {
                        biased;
                        observation = receiver.recv() => observation,
                        () = self.pending_local_notify.notified() => continue,
                        () = self.closed_notify.notified() => receiver.try_recv().ok(),
                    }
                }
            };
            let Some(observation) = observation else {
                self.close_stream();
                return Ok(None);
            };
            if let Some(lowered) = self.lower_observation(observation) {
                return Ok(Some(lowered));
            }
        }
    }

    fn status(&self) -> LiveAdapterStatus {
        self.current_status()
    }

    async fn close(&self) -> Result<(), LiveAdapterError> {
        self.close_stream();
        Ok(())
    }

    fn capabilities(&self) -> LiveChannelCapabilities {
        LiveChannelCapabilities {
            audio_in: true,
            audio_out: true,
            text_in: false,
            text_out: false,
            image_in: false,
            video_in: false,
            transcript_supported: true,
            barge_in_supported: true,
            provider_native_resume: false,
        }
    }
}

/// Nonshipping Gate0 feeder for exercising the exact shipping adapter
/// projection and playback-custody mechanics with candidate observations.
#[cfg(feature = "experimental-gpt-live-gate0-harness")]
#[doc(hidden)]
pub struct ExperimentalGptLiveGate0AdapterFeeder {
    adapter: Arc<ExperimentalGptLiveDeferredAdapter>,
}

#[cfg(feature = "experimental-gpt-live-gate0-harness")]
impl ExperimentalGptLiveGate0AdapterFeeder {
    #[must_use]
    pub fn __new(identity: meerkat_core::SessionLlmIdentity) -> Self {
        Self {
            adapter: Arc::new(ExperimentalGptLiveDeferredAdapter::new(identity)),
        }
    }

    #[must_use]
    pub fn __adapter(&self) -> Arc<dyn LiveAdapter> {
        Arc::clone(&self.adapter) as Arc<dyn LiveAdapter>
    }

    pub fn __push(
        &self,
        observation: LiveSidebandObservation,
    ) -> Result<(), ProviderWebrtcBrokerError> {
        self.adapter.push_observation(observation)
    }

    pub fn __close(&self) {
        self.adapter.close_stream();
    }
}

/// Concrete pending-open binder for the facade-owned GPT Live multiplexer.
pub struct ExperimentalGptLivePreparedOpen {
    pending: ExperimentalGptLivePendingChannel,
    transport: Arc<ExperimentalGptLiveWebrtcTransport>,
}

impl ExperimentalGptLivePreparedOpen {
    #[must_use]
    pub fn new(
        pending: ExperimentalGptLivePendingChannel,
        transport: Arc<ExperimentalGptLiveWebrtcTransport>,
    ) -> Self {
        Self { pending, transport }
    }
}

#[async_trait]
impl ExperimentalLivePendingOpen for ExperimentalGptLivePreparedOpen {
    fn apply_execution_identity(&self, projection: &mut RealtimeSessionOpenProjection) {
        self.pending.apply_execution_identity(projection);
    }

    fn session_factory(&self) -> &dyn RealtimeSessionFactory {
        &self.pending
    }

    fn execution_profile(&self) -> &meerkat_runtime::live_execution::LiveExecutionProfileSelection {
        &self.pending.execution_profile
    }

    async fn bind_opened(
        self: Box<Self>,
        opened: &LiveOpenResult,
    ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
        self.transport
            .bind_opened_channel(self.pending, opened)
            .await
            .map_err(|_| ExperimentalLiveOpenAuthorityError::ChannelBindingFailed)
    }
}

/// Facade-owned physical custody for one admitted experimental GPT Live
/// transport per durable session.
///
/// Answer materialization consumes the opaque generated machine admission in
/// [`LiveWebrtcAdmittedOffer`] before the private broker is called. Each active
/// sideband receives independent command and observation actors so provider
/// output remains full duplex while exact generation/fence cleanup is retained
/// here.
pub struct ExperimentalGptLiveWebrtcTransport {
    operations: Mutex<()>,
    registered_by_channel:
        Arc<Mutex<HashMap<meerkat_live::LiveChannelId, RegisteredExperimentalGptLiveChannel>>>,
    active_by_session:
        Arc<Mutex<HashMap<meerkat_core::SessionId, ActiveExperimentalGptLiveBinding>>>,
    pending_deliveries:
        Arc<Mutex<HashMap<LiveSidebandAppendAttempt, PendingExperimentalGptLiveDelivery>>>,
    answer_observation_sequence: AtomicU64,
    pump_retirement_tx: Mutex<Option<mpsc::Sender<ExperimentalGptLivePumpRetirement>>>,
    pump_retirement_actor: Mutex<Option<JoinHandle<()>>>,
    pending_pump_retirements: Arc<
        Mutex<
            HashMap<
                (meerkat_core::SessionId, meerkat_live::LiveChannelId),
                Arc<PreparedExperimentalGptLiveActivation>,
            >,
        >,
    >,
}

impl fmt::Debug for ExperimentalGptLiveWebrtcTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalGptLiveWebrtcTransport")
            .field("registered_by_channel", &"[REDACTED]")
            .field("active_by_session", &"[REDACTED]")
            .finish()
    }
}

impl Default for ExperimentalGptLiveWebrtcTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ExperimentalGptLiveWebrtcTransport {
    fn drop(&mut self) {
        if let Some(actor) = self.pump_retirement_actor.get_mut().take() {
            actor.abort();
        }
    }
}

impl ExperimentalGptLiveWebrtcTransport {
    #[must_use]
    pub fn new() -> Self {
        Self {
            operations: Mutex::new(()),
            registered_by_channel: Arc::new(Mutex::new(HashMap::new())),
            active_by_session: Arc::new(Mutex::new(HashMap::new())),
            pending_deliveries: Arc::new(Mutex::new(HashMap::new())),
            answer_observation_sequence: AtomicU64::new(0),
            pump_retirement_tx: Mutex::new(None),
            pump_retirement_actor: Mutex::new(None),
            pending_pump_retirements: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Bind provider custody only after the shared live/open pipeline has
    /// produced the channel that will be published to the caller.
    pub async fn bind_opened_channel(
        &self,
        pending: ExperimentalGptLivePendingChannel,
        opened: &LiveOpenResult,
    ) -> Result<(), ExperimentalGptLiveBridgeError> {
        if !matches!(&opened.transport, WireLiveTransportBootstrap::Webrtc { .. }) {
            return Err(ExperimentalGptLiveBridgeError::NonWebrtcOpen);
        }
        let _operation = self.operations.lock().await;
        let channel_id = meerkat_live::LiveChannelId::new(&opened.channel_id);
        let mut registrations = self.registered_by_channel.lock().await;
        if registrations.contains_key(&channel_id) {
            return Err(ExperimentalGptLiveBridgeError::ChannelAlreadyBound);
        }
        if registrations
            .values()
            .any(|entry| entry.session_id == pending.registration.session_id)
        {
            return Err(ExperimentalGptLiveBridgeError::SessionAlreadyBound);
        }
        registrations.insert(channel_id, pending.registration);
        Ok(())
    }

    async fn bound_execution_profile_id(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        session_id: &meerkat_core::SessionId,
    ) -> Option<String> {
        self.registered_by_channel
            .lock()
            .await
            .get(channel_id)
            .filter(|registration| registration.session_id == *session_id)
            .map(|registration| registration.execution_profile_id.clone())
    }

    /// Remove exact provider custody during open rollback or after physical
    /// close. A stale session cannot unbind another channel.
    pub async fn unbind_channel(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        session_id: &meerkat_core::SessionId,
    ) -> bool {
        let _operation = self.operations.lock().await;
        let active = {
            let mut active = self.active_by_session.lock().await;
            active
                .get(session_id)
                .is_some_and(|current| current.binding.channel_id() == channel_id)
                .then(|| active.remove(session_id))
                .flatten()
        };
        if let Some(active) = active {
            let _ = active.sideband.close().await;
            retire_sideband_actors(active).await;
        }
        retire_pending_deliveries(self.pending_deliveries.as_ref(), channel_id).await;
        self.pending_pump_retirements
            .lock()
            .await
            .remove(&(session_id.clone(), channel_id.clone()));
        self.unbind_channel_locked(channel_id, session_id).await
    }

    /// Claim and close physical custody only for an exact registered
    /// experimental binding. The registration remains until semantic close
    /// succeeds and the authority provider performs `unbind_channel`.
    pub async fn close_physical_if_bound(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        session_id: &meerkat_core::SessionId,
    ) -> Result<ExperimentalLivePhysicalClose, LiveWebrtcError> {
        let owns_binding = self
            .registered_by_channel
            .lock()
            .await
            .get(channel_id)
            .is_some_and(|registration| registration.session_id == *session_id);
        if !owns_binding {
            return Ok(ExperimentalLivePhysicalClose::NotBound);
        }
        let provider_binding = self
            .active_by_session
            .lock()
            .await
            .get(session_id)
            .filter(|active| active.binding.channel_id() == channel_id)
            .map(|active| active.binding.clone());
        if let Some(provider_binding) = provider_binding {
            self.close_exact(&provider_binding, None)
                .await
                .map_err(provider_signaling_error)?;
        }
        Ok(ExperimentalLivePhysicalClose::Closed)
    }

    async fn unbind_channel_locked(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        session_id: &meerkat_core::SessionId,
    ) -> bool {
        let mut registrations = self.registered_by_channel.lock().await;
        let matches = registrations
            .get(channel_id)
            .is_some_and(|entry| entry.session_id == *session_id);
        matches
            .then(|| registrations.remove(channel_id))
            .flatten()
            .is_some()
    }

    /// Drop all mechanical custody after generated semantic rollback has
    /// committed. This deliberately does not require provider close success:
    /// publication rejection must never leave a selectable active binding.
    async fn retire_after_semantic_rollback(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        session_id: &meerkat_core::SessionId,
    ) {
        let _operation = self.operations.lock().await;
        let active = {
            let mut active = self.active_by_session.lock().await;
            active
                .get(session_id)
                .is_some_and(|current| current.binding.channel_id() == channel_id)
                .then(|| active.remove(session_id))
                .flatten()
        };
        if let Some(active) = active {
            retire_sideband_actors(active).await;
        }
        let _ = self.unbind_channel_locked(channel_id, session_id).await;
    }

    /// Send one already machine-authorized command to the exact current
    /// binding. The command actor and observation actor remain independent.
    async fn send_authorized_command(
        &self,
        command: LiveSidebandCommand,
    ) -> Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError> {
        let binding = command.binding().clone();
        let sender = {
            let active = self.active_by_session.lock().await;
            let current = active
                .get(binding.session_id())
                .filter(|current| current.binding == binding)
                .ok_or(ProviderWebrtcBrokerError::Rejected)?;
            current.command_tx.clone()
        };
        let (result_tx, result_rx) = oneshot::channel();
        sender
            .send(SidebandCommandEnvelope {
                command,
                result: result_tx,
            })
            .await
            .map_err(|_| ProviderWebrtcBrokerError::Unavailable)?;
        result_rx
            .await
            .map_err(|_| ProviderWebrtcBrokerError::Unavailable)?
    }

    async fn current_binding_for_append(
        &self,
        authority: &LiveContextAppendAuthority,
    ) -> Result<ProviderWebrtcBinding, ExperimentalGptLiveBridgeError> {
        self.active_binding(authority.session_id())
            .await
            .filter(|binding| binding.channel_id() == authority.channel_id())
            .ok_or(ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)
    }

    /// Append canonical session context under one generated pre-send edge.
    pub async fn append_session_context(
        &self,
        authority: LiveContextAppendAuthority,
        text: impl Into<String>,
    ) -> Result<ExperimentalGptLiveAppendDispatch, ExperimentalGptLiveBridgeError> {
        let text = require_context_text(text)?;
        let binding = self.current_binding_for_append(&authority).await?;
        let (authority, sideband) = authority
            .into_sideband_append_authority(binding)
            .map_err(|_| ExperimentalGptLiveBridgeError::ContextAuthorityRejected)?;
        let command = LiveSidebandCommand::append_session_context(sideband, text)
            .map_err(|_| ExperimentalGptLiveBridgeError::ContextAuthorityRejected)?;
        self.dispatch_generated_append(authority, command).await
    }

    /// Deliver executor result context under its distinct one-use generated
    /// authority. This carries no canonical context cursor and never marks
    /// exact text for speech - the live model decides whether and how to
    /// respond from the appended context.
    pub async fn release_delegation_context(
        &self,
        authority: LiveDelegationResultDeliveryAuthority,
        delegation: LiveSidebandDelegationRef,
        text: impl Into<String>,
    ) -> Result<ExperimentalGptLiveResultDeliveryDispatch, ExperimentalGptLiveBridgeError> {
        let text = require_context_text(text)?;
        let binding = self
            .active_binding(authority.session_id())
            .await
            .filter(|binding| {
                binding.channel_id() == authority.operation().domain_correlation().channel_id()
            })
            .ok_or(ExperimentalGptLiveBridgeError::ActiveBindingUnavailable)?;
        let (authority, sideband) = authority
            .into_sideband_release_authority(binding, &delegation, &text)
            .map_err(|_| ExperimentalGptLiveBridgeError::ContextAuthorityRejected)?;
        let command = LiveSidebandCommand::release_delegation_context(sideband, delegation, text)
            .map_err(|_| ExperimentalGptLiveBridgeError::ContextAuthorityRejected)?;
        self.dispatch_delegation_result(authority, command).await
    }

    async fn dispatch_generated_append(
        &self,
        authority: LiveContextAppendAuthority,
        command: LiveSidebandCommand,
    ) -> Result<ExperimentalGptLiveAppendDispatch, ExperimentalGptLiveBridgeError> {
        let attempt = command.attempt();
        let (resolution_tx, resolution_rx) = oneshot::channel();
        self.pending_deliveries.lock().await.insert(
            attempt.clone(),
            PendingExperimentalGptLiveDelivery::CanonicalAppend {
                authority,
                resolution_tx,
            },
        );
        let terminal = match self.send_authorized_command(command).await {
            Ok(LiveSidebandCommandDelivery::Accepted) => {
                return Ok(ExperimentalGptLiveAppendDispatch::AwaitingAcknowledgement(
                    ExperimentalGptLiveAppendWaiter { resolution_rx },
                ));
            }
            Ok(LiveSidebandCommandDelivery::AmbiguousTerminal) => {
                meerkat_core::LiveAppendDeliveryOutcome::Ambiguous
            }
            Err(ProviderWebrtcBrokerError::Rejected) => {
                meerkat_core::LiveAppendDeliveryOutcome::Rejected
            }
            Err(
                ProviderWebrtcBrokerError::Unavailable | ProviderWebrtcBrokerError::ProtocolDrift,
            ) => meerkat_core::LiveAppendDeliveryOutcome::Ambiguous,
            Err(_) => meerkat_core::LiveAppendDeliveryOutcome::Ambiguous,
        };
        let pending = self
            .pending_deliveries
            .lock()
            .await
            .remove(&attempt)
            .ok_or(ExperimentalGptLiveBridgeError::ContextAuthorityRejected)?;
        let PendingExperimentalGptLiveDelivery::CanonicalAppend {
            authority,
            resolution_tx,
        } = pending
        else {
            return Err(ExperimentalGptLiveBridgeError::ContextAuthorityRejected);
        };
        drop(resolution_tx);
        Ok(ExperimentalGptLiveAppendDispatch::Resolved(
            ExperimentalGptLiveAppendResolution {
                authority,
                outcome: terminal,
            },
        ))
    }

    async fn dispatch_delegation_result(
        &self,
        authority: LiveDelegationResultDeliveryAuthority,
        command: LiveSidebandCommand,
    ) -> Result<ExperimentalGptLiveResultDeliveryDispatch, ExperimentalGptLiveBridgeError> {
        let attempt = command.attempt();
        let (resolution_tx, resolution_rx) = oneshot::channel();
        self.pending_deliveries.lock().await.insert(
            attempt.clone(),
            PendingExperimentalGptLiveDelivery::DelegationResult {
                authority,
                resolution_tx,
            },
        );
        let observation = match self.send_authorized_command(command).await {
            Ok(LiveSidebandCommandDelivery::Accepted) => {
                return Ok(
                    ExperimentalGptLiveResultDeliveryDispatch::AwaitingAcknowledgement(
                        ExperimentalGptLiveResultDeliveryWaiter { resolution_rx },
                    ),
                );
            }
            Ok(LiveSidebandCommandDelivery::AmbiguousTerminal) => {
                LiveDelegationResultDeliveryObservation::Ambiguous
            }
            Err(ProviderWebrtcBrokerError::Rejected) => {
                LiveDelegationResultDeliveryObservation::Rejected
            }
            Err(_) => LiveDelegationResultDeliveryObservation::Ambiguous,
        };
        let pending = self
            .pending_deliveries
            .lock()
            .await
            .remove(&attempt)
            .ok_or(ExperimentalGptLiveBridgeError::ContextAuthorityRejected)?;
        let PendingExperimentalGptLiveDelivery::DelegationResult {
            authority,
            resolution_tx,
        } = pending
        else {
            return Err(ExperimentalGptLiveBridgeError::ContextAuthorityRejected);
        };
        drop(resolution_tx);
        Ok(ExperimentalGptLiveResultDeliveryDispatch::Resolved(
            ExperimentalGptLiveResultDeliveryResolution {
                authority,
                observation,
            },
        ))
    }

    /// Receive one sanitized observation from the exact current binding.
    pub async fn next_observation(
        &self,
        binding: &ProviderWebrtcBinding,
    ) -> Result<Option<ExperimentalGptLiveControlObservation>, ProviderWebrtcBrokerError> {
        let receiver = {
            let active = self.active_by_session.lock().await;
            let current = active
                .get(binding.session_id())
                .filter(|current| current.binding == *binding)
                .ok_or(ProviderWebrtcBrokerError::Rejected)?;
            Arc::clone(&current.observation_rx)
        };
        let mut receiver = receiver.lock().await;
        let observation = receiver
            .recv()
            .await
            .unwrap_or(Err(ProviderWebrtcBrokerError::Unavailable))?;
        let Some(observation) = observation else {
            return Ok(None);
        };
        let observed_delivery = match observation.kind() {
            LiveSidebandObservationKind::AppendAcknowledged { attempt } => {
                Some((attempt.clone(), true))
            }
            LiveSidebandObservationKind::AppendDeliveryAmbiguousTerminal { attempt } => {
                Some((attempt.clone(), false))
            }
            _ => None,
        };
        if let Some((attempt, acknowledged)) = observed_delivery {
            let pending = self.pending_deliveries.lock().await.remove(&attempt);
            let Some(pending) = pending else {
                return if matches!(
                    observation.kind(),
                    LiveSidebandObservationKind::AppendDeliveryAmbiguousTerminal { .. }
                ) {
                    Ok(Some(ExperimentalGptLiveControlObservation::Provider(
                        observation,
                    )))
                } else {
                    Err(ProviderWebrtcBrokerError::ProtocolDrift)
                };
            };
            match pending {
                PendingExperimentalGptLiveDelivery::CanonicalAppend {
                    authority,
                    resolution_tx,
                } => {
                    let resolution = ExperimentalGptLiveAppendResolution {
                        authority,
                        outcome: if acknowledged {
                            meerkat_core::LiveAppendDeliveryOutcome::Acknowledged
                        } else {
                            meerkat_core::LiveAppendDeliveryOutcome::Ambiguous
                        },
                    };
                    if let Err(resolution) = resolution_tx.send(resolution) {
                        return Ok(Some(ExperimentalGptLiveControlObservation::AppendResolved(
                            resolution,
                        )));
                    }
                }
                PendingExperimentalGptLiveDelivery::DelegationResult {
                    authority,
                    resolution_tx,
                } => {
                    let resolution = ExperimentalGptLiveResultDeliveryResolution {
                        authority,
                        observation: if acknowledged {
                            LiveDelegationResultDeliveryObservation::Delivered
                        } else {
                            LiveDelegationResultDeliveryObservation::Ambiguous
                        },
                    };
                    if let Err(resolution) = resolution_tx.send(resolution) {
                        return Ok(Some(
                            ExperimentalGptLiveControlObservation::ResultDeliveryResolved(
                                resolution,
                            ),
                        ));
                    }
                }
            }
            return Ok(Some(ExperimentalGptLiveControlObservation::Provider(
                observation,
            )));
        }
        Ok(Some(ExperimentalGptLiveControlObservation::Provider(
            observation,
        )))
    }

    /// Exact active binding projection for lifecycle reconciliation only.
    pub async fn active_binding(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<ProviderWebrtcBinding> {
        self.active_by_session
            .lock()
            .await
            .get(session_id)
            .filter(|active| !active.activation_gate.cancelled.load(Ordering::Acquire))
            .map(|active| active.binding.clone())
    }

    async fn pump_retirement_sender(&self) -> mpsc::Sender<ExperimentalGptLivePumpRetirement> {
        let mut sender = self.pump_retirement_tx.lock().await;
        if let Some(sender) = sender.as_ref() {
            return sender.clone();
        }
        let (retirement_tx, mut retirement_rx) =
            mpsc::channel::<ExperimentalGptLivePumpRetirement>(8);
        let active_by_session = Arc::clone(&self.active_by_session);
        let registered_by_channel = Arc::clone(&self.registered_by_channel);
        let pending_deliveries = Arc::clone(&self.pending_deliveries);
        let pending_pump_retirements = Arc::clone(&self.pending_pump_retirements);
        let actor = tokio::spawn(async move {
            let mut retries =
                Vec::<(tokio::time::Instant, ExperimentalGptLivePumpRetirement)>::new();
            let mut retirement_rx_open = true;
            loop {
                let retirement = if retries.is_empty() {
                    if !retirement_rx_open {
                        break;
                    }
                    retirement_rx.recv().await
                } else {
                    let Some((retry_index, retry_at)) = retries
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, (retry_at, _))| *retry_at)
                        .map(|(index, (retry_at, _))| (index, *retry_at))
                    else {
                        continue;
                    };
                    if retirement_rx_open {
                        tokio::select! {
                            incoming = retirement_rx.recv() => {
                                if incoming.is_none() {
                                    retirement_rx_open = false;
                                }
                                incoming
                            },
                            () = tokio::time::sleep_until(retry_at) => {
                                Some(retries.swap_remove(retry_index).1)
                            }
                        }
                    } else {
                        tokio::time::sleep_until(retry_at).await;
                        Some(retries.swap_remove(retry_index).1)
                    }
                };
                let Some(retirement) = retirement else {
                    if retries.is_empty() && !retirement_rx_open {
                        break;
                    }
                    continue;
                };
                let binding = &retirement.activation.runtime_binding;
                let semantic_retirement = retirement
                    .activation
                    .activator
                    .retire_bound_channel_after_pump_exit(binding)
                    .await;
                if let Err(ExperimentalLivePumpRetirementError::SemanticUncommitted(_)) =
                    semantic_retirement
                {
                    pending_pump_retirements.lock().await.insert(
                        (binding.session_id().clone(), binding.channel_id().clone()),
                        Arc::clone(&retirement.activation),
                    );
                    let backoff_ms = 25_u64
                        .saturating_mul(1_u64 << retirement.attempt.min(7))
                        .min(2_000);
                    retries.push((
                        tokio::time::Instant::now() + std::time::Duration::from_millis(backoff_ms),
                        ExperimentalGptLivePumpRetirement {
                            activation: retirement.activation,
                            attempt: retirement.attempt.saturating_add(1),
                        },
                    ));
                    continue;
                }
                retirement
                    .activation
                    .runtime
                    .retire_live_assistant_output_handles(
                        binding.session_id(),
                        binding.channel_id(),
                    );
                let active = {
                    let mut active = active_by_session.lock().await;
                    active
                        .get(binding.session_id())
                        .is_some_and(|current| {
                            current.binding.channel_id() == binding.channel_id()
                                && current.binding.runtime_generation().get()
                                    == binding.generation()
                                && current.binding.runtime_fence().get() == binding.fence_token()
                        })
                        .then(|| active.remove(binding.session_id()))
                        .flatten()
                };
                if let Some(active) = active {
                    let _ = active.sideband.close().await;
                    retire_sideband_actors(active).await;
                }
                let mut registrations = registered_by_channel.lock().await;
                if registrations
                    .get(binding.channel_id())
                    .is_some_and(|registration| registration.session_id == *binding.session_id())
                {
                    registrations.remove(binding.channel_id());
                }
                drop(registrations);
                retire_pending_deliveries(pending_deliveries.as_ref(), binding.channel_id()).await;
                pending_pump_retirements
                    .lock()
                    .await
                    .remove(&(binding.session_id().clone(), binding.channel_id().clone()));
            }
        });
        *self.pump_retirement_actor.lock().await = Some(actor);
        *sender = Some(retirement_tx.clone());
        retirement_tx
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "this exact activation boundary carries independent runtime, provider, and publication authorities"
    )]
    async fn prepare_bound_channel_activation(
        &self,
        provider_binding: &ProviderWebrtcBinding,
        answer_observation_sequence: u64,
        runtime: Arc<meerkat_runtime::meerkat_machine::MeerkatMachine>,
        runtime_binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        activator: Arc<dyn ExperimentalLiveBoundChannelActivator>,
        control: Arc<dyn ExperimentalGptLiveControlPlane>,
        live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
        public_observation_publisher: Arc<dyn ExperimentalLivePublicObservationPublisher>,
    ) -> Result<(), String> {
        if runtime_binding.session_id() != provider_binding.session_id()
            || runtime_binding.channel_id() != provider_binding.channel_id()
            || runtime_binding.generation() != provider_binding.runtime_generation().get()
            || runtime_binding.fence_token() != provider_binding.runtime_fence().get()
        {
            return Err("provider activation does not match generated runtime binding".to_string());
        }
        activator
            .prepare_bound_channel(runtime_binding.clone(), Arc::clone(&control))
            .await?;
        let gate = {
            let active = self.active_by_session.lock().await;
            active
                .get(provider_binding.session_id())
                .filter(|current| {
                    current.binding == *provider_binding
                        && current.answer_observation_sequence == answer_observation_sequence
                })
                .map(|current| Arc::clone(&current.activation_gate))
        };
        let Some(gate) = gate else {
            let _ = activator.deactivate_bound_channel(&runtime_binding).await;
            return Err("provider activation lost exact active answer custody".to_string());
        };
        let mut prepared = gate.prepared.lock().await;
        if prepared.is_some() || gate.committed.load(Ordering::Acquire) {
            drop(prepared);
            let _ = activator.deactivate_bound_channel(&runtime_binding).await;
            return Err("provider activation was already prepared or committed".to_string());
        }
        *prepared = Some(Arc::new(PreparedExperimentalGptLiveActivation {
            runtime,
            runtime_binding,
            activator,
            control,
            live_adapter_host,
            public_observation_publisher,
        }));
        drop(prepared);
        gate.changed.notify_waiters();
        Ok(())
    }

    async fn commit_bound_channel_activation(
        &self,
        session_id: &meerkat_core::SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        generation: u64,
        fence: u64,
    ) -> bool {
        let gate = {
            let active = self.active_by_session.lock().await;
            active
                .get(session_id)
                .filter(|current| {
                    current.binding.channel_id() == channel_id
                        && current.binding.runtime_generation().get() == generation
                        && current.binding.runtime_fence().get() == fence
                })
                .map(|current| Arc::clone(&current.activation_gate))
        };
        let Some(gate) = gate else {
            return false;
        };
        if gate.prepared.lock().await.is_none() || gate.cancelled.load(Ordering::Acquire) {
            return false;
        }
        gate.committed.store(true, Ordering::Release);
        gate.changed.notify_waiters();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            gate.wait_for_started_tasks(3),
        )
        .await
        .unwrap_or(false)
    }

    async fn answer_provider_offer(
        &self,
        offer: ProviderWebrtcOffer,
    ) -> Result<LiveWebrtcAnswerAccepted, ProviderWebrtcSignalingError> {
        let _operation = self.operations.lock().await;
        let binding = offer.binding().clone();
        let (broker, adapter) = {
            let registrations = self.registered_by_channel.lock().await;
            let registration = registrations
                .get(binding.channel_id())
                .filter(|registration| registration.session_id == *binding.session_id())
                .ok_or(ProviderWebrtcSignalingError::Broker(
                    ProviderWebrtcBrokerError::Rejected,
                ))?;
            (
                Arc::clone(&registration.broker),
                Arc::clone(&registration.adapter),
            )
        };
        let broker_answer = broker
            .answer(offer)
            .await
            .map_err(ProviderWebrtcSignalingError::Broker)?;
        let (answer_sdp, candidate_sideband, pending_bound_ready) = broker_answer.into_parts();
        if answer_sdp.trim().is_empty() {
            candidate_sideband
                .close()
                .await
                .map_err(ProviderWebrtcSignalingError::SidebandClose)?;
            return Err(ProviderWebrtcSignalingError::EmptyAnswer);
        }

        let previous = self
            .active_by_session
            .lock()
            .await
            .remove(binding.session_id());
        if let Some(previous) = previous {
            self.active_by_session
                .lock()
                .await
                .insert(previous.binding.session_id().clone(), previous);
            let _ = candidate_sideband.close().await;
            return Err(ProviderWebrtcSignalingError::Broker(
                ProviderWebrtcBrokerError::Rejected,
            ));
        }

        let answer_observation_sequence = self
            .answer_observation_sequence
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        let pump_retirement_tx = self.pump_retirement_sender().await;
        let active = spawn_sideband_actors(
            binding.clone(),
            Arc::clone(&candidate_sideband),
            adapter,
            answer_observation_sequence,
            pump_retirement_tx,
        );
        self.active_by_session
            .lock()
            .await
            .insert(binding.session_id().clone(), active);
        Ok(LiveWebrtcAnswerAccepted {
            answer_sdp,
            answer_observation_sequence,
            pending_bound_ready: Some(pending_bound_ready),
        })
    }

    async fn close_exact(
        &self,
        binding: &ProviderWebrtcBinding,
        answer_observation_sequence: Option<u64>,
    ) -> Result<bool, ProviderWebrtcSignalingError> {
        let _operation = self.operations.lock().await;
        let active = {
            let mut active = self.active_by_session.lock().await;
            let matches = active.get(binding.session_id()).is_some_and(|current| {
                current.binding == *binding
                    && answer_observation_sequence
                        .is_none_or(|sequence| current.answer_observation_sequence == sequence)
            });
            matches
                .then(|| active.remove(binding.session_id()))
                .flatten()
        };
        let Some(active) = active else {
            return Ok(false);
        };
        if let Err(error) = active.sideband.close().await {
            self.active_by_session
                .lock()
                .await
                .insert(active.binding.session_id().clone(), active);
            return Err(ProviderWebrtcSignalingError::SidebandClose(error));
        }
        retire_sideband_actors(active).await;
        retire_pending_deliveries(self.pending_deliveries.as_ref(), binding.channel_id()).await;
        Ok(true)
    }
}

#[async_trait]
impl ExperimentalGptLiveControlPlane for ExperimentalGptLiveWebrtcTransport {
    async fn active_binding(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<ProviderWebrtcBinding> {
        ExperimentalGptLiveWebrtcTransport::active_binding(self, session_id).await
    }

    async fn next_observation(
        &self,
        binding: &ProviderWebrtcBinding,
    ) -> Result<Option<ExperimentalGptLiveControlObservation>, ProviderWebrtcBrokerError> {
        ExperimentalGptLiveWebrtcTransport::next_observation(self, binding).await
    }

    async fn append_session_context(
        &self,
        authority: LiveContextAppendAuthority,
        text: String,
    ) -> Result<ExperimentalGptLiveAppendDispatch, ExperimentalGptLiveBridgeError> {
        ExperimentalGptLiveWebrtcTransport::append_session_context(self, authority, text).await
    }

    async fn release_delegation_context(
        &self,
        authority: LiveDelegationResultDeliveryAuthority,
        delegation: LiveSidebandDelegationRef,
        text: String,
    ) -> Result<ExperimentalGptLiveResultDeliveryDispatch, ExperimentalGptLiveBridgeError> {
        ExperimentalGptLiveWebrtcTransport::release_delegation_context(
            self, authority, delegation, text,
        )
        .await
    }
}

fn spawn_sideband_actors(
    binding: ProviderWebrtcBinding,
    sideband: Arc<dyn ProviderWebrtcSidebandSession>,
    adapter: Arc<ExperimentalGptLiveDeferredAdapter>,
    answer_observation_sequence: u64,
    pump_retirement_tx: mpsc::Sender<ExperimentalGptLivePumpRetirement>,
) -> ActiveExperimentalGptLiveBinding {
    let activation_gate = Arc::new(ExperimentalGptLiveActivationGate::new());
    let (command_tx, mut command_rx) = mpsc::channel::<SidebandCommandEnvelope>(32);
    let command_sideband = Arc::clone(&sideband);
    let command_actor = tokio::spawn(async move {
        while let Some(envelope) = command_rx.recv().await {
            let result = command_sideband.send_command(envelope.command).await;
            let _ = envelope.result.send(result);
        }
    });

    let (observation_tx, observation_rx) = mpsc::channel(64);
    let observation_sideband = Arc::clone(&sideband);
    let observation_binding = binding.clone();
    let observation_gate = Arc::clone(&activation_gate);
    let observation_adapter = Arc::clone(&adapter);
    let observation_actor = tokio::spawn(async move {
        let Some(activation) = observation_gate.wait_for_commit().await else {
            observation_adapter.close_stream();
            return;
        };
        observation_gate.mark_started();
        loop {
            let next = tokio::select! {
                () = observation_gate.cancelled() => break,
                next = observation_sideband.next_observation() => next,
            };
            match next {
                Ok(Some(observation)) => {
                    let control_observation = matches!(
                        observation.kind(),
                        LiveSidebandObservationKind::UserTranscriptFragment { .. }
                            | LiveSidebandObservationKind::AssistantTranscriptFragment { .. }
                            | LiveSidebandObservationKind::TurnStarted { .. }
                            | LiveSidebandObservationKind::TurnSnapshotDelta { .. }
                            | LiveSidebandObservationKind::TurnFinished { .. }
                            | LiveSidebandObservationKind::DelegationRequested { .. }
                            | LiveSidebandObservationKind::DelegationActionableInputUnsupported { .. }
                            | LiveSidebandObservationKind::AppendAcknowledged { .. }
                            | LiveSidebandObservationKind::AppendDeliveryAmbiguousTerminal { .. }
                            | LiveSidebandObservationKind::UnsupportedProviderEvent
                    );
                    let adapter_observation = matches!(
                        observation.kind(),
                        LiveSidebandObservationKind::SessionReady
                            | LiveSidebandObservationKind::TurnStarted { .. }
                            | LiveSidebandObservationKind::TurnFinished { .. }
                            | LiveSidebandObservationKind::DelegationActionableInputUnsupported { .. }
                            | LiveSidebandObservationKind::UnsupportedProviderEvent
                    );
                    let lifecycle_observation = matches!(
                        observation.kind(),
                        LiveSidebandObservationKind::TurnStarted { .. }
                            | LiveSidebandObservationKind::TurnFinished { .. }
                            | LiveSidebandObservationKind::DelegationRequested { .. }
                    );
                    if lifecycle_observation
                        && let Err(error) = activation
                            .activator
                            .observe_provider_lifecycle(&observation)
                            .await
                    {
                        tracing::warn!(
                            error,
                            "experimental live lifecycle observation failed closed"
                        );
                        break;
                    }
                    if adapter_observation
                        && observation_adapter
                            .push_observation(observation.clone())
                            .is_err()
                    {
                        break;
                    }
                    if control_observation
                        && observation_tx.send(Ok(Some(observation))).await.is_err()
                    {
                        break;
                    }
                }
                Ok(None) => {
                    observation_adapter.close_stream();
                    let _ = observation_tx.send(Ok(None)).await;
                    break;
                }
                Err(error) => {
                    let _ = observation_adapter.push_observation(LiveSidebandObservation::new(
                        observation_binding.clone(),
                        LiveSidebandObservationKind::UnsupportedProviderEvent,
                    ));
                    observation_adapter.close_stream();
                    let _ = observation_tx.send(Err(error)).await;
                    break;
                }
            }
        }
        observation_adapter.close_stream();
    });

    let control_gate = Arc::clone(&activation_gate);
    let control_actor = tokio::spawn(async move {
        let Some(activation) = control_gate.wait_for_commit().await else {
            return;
        };
        control_gate.mark_started();
        activation
            .activator
            .run_bound_channel(
                activation.runtime_binding.clone(),
                Arc::clone(&activation.control),
            )
            .await;
    });

    let pump_gate = Arc::clone(&activation_gate);
    let pump_binding = binding.clone();
    let adapter_pump = tokio::spawn(async move {
        let Some(activation) = pump_gate.wait_for_commit().await else {
            return;
        };
        pump_gate.mark_started();
        loop {
            let next = tokio::select! {
                () = pump_gate.cancelled() => return,
                next = activation
                    .live_adapter_host
                    .next_observation_raw(pump_binding.channel_id()) => next,
            };
            let observation = match next {
                Ok(Some(observation)) => observation,
                Ok(None) => {
                    tracing::warn!("experimental live adapter observation stream ended");
                    break;
                }
                Err(_) => {
                    tracing::warn!("experimental live adapter observation read failed");
                    break;
                }
            };
            if matches!(&observation, LiveAdapterObservation::Error { .. })
                || matches!(
                    &observation,
                    LiveAdapterObservation::StatusChanged { status } if status.is_terminal()
                )
            {
                tracing::warn!("experimental live adapter emitted a terminal observation");
                break;
            }
            let outcome = match activation
                .live_adapter_host
                .apply_observation(pump_binding.channel_id(), &observation)
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    tracing::warn!(%error, "experimental live adapter observation application failed");
                    break;
                }
            };
            if let meerkat_live::ObservationOutcome::AssistantOutputAvailable(ref output) = outcome
            {
                let public = ExperimentalLivePublicObservation::assistant_output_available(
                    pump_binding.clone(),
                    output.clone(),
                );
                if activation
                    .public_observation_publisher
                    .publish(public)
                    .await
                    .is_err()
                {
                    tracing::warn!("experimental live public observation publication failed");
                    activation
                        .live_adapter_host
                        .fail_playback_waiters_for_channel(
                            pump_binding.channel_id(),
                            "assistant output publication was rejected",
                        )
                        .await;
                    let _ = activation
                        .live_adapter_host
                        .fail_assistant_output_publication(output)
                        .await;
                    break;
                }
            }
            if matches!(outcome, meerkat_live::ObservationOutcome::Terminal { .. }) {
                tracing::warn!("experimental live adapter reached a terminal outcome");
                break;
            }
        }
        // Mark the exact binding nonselectable before any awaited close.
        pump_gate.cancel();
        activation
            .live_adapter_host
            .fail_playback_waiters_for_channel(
                pump_binding.channel_id(),
                "provider observation pump retired before playback terminal settlement",
            )
            .await;
        let _ = pump_retirement_tx
            .send(ExperimentalGptLivePumpRetirement {
                activation,
                attempt: 0,
            })
            .await;
    });

    ActiveExperimentalGptLiveBinding {
        binding,
        sideband,
        answer_observation_sequence,
        command_tx,
        observation_rx: Arc::new(Mutex::new(observation_rx)),
        command_actor,
        observation_actor,
        control_actor,
        adapter_pump,
        activation_gate,
    }
}

async fn retire_pending_deliveries(
    pending_deliveries: &Mutex<
        HashMap<LiveSidebandAppendAttempt, PendingExperimentalGptLiveDelivery>,
    >,
    channel_id: &meerkat_live::LiveChannelId,
) {
    let mut pending_deliveries = pending_deliveries.lock().await;
    let retired_attempts = pending_deliveries
        .iter()
        .filter(|(_, pending)| pending.channel_id() == channel_id)
        .map(|(attempt, _)| attempt.clone())
        .collect::<Vec<_>>();
    for attempt in retired_attempts {
        if let Some(pending) = pending_deliveries.remove(&attempt) {
            match pending {
                PendingExperimentalGptLiveDelivery::CanonicalAppend {
                    authority,
                    resolution_tx,
                } => {
                    let _ = resolution_tx.send(ExperimentalGptLiveAppendResolution {
                        authority,
                        outcome: meerkat_core::LiveAppendDeliveryOutcome::Ambiguous,
                    });
                }
                PendingExperimentalGptLiveDelivery::DelegationResult {
                    authority,
                    resolution_tx,
                } => {
                    let _ = resolution_tx.send(ExperimentalGptLiveResultDeliveryResolution {
                        authority,
                        observation: LiveDelegationResultDeliveryObservation::Ambiguous,
                    });
                }
            }
        }
    }
}

async fn retire_sideband_actors(active: ActiveExperimentalGptLiveBinding) {
    if let Some(prepared) = active
        .activation_gate
        .prepared
        .lock()
        .await
        .as_ref()
        .cloned()
    {
        let _ = prepared
            .activator
            .deactivate_bound_channel(&prepared.runtime_binding)
            .await;
    }
    active.activation_gate.cancel();
    active.command_actor.abort();
    let _ = active.command_actor.await;
    let mut observation_actor = active.observation_actor;
    if tokio::time::timeout(std::time::Duration::from_secs(2), &mut observation_actor)
        .await
        .is_err()
    {
        observation_actor.abort();
        let _ = observation_actor.await;
    }
    let mut control_actor = active.control_actor;
    if tokio::time::timeout(std::time::Duration::from_secs(2), &mut control_actor)
        .await
        .is_err()
    {
        control_actor.abort();
        let _ = control_actor.await;
    }
    let mut adapter_pump = active.adapter_pump;
    if tokio::time::timeout(std::time::Duration::from_secs(2), &mut adapter_pump)
        .await
        .is_err()
    {
        adapter_pump.abort();
        let _ = adapter_pump.await;
    }
}

fn provider_signaling_error(error: ProviderWebrtcSignalingError) -> LiveWebrtcError {
    let reason = match error {
        ProviderWebrtcSignalingError::Broker(ProviderWebrtcBrokerError::Unavailable) => {
            "remote_unavailable"
        }
        ProviderWebrtcSignalingError::Broker(ProviderWebrtcBrokerError::Rejected) => {
            "remote_rejected"
        }
        ProviderWebrtcSignalingError::Broker(ProviderWebrtcBrokerError::ProtocolDrift) => {
            "remote_protocol_drift"
        }
        ProviderWebrtcSignalingError::EmptyAnswer => "empty_remote_answer",
        ProviderWebrtcSignalingError::SidebandClose(ProviderWebrtcBrokerError::Unavailable) => {
            "remote_close_unavailable"
        }
        ProviderWebrtcSignalingError::SidebandClose(ProviderWebrtcBrokerError::Rejected) => {
            "remote_close_rejected"
        }
        ProviderWebrtcSignalingError::SidebandClose(ProviderWebrtcBrokerError::ProtocolDrift) => {
            "remote_close_protocol_drift"
        }
        _ => "remote_protocol_drift",
    };
    LiveWebrtcError::RemoteSignaling { reason }
}

fn provider_binding(
    binding: &LiveWebrtcBindingRequest,
) -> Result<ProviderWebrtcBinding, LiveWebrtcError> {
    let runtime = binding
        .runtime_binding
        .ok_or(LiveWebrtcError::RuntimeBindingUnavailable)?;
    Ok(ProviderWebrtcBinding::new(
        binding.channel_id.clone(),
        binding.session_id.clone(),
        meerkat_live::LiveRuntimeBindingGeneration::new(runtime.generation),
        meerkat_live::LiveRuntimeBindingFence::new(runtime.fence),
    ))
}

#[async_trait]
impl LiveWebrtcAnswerTransport for ExperimentalGptLiveWebrtcTransport {
    async fn answer_admitted_offer(
        &self,
        offer: LiveWebrtcAdmittedOffer,
    ) -> Result<LiveWebrtcAnswerAccepted, LiveWebrtcError> {
        let provider_offer = offer.into_provider_offer()?;
        self.answer_provider_offer(provider_offer)
            .await
            .map_err(provider_signaling_error)
    }

    async fn reject_answer(
        &self,
        binding: &LiveWebrtcBindingRequest,
        answer_observation_sequence: u64,
    ) -> Result<(), LiveWebrtcError> {
        let binding = provider_binding(binding)?;
        self.close_exact(&binding, Some(answer_observation_sequence))
            .await
            .map(|_| ())
            .map_err(provider_signaling_error)
    }

    async fn accept_answer(
        &self,
        _binding: &LiveWebrtcBindingRequest,
        _answer_observation_sequence: u64,
    ) {
    }

    async fn wait_for_construction_cleanup(
        &self,
        _binding: &LiveWebrtcBindingRequest,
    ) -> Result<(), LiveWebrtcError> {
        Ok(())
    }

    async fn close_binding(
        &self,
        binding: &LiveWebrtcBindingRequest,
    ) -> Result<(), LiveWebrtcError> {
        let provider_binding = provider_binding(binding)?;
        self.close_exact(&provider_binding, None)
            .await
            .map_err(provider_signaling_error)?;
        self.unbind_channel(&binding.channel_id, &binding.session_id)
            .await;
        Ok(())
    }
}

#[derive(Default)]
struct SidebandCorrelations {
    next_delegation_ref: u64,
    next_transcript_item_ref: u64,
    next_turn_ref: u64,
    delegations: HashMap<String, GptLiveDelegationRef>,
    turns: HashMap<String, LiveSidebandTurnRef>,
    appends: SidebandAppendCorrelations<GptLiveAppendToken>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SidebandAppendLane {
    SessionContext,
    DelegationContext,
}

#[derive(Clone)]
struct SidebandAppendReservation {
    lane: SidebandAppendLane,
    attempt: LiveSidebandAppendAttempt,
}

enum SidebandAppendReservationState<Token> {
    Reserved {
        attempt: LiveSidebandAppendAttempt,
    },
    AcknowledgedBeforeCommit {
        attempt: LiveSidebandAppendAttempt,
        token: Token,
    },
}

struct CommittedSidebandAppend {
    lane: SidebandAppendLane,
    attempt: LiveSidebandAppendAttempt,
}

struct SidebandAppendCorrelations<Token> {
    reservations: HashMap<SidebandAppendLane, SidebandAppendReservationState<Token>>,
    committed: HashMap<Token, CommittedSidebandAppend>,
}

impl<Token> Default for SidebandAppendCorrelations<Token> {
    fn default() -> Self {
        Self {
            reservations: HashMap::new(),
            committed: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebandAppendCommit {
    AwaitingAcknowledgement,
    AlreadyAcknowledged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebandAppendRollback {
    RolledBack,
    AlreadyAcknowledged,
}

impl<Token> SidebandAppendCorrelations<Token>
where
    Token: Clone + Eq + std::hash::Hash,
{
    fn reserve(
        &mut self,
        lane: SidebandAppendLane,
        attempt: LiveSidebandAppendAttempt,
    ) -> Result<SidebandAppendReservation, ProviderWebrtcBrokerError> {
        if self.reservations.contains_key(&lane)
            || self
                .committed
                .values()
                .any(|committed| committed.lane == lane)
        {
            return Err(ProviderWebrtcBrokerError::ProtocolDrift);
        }
        self.reservations.insert(
            lane,
            SidebandAppendReservationState::Reserved {
                attempt: attempt.clone(),
            },
        );
        Ok(SidebandAppendReservation { lane, attempt })
    }

    fn commit(
        &mut self,
        reservation: &SidebandAppendReservation,
        token: Token,
    ) -> Result<SidebandAppendCommit, ProviderWebrtcBrokerError> {
        let state = self
            .reservations
            .remove(&reservation.lane)
            .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)?;
        match state {
            SidebandAppendReservationState::Reserved { attempt }
                if attempt == reservation.attempt =>
            {
                if self.committed.contains_key(&token) {
                    return Err(ProviderWebrtcBrokerError::ProtocolDrift);
                }
                self.committed.insert(
                    token,
                    CommittedSidebandAppend {
                        lane: reservation.lane,
                        attempt,
                    },
                );
                Ok(SidebandAppendCommit::AwaitingAcknowledgement)
            }
            SidebandAppendReservationState::AcknowledgedBeforeCommit {
                attempt,
                token: acknowledged,
            } if attempt == reservation.attempt && acknowledged == token => {
                Ok(SidebandAppendCommit::AlreadyAcknowledged)
            }
            _ => Err(ProviderWebrtcBrokerError::ProtocolDrift),
        }
    }

    fn rollback(
        &mut self,
        reservation: &SidebandAppendReservation,
    ) -> Result<SidebandAppendRollback, ProviderWebrtcBrokerError> {
        let state = self
            .reservations
            .remove(&reservation.lane)
            .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)?;
        match state {
            SidebandAppendReservationState::Reserved { attempt }
                if attempt == reservation.attempt =>
            {
                Ok(SidebandAppendRollback::RolledBack)
            }
            SidebandAppendReservationState::AcknowledgedBeforeCommit { attempt, .. }
                if attempt == reservation.attempt =>
            {
                Ok(SidebandAppendRollback::AlreadyAcknowledged)
            }
            _ => Err(ProviderWebrtcBrokerError::ProtocolDrift),
        }
    }

    fn acknowledge(
        &mut self,
        lane: SidebandAppendLane,
        token: &Token,
    ) -> Result<LiveSidebandAppendAttempt, ProviderWebrtcBrokerError> {
        if let Some(committed) = self.committed.remove(token) {
            return (committed.lane == lane)
                .then_some(committed.attempt)
                .ok_or(ProviderWebrtcBrokerError::ProtocolDrift);
        }
        let state = self
            .reservations
            .remove(&lane)
            .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)?;
        match state {
            SidebandAppendReservationState::Reserved { attempt } => {
                self.reservations.insert(
                    lane,
                    SidebandAppendReservationState::AcknowledgedBeforeCommit {
                        attempt: attempt.clone(),
                        token: token.clone(),
                    },
                );
                Ok(attempt)
            }
            SidebandAppendReservationState::AcknowledgedBeforeCommit { .. } => {
                Err(ProviderWebrtcBrokerError::ProtocolDrift)
            }
        }
    }
}

impl SidebandCorrelations {
    fn existing_turn_provider_id(
        &self,
        provider_turn_id: &str,
    ) -> Result<LiveSidebandTurnRef, ProviderWebrtcBrokerError> {
        self.turns
            .get(provider_turn_id)
            .cloned()
            .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)
    }

    fn lower_turn_provider_id(
        &mut self,
        channel_id: &meerkat_live::LiveChannelId,
        provider_turn_id: &str,
        terminal: bool,
    ) -> Result<LiveSidebandTurnRef, ProviderWebrtcBrokerError> {
        if terminal {
            return self
                .turns
                .remove(provider_turn_id)
                .ok_or(ProviderWebrtcBrokerError::ProtocolDrift);
        }
        if let Some(turn) = self.turns.get(provider_turn_id) {
            return Ok(turn.clone());
        }
        self.next_turn_ref = self.next_turn_ref.saturating_add(1);
        let turn = LiveSidebandTurnRef::__from_provider_observation(
            channel_id,
            format!("turn:{}", self.next_turn_ref),
            provider_turn_id.to_string(),
        )
        .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)?;
        self.turns
            .insert(provider_turn_id.to_string(), turn.clone());
        Ok(turn)
    }
}

struct ExperimentalGptLiveSideband {
    binding: ProviderWebrtcBinding,
    session: Arc<dyn ExperimentalGptLiveBrokerSession>,
    seed_custody: Mutex<ExperimentalGptLiveSeedCustody>,
    seed_changed: Notify,
    correlations: Mutex<SidebandCorrelations>,
    synthetic_tx: mpsc::Sender<LiveSidebandObservation>,
    synthetic_rx: Mutex<mpsc::Receiver<LiveSidebandObservation>>,
}

impl fmt::Debug for ExperimentalGptLiveSideband {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExperimentalGptLiveSideband([OPAQUE])")
    }
}

#[async_trait]
impl ProviderWebrtcSidebandSession for ExperimentalGptLiveSideband {
    async fn send_command(
        &self,
        command: LiveSidebandCommand,
    ) -> Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError> {
        if command.binding() != &self.binding {
            return Err(ProviderWebrtcBrokerError::Rejected);
        }
        match command.__into_provider_command() {
            LiveSidebandProviderCommand::AppendSessionContext { attempt, text, .. } => {
                let reservation = self
                    .correlations
                    .lock()
                    .await
                    .appends
                    .reserve(SidebandAppendLane::SessionContext, attempt)?;
                let result = self.session.append_session_context(text).await;
                self.lower_append_delivery(reservation, result).await
            }
            LiveSidebandProviderCommand::ReleaseDelegationContext {
                attempt,
                delegation,
                text,
                ..
            } => {
                let provider_delegation = self
                    .correlations
                    .lock()
                    .await
                    .delegations
                    .get(delegation.__provider_opaque_value())
                    .cloned()
                    .ok_or(ProviderWebrtcBrokerError::Rejected)?;
                let reservation = self
                    .correlations
                    .lock()
                    .await
                    .appends
                    .reserve(SidebandAppendLane::DelegationContext, attempt)?;
                let result = self
                    .session
                    .append_delegation_context(&provider_delegation, text)
                    .await;
                self.lower_append_delivery(reservation, result).await
            }
        }
    }

    async fn next_observation(
        &self,
    ) -> Result<Option<LiveSidebandObservation>, ProviderWebrtcBrokerError> {
        self.wait_for_seed_resolution().await?;
        let mut synthetic_rx = self.synthetic_rx.lock().await;
        tokio::select! {
            biased;
            observation = synthetic_rx.recv() => Ok(observation),
            observation = self.session.next_observation() => {
                let Some(observation) = observation.map_err(map_broker_error)? else {
                    return Ok(None);
                };
                self.lower_observation(observation).await.map(Some)
            }
        }
    }

    async fn close(&self) -> Result<(), ProviderWebrtcBrokerError> {
        self.session.close().await.map_err(map_broker_error)
    }
}

impl ExperimentalGptLiveSideband {
    async fn wait_for_seed_resolution(&self) -> Result<(), ProviderWebrtcBrokerError> {
        loop {
            let changed = self.seed_changed.notified();
            match &*self.seed_custody.lock().await {
                ExperimentalGptLiveSeedCustody::Ready => return Ok(()),
                ExperimentalGptLiveSeedCustody::Failed(error) => return Err(*error),
                ExperimentalGptLiveSeedCustody::Pending(_)
                | ExperimentalGptLiveSeedCustody::InFlight { .. } => {}
            }
            changed.await;
        }
    }

    /// Resolve the answer-bound seed exactly once from response-delivery
    /// custody. The spawned task owns the projection lease, so cancellation of
    /// an outer waiter cannot reconstruct or replay acknowledged seed state.
    async fn resolve_initial_seed(&self) -> Result<u64, ProviderWebrtcBrokerError> {
        let mut custody = self.seed_custody.lock().await;
        if let ExperimentalGptLiveSeedCustody::Pending(seed) = &mut *custody {
            let seed = seed
                .take()
                .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)?;
            let canonical_seed_cursor = seed.canonical_seed_cursor;
            let session = Arc::clone(&self.session);
            *custody = ExperimentalGptLiveSeedCustody::InFlight {
                canonical_seed_cursor,
                task: tokio::spawn(async move {
                    let ExperimentalGptLiveInitialSeed {
                        commentary,
                        canonical_seed_cursor: _,
                        _projection_lease,
                    } = seed;
                    session
                        .await_ready_and_seed_session_context(commentary)
                        .await
                }),
            };
        }

        let (canonical_seed_cursor, result) = match &mut *custody {
            ExperimentalGptLiveSeedCustody::InFlight {
                canonical_seed_cursor,
                task,
            } => (*canonical_seed_cursor, task.await),
            ExperimentalGptLiveSeedCustody::Ready => {
                return Err(ProviderWebrtcBrokerError::ProtocolDrift);
            }
            ExperimentalGptLiveSeedCustody::Failed(error) => return Err(*error),
            ExperimentalGptLiveSeedCustody::Pending(_) => {
                return Err(ProviderWebrtcBrokerError::ProtocolDrift);
            }
        };
        match result {
            Ok(Ok(())) => {
                self.synthetic_tx
                    .send(LiveSidebandObservation::new(
                        self.binding.clone(),
                        LiveSidebandObservationKind::SessionReady,
                    ))
                    .await
                    .map_err(|_| ProviderWebrtcBrokerError::Unavailable)?;
                *custody = ExperimentalGptLiveSeedCustody::Ready;
                self.seed_changed.notify_waiters();
                Ok(canonical_seed_cursor)
            }
            Ok(Err(error)) => {
                let error = map_broker_error(error);
                *custody = ExperimentalGptLiveSeedCustody::Failed(error);
                self.seed_changed.notify_waiters();
                Err(error)
            }
            Err(_) => {
                *custody =
                    ExperimentalGptLiveSeedCustody::Failed(ProviderWebrtcBrokerError::Unavailable);
                self.seed_changed.notify_waiters();
                Err(ProviderWebrtcBrokerError::Unavailable)
            }
        }
    }

    async fn lower_append_delivery(
        &self,
        reservation: SidebandAppendReservation,
        result: Result<GptLiveAppendToken, GptLiveBrokerError>,
    ) -> Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError> {
        match result {
            Ok(token) => {
                let commit = self
                    .correlations
                    .lock()
                    .await
                    .appends
                    .commit(&reservation, token)?;
                debug_assert!(matches!(
                    commit,
                    SidebandAppendCommit::AwaitingAcknowledgement
                        | SidebandAppendCommit::AlreadyAcknowledged
                ));
                Ok(LiveSidebandCommandDelivery::Accepted)
            }
            Err(GptLiveBrokerError::AppendDeliveryAmbiguous { token }) => {
                let commit = self
                    .correlations
                    .lock()
                    .await
                    .appends
                    .commit(&reservation, token)?;
                if commit == SidebandAppendCommit::AlreadyAcknowledged {
                    return Ok(LiveSidebandCommandDelivery::Accepted);
                }
                self.synthetic_tx
                    .send(LiveSidebandObservation::new(
                        self.binding.clone(),
                        LiveSidebandObservationKind::AppendDeliveryAmbiguousTerminal {
                            attempt: reservation.attempt,
                        },
                    ))
                    .await
                    .map_err(|_| ProviderWebrtcBrokerError::Unavailable)?;
                Ok(LiveSidebandCommandDelivery::AmbiguousTerminal)
            }
            Err(error) => {
                let rollback = self
                    .correlations
                    .lock()
                    .await
                    .appends
                    .rollback(&reservation)?;
                if rollback == SidebandAppendRollback::AlreadyAcknowledged {
                    Ok(LiveSidebandCommandDelivery::Accepted)
                } else {
                    Err(map_broker_error(error))
                }
            }
        }
    }

    async fn lower_observation(
        &self,
        observation: GptLiveBrokerObservation,
    ) -> Result<LiveSidebandObservation, ProviderWebrtcBrokerError> {
        let kind = match observation {
            GptLiveBrokerObservation::SessionReady => LiveSidebandObservationKind::SessionReady,
            GptLiveBrokerObservation::SessionContextAppendAcknowledged { token } => {
                let attempt = self
                    .correlations
                    .lock()
                    .await
                    .appends
                    .acknowledge(SidebandAppendLane::SessionContext, &token)?;
                LiveSidebandObservationKind::AppendAcknowledged { attempt }
            }
            GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token } => {
                let attempt = self
                    .correlations
                    .lock()
                    .await
                    .appends
                    .acknowledge(SidebandAppendLane::DelegationContext, &token)?;
                LiveSidebandObservationKind::AppendAcknowledged { attempt }
            }
            GptLiveBrokerObservation::UserTranscriptFragment { item, text } => {
                let mut correlations = self.correlations.lock().await;
                correlations.next_transcript_item_ref =
                    correlations.next_transcript_item_ref.saturating_add(1);
                let local = format!("input-item:{}", correlations.next_transcript_item_ref);
                let item = LiveSidebandTranscriptItemRef::__from_provider_observation(
                    local,
                    item.__opaque_provider_id().to_string(),
                )
                .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)?;
                LiveSidebandObservationKind::UserTranscriptFragment { item, text }
            }
            GptLiveBrokerObservation::AssistantTranscriptFragment { item, text } => {
                let mut correlations = self.correlations.lock().await;
                correlations.next_transcript_item_ref =
                    correlations.next_transcript_item_ref.saturating_add(1);
                let local = format!("output-item:{}", correlations.next_transcript_item_ref);
                let item = LiveSidebandTranscriptItemRef::__from_provider_observation(
                    local,
                    item.__opaque_provider_id().to_string(),
                )
                .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)?;
                LiveSidebandObservationKind::AssistantTranscriptFragment { item, text }
            }
            GptLiveBrokerObservation::TurnStarted { turn, role } => {
                let turn = self.lower_turn_ref(turn, false).await?;
                LiveSidebandObservationKind::TurnStarted {
                    turn,
                    role: lower_turn_role(role),
                }
            }
            GptLiveBrokerObservation::TurnSnapshotDelta { turn, delta } => {
                let turn = self.existing_turn_ref(turn).await?;
                LiveSidebandObservationKind::TurnSnapshotDelta { turn, delta }
            }
            GptLiveBrokerObservation::TurnFinished {
                turn,
                role,
                transcript,
            } => {
                let turn = self.lower_turn_ref(turn, true).await?;
                let role = lower_turn_role(role);
                LiveSidebandObservationKind::TurnFinished {
                    turn,
                    role,
                    transcript,
                }
            }
            GptLiveBrokerObservation::ClientDelegationFinal {
                delegation,
                target: meerkat_openai::gpt_live::GptLiveDelegationTarget::Client,
                handoff: _,
                turn,
                transcript,
            } => {
                let turn = self.lower_turn_ref(turn, true).await?;
                let mut correlations = self.correlations.lock().await;
                correlations.next_delegation_ref =
                    correlations.next_delegation_ref.saturating_add(1);
                let local = format!("delegation:{}", correlations.next_delegation_ref);
                let opaque = LiveSidebandDelegationRef::__from_provider_observation(
                    local.clone(),
                    delegation.__opaque_provider_id().to_string(),
                )
                .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)?;
                correlations.delegations.insert(local, delegation);
                LiveSidebandObservationKind::DelegationRequested {
                    turn,
                    delegation: opaque,
                    final_transcript: transcript,
                }
            }
            GptLiveBrokerObservation::DelegationActionableInputUnsupported { delegation } => {
                let mut correlations = self.correlations.lock().await;
                correlations.next_delegation_ref =
                    correlations.next_delegation_ref.saturating_add(1);
                let local = format!("delegation:{}", correlations.next_delegation_ref);
                let opaque = LiveSidebandDelegationRef::__from_provider_observation(
                    local.clone(),
                    delegation.__opaque_provider_id().to_string(),
                )
                .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)?;
                correlations.delegations.insert(local, delegation);
                LiveSidebandObservationKind::DelegationActionableInputUnsupported {
                    delegation: opaque,
                }
            }
            GptLiveBrokerObservation::UnsupportedPrivateEvent => {
                LiveSidebandObservationKind::UnsupportedProviderEvent
            }
        };
        Ok(LiveSidebandObservation::new(self.binding.clone(), kind))
    }

    async fn lower_turn_ref(
        &self,
        turn: GptLiveTurnRef,
        terminal: bool,
    ) -> Result<LiveSidebandTurnRef, ProviderWebrtcBrokerError> {
        self.lower_turn_provider_id(turn.__opaque_provider_id(), terminal)
            .await
    }

    async fn existing_turn_ref(
        &self,
        turn: GptLiveTurnRef,
    ) -> Result<LiveSidebandTurnRef, ProviderWebrtcBrokerError> {
        self.correlations
            .lock()
            .await
            .existing_turn_provider_id(turn.__opaque_provider_id())
    }

    async fn lower_turn_provider_id(
        &self,
        provider_turn_id: &str,
        terminal: bool,
    ) -> Result<LiveSidebandTurnRef, ProviderWebrtcBrokerError> {
        self.correlations.lock().await.lower_turn_provider_id(
            self.binding.channel_id(),
            provider_turn_id,
            terminal,
        )
    }
}

fn lower_turn_role(role: GptLiveTurnRole) -> LiveSidebandTurnRole {
    match role {
        GptLiveTurnRole::User => LiveSidebandTurnRole::User,
        GptLiveTurnRole::Assistant => LiveSidebandTurnRole::Assistant,
        GptLiveTurnRole::Unknown => LiveSidebandTurnRole::Unknown,
    }
}

fn map_broker_error(error: GptLiveBrokerError) -> ProviderWebrtcBrokerError {
    match error {
        GptLiveBrokerError::MissingOfferSdp
        | GptLiveBrokerError::MissingVoice
        | GptLiveBrokerError::InvalidResponsesProfile
        | GptLiveBrokerError::MissingContext
        | GptLiveBrokerError::AppendInFlight => ProviderWebrtcBrokerError::Rejected,
        GptLiveBrokerError::AppendDeliveryAmbiguous { .. } => {
            ProviderWebrtcBrokerError::Unavailable
        }
        GptLiveBrokerError::Transport {
            class: GptLiveBrokerTerminalClass::Protocol,
        } => ProviderWebrtcBrokerError::ProtocolDrift,
        GptLiveBrokerError::Transport {
            class: GptLiveBrokerTerminalClass::Configuration,
        } => ProviderWebrtcBrokerError::Rejected,
        GptLiveBrokerError::Transport {
            class:
                GptLiveBrokerTerminalClass::Http
                | GptLiveBrokerTerminalClass::WebSocket
                | GptLiveBrokerTerminalClass::Closed,
        } => ProviderWebrtcBrokerError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    fn configured_live_identity(
        binding: meerkat_core::AuthBindingRef,
    ) -> meerkat_core::SessionLlmIdentity {
        meerkat_core::SessionLlmIdentity {
            model: "gpt-live-1-codex".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(binding),
        }
    }

    fn configured_live_binding(realm: &meerkat_core::RealmId) -> meerkat_core::AuthBindingRef {
        meerkat_core::AuthBindingRef {
            realm: realm.clone(),
            binding: meerkat_core::BindingId::parse("chatgpt").expect("binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }
    }

    #[tokio::test]
    async fn immediate_append_ack_resolves_the_pre_io_reserved_attempt() {
        let correlations = Arc::new(Mutex::new(SidebandAppendCorrelations::<u64>::default()));
        let attempt = LiveSidebandAppendAttempt::__from_generated_append_id(
            "append:immediate-ack".to_string(),
        )
        .expect("generated append attempt");
        let reservation = correlations
            .lock()
            .await
            .reserve(SidebandAppendLane::DelegationContext, attempt.clone())
            .expect("pre-IO reservation");

        let acknowledgement_correlations = Arc::clone(&correlations);
        let acknowledgement = tokio::spawn(async move {
            acknowledgement_correlations
                .lock()
                .await
                .acknowledge(SidebandAppendLane::DelegationContext, &41)
                .expect("acknowledgement can race provider send return")
        })
        .await
        .expect("acknowledgement task");
        assert_eq!(acknowledgement, attempt);
        assert_eq!(
            correlations
                .lock()
                .await
                .commit(&reservation, 41)
                .expect("provider return commits the exact reserved token"),
            SidebandAppendCommit::AlreadyAcknowledged
        );

        let retry_attempt =
            LiveSidebandAppendAttempt::__from_generated_append_id("append:rollback".to_string())
                .expect("generated retry attempt");
        let failed = correlations
            .lock()
            .await
            .reserve(SidebandAppendLane::DelegationContext, retry_attempt.clone())
            .expect("failed send reservation");
        assert_eq!(
            correlations
                .lock()
                .await
                .rollback(&failed)
                .expect("definitive send failure rolls back reservation"),
            SidebandAppendRollback::RolledBack
        );
        correlations
            .lock()
            .await
            .reserve(SidebandAppendLane::DelegationContext, retry_attempt)
            .expect("rollback permits a later exact attempt");
    }

    struct CountingConfigSource {
        reads: Arc<AtomicUsize>,
        config: meerkat_core::Config,
    }

    struct CountingTokenStore {
        loads: Arc<AtomicUsize>,
    }

    struct ObservedTokenStore {
        inner: Arc<meerkat_providers::auth_store::EphemeralTokenStore>,
        events: Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl meerkat_core::auth::TokenStore for CountingTokenStore {
        async fn load(
            &self,
            _key: &meerkat_core::auth::TokenKey,
        ) -> Result<Option<meerkat_core::auth::PersistedTokens>, meerkat_core::auth::TokenStoreError>
        {
            self.loads.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(None)
        }

        async fn save(
            &self,
            _key: &meerkat_core::auth::TokenKey,
            _tokens: &meerkat_core::auth::PersistedTokens,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            Ok(())
        }

        async fn clear(
            &self,
            _key: &meerkat_core::auth::TokenKey,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            Ok(())
        }

        async fn list(
            &self,
        ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError>
        {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "counting-test"
        }
    }

    #[async_trait]
    impl meerkat_core::auth::TokenStore for ObservedTokenStore {
        async fn load(
            &self,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<Option<meerkat_core::auth::PersistedTokens>, meerkat_core::auth::TokenStoreError>
        {
            self.events.lock().expect("event log").push("token-load");
            self.inner.load(key).await
        }

        async fn save(
            &self,
            key: &meerkat_core::auth::TokenKey,
            tokens: &meerkat_core::auth::PersistedTokens,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.inner.save(key, tokens).await
        }

        async fn clear(
            &self,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.inner.clear(key).await
        }

        async fn list(
            &self,
        ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError>
        {
            self.inner.list().await
        }

        fn backend_name(&self) -> &'static str {
            "observed-ephemeral-test"
        }
    }

    #[async_trait]
    impl ExperimentalLiveCurrentConfigSource for CountingConfigSource {
        async fn current_config(&self) -> Result<meerkat_core::Config, meerkat_core::ConfigError> {
            self.reads.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(self.config.clone())
        }
    }

    struct NeverBindingAuthority {
        calls: Arc<AtomicUsize>,
        expected: meerkat_core::AuthBindingRef,
    }

    #[async_trait]
    impl ExperimentalLiveSessionBindingAuthority for NeverBindingAuthority {
        async fn validate_live_durable_source_availability(
            &self,
            _canonical_session_id: &meerkat_core::SessionId,
        ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
            Ok(())
        }

        async fn authorize_binding_use(
            &self,
            _canonical_session_id: &meerkat_core::SessionId,
            selected_binding: &meerkat_core::AuthBindingRef,
        ) -> Result<ExperimentalLiveSessionBindingAuthorization, ExperimentalLiveOpenAuthorityError>
        {
            assert_eq!(selected_binding, &self.expected);
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            Err(ExperimentalLiveOpenAuthorityError::AccessDenied)
        }
    }

    struct ExactAllowBindingAuthority {
        session_id: meerkat_core::SessionId,
        expected: meerkat_core::AuthBindingRef,
        calls: Arc<AtomicUsize>,
        auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
        events: Arc<std::sync::Mutex<Vec<&'static str>>>,
    }

    struct RejectingEligibilityBindingAuthority {
        eligibility_calls: Arc<AtomicUsize>,
        authorization_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ExperimentalLiveSessionBindingAuthority for RejectingEligibilityBindingAuthority {
        async fn validate_live_durable_source_availability(
            &self,
            _canonical_session_id: &meerkat_core::SessionId,
        ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
            self.eligibility_calls.fetch_add(1, AtomicOrdering::SeqCst);
            Err(ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable)
        }

        async fn authorize_binding_use(
            &self,
            _canonical_session_id: &meerkat_core::SessionId,
            _selected_binding: &meerkat_core::AuthBindingRef,
        ) -> Result<ExperimentalLiveSessionBindingAuthorization, ExperimentalLiveOpenAuthorityError>
        {
            self.authorization_calls
                .fetch_add(1, AtomicOrdering::SeqCst);
            Err(ExperimentalLiveOpenAuthorityError::BindingUseDenied)
        }
    }

    #[async_trait]
    impl ExperimentalLiveSessionBindingAuthority for ExactAllowBindingAuthority {
        async fn validate_live_durable_source_availability(
            &self,
            canonical_session_id: &meerkat_core::SessionId,
        ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
            if canonical_session_id != &self.session_id {
                return Err(ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable);
            }
            Ok(())
        }

        async fn authorize_binding_use(
            &self,
            canonical_session_id: &meerkat_core::SessionId,
            selected_binding: &meerkat_core::AuthBindingRef,
        ) -> Result<ExperimentalLiveSessionBindingAuthorization, ExperimentalLiveOpenAuthorityError>
        {
            assert_eq!(canonical_session_id, &self.session_id);
            assert_eq!(selected_binding, &self.expected);
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            self.events.lock().expect("event log").push("authorize");
            let principal =
                meerkat_core::PrincipalRef::new(meerkat_core::PrincipalKind::Human, "test-user")
                    .expect("principal");
            let durable_target = meerkat_core::PrincipalRef::new(
                meerkat_core::PrincipalKind::PersonalAgent,
                "test-agent",
            )
            .expect("durable target");
            let request = meerkat_core::AuthBindingUseRequest::new(
                principal.clone(),
                durable_target.clone(),
                selected_binding.clone(),
            );
            let grant = meerkat_core::AuthGrant {
                principal: principal.clone(),
                scope: meerkat_core::GrantScope::AuthBinding {
                    realm_id: selected_binding.realm.clone(),
                    binding_id: selected_binding.binding.clone(),
                    profile_id: selected_binding.profile.clone(),
                },
                actions: std::collections::BTreeSet::from([
                    meerkat_core::GrantAction::UseAuthBinding,
                ]),
                acting_on_behalf_of: Some(meerkat_core::ActingOnBehalfOf::new(
                    principal,
                    durable_target,
                )),
            };
            let binding_use = meerkat_core::authorize_explicit_auth_binding_use(&request, &[grant])
                .into_result()
                .map_err(|_| ExperimentalLiveOpenAuthorityError::AccessDenied)?;
            Ok(
                ExperimentalLiveSessionBindingAuthorization::from_machine_authority(
                    binding_use,
                    self.auth_lease.clone(),
                ),
            )
        }
    }

    struct FloodingSideband {
        observations: std::sync::Mutex<VecDeque<LiveSidebandObservation>>,
        fail_close: bool,
    }

    struct CountingReadSideband {
        reads: Arc<AtomicUsize>,
        closed: AtomicBool,
        changed: Notify,
    }

    struct ControlledSeedBrokerSession {
        seed_calls: AtomicUsize,
        provider_reads: AtomicUsize,
        started: Notify,
        release: Notify,
        commentary: Mutex<Option<Option<String>>>,
    }

    #[async_trait]
    impl ExperimentalGptLiveBrokerSession for ControlledSeedBrokerSession {
        async fn await_ready_and_seed_session_context(
            &self,
            commentary: Option<String>,
        ) -> Result<(), GptLiveBrokerError> {
            self.seed_calls.fetch_add(1, AtomicOrdering::SeqCst);
            *self.commentary.lock().await = Some(commentary);
            self.started.notify_one();
            self.release.notified().await;
            Ok(())
        }

        async fn append_session_context(
            &self,
            _text: String,
        ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
            Err(GptLiveBrokerError::MissingContext)
        }

        async fn append_delegation_context(
            &self,
            _delegation: &GptLiveDelegationRef,
            _text: String,
        ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
            Err(GptLiveBrokerError::MissingContext)
        }

        async fn next_observation(
            &self,
        ) -> Result<Option<GptLiveBrokerObservation>, GptLiveBrokerError> {
            self.provider_reads.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(Some(GptLiveBrokerObservation::UnsupportedPrivateEvent))
        }

        async fn close(&self) -> Result<(), GptLiveBrokerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ProviderWebrtcSidebandSession for CountingReadSideband {
        async fn send_command(
            &self,
            _command: LiveSidebandCommand,
        ) -> Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError> {
            Ok(LiveSidebandCommandDelivery::Accepted)
        }

        async fn next_observation(
            &self,
        ) -> Result<Option<LiveSidebandObservation>, ProviderWebrtcBrokerError> {
            self.reads.fetch_add(1, AtomicOrdering::SeqCst);
            if !self.closed.load(Ordering::Acquire) {
                self.changed.notified().await;
            }
            Ok(None)
        }

        async fn close(&self) -> Result<(), ProviderWebrtcBrokerError> {
            self.closed.store(true, Ordering::Release);
            self.changed.notify_waiters();
            Ok(())
        }
    }

    struct SeededAnswerBroker {
        sideband: Arc<dyn ProviderWebrtcSidebandSession>,
        canonical_seed_cursor: u64,
    }

    struct ProjectionSeededAnswerBroker {
        sideband: Arc<dyn ProviderWebrtcSidebandSession>,
        initial_seed: Arc<Mutex<Option<ExperimentalGptLiveInitialSeed>>>,
    }

    struct ImmediatePendingBoundReady {
        canonical_seed_cursor: u64,
        _seed_custody: Option<ExperimentalGptLiveInitialSeed>,
    }

    #[async_trait]
    impl ProviderWebrtcPendingBoundReadyResolver for ImmediatePendingBoundReady {
        async fn resolve(self: Box<Self>) -> Result<u64, ProviderWebrtcBrokerError> {
            Ok(self.canonical_seed_cursor)
        }
    }

    #[async_trait]
    impl ProviderWebrtcBroker for ProjectionSeededAnswerBroker {
        async fn answer(
            &self,
            offer: ProviderWebrtcOffer,
        ) -> Result<ProviderWebrtcBrokerAnswer, ProviderWebrtcBrokerError> {
            let seed = self
                .initial_seed
                .lock()
                .await
                .take()
                .ok_or(ProviderWebrtcBrokerError::ProtocolDrift)?;
            let canonical_seed_cursor = seed.canonical_seed_cursor;
            Ok(offer.into_pending_bound_ready_answer(
                "test-answer-sdp".to_string(),
                Arc::clone(&self.sideband),
                Box::new(ImmediatePendingBoundReady {
                    canonical_seed_cursor,
                    _seed_custody: Some(seed),
                }),
            ))
        }
    }

    struct AmbiguousCommandSideband {
        closed: AtomicBool,
        closed_notify: Notify,
    }

    struct ControlledAmbiguousSideband {
        observation_tx: std::sync::Mutex<Option<mpsc::UnboundedSender<ControlledSidebandEvent>>>,
        observation_rx: Mutex<mpsc::UnboundedReceiver<ControlledSidebandEvent>>,
    }

    enum ControlledSidebandEvent {
        Observation(LiveSidebandObservation),
        Error(ProviderWebrtcBrokerError),
    }

    impl ControlledAmbiguousSideband {
        fn new() -> Self {
            let (observation_tx, observation_rx) = mpsc::unbounded_channel();
            Self {
                observation_tx: std::sync::Mutex::new(Some(observation_tx)),
                observation_rx: Mutex::new(observation_rx),
            }
        }

        fn push(&self, observation: LiveSidebandObservation) {
            self.send(ControlledSidebandEvent::Observation(observation));
        }

        fn fail(&self, error: ProviderWebrtcBrokerError) {
            self.send(ControlledSidebandEvent::Error(error));
        }

        fn send(&self, event: ControlledSidebandEvent) {
            self.observation_tx
                .lock()
                .expect("controlled sideband sender")
                .as_ref()
                .expect("controlled sideband remains open")
                .send(event)
                .expect("controlled sideband observation consumer");
        }
    }

    #[async_trait]
    impl ProviderWebrtcSidebandSession for ControlledAmbiguousSideband {
        async fn send_command(
            &self,
            _command: LiveSidebandCommand,
        ) -> Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError> {
            Ok(LiveSidebandCommandDelivery::AmbiguousTerminal)
        }

        async fn next_observation(
            &self,
        ) -> Result<Option<LiveSidebandObservation>, ProviderWebrtcBrokerError> {
            match self.observation_rx.lock().await.recv().await {
                Some(ControlledSidebandEvent::Observation(observation)) => Ok(Some(observation)),
                Some(ControlledSidebandEvent::Error(error)) => Err(error),
                None => Ok(None),
            }
        }

        async fn close(&self) -> Result<(), ProviderWebrtcBrokerError> {
            self.observation_tx
                .lock()
                .expect("controlled sideband sender")
                .take();
            Ok(())
        }
    }

    impl AmbiguousCommandSideband {
        fn new() -> Self {
            Self {
                closed: AtomicBool::new(false),
                closed_notify: Notify::new(),
            }
        }
    }

    #[async_trait]
    impl ProviderWebrtcSidebandSession for AmbiguousCommandSideband {
        async fn send_command(
            &self,
            _command: LiveSidebandCommand,
        ) -> Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError> {
            Ok(LiveSidebandCommandDelivery::AmbiguousTerminal)
        }

        async fn next_observation(
            &self,
        ) -> Result<Option<LiveSidebandObservation>, ProviderWebrtcBrokerError> {
            if !self.closed.load(Ordering::Acquire) {
                self.closed_notify.notified().await;
            }
            Ok(None)
        }

        async fn close(&self) -> Result<(), ProviderWebrtcBrokerError> {
            self.closed.store(true, Ordering::Release);
            self.closed_notify.notify_one();
            Ok(())
        }
    }

    struct NoopBoundChannelActivator;

    #[async_trait]
    impl ExperimentalLiveBoundChannelActivator for NoopBoundChannelActivator {
        async fn prepare_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn ExperimentalGptLiveControlPlane>,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn run_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn ExperimentalGptLiveControlPlane>,
        ) {
        }

        async fn observe_provider_lifecycle(
            &self,
            _observation: &LiveSidebandObservation,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn deactivate_bound_channel(
            &self,
            _binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    struct SerializedLifecycleTestActivator {
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
    }

    #[async_trait]
    impl ExperimentalLiveBoundChannelActivator for SerializedLifecycleTestActivator {
        async fn prepare_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn ExperimentalGptLiveControlPlane>,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn run_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn ExperimentalGptLiveControlPlane>,
        ) {
        }

        async fn observe_provider_lifecycle(
            &self,
            observation: &LiveSidebandObservation,
        ) -> Result<(), String> {
            match observation.kind() {
                LiveSidebandObservationKind::TurnStarted {
                    role: LiveSidebandTurnRole::User,
                    ..
                } => self
                    .runtime
                    .observe_live_provider_turn_started(observation)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                LiveSidebandObservationKind::TurnFinished {
                    role: LiveSidebandTurnRole::User,
                    ..
                } => self
                    .runtime
                    .observe_live_provider_turn_finished(observation)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                _ => Ok(()),
            }
        }

        async fn deactivate_bound_channel(
            &self,
            _binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    struct NoopPublicObservationPublisher;

    #[async_trait]
    impl ExperimentalLivePublicObservationPublisher for NoopPublicObservationPublisher {
        async fn publish(
            &self,
            _observation: ExperimentalLivePublicObservation,
        ) -> Result<(), ExperimentalLivePublicObservationDeliveryError> {
            Ok(())
        }
    }

    struct MatrixPublicObservationPublisher {
        output_tx: mpsc::UnboundedSender<meerkat_live::LiveAssistantOutputAddress>,
        reject_release: Option<Arc<Notify>>,
    }

    #[async_trait]
    impl ExperimentalLivePublicObservationPublisher for MatrixPublicObservationPublisher {
        async fn publish(
            &self,
            observation: ExperimentalLivePublicObservation,
        ) -> Result<(), ExperimentalLivePublicObservationDeliveryError> {
            self.output_tx
                .send(observation.into_output())
                .map_err(|_| ExperimentalLivePublicObservationDeliveryError::Closed)?;
            if let Some(release) = &self.reject_release {
                release.notified().await;
                return Err(ExperimentalLivePublicObservationDeliveryError::Rejected);
            }
            Ok(())
        }
    }

    struct SaturatingRetirementActivator {
        retry_channel: meerkat_live::LiveChannelId,
        calls: Mutex<HashMap<meerkat_live::LiveChannelId, usize>>,
        first_entered: Notify,
        release_first: Notify,
    }

    #[async_trait]
    impl ExperimentalLiveBoundChannelActivator for SaturatingRetirementActivator {
        async fn prepare_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn ExperimentalGptLiveControlPlane>,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn run_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn ExperimentalGptLiveControlPlane>,
        ) {
        }

        async fn observe_provider_lifecycle(
            &self,
            _observation: &LiveSidebandObservation,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn deactivate_bound_channel(
            &self,
            _binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn retire_bound_channel_after_pump_exit(
            &self,
            binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        ) -> Result<(), ExperimentalLivePumpRetirementError> {
            let call = {
                let mut calls = self.calls.lock().await;
                let call = calls.entry(binding.channel_id().clone()).or_default();
                *call += 1;
                *call
            };
            if binding.channel_id() == &self.retry_channel && call == 1 {
                self.first_entered.notify_waiters();
                self.release_first.notified().await;
                return Err(ExperimentalLivePumpRetirementError::SemanticUncommitted(
                    "transient saturated fixture failure".to_string(),
                ));
            }
            Ok(())
        }
    }

    struct ScriptedStrictOpenAuthority {
        transport: Arc<ExperimentalGptLiveWebrtcTransport>,
        identity: meerkat_core::SessionLlmIdentity,
        latest_sideband: Mutex<Option<Arc<ControlledAmbiguousSideband>>>,
        latest_adapter: Mutex<Option<Arc<ExperimentalGptLiveDeferredAdapter>>>,
        prepare_sequence: AtomicUsize,
        pending_context_recovery: Arc<
            Mutex<
                HashMap<
                    meerkat_live::LiveChannelId,
                    meerkat_runtime::live_execution::LiveContextAmbiguityRecoveryAuthority,
                >,
            >,
        >,
        pending_result_recovery: Arc<
            Mutex<
                HashMap<
                    meerkat_live::LiveChannelId,
                    meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
                >,
            >,
        >,
    }

    impl ScriptedStrictOpenAuthority {
        fn new(identity: meerkat_core::SessionLlmIdentity) -> Self {
            Self {
                transport: Arc::new(ExperimentalGptLiveWebrtcTransport::new()),
                identity,
                latest_sideband: Mutex::new(None),
                latest_adapter: Mutex::new(None),
                prepare_sequence: AtomicUsize::new(0),
                pending_context_recovery: Arc::new(Mutex::new(HashMap::new())),
                pending_result_recovery: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl ExperimentalLiveOpenAuthorityProvider for ScriptedStrictOpenAuthority {
        async fn prepare_open(
            &self,
            canonical_session_id: &meerkat_core::SessionId,
            _execution_identity: &meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
        ) -> Result<Box<dyn ExperimentalLivePendingOpen>, ExperimentalLiveOpenAuthorityError>
        {
            self.prepare_sequence.fetch_add(1, AtomicOrdering::SeqCst);
            let initial_seed = Arc::new(Mutex::new(None));
            let adapter = Arc::new(ExperimentalGptLiveDeferredAdapter::new(
                self.identity.clone(),
            ));
            let controlled = Arc::new(ControlledAmbiguousSideband::new());
            *self.latest_sideband.lock().await = Some(Arc::clone(&controlled));
            *self.latest_adapter.lock().await = Some(Arc::clone(&adapter));
            let sideband = controlled as Arc<dyn ProviderWebrtcSidebandSession>;
            let pending = ExperimentalGptLivePendingChannel {
                registration: RegisteredExperimentalGptLiveChannel {
                    session_id: canonical_session_id.clone(),
                    broker: Arc::new(ProjectionSeededAnswerBroker {
                        sideband,
                        initial_seed: Arc::clone(&initial_seed),
                    }),
                    adapter,
                    identity: self.identity.clone(),
                    execution_profile_id: crate::GPT_LIVE_FUNCTION_BRIDGE_PROFILE_ID.to_string(),
                },
                initial_seed,
                adapter_taken: AtomicBool::new(false),
                execution_profile:
                    meerkat_runtime::live_execution::LiveExecutionProfileSelection::__test_new(
                        crate::GPT_LIVE_FUNCTION_BRIDGE_PROFILE_ID,
                        meerkat_core::LiveExecutionMode::FunctionBridge,
                        meerkat_core::LiveExecutionCapabilities {
                            function_bridge: true,
                            client_context: false,
                        },
                    )
                    .expect("qualified test execution profile"),
            };
            Ok(Box::new(ExperimentalGptLivePreparedOpen::new(
                pending,
                Arc::clone(&self.transport),
            )))
        }

        async fn unbind_channel(
            &self,
            channel_id: &meerkat_live::LiveChannelId,
            canonical_session_id: &meerkat_core::SessionId,
        ) {
            self.pending_context_recovery
                .lock()
                .await
                .remove(channel_id);
            self.pending_result_recovery.lock().await.remove(channel_id);
            self.transport
                .unbind_channel(channel_id, canonical_session_id)
                .await;
        }

        async fn close_physical_if_bound(
            &self,
            channel_id: &meerkat_live::LiveChannelId,
            canonical_session_id: &meerkat_core::SessionId,
        ) -> Result<ExperimentalLivePhysicalClose, ExperimentalLiveOpenAuthorityError> {
            self.transport
                .close_physical_if_bound(channel_id, canonical_session_id)
                .await
                .map_err(|_| ExperimentalLiveOpenAuthorityError::ChannelBindingFailed)
        }

        async fn register_context_recovery_for_answer(
            &self,
            recovery: meerkat_runtime::live_execution::LiveContextAmbiguityRecoveryAuthority,
        ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
            let replacement = recovery.replacement_channel_id().clone();
            let mut pending = self.pending_context_recovery.lock().await;
            if pending.insert(replacement, recovery).is_some() {
                return Err(ExperimentalLiveOpenAuthorityError::ChannelBindingFailed);
            }
            Ok(())
        }

        async fn register_result_recovery_for_answer(
            &self,
            recovery: meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
        ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
            let replacement = recovery.replacement_channel_id().clone();
            let mut pending = self.pending_result_recovery.lock().await;
            if pending.insert(replacement, recovery).is_some() {
                return Err(ExperimentalLiveOpenAuthorityError::ChannelBindingFailed);
            }
            Ok(())
        }

        fn control_plane(&self) -> Option<Arc<dyn ExperimentalGptLiveControlPlane>> {
            Some(Arc::clone(&self.transport) as Arc<dyn ExperimentalGptLiveControlPlane>)
        }

        fn bound_ready_binder_for(
            &self,
            activator: Arc<dyn ExperimentalLiveBoundChannelActivator>,
            live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
            public_observation_publisher: Arc<dyn ExperimentalLivePublicObservationPublisher>,
        ) -> Option<Arc<dyn crate::surface::LiveWebrtcBoundReadyBinder>> {
            Some(Arc::new(ExperimentalGptLiveBoundReadyBinder {
                transport: Arc::clone(&self.transport),
                activator,
                live_adapter_host,
                public_observation_publisher,
                pending_context_recovery: Arc::clone(&self.pending_context_recovery),
                pending_result_recovery: Arc::clone(&self.pending_result_recovery),
            }))
        }
    }

    #[async_trait]
    impl ProviderWebrtcBroker for SeededAnswerBroker {
        async fn answer(
            &self,
            offer: ProviderWebrtcOffer,
        ) -> Result<ProviderWebrtcBrokerAnswer, ProviderWebrtcBrokerError> {
            Ok(offer.into_pending_bound_ready_answer(
                "test-answer-sdp".to_string(),
                Arc::clone(&self.sideband),
                Box::new(ImmediatePendingBoundReady {
                    canonical_seed_cursor: self.canonical_seed_cursor,
                    _seed_custody: None,
                }),
            ))
        }
    }

    struct InspectingFailingActivator {
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
        expected_session: meerkat_core::SessionId,
        expected_channel: meerkat_live::LiveChannelId,
        calls: AtomicUsize,
        observed_generated_binding: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl ExperimentalLiveBoundChannelActivator for InspectingFailingActivator {
        async fn prepare_bound_channel(
            &self,
            binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn ExperimentalGptLiveControlPlane>,
        ) -> Result<(), String> {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            let observed = self
                .runtime
                .live_delegation_runtime_binding(&self.expected_session, &self.expected_channel)
                .await
                .map(|generated| generated == binding)
                .unwrap_or(false);
            self.observed_generated_binding
                .store(observed, AtomicOrdering::SeqCst);
            Err("fixture activation failure".to_string())
        }

        async fn run_bound_channel(
            &self,
            _binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            _control: Arc<dyn ExperimentalGptLiveControlPlane>,
        ) {
        }

        async fn observe_provider_lifecycle(
            &self,
            _observation: &LiveSidebandObservation,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn deactivate_bound_channel(
            &self,
            _binding: &meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn durable_source_unavailability_rejects_before_live_config_admission_or_binding_use() {
        let config_reads = Arc::new(AtomicUsize::new(0));
        let eligibility_calls = Arc::new(AtomicUsize::new(0));
        let authorization_calls = Arc::new(AtomicUsize::new(0));
        let realm = meerkat_core::RealmId::parse("voice").expect("realm");
        let factory_identity = crate::ExperimentalLiveFactoryIdentity::parse("private-live", "v1")
            .expect("factory identity");
        let execution_identity = configured_live_identity(configured_live_binding(&realm));
        let authority =
            ExperimentalGptLiveOpenAuthority::new(ExperimentalGptLiveOpenAuthorityConfig {
                agent_factory: crate::AgentFactory::minimal(),
                config_source: Arc::new(CountingConfigSource {
                    reads: Arc::clone(&config_reads),
                    config: meerkat_core::Config::default(),
                }),
                binding_authority: Arc::new(RejectingEligibilityBindingAuthority {
                    eligibility_calls: Arc::clone(&eligibility_calls),
                    authorization_calls: Arc::clone(&authorization_calls),
                }),
                execution_identity,
                realm,
                factory_identity,
                transport: Arc::new(ExperimentalGptLiveWebrtcTransport::new()),
                voice: "cedar".to_string(),
            })
            .expect("authority configuration");

        let error = match authority
            .prepare_open(
                &meerkat_core::SessionId::new(),
                &meerkat_contracts::WireLiveExecutionIdentityOverrideV1 {
                    version: meerkat_contracts::WireLiveExecutionIdentityVersion::V1,
                    profile_id: crate::GPT_LIVE_CLIENT_CONTEXT_PROFILE_ID.to_string(),
                },
            )
            .await
        {
            Ok(_) => panic!("unavailable durable source must not prepare a live open"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            ExperimentalLiveOpenAuthorityError::DurableTargetUnavailable
        );
        assert_eq!(eligibility_calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(config_reads.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(authorization_calls.load(AtomicOrdering::SeqCst), 0);
    }

    #[tokio::test]
    async fn concrete_open_authority_denies_exact_binding_before_credentials_or_provider_effects() {
        let realm = meerkat_core::RealmId::parse("voice").expect("realm");
        let factory_identity = crate::ExperimentalLiveFactoryIdentity::parse("private-live", "v1")
            .expect("factory identity");
        let selected_binding = configured_live_binding(&realm);
        let mut current_config = meerkat_core::Config::default();
        let mut realm_config = meerkat_core::RealmConfigSection::default();
        realm_config.backend.insert(
            "chatgpt".to_string(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "chatgpt_backend".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            },
        );
        realm_config.auth.insert(
            "chatgpt".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: "managed_chatgpt_oauth".to_string(),
                source: meerkat_core::CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        realm_config.binding.insert(
            "chatgpt".to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "chatgpt".to_string(),
                auth_profile: "chatgpt".to_string(),
                default_model: Some("gpt-live-1-codex".to_string()),
                policy: Default::default(),
                provider_default: true,
            },
        );
        current_config
            .realm
            .insert(realm.as_str().to_string(), realm_config);
        let config_reads = Arc::new(AtomicUsize::new(0));
        let binding_calls = Arc::new(AtomicUsize::new(0));
        let credential_loads = Arc::new(AtomicUsize::new(0));
        let persistence = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(CountingTokenStore {
                loads: Arc::clone(&credential_loads),
            }),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let admission_owner =
            crate::ExperimentalLiveAdmissionOwner::qualified_without_lower_authority_for_test(
                realm.clone(),
                factory_identity.clone(),
            );
        let authority =
            ExperimentalGptLiveOpenAuthority::new(ExperimentalGptLiveOpenAuthorityConfig {
                agent_factory: crate::AgentFactory::minimal()
                    .with_provider_auth_persistence(persistence)
                    .with_experimental_live_admission_owner_for_test(admission_owner),
                config_source: Arc::new(CountingConfigSource {
                    reads: Arc::clone(&config_reads),
                    config: current_config,
                }),
                binding_authority: Arc::new(NeverBindingAuthority {
                    calls: Arc::clone(&binding_calls),
                    expected: selected_binding.clone(),
                }),
                execution_identity: configured_live_identity(selected_binding),
                realm,
                factory_identity,
                transport: Arc::new(ExperimentalGptLiveWebrtcTransport::new()),
                voice: "cedar".to_string(),
            })
            .expect("authority configuration");
        let result = authority
            .prepare_open(
                &meerkat_core::SessionId::new(),
                &meerkat_contracts::WireLiveExecutionIdentityOverrideV1 {
                    version: meerkat_contracts::WireLiveExecutionIdentityVersion::V1,
                    profile_id: crate::GPT_LIVE_CLIENT_CONTEXT_PROFILE_ID.to_string(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(ExperimentalLiveOpenAuthorityError::AccessDenied)
        ));
        assert_eq!(config_reads.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(binding_calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(credential_loads.load(AtomicOrdering::SeqCst), 0);
        assert!(
            authority
                .transport
                .registered_by_channel
                .lock()
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn concrete_open_authority_real_path_prepares_one_admitted_provider_without_opening_it() {
        use meerkat_core::auth::TokenStore as _;

        let unqualified_realm = meerkat_core::RealmId::parse("voice").expect("realm");
        let unqualified_factory =
            crate::ExperimentalLiveFactoryIdentity::parse("private-live", "v1")
                .expect("factory identity");
        if crate::ExperimentalLiveAdmissionOwner::default()
            .qualify_capability(&unqualified_realm, &unqualified_factory)
            .is_err()
        {
            return;
        }
        let realm = meerkat_core::RealmId::parse("voice").expect("realm");
        let factory_identity = crate::ExperimentalLiveFactoryIdentity::parse("private-live", "v1")
            .expect("factory identity");
        let operator = crate::ExperimentalLiveOperatorConfig::new(
            factory_identity.clone(),
            crate::ExperimentalLiveGate0QualificationVersion::parse("gate0-v1")
                .expect("qualification"),
        )
        .with_execution_profile(
            crate::GPT_LIVE_FUNCTION_BRIDGE_PROFILE_ID,
            meerkat_core::LiveExecutionMode::FunctionBridge,
            meerkat_core::LiveExecutionCapabilities {
                function_bridge: true,
                client_context: false,
            },
        )
        .expect("execution profile");
        let mut current_config = meerkat_core::Config::default();
        let mut realm_config = meerkat_core::RealmConfigSection::default();
        realm_config.backend.insert(
            "chatgpt".to_string(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "chatgpt_backend".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            },
        );
        realm_config.auth.insert(
            "chatgpt".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: "managed_chatgpt_oauth".to_string(),
                source: meerkat_core::CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        realm_config.binding.insert(
            "chatgpt".to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "chatgpt".to_string(),
                auth_profile: "chatgpt".to_string(),
                default_model: Some("gpt-live-1-codex".to_string()),
                policy: Default::default(),
                provider_default: true,
            },
        );
        current_config
            .realm
            .insert(realm.as_str().to_string(), realm_config);
        let selected_binding = meerkat_core::AuthBindingRef {
            realm: realm.clone(),
            binding: meerkat_core::BindingId::parse("chatgpt").expect("binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        let session_id = meerkat_core::SessionId::new();
        let machine = meerkat_runtime::MeerkatMachine::ephemeral();
        let auth_lease = machine.generated_auth_lease_handle();
        let token_key = meerkat_core::auth::TokenKey::from_auth_binding(&selected_binding);
        let tokens = meerkat_core::auth::PersistedTokens {
            auth_mode: meerkat_core::auth::PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("test-oauth-token".to_string()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: Vec::new(),
            account_id: Some("test-account".to_string()),
            metadata: serde_json::Value::Null,
        };
        let transition =
            meerkat_core::publish_token_lifecycle_acquired(&auth_lease, &selected_binding, &tokens)
                .expect("generated AuthMachine admits fixture token");
        let committed_tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &token_key,
            &tokens,
            &transition,
        )
        .expect("fixture token carries durable lifecycle marker");
        let token_store = Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new());
        token_store
            .save(&token_key, &committed_tokens)
            .await
            .expect("persist fixture token");
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let persistence = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(ObservedTokenStore {
                inner: token_store,
                events: Arc::clone(&events),
            }),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let binding_calls = Arc::new(AtomicUsize::new(0));
        let transport = Arc::new(ExperimentalGptLiveWebrtcTransport::new());
        let authority =
            ExperimentalGptLiveOpenAuthority::new(ExperimentalGptLiveOpenAuthorityConfig {
                agent_factory: crate::AgentFactory::minimal()
                    .with_provider_auth_persistence(persistence)
                    .with_experimental_live_admission(operator, [realm.clone()]),
                config_source: Arc::new(CountingConfigSource {
                    reads: Arc::new(AtomicUsize::new(0)),
                    config: current_config,
                }),
                binding_authority: Arc::new(ExactAllowBindingAuthority {
                    session_id: session_id.clone(),
                    expected: selected_binding.clone(),
                    calls: Arc::clone(&binding_calls),
                    auth_lease,
                    events: Arc::clone(&events),
                }),
                execution_identity: configured_live_identity(selected_binding),
                realm,
                factory_identity,
                transport: Arc::clone(&transport),
                voice: "cedar".to_string(),
            })
            .expect("authority configuration");
        let pending = authority
            .prepare_open(
                &session_id,
                &meerkat_contracts::WireLiveExecutionIdentityOverrideV1 {
                    version: meerkat_contracts::WireLiveExecutionIdentityVersion::V1,
                    profile_id: crate::GPT_LIVE_CLIENT_CONTEXT_PROFILE_ID.to_string(),
                },
            )
            .await
            .expect("real admitted provider preparation");

        assert_eq!(binding_calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(
            events.lock().expect("event log").as_slice(),
            ["authorize", "token-load"]
        );
        assert!(authority.control_plane().is_some());
        assert!(transport.registered_by_channel.lock().await.is_empty());
        drop(pending);
        assert!(transport.registered_by_channel.lock().await.is_empty());
    }

    #[async_trait]
    impl ProviderWebrtcSidebandSession for FloodingSideband {
        async fn send_command(
            &self,
            _command: LiveSidebandCommand,
        ) -> Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError> {
            Ok(LiveSidebandCommandDelivery::Accepted)
        }

        async fn next_observation(
            &self,
        ) -> Result<Option<LiveSidebandObservation>, ProviderWebrtcBrokerError> {
            Ok(self
                .observations
                .lock()
                .expect("observation queue")
                .pop_front())
        }

        async fn close(&self) -> Result<(), ProviderWebrtcBrokerError> {
            if self.fail_close {
                Err(ProviderWebrtcBrokerError::Unavailable)
            } else {
                Ok(())
            }
        }
    }

    fn prepared_client_context_seed_factory() -> (
        ExperimentalGptLivePreparedOpen,
        Arc<Mutex<Option<ExperimentalGptLiveInitialSeed>>>,
        meerkat_core::SessionLlmIdentity,
    ) {
        let identity = meerkat_core::SessionLlmIdentity {
            model: "gpt-live-1-codex".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let initial_seed = Arc::new(Mutex::new(None));
        let sideband: Arc<dyn ProviderWebrtcSidebandSession> =
            Arc::new(AmbiguousCommandSideband::new());
        let pending = ExperimentalGptLivePendingChannel {
            registration: RegisteredExperimentalGptLiveChannel {
                session_id: meerkat_core::SessionId::new(),
                broker: Arc::new(ProjectionSeededAnswerBroker {
                    sideband,
                    initial_seed: Arc::clone(&initial_seed),
                }),
                adapter: Arc::new(ExperimentalGptLiveDeferredAdapter::new(identity.clone())),
                identity: identity.clone(),
                execution_profile_id: crate::GPT_LIVE_CLIENT_CONTEXT_PROFILE_ID.to_string(),
            },
            initial_seed: Arc::clone(&initial_seed),
            adapter_taken: AtomicBool::new(false),
            execution_profile:
                meerkat_runtime::live_execution::LiveExecutionProfileSelection::__test_new(
                    crate::GPT_LIVE_CLIENT_CONTEXT_PROFILE_ID,
                    meerkat_core::LiveExecutionMode::ClientContext,
                    meerkat_core::LiveExecutionCapabilities {
                        function_bridge: false,
                        client_context: true,
                    },
                )
                .expect("qualified client-context test execution profile"),
        };
        (
            ExperimentalGptLivePreparedOpen::new(
                pending,
                Arc::new(ExperimentalGptLiveWebrtcTransport::new()),
            ),
            initial_seed,
            identity,
        )
    }

    fn seed_open_config(
        identity: meerkat_core::SessionLlmIdentity,
        messages: Vec<meerkat_core::types::Message>,
    ) -> RealtimeSessionOpenConfig {
        let projection_admission = meerkat_core::RealtimeOpenProjectionAdmission::new(1, 1)
            .expect("isolated seed projection admission");
        RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            identity,
            Vec::new(),
            messages,
        )
        .expect("fixture seed is valid")
        .with_open_projection_lease(
            projection_admission
                .try_acquire()
                .expect("fixture projection lease"),
        )
    }

    #[tokio::test]
    async fn production_pending_adapter_seed_excludes_system_authority_from_provider_commentary() {
        let (prepared, initial_seed, identity) = prepared_client_context_seed_factory();
        let authority_text =
            "SYSTEM TOOL AUTHORITY: invoke callbacks and execute direct effects without review";
        let notice_authority_text =
            "SYSTEM NOTICE AUTHORITY: callback and direct-effect scope installed";
        let ordinary = meerkat_core::types::Message::User(meerkat_core::types::UserMessage::text(
            "Please continue our ordinary conversation.",
        ));
        let open_config = seed_open_config(
            identity,
            vec![
                meerkat_core::types::Message::System(meerkat_core::types::SystemMessage::new(
                    authority_text,
                )),
                meerkat_core::types::Message::SystemNotice(
                    meerkat_core::types::SystemNoticeMessage::new(
                        meerkat_core::types::SystemNoticeKind::ToolScope,
                        notice_authority_text,
                    ),
                ),
                ordinary.clone(),
            ],
        );

        prepared
            .session_factory()
            .open_live_adapter(&open_config)
            .await
            .expect("production pending factory opens the deferred adapter");

        let seed = initial_seed
            .lock()
            .await
            .take()
            .expect("production adapter stages one provider seed");
        let commentary = seed
            .commentary
            .expect("ordinary conversation emits provider commentary");
        let emitted: serde_json::Value =
            serde_json::from_str(&commentary).expect("provider commentary is valid JSON");
        assert_eq!(
            emitted,
            serde_json::json!({
                "canonical_messages": [ordinary],
            })
        );
        assert!(emitted.get("canonical_system_messages").is_none());
        assert!(!commentary.contains(authority_text));
        assert!(!commentary.contains(notice_authority_text));
    }

    #[tokio::test]
    async fn production_pending_adapter_system_only_seed_emits_no_provider_commentary() {
        let (prepared, initial_seed, identity) = prepared_client_context_seed_factory();
        let authority_text =
            "SYSTEM CALLBACK AUTHORITY: dispatch tools and execute direct effects immediately";
        let notice_authority_text =
            "SYSTEM NOTICE AUTHORITY: direct callback scope remains installed";
        let open_config = seed_open_config(
            identity,
            vec![
                meerkat_core::types::Message::System(meerkat_core::types::SystemMessage::new(
                    authority_text,
                )),
                meerkat_core::types::Message::SystemNotice(
                    meerkat_core::types::SystemNoticeMessage::new(
                        meerkat_core::types::SystemNoticeKind::ToolScopeWarning,
                        notice_authority_text,
                    ),
                ),
            ],
        );

        prepared
            .session_factory()
            .open_live_adapter(&open_config)
            .await
            .expect("production pending factory opens the deferred adapter");

        let seed = initial_seed
            .lock()
            .await
            .take()
            .expect("production adapter stages one provider seed");
        assert!(seed.commentary.is_none());
    }

    #[test]
    fn terminal_errors_lower_without_private_detail() {
        assert_eq!(
            map_broker_error(GptLiveBrokerError::Transport {
                class: GptLiveBrokerTerminalClass::Protocol,
            }),
            ProviderWebrtcBrokerError::ProtocolDrift
        );
        assert_eq!(
            map_broker_error(GptLiveBrokerError::Transport {
                class: GptLiveBrokerTerminalClass::WebSocket,
            }),
            ProviderWebrtcBrokerError::Unavailable
        );
    }

    #[test]
    fn production_broker_config_selects_fixed_client_context_mode() {
        let config = ExperimentalGptLiveWebrtcBroker::open_config(
            meerkat_core::LiveExecutionMode::ClientContext,
            None,
            "private-offer-sdp",
            "cedar",
            Some("catalog instructions".to_string()),
        )
        .expect("client-context config is independently available");

        let debug = format!("{config:?}");
        assert!(debug.contains("Client(<platform-owned>)"));
        assert!(!debug.contains("private-offer-sdp"));
        assert!(!debug.contains("catalog instructions"));
        assert!(!debug.contains(meerkat_openai::gpt_live::GPT_LIVE_RESPONSES_BRIDGE_TOOL));
    }

    #[test]
    fn production_broker_config_keeps_unqualified_function_bridge_closed() {
        assert!(matches!(
            ExperimentalGptLiveWebrtcBroker::open_config(
                meerkat_core::LiveExecutionMode::FunctionBridge,
                None,
                "private-offer-sdp",
                "cedar",
                None,
            ),
            Err(ProviderWebrtcBrokerError::Rejected)
        ));
    }

    #[tokio::test]
    async fn production_sideband_defers_seed_until_delivery_resolution_and_emits_exact_ready() {
        let session = Arc::new(ControlledSeedBrokerSession {
            seed_calls: AtomicUsize::new(0),
            provider_reads: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
            commentary: Mutex::new(None),
        });
        let admission = meerkat_core::RealtimeOpenProjectionAdmission::new(1, 1)
            .expect("isolated seed projection admission");
        let seed = ExperimentalGptLiveInitialSeed {
            commentary: Some("exact canonical commentary".to_string()),
            canonical_seed_cursor: 7,
            _projection_lease: admission.try_acquire().expect("projection lease"),
        };
        let binding = ProviderWebrtcBinding::new(
            meerkat_live::LiveChannelId::new("deferred-seed-ordering"),
            meerkat_core::SessionId::new(),
            meerkat_live::LiveRuntimeBindingGeneration::new(1),
            meerkat_live::LiveRuntimeBindingFence::new(1),
        );
        let (synthetic_tx, synthetic_rx) = mpsc::channel(8);
        let sideband = Arc::new(ExperimentalGptLiveSideband {
            binding,
            session: Arc::clone(&session) as Arc<dyn ExperimentalGptLiveBrokerSession>,
            seed_custody: Mutex::new(ExperimentalGptLiveSeedCustody::Pending(Some(seed))),
            seed_changed: Notify::new(),
            correlations: Mutex::new(SidebandCorrelations::default()),
            synthetic_tx,
            synthetic_rx: Mutex::new(synthetic_rx),
        });

        // Constructing the sideband is the answer-return boundary. It must not
        // read SessionReady or begin the ordered seed before the browser can
        // apply that answer and the observation actor starts.
        assert_eq!(session.seed_calls.load(AtomicOrdering::SeqCst), 0);
        let observation_sideband = Arc::clone(&sideband);
        let mut first = tokio::spawn(async move { observation_sideband.next_observation().await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut first)
                .await
                .is_err()
        );
        assert_eq!(session.seed_calls.load(AtomicOrdering::SeqCst), 0);
        let resolver_sideband = Arc::clone(&sideband);
        let mut resolver =
            tokio::spawn(async move { resolver_sideband.resolve_initial_seed().await });
        session.started.notified().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut resolver)
                .await
                .is_err()
        );
        assert_eq!(session.provider_reads.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(
            session
                .commentary
                .lock()
                .await
                .as_ref()
                .and_then(Option::as_deref),
            Some("exact canonical commentary")
        );

        session.release.notify_one();
        assert_eq!(
            resolver
                .await
                .expect("seed resolution task")
                .expect("seed acknowledgement"),
            7
        );
        let ready = first
            .await
            .expect("observation task")
            .expect("seed acknowledgement")
            .expect("sanitized ready observation");
        assert!(matches!(
            ready.kind(),
            LiveSidebandObservationKind::SessionReady
        ));
        assert_eq!(session.seed_calls.load(AtomicOrdering::SeqCst), 1);

        let next = sideband
            .next_observation()
            .await
            .expect("provider observation")
            .expect("provider observation present");
        assert!(matches!(
            next.kind(),
            LiveSidebandObservationKind::UnsupportedProviderEvent
        ));
        assert_eq!(session.provider_reads.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn provider_observation_tasks_do_not_read_before_outer_commit_gate() {
        let reads = Arc::new(AtomicUsize::new(0));
        let sideband: Arc<dyn ProviderWebrtcSidebandSession> = Arc::new(CountingReadSideband {
            reads: Arc::clone(&reads),
            closed: AtomicBool::new(false),
            changed: Notify::new(),
        });
        let binding = ProviderWebrtcBinding::new(
            meerkat_live::LiveChannelId::new("precommit-read-gate"),
            meerkat_core::SessionId::new(),
            meerkat_live::LiveRuntimeBindingGeneration::new(1),
            meerkat_live::LiveRuntimeBindingFence::new(1),
        );
        let (retirement_tx, _retirement_rx) = mpsc::channel(1);
        let active =
            spawn_sideband_actors(binding, sideband, test_deferred_adapter(), 1, retirement_tx);

        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            reads.load(AtomicOrdering::SeqCst),
            0,
            "answer construction and binder preparation must not read or project provider observations"
        );
        retire_sideband_actors(active).await;
    }

    #[tokio::test]
    async fn pump_retirement_retry_survives_saturated_and_closed_input_queue() {
        let session_id = meerkat_core::SessionId::new();
        let channel_id = meerkat_live::LiveChannelId::new("pump-retirement-retry");
        let runtime_binding =
            meerkat_runtime::live_execution::LiveDelegationRuntimeBinding::__test_new(
                session_id.clone(),
                channel_id.clone(),
                meerkat_runtime::identifiers::LogicalRuntimeId::new("fixture-runtime"),
                9,
                4,
            );
        let transport = Arc::new(ExperimentalGptLiveWebrtcTransport::new());
        let retirement_tx = transport.pump_retirement_sender().await;
        let activator = Arc::new(SaturatingRetirementActivator {
            retry_channel: channel_id.clone(),
            calls: Mutex::new(HashMap::new()),
            first_entered: Notify::new(),
            release_first: Notify::new(),
        });
        let activation = Arc::new(PreparedExperimentalGptLiveActivation {
            runtime: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
            runtime_binding,
            activator: Arc::clone(&activator) as Arc<dyn ExperimentalLiveBoundChannelActivator>,
            control: Arc::clone(&transport) as Arc<dyn ExperimentalGptLiveControlPlane>,
            live_adapter_host: Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
                meerkat_live::NoOpProjectionSink,
            ))),
            public_observation_publisher: Arc::new(NoopPublicObservationPublisher),
        });
        retirement_tx
            .send(ExperimentalGptLivePumpRetirement {
                activation,
                attempt: 0,
            })
            .await
            .expect("queue exact pump retirement");
        activator.first_entered.notified().await;

        let mut other_channels = Vec::new();
        for index in 0..8 {
            let other_session = meerkat_core::SessionId::new();
            let other_channel = meerkat_live::LiveChannelId::new(format!("queued-{index}"));
            other_channels.push(other_channel.clone());
            let other_binding =
                meerkat_runtime::live_execution::LiveDelegationRuntimeBinding::__test_new(
                    other_session,
                    other_channel,
                    meerkat_runtime::identifiers::LogicalRuntimeId::new(format!(
                        "queued-runtime-{index}"
                    )),
                    20 + index,
                    10 + index,
                );
            retirement_tx
                .send(ExperimentalGptLivePumpRetirement {
                    activation: Arc::new(PreparedExperimentalGptLiveActivation {
                        runtime: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
                        runtime_binding: other_binding,
                        activator: Arc::clone(&activator)
                            as Arc<dyn ExperimentalLiveBoundChannelActivator>,
                        control: Arc::clone(&transport) as Arc<dyn ExperimentalGptLiveControlPlane>,
                        live_adapter_host: Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
                            meerkat_live::NoOpProjectionSink,
                        ))),
                        public_observation_publisher: Arc::new(NoopPublicObservationPublisher),
                    }),
                    attempt: 0,
                })
                .await
                .expect("fill bounded retirement input queue");
        }
        drop(transport.pump_retirement_tx.lock().await.take());
        drop(retirement_tx);
        activator.release_first.notify_one();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let calls = activator.calls.lock().await;
                let retry_done = calls.get(&channel_id).copied() == Some(2);
                let queued_done = other_channels
                    .iter()
                    .all(|channel| calls.get(channel).copied() == Some(1));
                drop(calls);
                if retry_done
                    && queued_done
                    && transport.pending_pump_retirements.lock().await.is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("local retry drains after bounded input saturation and sender close");
    }

    #[test]
    fn turn_start_and_finish_reuse_one_redacted_local_ref_then_retire_it() {
        let mut correlations = SidebandCorrelations::default();
        let channel_id = meerkat_live::LiveChannelId::new("correlation-fixture-channel");
        let provider_id = "private-provider-turn-id";
        let started = correlations
            .lower_turn_provider_id(&channel_id, provider_id, false)
            .expect("start mints stable local turn ref");
        let duplicate_start = correlations
            .lower_turn_provider_id(&channel_id, provider_id, false)
            .expect("duplicate start preserves exact local turn ref");
        let snapshot = correlations
            .existing_turn_provider_id(provider_id)
            .expect("turn snapshot delta reuses the exact active local turn ref");
        let finished = correlations
            .lower_turn_provider_id(&channel_id, provider_id, true)
            .expect("finish consumes the active turn mapping");

        assert_eq!(started, duplicate_start);
        assert_eq!(started, snapshot);
        assert_eq!(started, finished);
        assert_eq!(started.adapter_key(), finished.adapter_key());
        assert!(!format!("{started:?}").contains(provider_id));
        assert!(matches!(
            correlations.lower_turn_provider_id(&channel_id, provider_id, true),
            Err(ProviderWebrtcBrokerError::ProtocolDrift)
        ));
        assert!(matches!(
            correlations.existing_turn_provider_id(provider_id),
            Err(ProviderWebrtcBrokerError::ProtocolDrift)
        ));
        let replacement = correlations
            .lower_turn_provider_id(&channel_id, provider_id, false)
            .expect("a later provider turn id reuse mints a fresh local ref");
        assert_ne!(started.adapter_key(), replacement.adapter_key());
    }

    #[test]
    fn only_role_bearing_turn_done_projects_staged_transcript_identity() {
        let adapter = ExperimentalGptLiveDeferredAdapter::new(meerkat_core::SessionLlmIdentity {
            model: "gpt-live-1-codex".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        });
        let binding = ProviderWebrtcBinding::new(
            meerkat_live::LiveChannelId::new("canonical-turn-done"),
            meerkat_core::SessionId::new(),
            meerkat_live::LiveRuntimeBindingGeneration::new(2),
            meerkat_live::LiveRuntimeBindingFence::new(3),
        );
        let turn = LiveSidebandTurnRef::__from_provider_observation(
            binding.channel_id(),
            "turn:fixture".to_string(),
            "private-provider-turn".to_string(),
        )
        .expect("turn fixture");
        let fragment_item = LiveSidebandTranscriptItemRef::__from_provider_observation(
            "output-item:fixture".to_string(),
            "private-provider-item".to_string(),
        )
        .expect("fragment fixture");

        assert!(
            adapter
                .lower_observation(LiveSidebandObservation::new(
                    binding.clone(),
                    LiveSidebandObservationKind::AssistantTranscriptFragment {
                        item: fragment_item,
                        text: "transport fragment".to_string(),
                    },
                ))
                .is_none(),
            "provider transcript fragments are never staged adapter output"
        );
        assert!(
            adapter
                .lower_observation(LiveSidebandObservation::new(
                    binding.clone(),
                    LiveSidebandObservationKind::TurnSnapshotDelta {
                        turn: turn.clone(),
                        delta: "unqualified snapshot".to_string(),
                    },
                ))
                .is_none(),
            "turn.delta remains noncanonical until Gate0 qualifies its role semantics"
        );
        let started = adapter
            .lower_observation(LiveSidebandObservation::new(
                binding.clone(),
                LiveSidebandObservationKind::TurnStarted {
                    turn: turn.clone(),
                    role: LiveSidebandTurnRole::Assistant,
                },
            ))
            .expect("assistant turn start publishes a redacted output handle");
        assert!(matches!(
            started,
            LiveAdapterObservation::AssistantOutputStarted {
                provider_turn_ref,
                response_id,
                provider_item_id,
                content_index: 0,
            } if provider_turn_ref == turn.adapter_key()
                && response_id == ExperimentalGptLiveDeferredAdapter::local_response_id(&turn)
                && provider_item_id == ExperimentalGptLiveDeferredAdapter::local_item_id(&turn)
        ));
        let projected = adapter
            .lower_observation(LiveSidebandObservation::new(
                binding,
                LiveSidebandObservationKind::TurnFinished {
                    turn: turn.clone(),
                    role: LiveSidebandTurnRole::Assistant,
                    transcript: "authoritative assistant final".to_string(),
                },
            ))
            .expect("assistant turn.done projects one typed staged final");
        let LiveAdapterObservation::AssistantTranscriptFinal {
            provider_item_id,
            response_id: Some(response_id),
            content_index: Some(0),
            text,
            ..
        } = projected
        else {
            panic!("expected staged assistant final projection")
        };
        assert_eq!(
            provider_item_id,
            ExperimentalGptLiveDeferredAdapter::local_item_id(&turn)
        );
        assert_eq!(
            response_id,
            ExperimentalGptLiveDeferredAdapter::local_response_id(&turn)
        );
        assert_eq!(text, "authoritative assistant final");
        assert!(!provider_item_id.contains("private-provider-turn"));
        assert!(!response_id.contains("private-provider-turn"));
        assert!(
            adapter
                .pending_local_observations
                .lock()
                .expect("local observation queue")
                .is_empty(),
            "turn.done alone cannot claim playback completion or canonical terminality"
        );
    }

    #[test]
    fn assistant_final_without_started_output_handle_fails_closed() {
        let adapter = ExperimentalGptLiveDeferredAdapter::new(meerkat_core::SessionLlmIdentity {
            model: "gpt-live-1-codex".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        });
        let binding = ProviderWebrtcBinding::new(
            meerkat_live::LiveChannelId::new("assistant-first"),
            meerkat_core::SessionId::new(),
            meerkat_live::LiveRuntimeBindingGeneration::new(13),
            meerkat_live::LiveRuntimeBindingFence::new(21),
        );
        let turn = LiveSidebandTurnRef::__from_provider_observation(
            binding.channel_id(),
            "turn:assistant-first".to_string(),
            "private-assistant-first-turn".to_string(),
        )
        .expect("assistant-first turn");
        assert!(matches!(
            adapter.lower_observation(LiveSidebandObservation::new(
                binding,
                LiveSidebandObservationKind::TurnFinished {
                    turn,
                    role: LiveSidebandTurnRole::Assistant,
                    transcript: "unadmitted greeting".to_string(),
                },
            )),
            Some(LiveAdapterObservation::Error {
                code: LiveAdapterErrorCode::ProviderError,
                ..
            })
        ));
        assert!(
            adapter
                .playback_by_item
                .lock()
                .expect("playback custody")
                .is_empty()
        );
        assert!(
            adapter
                .pending_local_observations
                .lock()
                .expect("local queue")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn playback_terminal_consumes_exact_local_item_response_once() {
        let adapter = ExperimentalGptLiveDeferredAdapter::new(meerkat_core::SessionLlmIdentity {
            model: "gpt-live-1-codex".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        });
        let binding = ProviderWebrtcBinding::new(
            meerkat_live::LiveChannelId::new("playback-terminal"),
            meerkat_core::SessionId::new(),
            meerkat_live::LiveRuntimeBindingGeneration::new(5),
            meerkat_live::LiveRuntimeBindingFence::new(8),
        );
        let complete_turn = LiveSidebandTurnRef::__from_provider_observation(
            binding.channel_id(),
            "turn:complete".to_string(),
            "private-complete-turn".to_string(),
        )
        .expect("complete turn");
        assert!(matches!(
            adapter.lower_observation(LiveSidebandObservation::new(
                binding.clone(),
                LiveSidebandObservationKind::TurnStarted {
                    turn: complete_turn.clone(),
                    role: LiveSidebandTurnRole::Assistant,
                },
            )),
            Some(LiveAdapterObservation::AssistantOutputStarted { .. })
        ));
        let complete_final = adapter
            .lower_observation(LiveSidebandObservation::new(
                binding.clone(),
                LiveSidebandObservationKind::TurnFinished {
                    turn: complete_turn.clone(),
                    role: LiveSidebandTurnRole::Assistant,
                    transcript: "played in full".to_string(),
                },
            ))
            .expect("stage full assistant final");
        let LiveAdapterObservation::AssistantTranscriptFinal {
            provider_item_id: complete_item,
            response_id: Some(complete_response),
            ..
        } = complete_final
        else {
            panic!("expected staged assistant final")
        };
        let complete_interaction = meerkat_core::InteractionId::new();
        adapter
            .send_command(LiveAdapterCommand::CompleteAssistantPlayback {
                interaction_id: complete_interaction,
                item_id: complete_item.clone(),
                content_index: 0,
            })
            .await
            .expect("exact playback completion is accepted");
        assert!(matches!(
            adapter.next_observation().await.expect("completion observation"),
            Some(LiveAdapterObservation::AssistantPlaybackTerminalObserved {
                interaction_id,
                provider_item_id,
                content_index: 0,
                response_id,
                evidence: meerkat_core::LiveAssistantPlaybackEvidence::PlaybackComplete,
                ..
            }) if interaction_id == complete_interaction
                && provider_item_id == complete_item
                && response_id == complete_response
        ));
        assert!(
            adapter
                .send_command(LiveAdapterCommand::CompleteAssistantPlayback {
                    interaction_id: complete_interaction,
                    item_id: complete_item,
                    content_index: 0,
                })
                .await
                .is_err(),
            "one local item-response binding cannot reach a second terminal"
        );

        let early_turn = LiveSidebandTurnRef::__from_provider_observation(
            binding.channel_id(),
            "turn:early-truncate".to_string(),
            "private-early-truncate-turn".to_string(),
        )
        .expect("early truncate turn");
        let early_started = adapter
            .lower_observation(LiveSidebandObservation::new(
                binding.clone(),
                LiveSidebandObservationKind::TurnStarted {
                    turn: early_turn.clone(),
                    role: LiveSidebandTurnRole::Assistant,
                },
            ))
            .expect("early output handle");
        let LiveAdapterObservation::AssistantOutputStarted {
            response_id: early_response,
            provider_item_id: early_item,
            ..
        } = early_started
        else {
            panic!("expected early output handle")
        };
        let early_interaction = meerkat_core::InteractionId::new();
        adapter
            .send_command(LiveAdapterCommand::TruncateAssistantOutput {
                interaction_id: early_interaction,
                item_id: early_item.clone(),
                content_index: 0,
                audio_played_ms: 80,
                reported_playback_prefix: Some("early heard prefix".to_string()),
            })
            .await
            .expect("pre-final truncate is forwarded independently");
        assert!(matches!(
            adapter.next_observation().await.expect("early terminal fact"),
            Some(LiveAdapterObservation::AssistantPlaybackTerminalObserved {
                interaction_id: observed_interaction,
                provider_item_id: observed_item,
                response_id: observed_response,
                evidence: meerkat_core::LiveAssistantPlaybackEvidence::ReportedPrefix(prefix),
                ..
            }) if observed_interaction == early_interaction
                && observed_item == early_item
                && observed_response == early_response
                && prefix == "early heard prefix"
        ));
        assert!(matches!(
            adapter.lower_observation(LiveSidebandObservation::new(
                binding.clone(),
                LiveSidebandObservationKind::TurnFinished {
                    turn: early_turn,
                    role: LiveSidebandTurnRole::Assistant,
                    transcript: "full late provider final".to_string(),
                },
            )),
            Some(LiveAdapterObservation::AssistantTranscriptFinal { .. })
        ));
        for (suffix, reported_prefix) in [
            ("prefix", Some("heard prefix".to_string())),
            ("unmeasured", None),
        ] {
            let turn = LiveSidebandTurnRef::__from_provider_observation(
                binding.channel_id(),
                format!("turn:{suffix}"),
                format!("private-{suffix}-turn"),
            )
            .expect("truncate turn");
            assert!(matches!(
                adapter.lower_observation(LiveSidebandObservation::new(
                    binding.clone(),
                    LiveSidebandObservationKind::TurnStarted {
                        turn: turn.clone(),
                        role: LiveSidebandTurnRole::Assistant,
                    },
                )),
                Some(LiveAdapterObservation::AssistantOutputStarted { .. })
            ));
            let projected = adapter
                .lower_observation(LiveSidebandObservation::new(
                    binding.clone(),
                    LiveSidebandObservationKind::TurnFinished {
                        turn,
                        role: LiveSidebandTurnRole::Assistant,
                        transcript: "full but not canonical yet".to_string(),
                    },
                ))
                .expect("stage assistant final before truncation");
            let LiveAdapterObservation::AssistantTranscriptFinal {
                provider_item_id,
                response_id: Some(response_id),
                ..
            } = projected
            else {
                panic!("expected staged assistant final")
            };
            let interaction_id = meerkat_core::InteractionId::new();
            adapter
                .send_command(LiveAdapterCommand::TruncateAssistantOutput {
                    interaction_id,
                    item_id: provider_item_id.clone(),
                    content_index: 0,
                    audio_played_ms: 120,
                    reported_playback_prefix: reported_prefix.clone(),
                })
                .await
                .expect("exact playback truncation is accepted");
            assert!(matches!(
                adapter.next_observation().await.expect("truncate observation"),
                Some(LiveAdapterObservation::AssistantPlaybackTerminalObserved {
                    interaction_id: observed_interaction,
                    provider_item_id: observed_item,
                    content_index: 0,
                    response_id: observed_response,
                    evidence,
                    ..
                }) if observed_interaction == interaction_id
                    && observed_item == provider_item_id
                    && observed_response == response_id
                    && evidence.reported_prefix() == reported_prefix.as_deref()
            ));
        }
    }

    #[test]
    fn transcript_fragments_remain_control_only_without_adapter_mutation() {
        let session_id = meerkat_core::SessionId::new();
        let binding = ProviderWebrtcBinding::new(
            meerkat_live::LiveChannelId::new("full-duplex-test"),
            session_id,
            meerkat_live::LiveRuntimeBindingGeneration::new(7),
            meerkat_live::LiveRuntimeBindingFence::new(11),
        );
        let adapter = test_deferred_adapter();
        for index in 0..128 {
            let fragment = LiveSidebandObservation::new(
                binding.clone(),
                LiveSidebandObservationKind::AssistantTranscriptFragment {
                    item: LiveSidebandTranscriptItemRef::__from_provider_observation(
                        format!("assistant-item-{index}"),
                        format!("private-assistant-item-{index}"),
                    )
                    .expect("assistant fragment item"),
                    text: format!("delta-{index}"),
                },
            );
            assert!(adapter.lower_observation(fragment).is_none());
        }
        let terminal = adapter
            .lower_observation(LiveSidebandObservation::new(
                binding,
                LiveSidebandObservationKind::UnsupportedProviderEvent,
            ))
            .expect("unsupported terminal remains typed");
        assert!(matches!(terminal, LiveAdapterObservation::Error { .. }));
    }

    fn test_deferred_adapter() -> Arc<ExperimentalGptLiveDeferredAdapter> {
        Arc::new(ExperimentalGptLiveDeferredAdapter::new(
            meerkat_core::SessionLlmIdentity {
                model: "gpt-live-1-codex".to_string(),
                provider: Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
        ))
    }

    #[tokio::test]
    async fn semantic_rollback_retirement_clears_binding_after_physical_close_failure() {
        let session_id = meerkat_core::SessionId::new();
        let channel_id = meerkat_live::LiveChannelId::new("rollback-close-failure");
        let binding = ProviderWebrtcBinding::new(
            channel_id.clone(),
            session_id.clone(),
            meerkat_live::LiveRuntimeBindingGeneration::new(3),
            meerkat_live::LiveRuntimeBindingFence::new(5),
        );
        let sideband: Arc<dyn ProviderWebrtcSidebandSession> = Arc::new(FloodingSideband {
            observations: std::sync::Mutex::new(VecDeque::new()),
            fail_close: true,
        });
        let adapter = Arc::new(ExperimentalGptLiveDeferredAdapter::new(
            meerkat_core::SessionLlmIdentity {
                model: "gpt-live-1-codex".to_string(),
                provider: Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
        ));
        let transport = ExperimentalGptLiveWebrtcTransport::new();
        let pump_retirement_tx = transport.pump_retirement_sender().await;
        transport.active_by_session.lock().await.insert(
            session_id.clone(),
            spawn_sideband_actors(binding.clone(), sideband, adapter, 13, pump_retirement_tx),
        );

        assert!(transport.close_exact(&binding, Some(13)).await.is_err());
        assert!(transport.active_binding(&session_id).await.is_some());

        transport
            .retire_after_semantic_rollback(&channel_id, &session_id)
            .await;
        assert!(transport.active_binding(&session_id).await.is_none());
    }

    #[tokio::test]
    async fn bound_ready_activation_runs_only_after_delivery_and_failure_rolls_back() {
        let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let session_id = meerkat_core::SessionId::new();
        let _runtime_bindings = runtime
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare fixture runtime binding");
        let channel_id = meerkat_live::LiveChannelId::new("bound-ready-activation-failure");
        let identity = meerkat_core::SessionLlmIdentity {
            model: "gpt-live-1-codex".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let open = runtime
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("resolve generated live open admission");
        let host = Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
            meerkat_live::NoOpProjectionSink,
        )));
        let opened_channel = host
            .open_channel_with_authority(
                open.channel_open_authority()
                    .expect("admitted open carries host handoff"),
            )
            .await
            .expect("open fixture host channel");
        assert_eq!(opened_channel, channel_id);
        let adapter = Arc::new(ExperimentalGptLiveDeferredAdapter::new(identity.clone()));
        host.attach_adapter(&channel_id, Arc::clone(&adapter) as Arc<dyn LiveAdapter>)
            .await
            .expect("attach fixture adapter");

        let now_ms: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("fixture clock")
            .as_millis()
            .try_into()
            .expect("fixture clock fits u64");
        let token = "bound-ready-fixture-token";
        runtime
            .record_live_webrtc_token_issued(&session_id, &channel_id, token, now_ms, 60_000)
            .await
            .expect("record fixture WebRTC token");
        let execution_profile =
            meerkat_runtime::live_execution::LiveExecutionProfileSelection::__test_new(
                "test-function-bridge",
                meerkat_core::LiveExecutionMode::FunctionBridge,
                meerkat_core::LiveExecutionCapabilities {
                    function_bridge: true,
                    client_context: false,
                },
            )
            .expect("construct exact fixture execution profile");
        runtime
            .resolve_live_execution_profile_admission(&session_id, &channel_id, &execution_profile)
            .await
            .expect("resolve fixture live execution mode");
        let stage = runtime
            .stage_experimental_live_execution(&session_id, &channel_id, 0)
            .await
            .expect("stage exact experimental execution seed cursor");
        let _playback_readiness = runtime
            .register_live_playback_owner(&stage, "test-playback-owner")
            .await
            .expect("register fixture playback owner readiness");
        let runtime_binding = runtime
            .live_webrtc_runtime_binding(&session_id)
            .await
            .expect("read fixture runtime binding")
            .expect("fixture has runtime binding");
        let provider_binding = ProviderWebrtcBinding::new(
            channel_id.clone(),
            session_id.clone(),
            meerkat_live::LiveRuntimeBindingGeneration::new(runtime_binding.generation),
            meerkat_live::LiveRuntimeBindingFence::new(runtime_binding.fence),
        );
        let sideband: Arc<dyn ProviderWebrtcSidebandSession> = Arc::new(FloodingSideband {
            observations: std::sync::Mutex::new(VecDeque::new()),
            fail_close: true,
        });
        let transport = Arc::new(ExperimentalGptLiveWebrtcTransport::new());
        transport.registered_by_channel.lock().await.insert(
            channel_id.clone(),
            RegisteredExperimentalGptLiveChannel {
                session_id: session_id.clone(),
                broker: Arc::new(SeededAnswerBroker {
                    sideband,
                    canonical_seed_cursor: 0,
                }),
                adapter,
                identity,
                execution_profile_id: crate::GPT_LIVE_FUNCTION_BRIDGE_PROFILE_ID.to_string(),
            },
        );
        let activator = Arc::new(InspectingFailingActivator {
            runtime: Arc::clone(&runtime),
            expected_session: session_id.clone(),
            expected_channel: channel_id.clone(),
            calls: AtomicUsize::new(0),
            observed_generated_binding: std::sync::atomic::AtomicBool::new(false),
        });
        let binder: Arc<dyn crate::surface::LiveWebrtcBoundReadyBinder> =
            Arc::new(ExperimentalGptLiveBoundReadyBinder {
                transport: Arc::clone(&transport),
                activator: Arc::clone(&activator) as Arc<dyn ExperimentalLiveBoundChannelActivator>,
                live_adapter_host: Arc::clone(&host),
                public_observation_publisher: Arc::new(NoopPublicObservationPublisher),
                pending_context_recovery: Arc::new(Mutex::new(HashMap::new())),
                pending_result_recovery: Arc::new(Mutex::new(HashMap::new())),
            });

        let coordinated = crate::surface::coordinate_live_webrtc_answer(
            Arc::clone(&runtime),
            Arc::clone(&transport) as Arc<dyn LiveWebrtcAnswerTransport>,
            Some(binder),
            channel_id.clone(),
            token.to_string(),
            "test-offer-sdp".to_string(),
        )
        .await
        .expect("answer materializes before response delivery settles bound readiness");

        assert_eq!(activator.calls.load(AtomicOrdering::SeqCst), 0);
        assert!(
            runtime
                .live_delegation_runtime_binding(&session_id, &channel_id)
                .await
                .is_err(),
            "answer construction cannot bind execution before delivery"
        );

        let result = coordinated.delivery_custody.delivered().await;

        let result_detail = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_else(|| "unexpected success".to_string());
        assert!(
            matches!(
                &result,
                Err(crate::surface::LiveWebrtcAnswerCoordinatorError::Settlement(detail))
                    if detail.contains("fixture activation failure")
            ),
            "unexpected delivered answer result: {result_detail}"
        );
        assert_eq!(activator.calls.load(AtomicOrdering::SeqCst), 1);
        assert!(
            activator
                .observed_generated_binding
                .load(AtomicOrdering::SeqCst),
            "activation must run only after the generated atomic answer-and-bind transition"
        );
        assert!(
            runtime
                .live_delegation_runtime_binding(&session_id, &channel_id)
                .await
                .is_err(),
            "activation failure must roll generated execution binding back before returning"
        );
        assert!(transport.active_binding(&session_id).await.is_none());
        assert_eq!(provider_binding.session_id(), &session_id);
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures",
        not(target_arch = "wasm32")
    ))]
    #[tokio::test]
    async fn shipping_pump_exit_matrix_retires_terminal_and_output_custody() {
        use meerkat_contracts::{LiveOpenTransport, WireLiveTransportBootstrap};
        use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};

        #[derive(Clone, Copy)]
        enum ExitKind {
            Eof,
            BrokerError,
            PublicationRejected,
            ReceiptFailed,
        }

        let persistence = crate::PersistenceBundle::new(
            Arc::new(crate::MemoryStore::new()),
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        let temp = tempfile::tempdir().expect("tempdir");
        let factory = crate::AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let mut config = crate::Config::default();
        config.realm.insert(
            "default".to_string(),
            meerkat_core::RealmConfigSection::from_inline_api_keys(&[(
                "openai",
                "test-openai-key",
            )]),
        );
        let mut builder = crate::FactoryAgentBuilder::new(factory, config);
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        let (service, runtime) =
            crate::surface::build_runtime_backed_service(builder, 4, persistence);
        let service = Arc::new(service);
        let projection = Arc::new(crate::surface::ServiceLiveProjection::new(
            Arc::clone(&service),
            Arc::clone(&runtime),
        ));
        let live_adapter_host = Arc::new(meerkat_live::LiveAdapterHost::new(projection.clone()));
        let ws_state = Arc::new(meerkat_live::LiveWsState::new(
            Arc::clone(&live_adapter_host),
            projection.clone(),
            projection.clone(),
            projection.clone(),
        ));
        let webrtc_state = Arc::new(meerkat_live::LiveWebrtcState::new(
            Arc::clone(&live_adapter_host),
            projection.clone(),
            projection.clone(),
        ));
        let session = crate::Session::new();
        let session_id = session.id().clone();
        let executor_service = Arc::clone(&service);
        let executor_runtime = Arc::clone(&runtime);
        Box::pin(crate::surface::materialize_session(
            &service,
            &runtime,
            session,
            crate::CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-realtime-2".to_string(),
                prompt: meerkat_core::ContentInput::Text(String::new()),
                system_prompt: crate::SystemPromptOverride::Disable,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions::default()),
                labels: None,
            },
            move |materialized_session_id| {
                crate::surface::default_persistent_executor(
                    executor_service,
                    executor_runtime,
                    materialized_session_id,
                )
            },
        ))
        .await
        .expect("materialize pump-exit fixture session");
        #[cfg(feature = "comms")]
        {
            let comms: Arc<dyn meerkat_core::agent::CommsRuntime> = Arc::new(
                meerkat_comms::CommsRuntime::inproc_only("gpt-live-pump-exit-test")
                    .expect("inproc comms runtime"),
            );
            runtime
                .maybe_spawn_mob_comms_drain(
                    &session_id,
                    comms,
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(
                        "mob-gpt-live-pump-exit-test",
                    ),
                )
                .await
                .expect("record mob-owned ingress");
        }
        let member_host = Arc::new(
            crate::surface::ServiceMemberLiveHost::new(
                crate::surface::ServiceMemberLiveHostConfig {
                    service: Arc::clone(&service),
                    runtime_adapter: Arc::clone(&runtime),
                    host: Arc::clone(&live_adapter_host),
                    ws_state: Some(ws_state),
                    base_url: Some("wss://pump-exit.test".to_string()),
                    session_factory: Arc::new(
                        crate::test_fixtures::realtime::ScriptedRealtimeSessionFactory::new(),
                    ),
                    realm_id: None,
                    instance_id: None,
                    backend: None,
                },
            )
            .with_webrtc_cleanup_state(webrtc_state),
        );
        let authority = Arc::new(ScriptedStrictOpenAuthority::new(
            meerkat_core::SessionLlmIdentity {
                model: "gpt-live-1-codex".to_string(),
                provider: Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
        ));
        let authority_trait: Arc<dyn ExperimentalLiveOpenAuthorityProvider> = authority.clone();
        let mirror_host = crate::surface::ExperimentalGptLiveContextMirrorHost::new(
            Arc::clone(&runtime),
            Arc::clone(&member_host),
            Arc::clone(&authority_trait),
            Arc::new(SerializedLifecycleTestActivator {
                runtime: Arc::clone(&runtime),
            }),
        );
        let execution_identity = meerkat_contracts::WireLiveExecutionIdentityOverrideV1 {
            version: meerkat_contracts::WireLiveExecutionIdentityVersion::V1,
            profile_id: crate::GPT_LIVE_FUNCTION_BRIDGE_PROFILE_ID.to_string(),
        };

        for (ordinal, exit) in [
            ExitKind::Eof,
            ExitKind::BrokerError,
            ExitKind::PublicationRejected,
            ExitKind::ReceiptFailed,
        ]
        .into_iter()
        .enumerate()
        {
            let opened = member_host
                .open_with_execution_identity(
                    authority.as_ref(),
                    &session_id,
                    &execution_identity,
                    None,
                    None,
                    Some(LiveOpenTransport::Webrtc),
                )
                .await
                .expect("open exact pump-exit channel");
            let channel_id = opened.channel_id().clone();
            let token = match &opened.open().transport {
                WireLiveTransportBootstrap::Webrtc { token, .. } => token.clone(),
                other => panic!("expected WebRTC bootstrap, got {other:?}"),
            };
            let (output_tx, mut output_rx) = mpsc::unbounded_channel();
            let reject_release =
                matches!(exit, ExitKind::PublicationRejected).then(|| Arc::new(Notify::new()));
            let binder = authority
                .bound_ready_binder_for(
                    Arc::clone(&mirror_host) as Arc<dyn ExperimentalLiveBoundChannelActivator>,
                    Arc::clone(&live_adapter_host),
                    Arc::new(MatrixPublicObservationPublisher {
                        output_tx,
                        reject_release: reject_release.clone(),
                    }),
                )
                .expect("scripted authority supplies pump-exit binder");
            let pending_status = member_host
                .validate_experimental_live_channel_custody(&channel_id, opened.pending_receipt())
                .await
                .expect("pending receipt reacquires exact channel custody");
            assert_eq!(
                pending_status.phase(),
                &crate::surface::ExperimentalLiveChannelPhaseStatus::Pending
            );
            assert_eq!(
                pending_status.execution_mode(),
                meerkat_core::LiveExecutionMode::FunctionBridge
            );
            assert!(
                member_host
                    .send_experimental_live_input(
                        &channel_id,
                        "not-an-active-receipt",
                        meerkat_core::live_adapter::LiveInputChunk::Text {
                            text: "must remain local".to_string(),
                        },
                    )
                    .await
                    .is_err(),
                "provider input cannot begin before exact active authority"
            );
            let readiness = member_host
                .register_experimental_live_playback_owner(&channel_id, opened.pending_receipt())
                .await
                .expect("Meerkat registers an independently minted playback owner");
            let answer = member_host
                .answer_experimental_live_webrtc_offer(
                    Arc::clone(&authority.transport) as Arc<dyn LiveWebrtcAnswerTransport>,
                    binder,
                    channel_id.clone(),
                    opened.pending_receipt(),
                    readiness.readiness_receipt(),
                    token,
                    format!("pump-exit-offer-{ordinal}"),
                )
                .await
                .expect("answer exact pump-exit channel");
            answer
                .delivery_custody
                .delivered()
                .await
                .expect("publish pump-exit answer");
            let active_status = member_host
                .validate_experimental_live_channel_custody(&channel_id, opened.pending_receipt())
                .await
                .expect("pending receipt projects exact active custody after answer");
            let activation_receipt = active_status
                .phase()
                .activation_receipt()
                .expect("active custody carries exact activation receipt")
                .to_string();
            let binding = authority
                .transport
                .active_binding(&session_id)
                .await
                .expect("pump-exit binding is active");
            let sideband = authority
                .latest_sideband
                .lock()
                .await
                .as_ref()
                .cloned()
                .expect("pump-exit sideband is retained");
            let adapter = authority
                .latest_adapter
                .lock()
                .await
                .as_ref()
                .cloned()
                .expect("pump-exit adapter is retained");
            let user_turn = LiveSidebandTurnRef::__from_provider_observation(
                binding.channel_id(),
                format!("matrix-user-{ordinal}"),
                format!("private-matrix-user-{ordinal}"),
            )
            .expect("matrix user turn");
            let assistant_turn = LiveSidebandTurnRef::__from_provider_observation(
                binding.channel_id(),
                format!("matrix-assistant-{ordinal}"),
                format!("private-matrix-assistant-{ordinal}"),
            )
            .expect("matrix assistant turn");
            for kind in [
                LiveSidebandObservationKind::TurnStarted {
                    turn: user_turn.clone(),
                    role: LiveSidebandTurnRole::User,
                },
                LiveSidebandObservationKind::TurnFinished {
                    turn: user_turn,
                    role: LiveSidebandTurnRole::User,
                    transcript: format!("matrix user {ordinal}"),
                },
                LiveSidebandObservationKind::TurnStarted {
                    turn: assistant_turn,
                    role: LiveSidebandTurnRole::Assistant,
                },
            ] {
                sideband.push(LiveSidebandObservation::new(binding.clone(), kind));
            }
            let output = tokio::time::timeout(std::time::Duration::from_secs(2), output_rx.recv())
                .await
                .expect("opaque output publication is prompt")
                .expect("matrix publisher remains present");
            let complete_host = Arc::clone(&member_host);
            let complete_channel = channel_id.clone();
            let complete_output = output.output_id.clone();
            let complete_activation = activation_receipt.clone();
            if matches!(exit, ExitKind::ReceiptFailed) {
                live_adapter_host
                    .__fail_next_command_receipt_for_test(channel_id.clone())
                    .await;
            }
            let completion = tokio::spawn(async move {
                complete_host
                    .complete_live_playback(
                        &complete_channel,
                        &complete_activation,
                        &complete_output,
                    )
                    .await
            });
            if !matches!(exit, ExitKind::ReceiptFailed) {
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        if adapter
                            .playback_by_item
                            .lock()
                            .expect("matrix playback custody")
                            .values()
                            .any(|pending| !pending.final_forwarded && pending.terminal_forwarded)
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("terminal waiter is retained before pump failure");
            }
            match exit {
                ExitKind::Eof => sideband.close().await.expect("inject EOF"),
                ExitKind::BrokerError => sideband.fail(ProviderWebrtcBrokerError::Unavailable),
                ExitKind::PublicationRejected => reject_release
                    .as_ref()
                    .expect("publication rejection release")
                    .notify_one(),
                ExitKind::ReceiptFailed => {}
            }
            let completed = tokio::time::timeout(std::time::Duration::from_secs(2), completion)
                .await
                .unwrap_or_else(|_| panic!("pump exit ordinal={ordinal} did not settle terminal"))
                .expect("terminal task joins");
            assert!(
                completed.is_err(),
                "pump failure cannot report terminal success"
            );
            let completion_error = completed
                .as_ref()
                .err()
                .map(ToString::to_string)
                .unwrap_or_default();
            let cleanup = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if runtime
                        .live_session_for_active_channel(&channel_id)
                        .await
                        .is_none()
                        && authority
                            .transport
                            .active_binding(&session_id)
                            .await
                            .is_none()
                        && authority
                            .transport
                            .pending_pump_retirements
                            .lock()
                            .await
                            .is_empty()
                        && !authority
                            .transport
                            .registered_by_channel
                            .lock()
                            .await
                            .contains_key(&channel_id)
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
            assert!(
                cleanup.is_ok(),
                "pump cleanup stalled ordinal={ordinal} runtime_active={} provider_active={} pending={} registered={} completion={completion_error}",
                runtime
                    .live_session_for_active_channel(&channel_id)
                    .await
                    .is_some(),
                authority
                    .transport
                    .active_binding(&session_id)
                    .await
                    .is_some(),
                authority
                    .transport
                    .pending_pump_retirements
                    .lock()
                    .await
                    .len(),
                authority
                    .transport
                    .registered_by_channel
                    .lock()
                    .await
                    .contains_key(&channel_id),
            );
            let durable = service
                .load_authoritative_session(&session_id)
                .await
                .expect("load matrix session")
                .expect("matrix session remains durable");
            assert!(
                durable
                    .live_assistant_playback_target_for_channel(&channel_id)
                    .is_none(),
                "channel close resolves the playback target as Unmeasured"
            );
            assert!(
                !format!("{:?}", durable.messages()).contains("fabricated matrix assistant"),
                "pump failure cannot fabricate canonical assistant text"
            );
            assert!(
                runtime
                    .reserve_live_assistant_output_handle(
                        &session_id,
                        &channel_id,
                        &output.output_id,
                    )
                    .await
                    .is_err(),
                "retired opaque output cannot be replayed"
            );
            assert!(
                authority
                    .transport
                    .pending_deliveries
                    .lock()
                    .await
                    .values()
                    .all(|pending| pending.channel_id() != &channel_id),
                "pump cleanup removes exact pending deliveries"
            );
        }
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures",
        not(target_arch = "wasm32")
    ))]
    #[tokio::test]
    async fn ambiguous_context_and_result_physically_replace_and_atomically_rebind_exact_seed() {
        use meerkat_contracts::{LiveOpenTransport, WireLiveTransportBootstrap};
        use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};

        let session_store: Arc<dyn crate::SessionStore> = Arc::new(crate::MemoryStore::new());
        let persistence = crate::PersistenceBundle::new(
            session_store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        let temp = tempfile::tempdir().expect("tempdir");
        let factory = crate::AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let mut config = crate::Config::default();
        config.realm.insert(
            "default".to_string(),
            meerkat_core::RealmConfigSection::from_inline_api_keys(&[(
                "openai",
                "test-openai-key",
            )]),
        );
        let mut builder = crate::FactoryAgentBuilder::new(factory, config);
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
        let (service, runtime) =
            crate::surface::build_runtime_backed_service(builder, 4, persistence);
        let service = Arc::new(service);

        let projection = Arc::new(crate::surface::ServiceLiveProjection::new(
            Arc::clone(&service),
            Arc::clone(&runtime),
        ));
        let projection_sink: Arc<dyn meerkat_live::LiveProjectionSink> = projection.clone();
        let close_feedback: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = projection.clone();
        let status_feedback: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = projection.clone();
        let token_authority: Arc<dyn meerkat_live::LiveWsTokenAuthority> = projection;
        let live_adapter_host = Arc::new(meerkat_live::LiveAdapterHost::new(projection_sink));
        let ws_state = Arc::new(meerkat_live::LiveWsState::new(
            Arc::clone(&live_adapter_host),
            Arc::clone(&close_feedback),
            Arc::clone(&status_feedback),
            token_authority,
        ));
        let webrtc_state = Arc::new(meerkat_live::LiveWebrtcState::new(
            Arc::clone(&live_adapter_host),
            close_feedback,
            status_feedback,
        ));

        let session = crate::Session::new();
        let session_id = session.id().clone();
        let request = crate::CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-realtime-2".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: crate::SystemPromptOverride::Disable,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        };
        let service_for_executor = Arc::clone(&service);
        let runtime_for_executor = Arc::clone(&runtime);
        Box::pin(crate::surface::materialize_session(
            &service,
            &runtime,
            session,
            request,
            move |materialized_session_id| {
                crate::surface::default_persistent_executor(
                    service_for_executor,
                    runtime_for_executor,
                    materialized_session_id,
                )
            },
        ))
        .await
        .expect("materialize recovery fixture session");

        #[cfg(feature = "comms")]
        {
            let comms: Arc<dyn meerkat_core::agent::CommsRuntime> = Arc::new(
                meerkat_comms::CommsRuntime::inproc_only("gpt-live-recovery-test")
                    .expect("inproc comms runtime"),
            );
            runtime
                .maybe_spawn_mob_comms_drain(
                    &session_id,
                    comms,
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(
                        "mob-gpt-live-recovery-test",
                    ),
                )
                .await
                .expect("record mob-owned ingress");
        }

        let fallback_factory: Arc<crate::test_fixtures::realtime::ScriptedRealtimeSessionFactory> =
            Arc::new(crate::test_fixtures::realtime::ScriptedRealtimeSessionFactory::new());
        let member_host = Arc::new(
            crate::surface::ServiceMemberLiveHost::new(
                crate::surface::ServiceMemberLiveHostConfig {
                    service: Arc::clone(&service),
                    runtime_adapter: Arc::clone(&runtime),
                    host: Arc::clone(&live_adapter_host),
                    ws_state: Some(ws_state),
                    base_url: Some("wss://recovery.test".to_string()),
                    session_factory: fallback_factory as Arc<dyn RealtimeSessionFactory>,
                    realm_id: None,
                    instance_id: None,
                    backend: None,
                },
            )
            .with_webrtc_cleanup_state(webrtc_state),
        );
        let identity = meerkat_core::SessionLlmIdentity {
            model: "gpt-live-1-codex".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let authority = Arc::new(ScriptedStrictOpenAuthority::new(identity));
        let authority_trait: Arc<dyn ExperimentalLiveOpenAuthorityProvider> = authority.clone();
        let downstream: Arc<dyn ExperimentalLiveBoundChannelActivator> =
            Arc::new(SerializedLifecycleTestActivator {
                runtime: Arc::clone(&runtime),
            });
        let mirror_host = crate::surface::ExperimentalGptLiveContextMirrorHost::new(
            Arc::clone(&runtime),
            Arc::clone(&member_host),
            Arc::clone(&authority_trait),
            downstream,
        );
        let execution_identity = meerkat_contracts::WireLiveExecutionIdentityOverrideV1 {
            version: meerkat_contracts::WireLiveExecutionIdentityVersion::V1,
            profile_id: crate::GPT_LIVE_FUNCTION_BRIDGE_PROFILE_ID.to_string(),
        };

        let opened = member_host
            .open_with_execution_identity(
                authority.as_ref(),
                &session_id,
                &execution_identity,
                None,
                None,
                Some(LiveOpenTransport::Webrtc),
            )
            .await
            .expect("strict initial experimental open");
        let old_channel = opened.channel_id().clone();
        let old_token = match &opened.open().transport {
            WireLiveTransportBootstrap::Webrtc { token, .. } => token.clone(),
            other => panic!("expected WebRTC bootstrap, got {other:?}"),
        };
        let binder = authority
            .bound_ready_binder_for(
                Arc::clone(&mirror_host) as Arc<dyn ExperimentalLiveBoundChannelActivator>,
                Arc::clone(&live_adapter_host),
                Arc::new(NoopPublicObservationPublisher),
            )
            .expect("scripted authority supplies atomic binder");
        let initial_answer = crate::surface::coordinate_live_webrtc_answer(
            Arc::clone(&runtime),
            Arc::clone(&authority.transport) as Arc<dyn LiveWebrtcAnswerTransport>,
            Some(binder),
            old_channel.clone(),
            old_token,
            "initial-offer-sdp".to_string(),
        )
        .await
        .expect("initial answer binds exact experimental execution");
        initial_answer
            .delivery_custody
            .delivered()
            .await
            .expect("publish initial answer");
        let stale_old_binding = authority
            .transport
            .active_binding(&session_id)
            .await
            .expect("old provider binding is physically active");
        let first_channel_a_turn = LiveSidebandTurnRef::__from_provider_observation(
            &old_channel,
            "turn:1".to_string(),
            "private-channel-a-first-turn".to_string(),
        )
        .expect("channel A first provider turn");
        runtime
            .observe_live_provider_turn_started(&LiveSidebandObservation::new(
                stale_old_binding.clone(),
                LiveSidebandObservationKind::TurnStarted {
                    turn: first_channel_a_turn.clone(),
                    role: LiveSidebandTurnRole::User,
                },
            ))
            .await
            .expect("channel A first provider turn is admitted");
        runtime
            .observe_live_provider_turn_finished(&LiveSidebandObservation::new(
                stale_old_binding.clone(),
                LiveSidebandObservationKind::TurnFinished {
                    turn: first_channel_a_turn.clone(),
                    role: LiveSidebandTurnRole::User,
                    transcript: "channel A first user turn".to_string(),
                },
            ))
            .await
            .expect("channel A first provider turn completes before replacement");

        service
            .append_external_user_content(
                &session_id,
                meerkat_core::ContentInput::Text("canonical text during voice".to_string()),
            )
            .await
            .expect("commit external canonical text");
        let (committed, store_authority) = service
            .export_live_context_committed_boundary(&session_id)
            .await
            .expect("export exact committed boundary");
        assert_eq!(
            runtime
                .enqueue_committed_parent_session_boundary(
                    &session_id,
                    &committed,
                    &store_authority,
                )
                .await
                .expect("ambiguous append realizes replacement"),
            1
        );

        let replacement = mirror_host
            .pending_replacement_required(&session_id)
            .await
            .expect("ambiguity publishes typed replacement-required bootstrap");
        assert_eq!(
            mirror_host.pending_replacement_required(&session_id).await,
            Some(replacement.clone()),
            "a lost pull response must return the identical pending bootstrap"
        );
        let crate::surface::ExperimentalLiveReplacementRequired::CanonicalContext {
            open: replacement_open,
            canonical_seed_cursor: replacement_seed_cursor,
        } = replacement
        else {
            panic!("context ambiguity must publish the canonical-context reason")
        };
        let replacement_channel = meerkat_live::LiveChannelId::new(&replacement_open.channel_id);
        assert!(replacement_seed_cursor > 0);
        assert_ne!(replacement_channel, old_channel);
        assert_eq!(
            authority.transport.active_binding(&session_id).await,
            None,
            "old provider transport is physically closed before replacement answer"
        );
        assert!(
            runtime
                .live_session_for_active_channel(&old_channel)
                .await
                .is_none(),
            "old semantic channel is closed"
        );

        let stale_turn = LiveSidebandTurnRef::__from_provider_observation(
            &old_channel,
            "stale-old-turn".to_string(),
            "private-old-turn".to_string(),
        )
        .expect("stale turn fixture");
        let stale_observation = LiveSidebandObservation::new(
            stale_old_binding,
            LiveSidebandObservationKind::TurnStarted {
                turn: stale_turn,
                role: LiveSidebandTurnRole::User,
            },
        );
        assert!(
            runtime
                .observe_live_provider_turn_started(&stale_observation)
                .await
                .is_err(),
            "a callback from the physically closed old channel is fenced"
        );

        let replacement_token = match &replacement_open.transport {
            WireLiveTransportBootstrap::Webrtc { token, .. } => token.clone(),
            other => panic!("expected replacement WebRTC bootstrap, got {other:?}"),
        };
        let recovery_binder = authority
            .bound_ready_binder_for(
                Arc::clone(&mirror_host) as Arc<dyn ExperimentalLiveBoundChannelActivator>,
                Arc::clone(&live_adapter_host),
                Arc::new(NoopPublicObservationPublisher),
            )
            .expect("scripted authority supplies recovery binder");
        let recovery_answer = crate::surface::coordinate_live_webrtc_answer(
            Arc::clone(&runtime),
            Arc::clone(&authority.transport) as Arc<dyn LiveWebrtcAnswerTransport>,
            Some(recovery_binder),
            replacement_channel.clone(),
            replacement_token,
            "replacement-offer-sdp".to_string(),
        )
        .await
        .expect("replacement answer atomically binds recovery seed");
        recovery_answer
            .delivery_custody
            .delivered()
            .await
            .expect("publish replacement answer");
        assert_eq!(
            mirror_host.pending_replacement_required(&session_id).await,
            None,
            "exact replacement activation retires the pending bootstrap"
        );

        let recovered_binding = runtime
            .live_delegation_runtime_binding(&session_id, &replacement_channel)
            .await
            .expect("atomic recovery answer publishes exact execution binding");
        let recovered_provider_binding = authority
            .transport
            .active_binding(&session_id)
            .await
            .expect("replacement provider binding is physically active");
        assert_eq!(recovered_binding.channel_id(), &replacement_channel);
        assert_eq!(
            recovered_binding.fence_token(),
            recovered_provider_binding.runtime_fence().get()
        );
        assert_eq!(
            recovered_binding.generation(),
            recovered_provider_binding.runtime_generation().get()
        );
        assert_eq!(authority.prepare_sequence.load(AtomicOrdering::SeqCst), 2);
        assert!(authority.pending_context_recovery.lock().await.is_empty());

        // Exercise the distinct result-delivery ambiguity authority through
        // the same physical replacement choreography without manufacturing a
        // canonical context append or replaying the ambiguous result.
        let result_turn = LiveSidebandTurnRef::__from_provider_observation(
            &replacement_channel,
            "turn:1".to_string(),
            "private-result-recovery-turn".to_string(),
        )
        .expect("result recovery turn fixture");
        assert_ne!(
            first_channel_a_turn.adapter_key(),
            result_turn.adapter_key(),
            "replacement channel B namespaces its reset provider-local turn:1"
        );
        let turn_started = runtime
            .observe_live_provider_turn_started(&LiveSidebandObservation::new(
                recovered_provider_binding.clone(),
                LiveSidebandObservationKind::TurnStarted {
                    turn: result_turn.clone(),
                    role: LiveSidebandTurnRole::User,
                },
            ))
            .await
            .expect("channel B first provider turn is admitted after A used local turn:1");
        let correlation = meerkat_core::LiveUserTurnCorrelation::new(
            replacement_channel.clone(),
            turn_started.interaction_id(),
            meerkat_core::OpaqueProviderCorrelation::new(
                "result-recovery-delegation",
                turn_started.provider_turn_ref(),
            )
            .expect("provider correlation fixture"),
        )
        .expect("live turn correlation fixture");
        let operation = meerkat_core::exact_operation::ExactOperationIdentity::for_domain(
            meerkat_core::ops::OperationId::new(),
            correlation.clone(),
        );
        let provisional = meerkat_core::ProvisionalLiveHandoff::new(
            correlation,
            "confirmed result recovery input",
            meerkat_core::LiveHandoffInputProvenance::NormalizedHandoff,
        )
        .expect("provisional handoff fixture");
        runtime
            .admit_live_delegation(turn_started.binding(), &operation, &provisional)
            .await
            .expect("admit exact result-recovery delegation");
        let worker = runtime
            .authorize_live_delegation_worker_start(
                &session_id,
                turn_started.binding().runtime_id(),
                turn_started.binding().fence_token(),
                turn_started.binding().generation(),
                &operation,
                &provisional,
                "result-recovery-worker",
            )
            .await
            .expect("authorize result-recovery worker");
        runtime
            .resolve_live_delegation_worker_start(
                turn_started.binding().runtime_id(),
                turn_started.binding().fence_token(),
                turn_started.binding().generation(),
                &worker,
                true,
            )
            .await
            .expect("start result-recovery worker");
        let final_transcript = service
            .commit_live_user_transcript_final(
                &session_id,
                provisional.clone(),
                Some(meerkat_core::RealtimeTranscriptEvent::UserTranscriptFinal {
                    item_id: turn_started.provider_turn_ref().to_string(),
                    previous_item_id: None,
                    content_index: 0,
                    text: "confirmed result recovery input".to_string(),
                }),
            )
            .await
            .expect("commit exact final transcript evidence");
        let (final_transcript_boundary, final_transcript_store_authority) = service
            .export_live_context_committed_boundary(&session_id)
            .await
            .expect("export exact final transcript boundary");
        runtime
            .enqueue_committed_live_transcript_boundary(
                &session_id,
                &final_transcript_boundary,
                &final_transcript_store_authority,
            )
            .await
            .expect("advance canonical coverage without echoing live transcript");
        let reconciliation = runtime
            .reconcile_live_delegation_transcript(
                &session_id,
                turn_started.binding().runtime_id(),
                turn_started.binding().fence_token(),
                turn_started.binding().generation(),
                &operation,
                &provisional,
                &final_transcript,
            )
            .await
            .expect("reconcile exact final transcript");
        runtime
            .record_live_delegation_worker_terminal(
                turn_started.binding().runtime_id(),
                turn_started.binding().fence_token(),
                turn_started.binding().generation(),
                &worker,
                meerkat_runtime::live_execution::LiveDelegationWorkerTerminalKind::Completed,
            )
            .await
            .expect("record eligible completed worker result");
        runtime
            .observe_live_provider_turn_finished(&LiveSidebandObservation::new(
                recovered_provider_binding.clone(),
                LiveSidebandObservationKind::TurnFinished {
                    turn: result_turn,
                    role: LiveSidebandTurnRole::User,
                    transcript: "final delegated user turn".to_string(),
                },
            ))
            .await
            .expect("finish exact provider turn before deferred result release");
        let release = runtime
            .authorize_live_delegation_result_release(
                &session_id,
                turn_started.binding().runtime_id(),
                turn_started.binding().fence_token(),
                turn_started.binding().generation(),
                &operation,
                &reconciliation,
            )
            .await
            .expect("authorize one exact deferred result release");
        let delivery = runtime
            .authorize_live_delegation_result_delivery(&release, "ambiguous worker result")
            .await
            .expect("authorize distinct one-use result delivery");
        let result_recovery = match runtime
            .resolve_live_delegation_result_delivery(
                &delivery,
                meerkat_runtime::live_execution::LiveDelegationResultDeliveryObservation::Ambiguous,
            )
            .await
            .expect("terminalize ambiguous result without retry")
        {
            meerkat_runtime::live_execution::LiveDelegationResultDeliveryResolution::AmbiguityRecovery(
                recovery,
            ) => recovery,
            other => panic!("expected result ambiguity recovery, got {other:?}"),
        };
        runtime
            .realize_live_delegation_result_ambiguity_recovery(result_recovery)
            .await
            .expect("physically realize result ambiguity replacement");

        let result_replacement = mirror_host
            .pending_replacement_required(&session_id)
            .await
            .expect("result ambiguity publishes distinct replacement bootstrap");
        assert_eq!(
            mirror_host.pending_replacement_required(&session_id).await,
            Some(result_replacement.clone()),
            "lost result-replacement pulls must remain idempotent"
        );
        let crate::surface::ExperimentalLiveReplacementRequired::DelegationResult {
            open: result_replacement_open,
            canonical_seed_cursor: result_replacement_seed_cursor,
        } = result_replacement
        else {
            panic!("result ambiguity must publish the delegation-result reason")
        };
        assert!(result_replacement_seed_cursor >= replacement_seed_cursor);
        let result_replacement_channel =
            meerkat_live::LiveChannelId::new(&result_replacement_open.channel_id);
        assert_ne!(result_replacement_channel, replacement_channel);
        assert!(
            runtime
                .live_session_for_active_channel(&replacement_channel)
                .await
                .is_none(),
            "result ambiguity closes the exact old semantic channel"
        );
        assert_eq!(
            authority.transport.active_binding(&session_id).await,
            None,
            "result ambiguity physically closes the exact old provider binding"
        );
        assert!(
            runtime
                .observe_live_provider_turn_started(&LiveSidebandObservation::new(
                    recovered_provider_binding,
                    LiveSidebandObservationKind::TurnStarted {
                        turn: LiveSidebandTurnRef::__from_provider_observation(
                            &replacement_channel,
                            "stale-result-turn".to_string(),
                            "private-stale-result-turn".to_string(),
                        )
                        .expect("stale result turn fixture"),
                        role: LiveSidebandTurnRole::User,
                    },
                ))
                .await
                .is_err(),
            "a callback from the result recovery's old channel is fenced"
        );

        let result_replacement_token = match &result_replacement_open.transport {
            WireLiveTransportBootstrap::Webrtc { token, .. } => token.clone(),
            other => panic!("expected result replacement WebRTC bootstrap, got {other:?}"),
        };
        let result_recovery_binder = authority
            .bound_ready_binder_for(
                Arc::clone(&mirror_host) as Arc<dyn ExperimentalLiveBoundChannelActivator>,
                Arc::clone(&live_adapter_host),
                Arc::new(NoopPublicObservationPublisher),
            )
            .expect("scripted authority supplies result-recovery binder");
        let result_recovery_answer = crate::surface::coordinate_live_webrtc_answer(
            Arc::clone(&runtime),
            Arc::clone(&authority.transport) as Arc<dyn LiveWebrtcAnswerTransport>,
            Some(result_recovery_binder),
            result_replacement_channel.clone(),
            result_replacement_token,
            "result-replacement-offer-sdp".to_string(),
        )
        .await
        .expect("result replacement answer atomically binds exact seed");
        result_recovery_answer
            .delivery_custody
            .delivered()
            .await
            .expect("publish result replacement answer");
        assert_eq!(
            mirror_host.pending_replacement_required(&session_id).await,
            None,
            "exact result replacement activation retires the pending bootstrap"
        );
        let result_recovered_binding = runtime
            .live_delegation_runtime_binding(&session_id, &result_replacement_channel)
            .await
            .expect("atomic result recovery publishes exact execution binding");
        let result_recovered_provider_binding = authority
            .transport
            .active_binding(&session_id)
            .await
            .expect("result replacement provider binding is physically active");
        assert_eq!(
            result_recovered_binding.fence_token(),
            result_recovered_provider_binding.runtime_fence().get()
        );
        assert_eq!(
            result_recovered_binding.generation(),
            result_recovered_provider_binding.runtime_generation().get()
        );
        assert_eq!(authority.prepare_sequence.load(AtomicOrdering::SeqCst), 3);
        assert!(authority.pending_result_recovery.lock().await.is_empty());

        member_host
            .close_live_channel(Some(authority.as_ref()), &result_replacement_channel)
            .await
            .expect("close replacement fixture channel");
        mirror_host
            .retire_bound_channel_after_pump_exit(&result_recovered_binding)
            .await
            .expect("pump retirement classifies an exact already-closed channel as complete");
        assert!(
            runtime
                .live_session_for_active_channel(&result_replacement_channel)
                .await
                .is_none(),
            "the production already-closed branch must not recreate semantic custody"
        );
        assert!(
            authority
                .transport
                .active_binding(&session_id)
                .await
                .is_none(),
            "the production already-closed branch must not recreate provider custody"
        );
    }
}
