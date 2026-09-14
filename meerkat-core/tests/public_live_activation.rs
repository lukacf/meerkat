use meerkat_core::live_execution::activation::{
    LiveActivationDeclaration, LiveActivationDocument, LiveActivationEntry, LiveActivationId,
    LiveExecutorSelector, LiveProfileRevision, LiveToolRestriction, LiveWorkLimits,
};
use meerkat_core::live_execution::profile::LiveProfileDefinition;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The real mob boundary supplies its own canonical identity types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberFixture {
    mob_id: String,
    agent_identity: String,
}

fn declaration() -> Value {
    json!({
        "issuer_realm": "owner",
        "profile_id": "voice",
        "profile_revision": vec![7; 32],
        "requesting_realms": ["child", "owner"],
        "executor": {
            "kind": "session",
            "session_id": "00000000-0000-0000-0000-000000000001"
        },
        "allowed_evidence": ["application_snapshot", "structured_function_request"],
        "permission": {
            "allowed_mutations": ["read_only"],
            "tools": {"kind": "allow_listed", "names": ["lookup"]},
            "limits": {
                "max_requests": 100,
                "max_concurrent_requests": 1,
                "max_effects_per_request": 10,
                "max_tokens_per_request": 8192,
                "max_duration_ms": 30000
            }
        },
        "generation": 1,
        "revoke_policy": "cancel_pending_and_request_running_cancellation"
    })
}

fn reject_extra<T: DeserializeOwned + Serialize>(value: Value) -> TestResult {
    let parsed: T = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(parsed)?, value);
    let mut altered = value;
    altered["unexpected_permission"] = json!(true);
    assert!(serde_json::from_value::<T>(altered).is_err());
    Ok(())
}

#[test]
fn activation_is_a_distinct_strict_document_with_no_default_permission() -> TestResult {
    let document: LiveActivationDocument<MemberFixture> = serde_json::from_value(json!({}))?;
    assert!(document.activations.is_empty());
    for entry in [
        json!({"mode": "inherit"}),
        json!({"mode": "disable"}),
        json!({"mode": "set", "declaration": declaration()}),
    ] {
        reject_extra::<LiveActivationEntry<MemberFixture>>(entry)?;
    }
    for field in ["grant", "token", "api_key", "sealed_authority"] {
        let mut value = declaration();
        value[field] = json!("not-permission");
        assert!(serde_json::from_value::<LiveActivationDeclaration<MemberFixture>>(value).is_err());
        let mut profile = json!({"live": {"profiles": {}}});
        profile["live"][field] = declaration();
        assert!(serde_json::from_value::<meerkat_core::Config>(profile).is_err());
    }
    Ok(())
}

#[test]
fn every_activation_selector_and_tool_case_rejects_extra_fields() -> TestResult {
    for selector in [
        declaration()["executor"].clone(),
        json!({"kind": "mob_member", "member": {
            "mob_id": "team", "agent_identity": "worker"
        }}),
    ] {
        reject_extra::<LiveExecutorSelector<MemberFixture>>(selector)?;
    }
    for restriction in [
        json!({"kind": "unrestricted"}),
        json!({"kind": "allow_listed", "names": ["lookup"]}),
    ] {
        reject_extra::<LiveToolRestriction>(restriction)?;
    }
    for selector in [
        json!({"kind": "all_sessions"}),
        json!({"kind": "session"}),
        json!({"kind": "mob_member", "member": {
            "mob_id": "team", "agent_identity": "worker", "role": "inferred"
        }}),
    ] {
        assert!(serde_json::from_value::<LiveExecutorSelector<MemberFixture>>(selector).is_err());
    }
    Ok(())
}

#[test]
fn activation_exact_identity_expiry_and_generation_round_trip() -> TestResult {
    let mut value = declaration();
    value["expires_at"] = json!("2026-09-12T00:00:00Z");
    reject_extra::<LiveActivationDeclaration<MemberFixture>>(value.clone())?;
    let expected: LiveActivationDeclaration<MemberFixture> = serde_json::from_value(value.clone())?;
    for (field, replacement) in [
        ("issuer_realm", json!("another-owner")),
        ("profile_id", json!("another-profile")),
        ("requesting_realms", json!(["another-child"])),
        ("generation", json!(2)),
    ] {
        let mut altered = value.clone();
        altered[field] = replacement;
        assert_ne!(
            serde_json::from_value::<LiveActivationDeclaration<MemberFixture>>(altered)?,
            expected
        );
    }
    for field in [
        "issuer_realm",
        "profile_revision",
        "generation",
        "permission",
        "executor",
    ] {
        let mut altered = value.clone();
        altered.as_object_mut().ok_or("object")?.remove(field);
        assert!(
            serde_json::from_value::<LiveActivationDeclaration<MemberFixture>>(altered).is_err()
        );
    }
    value["generation"] = json!(0);
    assert!(serde_json::from_value::<LiveActivationDeclaration<MemberFixture>>(value).is_err());
    assert!(LiveActivationId::parse("../voice").is_err());
    Ok(())
}

#[test]
fn work_limits_require_all_positive_bounds_and_consistent_concurrency() -> TestResult {
    let valid = declaration()["permission"]["limits"].clone();
    reject_extra::<LiveWorkLimits>(valid.clone())?;
    for field in [
        "max_requests",
        "max_concurrent_requests",
        "max_effects_per_request",
        "max_tokens_per_request",
        "max_duration_ms",
    ] {
        let mut value = valid.clone();
        value[field] = json!(0);
        assert!(serde_json::from_value::<LiveWorkLimits>(value).is_err());
        let mut value = valid.clone();
        value.as_object_mut().ok_or("object")?.remove(field);
        assert!(serde_json::from_value::<LiveWorkLimits>(value).is_err());
    }
    let mut value = valid;
    value["max_concurrent_requests"] = json!(101);
    assert!(serde_json::from_value::<LiveWorkLimits>(value).is_err());
    Ok(())
}

#[test]
fn profile_revision_binds_guidance_and_backend_not_just_model_name() -> TestResult {
    let mut value = json!({
        "voice_identity": {"provider": "openai", "model": "voice"},
        "execution": {"mode": "client_context", "request_policy": "snapshot_at_delegation"},
        "context_projection": "recent_authorized_suffix"
    });
    let profile: LiveProfileDefinition = serde_json::from_value(value.clone())?;
    let revision = LiveProfileRevision::of(&profile)?;
    assert_eq!(
        serde_json::from_value::<LiveProfileRevision>(serde_json::to_value(revision)?)?,
        revision
    );
    value["instructions"] = json!("different exact guidance\n");
    assert_ne!(
        LiveProfileRevision::of(&serde_json::from_value(value)?)?,
        revision
    );
    Ok(())
}
