//! Exact public continuous-Live construction targets.
//!
//! Resolution binds registry identity and credential ownership. It grants no
//! executor permission; trusted activation and generated request authority are
//! separate from provider construction.

pub mod open;

use meerkat_core::live_execution::profile::{
    LiveClientRequestPolicy, LiveManagedBackendModel, LiveProfileId,
};
use meerkat_core::model_profile::ModelInteractionKind;
use meerkat_core::{ModelProfileWitness, SessionLlmIdentity};

use super::{ResolvedConnection, ResolvedLiveConnection, ResolvedTextTarget};

/// A managed backend uses the voice connection, never an independently
/// supplied credential or the durable executor's model identity.
#[derive(Debug, Clone)]
pub enum ResolvedLiveExecution {
    ClientContext {
        request_policy: LiveClientRequestPolicy,
    },
    FunctionBridge {
        backend: ModelProfileWitness,
    },
}

#[derive(Clone)]
pub struct ResolvedLiveTarget {
    profile_id: LiveProfileId,
    voice_identity: SessionLlmIdentity,
    voice_profile: ModelProfileWitness,
    connection: ResolvedLiveConnection,
    execution: ResolvedLiveExecution,
}

impl ResolvedLiveTarget {
    pub fn new(
        profile_id: LiveProfileId,
        voice_identity: SessionLlmIdentity,
        voice_profile: ModelProfileWitness,
        connection: ResolvedLiveConnection,
        execution: ResolvedLiveExecution,
    ) -> Result<Self, LiveTargetError> {
        if !voice_profile.matches_identity(&voice_identity) {
            return Err(LiveTargetError::VoiceIdentityMismatch);
        }
        if voice_profile.profile().interaction_kind != ModelInteractionKind::ContinuousLive {
            return Err(LiveTargetError::VoiceNotContinuous);
        }
        if connection.connection().provider != voice_identity.provider
            || connection.connection().backend_profile.provider != voice_identity.provider
        {
            return Err(LiveTargetError::ConnectionProviderMismatch);
        }
        let binding = voice_identity
            .auth_binding
            .as_ref()
            .ok_or(LiveTargetError::MissingResolvedBinding)?;
        if connection.auth_binding() != binding {
            return Err(LiveTargetError::CredentialOwnerMismatch);
        }
        if let ResolvedLiveExecution::FunctionBridge { backend } = &execution {
            if backend.provider() != voice_identity.provider {
                return Err(LiveTargetError::BackendProviderMismatch);
            }
            if backend.profile().interaction_kind != ModelInteractionKind::Text {
                return Err(LiveTargetError::BackendNotText);
            }
        }
        Ok(Self {
            profile_id,
            voice_identity,
            voice_profile,
            connection,
            execution,
        })
    }

    pub fn profile_id(&self) -> &LiveProfileId {
        &self.profile_id
    }

    pub fn voice_identity(&self) -> &SessionLlmIdentity {
        &self.voice_identity
    }

    pub fn voice_profile(&self) -> &ModelProfileWitness {
        &self.voice_profile
    }

    pub fn connection(&self) -> &ResolvedConnection {
        self.connection.connection()
    }

    pub fn execution(&self) -> &ResolvedLiveExecution {
        &self.execution
    }

    /// Materialize the backend identity from its registry witness and the ONE
    /// resolved voice connection. Voice-specific params never become text
    /// params. The durable executor remains outside this carrier.
    pub fn managed_backend_target(&self) -> Result<Option<ResolvedTextTarget>, LiveTargetError> {
        let ResolvedLiveExecution::FunctionBridge { backend } = &self.execution else {
            return Ok(None);
        };
        let identity = SessionLlmIdentity {
            provider: backend.provider(),
            model: backend.model().to_owned(),
            self_hosted_server_id: self.voice_identity.self_hosted_server_id.clone(),
            provider_params: None,
            auth_binding: self.voice_identity.auth_binding.clone(),
        };
        ResolvedTextTarget::new(
            identity,
            backend.clone(),
            self.connection.connection().clone(),
        )
        .map(Some)
        .ok_or(LiveTargetError::BackendProviderMismatch)
    }
}

impl std::fmt::Debug for ResolvedLiveTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedLiveTarget")
            .field("profile_id", &self.profile_id)
            .field("voice_identity", &"[REDACTED]")
            .field("connection", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl ResolvedLiveExecution {
    /// Bind a configured managed model to the registry result without model
    /// prefix inference or substituting the executor's profile.
    pub fn function_bridge(
        declaration: &LiveManagedBackendModel,
        witness: ModelProfileWitness,
    ) -> Result<Self, LiveTargetError> {
        if declaration.provider != witness.provider() || declaration.model != witness.model() {
            return Err(LiveTargetError::BackendIdentityMismatch);
        }
        if witness.profile().interaction_kind != ModelInteractionKind::Text {
            return Err(LiveTargetError::BackendNotText);
        }
        Ok(Self::FunctionBridge { backend: witness })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveTargetError {
    #[error("live voice identity does not match its registry witness")]
    VoiceIdentityMismatch,
    #[error("live voice model does not declare continuous interaction")]
    VoiceNotContinuous,
    #[error("live connection provider does not match the voice identity")]
    ConnectionProviderMismatch,
    #[error("live target requires the resolved owning-realm auth binding")]
    MissingResolvedBinding,
    #[error("live connection credential owner does not match the resolved binding")]
    CredentialOwnerMismatch,
    #[error("live managed backend does not match its registry witness")]
    BackendIdentityMismatch,
    #[error("live managed backend cannot share the voice provider connection")]
    BackendProviderMismatch,
    #[error("live managed backend must declare text interaction")]
    BackendNotText,
}
