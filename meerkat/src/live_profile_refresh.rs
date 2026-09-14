//! Pure classification of current, owner-resolved Live configuration content.
//!
//! Views and plans are not grant, admission, or update-ACK authority. The
//! generated owners must realize fences, reserve a quiescent update, and bind
//! its exact acknowledgement before installing a changed runnable binding.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use meerkat_core::AuthCredentialIdentity;
use meerkat_core::execution_scope::{ExecutionGrantRef, ScopedExecutorBinding};
use meerkat_core::live_adapter::LiveAudioConfig;
use meerkat_core::live_execution::activation::{
    LiveActivationDeclaration, LiveActivationId, LiveProfileRevision,
};
use meerkat_core::live_execution::profile::{
    LiveProfileDefinition, LiveProfileExecution, LiveProfileId,
};

/// Borrowed comparison content, never a deserialized runnable binding.
///
/// The owning host supplies freshly resolved configuration and activation.
/// Ordinary executor model, auth, and transcript changes are deliberately not
/// voice configuration; their owners must revalidate scope before next work.
pub struct LiveProfileRefreshView<'a, Member> {
    pub profile_id: &'a LiveProfileId,
    pub definition: &'a LiveProfileDefinition,
    pub activation_id: &'a LiveActivationId,
    pub activation: &'a LiveActivationDeclaration<Member>,
    pub grant: &'a ExecutionGrantRef,
    pub executor: &'a ScopedExecutorBinding,
    pub credential_owner: &'a AuthCredentialIdentity,
    pub audio: &'a LiveAudioConfig,
}

pub enum LiveProfileRefreshCandidate<'a, Member> {
    Selected(LiveProfileRefreshView<'a, Member>),
    ProfileDisabled,
    ActivationRevoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveContextRefresh {
    Unchanged,
    ReplanAtNextAuthorizedBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveAdmissionRefresh {
    RevalidateBeforeNextWork,
    FenceRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LiveProfileReplacementReason {
    ProfileSelection,
    ExecutorBinding,
    Permission,
    VoiceIdentity,
    CredentialOwner,
    VoiceGuidance,
    AudioFormat,
    ExecutionMode,
    ClientRequestPolicy,
    BackendProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveProfileRevocationReason {
    ProfileDisabled,
    ActivationRevoked,
    ActivationChanged,
    ActivationExpired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveProfileRefreshPlan {
    Preserve {
        profile_revision: LiveProfileRevision,
        context: LiveContextRefresh,
    },
    QuiescentBackendUpdate {
        from_revision: LiveProfileRevision,
        to_revision: LiveProfileRevision,
        context: LiveContextRefresh,
    },
    ReplaceRequired {
        reasons: BTreeSet<LiveProfileReplacementReason>,
        admission: LiveAdmissionRefresh,
    },
    Revoke {
        reason: LiveProfileRevocationReason,
    },
}

impl<Member> LiveProfileRefreshView<'_, Member> {
    fn validate_content(&self) -> Result<(), LiveProfileRefreshError> {
        if self.profile_id != &self.activation.profile_id
            || LiveProfileRevision::of(self.definition)? != self.activation.profile_revision
        {
            return Err(LiveProfileRefreshError::ProfileRevisionMismatch);
        }
        if self.grant.issuer_realm != self.activation.issuer_realm
            || self.grant.generation != self.activation.generation
        {
            return Err(LiveProfileRefreshError::GrantDeclarationMismatch);
        }
        Ok(())
    }
}

/// Classify content changes without applying them or replaying provider history.
/// A failed resolution is an error at the caller, not a disabled/default view.
pub fn classify_live_profile_refresh<Member: PartialEq>(
    before: &LiveProfileRefreshView<'_, Member>,
    candidate: &LiveProfileRefreshCandidate<'_, Member>,
    now: DateTime<Utc>,
) -> Result<LiveProfileRefreshPlan, LiveProfileRefreshError> {
    use LiveProfileReplacementReason as Replacement;
    use LiveProfileRevocationReason as Revocation;

    let after = match candidate {
        LiveProfileRefreshCandidate::ProfileDisabled => {
            return Ok(LiveProfileRefreshPlan::Revoke {
                reason: Revocation::ProfileDisabled,
            });
        }
        LiveProfileRefreshCandidate::ActivationRevoked => {
            return Ok(LiveProfileRefreshPlan::Revoke {
                reason: Revocation::ActivationRevoked,
            });
        }
        LiveProfileRefreshCandidate::Selected(after) => after,
    };
    before.validate_content()?;
    after.validate_content()?;
    if after
        .activation
        .expires_at
        .is_some_and(|expiry| now >= expiry)
    {
        return Ok(LiveProfileRefreshPlan::Revoke {
            reason: Revocation::ActivationExpired,
        });
    }

    let LiveActivationDeclaration {
        issuer_realm,
        profile_id: _,
        profile_revision: _,
        requesting_realms,
        executor,
        allowed_evidence,
        permission,
        expires_at,
        generation,
        revoke_policy,
    } = after.activation;
    if before.activation_id != after.activation_id
        || before.grant != after.grant
        || &before.activation.issuer_realm != issuer_realm
        || &before.activation.generation != generation
        || &before.activation.expires_at != expires_at
        || &before.activation.revoke_policy != revoke_policy
    {
        return Ok(LiveProfileRefreshPlan::Revoke {
            reason: Revocation::ActivationChanged,
        });
    }

    let mut reasons = BTreeSet::new();
    let mut admission = LiveAdmissionRefresh::RevalidateBeforeNextWork;
    if before.profile_id != after.profile_id {
        reasons.insert(Replacement::ProfileSelection);
        admission = LiveAdmissionRefresh::FenceRequired;
    }
    if before.executor != after.executor || &before.activation.executor != executor {
        reasons.insert(Replacement::ExecutorBinding);
        admission = LiveAdmissionRefresh::FenceRequired;
    }
    if &before.activation.permission != permission
        || &before.activation.allowed_evidence != allowed_evidence
        || &before.activation.requesting_realms != requesting_realms
    {
        reasons.insert(Replacement::Permission);
        admission = LiveAdmissionRefresh::FenceRequired;
    }
    let LiveProfileDefinition {
        voice_identity,
        execution,
        voice,
        instructions,
        context_projection,
    } = after.definition;
    if &before.definition.voice_identity != voice_identity {
        reasons.insert(Replacement::VoiceIdentity);
    }
    if before.credential_owner != after.credential_owner {
        reasons.insert(Replacement::CredentialOwner);
    }
    if before.audio != after.audio {
        reasons.insert(Replacement::AudioFormat);
    }
    if &before.definition.voice != voice || &before.definition.instructions != instructions {
        reasons.insert(Replacement::VoiceGuidance);
    }
    let backend_update = match (&before.definition.execution, execution) {
        (
            LiveProfileExecution::ClientContext {
                request_policy: old,
            },
            LiveProfileExecution::ClientContext {
                request_policy: new,
            },
        ) => {
            if old != new {
                reasons.insert(Replacement::ClientRequestPolicy);
            }
            false
        }
        (
            LiveProfileExecution::FunctionBridge { backend: old },
            LiveProfileExecution::FunctionBridge { backend: new },
        ) => {
            let meerkat_core::live_execution::profile::LiveManagedBackendModel { provider, model } =
                new;
            if &old.provider != provider {
                reasons.insert(Replacement::BackendProvider);
            }
            &old.model != model
        }
        (
            LiveProfileExecution::ClientContext { .. },
            LiveProfileExecution::FunctionBridge { .. },
        )
        | (
            LiveProfileExecution::FunctionBridge { .. },
            LiveProfileExecution::ClientContext { .. },
        ) => {
            reasons.insert(Replacement::ExecutionMode);
            false
        }
    };
    if !reasons.is_empty() {
        return Ok(LiveProfileRefreshPlan::ReplaceRequired { reasons, admission });
    }
    let context = if &before.definition.context_projection == context_projection {
        LiveContextRefresh::Unchanged
    } else {
        LiveContextRefresh::ReplanAtNextAuthorizedBoundary
    };
    if backend_update {
        Ok(LiveProfileRefreshPlan::QuiescentBackendUpdate {
            from_revision: before.activation.profile_revision,
            to_revision: after.activation.profile_revision,
            context,
        })
    } else {
        Ok(LiveProfileRefreshPlan::Preserve {
            profile_revision: after.activation.profile_revision,
            context,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LiveProfileRefreshError {
    #[error("Live profile content does not match its selected activation revision")]
    ProfileRevisionMismatch,
    #[error("Live grant reference does not match its activation issuer and generation")]
    GrantDeclarationMismatch,
    #[error("Live profile revision encoding failed: {0}")]
    RevisionEncoding(#[from] serde_json::Error),
}
