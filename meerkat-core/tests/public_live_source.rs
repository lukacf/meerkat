use meerkat_core::SessionId;
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_execution::request::{
    LiveApplicationRequestId, LiveDelegationAttribution, LiveProviderReference,
    LiveRequestCancelIntent, LiveRequestCancellationReason, LiveRequestEvidenceKind,
    LiveResponseIdentity, LiveSourceIdentity, LiveSourceKey, LiveSourceScopeError,
};
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn reference(value: &str) -> Result<LiveProviderReference, Box<dyn std::error::Error>> {
    Ok(LiveProviderReference::new(value)?)
}

#[test]
fn source_key_contains_identity_not_payload_or_admission_state() -> TestResult {
    let key = LiveSourceKey::new(
        SessionId::new(),
        LiveChannelId::new("channel-a"),
        LiveSourceIdentity::ClientDelegation {
            delegation: reference("delegation-a")?,
        },
    )?;
    let encoded = serde_json::to_value(&key)?;
    let object = encoded.as_object().ok_or("expected object")?;
    assert_eq!(object.len(), 3);
    for field in [
        "request",
        "request_digest",
        "snapshot",
        "watermark",
        "permission",
        "input_id",
        "run_id",
    ] {
        assert!(object.get(field).is_none());
    }
    assert_eq!(serde_json::from_value::<LiveSourceKey>(encoded)?, key);
    Ok(())
}

#[test]
fn channel_or_session_replacement_changes_the_source_namespace() -> TestResult {
    let session = SessionId::new();
    let source = LiveSourceIdentity::ClientDelegation {
        delegation: reference("same-provider-source")?,
    };
    let original = LiveSourceKey::new(
        session.clone(),
        LiveChannelId::new("old-channel"),
        source.clone(),
    )?;
    let new_channel =
        LiveSourceKey::new(session, LiveChannelId::new("new-channel"), source.clone())?;
    let new_session =
        LiveSourceKey::new(SessionId::new(), LiveChannelId::new("old-channel"), source)?;
    assert_ne!(original, new_channel);
    assert_ne!(original, new_session);
    Ok(())
}

#[test]
fn known_unscoped_response_is_retained_but_cannot_form_a_function_source() -> TestResult {
    for attribution in [
        LiveDelegationAttribution::Absent {},
        LiveDelegationAttribution::ExplicitNull {},
    ] {
        let identity = LiveResponseIdentity {
            response: reference("known-response")?,
            attribution,
        };
        assert_eq!(
            identity.function_source(reference("call")?),
            Err(LiveSourceScopeError::MissingDelegationAttribution)
        );
        let encoded = serde_json::to_value(&identity)?;
        assert_eq!(
            serde_json::from_value::<LiveResponseIdentity>(encoded)?,
            identity
        );
        assert_eq!(identity.response.as_str(), "known-response");
    }
    assert_ne!(
        serde_json::to_value(LiveDelegationAttribution::Absent {})?,
        serde_json::to_value(LiveDelegationAttribution::ExplicitNull {})?
    );
    for kind in ["absent", "explicit_null"] {
        assert!(
            serde_json::from_value::<LiveDelegationAttribution>(
                json!({"kind": kind, "delegation": "must-not-disappear"})
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn function_identity_preserves_response_and_delegation_scope() -> TestResult {
    let identity = LiveResponseIdentity {
        response: reference("response-a")?,
        attribution: LiveDelegationAttribution::Known {
            delegation: reference("delegation-a")?,
        },
    };
    let source = identity.function_source(reference("call")?)?;
    let other_response = LiveResponseIdentity {
        response: reference("response-b")?,
        attribution: identity.attribution.clone(),
    };
    let other_delegation = LiveResponseIdentity {
        response: identity.response,
        attribution: LiveDelegationAttribution::Known {
            delegation: reference("delegation-b")?,
        },
    };
    assert_ne!(source, other_response.function_source(reference("call")?)?);
    assert_ne!(
        source,
        other_delegation.function_source(reference("call")?)?
    );
    assert_eq!(
        serde_json::from_value::<LiveSourceIdentity>(serde_json::to_value(&source)?)?,
        source
    );
    Ok(())
}

#[test]
fn explicit_application_resubmission_is_not_a_fake_provider_delegation() -> TestResult {
    let source = LiveSourceIdentity::ApplicationRequest {
        request_id: LiveApplicationRequestId::from_uuid(uuid::Uuid::new_v4()),
    };
    let encoded = serde_json::to_value(&source)?;
    assert_eq!(encoded["kind"], "application_request");
    assert!(encoded.get("delegation").is_none());
    assert_eq!(
        serde_json::from_value::<LiveSourceIdentity>(encoded)?,
        source
    );
    Ok(())
}

#[test]
fn empty_references_and_added_digest_cannot_enter_source_representation() -> TestResult {
    for value in ["", " ", "\t\n"] {
        assert!(LiveProviderReference::new(value).is_err());
        assert!(serde_json::from_value::<LiveProviderReference>(json!(value)).is_err());
    }
    let mut key = json!({
        "session_id": SessionId::new(),
        "channel_id": "channel",
        "source": {"kind": "client_delegation", "delegation": "source"},
        "request_digest": "must-not-change-identity",
    });
    assert!(serde_json::from_value::<LiveSourceKey>(key.clone()).is_err());
    key.as_object_mut()
        .ok_or("expected object")?
        .remove("request_digest");
    key["channel_id"] = json!(" ");
    assert!(serde_json::from_value::<LiveSourceKey>(key).is_err());
    Ok(())
}

#[test]
fn provider_references_are_exact_and_debug_redacted() -> TestResult {
    let reference = reference("  private-reference-canary  ")?;
    assert_eq!(reference.as_str(), "  private-reference-canary  ");
    assert!(!format!("{reference:?}").contains("private-reference-canary"));
    assert_eq!(
        serde_json::to_value(reference)?,
        json!("  private-reference-canary  ")
    );
    assert_eq!(
        serde_json::to_value(LiveRequestEvidenceKind::ApplicationSnapshot)?,
        json!("application_snapshot")
    );
    Ok(())
}

#[test]
fn cancellation_intent_precedes_input_identity_without_targeting_current_run() -> TestResult {
    let intent = LiveRequestCancelIntent {
        source: LiveSourceKey::new(
            SessionId::new(),
            LiveChannelId::new("channel"),
            LiveSourceIdentity::ClientDelegation {
                delegation: reference("source")?,
            },
        )?,
        reason: LiveRequestCancellationReason::OperatorRequested,
    };
    let mut encoded = serde_json::to_value(&intent)?;
    assert!(encoded.get("input_id").is_none());
    assert!(encoded.get("run_id").is_none());
    assert_eq!(
        serde_json::from_value::<LiveRequestCancelIntent>(encoded.clone())?,
        intent
    );
    encoded["interrupt_current"] = json!(true);
    assert!(serde_json::from_value::<LiveRequestCancelIntent>(encoded).is_err());
    for reason in ["channel_closed", "speech_interrupted", "barge_in"] {
        assert!(serde_json::from_value::<LiveRequestCancellationReason>(json!(reason)).is_err());
    }
    Ok(())
}
