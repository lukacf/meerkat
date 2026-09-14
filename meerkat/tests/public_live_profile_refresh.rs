#![cfg(feature = "live")]

use std::collections::BTreeSet;

use meerkat::live_profile_refresh::{
    LiveAdmissionRefresh, LiveContextRefresh, LiveProfileRefreshCandidate, LiveProfileRefreshError,
    LiveProfileRefreshPlan, LiveProfileRefreshView, LiveProfileReplacementReason as Replacement,
    LiveProfileRevocationReason as Revocation, classify_live_profile_refresh,
};
use meerkat_core::execution_scope::{ExecutionGrantRef, ScopedExecutorBinding};
use meerkat_core::live_adapter::LiveAudioConfig;
use meerkat_core::live_execution::activation::{
    LiveActivationDeclaration, LiveActivationId, LiveProfileRevision,
};
use meerkat_core::live_execution::profile::{
    LiveContextProjectionPolicy, LiveProfileDefinition, LiveProfileExecution, LiveProfileId,
};
use meerkat_core::{AuthCredentialIdentity, RuntimeEpochId};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Clone)]
struct Configuration {
    profile_id: LiveProfileId,
    definition: LiveProfileDefinition,
    activation_id: LiveActivationId,
    activation: LiveActivationDeclaration<()>,
    grant: ExecutionGrantRef,
    executor: ScopedExecutorBinding,
    credential_owner: AuthCredentialIdentity,
    audio: LiveAudioConfig,
}

impl Configuration {
    fn fixture() -> Result<Self, Box<dyn std::error::Error>> {
        let mut value = Self {
            profile_id: LiveProfileId::parse("voice")?,
            definition: serde_json::from_value(json!({
                "voice_identity": {
                    "provider": "openai", "model": "gpt-live-1",
                    "auth_binding": {"realm": "voice-owner", "binding": "voice"}
                },
                "execution": {"mode": "function_bridge", "backend": {
                    "provider": "openai", "model": "gpt-5.5"
                }},
                "voice": "marin", "instructions": "private voice guidance",
                "context_projection": "reject"
            }))?,
            activation_id: LiveActivationId::parse("activation")?,
            activation: serde_json::from_value(json!({
                "issuer_realm": "owner", "profile_id": "voice", "profile_revision": vec![1; 32],
                "requesting_realms": ["owner"],
                "executor": {"kind": "session", "session_id": "00000000-0000-0000-0000-000000000001"},
                "allowed_evidence": ["application_snapshot"],
                "permission": {
                    "allowed_mutations": ["read_only"], "tools": {"kind": "unrestricted"},
                    "limits": {"max_requests": 100, "max_concurrent_requests": 1,
                        "max_effects_per_request": 10, "max_tokens_per_request": 1000,
                        "max_duration_ms": 30000}
                },
                "generation": 1, "revoke_policy": "cancel_pending_and_request_running_cancellation"
            }))?,
            grant: serde_json::from_value(json!({
                "id": "00000000-0000-0000-0000-000000000002", "issuer_realm": "owner", "generation": 1
            }))?,
            executor: serde_json::from_value(json!({
                "session_id": "00000000-0000-0000-0000-000000000001", "realm": "owner",
                "runtime_epoch": "00000000-0000-0000-0000-000000000003", "binding_generation": 1
            }))?,
            credential_owner: serde_json::from_value(
                json!({"realm": "voice-owner", "binding": "voice"}),
            )?,
            audio: LiveAudioConfig {
                input_sample_rate_hz: 24_000,
                input_channels: 1,
                output_sample_rate_hz: 24_000,
                output_channels: 1,
            },
        };
        value.revise()?;
        Ok(value)
    }

    fn revise(&mut self) -> Result<(), serde_json::Error> {
        self.activation.profile_revision = LiveProfileRevision::of(&self.definition)?;
        Ok(())
    }

    fn view(&self) -> LiveProfileRefreshView<'_, ()> {
        LiveProfileRefreshView {
            profile_id: &self.profile_id,
            definition: &self.definition,
            activation_id: &self.activation_id,
            activation: &self.activation,
            grant: &self.grant,
            executor: &self.executor,
            credential_owner: &self.credential_owner,
            audio: &self.audio,
        }
    }

    fn compare(&self, after: &Self) -> Result<LiveProfileRefreshPlan, LiveProfileRefreshError> {
        classify_live_profile_refresh(
            &self.view(),
            &LiveProfileRefreshCandidate::Selected(after.view()),
            chrono::DateTime::UNIX_EPOCH,
        )
    }
}

#[test]
fn unchanged_and_context_only_refresh_preserve_voice_without_replaying_history() -> TestResult {
    let before = Configuration::fixture()?;
    assert_eq!(
        before.compare(&before)?,
        LiveProfileRefreshPlan::Preserve {
            profile_revision: before.activation.profile_revision,
            context: LiveContextRefresh::Unchanged,
        }
    );
    let mut after = before.clone();
    after.definition.context_projection = LiveContextProjectionPolicy::RecentAuthorizedSuffix;
    after.revise()?;
    assert_ne!(
        before.activation.profile_revision,
        after.activation.profile_revision
    );
    assert_eq!(
        before.compare(&after)?,
        LiveProfileRefreshPlan::Preserve {
            profile_revision: after.activation.profile_revision,
            context: LiveContextRefresh::ReplanAtNextAuthorizedBoundary,
        }
    );
    assert_eq!(
        before.definition.context_projection,
        LiveContextProjectionPolicy::Reject
    );
    Ok(())
}

#[test]
fn immutable_profile_changes_are_exact_replacement_reasons() -> TestResult {
    let before = Configuration::fixture()?;
    for (path, replacement, reason) in [
        (
            vec!["voice_identity", "model"],
            json!("different-voice"),
            Replacement::VoiceIdentity,
        ),
        (
            vec!["voice_identity", "provider"],
            json!("anthropic"),
            Replacement::VoiceIdentity,
        ),
        (
            vec!["voice_identity", "auth_binding", "binding"],
            json!("other"),
            Replacement::VoiceIdentity,
        ),
        (vec!["voice"], json!("cedar"), Replacement::VoiceGuidance),
        (
            vec!["instructions"],
            json!("different private guidance"),
            Replacement::VoiceGuidance,
        ),
        (
            vec!["execution"],
            json!({"mode": "client_context", "request_policy": "snapshot_at_delegation"}),
            Replacement::ExecutionMode,
        ),
        (
            vec!["execution", "backend", "provider"],
            json!("anthropic"),
            Replacement::BackendProvider,
        ),
    ] {
        let mut after = before.clone();
        let mut wire = serde_json::to_value(&after.definition)?;
        let mut field = &mut wire;
        for key in &path {
            field = field.get_mut(*key).ok_or("missing fixture field")?;
        }
        *field = replacement;
        after.definition = serde_json::from_value(wire)?;
        after.revise()?;
        assert_eq!(
            before.compare(&after)?,
            LiveProfileRefreshPlan::ReplaceRequired {
                reasons: BTreeSet::from([reason]),
                admission: LiveAdmissionRefresh::RevalidateBeforeNextWork,
            },
            "{path:?}"
        );
    }
    Ok(())
}

#[test]
fn mutable_backend_update_is_revision_bound_and_carries_context_replan() -> TestResult {
    let before = Configuration::fixture()?;
    let mut after = before.clone();
    let LiveProfileExecution::FunctionBridge { backend } = &mut after.definition.execution else {
        return Err("fixture must use bridge mode".into());
    };
    backend.model = "gpt-5.4".into();
    after.definition.context_projection = LiveContextProjectionPolicy::RecentAuthorizedSuffix;
    after.revise()?;
    assert_eq!(
        before.compare(&after)?,
        LiveProfileRefreshPlan::QuiescentBackendUpdate {
            from_revision: before.activation.profile_revision,
            to_revision: after.activation.profile_revision,
            context: LiveContextRefresh::ReplanAtNextAuthorizedBoundary,
        }
    );
    after.definition.voice = Some("cedar".into());
    after.revise()?;
    assert!(
        matches!(
            before.compare(&after)?,
            LiveProfileRefreshPlan::ReplaceRequired { .. }
        ),
        "mutable backend edit must not hide an immutable voice change"
    );
    Ok(())
}

#[test]
fn executor_rebinding_and_permission_change_require_admission_fence() -> TestResult {
    let before = Configuration::fixture()?;
    let mut after = before.clone();
    after.executor.runtime_epoch = RuntimeEpochId::new();
    after.activation.permission.allowed_mutations.clear();
    assert_eq!(
        before.compare(&after)?,
        LiveProfileRefreshPlan::ReplaceRequired {
            reasons: BTreeSet::from([Replacement::ExecutorBinding, Replacement::Permission]),
            admission: LiveAdmissionRefresh::FenceRequired,
        }
    );
    after.executor = before.executor.clone();
    after.executor.binding_generation = serde_json::from_value(json!(2))?;
    after.activation.permission = before.activation.permission.clone();
    assert_eq!(
        before.compare(&after)?,
        LiveProfileRefreshPlan::ReplaceRequired {
            reasons: BTreeSet::from([Replacement::ExecutorBinding]),
            admission: LiveAdmissionRefresh::FenceRequired,
        }
    );
    Ok(())
}

#[test]
fn credential_owner_is_not_substituted_by_voice_or_executor_identity() -> TestResult {
    let before = Configuration::fixture()?;
    let mut after = before.clone();
    after.credential_owner =
        serde_json::from_value(json!({"realm": "different-owner", "binding": "voice"}))?;
    assert_eq!(
        before.compare(&after)?,
        LiveProfileRefreshPlan::ReplaceRequired {
            reasons: BTreeSet::from([Replacement::CredentialOwner]),
            admission: LiveAdmissionRefresh::RevalidateBeforeNextWork,
        }
    );
    Ok(())
}

#[test]
fn disable_revoke_expiry_and_reissued_grant_are_not_preserve() -> TestResult {
    let mut before = Configuration::fixture()?;
    for (candidate, reason) in [
        (
            LiveProfileRefreshCandidate::ProfileDisabled,
            Revocation::ProfileDisabled,
        ),
        (
            LiveProfileRefreshCandidate::ActivationRevoked,
            Revocation::ActivationRevoked,
        ),
    ] {
        assert_eq!(
            classify_live_profile_refresh(
                &before.view(),
                &candidate,
                chrono::DateTime::UNIX_EPOCH
            )?,
            LiveProfileRefreshPlan::Revoke { reason }
        );
    }
    let mut after = before.clone();
    after.grant.generation = serde_json::from_value(json!(2))?;
    after.activation.generation = after.grant.generation;
    assert_eq!(
        before.compare(&after)?,
        LiveProfileRefreshPlan::Revoke {
            reason: Revocation::ActivationChanged,
        }
    );
    before.activation.expires_at = Some(chrono::DateTime::UNIX_EPOCH);
    assert_eq!(
        before.compare(&before)?,
        LiveProfileRefreshPlan::Revoke {
            reason: Revocation::ActivationExpired,
        }
    );
    Ok(())
}

#[test]
fn incoherent_revision_or_grant_content_never_defaults_to_preserve() -> TestResult {
    let before = Configuration::fixture()?;
    let mut after = before.clone();
    after.definition.instructions = None;
    assert!(matches!(
        before.compare(&after),
        Err(LiveProfileRefreshError::ProfileRevisionMismatch)
    ));
    after.revise()?;
    after.grant.generation = serde_json::from_value(json!(2))?;
    assert!(matches!(
        before.compare(&after),
        Err(LiveProfileRefreshError::GrantDeclarationMismatch)
    ));
    Ok(())
}

#[test]
fn activation_scope_edits_require_fencing_even_without_a_profile_revision_change() -> TestResult {
    let before = Configuration::fixture()?;
    for (pointer, replacement) in [
        ("/allowed_evidence", json!([])),
        ("/requesting_realms", json!([])),
        ("/permission/allowed_mutations", json!([])),
        ("/permission/limits/max_requests", json!(99)),
        ("/permission/limits/max_duration_ms", json!(20000)),
    ] {
        let mut after = before.clone();
        let mut wire = serde_json::to_value(&after.activation)?;
        *wire
            .pointer_mut(pointer)
            .ok_or("missing activation field")? = replacement;
        after.activation = serde_json::from_value(wire)?;
        assert_eq!(
            after.activation.profile_revision,
            before.activation.profile_revision
        );
        assert_eq!(
            before.compare(&after)?,
            LiveProfileRefreshPlan::ReplaceRequired {
                reasons: BTreeSet::from([Replacement::Permission]),
                admission: LiveAdmissionRefresh::FenceRequired,
            },
            "{pointer}"
        );
    }
    Ok(())
}

#[test]
fn client_request_policy_and_selected_profile_do_not_silently_rebind() -> TestResult {
    use meerkat_core::live_execution::profile::LiveClientRequestPolicy;

    let mut before = Configuration::fixture()?;
    before.definition.execution = LiveProfileExecution::ClientContext {
        request_policy: LiveClientRequestPolicy::SnapshotAtDelegation,
    };
    before.revise()?;
    let mut after = before.clone();
    after.definition.execution = LiveProfileExecution::ClientContext {
        request_policy: LiveClientRequestPolicy::ExplicitApplicationRequest,
    };
    after.revise()?;
    assert_eq!(
        before.compare(&after)?,
        LiveProfileRefreshPlan::ReplaceRequired {
            reasons: BTreeSet::from([Replacement::ClientRequestPolicy]),
            admission: LiveAdmissionRefresh::RevalidateBeforeNextWork,
        }
    );
    after.profile_id = LiveProfileId::parse("another-profile")?;
    after.activation.profile_id = after.profile_id.clone();
    assert_eq!(
        before.compare(&after)?,
        LiveProfileRefreshPlan::ReplaceRequired {
            reasons: BTreeSet::from([
                Replacement::ProfileSelection,
                Replacement::ClientRequestPolicy,
            ]),
            admission: LiveAdmissionRefresh::FenceRequired,
        }
    );
    Ok(())
}

#[test]
fn negotiated_audio_format_changes_require_replacement() -> TestResult {
    let before = Configuration::fixture()?;
    for (field, value) in [
        ("input_sample_rate_hz", 48_000),
        ("input_channels", 2),
        ("output_sample_rate_hz", 48_000),
        ("output_channels", 2),
    ] {
        let mut after = before.clone();
        let mut audio = serde_json::to_value(&after.audio)?;
        audio[field] = json!(value);
        after.audio = serde_json::from_value(audio)?;
        assert_eq!(
            before.compare(&after)?,
            LiveProfileRefreshPlan::ReplaceRequired {
                reasons: BTreeSet::from([Replacement::AudioFormat]),
                admission: LiveAdmissionRefresh::RevalidateBeforeNextWork,
            },
            "{field}"
        );
    }
    Ok(())
}
