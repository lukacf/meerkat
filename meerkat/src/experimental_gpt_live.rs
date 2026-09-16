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
use meerkat_llm_core::provider_runtime::ResolvedRealtimeTarget;
use meerkat_llm_core::realtime_session::{
    RealtimeExternalSessionTarget, RealtimeSessionFactory, RealtimeSessionOpenConfig,
};
use meerkat_llm_core::{LlmError, RealtimeSession};
#[cfg(feature = "experimental-gpt-live")]
use meerkat_openai::gpt_live::{
    GptLiveBrokerFactory, GptLiveBrokerOpenConfig, GptLiveBrokerSession,
};
use meerkat_openai::gpt_live_broker::{
    GptLiveAppendToken, GptLiveBrokerError, GptLiveBrokerObservation, GptLiveBrokerTerminalClass,
    GptLiveDelegationRef, GptLiveTurnRef, GptLiveTurnRole,
};
use meerkat_openai::public_live::{
    PublicLiveBrokerFactory, PublicLiveBrokerSession, PublicLiveOpenConfig,
};
use meerkat_runtime::live_execution::{
    LiveContextAppendAuthority, LiveDelegationResultDeliveryAuthority,
    LiveDelegationResultDeliveryObservation,
};
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::session_runtime::live_orchestration::RealtimeSessionOpenProjection;

/// Public Live client-context execution profile: the released `gpt-live-1`
/// voice model speaks while the channel-bound Meerkat executor performs
/// delegated work and returns commentary. Consumers select this identity
/// through `live/open { execution_identity: { version: "v1", profile_id } }`.
pub const GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID: &str = "openai.gpt-live-1.client-context.v1";
/// Catalog model row served by the public Live broker.
pub const GPT_LIVE_PUBLIC_MODEL: &str = "gpt-live-1";

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

    /// Accept only a summary sealed by the shared snapshot owner. Alternate
    /// factories must explicitly support this instead of silently replaying
    /// the full history while claiming summarized continuity.
    fn set_context_summary(
        &mut self,
        _summary: crate::session_runtime::live_summary::LiveContextSummary,
    ) -> Result<(), crate::session_runtime::live_summary::LiveContextSummaryError> {
        Err(crate::session_runtime::live_summary::LiveContextSummaryError::Unsupported)
    }

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
    /// Positive per-target readiness, including exact configured credential
    /// resolution and durable-source authorization. This prepares and drops
    /// one unbound factory: it performs no provider session open or channel
    /// registration. The eventual open independently revalidates authority.
    async fn probe_execution_readiness(
        &self,
        session_id: &meerkat_core::SessionId,
        execution_identity: &meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
    ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
        let _pending = self.prepare_open(session_id, execution_identity).await?;
        Ok(())
    }

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

/// Host composition for the public Live (`gpt-live-1`) open authority.
///
/// `execution_identity` is the host-owned voice identity: provider OpenAI,
/// model `gpt-live-1`, and a configured auth binding in `realm` that resolves
/// to an OpenAI API key. Session instructions default to the platform
/// client-context guidance; hosts may replace them with their own trusted
/// voice guidance. No operator, Gate0, or realm admission exists for the
/// public path: the catalog row, the configured binding, and the compiled
/// `openai-live` feature are the admission.
pub struct PublicGptLiveOpenAuthorityConfig {
    pub agent_factory: crate::AgentFactory,
    pub config_source: Arc<dyn ExperimentalLiveCurrentConfigSource>,
    pub binding_authority: Arc<dyn ExperimentalLiveSessionBindingAuthority>,
    pub execution_identity: meerkat_core::SessionLlmIdentity,
    pub realm: meerkat_core::RealmId,
    pub transport: Arc<ExperimentalGptLiveWebrtcTransport>,
    pub voice: String,
    pub session_instructions: Option<String>,
}

/// Host-selected bookkeeping policy for public Live's continuous media.
///
/// This is not voice activity detection. Public Live supplies neither a
/// per-utterance playback completion event nor a measured played prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PublicGptLivePlaybackPolicy {
    /// Preserve caller-confirmed cuts of the observed transcript (default).
    #[default]
    CallerConfirmedSnapshots,
    /// Retain canonical assistant transcript snapshots with explicit
    /// unmeasured provenance, never as confirmed played assistant rows. Release
    /// each segment without waiting for a browser playback ACK.
    ProviderManagedUnmeasured,
}

/// Which provider path an open authority admits targets through.
enum GptLiveOpenAdmission {
    #[cfg(feature = "experimental-gpt-live")]
    Experimental {
        factory_identity: crate::ExperimentalLiveFactoryIdentity,
    },
    Public {
        session_instructions: Option<String>,
        playback_policy: PublicGptLivePlaybackPolicy,
    },
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
    admission: GptLiveOpenAdmission,
    transport: Arc<ExperimentalGptLiveWebrtcTransport>,
    voice: String,
    #[cfg(feature = "test-realtime-fixtures")]
    test_endpoints: Option<(String, String)>,
    #[cfg(feature = "test-realtime-fixtures")]
    test_base_url: Option<String>,

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
    /// Compose the deprecated private-protocol authority.
    #[cfg(feature = "experimental-gpt-live")]
    pub fn new(
        config: ExperimentalGptLiveOpenAuthorityConfig,
    ) -> Result<Self, ExperimentalGptLiveOpenAuthorityError> {
        Self::validate_host_identity(
            &config.voice,
            &config.execution_identity,
            &config.realm,
            "gpt-live-1-codex",
        )?;
        Ok(Self::compose(
            config.agent_factory,
            config.config_source,
            config.binding_authority,
            config.execution_identity,
            config.realm,
            GptLiveOpenAdmission::Experimental {
                factory_identity: config.factory_identity,
            },
            config.transport,
            config.voice,
        ))
    }

    /// Compose the public Live (`gpt-live-1`) authority.
    pub fn new_public(
        config: PublicGptLiveOpenAuthorityConfig,
    ) -> Result<Self, ExperimentalGptLiveOpenAuthorityError> {
        Self::validate_host_identity(
            &config.voice,
            &config.execution_identity,
            &config.realm,
            GPT_LIVE_PUBLIC_MODEL,
        )?;
        Ok(Self::compose(
            config.agent_factory,
            config.config_source,
            config.binding_authority,
            config.execution_identity,
            config.realm,
            GptLiveOpenAdmission::Public {
                session_instructions: config.session_instructions,
                playback_policy: PublicGptLivePlaybackPolicy::default(),
            },
            config.transport,
            config.voice,
        ))
    }

    /// Opt in to an observation-only lifecycle for continuous public Live
    /// media. Private-protocol authorities refuse this policy seam.
    pub fn with_public_playback_policy(
        mut self,
        policy: PublicGptLivePlaybackPolicy,
    ) -> Result<Self, ExperimentalGptLiveOpenAuthorityError> {
        match &mut self.admission {
            GptLiveOpenAdmission::Public {
                playback_policy, ..
            } => *playback_policy = policy,
            #[cfg(feature = "experimental-gpt-live")]
            GptLiveOpenAdmission::Experimental { .. } => {
                return Err(ExperimentalGptLiveOpenAuthorityError::UnsupportedPlaybackPolicy);
            }
        }
        Ok(self)
    }

    fn validate_host_identity(
        voice: &str,
        execution_identity: &meerkat_core::SessionLlmIdentity,
        realm: &meerkat_core::RealmId,
        model: &str,
    ) -> Result<(), ExperimentalGptLiveOpenAuthorityError> {
        if voice.trim().is_empty() {
            return Err(ExperimentalGptLiveOpenAuthorityError::MissingVoice);
        }
        if execution_identity.provider != meerkat_core::Provider::OpenAI
            || execution_identity.model != model
            || execution_identity.self_hosted_server_id.is_some()
            || execution_identity.provider_params.is_some()
            || !matches!(
                execution_identity.auth_binding.as_ref(),
                Some(binding)
                    if binding.origin == meerkat_core::BindingOrigin::Configured
                        && binding.realm == *realm
            )
        {
            return Err(ExperimentalGptLiveOpenAuthorityError::InvalidExecutionIdentity);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn compose(
        agent_factory: crate::AgentFactory,
        config_source: Arc<dyn ExperimentalLiveCurrentConfigSource>,
        binding_authority: Arc<dyn ExperimentalLiveSessionBindingAuthority>,
        execution_identity: meerkat_core::SessionLlmIdentity,
        realm: meerkat_core::RealmId,
        admission: GptLiveOpenAdmission,
        transport: Arc<ExperimentalGptLiveWebrtcTransport>,
        voice: String,
    ) -> Self {
        Self {
            agent_factory,
            config_source,
            binding_authority,
            execution_identity,
            realm,
            admission,
            transport,
            voice,
            #[cfg(feature = "test-realtime-fixtures")]
            test_endpoints: None,
            #[cfg(feature = "test-realtime-fixtures")]
            test_base_url: None,
            pending_context_recovery: Arc::new(Mutex::new(HashMap::new())),
            pending_result_recovery: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Test-only public base URL injection: the real admission path runs
    /// unchanged while the public HTTP and WebSocket destination moves to a
    /// deterministic local server.
    #[cfg(feature = "test-realtime-fixtures")]
    #[doc(hidden)]
    #[must_use]
    pub fn with_test_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.test_base_url = Some(base_url.into());
        self
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

impl ExperimentalGptLiveOpenAuthority {
    #[cfg(feature = "experimental-gpt-live")]
    async fn prepare_experimental_pending(
        &self,
        canonical_session_id: &meerkat_core::SessionId,
        execution_identity: &meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
        config: &meerkat_core::Config,
        identity: meerkat_core::SessionLlmIdentity,
        factory_identity: &crate::ExperimentalLiveFactoryIdentity,
    ) -> Result<ExperimentalGptLivePendingChannel, ExperimentalLiveOpenAuthorityError> {
        let preparation = self
            .agent_factory
            .prepare_experimental_live_admission_for_identity(
                config,
                &self.realm,
                &identity,
                factory_identity,
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
        if let Some((call_url, sideband_base_url)) = &self.test_endpoints {
            return ExperimentalGptLivePendingChannel::__from_admission_with_test_endpoints(
                &self.agent_factory,
                admission,
                &self.realm,
                factory_identity,
                canonical_session_id.clone(),
                self.voice.clone(),
                call_url,
                sideband_base_url,
            )
            .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed);
        }
        ExperimentalGptLivePendingChannel::from_admission(
            &self.agent_factory,
            admission,
            &self.realm,
            factory_identity,
            canonical_session_id.clone(),
            self.voice.clone(),
        )
        .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed)
    }

    /// Public admission: the selected profile must be the public client-context
    /// profile, the host identity's configured binding is authorized for this
    /// session, and the credentialed target is resolved against the catalog.
    async fn prepare_public_pending(
        &self,
        canonical_session_id: &meerkat_core::SessionId,
        execution_identity: &meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
        config: &meerkat_core::Config,
        identity: meerkat_core::SessionLlmIdentity,
        session_instructions: Option<String>,
    ) -> Result<ExperimentalGptLivePendingChannel, ExperimentalLiveOpenAuthorityError> {
        if execution_identity.profile_id != GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID {
            return Err(ExperimentalLiveOpenAuthorityError::AdmissionFailed);
        }
        let auth_binding = self
            .agent_factory
            .resolve_public_live_binding_for_identity(config, &self.realm, &identity)
            .inspect_err(|error| {
                tracing::warn!(
                    stage = "binding_resolution",
                    cause = live_factory_failure_class(error),
                    "public Live admission failed"
                );
            })
            .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed)?;
        let authorization = self
            .binding_authority
            .authorize_binding_use(canonical_session_id, &auth_binding)
            .await?;
        let (binding_use, auth_lease) = authorization.into_parts();
        let target = self
            .agent_factory
            .resolve_public_live_target(config, &self.realm, &identity, binding_use, auth_lease)
            .await
            .inspect_err(|error| {
                tracing::warn!(
                    stage = "target_resolution",
                    cause = live_factory_failure_class(error),
                    "public Live admission failed"
                );
            })
            .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed)?;
        let execution_profile =
            meerkat_runtime::live_execution::LiveExecutionProfileSelection::from_public_profile(
                GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID,
                meerkat_core::LiveExecutionMode::ClientContext,
                meerkat_core::LiveExecutionCapabilities {
                    function_bridge: false,
                    client_context: true,
                },
            )
            .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed)?;
        let session_instructions = session_instructions
            .or_else(|| Some(crate::gpt_live_client_context_session_instructions().to_string()));
        #[cfg(feature = "test-realtime-fixtures")]
        if let Some(base_url) = &self.test_base_url {
            return ExperimentalGptLivePendingChannel::__from_public_target_with_base_url(
                target,
                execution_profile,
                canonical_session_id.clone(),
                self.voice.clone(),
                session_instructions,
                base_url,
            )
            .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed);
        }
        ExperimentalGptLivePendingChannel::from_public_target(
            target,
            execution_profile,
            canonical_session_id.clone(),
            self.voice.clone(),
            session_instructions,
        )
        .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed)
    }
}

// Auth commands, URLs, and provider diagnostics can contain secrets. Preserve
// the typed cause without forwarding their opaque payloads to operator logs.
fn live_factory_failure_class(error: &meerkat_client::FactoryError) -> &'static str {
    use meerkat_client::FactoryError;
    use meerkat_core::ConnectionTargetError;
    use meerkat_llm_core::provider_runtime::errors::ProviderAuthError;

    match error {
        FactoryError::ProviderAuth(error) => match error {
            ProviderAuthError::Auth(error) => error.kind().as_str(),
            ProviderAuthError::Binding(_) => "invalid_provider_binding",
            ProviderAuthError::SourceResolutionFailed(_) => "credential_source_resolution_failed",
            ProviderAuthError::ExternalResolverMissing(_) => "external_resolver_missing",
            ProviderAuthError::NoRuntimeRegistered(_) => "provider_runtime_missing",
            ProviderAuthError::ResolvedProviderMismatch { .. } => "resolved_provider_mismatch",
        },
        FactoryError::ConnectionTarget(error) => match error {
            ConnectionTargetError::MissingRealm => "missing_realm",
            ConnectionTargetError::UnknownRealm(_) => "unknown_realm",
            ConnectionTargetError::MissingDefaultBinding { .. } => "missing_default_binding",
            ConnectionTargetError::InvalidRealmId { .. } => "invalid_realm_id",
            ConnectionTargetError::InvalidBindingId { .. } => "invalid_binding_id",
            ConnectionTargetError::RealmConfigInvalid { .. } => "invalid_realm_config",
            ConnectionTargetError::BindingInvalid { .. } => "invalid_binding",
            ConnectionTargetError::ProviderMismatch { .. } => "binding_provider_mismatch",
            ConnectionTargetError::AmbiguousCredentialAccountBindings { .. } => "ambiguous_binding",
            ConnectionTargetError::RealmChain(_) => "invalid_realm_chain",
        },
        FactoryError::ClientBuild(error) => live_provider_failure_class(error),
        FactoryError::UnsupportedProvider(_) => "unsupported_provider",
        FactoryError::TokenStore(_) => "token_store_unavailable",
        FactoryError::ClientCreationFailed(_) => "client_creation_failed",
        FactoryError::ExperimentalModelRequiresLiveChannel { .. } => "experimental_model",
        FactoryError::SelfHostedBinding(_) => "self_hosted_binding",
    }
}

fn live_provider_failure_class(
    error: &meerkat_llm_core::provider_runtime::errors::ProviderClientError,
) -> &'static str {
    use meerkat_llm_core::provider_runtime::errors::ProviderClientError;
    match error {
        ProviderClientError::MissingFeature(feature) => feature,
        ProviderClientError::ClientInit(_) => "client_init",
        ProviderClientError::NoCredentialMaterial => "no_credential_material",
        ProviderClientError::DynamicAuthorizerNotYetSupportedInShimMode => "unsupported_authorizer",
        ProviderClientError::InvalidBaseUrl(_) => "invalid_base_url",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExperimentalGptLiveOpenAuthorityError {
    #[error("experimental GPT Live open authority requires a non-empty voice")]
    MissingVoice,
    #[error("experimental GPT Live open authority requires the canonical host-owned identity")]
    InvalidExecutionIdentity,
    #[error("provider-managed playback policy requires the public Live authority")]
    UnsupportedPlaybackPolicy,
}

#[async_trait]
impl ExperimentalLiveOpenAuthorityProvider for ExperimentalGptLiveOpenAuthority {
    fn execution_feature_capabilities(
        &self,
    ) -> Result<Vec<&'static str>, ExperimentalLiveOpenAuthorityError> {
        match &self.admission {
            #[cfg(feature = "experimental-gpt-live")]
            GptLiveOpenAdmission::Experimental { factory_identity } => self
                .agent_factory
                .experimental_live_execution_feature_capabilities(
                    &self.realm,
                    factory_identity,
                    crate::GPT_LIVE_CLIENT_CONTEXT_PROFILE_ID,
                )
                .map_err(|_| ExperimentalLiveOpenAuthorityError::Unavailable),
            GptLiveOpenAdmission::Public { .. } => Ok(vec![
                meerkat_contracts::LIVE_EXECUTION_IDENTITY_V1_CAPABILITY,
                meerkat_contracts::LIVE_CLIENT_CONTEXT_V1_CAPABILITY,
            ]),
        }
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
        let pending = match &self.admission {
            #[cfg(feature = "experimental-gpt-live")]
            GptLiveOpenAdmission::Experimental { factory_identity } => {
                self.prepare_experimental_pending(
                    canonical_session_id,
                    execution_identity,
                    &config,
                    identity,
                    factory_identity,
                )
                .await?
            }
            GptLiveOpenAdmission::Public {
                session_instructions,
                playback_policy,
            } => self
                .prepare_public_pending(
                    canonical_session_id,
                    execution_identity,
                    &config,
                    identity,
                    session_instructions.clone(),
                )
                .await?
                .with_public_playback_policy(*playback_policy)
                .map_err(|_| ExperimentalLiveOpenAuthorityError::AdmissionFailed)?,
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
            .map_err(|error| {
                tracing::warn!(%error, %channel_id, "experimental live physical close remains incomplete");
                ExperimentalLiveOpenAuthorityError::ChannelBindingFailed
            })
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

#[derive(Debug, Clone)]
pub enum ExperimentalLivePhysicalClose {
    NotBound,
    Closed,
    /// Local transport retirement, not a graceful provider close or model final.
    Terminated(Arc<ExperimentalLiveTerminalCloseReceipt>),
}

/// Exact transport-owned fault retained until generated close and fault
/// projection both complete. Callers cannot mint or rebind this receipt.
pub struct ExperimentalLiveTerminalCloseReceipt {
    binding: ProviderWebrtcBinding,
    observation: LiveAdapterObservation,
    _projection_custody: meerkat_live::LiveChannelCloseProjectionLease,
    report: Mutex<ExperimentalLiveTerminalReport>,
}

#[derive(Default)]
enum ExperimentalLiveTerminalReport {
    #[default]
    Pending,
    Running(JoinHandle<Result<(), meerkat_live::LiveAdapterHostError>>),
    Completed,
    Failed(meerkat_live::LiveAdapterHostError),
}

impl fmt::Debug for ExperimentalLiveTerminalCloseReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExperimentalLiveTerminalCloseReceipt")
            .field("channel_id", self.binding.channel_id())
            .finish_non_exhaustive()
    }
}

impl ExperimentalLiveTerminalCloseReceipt {
    async fn report(
        &self,
        host: &Arc<meerkat_live::LiveAdapterHost>,
        session_id: &meerkat_core::SessionId,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> Result<(), meerkat_live::LiveAdapterHostError> {
        if self.binding.session_id() != session_id || self.binding.channel_id() != channel_id {
            return Err(meerkat_live::LiveAdapterHostError::CloseNotAuthorized);
        }
        let mut report = self.report.lock().await;
        if matches!(*report, ExperimentalLiveTerminalReport::Pending) {
            let host = Arc::clone(host);
            let channel = channel_id.clone();
            let observation = self.observation.clone();
            // Cancellation of a close observer must not duplicate an already
            // published fault. The receipt owns the in-flight handoff.
            *report = ExperimentalLiveTerminalReport::Running(tokio::spawn(async move {
                host.apply_observation(&channel, &observation).await?;
                Ok(())
            }));
        }
        let task = match &mut *report {
            ExperimentalLiveTerminalReport::Running(task) => task,
            ExperimentalLiveTerminalReport::Completed => return Ok(()),
            ExperimentalLiveTerminalReport::Failed(error) => return Err(error.clone()),
            ExperimentalLiveTerminalReport::Pending => {
                return Err(meerkat_live::LiveAdapterHostError::CloseNotAuthorized);
            }
        };
        let outcome = task.await;
        match outcome {
            Ok(Ok(())) => {
                *report = ExperimentalLiveTerminalReport::Completed;
                Ok(())
            }
            Ok(Err(error)) => {
                *report = ExperimentalLiveTerminalReport::Pending;
                Err(error)
            }
            Err(error) => {
                let error = meerkat_live::LiveAdapterHostError::ProjectionError(
                    meerkat_live::LiveProjectionError::Rejected(format!(
                        "terminal fault reporting task failed: {error}"
                    )),
                );
                *report = ExperimentalLiveTerminalReport::Failed(error.clone());
                Err(error)
            }
        }
    }
}

impl ExperimentalLivePhysicalClose {
    pub(crate) async fn report_terminal(
        &self,
        host: &Arc<meerkat_live::LiveAdapterHost>,
        session_id: &meerkat_core::SessionId,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> Result<(), meerkat_live::LiveAdapterHostError> {
        if let Self::Terminated(receipt) = self {
            receipt.report(host, session_id, channel_id).await?;
        }
        Ok(())
    }
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
    factory: Arc<dyn GptLiveBrokerOpen>,
    voice: String,
    execution_mode: meerkat_core::LiveExecutionMode,
    session_instructions: Option<String>,
    initial_seed: Arc<Mutex<Option<ExperimentalGptLiveInitialSeed>>>,
}

struct ExperimentalGptLiveInitialSeed {
    context: GptLiveSeedContext,
    canonical_seed_cursor: u64,
    _projection_lease: meerkat_core::RealtimeOpenProjectionLease,
}

enum GptLiveSeedContext {
    Canonical(Vec<meerkat_core::types::Message>),
    Summary(crate::session_runtime::live_summary::LiveContextSummary),
    SeededSummary(crate::session_runtime::live_summary::LiveContextSummary),
    #[cfg(any(feature = "experimental-gpt-live", test))]
    Commentary(Option<String>),
    SeededAtCreation,
}

impl GptLiveSeedContext {
    fn canonical_messages(&self) -> Result<&[meerkat_core::types::Message], GptLiveBrokerError> {
        match self {
            Self::Canonical(messages) => Ok(messages),
            _ => Err(GptLiveBrokerError::Transport {
                class: GptLiveBrokerTerminalClass::Protocol,
            }),
        }
    }

    async fn into_commentary(self) -> Result<Option<String>, GptLiveBrokerError> {
        match self {
            Self::SeededAtCreation => Ok(None),
            Self::SeededSummary(summary) => {
                summary.validate_provider_source().await.map_err(|_| {
                    GptLiveBrokerError::Transport {
                        class: GptLiveBrokerTerminalClass::Protocol,
                    }
                })?;
                Ok(None)
            }
            #[cfg(any(feature = "experimental-gpt-live", test))]
            Self::Commentary(commentary) => Ok(commentary),
            Self::Canonical(_) | Self::Summary(_) => Err(GptLiveBrokerError::Transport {
                class: GptLiveBrokerTerminalClass::Protocol,
            }),
        }
    }
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
    fn eof_evidence(&self) -> meerkat_live::ProviderWebrtcEofEvidence {
        meerkat_live::ProviderWebrtcEofEvidence::Unconfirmed
    }

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

#[cfg(feature = "experimental-gpt-live")]
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

#[async_trait]
impl ExperimentalGptLiveBrokerSession for PublicLiveBrokerSession {
    fn eof_evidence(&self) -> meerkat_live::ProviderWebrtcEofEvidence {
        // PublicLiveBrokerSession rejects EOF lacking `session.closed`.
        meerkat_live::ProviderWebrtcEofEvidence::ProviderConfirmed
    }

    async fn await_ready_and_seed_session_context(
        &self,
        commentary: Option<String>,
    ) -> Result<(), GptLiveBrokerError> {
        PublicLiveBrokerSession::await_ready_and_seed_session_context(self, commentary).await
    }

    async fn append_session_context(
        &self,
        text: String,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        PublicLiveBrokerSession::append_session_context(self, text).await
    }

    async fn append_delegation_context(
        &self,
        delegation: &GptLiveDelegationRef,
        text: String,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        PublicLiveBrokerSession::append_delegation_context(self, delegation, text).await
    }

    async fn next_observation(
        &self,
    ) -> Result<Option<GptLiveBrokerObservation>, GptLiveBrokerError> {
        PublicLiveBrokerSession::next_observation(self).await
    }

    async fn close(&self) -> Result<(), GptLiveBrokerError> {
        PublicLiveBrokerSession::close(self).await
    }
}

/// Provider factory seam for the WebRTC broker: open one provider session for
/// a browser offer and hand back the answer SDP plus an opaque sideband.
///
/// Mode selection is sealed by the admitted execution profile. The Responses
/// function bridge has no shipping provider configuration on either path and
/// is rejected before any provider I/O.
#[async_trait]
trait GptLiveBrokerOpen: Send + Sync {
    fn supports_context_summary(&self) -> bool {
        false
    }

    fn supports_snapshot_playback_cuts(&self) -> bool {
        false
    }

    async fn open(
        &self,
        offer_sdp: &str,
        voice: &str,
        execution_mode: meerkat_core::LiveExecutionMode,
        session_instructions: Option<String>,
        seed: &mut ExperimentalGptLiveInitialSeed,
    ) -> Result<(String, Arc<dyn ExperimentalGptLiveBrokerSession>), GptLiveBrokerError>;
}

#[cfg(feature = "experimental-gpt-live")]
#[async_trait]
impl GptLiveBrokerOpen for GptLiveBrokerFactory {
    async fn open(
        &self,
        offer_sdp: &str,
        voice: &str,
        execution_mode: meerkat_core::LiveExecutionMode,
        session_instructions: Option<String>,
        seed: &mut ExperimentalGptLiveInitialSeed,
    ) -> Result<(String, Arc<dyn ExperimentalGptLiveBrokerSession>), GptLiveBrokerError> {
        let config = GptLiveBrokerOpenConfig::new(offer_sdp, voice)?;
        let mut config = match execution_mode {
            meerkat_core::LiveExecutionMode::ClientContext => config.with_client_delegation(),
            // Gate0 never promoted the raw Responses function events; no
            // shipping constructor can populate the catalog-bound bridge.
            meerkat_core::LiveExecutionMode::FunctionBridge => {
                return Err(GptLiveBrokerError::InvalidResponsesProfile);
            }
        };
        if let Some(instructions) = session_instructions {
            config = config.with_session_instructions(instructions);
        }
        let messages = seed.context.canonical_messages()?;
        let commentary = (!messages.is_empty())
            .then(|| serde_json::to_string(&serde_json::json!({ "canonical_messages": messages })))
            .transpose()
            .map_err(|_| GptLiveBrokerError::Transport {
                class: GptLiveBrokerTerminalClass::Protocol,
            })?;
        let (answer_sdp, session) = GptLiveBrokerFactory::open(self, config).await?.into_parts();
        seed.context = GptLiveSeedContext::Commentary(commentary);
        Ok((answer_sdp, Arc::new(session)))
    }
}

#[async_trait]
impl GptLiveBrokerOpen for PublicLiveBrokerFactory {
    fn supports_context_summary(&self) -> bool {
        true
    }

    fn supports_snapshot_playback_cuts(&self) -> bool {
        true
    }

    async fn open(
        &self,
        offer_sdp: &str,
        voice: &str,
        execution_mode: meerkat_core::LiveExecutionMode,
        session_instructions: Option<String>,
        seed: &mut ExperimentalGptLiveInitialSeed,
    ) -> Result<(String, Arc<dyn ExperimentalGptLiveBrokerSession>), GptLiveBrokerError> {
        if execution_mode != meerkat_core::LiveExecutionMode::ClientContext {
            return Err(GptLiveBrokerError::InvalidResponsesProfile);
        }
        let config = PublicLiveOpenConfig::new(offer_sdp, voice)?;
        let mut config = match &seed.context {
            GptLiveSeedContext::Summary(summary) => {
                summary.validate_provider_source().await.map_err(|_| {
                    GptLiveBrokerError::Transport {
                        class: GptLiveBrokerTerminalClass::Protocol,
                    }
                })?;
                config.with_context_summary(summary.text())
            }
            _ => config.with_history(seed.context.canonical_messages()?),
        };
        if let Some(instructions) = session_instructions {
            config = config.with_instructions(instructions);
        }
        let (answer_sdp, session) = PublicLiveBrokerFactory::open(self, config)
            .await?
            .into_parts();
        seed.context =
            match std::mem::replace(&mut seed.context, GptLiveSeedContext::SeededAtCreation) {
                GptLiveSeedContext::Summary(summary) => GptLiveSeedContext::SeededSummary(summary),
                _ => GptLiveSeedContext::SeededAtCreation,
            };
        Ok((answer_sdp, Arc::new(session)))
    }
}

impl fmt::Debug for ExperimentalGptLiveWebrtcBroker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalGptLiveWebrtcBroker")
            .field("factory", &"[OPAQUE]")
            .field("voice", &"[REDACTED]")
            .field("execution_mode", &self.execution_mode)
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
        factory: Arc<dyn GptLiveBrokerOpen>,
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
            session_instructions,
            initial_seed,
        })
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
        let binding = offer.binding().clone();
        let mut seed = self
            .initial_seed
            .lock()
            .await
            .take()
            .ok_or(ProviderWebrtcBrokerError::Rejected)?;
        let (answer_sdp, session) = self
            .factory
            .open(
                offer.offer_sdp(),
                &self.voice,
                self.execution_mode,
                self.session_instructions.clone(),
                &mut seed,
            )
            .await
            .map_err(map_broker_error)?;
        let (synthetic_tx, synthetic_rx) = mpsc::channel(8);
        let sideband = Arc::new(ExperimentalGptLiveSideband {
            binding,
            session,
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
        Mutex<
            mpsc::Receiver<
                Result<Option<ExperimentalGptLiveControlObservation>, ProviderWebrtcBrokerError>,
            >,
        >,
    >,
    command_actor: JoinHandle<()>,
    observation_actor: JoinHandle<()>,
    control_actor: JoinHandle<()>,
    adapter_pump: JoinHandle<()>,
    activation_gate: Arc<ExperimentalGptLiveActivationGate>,
    drain: Arc<ExperimentalGptLiveDrain>,
}

/// Receipts from provider ingress and both consumers, not a turn deadline.
#[derive(Default)]
struct ExperimentalGptLiveDrain {
    requested: AtomicBool,
    close_sent: AtomicBool,
    physically_retired: AtomicBool,
    reader: std::sync::Mutex<
        Option<Result<meerkat_live::ProviderWebrtcEofEvidence, ProviderWebrtcBrokerError>>,
    >,
    projection: std::sync::Mutex<Option<Result<(), ProviderWebrtcBrokerError>>>,
    terminal: std::sync::Mutex<Option<Arc<ExperimentalLiveTerminalCloseReceipt>>>,
    control_finished: AtomicBool,
    close_task: Mutex<Option<JoinHandle<Result<(), ProviderWebrtcBrokerError>>>>,
    retirement_task: Mutex<Option<JoinHandle<()>>>,
    projection_retry: AtomicU64,
    projection_retryable: AtomicBool,
    changed: Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExperimentalGptLiveDrainOutcome {
    Graceful,
    Terminated,
}

impl Drop for ExperimentalGptLiveDrain {
    fn drop(&mut self) {
        if let Some(task) = self.close_task.get_mut().take() {
            task.abort();
        }
    }
}

impl ExperimentalGptLiveDrain {
    async fn await_physical_retirement(&self) -> Result<(), ProviderWebrtcBrokerError> {
        let mut pending = self.retirement_task.lock().await;
        if self.physically_retired.load(Ordering::Acquire) {
            return Ok(());
        }
        let task = pending
            .as_mut()
            .ok_or(ProviderWebrtcBrokerError::Unavailable)?;
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .map_err(|_| ProviderWebrtcBrokerError::Unavailable)?
            .map_err(|_| ProviderWebrtcBrokerError::Unavailable)?;
        *pending = None;
        if !self.physically_retired.load(Ordering::Acquire) {
            return Err(ProviderWebrtcBrokerError::Unavailable);
        }
        Ok(())
    }

    fn retry_projection(&self) {
        if self.projection_retryable.swap(false, Ordering::AcqRel) {
            if let Ok(mut receipt) = self.projection.lock() {
                *receipt = None;
            }
            self.projection_retry.fetch_add(1, Ordering::AcqRel);
            self.changed.notify_waiters();
        }
    }

    async fn await_projection_retry(
        &self,
        gate: &ExperimentalGptLiveActivationGate,
        previous: u64,
    ) -> bool {
        loop {
            let changed = self.changed.notified();
            if self.projection_retry.load(Ordering::Acquire) != previous {
                return true;
            }
            tokio::select! {
                () = gate.cancelled() => return false,
                () = changed => {}
            }
        }
    }

    async fn request_close(
        &self,
        sideband: Arc<dyn ProviderWebrtcSidebandSession>,
    ) -> Result<(), ProviderWebrtcBrokerError> {
        let mut pending = self.close_task.lock().await;
        if self.close_sent.load(Ordering::Acquire) {
            return Ok(());
        }
        let task =
            pending.get_or_insert_with(|| tokio::spawn(async move { sideband.close().await }));
        // Only the observation is bounded. Cancellation or timeout leaves the
        // in-flight close owned here; a retry observes the same operation.
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .map_err(|_| ProviderWebrtcBrokerError::Unavailable)?;
        *pending = None;
        outcome.map_err(|_| ProviderWebrtcBrokerError::Unavailable)??;
        self.close_sent.store(true, Ordering::Release);
        Ok(())
    }

    fn result(&self) -> Option<Result<ExperimentalGptLiveDrainOutcome, ProviderWebrtcBrokerError>> {
        let reader = match self.reader.lock() {
            Ok(receipt) => *receipt,
            Err(_) => return Some(Err(ProviderWebrtcBrokerError::Unavailable)),
        };
        let projection = match self.projection.lock() {
            Ok(receipt) => *receipt,
            Err(_) => return Some(Err(ProviderWebrtcBrokerError::Unavailable)),
        };
        let terminal = match self.terminal.lock() {
            Ok(receipt) => receipt.is_some(),
            Err(_) => return Some(Err(ProviderWebrtcBrokerError::Unavailable)),
        };
        match (reader, projection) {
            (_, Some(Err(error))) => Some(Err(error)),
            (Some(reader), Some(Ok(()))) if self.control_finished.load(Ordering::Acquire) => {
                Some(if terminal {
                    Ok(ExperimentalGptLiveDrainOutcome::Terminated)
                } else {
                    reader.and_then(|evidence| match evidence {
                        meerkat_live::ProviderWebrtcEofEvidence::ProviderConfirmed => {
                            Ok(ExperimentalGptLiveDrainOutcome::Graceful)
                        }
                        meerkat_live::ProviderWebrtcEofEvidence::Unconfirmed => {
                            Err(ProviderWebrtcBrokerError::ProtocolDrift)
                        }
                    })
                })
            }
            _ => None,
        }
    }

    async fn retain_terminal(
        &self,
        host: &meerkat_live::LiveAdapterHost,
        binding: &ProviderWebrtcBinding,
        observation: LiveAdapterObservation,
    ) -> Result<(), ProviderWebrtcBrokerError> {
        let projection_custody = host
            .retain_channel_close_projection(binding.session_id(), binding.channel_id())
            .await
            .map_err(|_| ProviderWebrtcBrokerError::Unavailable)?;
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| ProviderWebrtcBrokerError::Unavailable)?;
        if terminal.is_none() {
            *terminal = Some(Arc::new(ExperimentalLiveTerminalCloseReceipt {
                binding: binding.clone(),
                observation,
                _projection_custody: projection_custody,
                report: Mutex::new(ExperimentalLiveTerminalReport::default()),
            }));
        }
        Ok(())
    }

    fn finish_reader(
        &self,
        result: Result<meerkat_live::ProviderWebrtcEofEvidence, ProviderWebrtcBrokerError>,
    ) {
        if let Ok(mut receipt) = self.reader.lock() {
            *receipt = Some(result);
        }
        self.changed.notify_waiters();
    }

    fn finish_projection(&self, result: Result<(), ProviderWebrtcBrokerError>) {
        if let Ok(mut receipt) = self.projection.lock() {
            *receipt = Some(result);
        }
        self.changed.notify_waiters();
    }

    async fn wait(&self) -> Result<ExperimentalGptLiveDrainOutcome, ProviderWebrtcBrokerError> {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let changed = self.changed.notified();
                if let Some(result) = self.result() {
                    return result;
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| ProviderWebrtcBrokerError::Unavailable)?
    }
}

struct RegisteredExperimentalGptLiveChannel {
    session_id: meerkat_core::SessionId,
    broker: Arc<dyn ProviderWebrtcBroker>,
    adapter: Arc<ExperimentalGptLiveDeferredAdapter>,
    identity: meerkat_core::SessionLlmIdentity,
    execution_profile_id: String,
    context_summary_provenance:
        Option<crate::session_runtime::live_summary::LiveContextSummaryProvenance>,
}

/// Opaque one-use provider custody prepared from one exact per-open admission.
/// It has no channel identity until the shared live/open pipeline succeeds.
pub struct ExperimentalGptLivePendingChannel {
    registration: RegisteredExperimentalGptLiveChannel,
    initial_seed: Arc<Mutex<Option<ExperimentalGptLiveInitialSeed>>>,
    adapter_taken: AtomicBool,
    execution_profile: meerkat_runtime::live_execution::LiveExecutionProfileSelection,
    context_summary: Option<crate::session_runtime::live_summary::LiveContextSummary>,
    supports_context_summary: bool,
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
    fn from_broker_factory(
        provider_factory: Arc<dyn GptLiveBrokerOpen>,
        identity: meerkat_core::SessionLlmIdentity,
        execution_profile: meerkat_runtime::live_execution::LiveExecutionProfileSelection,
        canonical_session_id: meerkat_core::SessionId,
        voice: impl Into<String>,
        session_instructions: Option<String>,
    ) -> Result<Self, ExperimentalGptLiveBridgeError> {
        let initial_seed = Arc::new(Mutex::new(None));
        let snapshot_cuts = provider_factory.supports_snapshot_playback_cuts();
        let supports_context_summary = provider_factory.supports_context_summary();
        let broker = ExperimentalGptLiveWebrtcBroker::new(
            provider_factory,
            voice,
            execution_profile.mode(),
            session_instructions,
            Arc::clone(&initial_seed),
        )?;
        let mut adapter = ExperimentalGptLiveDeferredAdapter::new(identity.clone());
        adapter.snapshot_cuts = snapshot_cuts;
        let adapter = Arc::new(adapter);
        Ok(Self {
            registration: RegisteredExperimentalGptLiveChannel {
                session_id: canonical_session_id,
                broker: Arc::new(broker),
                adapter,
                identity,
                execution_profile_id: execution_profile.profile_id().to_string(),
                context_summary_provenance: None,
            },
            initial_seed,
            adapter_taken: AtomicBool::new(false),
            execution_profile,
            context_summary: None,
            supports_context_summary,
        })
    }

    /// Take one resolved public Live target into provider custody without
    /// opening any provider transport. Channel binding happens only after
    /// shared live/open succeeds.
    pub fn from_public_target(
        target: ResolvedRealtimeTarget,
        execution_profile: meerkat_runtime::live_execution::LiveExecutionProfileSelection,
        canonical_session_id: meerkat_core::SessionId,
        voice: impl Into<String>,
        session_instructions: Option<String>,
    ) -> Result<Self, ExperimentalGptLiveBridgeError> {
        let identity = target.identity().clone();
        let provider_factory = PublicLiveBrokerFactory::try_from_target(target)
            .inspect_err(|error| {
                tracing::warn!(
                    stage = "provider_factory",
                    cause = live_provider_failure_class(error),
                    "public Live admission failed"
                );
            })
            .map_err(|_| ExperimentalGptLiveBridgeError::TargetRejected)?;
        Self::from_broker_factory(
            Arc::new(provider_factory),
            identity,
            execution_profile,
            canonical_session_id,
            voice,
            session_instructions,
        )
    }

    /// Same real public target, with only the provider base URL redirected to
    /// a deterministic local test server.
    #[cfg(feature = "test-realtime-fixtures")]
    #[doc(hidden)]
    pub fn __from_public_target_with_base_url(
        target: ResolvedRealtimeTarget,
        execution_profile: meerkat_runtime::live_execution::LiveExecutionProfileSelection,
        canonical_session_id: meerkat_core::SessionId,
        voice: impl Into<String>,
        session_instructions: Option<String>,
        base_url: &str,
    ) -> Result<Self, ExperimentalGptLiveBridgeError> {
        let identity = target.identity().clone();
        let provider_factory =
            PublicLiveBrokerFactory::__try_from_target_with_base_url(target, base_url)
                .inspect_err(|error| {
                    tracing::warn!(
                        stage = "provider_factory",
                        cause = live_provider_failure_class(error),
                        "public Live admission failed"
                    );
                })
                .map_err(|_| ExperimentalGptLiveBridgeError::TargetRejected)?;
        Self::from_broker_factory(
            Arc::new(provider_factory),
            identity,
            execution_profile,
            canonical_session_id,
            voice,
            session_instructions,
        )
    }

    /// Consume one exact admitted target into provider custody without opening
    /// any provider transport. Channel binding happens only after shared
    /// live/open succeeds.
    #[cfg(feature = "experimental-gpt-live")]
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
        Self::from_broker_factory(
            Arc::new(provider_factory),
            identity,
            execution_profile,
            canonical_session_id,
            voice,
            session_instructions,
        )
    }

    /// Consume the same real admission while redirecting only the provider's
    /// private external endpoints to a deterministic local test server.
    #[cfg(all(feature = "test-realtime-fixtures", feature = "experimental-gpt-live"))]
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
        Self::from_broker_factory(
            Arc::new(provider_factory),
            identity,
            execution_profile,
            canonical_session_id,
            voice,
            session_instructions,
        )
    }

    /// Project the exact admitted execution identity into the canonical
    /// session projection before the shared S5 machine admission. This does
    /// not alter durable session identity.
    pub fn apply_execution_identity(&self, projection: &mut RealtimeSessionOpenProjection) {
        projection.open_config.llm_identity = self.registration.identity.clone();
    }

    fn with_public_playback_policy(
        mut self,
        policy: PublicGptLivePlaybackPolicy,
    ) -> Result<Self, ExperimentalGptLiveBridgeError> {
        let adapter = Arc::get_mut(&mut self.registration.adapter)
            .filter(|adapter| adapter.snapshot_cuts)
            .ok_or(ExperimentalGptLiveBridgeError::TargetRejected)?;
        adapter.playback_policy = policy;
        Ok(self)
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
        if let Some(summary) = &self.context_summary {
            summary
                .validate_projection(&self.registration.session_id, open_config)
                .map_err(|error| LlmError::InvalidRequest {
                    message: error.to_string(),
                })?;
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
        let conversation_messages = open_config
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
        let context = match &self.context_summary {
            Some(summary) => GptLiveSeedContext::Summary(summary.clone()),
            None => GptLiveSeedContext::Canonical(conversation_messages),
        };
        let seed = ExperimentalGptLiveInitialSeed {
            context,
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
    observation_tx: mpsc::UnboundedSender<ExperimentalGptLiveAdapterIngress>,
    observation_rx: Mutex<mpsc::UnboundedReceiver<ExperimentalGptLiveAdapterIngress>>,
    pending_local_observations: std::sync::Mutex<VecDeque<LiveAdapterObservation>>,
    pending_local_notify: Notify,
    playback_by_item: std::sync::Mutex<HashMap<String, PendingExperimentalGptLivePlayback>>,
    drain: std::sync::Mutex<Option<Arc<ExperimentalGptLiveDrain>>>,
    closed: AtomicBool,
    closed_notify: Notify,
    snapshot_cuts: bool,
    playback_policy: PublicGptLivePlaybackPolicy,
}

enum ExperimentalGptLiveAdapterIngress {
    Provider(LiveSidebandObservation),
    SnapshotCut {
        interaction_id: meerkat_core::InteractionId,
        item_id: String,
        content_index: u32,
        reported_prefix: Option<String>,
    },
}

struct PendingExperimentalGptLivePlayback {
    provider_turn_ref: String,
    response_id: String,
    stop_reason: StopReason,
    usage: TurnUsage,
    final_forwarded: bool,
    terminal_forwarded: bool,
    snapshot: String,
    committed_prefix: String,
    output_started_forwarded: bool,
    cut_captured: bool,
    next_segment: Option<u64>,
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
            drain: std::sync::Mutex::new(None),
            closed: AtomicBool::new(false),
            closed_notify: Notify::new(),
            snapshot_cuts: false,
            playback_policy: PublicGptLivePlaybackPolicy::default(),
        }
    }

    fn push_observation(
        &self,
        observation: LiveSidebandObservation,
    ) -> Result<(), ProviderWebrtcBrokerError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ProviderWebrtcBrokerError::Rejected);
        }
        self.observation_tx
            .send(ExperimentalGptLiveAdapterIngress::Provider(observation))
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
        let _playback = self.playback_by_item.lock();
        // EOF seals ingress; exact playback custody is still needed while
        // already queued provider finals and browser reports are projected.
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.replace_status(LiveAdapterStatus::Closing);
        }
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

    fn capture_snapshot_cut(
        &self,
        interaction_id: meerkat_core::InteractionId,
        item_id: String,
        content_index: u32,
        reported_prefix: Option<String>,
    ) -> Result<LiveAdapterObservation, LiveAdapterError> {
        let mut playback =
            self.playback_by_item
                .lock()
                .map_err(|_| LiveAdapterError::ProviderError {
                    code: LiveAdapterErrorCode::InternalError,
                    message: "playback snapshot custody is unavailable".to_string(),
                })?;
        let pending = playback
            .get_mut(&item_id)
            .filter(|pending| {
                pending.terminal_forwarded && !pending.cut_captured && !pending.snapshot.is_empty()
            })
            .ok_or_else(|| LiveAdapterError::ProviderError {
                code: LiveAdapterErrorCode::InternalError,
                message: "playback cut has no exact observed snapshot".to_string(),
            })?;
        let evidence = match reported_prefix {
            Some(prefix) if pending.snapshot.starts_with(&prefix) => {
                meerkat_core::LiveAssistantPlaybackEvidence::CallerConfirmedPrefix {
                    snapshot: pending.snapshot.clone(),
                    prefix,
                }
            }
            Some(_) => {
                return Err(LiveAdapterError::ProviderError {
                    code: LiveAdapterErrorCode::InternalError,
                    message: "reported playback prefix does not match the ordered snapshot"
                        .to_string(),
                });
            }
            None => meerkat_core::LiveAssistantPlaybackEvidence::CallerConfirmedSnapshot(
                pending.snapshot.clone(),
            ),
        };
        pending.cut_captured = true;
        Ok(LiveAdapterObservation::AssistantPlaybackTerminalObserved {
            interaction_id,
            provider_item_id: item_id,
            content_index,
            response_id: pending.response_id.clone(),
            evidence,
            stop_reason: pending.stop_reason,
            usage: pending.usage.clone(),
        })
    }

    fn released_segment_turn(&self, item_id: &str) -> Option<String> {
        self.playback_by_item
            .lock()
            .ok()?
            .get(item_id)
            .filter(|pending| pending.cut_captured)
            .map(|pending| pending.provider_turn_ref.clone())
    }

    fn capture_unmeasured_segment(
        &self,
        handle: &meerkat_runtime::meerkat_machine::LiveAssistantOutputHandle,
    ) -> Result<LiveAdapterObservation, String> {
        if self.playback_policy != PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured {
            return Err("provider-managed output release was not enabled".to_string());
        }
        let (response_id, item_id, content_index) = handle
            .__target()
            .ok_or("unmeasured segment has no generated output target")?;
        let mut playback = self
            .playback_by_item
            .lock()
            .map_err(|_| "playback observation custody is unavailable")?;
        let pending = playback
            .get_mut(&item_id)
            .filter(|pending| {
                pending.response_id == response_id
                    && pending.provider_turn_ref == handle.__assistant_turn_ref()
                    && pending.output_started_forwarded
                    && !pending.snapshot.is_empty()
            })
            .ok_or("unmeasured segment has no exact observed output")?;
        pending.cut_captured = true;
        pending.terminal_forwarded = true;
        Ok(LiveAdapterObservation::AssistantPlaybackTerminalObserved {
            interaction_id: handle.interaction_id(),
            provider_item_id: item_id,
            content_index,
            response_id,
            evidence: meerkat_core::LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(
                pending.snapshot.clone(),
            ),
            stop_reason: pending.stop_reason,
            usage: pending.usage.clone(),
        })
    }

    fn settle_output_segment(&self, item_id: &str, next_segment: u64) -> Result<(), String> {
        let mut playback = self
            .playback_by_item
            .lock()
            .map_err(|_| "playback segment custody is unavailable")?;
        let pending = playback
            .get_mut(item_id)
            .filter(|pending| pending.cut_captured)
            .ok_or("settled segment has no exact output")?;
        pending.next_segment = Some(next_segment);
        if pending.final_forwarded {
            playback.remove(item_id);
        }
        Ok(())
    }

    fn lower_snapshot_delta(
        &self,
        turn: &LiveSidebandTurnRef,
        delta: &str,
    ) -> Result<Option<LiveAdapterObservation>, String> {
        let mut playback = self
            .playback_by_item
            .lock()
            .map_err(|_| "playback custody unavailable")?;
        let Some(item_id) = playback.iter().find_map(|(item, pending)| {
            (pending.provider_turn_ref == turn.adapter_key()).then(|| item.clone())
        }) else {
            return Ok(None); // User snapshots do not carry assistant playback.
        };
        let next = playback
            .get(&item_id)
            .and_then(|pending| pending.next_segment);
        let item_id = if let Some(segment) = next {
            let mut pending = playback
                .remove(&item_id)
                .ok_or("prior snapshot segment disappeared")?;
            if self.playback_policy == PublicGptLivePlaybackPolicy::CallerConfirmedSnapshots {
                pending.committed_prefix.push_str(&pending.snapshot);
            }
            pending.snapshot.clear();
            pending.response_id = format!("{}:segment:{segment}", Self::local_response_id(turn));
            pending.final_forwarded = false;
            pending.terminal_forwarded = false;
            pending.output_started_forwarded = false;
            pending.cut_captured = false;
            pending.next_segment = None;
            let next_item = format!("{}:segment:{segment}", Self::local_item_id(turn));
            playback.insert(next_item.clone(), pending);
            next_item
        } else {
            item_id
        };
        let pending = playback
            .get_mut(&item_id)
            .ok_or("snapshot segment disappeared")?;
        if pending.cut_captured {
            return Err("continuation arrived before the snapshot receipt settled".to_string());
        }
        pending.snapshot.push_str(delta);
        if !pending.output_started_forwarded && !pending.snapshot.is_empty() {
            pending.output_started_forwarded = true;
            return Ok(Some(LiveAdapterObservation::AssistantOutputStarted {
                provider_turn_ref: pending.provider_turn_ref.clone(),
                response_id: pending.response_id.clone(),
                provider_item_id: item_id,
                content_index: 0,
            }));
        }
        Ok(None)
    }

    fn lower_snapshot_final(
        &self,
        turn: &LiveSidebandTurnRef,
        transcript: &str,
    ) -> Result<Option<LiveAdapterObservation>, String> {
        let mut playback = self
            .playback_by_item
            .lock()
            .map_err(|_| "playback custody unavailable")?;
        let item_id = playback
            .iter()
            .find_map(|(item, pending)| {
                (pending.provider_turn_ref == turn.adapter_key()).then(|| item.clone())
            })
            .ok_or("assistant final has no exact snapshot target")?;
        let pending = playback
            .get_mut(&item_id)
            .ok_or("snapshot target disappeared")?;
        if pending.next_segment.is_some() {
            let prefix = format!("{}{}", pending.committed_prefix, pending.snapshot);
            let suffix = transcript
                .strip_prefix(&prefix)
                .ok_or("assistant final conflicts with an already committed snapshot")?;
            if suffix.is_empty() {
                playback.remove(&item_id);
                return Ok(None);
            }
            drop(playback);
            let started = self.lower_snapshot_delta(turn, suffix)?;
            if let Some(final_observation) = self.lower_snapshot_final(turn, transcript)? {
                self.queue_local_observation(final_observation);
            }
            return Ok(started);
        }
        if pending.cut_captured {
            return Err("assistant final arrived before snapshot settlement".to_string());
        }
        let suffix = transcript
            .strip_prefix(&pending.committed_prefix)
            .ok_or("assistant final conflicts with committed playback prefix")?;
        pending.snapshot = suffix.to_string();
        pending.final_forwarded = true;
        let final_observation = LiveAdapterObservation::AssistantTranscriptFinal {
            provider_item_id: item_id.clone(),
            previous_item_id: None,
            content_index: Some(0),
            response_id: Some(pending.response_id.clone()),
            text: suffix.to_string(),
            stop_reason: pending.stop_reason,
            usage: Usage::default(),
        };
        if !pending.output_started_forwarded {
            if suffix.is_empty() {
                playback.remove(&item_id);
                return Ok(None);
            }
            pending.output_started_forwarded = true;
            self.queue_local_observation(final_observation);
            return Ok(Some(LiveAdapterObservation::AssistantOutputStarted {
                provider_turn_ref: pending.provider_turn_ref.clone(),
                response_id: pending.response_id.clone(),
                provider_item_id: item_id,
                content_index: 0,
            }));
        }
        Ok(Some(final_observation))
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
            LiveSidebandObservationKind::TurnFinished {
                turn,
                role: LiveSidebandTurnRole::Assistant,
                ..
            } if self.playback_policy == PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured => {
                // A local grouping boundary is not provider-final speech.
                // Each observed segment already released its bookkeeping.
                let Ok(mut playback) = self.playback_by_item.lock() else {
                    return Some(LiveAdapterObservation::Error {
                        code: LiveAdapterErrorCode::InternalError,
                        message: "playback observation custody is unavailable".to_string(),
                    });
                };
                playback.retain(|_, pending| pending.provider_turn_ref != turn.adapter_key());
                None
            }
            LiveSidebandObservationKind::TurnSnapshotDelta { turn, delta }
                if self.snapshot_cuts =>
            {
                self.lower_snapshot_delta(&turn, &delta)
                    .unwrap_or_else(|message| {
                        Some(LiveAdapterObservation::Error {
                            code: LiveAdapterErrorCode::ProviderError,
                            message,
                        })
                    })
            }
            LiveSidebandObservationKind::TurnFinished {
                turn,
                role: LiveSidebandTurnRole::Assistant,
                transcript,
            } if self.snapshot_cuts => self
                .lower_snapshot_final(&turn, &transcript)
                .unwrap_or_else(|message| {
                    Some(LiveAdapterObservation::Error {
                        code: LiveAdapterErrorCode::ProviderError,
                        message,
                    })
                }),
            LiveSidebandObservationKind::SessionReady => {
                if !self.closed.load(Ordering::Acquire) {
                    self.replace_status(LiveAdapterStatus::Ready);
                }
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
                    snapshot: String::new(),
                    committed_prefix: String::new(),
                    output_started_forwarded: !self.snapshot_cuts,
                    cut_captured: false,
                    next_segment: None,
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
                if self.snapshot_cuts {
                    return None;
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
            | LiveSidebandObservationKind::AppendRejected { .. }
            | LiveSidebandObservationKind::AppendDeliveryAmbiguousTerminal { .. } => None,
        }
    }
}

#[async_trait]
impl LiveAdapter for ExperimentalGptLiveDeferredAdapter {
    async fn send_command(&self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
        if self.playback_policy == PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured
            && matches!(
                command,
                LiveAdapterCommand::CompleteAssistantPlayback { .. }
                    | LiveAdapterCommand::TruncateAssistantOutput { .. }
            )
        {
            return Err(LiveAdapterError::ProviderError {
                code: LiveAdapterErrorCode::InternalError,
                message: "provider-managed observation mode has no measured playback report"
                    .to_string(),
            });
        }
        if self.closed.load(Ordering::Acquire) && !matches!(command, LiveAdapterCommand::Close) {
            return Err(LiveAdapterError::NotReady {
                status: self.current_status(),
            });
        }
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
                if self.closed.load(Ordering::Acquire) {
                    return Err(LiveAdapterError::NotReady {
                        status: self.current_status(),
                    });
                }
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
                if self.snapshot_cuts
                    && let Some(prefix) = reported_playback_prefix.as_ref()
                {
                    if !pending.snapshot.starts_with(prefix) {
                        return Err(LiveAdapterError::ProviderError {
                            code: LiveAdapterErrorCode::InternalError,
                            message:
                                "reported playback prefix does not match the observed snapshot"
                                    .to_string(),
                        });
                    }
                    self.observation_tx
                        .send(ExperimentalGptLiveAdapterIngress::SnapshotCut {
                            interaction_id,
                            item_id,
                            content_index,
                            reported_prefix: reported_playback_prefix,
                        })
                        .map_err(|_| LiveAdapterError::ProviderError {
                            code: LiveAdapterErrorCode::InternalError,
                            message: "snapshot cut queue is unavailable".to_string(),
                        })?;
                    pending.terminal_forwarded = true;
                    return Ok(());
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
                if self.closed.load(Ordering::Acquire) {
                    return Err(LiveAdapterError::NotReady {
                        status: self.current_status(),
                    });
                }
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
                if self.snapshot_cuts {
                    if let Err(error) =
                        self.observation_tx
                            .send(ExperimentalGptLiveAdapterIngress::SnapshotCut {
                                interaction_id,
                                item_id,
                                content_index,
                                reported_prefix: None,
                            })
                    {
                        pending.terminal_forwarded = false;
                        return Err(LiveAdapterError::ProviderError {
                            code: LiveAdapterErrorCode::InternalError,
                            message: format!("playback snapshot queue failed: {error}"),
                        });
                    }
                    return Ok(());
                }
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
            let lowered = match observation {
                ExperimentalGptLiveAdapterIngress::Provider(observation) => {
                    self.lower_observation(observation)
                }
                ExperimentalGptLiveAdapterIngress::SnapshotCut {
                    interaction_id,
                    item_id,
                    content_index,
                    reported_prefix,
                } => Some(self.capture_snapshot_cut(
                    interaction_id,
                    item_id,
                    content_index,
                    reported_prefix,
                )?),
            };
            if let Some(lowered) = lowered {
                return Ok(Some(lowered));
            }
        }
    }

    fn status(&self) -> LiveAdapterStatus {
        self.current_status()
    }

    async fn close(&self) -> Result<(), LiveAdapterError> {
        let drain = self
            .drain
            .lock()
            .map_err(|_| LiveAdapterError::ProviderError {
                code: LiveAdapterErrorCode::InternalError,
                message: "experimental GPT Live close custody is unavailable".to_string(),
            })?;
        if let Some(drain) = drain.as_ref()
            && (!drain.requested.load(Ordering::Acquire)
                || !drain.physically_retired.load(Ordering::Acquire))
        {
            return Err(LiveAdapterError::ProviderError {
                code: LiveAdapterErrorCode::InternalError,
                message:
                    "experimental GPT Live provider close and canonical drain are not confirmed"
                        .to_string(),
            });
        }
        self.close_stream();
        self.replace_status(LiveAdapterStatus::Closed);
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

    fn set_context_summary(
        &mut self,
        summary: crate::session_runtime::live_summary::LiveContextSummary,
    ) -> Result<(), crate::session_runtime::live_summary::LiveContextSummaryError> {
        if !self.pending.supports_context_summary {
            return Err(crate::session_runtime::live_summary::LiveContextSummaryError::Unsupported);
        }
        self.pending.context_summary = Some(summary);
        Ok(())
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
        mut pending: ExperimentalGptLivePendingChannel,
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
        pending.registration.context_summary_provenance = pending
            .context_summary
            .as_ref()
            .map(crate::session_runtime::live_summary::LiveContextSummary::provenance);
        registrations.insert(channel_id, pending.registration);
        Ok(())
    }

    /// Read-only provenance for the exact registered channel/session pair.
    /// Unbinding removes it; this is never used as lifecycle authority.
    pub async fn bound_context_summary(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        session_id: &meerkat_core::SessionId,
    ) -> Option<crate::session_runtime::live_summary::LiveContextSummaryProvenance> {
        self.registered_by_channel
            .lock()
            .await
            .get(channel_id)
            .filter(|entry| &entry.session_id == session_id)
            .and_then(|entry| entry.context_summary_provenance.clone())
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
        let adapter = self
            .registered_by_channel
            .lock()
            .await
            .get(channel_id)
            .filter(|registration| registration.session_id == *session_id)
            .map(|registration| Arc::clone(&registration.adapter));
        let Some(adapter) = adapter else {
            return Ok(ExperimentalLivePhysicalClose::NotBound);
        };
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
        let drain = adapter
            .drain
            .lock()
            .map_err(|_| {
                provider_signaling_error(ProviderWebrtcSignalingError::SidebandClose(
                    ProviderWebrtcBrokerError::Unavailable,
                ))
            })?
            .clone();
        if let Some(drain) = drain.as_ref() {
            drain.await_physical_retirement().await.map_err(|error| {
                provider_signaling_error(ProviderWebrtcSignalingError::SidebandClose(error))
            })?;
            let terminal = drain.terminal.lock().map_err(|_| {
                provider_signaling_error(ProviderWebrtcSignalingError::SidebandClose(
                    ProviderWebrtcBrokerError::Unavailable,
                ))
            })?;
            if let Some(receipt) = terminal.as_ref() {
                return Ok(ExperimentalLivePhysicalClose::Terminated(Arc::clone(
                    receipt,
                )));
            }
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
                .filter(|current| {
                    current.binding == binding && !current.drain.requested.load(Ordering::Acquire)
                })
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

    fn observed_append_delivery(
        observation: &LiveSidebandObservationKind,
    ) -> Option<(
        &LiveSidebandAppendAttempt,
        meerkat_core::LiveAppendDeliveryOutcome,
    )> {
        match observation {
            LiveSidebandObservationKind::AppendAcknowledged { attempt } => Some((
                attempt,
                meerkat_core::LiveAppendDeliveryOutcome::Acknowledged,
            )),
            LiveSidebandObservationKind::AppendRejected { attempt } => Some((
                attempt,
                meerkat_core::LiveAppendDeliveryOutcome::InterruptedByClose,
            )),
            LiveSidebandObservationKind::AppendDeliveryAmbiguousTerminal { attempt } => {
                Some((attempt, meerkat_core::LiveAppendDeliveryOutcome::Ambiguous))
            }
            _ => None,
        }
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
        receiver
            .recv()
            .await
            .unwrap_or(Err(ProviderWebrtcBrokerError::Unavailable))
    }

    async fn route_append_delivery(
        pending_deliveries: &Mutex<
            HashMap<LiveSidebandAppendAttempt, PendingExperimentalGptLiveDelivery>,
        >,
        observation: LiveSidebandObservation,
    ) -> Result<Option<ExperimentalGptLiveControlObservation>, ProviderWebrtcBrokerError> {
        let observed_delivery = Self::observed_append_delivery(observation.kind());
        if let Some((attempt, outcome)) = observed_delivery {
            let pending = pending_deliveries.lock().await.remove(attempt);
            let Some(pending) = pending else {
                return if matches!(
                    observation.kind(),
                    LiveSidebandObservationKind::AppendDeliveryAmbiguousTerminal { .. }
                        | LiveSidebandObservationKind::AppendRejected { .. }
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
                    let resolution = ExperimentalGptLiveAppendResolution { authority, outcome };
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
                        observation: match outcome {
                            meerkat_core::LiveAppendDeliveryOutcome::Acknowledged => {
                                LiveDelegationResultDeliveryObservation::Delivered
                            }
                            meerkat_core::LiveAppendDeliveryOutcome::Rejected => {
                                LiveDelegationResultDeliveryObservation::Rejected
                            }
                            meerkat_core::LiveAppendDeliveryOutcome::Ambiguous => {
                                LiveDelegationResultDeliveryObservation::Ambiguous
                            }
                            meerkat_core::LiveAppendDeliveryOutcome::InterruptedByClose => {
                                LiveDelegationResultDeliveryObservation::InterruptedByClose
                            }
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
            Arc::clone(&self.pending_deliveries),
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
        let closing = {
            let active = self.active_by_session.lock().await;
            active
                .get(binding.session_id())
                .filter(|current| {
                    current.binding == *binding
                        && answer_observation_sequence
                            .is_none_or(|sequence| current.answer_observation_sequence == sequence)
                })
                .map(|current| {
                    current.drain.requested.store(true, Ordering::Release);
                    (
                        Arc::clone(&current.sideband),
                        Arc::clone(&current.drain),
                        current.activation_gate.committed.load(Ordering::Acquire),
                    )
                })
        };
        let Some((sideband, drain, activated)) = closing else {
            return Ok(false);
        };
        drain.retry_projection();
        let (outcome, close_result) = if activated {
            // Keep the exact binding reachable by the control consumer until
            // provider EOF and successful canonical projection are witnessed.
            tokio::select! {
                result = drain.wait() => (
                    result.map_err(ProviderWebrtcSignalingError::SidebandClose)?,
                    None,
                ),
                result = drain.request_close(Arc::clone(&sideband)) => (
                    drain.wait().await.map_err(ProviderWebrtcSignalingError::SidebandClose)?,
                    Some(result),
                ),
            }
        } else {
            (
                ExperimentalGptLiveDrainOutcome::Graceful,
                Some(drain.request_close(Arc::clone(&sideband)).await),
            )
        };
        if outcome == ExperimentalGptLiveDrainOutcome::Graceful {
            match close_result {
                Some(result) => result,
                None => drain.request_close(Arc::clone(&sideband)).await,
            }
            .map_err(ProviderWebrtcSignalingError::SidebandClose)?;
        } else {
            if let Some(Err(error)) = close_result {
                tracing::warn!(%error, "failed live transport retired locally without provider close acknowledgement");
            }
            if let Some(task) = drain.close_task.lock().await.take() {
                task.abort();
                let _ = task.await;
            }
        }
        drop(sideband);
        let mut retirement = drain.retirement_task.lock().await;
        let active = self
            .active_by_session
            .lock()
            .await
            .remove(binding.session_id());
        if let Some(active) = active {
            let retired_drain = Arc::clone(&drain);
            *retirement = Some(tokio::spawn(async move {
                retire_sideband_actors(active).await;
                retired_drain
                    .physically_retired
                    .store(true, Ordering::Release);
            }));
        }
        drop(retirement);
        drain
            .await_physical_retirement()
            .await
            .map_err(ProviderWebrtcSignalingError::SidebandClose)?;
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
    pending_deliveries: Arc<
        Mutex<HashMap<LiveSidebandAppendAttempt, PendingExperimentalGptLiveDelivery>>,
    >,
) -> ActiveExperimentalGptLiveBinding {
    let activation_gate = Arc::new(ExperimentalGptLiveActivationGate::new());
    let drain = Arc::new(ExperimentalGptLiveDrain::default());
    if let Ok(mut adapter_drain) = adapter.drain.lock() {
        *adapter_drain = Some(Arc::clone(&drain));
    }
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
    let observation_drain = Arc::clone(&drain);
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
                        LiveSidebandObservationKind::DelegationRequested { .. }
                            | LiveSidebandObservationKind::DelegationActionableInputUnsupported { .. }
                            | LiveSidebandObservationKind::AppendAcknowledged { .. }
                            | LiveSidebandObservationKind::AppendRejected { .. }
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
                    let adapter_observation = adapter_observation
                        || (observation_adapter.snapshot_cuts
                            && matches!(
                                observation.kind(),
                                LiveSidebandObservationKind::TurnSnapshotDelta { .. }
                            ));
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
                    if control_observation {
                        // Delivery receipts must reach their owned waiter even
                        // while the control consumer commits a transcript.
                        let routed = ExperimentalGptLiveWebrtcTransport::route_append_delivery(
                            &pending_deliveries,
                            observation,
                        )
                        .await;
                        if let Err(error) = observation_tx.try_send(routed) {
                            tracing::warn!(%error, "live control ingress capacity exhausted or consumer closed");
                            break;
                        }
                    }
                }
                Ok(None) => {
                    observation_drain.finish_reader(Ok(observation_sideband.eof_evidence()));
                    resolve_pending_deliveries(
                        &pending_deliveries,
                        observation_binding.channel_id(),
                        if observation_drain.requested.load(Ordering::Acquire) {
                            meerkat_core::LiveAppendDeliveryOutcome::InterruptedByClose
                        } else {
                            meerkat_core::LiveAppendDeliveryOutcome::Ambiguous
                        },
                    )
                    .await;
                    observation_adapter.close_stream();
                    let _ = observation_tx.try_send(Ok(None));
                    break;
                }
                Err(error) => {
                    observation_drain.finish_reader(Err(error));
                    observation_adapter.close_stream();
                    let _ = observation_tx.try_send(Err(error));
                    break;
                }
            }
        }
        if observation_drain
            .reader
            .lock()
            .is_ok_and(|receipt| receipt.is_none())
        {
            observation_drain.finish_reader(Err(ProviderWebrtcBrokerError::ProtocolDrift));
        }
        resolve_pending_deliveries(
            &pending_deliveries,
            observation_binding.channel_id(),
            if observation_drain.requested.load(Ordering::Acquire) {
                meerkat_core::LiveAppendDeliveryOutcome::InterruptedByClose
            } else {
                meerkat_core::LiveAppendDeliveryOutcome::Ambiguous
            },
        )
        .await;
        observation_adapter.close_stream();
    });

    let control_gate = Arc::clone(&activation_gate);
    let control_drain = Arc::clone(&drain);
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
        control_drain
            .control_finished
            .store(true, Ordering::Release);
        control_drain.changed.notify_waiters();
    });

    let pump_gate = Arc::clone(&activation_gate);
    let pump_binding = binding.clone();
    let pump_drain = Arc::clone(&drain);
    let pump_adapter = Arc::clone(&adapter);
    let adapter_pump = tokio::spawn(async move {
        let Some(activation) = pump_gate.wait_for_commit().await else {
            return;
        };
        pump_gate.mark_started();
        let mut pending_projection: Option<(
            LiveAdapterObservation,
            Option<meerkat_live::ObservationOutcome>,
        )> = None;
        loop {
            if pending_projection.is_none() {
                let next = tokio::select! {
                    () = pump_gate.cancelled() => return,
                    next = activation
                        .live_adapter_host
                        .next_observation_raw(pump_binding.channel_id()) => next,
                };
                let observation = match next {
                    Ok(Some(observation)) => observation,
                    Ok(None) => {
                        let reader = pump_drain
                            .reader
                            .lock()
                            .map(|receipt| *receipt)
                            .map_err(|_| ProviderWebrtcBrokerError::Unavailable);
                        let result = match reader {
                            Ok(Some(Ok(meerkat_live::ProviderWebrtcEofEvidence::Unconfirmed))) =>
                                pump_drain.retain_terminal(
                                    &activation.live_adapter_host,
                                    &pump_binding,
                                    LiveAdapterObservation::Error {
                                        code: LiveAdapterErrorCode::ConnectionLost,
                                        message: "live provider stream ended without provider closure confirmation".to_string(),
                                    },
                                ).await,
                            Ok(Some(Err(error))) => pump_drain.retain_terminal(
                                &activation.live_adapter_host,
                                &pump_binding,
                                LiveAdapterObservation::Error {
                                    code: LiveAdapterErrorCode::ProviderError,
                                    message: format!(
                                        "live provider observation transport terminated: {error}"
                                    ),
                                },
                            ).await,
                            Ok(_) => Ok(()),
                            Err(_) => Err(ProviderWebrtcBrokerError::Unavailable),
                        };
                        pump_drain.finish_projection(result);
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
                    let retained = pump_drain
                        .retain_terminal(&activation.live_adapter_host, &pump_binding, observation)
                        .await;
                    pump_drain.finish_projection(retained);
                    break;
                }
                pending_projection = Some((observation, None));
            }
            let Some((observation, applied)) = pending_projection.as_mut() else {
                continue;
            };
            let retry_sequence = pump_drain.projection_retry.load(Ordering::Acquire);
            let apply_result = async {
                let outcome = if let Some(outcome) = applied.as_ref() {
                    outcome.clone()
                } else {
                    let outcome = activation
                        .live_adapter_host
                        .apply_observation(pump_binding.channel_id(), observation)
                        .await
                        .map_err(|error| error.to_string())?;
                    *applied = Some(outcome.clone());
                    outcome
                };
                tracing::debug!(
                    observation = ?observation,
                    outcome = ?outcome,
                    "live adapter observation applied"
                );
                if let meerkat_live::ObservationOutcome::PlaybackTerminalSettled {
                    ref item_id, ..
                } = outcome
                    && let Some(turn) = pump_adapter.released_segment_turn(item_id)
                {
                    let Some(next) = activation.runtime.live_assistant_output_handle_for_turn(
                        pump_binding.session_id(),
                        pump_binding.channel_id(),
                        &turn,
                    ) else {
                        return Err(
                            "snapshot receipt omitted generated continuation custody".to_string()
                        );
                    };
                    pump_adapter.settle_output_segment(item_id, next.__playback_segment())?;
                }
                if let meerkat_live::ObservationOutcome::AssistantOutputAvailable(ref output) =
                    outcome
                {
                    if pump_adapter.playback_policy
                        == PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured
                    {
                        let reservation = activation
                            .runtime
                            .reserve_live_assistant_output_handle(
                                pump_binding.session_id(),
                                pump_binding.channel_id(),
                                &output.output_id,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                        let handle = reservation.handle();
                        let release = pump_adapter.capture_unmeasured_segment(handle)?;
                        let settled = activation
                            .live_adapter_host
                            .apply_observation(pump_binding.channel_id(), &release)
                            .await
                            .map_err(|error| error.to_string())?;
                        let meerkat_live::ObservationOutcome::PlaybackTerminalSettled {
                            item_id,
                            ..
                        } = settled
                        else {
                            return Err("unmeasured output release did not settle".to_string());
                        };
                        let next = activation
                            .runtime
                            .live_assistant_output_handle_for_turn(
                                pump_binding.session_id(),
                                pump_binding.channel_id(),
                                handle.__assistant_turn_ref(),
                            )
                            .ok_or("unmeasured release omitted generated continuation custody")?;
                        pump_adapter.settle_output_segment(&item_id, next.__playback_segment())?;
                        activation
                            .runtime
                            .commit_live_assistant_output_terminal(reservation)
                            .map_err(|error| error.to_string())?;
                        // No actionable playback-complete handle is published
                        // for an observation-only output.
                        return Ok(());
                    }
                    let public = ExperimentalLivePublicObservation::assistant_output_available(
                        pump_binding.clone(),
                        output.clone(),
                    );
                    activation
                        .public_observation_publisher
                        .publish(public)
                        .await
                        .map_err(|error| error.to_string())?;
                }
                if matches!(outcome, meerkat_live::ObservationOutcome::Terminal { .. }) {
                    return Err("experimental live adapter reached a terminal outcome".to_string());
                }
                Ok::<(), String>(())
            }
            .await;
            if let Err(error) = apply_result {
                tracing::warn!(error, "live projection remains retained for exact retry");
                pump_drain
                    .projection_retryable
                    .store(true, Ordering::Release);
                pump_drain.finish_projection(Err(ProviderWebrtcBrokerError::Unavailable));
                activation
                    .live_adapter_host
                    .fail_playback_waiters_for_channel(pump_binding.channel_id(), &error)
                    .await;
                if !pump_drain
                    .await_projection_retry(&pump_gate, retry_sequence)
                    .await
                {
                    return;
                }
                continue;
            }
            pending_projection = None;
        }
        if pump_drain
            .projection
            .lock()
            .is_ok_and(|receipt| receipt.is_none())
        {
            pump_drain.finish_projection(Err(ProviderWebrtcBrokerError::ProtocolDrift));
        }
        let closing = pump_drain.requested.load(Ordering::Acquire);
        if !closing {
            // Mark the exact binding nonselectable before any awaited close.
            pump_gate.cancel();
        }
        // Playback observers hold lifecycle custody. A terminal pump must
        // release them even when an explicit close is already draining.
        activation
            .live_adapter_host
            .fail_playback_waiters_for_channel(
                pump_binding.channel_id(),
                "provider observation pump retired before playback terminal settlement",
            )
            .await;
        if closing {
            return;
        }
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
        drain,
    }
}

async fn retire_pending_deliveries(
    pending_deliveries: &Mutex<
        HashMap<LiveSidebandAppendAttempt, PendingExperimentalGptLiveDelivery>,
    >,
    channel_id: &meerkat_live::LiveChannelId,
) {
    resolve_pending_deliveries(
        pending_deliveries,
        channel_id,
        meerkat_core::LiveAppendDeliveryOutcome::Ambiguous,
    )
    .await;
}

async fn resolve_pending_deliveries(
    pending_deliveries: &Mutex<
        HashMap<LiveSidebandAppendAttempt, PendingExperimentalGptLiveDelivery>,
    >,
    channel_id: &meerkat_live::LiveChannelId,
    outcome: meerkat_core::LiveAppendDeliveryOutcome,
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
                    let _ = resolution_tx
                        .send(ExperimentalGptLiveAppendResolution { authority, outcome });
                }
                PendingExperimentalGptLiveDelivery::DelegationResult {
                    authority,
                    resolution_tx,
                } => {
                    let _ = resolution_tx.send(ExperimentalGptLiveResultDeliveryResolution {
                        authority,
                        observation: match outcome {
                            meerkat_core::LiveAppendDeliveryOutcome::InterruptedByClose => {
                                LiveDelegationResultDeliveryObservation::InterruptedByClose
                            }
                            _ => LiveDelegationResultDeliveryObservation::Ambiguous,
                        },
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
        if matches!(
            self.close_physical_if_bound(&binding.channel_id, &binding.session_id)
                .await?,
            ExperimentalLivePhysicalClose::Terminated(_)
        ) {
            return Err(LiveWebrtcError::RemoteSignaling {
                reason: "terminal transport fault requires shared generated close and reporting",
            });
        }
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
    TerminalObservedBeforeCommit {
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
    AwaitingTerminal,
    TerminalAlreadyObserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebandAppendRollback {
    RolledBack,
    TerminalAlreadyObserved,
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
                Ok(SidebandAppendCommit::AwaitingTerminal)
            }
            SidebandAppendReservationState::TerminalObservedBeforeCommit {
                attempt,
                token: acknowledged,
            } if attempt == reservation.attempt && acknowledged == token => {
                Ok(SidebandAppendCommit::TerminalAlreadyObserved)
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
            SidebandAppendReservationState::TerminalObservedBeforeCommit { attempt, .. }
                if attempt == reservation.attempt =>
            {
                Ok(SidebandAppendRollback::TerminalAlreadyObserved)
            }
            _ => Err(ProviderWebrtcBrokerError::ProtocolDrift),
        }
    }

    fn observe_terminal(
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
                    SidebandAppendReservationState::TerminalObservedBeforeCommit {
                        attempt: attempt.clone(),
                        token: token.clone(),
                    },
                );
                Ok(attempt)
            }
            SidebandAppendReservationState::TerminalObservedBeforeCommit { .. } => {
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
    fn eof_evidence(&self) -> meerkat_live::ProviderWebrtcEofEvidence {
        self.session.eof_evidence()
    }

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
                        context,
                        canonical_seed_cursor: _,
                        _projection_lease,
                    } = seed;
                    let summary = match &context {
                        GptLiveSeedContext::SeededSummary(summary) => Some(summary.clone()),
                        _ => None,
                    };
                    session
                        .await_ready_and_seed_session_context(context.into_commentary().await?)
                        .await?;
                    if let Some(summary) = summary {
                        summary.validate_provider_source().await.map_err(|_| {
                            GptLiveBrokerError::Transport {
                                class: GptLiveBrokerTerminalClass::Protocol,
                            }
                        })?;
                    }
                    Ok(())
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
                    SidebandAppendCommit::AwaitingTerminal
                        | SidebandAppendCommit::TerminalAlreadyObserved
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
                if commit == SidebandAppendCommit::TerminalAlreadyObserved {
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
                if rollback == SidebandAppendRollback::TerminalAlreadyObserved {
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
                    .observe_terminal(SidebandAppendLane::SessionContext, &token)?;
                LiveSidebandObservationKind::AppendAcknowledged { attempt }
            }
            GptLiveBrokerObservation::SessionContextAppendRejected { token } => {
                let attempt = self
                    .correlations
                    .lock()
                    .await
                    .appends
                    .observe_terminal(SidebandAppendLane::SessionContext, &token)?;
                LiveSidebandObservationKind::AppendRejected { attempt }
            }
            GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token } => {
                let attempt = self
                    .correlations
                    .lock()
                    .await
                    .appends
                    .observe_terminal(SidebandAppendLane::DelegationContext, &token)?;
                LiveSidebandObservationKind::AppendAcknowledged { attempt }
            }
            GptLiveBrokerObservation::DelegationContextAppendRejected { token } => {
                let attempt = self
                    .correlations
                    .lock()
                    .await
                    .appends
                    .observe_terminal(SidebandAppendLane::DelegationContext, &token)?;
                LiveSidebandObservationKind::AppendRejected { attempt }
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
                target: meerkat_openai::gpt_live_broker::GptLiveDelegationTarget::Client,
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
            GptLiveBrokerObservation::UnsupportedProviderEvent => {
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

    fn unmeasured_fragments(session: &meerkat_core::Session) -> impl Iterator<Item = &str> {
        session
            .messages()
            .iter()
            .filter_map(|message| {
                if let meerkat_core::Message::BlockAssistant(assistant) = message {
                    Some(&assistant.blocks)
                } else {
                    None
                }
            })
            .flatten()
            .filter_map(|block| match block {
                meerkat_core::AssistantBlock::Transcript {
                    text,
                    source: meerkat_core::types::TranscriptSource::SpokenUnmeasured,
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
    }

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
    async fn append_rejection_is_a_negative_terminal_even_before_send_returns() {
        let attempt =
            LiveSidebandAppendAttempt::__from_generated_append_id("append:rejected".to_string())
                .expect("append attempt");
        let rejection = LiveSidebandObservationKind::AppendRejected {
            attempt: attempt.clone(),
        };
        assert_eq!(
            ExperimentalGptLiveWebrtcTransport::observed_append_delivery(&rejection),
            Some((
                &attempt,
                meerkat_core::LiveAppendDeliveryOutcome::InterruptedByClose
            ))
        );
        let mut correlations = SidebandAppendCorrelations::<u64>::default();
        let reservation = correlations
            .reserve(SidebandAppendLane::SessionContext, attempt.clone())
            .expect("reserve before IO");
        assert_eq!(
            correlations
                .observe_terminal(SidebandAppendLane::SessionContext, &7)
                .expect("early rejection resolves exact attempt"),
            attempt
        );
        assert_eq!(
            correlations
                .commit(&reservation, 7)
                .expect("finish send bookkeeping"),
            SidebandAppendCommit::TerminalAlreadyObserved
        );
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
                .observe_terminal(SidebandAppendLane::DelegationContext, &41)
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
            SidebandAppendCommit::TerminalAlreadyObserved
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

    struct RevocableBindingAuthority {
        inner: ExactAllowBindingAuthority,
        allowed: AtomicBool,
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl ExperimentalLiveSessionBindingAuthority for RevocableBindingAuthority {
        async fn validate_live_durable_source_availability(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<(), ExperimentalLiveOpenAuthorityError> {
            self.inner
                .validate_live_durable_source_availability(session_id)
                .await
        }

        async fn authorize_binding_use(
            &self,
            session_id: &meerkat_core::SessionId,
            binding: &meerkat_core::AuthBindingRef,
        ) -> Result<ExperimentalLiveSessionBindingAuthorization, ExperimentalLiveOpenAuthorityError>
        {
            self.attempts.fetch_add(1, AtomicOrdering::SeqCst);
            if !self.allowed.load(Ordering::Acquire) {
                return Err(ExperimentalLiveOpenAuthorityError::BindingUseDenied);
            }
            self.inner.authorize_binding_use(session_id, binding).await
        }
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

    #[derive(Debug, PartialEq, Eq)]
    enum ControlledSeedCommentary {
        NotCalled,
        Called(Option<String>),
    }

    struct ControlledSeedBrokerSession {
        seed_calls: AtomicUsize,
        provider_reads: AtomicUsize,
        started: Notify,
        release: Notify,
        commentary: Mutex<ControlledSeedCommentary>,
    }

    #[async_trait]
    impl ExperimentalGptLiveBrokerSession for ControlledSeedBrokerSession {
        async fn await_ready_and_seed_session_context(
            &self,
            commentary: Option<String>,
        ) -> Result<(), GptLiveBrokerError> {
            self.seed_calls.fetch_add(1, AtomicOrdering::SeqCst);
            *self.commentary.lock().await = ControlledSeedCommentary::Called(commentary);
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
            Ok(Some(GptLiveBrokerObservation::UnsupportedProviderEvent))
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
        fail_close: AtomicBool,
        confirmed_eof: AtomicBool,
        block_close: AtomicBool,
        close_started: AtomicBool,
        acknowledge_after_fragment_flood: AtomicBool,
        hold_context_ack: AtomicBool,
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
                fail_close: AtomicBool::new(false),
                confirmed_eof: AtomicBool::new(true),
                block_close: AtomicBool::new(false),
                close_started: AtomicBool::new(false),
                acknowledge_after_fragment_flood: AtomicBool::new(false),
                hold_context_ack: AtomicBool::new(false),
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
        fn eof_evidence(&self) -> meerkat_live::ProviderWebrtcEofEvidence {
            if self.confirmed_eof.load(Ordering::Acquire) {
                meerkat_live::ProviderWebrtcEofEvidence::ProviderConfirmed
            } else {
                meerkat_live::ProviderWebrtcEofEvidence::Unconfirmed
            }
        }

        async fn send_command(
            &self,
            command: LiveSidebandCommand,
        ) -> Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError> {
            if self.hold_context_ack.load(Ordering::Acquire) {
                return Ok(LiveSidebandCommandDelivery::Accepted);
            }
            if self
                .acknowledge_after_fragment_flood
                .load(Ordering::Acquire)
            {
                for ordinal in 0..128 {
                    self.push(LiveSidebandObservation::new(
                        command.binding().clone(),
                        LiveSidebandObservationKind::UserTranscriptFragment {
                            item: LiveSidebandTranscriptItemRef::__from_provider_observation(
                                format!("fragment-{ordinal}"),
                                format!("private-fragment-{ordinal}"),
                            )
                            .expect("fixture fragment identity"),
                            text: "provisional fragment".to_string(),
                        },
                    ));
                }
                self.push(LiveSidebandObservation::new(
                    command.binding().clone(),
                    LiveSidebandObservationKind::AppendAcknowledged {
                        attempt: command.attempt(),
                    },
                ));
                return Ok(LiveSidebandCommandDelivery::Accepted);
            }
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
            self.close_started.store(true, Ordering::Release);
            if self.block_close.load(Ordering::Acquire) {
                return futures::future::pending().await;
            }
            if self.fail_close.load(Ordering::Acquire) {
                return Err(ProviderWebrtcBrokerError::Unavailable);
            }
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
        rejected_appends: Arc<AtomicUsize>,
        control_release: Option<Arc<Notify>>,
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
            binding: meerkat_runtime::live_execution::LiveDelegationRuntimeBinding,
            control: Arc<dyn ExperimentalGptLiveControlPlane>,
        ) {
            if let Some(release) = &self.control_release {
                release.notified().await;
            }
            let provider_binding = control
                .active_binding(binding.session_id())
                .await
                .expect("control consumer starts with exact provider custody");
            loop {
                match control.next_observation(&provider_binding).await {
                    Ok(Some(ExperimentalGptLiveControlObservation::Provider(observation)))
                        if matches!(
                            observation.kind(),
                            LiveSidebandObservationKind::AppendRejected { .. }
                        ) =>
                    {
                        self.rejected_appends.fetch_add(1, AtomicOrdering::SeqCst);
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => return,
                    Err(ProviderWebrtcBrokerError::Rejected) => {
                        panic!("close removed control custody before the consumer observed EOF");
                    }
                    Err(_) => return,
                }
            }
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
        runtime: Arc<meerkat_runtime::MeerkatMachine>,
        publication_gate: Option<(Arc<Notify>, Arc<Notify>)>,
        fail_once: AtomicBool,
    }

    #[async_trait]
    impl ExperimentalLivePublicObservationPublisher for MatrixPublicObservationPublisher {
        async fn publish(
            &self,
            observation: ExperimentalLivePublicObservation,
        ) -> Result<(), ExperimentalLivePublicObservationDeliveryError> {
            if self.fail_once.swap(false, Ordering::AcqRel) {
                return Err(ExperimentalLivePublicObservationDeliveryError::Rejected);
            }
            if let Some(release) = &self.reject_release {
                self.output_tx
                    .send(observation.into_output())
                    .map_err(|_| ExperimentalLivePublicObservationDeliveryError::Closed)?;
                release.notified().await;
                return Err(ExperimentalLivePublicObservationDeliveryError::Rejected);
            }
            if let Some((entered, release)) = &self.publication_gate {
                entered.notify_one();
                release.notified().await;
            }
            let custody = self
                .runtime
                .acquire_live_binding_publication_custody(observation.binding())
                .await
                .map_err(|_| ExperimentalLivePublicObservationDeliveryError::Rejected)?;
            let meerkat_runtime::meerkat_machine::LiveBindingPublicationAdmission::Current(
                _custody,
            ) = custody
            else {
                return Err(ExperimentalLivePublicObservationDeliveryError::Rejected);
            };
            self.output_tx
                .send(observation.into_output())
                .map_err(|_| ExperimentalLivePublicObservationDeliveryError::Closed)?;
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

    type TestInitialLiveSeed = std::sync::Weak<Mutex<Option<ExperimentalGptLiveInitialSeed>>>;

    #[derive(Default)]
    struct RecoveryRegistrationBarrier {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    struct ScriptedStrictOpenAuthority {
        transport: Arc<ExperimentalGptLiveWebrtcTransport>,
        identity: meerkat_core::SessionLlmIdentity,
        latest_sideband: Mutex<Option<Arc<ControlledAmbiguousSideband>>>,
        latest_adapter: Mutex<Option<Arc<ExperimentalGptLiveDeferredAdapter>>>,
        latest_initial_seed: Mutex<Option<TestInitialLiveSeed>>,
        prepare_sequence: AtomicUsize,
        snapshot_cuts: bool,
        playback_policy: PublicGptLivePlaybackPolicy,
        recovery_registration_barrier: Mutex<Option<Arc<RecoveryRegistrationBarrier>>>,
        execution_profile: meerkat_runtime::live_execution::LiveExecutionProfileSelection,
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
                latest_initial_seed: Mutex::new(None),
                prepare_sequence: AtomicUsize::new(0),
                snapshot_cuts: false,
                playback_policy: PublicGptLivePlaybackPolicy::default(),
                recovery_registration_barrier: Mutex::new(None),
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
                pending_context_recovery: Arc::new(Mutex::new(HashMap::new())),
                pending_result_recovery: Arc::new(Mutex::new(HashMap::new())),
            }
        }

        fn with_client_context(mut self) -> Self {
            self.execution_profile = meerkat_runtime::live_execution::LiveExecutionProfileSelection::from_public_profile(
                GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID,
                meerkat_core::LiveExecutionMode::ClientContext,
                meerkat_core::LiveExecutionCapabilities { function_bridge: false, client_context: true },
            ).expect("public client-context profile");
            self
        }
    }

    #[async_trait]
    impl ExperimentalLiveOpenAuthorityProvider for ScriptedStrictOpenAuthority {
        async fn bound_execution_profile_id(
            &self,
            channel_id: &meerkat_live::LiveChannelId,
            session_id: &meerkat_core::SessionId,
        ) -> Result<String, ExperimentalLiveOpenAuthorityError> {
            self.transport
                .bound_execution_profile_id(channel_id, session_id)
                .await
                .ok_or(ExperimentalLiveOpenAuthorityError::ChannelBindingFailed)
        }

        async fn prepare_open(
            &self,
            canonical_session_id: &meerkat_core::SessionId,
            _execution_identity: &meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
        ) -> Result<Box<dyn ExperimentalLivePendingOpen>, ExperimentalLiveOpenAuthorityError>
        {
            self.prepare_sequence.fetch_add(1, AtomicOrdering::SeqCst);
            let initial_seed = Arc::new(Mutex::new(None));
            *self.latest_initial_seed.lock().await = Some(Arc::downgrade(&initial_seed));
            let mut adapter = ExperimentalGptLiveDeferredAdapter::new(self.identity.clone());
            adapter.snapshot_cuts = self.snapshot_cuts;
            adapter.playback_policy = self.playback_policy;
            let adapter = Arc::new(adapter);
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
                    execution_profile_id: self.execution_profile.profile_id().to_string(),
                    context_summary_provenance: None,
                },
                initial_seed,
                adapter_taken: AtomicBool::new(false),
                execution_profile: self.execution_profile.clone(),
                context_summary: None,
                supports_context_summary: true,
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
            let barrier = self.recovery_registration_barrier.lock().await.take();
            if let Some(barrier) = barrier {
                barrier.entered.notify_one();
                barrier.release.notified().await;
            }
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

    #[cfg(feature = "experimental-gpt-live")]
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

    #[cfg(feature = "experimental-gpt-live")]
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
                credential_account: None,
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
                    profile_id: crate::GPT_LIVE_FUNCTION_BRIDGE_PROFILE_ID.to_string(),
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

    #[cfg(feature = "experimental-gpt-live")]
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
                credential_account: None,
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

    // ----------------------------------------------------------------------
    // Public GPT Live (`gpt-live-1`) open path. These tests never touch the
    // network or real credentials: the host identity carries an inline API
    // key binding and any provider destination is a local listener.
    // ----------------------------------------------------------------------

    fn public_live_identity(
        binding: meerkat_core::AuthBindingRef,
    ) -> meerkat_core::SessionLlmIdentity {
        meerkat_core::SessionLlmIdentity {
            model: GPT_LIVE_PUBLIC_MODEL.to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(binding),
        }
    }

    fn public_live_binding(realm: &meerkat_core::RealmId) -> meerkat_core::AuthBindingRef {
        meerkat_core::AuthBindingRef {
            realm: realm.clone(),
            binding: meerkat_core::BindingId::parse("openai").expect("binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }
    }

    const PUBLIC_LIVE_FIXTURE_SECRET: &str = "sk-public-live-fixture";

    /// Realm config with one plain OpenAI API backend, an inline API-key auth
    /// profile, and the `openai` binding the public host identity selects.
    fn public_live_realm_config(realm: &meerkat_core::RealmId) -> meerkat_core::Config {
        let mut current_config = meerkat_core::Config::default();
        let mut realm_config = meerkat_core::RealmConfigSection::default();
        realm_config.backend.insert(
            "openai_api".to_string(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "openai_api".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            },
        );
        realm_config.auth.insert(
            "openai_key".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: "api_key".to_string(),
                source: meerkat_core::CredentialSourceSpec::InlineSecret {
                    secret: PUBLIC_LIVE_FIXTURE_SECRET.to_string(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        realm_config.binding.insert(
            "openai".to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "openai_api".to_string(),
                auth_profile: "openai_key".to_string(),
                credential_account: None,
                default_model: Some(GPT_LIVE_PUBLIC_MODEL.to_string()),
                policy: Default::default(),
                provider_default: true,
            },
        );
        current_config
            .realm
            .insert(realm.as_str().to_string(), realm_config);
        current_config
    }

    fn public_live_authority_config(
        realm: &meerkat_core::RealmId,
        voice: &str,
        execution_identity: meerkat_core::SessionLlmIdentity,
        config_source: Arc<dyn ExperimentalLiveCurrentConfigSource>,
        binding_authority: Arc<dyn ExperimentalLiveSessionBindingAuthority>,
        transport: Arc<ExperimentalGptLiveWebrtcTransport>,
    ) -> PublicGptLiveOpenAuthorityConfig {
        PublicGptLiveOpenAuthorityConfig {
            agent_factory: crate::AgentFactory::minimal(),
            config_source,
            binding_authority,
            execution_identity,
            realm: realm.clone(),
            transport,
            voice: voice.to_string(),
            session_instructions: None,
        }
    }

    fn public_profile_override() -> meerkat_contracts::WireLiveExecutionIdentityOverrideV1 {
        meerkat_contracts::WireLiveExecutionIdentityOverrideV1 {
            version: meerkat_contracts::WireLiveExecutionIdentityVersion::V1,
            profile_id: GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID.to_string(),
        }
    }

    #[tokio::test]
    async fn public_open_authority_rejects_wrong_profile_and_host_identity() {
        let realm = meerkat_core::RealmId::parse("voice").expect("realm");
        let other_realm = meerkat_core::RealmId::parse("elsewhere").expect("realm");
        let selected_binding = public_live_binding(&realm);
        let config_reads = Arc::new(AtomicUsize::new(0));
        let binding_calls = Arc::new(AtomicUsize::new(0));
        let transport = Arc::new(ExperimentalGptLiveWebrtcTransport::new());
        let compose = |voice: &str, execution_identity: meerkat_core::SessionLlmIdentity| {
            ExperimentalGptLiveOpenAuthority::new_public(public_live_authority_config(
                &realm,
                voice,
                execution_identity,
                Arc::new(CountingConfigSource {
                    reads: Arc::clone(&config_reads),
                    config: meerkat_core::Config::default(),
                }),
                Arc::new(NeverBindingAuthority {
                    calls: Arc::clone(&binding_calls),
                    expected: selected_binding.clone(),
                }),
                Arc::clone(&transport),
            ))
        };

        // Host composition: blank voice, wrong catalog row, and any binding
        // that is not a configured binding in the authority realm are
        // rejected before an authority exists.
        assert_eq!(
            compose("   ", public_live_identity(selected_binding.clone())).err(),
            Some(ExperimentalGptLiveOpenAuthorityError::MissingVoice)
        );
        assert_eq!(
            compose("marin", configured_live_identity(selected_binding.clone())).err(),
            Some(ExperimentalGptLiveOpenAuthorityError::InvalidExecutionIdentity),
            "the deprecated gpt-live-1-codex row is not the public model"
        );
        assert_eq!(
            compose(
                "marin",
                public_live_identity(meerkat_core::AuthBindingRef {
                    origin: meerkat_core::BindingOrigin::SyntheticEnvDefault,
                    ..selected_binding.clone()
                })
            )
            .err(),
            Some(ExperimentalGptLiveOpenAuthorityError::InvalidExecutionIdentity),
            "a synthetic env binding is not a configured host binding"
        );
        assert_eq!(
            compose(
                "marin",
                public_live_identity(public_live_binding(&other_realm))
            )
            .err(),
            Some(ExperimentalGptLiveOpenAuthorityError::InvalidExecutionIdentity),
            "the configured binding must live in the authority realm"
        );
        assert_eq!(
            compose(
                "marin",
                meerkat_core::SessionLlmIdentity {
                    auth_binding: None,
                    ..public_live_identity(selected_binding.clone())
                }
            )
            .err(),
            Some(ExperimentalGptLiveOpenAuthorityError::InvalidExecutionIdentity),
            "the host identity must name its configured binding"
        );
        assert_eq!(config_reads.load(AtomicOrdering::SeqCst), 0);

        // A well-formed authority still rejects a non-public profile before
        // any binding authorization, credential, or provider effect.
        let authority = compose("marin", public_live_identity(selected_binding.clone()))
            .expect("public authority configuration");
        assert!(matches!(
            authority.admission,
            GptLiveOpenAdmission::Public {
                playback_policy: PublicGptLivePlaybackPolicy::CallerConfirmedSnapshots,
                ..
            }
        ));
        let authority = authority
            .with_public_playback_policy(PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured)
            .expect("public authority explicitly opts in");
        assert!(matches!(
            authority.admission,
            GptLiveOpenAdmission::Public {
                playback_policy: PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured,
                ..
            }
        ));
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
            Ok(_) => panic!("the experimental profile must not open the public path"),
            Err(error) => error,
        };
        assert_eq!(error, ExperimentalLiveOpenAuthorityError::AdmissionFailed);
        assert_eq!(config_reads.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(binding_calls.load(AtomicOrdering::SeqCst), 0);

        // The exact public profile with no configured realm binding also
        // fails as admission, still without asking the binding authority.
        let error = match authority
            .prepare_open(&meerkat_core::SessionId::new(), &public_profile_override())
            .await
        {
            Ok(_) => panic!("an unconfigured realm binding must not open the public path"),
            Err(error) => error,
        };
        assert_eq!(error, ExperimentalLiveOpenAuthorityError::AdmissionFailed);
        assert_eq!(config_reads.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(binding_calls.load(AtomicOrdering::SeqCst), 0);
        assert!(transport.registered_by_channel.lock().await.is_empty());
    }

    /// Bind a local listener that records every accepted connection. The
    /// public open path must never reach it before signaling.
    #[cfg(feature = "test-realtime-fixtures")]
    async fn provider_io_tripwire() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind tripwire listener");
        let address = listener.local_addr().expect("tripwire address");
        let hits = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::clone(&hits);
        let task = tokio::spawn(async move {
            loop {
                if listener.accept().await.is_ok() {
                    recorded.fetch_add(1, AtomicOrdering::SeqCst);
                }
            }
        });
        (format!("http://{address}/v1/"), hits, task)
    }

    #[cfg(feature = "test-realtime-fixtures")]
    #[tokio::test]
    async fn public_open_authority_prepares_pending_channel_without_provider_io() {
        let realm = meerkat_core::RealmId::parse("voice").expect("realm");
        let selected_binding = public_live_binding(&realm);
        let session_id = meerkat_core::SessionId::new();
        let machine = meerkat_runtime::MeerkatMachine::ephemeral();
        let auth_lease = machine.generated_auth_lease_handle();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let config_reads = Arc::new(AtomicUsize::new(0));
        let binding_calls = Arc::new(AtomicUsize::new(0));
        let transport = Arc::new(ExperimentalGptLiveWebrtcTransport::new());
        let (tripwire_base_url, provider_hits, tripwire) = provider_io_tripwire().await;

        let authority = ExperimentalGptLiveOpenAuthority::new_public(public_live_authority_config(
            &realm,
            "marin",
            public_live_identity(selected_binding.clone()),
            Arc::new(CountingConfigSource {
                reads: Arc::clone(&config_reads),
                config: public_live_realm_config(&realm),
            }),
            Arc::new(ExactAllowBindingAuthority {
                session_id: session_id.clone(),
                expected: selected_binding,
                calls: Arc::clone(&binding_calls),
                auth_lease,
                events: Arc::clone(&events),
            }),
            Arc::clone(&transport),
        ))
        .expect("public authority configuration")
        .with_test_base_url(tripwire_base_url);

        let pending = authority
            .prepare_open(&session_id, &public_profile_override())
            .await
            .expect("public pending open");

        assert_eq!(config_reads.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(binding_calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(events.lock().expect("event log").as_slice(), ["authorize"]);

        let profile = pending.execution_profile();
        assert_eq!(
            profile.profile_id(),
            GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID
        );
        assert_eq!(
            profile.mode(),
            meerkat_core::LiveExecutionMode::ClientContext
        );
        assert_eq!(
            profile.capabilities(),
            meerkat_core::LiveExecutionCapabilities {
                function_bridge: false,
                client_context: true,
            }
        );
        let factory = pending.session_factory();
        assert_eq!(
            factory.capabilities(),
            experimental_gpt_live_realtime_capabilities()
        );
        assert!(factory.supports_provider(Provider::OpenAI));
        assert!(!factory.supports_provider(Provider::Anthropic));
        assert_eq!(
            authority
                .execution_feature_capabilities()
                .expect("public capability atoms"),
            vec![
                meerkat_contracts::LIVE_EXECUTION_IDENTITY_V1_CAPABILITY,
                meerkat_contracts::LIVE_CLIENT_CONTEXT_V1_CAPABILITY,
            ]
        );

        // Preparation is side-effect free: no channel custody and no provider
        // connection until the browser offer is answered.
        assert!(transport.registered_by_channel.lock().await.is_empty());
        drop(pending);
        authority
            .probe_execution_readiness(&session_id, &public_profile_override())
            .await
            .expect("positive readiness resolves the exact configured binding");
        assert_eq!(config_reads.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(binding_calls.load(AtomicOrdering::SeqCst), 2);
        assert!(
            authority
                .probe_execution_readiness(
                    &session_id,
                    &meerkat_contracts::WireLiveExecutionIdentityOverrideV1 {
                        version: meerkat_contracts::WireLiveExecutionIdentityVersion::V1,
                        profile_id: "unconfigured-profile".into(),
                    },
                )
                .await
                .is_err(),
            "catalog capability cannot authorize a different target profile"
        );
        tokio::task::yield_now().await;
        assert_eq!(provider_hits.load(AtomicOrdering::SeqCst), 0);
        assert!(transport.registered_by_channel.lock().await.is_empty());
        tripwire.abort();
    }

    // Local public Live wire: `POST /v1/live/sessions` returns the WebRTC
    // answer, `GET /v1/live/sessions/{id}/attach` is the scripted sideband.
    #[cfg(feature = "test-realtime-fixtures")]
    mod public_wire {
        use std::sync::Arc;

        use axum::Router;
        use axum::body::Bytes;
        use axum::extract::State;
        use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::{IntoResponse, Response};
        use axum::routing::{get, post};
        use serde_json::{Value, json};

        pub(super) const USER_TRANSCRIPT: &str = "book a table";
        pub(super) const ASSISTANT_TRANSCRIPT: &str = "one moment";
        pub(super) const DELEGATION_ID: &str = "dlg_public";
        pub(super) const ANSWER_SDP: &str = "v=0\r\nPUBLIC_ANSWER_SDP";

        #[derive(Default)]
        pub(super) struct Capture {
            pub(super) create_body: Option<Value>,
            pub(super) create_authorization: Option<String>,
            pub(super) attach_authorization: Option<String>,
            pub(super) client_events: Vec<Value>,
        }
        pub(super) type SharedCapture = Arc<std::sync::Mutex<Capture>>;

        fn input_delta(text: &str) -> Value {
            json!({"type":"session.input_transcript.delta","event_id":"i","delta":text,"start_ms":0.0,"end_ms":1.0})
        }

        fn output_delta(text: &str) -> Value {
            json!({"type":"session.output_transcript.delta","event_id":"o","delta":text,"start_ms":1.0,"end_ms":2.0})
        }

        fn delegation_created(id: &str, target: &str) -> Value {
            json!({"type":"session.delegation.created","event_id":"d","offset_ms":1.5,
                "delegation":{"type":"delegation","id":id,"target":target}})
        }

        async fn create_session(
            State(capture): State<SharedCapture>,
            headers: HeaderMap,
            body: Bytes,
        ) -> Response {
            let mut capture = capture.lock().expect("capture lock");
            capture.create_body = serde_json::from_slice(&body).ok();
            capture.create_authorization = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            (
                StatusCode::CREATED,
                [("content-type", "application/json")],
                json!({"session":{"id":"live_fixture"},"transport":{"type":"webrtc","sdp":ANSWER_SDP}})
                    .to_string(),
            )
                .into_response()
        }

        async fn attach(
            State(capture): State<SharedCapture>,
            headers: HeaderMap,
            upgrade: WebSocketUpgrade,
        ) -> Response {
            capture.lock().expect("capture lock").attach_authorization = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            upgrade.on_upgrade(move |socket| serve_sideband(socket, capture))
        }

        async fn send_json(socket: &mut WebSocket, value: Value) {
            socket
                .send(AxumMessage::Text(value.to_string().into()))
                .await
                .expect("fixture send");
        }

        async fn recv_json(socket: &mut WebSocket, capture: &SharedCapture) -> Value {
            loop {
                match socket.recv().await {
                    Some(Ok(AxumMessage::Text(text))) => {
                        let value: Value = serde_json::from_str(&text).expect("client event JSON");
                        capture
                            .lock()
                            .expect("capture lock")
                            .client_events
                            .push(value.clone());
                        return value;
                    }
                    Some(Ok(_)) => continue,
                    other => panic!("sideband closed early: {other:?}"),
                }
            }
        }

        async fn serve_sideband(mut socket: WebSocket, capture: SharedCapture) {
            let snapshot = json!({"id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":12345.5});
            send_json(
                &mut socket,
                json!({"type":"session.started","event_id":"s","session":snapshot}),
            )
            .await;
            // Media reflection and a user transcript race ahead of the seed
            // acknowledgement; the facade must preserve their order.
            send_json(
                &mut socket,
                json!({"type":"session.input_audio.append","audio":"AAAA"}),
            )
            .await;
            send_json(&mut socket, input_delta(USER_TRANSCRIPT)).await;
            send_json(&mut socket, delegation_created(DELEGATION_ID, "client")).await;
            send_json(&mut socket, output_delta(ASSISTANT_TRANSCRIPT)).await;
            let release = recv_json(&mut socket, &capture).await;
            assert_eq!(release["type"], "session.commentary.append");
            assert_eq!(release["delegation_id"], DELEGATION_ID);
            send_json(&mut socket, json!({"type":"session.commentary.appended","event_id":"a2","start_ms":2.0,"end_ms":2.0})).await;
            let close = recv_json(&mut socket, &capture).await;
            assert_eq!(close["type"], "session.close");
            send_json(&mut socket, json!({"type":"session.closed","event_id":"c","session":snapshot,"reason":"close_requested","usage":{"seconds":2.5}})).await;
            drop(socket);
        }

        pub(super) async fn local_server() -> (String, SharedCapture, tokio::task::JoinHandle<()>) {
            let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
            let app = Router::new()
                .route("/v1/live/sessions", post(create_session))
                .route("/v1/live/sessions/{session_id}/attach", get(attach))
                .with_state(Arc::clone(&capture));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind public wire listener");
            let address = listener.local_addr().expect("public wire address");
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.expect("serve public wire");
            });
            (format!("http://{address}/v1/"), capture, server)
        }
    }

    /// One resolved `gpt-live-1` target over the plain OpenAI API backend with
    /// an inline fixture secret, exactly what `resolve_public_live_target`
    /// hands the pending channel.
    #[cfg(feature = "test-realtime-fixtures")]
    fn public_fixture_target(realm: &meerkat_core::RealmId) -> ResolvedRealtimeTarget {
        use meerkat_llm_core::provider_runtime::{
            NormalizedBackendKind, ResolvedConnection, StaticLease,
        };

        let registry = meerkat_core::ModelRegistry::from_config(
            &meerkat_core::Config::default(),
            meerkat_models::canonical(),
        )
        .expect("canonical model registry");
        let selected_binding = public_live_binding(realm);
        let identity = public_live_identity(selected_binding.clone());
        let witness = registry
            .profile_witness_for_provider(Provider::OpenAI, &identity.model)
            .expect("gpt-live-1 catalog witness");
        let backend_kind = meerkat_openai::OpenAiBackendKind::OpenAiApi;
        let connection = ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(backend_kind),
            backend_profile: Arc::new(meerkat_core::connection::BackendProfile {
                id: "openai_api".to_string(),
                provider: Provider::OpenAI,
                backend_kind: backend_kind.as_str().to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            }),
            credential_identity: meerkat_core::AuthCredentialIdentity::from_auth_binding(
                &selected_binding,
            ),
            auth_lease: Arc::new(StaticLease::inline_secret(
                PUBLIC_LIVE_FIXTURE_SECRET.to_string(),
                meerkat_core::AuthMetadata::default(),
                None,
                "openai:public-live-fixture",
            )),
        };
        ResolvedRealtimeTarget::new(identity, witness, connection).expect("matching public target")
    }

    #[cfg(feature = "test-realtime-fixtures")]
    #[test]
    fn public_pending_playback_policy_defaults_measured_and_opts_in_before_open() {
        let realm = meerkat_core::RealmId::parse("voice").expect("realm");
        let pending = ExperimentalGptLivePendingChannel::from_public_target(
            public_fixture_target(&realm),
            meerkat_runtime::live_execution::LiveExecutionProfileSelection::from_public_profile(
                GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID,
                meerkat_core::LiveExecutionMode::ClientContext,
                meerkat_core::LiveExecutionCapabilities {
                    function_bridge: false,
                    client_context: true,
                },
            )
            .expect("public profile"),
            meerkat_core::SessionId::new(),
            "marin",
            None,
        )
        .expect("public pending owner");
        assert!(pending.registration.adapter.snapshot_cuts);
        assert_eq!(
            pending.registration.adapter.playback_policy,
            PublicGptLivePlaybackPolicy::CallerConfirmedSnapshots
        );
        let pending = pending
            .with_public_playback_policy(PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured)
            .expect("opt-in remains sealed in pending owner");
        assert_eq!(
            pending.registration.adapter.playback_policy,
            PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured
        );
        assert!(!pending.adapter_taken.load(Ordering::Acquire));
    }

    #[cfg(feature = "test-realtime-fixtures")]
    #[tokio::test]
    async fn public_broker_answers_offer_and_lowers_public_events_end_to_end() {
        run_public_broker_seed_end_to_end(false).await;
    }

    #[cfg(feature = "test-realtime-fixtures")]
    #[tokio::test]
    async fn public_broker_seeds_generated_summary_with_exact_canonical_cursor_end_to_end() {
        run_public_broker_seed_end_to_end(true).await;
    }

    #[cfg(feature = "test-realtime-fixtures")]
    struct PublicSeedSummarySource(crate::Session, meerkat_core::SessionLlmIdentity);

    #[cfg(feature = "test-realtime-fixtures")]
    #[async_trait]
    impl crate::session_runtime::live_summary::LiveSummarySource for PublicSeedSummarySource {
        async fn read(
            &self,
            id: &meerkat_core::SessionId,
        ) -> Result<
            (crate::Session, meerkat_core::SessionLlmIdentity),
            crate::session_runtime::live_summary::LiveContextSummaryError,
        > {
            assert_eq!(id, self.0.id());
            Ok((self.0.clone(), self.1.clone()))
        }
    }

    #[cfg(feature = "test-realtime-fixtures")]
    struct PublicSeedSummarizer;

    #[cfg(feature = "test-realtime-fixtures")]
    #[async_trait]
    impl crate::session_runtime::live_summary::LiveContextSummarizer for PublicSeedSummarizer {
        async fn summarize(
            &self,
            snapshot: crate::session_runtime::live_summary::LiveContextSummarySnapshot<'_>,
        ) -> Result<String, crate::session_runtime::live_summary::LiveContextSummaryError> {
            assert!(
                serde_json::to_string(snapshot.messages())
                    .unwrap()
                    .contains("Earlier conversation detail")
            );
            Ok("Earlier discussion covered the project.".into())
        }
    }

    #[cfg(feature = "test-realtime-fixtures")]
    async fn run_public_broker_seed_end_to_end(summarized: bool) {
        let (base_url, capture, server) = public_wire::local_server().await;
        let realm = meerkat_core::RealmId::parse("voice").expect("realm");
        let target = public_fixture_target(&realm);
        let identity = target.identity().clone();
        let session_id = meerkat_core::SessionId::new();
        let execution_profile =
            meerkat_runtime::live_execution::LiveExecutionProfileSelection::from_public_profile(
                GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID,
                meerkat_core::LiveExecutionMode::ClientContext,
                meerkat_core::LiveExecutionCapabilities {
                    function_bridge: false,
                    client_context: true,
                },
            )
            .expect("public client-context profile");
        let mut pending = ExperimentalGptLivePendingChannel::__from_public_target_with_base_url(
            target,
            execution_profile,
            session_id.clone(),
            "marin",
            Some("Catalog guidance.".to_string()),
            &base_url,
        )
        .expect("public pending channel");

        // Stage the canonical seed exactly as the shared live/open pipeline
        // does, then answer the browser offer through the neutral broker.
        // Larger than a commentary append's 500-token budget, but within
        // startup history's budget. Reopen must not prompt this history aloud.
        let history_text = "Earlier conversation detail. ".repeat(600);
        let user = meerkat_core::types::Message::User(meerkat_core::types::UserMessage::text(
            history_text.clone(),
        ));
        let open_config = seed_open_config(identity, vec![user.clone()]);
        if summarized {
            let mut source = crate::Session::with_id(session_id.clone());
            source.push(user.clone());
            let source_reader = Arc::new(PublicSeedSummarySource(
                source.clone(),
                open_config.llm_identity.clone(),
            ));
            pending.context_summary = Some(
                crate::session_runtime::live_summary::LiveContextSummaryPolicy::new(
                    Arc::new(PublicSeedSummarizer),
                    128 * 1024,
                    1024,
                    std::time::Duration::from_secs(1),
                )
                .unwrap()
                .summarize(source, &open_config, source_reader)
                .await
                .unwrap(),
            );
        }
        let expected_seed_cursor = open_config.canonical_message_cursor();
        pending
            .open_live_adapter(&open_config)
            .await
            .expect("public pending factory opens the deferred adapter");
        assert!(
            capture.lock().expect("capture lock").create_body.is_none(),
            "seed staging must not open a provider session"
        );

        let channel_id = meerkat_live::LiveChannelId::new("public-live-end-to-end");
        let offer = LiveWebrtcAdmittedOffer::from_machine_admission(
            channel_id.clone(),
            session_id.clone(),
            Some(meerkat_live::LiveWebrtcRuntimeBinding {
                generation: 1,
                fence: 1,
            }),
            "v=0\r\nOFFER_SDP".to_string(),
            meerkat_live::LiveWebrtcAnswerAdmissionSeal::__from_generated_admission(
                channel_id.clone(),
                session_id.clone(),
            ),
        )
        .into_provider_offer()
        .expect("admitted offer lowers to the provider offer");
        let binding = offer.binding().clone();
        let broker = Arc::clone(&pending.registration.broker);
        let (answer_sdp, sideband, pending_bound_ready) = broker
            .answer(offer)
            .await
            .expect("public broker answers the offer")
            .into_parts();
        assert_eq!(answer_sdp, public_wire::ANSWER_SDP);
        {
            let capture = capture.lock().expect("capture lock");
            let body = capture.create_body.as_ref().expect("create body");
            assert_eq!(body["session"]["model"], GPT_LIVE_PUBLIC_MODEL);
            assert_eq!(body["session"]["delegation"]["type"], "client");
            assert_eq!(body["session"]["audio"]["output"]["voice"], "marin");
            assert_eq!(body["session"]["instructions"], "Catalog guidance.");
            if summarized {
                assert_eq!(body["session"]["input"].as_array().unwrap().len(), 1);
                let text = body["session"]["input"][0]["content"][0]["text"]
                    .as_str()
                    .unwrap();
                assert!(text.ends_with("Earlier discussion covered the project."));
                assert!(!text.contains("Earlier conversation detail"));
                assert!(text.contains("context data, not a new user request"));
            } else {
                assert_eq!(
                    body["session"]["input"],
                    serde_json::json!([{
                        "type": "message", "role": "user",
                        "content": [{"type": "input_text", "text": history_text}]
                    }])
                );
            }
            assert_eq!(body["transport"]["type"], "webrtc");
            assert_eq!(body["transport"]["sdp"], "v=0\r\nOFFER_SDP");
            let expected_authorization = format!("Bearer {PUBLIC_LIVE_FIXTURE_SECRET}");
            assert_eq!(
                capture.create_authorization.as_deref(),
                Some(expected_authorization.as_str())
            );
            assert_eq!(
                capture.attach_authorization.as_deref(),
                Some(expected_authorization.as_str())
            );
            assert!(
                capture.client_events.is_empty(),
                "history was seeded at creation without a commentary append"
            );
        }

        // Answer delivery releases the exact startup seed receipt; SessionReady is the first
        // observation and carries the exact staged cursor.
        let receipt = pending_bound_ready
            .__resolve_after_answer_delivery()
            .await
            .expect("startup history receipt released after answer delivery");
        assert_eq!(
            receipt
                .__consume_for_generated_bind(&binding)
                .expect("receipt binds the exact answered channel"),
            expected_seed_cursor
        );
        let next = || async {
            sideband
                .next_observation()
                .await
                .expect("provider observation")
                .expect("provider observation present")
        };
        let ready = next().await;
        assert_eq!(ready.binding(), &binding);
        assert!(matches!(
            ready.kind(),
            LiveSidebandObservationKind::SessionReady
        ));
        assert!(matches!(
            next().await.kind(),
            LiveSidebandObservationKind::TurnStarted {
                role: LiveSidebandTurnRole::User,
                ..
            }
        ));
        assert!(matches!(
            next().await.kind(),
            LiveSidebandObservationKind::UserTranscriptFragment { text, .. }
                if text == public_wire::USER_TRANSCRIPT
        ));
        assert!(matches!(
            next().await.kind(),
            LiveSidebandObservationKind::TurnSnapshotDelta { .. }
        ));
        let delegation = match next().await.into_kind() {
            LiveSidebandObservationKind::DelegationRequested {
                delegation,
                final_transcript,
                ..
            } => {
                assert_eq!(final_transcript, public_wire::USER_TRANSCRIPT);
                delegation
            }
            other => panic!("expected the joined client delegation, got {other:?}"),
        };
        assert!(matches!(
            next().await.kind(),
            LiveSidebandObservationKind::TurnStarted {
                role: LiveSidebandTurnRole::Assistant,
                ..
            }
        ));
        assert!(matches!(
            next().await.kind(),
            LiveSidebandObservationKind::AssistantTranscriptFragment { text, .. }
                if text == public_wire::ASSISTANT_TRANSCRIPT
        ));
        assert!(matches!(
            next().await.kind(),
            LiveSidebandObservationKind::TurnSnapshotDelta { .. }
        ));

        // Releasing executor context for the delegation lowers to one
        // delegation-scoped commentary append and its exact acknowledgement.
        let release =
            meerkat_live::LiveSidebandReleaseAuthority::__from_generated_result_authority(
                binding.clone(),
                "public-live-result".to_string(),
                meerkat_core::LiveResultDisposition::DeferredContext,
                "content-digest".to_string(),
            )
            .expect("generated release authority");
        let command = LiveSidebandCommand::release_delegation_context(
            release,
            delegation,
            "Table booked for two.",
        )
        .expect("release command");
        let attempt = command.attempt();
        assert_eq!(
            sideband
                .send_command(command)
                .await
                .expect("release delivered"),
            LiveSidebandCommandDelivery::Accepted
        );
        assert!(matches!(
            next().await.kind(),
            LiveSidebandObservationKind::AppendAcknowledged { attempt: acked }
                if *acked == attempt
        ));

        sideband.close().await.expect("close requested");
        assert!(matches!(
            next().await.kind(),
            LiveSidebandObservationKind::TurnFinished {
                role: LiveSidebandTurnRole::Assistant,
                transcript,
                ..
            } if transcript == public_wire::ASSISTANT_TRANSCRIPT
        ));
        assert!(
            sideband
                .next_observation()
                .await
                .expect("stream end")
                .is_none()
        );

        let events = capture.lock().expect("capture lock").client_events.clone();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "session.commentary.append");
        assert_eq!(events[0]["delegation_id"], public_wire::DELEGATION_ID);
        assert_eq!(events[0]["content"], "Table booked for two.");
        assert_eq!(events[1]["type"], "session.close");
        server.abort();
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
                context_summary_provenance: None,
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
            context_summary: None,
            supports_context_summary: true,
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
        let messages = seed.context.canonical_messages().expect("canonical seed");
        let emitted = serde_json::to_value(messages).expect("canonical messages");
        assert_eq!(emitted, serde_json::json!([ordinary]));
        assert!(!emitted.to_string().contains(authority_text));
        assert!(!emitted.to_string().contains(notice_authority_text));
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
        assert!(
            seed.context
                .canonical_messages()
                .expect("canonical seed")
                .is_empty()
        );
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
    fn admission_diagnostics_preserve_typed_causes_without_secret_payloads() {
        use meerkat_client::FactoryError;
        use meerkat_llm_core::provider_runtime::errors::{ProviderAuthError, ProviderClientError};

        assert_eq!(
            live_factory_failure_class(&FactoryError::ProviderAuth(ProviderAuthError::Auth(
                meerkat_core::AuthError::LeaseAbsent
            ))),
            "lease_absent"
        );
        assert_eq!(
            live_factory_failure_class(&FactoryError::ConnectionTarget(
                meerkat_core::ConnectionTargetError::MissingDefaultBinding {
                    realm: "private realm".to_string()
                }
            )),
            "missing_default_binding"
        );
        assert_eq!(
            live_factory_failure_class(&FactoryError::ProviderAuth(
                ProviderAuthError::SourceResolutionFailed("PRIVATE_CREDENTIAL".to_string())
            )),
            "credential_source_resolution_failed"
        );
        assert_eq!(
            live_provider_failure_class(&ProviderClientError::InvalidBaseUrl(
                "https://PRIVATE_CREDENTIAL@example.invalid".to_string()
            )),
            "invalid_base_url"
        );
        assert_eq!(
            live_provider_failure_class(&ProviderClientError::MissingFeature(
                "openai-live-openai-api-backend"
            )),
            "openai-live-openai-api-backend"
        );
    }

    #[tokio::test]
    async fn production_sideband_defers_seed_until_delivery_resolution_and_emits_exact_ready() {
        let session = Arc::new(ControlledSeedBrokerSession {
            seed_calls: AtomicUsize::new(0),
            provider_reads: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
            commentary: Mutex::new(ControlledSeedCommentary::NotCalled),
        });
        let admission = meerkat_core::RealtimeOpenProjectionAdmission::new(1, 1)
            .expect("isolated seed projection admission");
        let seed = ExperimentalGptLiveInitialSeed {
            context: GptLiveSeedContext::Commentary(Some("exact canonical commentary".to_string())),
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
            &*session.commentary.lock().await,
            &ControlledSeedCommentary::Called(Some("exact canonical commentary".to_string()))
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
        let active = spawn_sideband_actors(
            binding,
            sideband,
            test_deferred_adapter(),
            1,
            retirement_tx,
            Arc::new(Mutex::new(HashMap::new())),
        );

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
    async fn snapshot_cut_freezes_only_prior_candidates_and_rotates_after_receipt() {
        let mut adapter =
            ExperimentalGptLiveDeferredAdapter::new(test_deferred_adapter().identity.clone());
        adapter.snapshot_cuts = true;
        let binding = ProviderWebrtcBinding::new(
            meerkat_live::LiveChannelId::new("snapshot-cut-order"),
            meerkat_core::SessionId::new(),
            meerkat_live::LiveRuntimeBindingGeneration::new(1),
            meerkat_live::LiveRuntimeBindingFence::new(1),
        );
        let turn = LiveSidebandTurnRef::__from_provider_observation(
            binding.channel_id(),
            "snapshot-turn".to_string(),
            "private-turn".to_string(),
        )
        .expect("grouped turn");
        for kind in [
            LiveSidebandObservationKind::TurnStarted {
                turn: turn.clone(),
                role: LiveSidebandTurnRole::Assistant,
            },
            LiveSidebandObservationKind::TurnSnapshotDelta {
                turn: turn.clone(),
                delta: "observed prefix".to_string(),
            },
        ] {
            adapter
                .push_observation(LiveSidebandObservation::new(binding.clone(), kind))
                .expect("queue candidate");
        }
        let Some(LiveAdapterObservation::AssistantOutputStarted {
            provider_item_id: first_item,
            ..
        }) = adapter.next_observation().await.expect("first output")
        else {
            panic!("output starts after a candidate exists")
        };
        let interaction_id = meerkat_core::InteractionId::new();
        adapter
            .send_command(LiveAdapterCommand::CompleteAssistantPlayback {
                interaction_id,
                item_id: first_item.clone(),
                content_index: 0,
            })
            .await
            .expect("enqueue exact cut");
        assert!(
            adapter
                .send_command(LiveAdapterCommand::CompleteAssistantPlayback {
                    interaction_id,
                    item_id: first_item.clone(),
                    content_index: 0,
                })
                .await
                .is_err(),
            "duplicate report cannot queue a second cut before settlement"
        );
        adapter
            .push_observation(LiveSidebandObservation::new(
                binding.clone(),
                LiveSidebandObservationKind::TurnSnapshotDelta {
                    turn: turn.clone(),
                    delta: " future suffix".to_string(),
                },
            ))
            .expect("queue later candidate");
        assert!(
            matches!(adapter.next_observation().await.expect("capture ordered snapshot"),
            Some(LiveAdapterObservation::AssistantPlaybackTerminalObserved {
                evidence: meerkat_core::LiveAssistantPlaybackEvidence::CallerConfirmedSnapshot(text), ..
            }) if text == "observed prefix")
        );
        adapter
            .settle_output_segment(&first_item, 1)
            .expect("observe committed receipt");
        let Some(LiveAdapterObservation::AssistantOutputStarted {
            provider_turn_ref,
            provider_item_id: next_item,
            ..
        }) = adapter.next_observation().await.expect("continuation")
        else {
            panic!("fresh playback segment")
        };
        assert_eq!(provider_turn_ref, turn.adapter_key());
        assert_ne!(first_item, next_item);
        adapter
            .push_observation(LiveSidebandObservation::new(
                binding,
                LiveSidebandObservationKind::TurnFinished {
                    turn,
                    role: LiveSidebandTurnRole::Assistant,
                    transcript: "observed prefix future suffix".to_string(),
                },
            ))
            .expect("queue eventual group final");
        assert!(
            matches!(adapter.next_observation().await.expect("only suffix remains"),
            Some(LiveAdapterObservation::AssistantTranscriptFinal { text, .. }) if text == " future suffix")
        );
    }

    #[cfg(feature = "test-realtime-fixtures")]
    #[test]
    fn public_broker_declares_snapshot_cut_capability() {
        let realm = meerkat_core::RealmId::parse("voice").expect("realm");
        let factory = PublicLiveBrokerFactory::try_from_target(public_fixture_target(&realm))
            .expect("public broker factory");
        assert!(factory.supports_snapshot_playback_cuts());
        assert!(
            !test_deferred_adapter().snapshot_cuts,
            "legacy/private adapters do not acquire public snapshot semantics by default"
        );
    }

    #[tokio::test]
    async fn close_drains_queued_final_without_erasing_early_playback_evidence() {
        for queued_start in [false, true] {
            let adapter = test_deferred_adapter();
            let binding = ProviderWebrtcBinding::new(
                meerkat_live::LiveChannelId::new("queued-final-close"),
                meerkat_core::SessionId::new(),
                meerkat_live::LiveRuntimeBindingGeneration::new(1),
                meerkat_live::LiveRuntimeBindingFence::new(1),
            );
            let turn = LiveSidebandTurnRef::__from_provider_observation(
                binding.channel_id(),
                "turn:close".to_string(),
                "private-close".to_string(),
            )
            .expect("turn identity");
            adapter
                .push_observation(LiveSidebandObservation::new(
                    binding.clone(),
                    LiveSidebandObservationKind::TurnStarted {
                        turn: turn.clone(),
                        role: LiveSidebandTurnRole::Assistant,
                    },
                ))
                .expect("queue start");
            if !queued_start {
                assert!(matches!(
                    adapter.next_observation().await.expect("read output start"),
                    Some(LiveAdapterObservation::AssistantOutputStarted { .. })
                ));
                adapter
                    .send_command(LiveAdapterCommand::CompleteAssistantPlayback {
                        interaction_id: meerkat_core::InteractionId::new(),
                        item_id: ExperimentalGptLiveDeferredAdapter::local_item_id(&turn),
                        content_index: 0,
                    })
                    .await
                    .expect("retain actual playback report before final");
            }
            let item_id = ExperimentalGptLiveDeferredAdapter::local_item_id(&turn);
            adapter
                .push_observation(LiveSidebandObservation::new(
                    binding,
                    LiveSidebandObservationKind::TurnFinished {
                        turn,
                        role: LiveSidebandTurnRole::Assistant,
                        transcript: "The final spoken reply.".to_string(),
                    },
                ))
                .expect("queue final");
            adapter.close().await.expect("seal ingress");
            adapter.close().await.expect("repeat close");
            let first = adapter
                .next_observation()
                .await
                .expect("read queued observation")
                .expect("queued observation exists after close");
            if queued_start {
                assert!(matches!(
                    first,
                    LiveAdapterObservation::AssistantOutputStarted { .. }
                ));
            } else {
                assert!(matches!(
                    first,
                    LiveAdapterObservation::AssistantPlaybackTerminalObserved {
                        evidence: meerkat_core::LiveAssistantPlaybackEvidence::PlaybackComplete,
                        ..
                    }
                ));
            }
            assert!(
                matches!(adapter.next_observation().await.expect("read queued final"),
                Some(LiveAdapterObservation::AssistantTranscriptFinal { text, .. })
                    if text == "The final spoken reply.")
            );
            assert!(
                adapter
                    .next_observation()
                    .await
                    .expect("read sealed stream")
                    .is_none()
            );
            assert!(
                adapter
                    .send_command(LiveAdapterCommand::CompleteAssistantPlayback {
                        interaction_id: meerkat_core::InteractionId::new(),
                        item_id,
                        content_index: 0,
                    })
                    .await
                    .is_err(),
                "late playback cannot mutate sealed output custody"
            );
            assert!(
                adapter
                    .next_observation()
                    .await
                    .expect("sealed stream remains empty")
                    .is_none()
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn close_timeout_retains_binding_until_all_drain_receipts_and_allows_retry() {
        let transport = ExperimentalGptLiveWebrtcTransport::new();
        let binding = ProviderWebrtcBinding::new(
            meerkat_live::LiveChannelId::new("close-drain-retry"),
            meerkat_core::SessionId::new(),
            meerkat_live::LiveRuntimeBindingGeneration::new(1),
            meerkat_live::LiveRuntimeBindingFence::new(1),
        );
        let (retirement_tx, _retirement_rx) = mpsc::channel(1);
        let active = spawn_sideband_actors(
            binding.clone(),
            Arc::new(ControlledAmbiguousSideband::new()),
            test_deferred_adapter(),
            1,
            retirement_tx,
            Arc::clone(&transport.pending_deliveries),
        );
        let drain = Arc::clone(&active.drain);
        active.observation_actor.abort();
        active.adapter_pump.abort();
        active.control_actor.abort();
        active
            .activation_gate
            .committed
            .store(true, Ordering::Release);
        transport
            .active_by_session
            .lock()
            .await
            .insert(binding.session_id().clone(), active);
        drain.finish_reader(Ok(
            meerkat_live::ProviderWebrtcEofEvidence::ProviderConfirmed,
        ));
        assert!(transport.close_exact(&binding, None).await.is_err());
        assert_eq!(
            transport.active_binding(binding.session_id()).await,
            Some(binding.clone())
        );
        drain.finish_projection(Ok(()));
        assert!(
            transport.close_exact(&binding, None).await.is_err(),
            "canonical projection alone cannot bypass unfinished control settlement"
        );
        drain.control_finished.store(true, Ordering::Release);
        assert!(
            transport
                .close_exact(&binding, None)
                .await
                .expect("retry settles")
        );
        assert!(
            !transport
                .close_exact(&binding, None)
                .await
                .expect("repeat is idempotent")
        );
        assert!(
            transport
                .active_binding(binding.session_id())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn pending_answer_close_also_requires_exact_provider_cleanup() {
        let transport = ExperimentalGptLiveWebrtcTransport::new();
        let binding = ProviderWebrtcBinding::new(
            meerkat_live::LiveChannelId::new("pending-answer-close"),
            meerkat_core::SessionId::new(),
            meerkat_live::LiveRuntimeBindingGeneration::new(1),
            meerkat_live::LiveRuntimeBindingFence::new(1),
        );
        let adapter = test_deferred_adapter();
        let (retirement_tx, _retirement_rx) = mpsc::channel(1);
        let active = spawn_sideband_actors(
            binding.clone(),
            Arc::new(ControlledAmbiguousSideband::new()),
            Arc::clone(&adapter),
            1,
            retirement_tx,
            Arc::clone(&transport.pending_deliveries),
        );
        transport
            .active_by_session
            .lock()
            .await
            .insert(binding.session_id().clone(), active);
        assert!(
            adapter.close().await.is_err(),
            "an answered provider binding cannot bypass cleanup before activation"
        );
        assert!(
            transport
                .close_exact(&binding, None)
                .await
                .expect("close unactivated binding")
        );
        adapter
            .close()
            .await
            .expect("unactivated consumers require no fabricated drain receipts");
    }

    #[tokio::test(start_paused = true)]
    async fn close_observation_timeout_does_not_cancel_or_repeat_provider_close() {
        struct GatedClose {
            calls: AtomicUsize,
            release: tokio::sync::Semaphore,
        }
        #[async_trait]
        impl ProviderWebrtcSidebandSession for GatedClose {
            async fn send_command(
                &self,
                _command: LiveSidebandCommand,
            ) -> Result<LiveSidebandCommandDelivery, ProviderWebrtcBrokerError> {
                Err(ProviderWebrtcBrokerError::Rejected)
            }
            async fn next_observation(
                &self,
            ) -> Result<Option<LiveSidebandObservation>, ProviderWebrtcBrokerError> {
                futures::future::pending().await
            }
            async fn close(&self) -> Result<(), ProviderWebrtcBrokerError> {
                self.calls.fetch_add(1, AtomicOrdering::SeqCst);
                self.release
                    .acquire()
                    .await
                    .expect("release close operation")
                    .forget();
                Ok(())
            }
        }
        let sideband = Arc::new(GatedClose {
            calls: AtomicUsize::new(0),
            release: tokio::sync::Semaphore::new(0),
        });
        let drain = ExperimentalGptLiveDrain::default();
        assert!(drain.request_close(sideband.clone()).await.is_err());
        assert_eq!(sideband.calls.load(AtomicOrdering::SeqCst), 1);
        assert!(
            !drain
                .close_task
                .lock()
                .await
                .as_ref()
                .expect("close operation remains retained")
                .is_finished()
        );
        sideband.release.add_permits(1);
        drain
            .request_close(sideband.clone())
            .await
            .expect("retry observes original close");
        drain
            .request_close(sideband.clone())
            .await
            .expect("completed close is idempotent");
        assert_eq!(sideband.calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn adapter_close_cannot_bypass_provider_and_projection_settlement() {
        let adapter = test_deferred_adapter();
        let drain = Arc::new(ExperimentalGptLiveDrain::default());
        *adapter.drain.lock().expect("adapter drain custody") = Some(Arc::clone(&drain));
        assert!(adapter.close().await.is_err());
        assert!(!adapter.closed.load(Ordering::Acquire));
        drain.requested.store(true, Ordering::Release);
        adapter.close_stream();
        assert_eq!(adapter.status(), LiveAdapterStatus::Closing);
        drain.finish_reader(Ok(
            meerkat_live::ProviderWebrtcEofEvidence::ProviderConfirmed,
        ));
        drain.finish_projection(Ok(()));
        drain.control_finished.store(true, Ordering::Release);
        assert!(
            adapter.close().await.is_err(),
            "a concurrent local close still requires the physical close call to succeed"
        );
        drain.close_sent.store(true, Ordering::Release);
        assert!(
            adapter.close().await.is_err(),
            "a provider acknowledgement does not prove local actors were retired"
        );
        drain.physically_retired.store(true, Ordering::Release);
        adapter
            .close()
            .await
            .expect("all ordered receipts permit adapter removal");
        assert_eq!(adapter.status(), LiveAdapterStatus::Closed);
        adapter
            .close()
            .await
            .expect("repeat adapter close remains idempotent");
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
            spawn_sideband_actors(
                binding.clone(),
                sideband,
                adapter,
                13,
                pump_retirement_tx,
                Arc::clone(&transport.pending_deliveries),
            ),
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
                context_summary_provenance: None,
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
    async fn shipping_close_matrix_preserves_history_or_retains_failed_custody() {
        use meerkat_contracts::{LiveOpenTransport, WireLiveTransportBootstrap};
        use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};

        #[derive(Clone, Copy)]
        enum ExitKind {
            ContextReceiptWhileControlBusy,
            ContextAppendPendingClose,
            ProviderManaged,
            SnapshotCut,
            PrefixCut,
            UnmeasuredCut,
            UnmeasuredRetry,
            LegacyCompleteRetry,
            LegacyPrefixRetry,
            ProjectionRetry,
            PublicationDrain,
            PublicationRetry,
            Graceful,
            Eof,
            UnconfirmedEof,
            UnconfirmedEofDuringClose,
            BrokerError,
            BrokerErrorCloseRejected,
            BrokerErrorDuringPendingClose,
            BrokerErrorReportRetry,
            BrokerErrorReportReceiptLost,
            BrokerErrorCloseObserverCancelled,
            PublicationRejected,
            ReceiptFailed,
        }

        for (ordinal, exit) in [
            ExitKind::ContextReceiptWhileControlBusy,
            ExitKind::ContextAppendPendingClose,
            ExitKind::ProviderManaged,
            ExitKind::SnapshotCut,
            ExitKind::PrefixCut,
            ExitKind::UnmeasuredCut,
            ExitKind::UnmeasuredRetry,
            ExitKind::LegacyCompleteRetry,
            ExitKind::LegacyPrefixRetry,
            ExitKind::ProjectionRetry,
            ExitKind::PublicationDrain,
            ExitKind::PublicationRetry,
            ExitKind::Graceful,
            ExitKind::Eof,
            ExitKind::UnconfirmedEof,
            ExitKind::UnconfirmedEofDuringClose,
            ExitKind::BrokerError,
            ExitKind::BrokerErrorCloseRejected,
            ExitKind::BrokerErrorDuringPendingClose,
            ExitKind::BrokerErrorReportRetry,
            ExitKind::BrokerErrorReportReceiptLost,
            ExitKind::BrokerErrorCloseObserverCancelled,
            ExitKind::PublicationRejected,
            ExitKind::ReceiptFailed,
        ]
        .into_iter()
        .enumerate()
        {
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
            let live_adapter_host =
                Arc::new(meerkat_live::LiveAdapterHost::new(projection.clone()));
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
            let mut authority =
                ScriptedStrictOpenAuthority::new(meerkat_core::SessionLlmIdentity {
                    model: "gpt-live-1-codex".to_string(),
                    provider: Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                });
            authority.snapshot_cuts = matches!(
                exit,
                ExitKind::SnapshotCut
                    | ExitKind::ProviderManaged
                    | ExitKind::PrefixCut
                    | ExitKind::UnmeasuredCut
                    | ExitKind::UnmeasuredRetry
                    | ExitKind::ProjectionRetry
            );
            if matches!(exit, ExitKind::ProviderManaged) {
                authority.playback_policy = PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured;
            }
            let authority = Arc::new(authority);
            let authority_trait: Arc<dyn ExperimentalLiveOpenAuthorityProvider> = authority.clone();
            let rejected_appends = Arc::new(AtomicUsize::new(0));
            let control_release = matches!(exit, ExitKind::ContextReceiptWhileControlBusy)
                .then(|| Arc::new(Notify::new()));
            let mirror_host = crate::surface::ExperimentalGptLiveContextMirrorHost::new(
                Arc::clone(&runtime),
                Arc::clone(&member_host),
                Arc::clone(&authority_trait),
                Arc::new(SerializedLifecycleTestActivator {
                    runtime: Arc::clone(&runtime),
                    rejected_appends: Arc::clone(&rejected_appends),
                    control_release: control_release.clone(),
                }),
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
                .expect("open exact pump-exit channel");
            let channel_id = opened.channel_id().clone();
            let token = match &opened.open().transport {
                WireLiveTransportBootstrap::Webrtc { token, .. } => token.clone(),
                other => panic!("expected WebRTC bootstrap, got {other:?}"),
            };
            let (output_tx, mut output_rx) = mpsc::unbounded_channel();
            let reject_release =
                matches!(exit, ExitKind::PublicationRejected).then(|| Arc::new(Notify::new()));
            let publication_gate = matches!(exit, ExitKind::PublicationDrain)
                .then(|| (Arc::new(Notify::new()), Arc::new(Notify::new())));
            let binder = authority
                .bound_ready_binder_for(
                    Arc::clone(&mirror_host) as Arc<dyn ExperimentalLiveBoundChannelActivator>,
                    Arc::clone(&live_adapter_host),
                    Arc::new(MatrixPublicObservationPublisher {
                        output_tx,
                        reject_release: reject_release.clone(),
                        runtime: Arc::clone(&runtime),
                        publication_gate: publication_gate.clone(),
                        fail_once: AtomicBool::new(matches!(exit, ExitKind::PublicationRetry)),
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
            if matches!(
                exit,
                ExitKind::ContextReceiptWhileControlBusy | ExitKind::ContextAppendPendingClose
            ) {
                sideband
                    .acknowledge_after_fragment_flood
                    .store(true, Ordering::Release);
                service
                    .append_external_user_content(
                        &session_id,
                        meerkat_core::ContentInput::Text("typed background context".to_string()),
                    )
                    .await
                    .expect("persist real background user row");
                let canonical = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("canonical snapshot")
                    .expect("session");
                let persisted = service
                    .observe_persisted_session_authority(&session_id)
                    .await
                    .expect("store authority")
                    .expect("persisted");
                let token = match persisted {
                    meerkat_runtime::store::RuntimeSessionAuthority::WholeBlob(value) => {
                        value.blob_sha256().to_string()
                    }
                    meerkat_runtime::store::RuntimeSessionAuthority::HeadCanonical(value) => {
                        value.committed_head_token().to_string()
                    }
                };
                let committed = meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(
                    Arc::new(canonical),
                )
                .expect("seal committed source");
                if matches!(exit, ExitKind::ContextAppendPendingClose) {
                    sideband.hold_context_ack.store(true, Ordering::Release);
                    let sending_runtime = Arc::clone(&runtime);
                    let sending_session = session_id.clone();
                    let sending = tokio::spawn(async move {
                        sending_runtime
                            .enqueue_committed_parent_session_boundary(
                                &sending_session,
                                &committed,
                                &token,
                            )
                            .await
                    });
                    tokio::time::timeout(std::time::Duration::from_secs(2), async {
                        while authority
                            .transport
                            .pending_deliveries
                            .lock()
                            .await
                            .is_empty()
                        {
                            tokio::task::yield_now().await;
                        }
                    })
                    .await
                    .expect("exact authorized context append awaits provider ACK");
                    tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        member_host.close_experimental_live_pending_channel(
                            authority.as_ref(),
                            &channel_id,
                            opened.pending_receipt(),
                        ),
                    )
                    .await
                    .expect("close with pending context ACK is bounded")
                    .expect("close resolves pending delivery without fake acknowledgement");
                    sending
                        .await
                        .expect("context sender joins")
                        .expect("generated InterruptedByClose");
                    assert!(
                        runtime
                            .live_session_for_active_channel(&channel_id)
                            .await
                            .is_none()
                    );
                    assert!(
                        authority
                            .transport
                            .pending_deliveries
                            .lock()
                            .await
                            .is_empty()
                    );
                    continue;
                }
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    runtime.enqueue_committed_parent_session_boundary(
                        &session_id,
                        &committed,
                        &token,
                    ),
                )
                .await
                .expect("ACK cannot wait on busy control consumer or 128 duplicate fragments")
                .expect("exact provider ACK advances canonical context");
                control_release.expect("busy control release").notify_one();
                member_host
                    .close_experimental_live_pending_channel(
                        authority.as_ref(),
                        &channel_id,
                        opened.pending_receipt(),
                    )
                    .await
                    .expect("close after independent ACK handoff");
                continue;
            }
            let mut terminal_events = service
                .subscribe_session_events(&session_id)
                .await
                .expect("observe terminal fault projection");
            let mut snapshot_events = if matches!(exit, ExitKind::ProviderManaged) {
                Some(
                    service
                        .subscribe_session_events(&session_id)
                        .await
                        .expect("snapshot events"),
                )
            } else {
                None
            };
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
                    turn: assistant_turn.clone(),
                    role: LiveSidebandTurnRole::Assistant,
                },
            ] {
                sideband.push(LiveSidebandObservation::new(binding.clone(), kind));
            }
            if matches!(
                exit,
                ExitKind::SnapshotCut
                    | ExitKind::ProviderManaged
                    | ExitKind::PrefixCut
                    | ExitKind::UnmeasuredCut
                    | ExitKind::UnmeasuredRetry
                    | ExitKind::ProjectionRetry
            ) {
                sideband.push(LiveSidebandObservation::new(
                    binding.clone(),
                    LiveSidebandObservationKind::TurnSnapshotDelta {
                        turn: assistant_turn.clone(),
                        delta: "First spoken checkpoint.".to_string(),
                    },
                ));
            }
            if matches!(exit, ExitKind::ProviderManaged) {
                use futures::{FutureExt as _, StreamExt as _};
                let mut last_turn = assistant_turn.clone();
                let mut prior_output = None;
                for group in 0..4 {
                    if group > 0 {
                        let user = LiveSidebandTurnRef::__from_provider_observation(
                            binding.channel_id(),
                            format!("continuous-user-{group}"),
                            format!("private-continuous-user-{group}"),
                        )
                        .expect("local user grouping");
                        let assistant = LiveSidebandTurnRef::__from_provider_observation(
                            binding.channel_id(),
                            format!("continuous-assistant-{group}"),
                            format!("private-continuous-assistant-{group}"),
                        )
                        .expect("local assistant grouping");
                        for kind in [
                            LiveSidebandObservationKind::TurnFinished {
                                turn: last_turn,
                                role: LiveSidebandTurnRole::Assistant,
                                transcript: "revisable grouping, not provider final".to_string(),
                            },
                            LiveSidebandObservationKind::TurnStarted {
                                turn: user.clone(),
                                role: LiveSidebandTurnRole::User,
                            },
                            LiveSidebandObservationKind::TurnFinished {
                                turn: user,
                                role: LiveSidebandTurnRole::User,
                                transcript: format!("continuous user {group}"),
                            },
                            LiveSidebandObservationKind::TurnStarted {
                                turn: assistant.clone(),
                                role: LiveSidebandTurnRole::Assistant,
                            },
                            LiveSidebandObservationKind::TurnSnapshotDelta {
                                turn: assistant.clone(),
                                delta: format!("observed assistant {group}"),
                            },
                        ] {
                            sideband.push(LiveSidebandObservation::new(binding.clone(), kind));
                        }
                        last_turn = assistant;
                    }
                    // Wait for the generated continuation, not for analyser
                    // silence, role alternation, provider done, or a browser ACK.
                    for segment in 1..=2 {
                        tokio::time::timeout(std::time::Duration::from_secs(3), async {
                            loop {
                                if let Some(handle) = runtime.live_assistant_output_handle_for_turn(
                                    &session_id,
                                    &channel_id,
                                    last_turn.adapter_key(),
                                ) && handle.__playback_segment() == segment
                                {
                                    prior_output = Some(handle);
                                    break;
                                }
                                tokio::task::yield_now().await;
                            }
                        })
                        .await
                        .expect("continuous output advances without playback reports");
                        let observed = service
                            .load_authoritative_session(&session_id)
                            .await
                            .expect("read target")
                            .expect("session");
                        assert!(
                            observed
                                .live_assistant_playback_target_for_channel(&channel_id)
                                .is_none()
                        );
                        let suffix = if segment == 1 {
                            String::new()
                        } else {
                            format!(":segment:{}", segment - 1)
                        };
                        let response = format!(
                            "{}{suffix}",
                            ExperimentalGptLiveDeferredAdapter::local_response_id(&last_turn)
                        );
                        let item = format!(
                            "{}{suffix}",
                            ExperimentalGptLiveDeferredAdapter::local_item_id(&last_turn)
                        );
                        let receipt = observed
                            .live_assistant_playback_settlement(
                                &channel_id,
                                prior_output
                                    .as_ref()
                                    .expect("continuation")
                                    .interaction_id(),
                                &response,
                                &item,
                                0,
                            )
                            .expect("unmeasured bookkeeping is durable");
                        assert!(matches!(receipt.evidence,
                            meerkat_core::LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(ref text)
                                if !text.is_empty()
                        ));
                        assert_eq!(receipt.authoritative_final, None);
                        assert_eq!(
                            receipt.completion, None,
                            "no synthetic stop reason or provider usage enters durable evidence"
                        );
                        assert!(
                            output_rx.try_recv().is_err(),
                            "unmeasured mode never publishes a measured playback handle"
                        );
                        if segment == 1 {
                            sideband.push(LiveSidebandObservation::new(
                                binding.clone(),
                                LiveSidebandObservationKind::TurnSnapshotDelta {
                                    turn: last_turn.clone(),
                                    delta: " continuing same group".to_string(),
                                },
                            ));
                        }
                    }
                    let canonical = service
                        .load_authoritative_session(&session_id)
                        .await
                        .expect("read canonical users")
                        .expect("session");
                    assert_eq!(
                        canonical
                            .messages()
                            .iter()
                            .filter(|message| matches!(message, meerkat_core::Message::User(_)))
                            .count(),
                        group + 1,
                    );
                    assert_eq!(
                        unmeasured_fragments(&canonical).count(),
                        (group + 1) * 2,
                        "every assistant fragment is retained despite absent playback measurement"
                    );
                    let context = serde_json::to_string(&canonical.messages_for_model_boundary())
                        .expect("project retained dialogue");
                    assert!(context.contains("First spoken checkpoint."));
                    assert!(context.contains("spoken_unmeasured"));
                }
                let old_output = prior_output.expect("generated continuation handle");
                while let Some(Some(envelope)) = snapshot_events
                    .as_mut()
                    .expect("snapshot event subscription")
                    .next()
                    .now_or_never()
                {
                    assert!(
                        !matches!(
                            envelope.payload,
                            meerkat_core::AgentEvent::TurnCompleted { .. }
                                | meerkat_core::AgentEvent::TextComplete { .. }
                        ),
                        "nonterminal transcript snapshots must not emit completion events"
                    );
                }
                let original_context = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("read replacement context")
                    .expect("session");
                assert_eq!(unmeasured_fragments(&original_context).count(), 8);
                let voice_context = original_context.messages_for_model_boundary();
                let rendered = serde_json::to_string(&voice_context).expect("live context");
                assert!(rendered.contains("First spoken checkpoint."));
                assert!(rendered.contains("observed assistant 3"));
                assert!(rendered.contains("spoken_unmeasured"));
                assert_eq!(
                    voice_context,
                    original_context.messages_for_model_boundary(),
                    "observation projection is deterministic for summary equality"
                );
                assert!(
                    adapter
                        .send_command(LiveAdapterCommand::CompleteAssistantPlayback {
                            interaction_id: old_output.interaction_id(),
                            item_id: "not-a-playback-target".to_string(),
                            content_index: 0,
                        })
                        .await
                        .is_err(),
                    "observation mode rejects measured completion"
                );
                member_host
                    .close_live_channel(Some(authority.as_ref()), &channel_id)
                    .await
                    .expect("continuous channel drains and closes without playback ACK");
                assert!(
                    runtime
                        .reserve_live_assistant_output_handle(
                            &session_id,
                            &channel_id,
                            old_output.output_id(),
                        )
                        .await
                        .is_err(),
                    "closed output handles cannot regain custody"
                );
                assert!(
                    adapter
                        .push_observation(LiveSidebandObservation::new(
                            binding,
                            LiveSidebandObservationKind::TurnSnapshotDelta {
                                turn: last_turn,
                                delta: "late old-channel fragment".to_string(),
                            },
                        ))
                        .is_err()
                );
                assert!(
                    service
                        .load_authoritative_session(&session_id)
                        .await
                        .expect("closed target")
                        .expect("session")
                        .live_assistant_playback_target_for_channel(&channel_id)
                        .is_none()
                );
                let reopened = member_host
                    .open_with_execution_identity(
                        authority.as_ref(),
                        &session_id,
                        &execution_identity,
                        None,
                        None,
                        Some(LiveOpenTransport::Webrtc),
                    )
                    .await
                    .expect("same session admits a replacement channel");
                assert_ne!(reopened.channel_id(), &channel_id);
                let replacement_seed = authority
                    .latest_initial_seed
                    .lock()
                    .await
                    .clone()
                    .and_then(|seed| seed.upgrade())
                    .expect("replacement seed custody");
                let replacement_context = replacement_seed
                    .lock()
                    .await
                    .as_ref()
                    .expect("replacement seed")
                    .context
                    .canonical_messages()
                    .expect("raw replacement context")
                    .to_vec();
                assert_eq!(
                    replacement_context,
                    voice_context
                        .into_iter()
                        .filter(|message| !matches!(message, meerkat_core::Message::System(_)))
                        .collect::<Vec<_>>(),
                    "replacement preserves prior dialogue without promoting it to played speech"
                );
                assert!(
                    runtime
                        .reserve_live_assistant_output_handle(
                            &session_id,
                            reopened.channel_id(),
                            old_output.output_id(),
                        )
                        .await
                        .is_err(),
                    "old handles cannot affect a replacement channel"
                );
                member_host
                    .close_live_channel(Some(authority.as_ref()), reopened.channel_id())
                    .await
                    .expect("close pending replacement");
                continue;
            }
            if let Some((entered, release)) = publication_gate {
                entered.notified().await;
                let closing_host = Arc::clone(&member_host);
                let closing_authority = Arc::clone(&authority);
                let closing_channel = channel_id.clone();
                let close = tokio::spawn(async move {
                    closing_host
                        .close_live_channel(Some(closing_authority.as_ref()), &closing_channel)
                        .await
                });
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        if authority
                            .transport
                            .active_by_session
                            .lock()
                            .await
                            .get(&session_id)
                            .is_some_and(|active| active.drain.requested.load(Ordering::Acquire))
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("close reaches ordered drain");
                release.notify_one();
                tokio::time::timeout(std::time::Duration::from_secs(2), close)
                    .await
                    .expect("real publication lifecycle custody must not deadlock close")
                    .expect("close task")
                    .expect("drain then terminal commit");
                assert!(
                    output_rx.recv().await.is_some(),
                    "already-admitted publication completes before teardown"
                );
                continue;
            }
            if matches!(exit, ExitKind::PublicationRetry) {
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        if authority
                            .transport
                            .active_by_session
                            .lock()
                            .await
                            .get(&session_id)
                            .is_some_and(|active| {
                                active.drain.projection_retryable.load(Ordering::Acquire)
                            })
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("failed publication remains retryable");
                member_host
                    .close_live_channel(Some(authority.as_ref()), &channel_id)
                    .await
                    .expect("close retries exact publication instead of re-admitting output");
                assert!(
                    output_rx.recv().await.is_some(),
                    "same admitted output is eventually published"
                );
                assert!(
                    output_rx.try_recv().is_err(),
                    "publication retry cannot duplicate output admission"
                );
                continue;
            }
            let output = tokio::time::timeout(std::time::Duration::from_secs(2), output_rx.recv())
                .await
                .expect("opaque output publication is prompt")
                .expect("matrix publisher remains present");
            if matches!(exit, ExitKind::UnmeasuredRetry) {
                let before = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("read pre-terminal history")
                    .expect("session");
                live_adapter_host
                    .__fail_next_projection_for_test(channel_id.clone(), true)
                    .await;
                assert!(
                    member_host
                        .truncate_live_output(
                            &channel_id,
                            &activation_receipt,
                            &output.output_id,
                            100,
                            None,
                        )
                        .await
                        .is_err(),
                    "lost post-commit acknowledgement is explicit"
                );
                assert!(
                    member_host
                        .truncate_live_output(
                            &channel_id,
                            &activation_receipt,
                            &output.output_id,
                            100,
                            None,
                        )
                        .await
                        .is_err(),
                    "internal retry cannot restore stale caller permission"
                );
                let committed = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("read unmeasured commit")
                    .expect("session");
                assert_eq!(committed.messages(), before.messages());
                assert!(
                    committed
                        .live_assistant_playback_target_for_channel(&channel_id)
                        .is_none()
                );
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    member_host.close_live_channel(Some(authority.as_ref()), &channel_id),
                )
                .await
                .expect("unmeasured internal retry must not poison close")
                .expect("close recovers exact durable unmeasured settlement");
                let after = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("read after retry")
                    .expect("session");
                assert_eq!(
                    after.messages(),
                    before.messages(),
                    "retry cannot create heard text"
                );
                continue;
            }
            if matches!(
                exit,
                ExitKind::LegacyCompleteRetry | ExitKind::LegacyPrefixRetry
            ) {
                let (response_id, item_id, index) = runtime
                    .live_assistant_output_handle(&output.output_id)
                    .expect("exact output")
                    .__target()
                    .expect("bound target");
                let report_host = Arc::clone(&member_host);
                let report_channel = channel_id.clone();
                let report_activation = activation_receipt.clone();
                let report_output = output.output_id.clone();
                let prefix_report = matches!(exit, ExitKind::LegacyPrefixRetry);
                let report = tokio::spawn(async move {
                    if prefix_report {
                        report_host
                            .truncate_live_output(
                                &report_channel,
                                &report_activation,
                                &report_output,
                                100,
                                Some("played".to_string()),
                            )
                            .await
                            .map(|_| ())
                    } else {
                        report_host
                            .complete_live_playback(
                                &report_channel,
                                &report_activation,
                                &report_output,
                            )
                            .await
                            .map(|_| ())
                    }
                });
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        if service
                            .live_assistant_playback_target(
                                &session_id,
                                channel_id.clone(),
                                item_id.clone(),
                                index,
                            )
                            .await
                            .expect("target read")
                            .is_some_and(|target| target.pending_terminal().is_some())
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("legacy terminal retained before final");
                live_adapter_host
                    .__fail_next_projection_for_test(channel_id.clone(), true)
                    .await;
                sideband.push(LiveSidebandObservation::new(
                    binding.clone(),
                    LiveSidebandObservationKind::TurnFinished {
                        turn: assistant_turn,
                        role: LiveSidebandTurnRole::Assistant,
                        transcript: "played and remaining".to_string(),
                    },
                ));
                assert!(
                    tokio::time::timeout(std::time::Duration::from_secs(2), report)
                        .await
                        .expect("failed final receipt observed")
                        .expect("report task")
                        .is_err()
                );
                let committed = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("read legacy commit")
                    .expect("session");
                assert!(
                    committed
                        .live_assistant_playback_target_for_channel(&channel_id)
                        .is_none()
                );
                assert!(
                    runtime
                        .live_assistant_output_handle_for_target(
                            &session_id,
                            &channel_id,
                            &response_id,
                            &item_id,
                            index,
                        )
                        .is_some(),
                    "spent identity remains for internal final retry"
                );
                let stale_refused = if prefix_report {
                    member_host
                        .truncate_live_output(
                            &channel_id,
                            &activation_receipt,
                            &output.output_id,
                            100,
                            Some("played".to_string()),
                        )
                        .await
                        .is_err()
                } else {
                    member_host
                        .complete_live_playback(&channel_id, &activation_receipt, &output.output_id)
                        .await
                        .is_err()
                };
                assert!(
                    stale_refused,
                    "legacy internal settlement replay cannot restore caller permission"
                );
                member_host
                    .close_live_channel(Some(authority.as_ref()), &channel_id)
                    .await
                    .expect("legacy final retry recovers the recorded terminal receipt");
                let after = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("read retried legacy")
                    .expect("session");
                assert_eq!(after.messages(), committed.messages());
                continue;
            }
            if matches!(exit, ExitKind::ProjectionRetry) {
                live_adapter_host
                    .__fail_next_projection_for_test(channel_id.clone(), true)
                    .await;
                assert!(
                    member_host
                        .complete_live_playback(&channel_id, &activation_receipt, &output.output_id)
                        .await
                        .is_err(),
                    "post-commit projection receipt failure stays explicit"
                );
                let committed = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("read partial commit")
                    .expect("session");
                assert!(format!("{:?}", committed.messages()).contains("First spoken checkpoint."));
                // Fail the first close retry before application; the second
                // retries the identical cut after its canonical commit.
                live_adapter_host
                    .__fail_next_projection_for_test(channel_id.clone(), false)
                    .await;
                assert!(
                    member_host
                        .close_live_channel(Some(authority.as_ref()), &channel_id)
                        .await
                        .is_err()
                );
                member_host
                    .close_live_channel(Some(authority.as_ref()), &channel_id)
                    .await
                    .expect("exact retained cut retries after both pre/post-commit failures");
                let final_session = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("read settled history")
                    .expect("session");
                assert_eq!(
                    committed.messages(),
                    final_session.messages(),
                    "replay cannot duplicate canonical text"
                );
                continue;
            }
            if matches!(exit, ExitKind::PrefixCut) {
                assert!(
                    member_host
                        .truncate_live_output(
                            &channel_id,
                            &activation_receipt,
                            &output.output_id,
                            100,
                            Some("not the observed prefix".to_string()),
                        )
                        .await
                        .is_err(),
                    "mismatched prefix must refuse before consuming caller permission"
                );
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    member_host.truncate_live_output(
                        &channel_id,
                        &activation_receipt,
                        &output.output_id,
                        100,
                        Some("First spoken".to_string()),
                    ),
                )
                .await
                .expect("prefix report must not require a provider final")
                .expect("prefix cut");
                let committed = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("read prefix")
                    .expect("session");
                let text = format!("{:?}", committed.messages());
                assert!(text.contains("First spoken"));
                assert!(!text.contains("checkpoint."));
                member_host
                    .close_live_channel(Some(authority.as_ref()), &channel_id)
                    .await
                    .expect("sequential prefix then close");
                continue;
            }
            if matches!(exit, ExitKind::UnmeasuredCut) {
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    member_host.truncate_live_output(
                        &channel_id,
                        &activation_receipt,
                        &output.output_id,
                        100,
                        None,
                    ),
                )
                .await
                .expect("unmeasured report settles without final")
                .expect("unmeasured disposition");
                let committed = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("read unmeasured")
                    .expect("session");
                assert!(
                    !format!("{:?}", committed.messages()).contains("First spoken checkpoint.")
                );
                member_host
                    .close_live_channel(Some(authority.as_ref()), &channel_id)
                    .await
                    .expect("close unmeasured output");
                continue;
            }
            if matches!(exit, ExitKind::SnapshotCut) {
                use futures::{FutureExt as _, StreamExt as _};
                let mut events = service
                    .subscribe_session_events(&session_id)
                    .await
                    .expect("event observer");
                let interaction = runtime
                    .live_assistant_output_handle(&output.output_id)
                    .expect("first output handle")
                    .interaction_id();
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    member_host.complete_live_playback(
                        &channel_id,
                        &activation_receipt,
                        &output.output_id,
                    ),
                )
                .await
                .expect("playback cut must not await a next turn or close")
                .expect("first snapshot cut commits");
                assert!(
                    runtime
                        .live_session_for_active_channel(&channel_id)
                        .await
                        .is_some()
                );
                let first = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("load first checkpoint")
                    .expect("session retained");
                assert!(format!("{:?}", first.messages()).contains("First spoken checkpoint."));
                tokio::time::sleep(std::time::Duration::from_millis(1750)).await;
                sideband.push(LiveSidebandObservation::new(
                    binding.clone(),
                    LiveSidebandObservationKind::TurnSnapshotDelta {
                        turn: assistant_turn.clone(),
                        delta: " Continued without a new turn.".to_string(),
                    },
                ));
                let continued =
                    tokio::time::timeout(std::time::Duration::from_secs(2), output_rx.recv())
                        .await
                        .expect("continuation publishes a fresh segment")
                        .expect("output stream");
                assert_ne!(continued.output_id, output.output_id);
                assert_eq!(
                    runtime
                        .live_assistant_output_handle(&continued.output_id)
                        .expect("continuation handle")
                        .interaction_id(),
                    interaction
                );
                assert!(
                    member_host
                        .complete_live_playback(&channel_id, &activation_receipt, &output.output_id)
                        .await
                        .is_err(),
                    "old receipt cannot consume continuation"
                );
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    member_host.complete_live_playback(
                        &channel_id,
                        &activation_receipt,
                        &continued.output_id,
                    ),
                )
                .await
                .expect("second cut is independent of provider final")
                .expect("second snapshot cut commits");
                while let Some(Some(event)) = events.next().now_or_never() {
                    assert!(
                        !matches!(
                            event.payload,
                            meerkat_core::AgentEvent::TurnCompleted { .. }
                        ),
                        "a local playback cut must not publish a provider turn terminal"
                    );
                }
                let before_close = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("load checkpoints")
                    .expect("session retained");
                let spoken: String = before_close
                    .messages()
                    .iter()
                    .filter_map(|message| match message {
                        meerkat_core::types::Message::BlockAssistant(assistant) => Some(assistant),
                        _ => None,
                    })
                    .flat_map(|assistant| &assistant.blocks)
                    .filter_map(|block| match block {
                        meerkat_core::types::AssistantBlock::Transcript {
                            text,
                            source: meerkat_core::types::TranscriptSource::Spoken,
                            ..
                        } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    spoken,
                    "First spoken checkpoint. Continued without a new turn."
                );
                assert!(
                    before_close.messages().iter().all(|message| match message {
                        meerkat_core::types::Message::ToolResults { .. } => false,
                        meerkat_core::types::Message::BlockAssistant(assistant) =>
                            !assistant.has_tool_calls(),
                        _ => true,
                    }),
                    "playback cuts cannot invent tool requests or result rows"
                );
                assert_eq!(before_close.messages().len(), first.messages().len() + 1);
                sideband.push(LiveSidebandObservation::new(
                    binding.clone(),
                    LiveSidebandObservationKind::TurnFinished {
                        turn: assistant_turn,
                        role: LiveSidebandTurnRole::Assistant,
                        transcript: "First spoken checkpoint. Continued without a new turn."
                            .to_string(),
                    },
                ));
                member_host
                    .close_live_channel(Some(authority.as_ref()), &channel_id)
                    .await
                    .expect("sequential playback then close settles without a concurrent caller");
                let reopened = member_host
                    .open_with_execution_identity(
                        authority.as_ref(),
                        &session_id,
                        &execution_identity,
                        None,
                        None,
                        Some(LiveOpenTransport::Webrtc),
                    )
                    .await
                    .expect("reopen committed snapshot history");
                let after_reopen = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("load reopened snapshot history")
                    .expect("session retained");
                assert_eq!(
                    after_reopen.messages(),
                    before_close.messages(),
                    "late group final and reopen must not replay a committed prefix"
                );
                member_host
                    .close_live_channel(Some(authority.as_ref()), reopened.channel_id())
                    .await
                    .expect("close unused replacement");
                continue;
            }
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
            if matches!(exit, ExitKind::Graceful) {
                for id in ["append:closing-context", "append:closing-result"] {
                    sideband.push(LiveSidebandObservation::new(
                        binding.clone(),
                        LiveSidebandObservationKind::AppendRejected {
                            attempt: LiveSidebandAppendAttempt::__from_generated_append_id(
                                id.to_string(),
                            )
                            .expect("exact rejected append"),
                        },
                    ));
                }
                sideband.push(LiveSidebandObservation::new(
                    binding.clone(),
                    LiveSidebandObservationKind::TurnFinished {
                        turn: assistant_turn,
                        role: LiveSidebandTurnRole::Assistant,
                        transcript: "The final spoken reply survives close and reopen.".to_string(),
                    },
                ));
                member_host
                    .close_live_channel(Some(authority.as_ref()), &channel_id)
                    .await
                    .expect("graceful close drains queued final and exact playback evidence");
                assert_eq!(
                    rejected_appends.load(AtomicOrdering::SeqCst),
                    2,
                    "both negative delivery receipts must drain before channel close"
                );
                tokio::time::timeout(std::time::Duration::from_secs(2), completion)
                    .await
                    .expect("completion joins before close")
                    .expect("completion task")
                    .expect("actual playback completion is canonical");
                let durable = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("load closed transcript")
                    .expect("durable session retained");
                assert!(
                    format!("{:?}", durable.messages())
                        .contains("The final spoken reply survives close and reopen.")
                );
                assert!(
                    durable
                        .live_assistant_playback_target_for_channel(&channel_id)
                        .is_none()
                );
                let reopened = member_host
                    .open_with_execution_identity(
                        authority.as_ref(),
                        &session_id,
                        &execution_identity,
                        None,
                        None,
                        Some(LiveOpenTransport::Webrtc),
                    )
                    .await
                    .expect("reopen the same durable session");
                assert_ne!(reopened.channel_id(), &channel_id);
                let after_reopen = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("load reopened transcript")
                    .expect("durable session retained");
                assert_eq!(durable.messages(), after_reopen.messages());
                assert!(
                    runtime
                        .reserve_live_assistant_output_handle(
                            &session_id,
                            &channel_id,
                            &output.output_id,
                        )
                        .await
                        .is_err(),
                    "late completion cannot revive a closed channel"
                );
                member_host
                    .close_live_channel(Some(authority.as_ref()), reopened.channel_id())
                    .await
                    .expect("close pending replacement");
                continue;
            }
            if matches!(exit, ExitKind::BrokerErrorCloseRejected) {
                sideband.fail_close.store(true, Ordering::Release);
            }
            if matches!(exit, ExitKind::BrokerErrorDuringPendingClose) {
                sideband.block_close.store(true, Ordering::Release);
            }
            if matches!(
                exit,
                ExitKind::UnconfirmedEof | ExitKind::UnconfirmedEofDuringClose
            ) {
                sideband.confirmed_eof.store(false, Ordering::Release);
            }
            if matches!(
                exit,
                ExitKind::BrokerErrorReportRetry
                    | ExitKind::BrokerErrorReportReceiptLost
                    | ExitKind::BrokerErrorCloseObserverCancelled
            ) {
                adapter
                    .drain
                    .lock()
                    .expect("drain")
                    .as_ref()
                    .expect("bound")
                    .requested
                    .store(true, Ordering::Release);
            }
            if matches!(
                exit,
                ExitKind::BrokerErrorReportRetry | ExitKind::BrokerErrorReportReceiptLost
            ) {
                live_adapter_host
                    .__fail_next_projection_for_test(
                        channel_id.clone(),
                        matches!(exit, ExitKind::BrokerErrorReportReceiptLost),
                    )
                    .await;
            }
            let close_barrier = if matches!(exit, ExitKind::BrokerErrorCloseObserverCancelled) {
                Some(
                    live_adapter_host
                        .__block_next_close_commit_for_test(channel_id.clone())
                        .await,
                )
            } else {
                None
            };
            let provider_loss_started = std::time::Instant::now();
            match exit {
                ExitKind::ContextReceiptWhileControlBusy | ExitKind::ContextAppendPendingClose => {
                    unreachable!()
                }
                ExitKind::ProviderManaged
                | ExitKind::PrefixCut
                | ExitKind::UnmeasuredCut
                | ExitKind::UnmeasuredRetry
                | ExitKind::LegacyCompleteRetry
                | ExitKind::LegacyPrefixRetry
                | ExitKind::ProjectionRetry
                | ExitKind::PublicationDrain => {
                    unreachable!()
                }
                ExitKind::PublicationRetry => unreachable!(),
                ExitKind::SnapshotCut => unreachable!(),
                ExitKind::Graceful => unreachable!(),
                ExitKind::Eof => sideband.close().await.expect("inject EOF"),
                ExitKind::UnconfirmedEof => sideband
                    .close()
                    .await
                    .expect("inject unconfirmed stream EOF"),
                ExitKind::UnconfirmedEofDuringClose => {}
                ExitKind::BrokerErrorDuringPendingClose => {}
                ExitKind::BrokerError
                | ExitKind::BrokerErrorCloseRejected
                | ExitKind::BrokerErrorReportRetry
                | ExitKind::BrokerErrorReportReceiptLost
                | ExitKind::BrokerErrorCloseObserverCancelled => {
                    sideband.fail(ProviderWebrtcBrokerError::Unavailable);
                }
                ExitKind::PublicationRejected => reject_release
                    .as_ref()
                    .expect("publication rejection release")
                    .notify_one(),
                ExitKind::ReceiptFailed => {}
            }
            if matches!(exit, ExitKind::BrokerErrorDuringPendingClose) {
                let close_host = Arc::clone(&member_host);
                let close_authority = Arc::clone(&authority);
                let close_channel = channel_id.clone();
                let receipt = opened.pending_receipt().to_string();
                let close = tokio::spawn(async move {
                    close_host
                        .close_experimental_live_pending_channel(
                            close_authority.as_ref(),
                            &close_channel,
                            &receipt,
                        )
                        .await
                });
                tokio::time::timeout(std::time::Duration::from_secs(1), async {
                    while !sideband.close_started.load(Ordering::Acquire) {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("provider close request starts and remains pending");
                sideband.fail(ProviderWebrtcBrokerError::Unavailable);
                tokio::time::timeout(std::time::Duration::from_secs(3), close)
                    .await
                    .expect("known failed stream cannot wait for hung remote close")
                    .expect("close task")
                    .expect("exact local close");
            }
            if matches!(exit, ExitKind::UnconfirmedEofDuringClose) {
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    member_host.close_experimental_live_pending_channel(
                        authority.as_ref(),
                        &channel_id,
                        opened.pending_receipt(),
                    ),
                )
                .await
                .expect("unconfirmed EOF during close is bounded")
                .expect("unconfirmed EOF retires locally through shared close");
            }
            if let Some((entered, release)) = close_barrier {
                let close_host = Arc::clone(&member_host);
                let close_authority = Arc::clone(&authority);
                let close_channel = channel_id.clone();
                let receipt = opened.pending_receipt().to_string();
                let mut observer = tokio::spawn(async move {
                    close_host
                        .close_experimental_live_pending_channel(
                            close_authority.as_ref(),
                            &close_channel,
                            &receipt,
                        )
                        .await
                });
                tokio::select! {
                    outcome = &mut observer => panic!("close returned before blocked host commit: {outcome:?}"),
                    entered = entered => entered.expect("generated close reaches blocked host commit"),
                    () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                        let physical_retired = adapter.drain.lock().expect("drain").as_ref().expect("bound")
                            .physically_retired.load(Ordering::Acquire);
                        let active = runtime.live_session_for_active_channel(&channel_id).await.is_some();
                        panic!("close did not reach host commit: physical_retired={physical_retired} active={active}");
                    }
                }
                assert!(
                    runtime
                        .live_session_for_active_channel(&channel_id)
                        .await
                        .is_none(),
                    "cancel at the exact generated-close/host-commit gap"
                );
                observer.abort();
                assert!(
                    observer
                        .await
                        .expect_err("close observer cancelled")
                        .is_cancelled()
                );
                release
                    .send(())
                    .expect("owned commit continues after observer cancellation");
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        if matches!(live_adapter_host.channel_status_observation(&channel_id).await,
                            Ok(observation) if observation.status() == &LiveAdapterStatus::Closed)
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("owned close handoff commits despite observer cancellation");
            }
            if matches!(
                exit,
                ExitKind::BrokerErrorReportRetry | ExitKind::BrokerErrorReportReceiptLost
            ) {
                let error = member_host
                    .close_experimental_live_pending_channel(
                        authority.as_ref(),
                        &channel_id,
                        opened.pending_receipt(),
                    )
                    .await
                    .expect_err("fault reporting failure must remain visible");
                assert!(matches!(
                    error,
                    crate::surface::ExperimentalLiveChannelCloseError::TerminalProjection(_)
                ));
                assert!(
                    runtime
                        .live_session_for_active_channel(&channel_id)
                        .await
                        .is_none()
                );
                assert!(
                    authority
                        .transport
                        .registered_by_channel
                        .lock()
                        .await
                        .contains_key(&channel_id),
                    "generated close cannot discard an unreported terminal fault"
                );
                assert!(
                    authority
                        .transport
                        .active_by_session
                        .lock()
                        .await
                        .is_empty(),
                    "failed fault publication must not keep a dead transport selectable"
                );
                live_adapter_host
                    .__expire_closed_projection_for_test(&channel_id)
                    .await
                    .expect("expire closed projection");
                assert_eq!(
                    live_adapter_host
                        .channel_status_observation(&channel_id)
                        .await
                        .expect("pending report pins projection past TTL")
                        .status(),
                    &LiveAdapterStatus::Closed
                );
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
            if matches!(
                exit,
                ExitKind::BrokerError
                    | ExitKind::UnconfirmedEof
                    | ExitKind::UnconfirmedEofDuringClose
                    | ExitKind::BrokerErrorCloseRejected
                    | ExitKind::BrokerErrorDuringPendingClose
                    | ExitKind::BrokerErrorReportRetry
                    | ExitKind::BrokerErrorReportReceiptLost
                    | ExitKind::BrokerErrorCloseObserverCancelled
            ) {
                use futures::{FutureExt as _, StreamExt as _};
                assert_eq!(
                    member_host
                        .close_experimental_live_pending_channel(
                            authority.as_ref(),
                            &channel_id,
                            opened.pending_receipt(),
                        )
                        .await
                        .expect("terminal provider failure must allow exact local cleanup"),
                    meerkat_contracts::LiveCloseStatus::Closed,
                );
                assert!(
                    provider_loss_started.elapsed() < std::time::Duration::from_secs(5),
                    "terminal provider cleanup must confirm within the browser's five-second close bound"
                );
                let custody = member_host
                    .validate_experimental_live_channel_custody(
                        &channel_id,
                        opened.pending_receipt(),
                    )
                    .await
                    .expect("terminal custody survives physical retirement");
                assert!(matches!(
                    custody.phase(),
                    crate::surface::ExperimentalLiveChannelPhaseStatus::Closed
                ));
                assert_eq!(
                    member_host
                        .close_experimental_live_pending_channel(
                            authority.as_ref(),
                            &channel_id,
                            opened.pending_receipt(),
                        )
                        .await
                        .expect("exact close receipt replays after provider loss"),
                    meerkat_contracts::LiveCloseStatus::Closed,
                );
                assert!(
                    member_host
                        .close_experimental_live_pending_channel(
                            authority.as_ref(),
                            &channel_id,
                            "wrong-receipt",
                        )
                        .await
                        .is_err()
                );
                let mut faults = 0;
                while let Some(Some(envelope)) = terminal_events.next().now_or_never() {
                    match envelope.payload {
                        meerkat_core::AgentEvent::RunFailed { .. } => faults += 1,
                        meerkat_core::AgentEvent::TurnCompleted { .. }
                        | meerkat_core::AgentEvent::TextComplete { .. } => {
                            panic!("provider failure cannot fabricate turn completion")
                        }
                        _ => {}
                    }
                }
                assert_eq!(
                    faults, 1,
                    "retained terminal fault must publish exactly once"
                );
                let before = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("session read")
                    .expect("session");
                let reopened = member_host
                    .open_with_execution_identity(
                        authority.as_ref(),
                        &session_id,
                        &execution_identity,
                        None,
                        None,
                        Some(LiveOpenTransport::Webrtc),
                    )
                    .await
                    .expect("failed provider no longer poisons source reopen");
                assert_ne!(reopened.channel_id(), &channel_id);
                drop(adapter);
                live_adapter_host
                    .__expire_closed_projection_for_test(&channel_id)
                    .await
                    .expect("expire completed projection");
                assert!(
                    live_adapter_host
                        .channel_status_observation(&channel_id)
                        .await
                        .is_err(),
                    "completed close replay must not rely on retained host cache"
                );
                member_host
                    .close_experimental_live_pending_channel(
                        authority.as_ref(),
                        &channel_id,
                        opened.pending_receipt(),
                    )
                    .await
                    .expect("old exact close replays after new open");
                assert_eq!(
                    runtime
                        .live_session_for_active_channel(reopened.channel_id())
                        .await,
                    Some(session_id.clone())
                );
                let after = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("session read")
                    .expect("session");
                assert_eq!(
                    before.messages(),
                    after.messages(),
                    "retirement/replay/reopen never rewrites background conversation"
                );
                member_host
                    .close_experimental_live_pending_channel(
                        authority.as_ref(),
                        reopened.channel_id(),
                        reopened.pending_receipt(),
                    )
                    .await
                    .expect("close newly opened channel");
            }
            if matches!(exit, ExitKind::PublicationRejected) {
                assert!(
                    member_host
                        .close_live_channel(Some(authority.as_ref()), &channel_id)
                        .await
                        .is_err(),
                    "failed projection cannot manufacture close success"
                );
                assert!(
                    runtime
                        .live_session_for_active_channel(&channel_id)
                        .await
                        .is_some(),
                    "failed close retains generated binding for diagnosis/retry"
                );
                assert!(
                    authority
                        .transport
                        .active_by_session
                        .lock()
                        .await
                        .contains_key(&session_id),
                    "failed close retains exact provider custody"
                );
                continue;
            }
            if matches!(exit, ExitKind::ReceiptFailed) {
                member_host
                    .close_live_channel(Some(authority.as_ref()), &channel_id)
                    .await
                    .expect("explicit close observes provider EOF before unmeasured settlement");
            }
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
        run_context_and_result_recovery_with_summary(None).await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    #[derive(Clone, Copy)]
    enum SummaryTestCase {
        Normal,
        Changed,
        Failed,
        BlockRecovery,
        BlockRegistration,
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    struct RecoverySummaryProducer {
        service: Arc<crate::PersistentSessionService<crate::FactoryAgentBuilder>>,
        case: SummaryTestCase,
        calls: AtomicUsize,
        require_unmeasured_history: bool,
        recovery_entered: tokio::sync::Notify,
        recovery_release: tokio::sync::Notify,
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    #[async_trait]
    impl crate::session_runtime::live_summary::LiveContextSummarizer for RecoverySummaryProducer {
        async fn summarize(
            &self,
            snapshot: crate::session_runtime::live_summary::LiveContextSummarySnapshot<'_>,
        ) -> Result<String, crate::session_runtime::live_summary::LiveContextSummaryError> {
            use crate::session_runtime::live_summary::LiveContextSummaryError;
            let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            assert_eq!(
                snapshot.llm_identity().model,
                "gpt-realtime-2",
                "summary sees background identity, never voice override"
            );
            match self.case {
                SummaryTestCase::Changed => {
                    self.service
                        .append_external_user_content(
                            snapshot.session_id(),
                            meerkat_core::ContentInput::Text("text arrived during summary".into()),
                        )
                        .await?;
                }
                SummaryTestCase::Failed => {
                    return Err(LiveContextSummaryError::Producer(
                        "summary service failed".into(),
                    ));
                }
                SummaryTestCase::BlockRecovery if call == 1 => {
                    self.recovery_entered.notify_one();
                    self.recovery_release.notified().await;
                }
                SummaryTestCase::Normal
                | SummaryTestCase::BlockRecovery
                | SummaryTestCase::BlockRegistration => {}
            }
            let mut summary = format!(
                "Factual context summary covering {} canonical rows.",
                snapshot.canonical_message_cursor()
            );
            if self.require_unmeasured_history && call > 0 {
                let observation = snapshot
                    .messages()
                    .iter()
                    .find_map(|message| {
                        if let meerkat_core::Message::BlockAssistant(assistant) = message {
                            assistant.blocks.iter().find_map(|block| match block {
                                meerkat_core::AssistantBlock::Transcript {
                                    text,
                                    source: meerkat_core::types::TranscriptSource::SpokenUnmeasured,
                                    ..
                                } => Some(text.as_str()),
                                _ => None,
                            })
                        } else {
                            None
                        }
                    })
                    .expect("replacement summarizer sees typed observation context");
                assert!(observation.contains("prior unmeasured voice dialogue"));
                summary.push_str("\nObserved assistant speech (playback UNMEASURED): ");
                summary.push_str(observation);
            }
            Ok(summary)
        }
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    #[tokio::test]
    async fn summary_policy_reapplies_on_context_and_result_replacement_without_mutating_source() {
        run_context_and_result_recovery_with_summary(Some(SummaryTestCase::Normal)).await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    #[tokio::test]
    async fn summary_open_rejects_stale_snapshot_before_channel_or_provider_effects() {
        run_context_and_result_recovery_with_summary(Some(SummaryTestCase::Changed)).await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    #[tokio::test]
    async fn summary_open_producer_failure_has_no_raw_history_fallback_or_channel_leak() {
        run_context_and_result_recovery_with_summary(Some(SummaryTestCase::Failed)).await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    async fn run_context_and_result_recovery_with_summary(summary_case: Option<SummaryTestCase>) {
        run_context_and_result_recovery_with_history(summary_case, false).await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    #[tokio::test]
    async fn unmeasured_voice_dialogue_survives_canonical_text_and_result_replacements() {
        run_context_and_result_recovery_with_history(None, true).await;
        run_context_and_result_recovery_with_history(Some(SummaryTestCase::Normal), true).await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    async fn run_context_and_result_recovery_with_history(
        summary_case: Option<SummaryTestCase>,
        retain_voice: bool,
    ) {
        run_context_and_result_recovery_case(summary_case, retain_voice, false).await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    #[tokio::test]
    async fn closed_pending_replacement_does_not_poison_reopened_source_or_new_replacement() {
        run_context_and_result_recovery_case(None, false, true).await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    #[tokio::test]
    async fn explicit_close_during_recovery_summary_cannot_resurrect_old_lineage() {
        run_context_and_result_recovery_case(Some(SummaryTestCase::BlockRecovery), false, false)
            .await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    #[tokio::test]
    async fn explicit_close_before_recovery_registration_retires_late_provider_custody() {
        run_context_and_result_recovery_case(
            Some(SummaryTestCase::BlockRegistration),
            false,
            false,
        )
        .await;
    }

    #[cfg(all(
        feature = "session-store",
        feature = "memory-store",
        feature = "test-realtime-fixtures"
    ))]
    async fn run_context_and_result_recovery_case(
        summary_case: Option<SummaryTestCase>,
        retain_voice: bool,
        close_pending_replacement: bool,
    ) {
        use meerkat_contracts::{LiveOpenTransport, WireLiveTransportBootstrap};
        use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};

        // These cases share the production one-open memory reservation when
        // run in one cargo-test process. Nextest isolates them by process.
        static OPEN_RESERVATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _reservation = OPEN_RESERVATION.lock().await;

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
                meerkat_comms::CommsRuntime::inproc_only(&format!(
                    "gpt-live-recovery-{session_id}"
                ))
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
        let summary_producer = summary_case.map(|case| {
            Arc::new(RecoverySummaryProducer {
                service: Arc::clone(&service),
                case,
                calls: AtomicUsize::new(0),
                require_unmeasured_history: retain_voice,
                recovery_entered: tokio::sync::Notify::new(),
                recovery_release: tokio::sync::Notify::new(),
            })
        });
        let member_host = crate::surface::ServiceMemberLiveHost::new(
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
        .with_webrtc_cleanup_state(webrtc_state);
        let member_host = Arc::new(match &summary_producer {
            Some(producer) => member_host.with_context_summary_policy(
                crate::session_runtime::live_summary::LiveContextSummaryPolicy::new(
                    producer.clone(),
                    64 * 1024,
                    1024,
                    std::time::Duration::from_secs(5),
                )
                .expect("bounded host summary policy"),
            ),
            None => member_host,
        });
        let readiness_realm = meerkat_core::RealmId::parse("active-readiness").unwrap();
        let readiness_binding = public_live_binding(&readiness_realm);
        let identity = public_live_identity(readiness_binding.clone());
        let mut authority = ScriptedStrictOpenAuthority::new(identity).with_client_context();
        if retain_voice {
            authority.snapshot_cuts = true;
            authority.playback_policy = PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured;
        }
        let authority = Arc::new(authority);
        let registration_barrier = Arc::new(RecoveryRegistrationBarrier::default());
        if matches!(summary_case, Some(SummaryTestCase::BlockRegistration)) {
            *authority.recovery_registration_barrier.lock().await =
                Some(Arc::clone(&registration_barrier));
        }
        let authority_trait: Arc<dyn ExperimentalLiveOpenAuthorityProvider> = authority.clone();
        let downstream: Arc<dyn ExperimentalLiveBoundChannelActivator> =
            Arc::new(SerializedLifecycleTestActivator {
                runtime: Arc::clone(&runtime),
                rejected_appends: Arc::new(AtomicUsize::new(0)),
                control_release: None,
            });
        let mirror_host = crate::surface::ExperimentalGptLiveContextMirrorHost::new(
            Arc::clone(&runtime),
            Arc::clone(&member_host),
            Arc::clone(&authority_trait),
            downstream,
        );
        let execution_identity = meerkat_contracts::WireLiveExecutionIdentityOverrideV1 {
            version: meerkat_contracts::WireLiveExecutionIdentityVersion::V1,
            profile_id: GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID.to_string(),
        };

        let before = service
            .export_realtime_refresh_session_snapshot(&session_id)
            .await
            .unwrap();
        let opening = member_host
            .open_with_execution_identity(
                authority.as_ref(),
                &session_id,
                &execution_identity,
                None,
                None,
                Some(LiveOpenTransport::Webrtc),
            )
            .await;
        if let Some(case @ (SummaryTestCase::Changed | SummaryTestCase::Failed)) = summary_case {
            use crate::session_runtime::live_orchestration::{
                ExperimentalLiveChannelOpenError, RealtimeSessionOpenProjectionError,
            };
            use crate::session_runtime::live_summary::LiveContextSummaryError;
            let error = opening.expect_err("summary failure must refuse strict open");
            match case {
                SummaryTestCase::Changed => assert!(
                    matches!(
                        error,
                        ExperimentalLiveChannelOpenError::Projection(
                            RealtimeSessionOpenProjectionError::Summary(
                                LiveContextSummaryError::StaleSnapshot
                            )
                        )
                    ),
                    "{error:?}"
                ),
                SummaryTestCase::Failed => assert!(
                    matches!(
                        error,
                        ExperimentalLiveChannelOpenError::Projection(
                            RealtimeSessionOpenProjectionError::Summary(
                                LiveContextSummaryError::Producer(_)
                            )
                        )
                    ),
                    "{error:?}"
                ),
                SummaryTestCase::Normal
                | SummaryTestCase::BlockRecovery
                | SummaryTestCase::BlockRegistration => unreachable!(),
            }
            assert!(
                runtime
                    .live_active_channel_for_session(&session_id)
                    .await
                    .is_none()
            );
            assert!(
                authority
                    .transport
                    .registered_by_channel
                    .lock()
                    .await
                    .is_empty()
            );
            assert!(
                authority
                    .transport
                    .active_by_session
                    .lock()
                    .await
                    .is_empty()
            );
            assert_eq!(
                summary_producer
                    .as_ref()
                    .unwrap()
                    .calls
                    .load(AtomicOrdering::SeqCst),
                1
            );
            return;
        }
        let opened = opening.expect("strict initial experimental open");
        if summary_producer.is_some() {
            let after = service
                .export_realtime_refresh_session_snapshot(&session_id)
                .await
                .unwrap();
            assert_eq!(
                before.messages(),
                after.messages(),
                "voice summary does not mutate the transcript"
            );
            assert_eq!(
                opened.open().continuity,
                meerkat_contracts::WireLiveContinuityMode::Degraded
            );
            let provenance = authority
                .transport
                .bound_context_summary(opened.channel_id(), &session_id)
                .await
                .expect("summary provenance follows the exact channel");
            assert_eq!(
                provenance.source_revision(),
                &before.canonical_context_revision().unwrap()
            );
            assert_eq!(
                provenance.canonical_message_cursor(),
                before.messages().len() as u64
            );
            assert!(provenance.text().starts_with("Factual context summary"));
            assert!(
                authority
                    .transport
                    .bound_context_summary(opened.channel_id(), &meerkat_core::SessionId::new(),)
                    .await
                    .is_none()
            );
        }
        let mut old_channel = opened.channel_id().clone();
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
        let readiness = member_host
            .register_experimental_live_playback_owner(&old_channel, opened.pending_receipt())
            .await
            .expect("register initial playback readiness");
        let initial_answer = member_host
            .answer_experimental_live_webrtc_offer(
                Arc::clone(&authority.transport) as Arc<dyn LiveWebrtcAnswerTransport>,
                binder,
                old_channel.clone(),
                opened.pending_receipt(),
                readiness.readiness_receipt(),
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
        let mut stale_old_binding = authority
            .transport
            .active_binding(&session_id)
            .await
            .expect("old provider binding is physically active");
        let custody = member_host
            .validate_experimental_live_channel_custody(&old_channel, opened.pending_receipt())
            .await
            .expect("actual active source custody");
        assert!(matches!(
            custody.phase(),
            crate::surface::ExperimentalLiveChannelPhaseStatus::Active { .. }
        ));
        let summary_calls = summary_producer
            .as_ref()
            .map(|producer| producer.calls.load(AtomicOrdering::SeqCst));
        let readiness_auth = Arc::new(RevocableBindingAuthority {
            inner: ExactAllowBindingAuthority {
                session_id: session_id.clone(),
                expected: readiness_binding.clone(),
                calls: Arc::new(AtomicUsize::new(0)),
                auth_lease: runtime.generated_auth_lease_handle(),
                events: Arc::new(std::sync::Mutex::new(Vec::new())),
            },
            allowed: AtomicBool::new(true),
            attempts: AtomicUsize::new(0),
        });
        let config_reads = Arc::new(AtomicUsize::new(0));
        let readiness_authority =
            ExperimentalGptLiveOpenAuthority::new_public(public_live_authority_config(
                &readiness_realm,
                "marin",
                public_live_identity(readiness_binding),
                Arc::new(CountingConfigSource {
                    reads: Arc::clone(&config_reads),
                    config: public_live_realm_config(&readiness_realm),
                }),
                readiness_auth.clone(),
                Arc::clone(&authority.transport),
            ))
            .unwrap();
        for _ in 0..2 {
            readiness_authority
                .probe_execution_readiness(&session_id, &public_profile_override())
                .await
                .expect("active voice does not consume per-target readiness");
        }
        readiness_auth.allowed.store(false, Ordering::Release);
        assert!(matches!(
            readiness_authority
                .probe_execution_readiness(&session_id, &public_profile_override(),)
                .await,
            Err(ExperimentalLiveOpenAuthorityError::BindingUseDenied)
        ));
        assert_eq!(readiness_auth.attempts.load(AtomicOrdering::SeqCst), 3);
        assert_eq!(config_reads.load(AtomicOrdering::SeqCst), 3);
        assert_eq!(
            authority.transport.active_binding(&session_id).await,
            Some(stale_old_binding.clone())
        );
        assert_eq!(
            authority.transport.registered_by_channel.lock().await.len(),
            1
        );
        assert_eq!(
            summary_producer
                .as_ref()
                .map(|producer| producer.calls.load(AtomicOrdering::SeqCst)),
            summary_calls,
            "readiness never starts a summary or second channel"
        );
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

        if retain_voice {
            let sideband = authority
                .latest_sideband
                .lock()
                .await
                .clone()
                .expect("voice sideband");
            let assistant = LiveSidebandTurnRef::__from_provider_observation(
                &old_channel,
                "prior-voice-assistant".to_string(),
                "private-voice-assistant".to_string(),
            )
            .expect("assistant grouping");
            let assistant_key = assistant.adapter_key().to_string();
            for kind in [
                LiveSidebandObservationKind::TurnStarted {
                    turn: assistant.clone(),
                    role: LiveSidebandTurnRole::Assistant,
                },
                LiveSidebandObservationKind::TurnSnapshotDelta {
                    turn: assistant,
                    delta: "prior unmeasured voice dialogue".to_string(),
                },
            ] {
                sideband.push(LiveSidebandObservation::new(
                    stale_old_binding.clone(),
                    kind,
                ));
            }
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    let snapshot = service
                        .export_realtime_refresh_session_snapshot(&session_id)
                        .await
                        .expect("authoritative voice observation");
                    if unmeasured_fragments(&snapshot).count() == 1
                        && runtime
                            .live_assistant_output_handle_for_turn(
                                &session_id,
                                &old_channel,
                                &assistant_key,
                            )
                            .is_some_and(|handle| handle.__playback_segment() == 1)
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("unmeasured voice observation is retained without playback completion");
            assert!(
                mirror_host
                    .pending_replacement_required(&session_id)
                    .await
                    .is_none(),
                "canonical observed voice must advance live coverage without echo or replacement"
            );
            assert_eq!(
                authority.transport.active_binding(&session_id).await,
                Some(stale_old_binding.clone())
            );
        }

        let preparation_closed_origin = if matches!(
            summary_case,
            Some(SummaryTestCase::BlockRecovery | SummaryTestCase::BlockRegistration)
        ) {
            let producer = summary_producer.as_ref().unwrap();
            service
                .append_external_user_content(
                    &session_id,
                    meerkat_core::ContentInput::Text("text preceding cancelled recovery".into()),
                )
                .await
                .expect("commit text triggering blocked recovery");
            let (committed, store_authority) = service
                .export_live_context_committed_boundary(&session_id)
                .await
                .expect("exact boundary for blocked recovery");
            let recovery_runtime = Arc::clone(&runtime);
            let recovery_session = session_id.clone();
            let recovery_task = tokio::spawn(async move {
                recovery_runtime
                    .enqueue_committed_parent_session_boundary(
                        &recovery_session,
                        &committed,
                        &store_authority,
                    )
                    .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                if matches!(summary_case, Some(SummaryTestCase::BlockRegistration)) {
                    registration_barrier.entered.notified().await;
                } else {
                    producer.recovery_entered.notified().await;
                }
            })
            .await
            .expect("recovery blocks after old transport close");
            assert!(
                authority
                    .transport
                    .active_binding(&session_id)
                    .await
                    .is_none()
            );
            assert!(
                mirror_host
                    .pending_replacement_required(&session_id)
                    .await
                    .is_none()
            );
            let origin = (old_channel.clone(), opened.pending_receipt().to_string());
            member_host
                .close_experimental_live_pending_channel(authority.as_ref(), &origin.0, &origin.1)
                .await
                .expect("explicit exact close cancels recovery despite already-closed origin");
            if matches!(summary_case, Some(SummaryTestCase::BlockRegistration)) {
                registration_barrier.release.notify_one();
            } else {
                producer.recovery_release.notify_one();
            }
            assert!(
                recovery_task.await.expect("join blocked recovery").is_err(),
                "late preparation is refused even while no fresh channel occupies the source"
            );
            assert!(
                mirror_host
                    .pending_replacement_required(&session_id)
                    .await
                    .is_none()
            );
            assert!(
                runtime
                    .live_active_channel_for_session(&session_id)
                    .await
                    .is_none()
            );
            assert!(
                authority
                    .transport
                    .registered_by_channel
                    .lock()
                    .await
                    .is_empty()
            );
            assert!(authority.pending_context_recovery.lock().await.is_empty());
            let fresh = member_host
                .open_with_execution_identity(
                    authority.as_ref(),
                    &session_id,
                    &execution_identity,
                    None,
                    None,
                    Some(LiveOpenTransport::Webrtc),
                )
                .await
                .expect("independent same-source open remains healthy after cancelled summary");
            old_channel = fresh.channel_id().clone();
            let token = match &fresh.open().transport {
                WireLiveTransportBootstrap::Webrtc { token, .. } => token.clone(),
                other => panic!("expected fresh WebRTC bootstrap, got {other:?}"),
            };
            let binder = authority
                .bound_ready_binder_for(
                    Arc::clone(&mirror_host) as Arc<dyn ExperimentalLiveBoundChannelActivator>,
                    Arc::clone(&live_adapter_host),
                    Arc::new(NoopPublicObservationPublisher),
                )
                .expect("fresh binder");
            let readiness = member_host
                .register_experimental_live_playback_owner(&old_channel, fresh.pending_receipt())
                .await
                .expect("fresh playback owner");
            member_host
                .answer_experimental_live_webrtc_offer(
                    Arc::clone(&authority.transport) as Arc<dyn LiveWebrtcAnswerTransport>,
                    binder,
                    old_channel.clone(),
                    fresh.pending_receipt(),
                    readiness.readiness_receipt(),
                    token,
                    "fresh-after-cancel-offer".into(),
                )
                .await
                .expect("fresh answer")
                .delivery_custody
                .delivered()
                .await
                .expect("fresh activation");
            stale_old_binding = authority
                .transport
                .active_binding(&session_id)
                .await
                .unwrap();
            assert!(
                mirror_host
                    .pending_replacement_required(&session_id)
                    .await
                    .is_none()
            );
            assert_eq!(
                authority.transport.active_binding(&session_id).await,
                Some(stale_old_binding.clone()),
                "late old recovery cannot close the independently opened healthy channel"
            );
            assert_eq!(
                authority.transport.registered_by_channel.lock().await.len(),
                1
            );
            Some(origin)
        } else {
            None
        };

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
            open: mut replacement_open,
            canonical_seed_cursor: replacement_seed_cursor,
            pending_receipt: mut replacement_pending_receipt,
        } = replacement
        else {
            panic!("context ambiguity must publish the canonical-context reason")
        };
        let mut replacement_channel =
            meerkat_live::LiveChannelId::new(&replacement_open.channel_id);
        let closed_replacement = if close_pending_replacement {
            let stale = (
                replacement_channel.clone(),
                replacement_pending_receipt.clone(),
            );
            assert!(
                member_host
                    .close_experimental_live_pending_channel(
                        authority.as_ref(),
                        &replacement_channel,
                        opened.pending_receipt(),
                    )
                    .await
                    .is_err(),
                "another channel's receipt cannot retire the pending replacement"
            );
            assert!(
                mirror_host
                    .pending_replacement_required(&session_id)
                    .await
                    .is_some()
            );
            member_host
                .close_experimental_live_pending_channel(
                    authority.as_ref(),
                    &replacement_channel,
                    &replacement_pending_receipt,
                )
                .await
                .expect("close replacement before any activation");
            assert_eq!(
                mirror_host.pending_replacement_required(&session_id).await,
                None,
                "generated pending close retires its retained bootstrap without activation"
            );
            let reopened = member_host
                .open_with_execution_identity(
                    authority.as_ref(),
                    &session_id,
                    &execution_identity,
                    None,
                    None,
                    Some(LiveOpenTransport::Webrtc),
                )
                .await
                .expect("same existing source reopens normally after pending replacement close");
            assert_ne!(reopened.channel_id(), &stale.0);
            assert_eq!(
                mirror_host.pending_replacement_required(&session_id).await,
                None,
                "a healthy new open cannot expose the old closed replacement"
            );
            replacement_channel = reopened.channel_id().clone();
            replacement_open = reopened.open().clone();
            replacement_pending_receipt = reopened.pending_receipt().to_string();
            Some(stale)
        } else {
            None
        };
        if retain_voice {
            let retained = authority
                .latest_initial_seed
                .lock()
                .await
                .clone()
                .and_then(|seed| seed.upgrade())
                .expect("replacement seed");
            let retained = retained.lock().await;
            let retained = retained
                .as_ref()
                .expect("replacement owns the exact retained context");
            let rendered = match &retained.context {
                GptLiveSeedContext::Canonical(messages) => {
                    serde_json::to_string(messages).expect("seed")
                }
                GptLiveSeedContext::Summary(summary) => summary.text().to_string(),
                _ => panic!("replacement has not been sent yet"),
            };
            assert!(rendered.contains("prior unmeasured voice dialogue"));
            assert!(rendered.contains("spoken_unmeasured") || rendered.contains("UNMEASURED"));
            assert_eq!(
                unmeasured_fragments(
                    &service
                        .export_realtime_refresh_session_snapshot(&session_id)
                        .await
                        .expect("retained observation")
                )
                .count(),
                1
            );
        }
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
        let readiness = member_host
            .register_experimental_live_playback_owner(
                &replacement_channel,
                &replacement_pending_receipt,
            )
            .await
            .expect("register replacement playback readiness");
        let recovery_answer = member_host
            .answer_experimental_live_webrtc_offer(
                Arc::clone(&authority.transport) as Arc<dyn LiveWebrtcAnswerTransport>,
                recovery_binder,
                replacement_channel.clone(),
                &replacement_pending_receipt,
                readiness.readiness_receipt(),
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
        let extra_opens = usize::from(close_pending_replacement)
            + 2 * usize::from(preparation_closed_origin.is_some());
        assert_eq!(
            authority.prepare_sequence.load(AtomicOrdering::SeqCst),
            2 + extra_opens
        );
        if let Some(producer) = &summary_producer {
            assert_eq!(
                producer.calls.load(AtomicOrdering::SeqCst),
                2 + extra_opens,
                "context replacement regenerates the summary"
            );
        }
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
            .expect("authorize worker only after canonical final reconciliation");
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
                    transcript: "confirmed result recovery input".to_string(),
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
        for (closed_channel, closed_receipt) in closed_replacement
            .iter()
            .chain(preparation_closed_origin.iter())
        {
            member_host
                .close_experimental_live_pending_channel(
                    authority.as_ref(),
                    closed_channel,
                    closed_receipt,
                )
                .await
                .expect("delayed old close remains idempotent");
            assert_eq!(
                mirror_host.pending_replacement_required(&session_id).await,
                Some(result_replacement.clone()),
                "delayed close cannot clear a newer replacement for the same source"
            );
        }
        if let Some(producer) = &summary_producer {
            assert_eq!(
                producer.calls.load(AtomicOrdering::SeqCst),
                3 + extra_opens,
                "result replacement regenerates the summary"
            );
        }
        assert_eq!(
            mirror_host.pending_replacement_required(&session_id).await,
            Some(result_replacement.clone()),
            "lost result-replacement pulls must remain idempotent"
        );
        let crate::surface::ExperimentalLiveReplacementRequired::DelegationResult {
            open: result_replacement_open,
            canonical_seed_cursor: result_replacement_seed_cursor,
            pending_receipt: result_pending_receipt,
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
        let readiness = member_host
            .register_experimental_live_playback_owner(
                &result_replacement_channel,
                &result_pending_receipt,
            )
            .await
            .expect("register result-replacement playback readiness");
        let result_recovery_answer = member_host
            .answer_experimental_live_webrtc_offer(
                Arc::clone(&authority.transport) as Arc<dyn LiveWebrtcAnswerTransport>,
                result_recovery_binder,
                result_replacement_channel.clone(),
                &result_pending_receipt,
                readiness.readiness_receipt(),
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
        assert_eq!(
            authority.prepare_sequence.load(AtomicOrdering::SeqCst),
            3 + extra_opens
        );
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
