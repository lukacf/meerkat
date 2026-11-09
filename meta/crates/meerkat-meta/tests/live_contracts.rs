#![allow(clippy::expect_used)]

use meerkat_contracts::{LiveOpenResult, WireLiveOpenAdvisory};
use meerkat_meta::{
    ActiveLiveChannelHandle, ExperimentalLiveChannelStatus, FeatureCapability,
    LIVE_EXECUTION_CLIENT_CONTEXT_V1, LIVE_EXECUTION_FUNCTION_BRIDGE_V1,
    LIVE_EXECUTION_IDENTITY_V1, LiveChannelHandle, LiveExecutionIdentityV1, LiveExecutionMode,
    LivePlaybackOwnerReadiness, PendingLiveChannelHandle, RpcCapabilities,
    parse_live_open_execution_identity, validate_experimental_live_open_surface,
    validate_experimental_live_target_surface,
};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/live_contracts_v1.json"))
        .expect("live contract fixture must be valid JSON")
}

#[test]
fn execution_identity_v1_matches_shared_fixture() {
    let expected = fixture()["execution_identity"].clone();
    let identity: LiveExecutionIdentityV1 =
        serde_json::from_value(expected.clone()).expect("fixture identity must decode");
    assert_eq!(
        serde_json::to_value(identity).expect("identity must encode"),
        expected
    );
}

#[test]
fn execution_identity_v1_rejects_unknown_fields_and_ambiguous_clear() {
    let unknown_identity = json!({"version": "v1", "model": "gpt-live-1-codex", "future": true});
    assert!(serde_json::from_value::<LiveExecutionIdentityV1>(unknown_identity).is_err());
    assert!(
        serde_json::from_value::<LiveExecutionIdentityV1>(json!({"version": "v1", "model": ""}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<LiveExecutionIdentityV1>(json!({
            "version": "v1",
            "profile_id": "   "
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<LiveExecutionIdentityV1>(
            json!({"version": "v1", "provider": "unknown"})
        )
        .is_err()
    );

    for invalid in [
        json!({"profile_id": "example.device.open-room.v1"}),
        json!({"version": "v2", "profile_id": "example.device.open-room.v1"}),
        json!({"version": "v1", "profile_id": "example.device.open-room.v1", "model": "gpt-live-1-codex"}),
        json!({"version": "v1", "profile_id": "example.device.open-room.v1", "provider": "openai"}),
        json!({"version": "v1", "profile_id": "example.device.open-room.v1", "self_hosted_server_id": "server"}),
        json!({"version": "v1", "profile_id": "example.device.open-room.v1", "auth_binding": {"action": "clear"}}),
    ] {
        assert!(serde_json::from_value::<LiveExecutionIdentityV1>(invalid).is_err());
    }
}

#[test]
fn live_open_rejects_legacy_model_or_provider_conflicts() {
    for legacy in [json!({"model": "legacy"}), json!({"provider": "openai"})] {
        let mut params = legacy.as_object().expect("object").clone();
        params.insert(
            "execution_identity".to_string(),
            fixture()["execution_identity"].clone(),
        );
        let error = parse_live_open_execution_identity(&Value::Object(params))
            .expect_err("legacy/new conflict must fail");
        assert!(
            error
                .to_string()
                .contains("conflicts with legacy top-level")
        );
    }
}

#[test]
fn channel_handle_matches_shared_fixture() {
    let expected = fixture()["channel_handle"].clone();
    let handle: LiveChannelHandle =
        serde_json::from_value(expected.clone()).expect("fixture handle must decode");
    assert_eq!(
        serde_json::to_value(handle).expect("handle must encode"),
        expected
    );
}

/// A live open's advisories (what it went ahead without, such as a refused
/// staged create-time prompt) ride both handles as Meerkat's open result
/// reported them, an advisory this version does not know kept as it was
/// sent, and are left out of the wire when there are none.
#[test]
fn handles_carry_the_open_advisories_and_omit_them_when_empty() {
    let expected_advisories = vec![
        WireLiveOpenAdvisory::StagedPromptRefused,
        WireLiveOpenAdvisory::from("a_future_advisory".to_string()),
    ];
    let open_result = |handle: &Value| -> LiveOpenResult {
        let mut result = handle.as_object().expect("handle object").clone();
        for key in ["target_identity", "execution_mode", "pending_receipt"] {
            result.remove(key);
        }
        serde_json::from_value(Value::Object(result)).expect("open result must decode")
    };

    let expected = fixture()["channel_handle_with_advisories"].clone();
    let handle: LiveChannelHandle =
        serde_json::from_value(expected.clone()).expect("handle with advisories must decode");
    assert_eq!(handle.advisories, expected_advisories);
    assert_eq!(
        serde_json::to_value(&handle).expect("handle must encode"),
        expected
    );
    let from_open = LiveChannelHandle::from_open_result("identity:example", open_result(&expected));
    assert_eq!(
        serde_json::to_value(from_open).expect("handle must encode"),
        expected
    );

    let expected_pending = fixture()["pending_channel_handle_with_advisories"].clone();
    let pending: PendingLiveChannelHandle = serde_json::from_value(expected_pending.clone())
        .expect("pending handle with advisories must decode");
    assert_eq!(pending.advisories, expected_advisories);
    assert_eq!(
        serde_json::to_value(&pending).expect("pending handle must encode"),
        expected_pending
    );
    let pending_from_open = PendingLiveChannelHandle::new(
        "identity:example",
        LiveExecutionMode::ClientContext,
        "opaque-pending-receipt",
        open_result(&expected_pending),
    );
    assert_eq!(
        serde_json::to_value(pending_from_open).expect("pending handle must encode"),
        expected_pending
    );

    for (key, kind) in [
        ("channel_handle", "handle"),
        ("pending_channel_handle", "pending"),
    ] {
        let without = fixture()[key].clone();
        let encoded = if kind == "handle" {
            let handle =
                LiveChannelHandle::from_open_result("identity:example", open_result(&without));
            assert!(handle.advisories.is_empty());
            serde_json::to_value(handle).expect("handle must encode")
        } else {
            let pending = PendingLiveChannelHandle::new(
                "identity:example",
                LiveExecutionMode::ClientContext,
                "opaque-pending-receipt",
                open_result(&without),
            );
            assert!(pending.advisories.is_empty());
            serde_json::to_value(pending).expect("pending handle must encode")
        };
        assert_eq!(
            encoded, without,
            "no advisories leaves the {kind} wire unchanged"
        );
    }

    let mut not_a_list = fixture()["channel_handle"].clone();
    not_a_list["advisories"] = json!("staged_prompt_refused");
    assert!(serde_json::from_value::<LiveChannelHandle>(not_a_list).is_err());
    let mut not_strings = fixture()["pending_channel_handle"].clone();
    not_strings["advisories"] = json!([1]);
    assert!(serde_json::from_value::<PendingLiveChannelHandle>(not_strings).is_err());
}

#[test]
fn pending_and_active_handles_match_shared_fixtures() {
    let pending: PendingLiveChannelHandle =
        serde_json::from_value(fixture()["pending_channel_handle"].clone())
            .expect("pending handle must decode");
    assert_eq!(pending.execution_mode, LiveExecutionMode::ClientContext);
    assert_eq!(
        serde_json::to_value(&pending).expect("pending handle must encode"),
        fixture()["pending_channel_handle"]
    );

    let active: ActiveLiveChannelHandle =
        serde_json::from_value(fixture()["active_channel_handle"].clone())
            .expect("active handle must decode");
    assert_eq!(active.channel_id, pending.channel_id);
    assert_ne!(active.activation_receipt, pending.pending_receipt);

    let readiness: LivePlaybackOwnerReadiness =
        serde_json::from_value(fixture()["playback_owner_readiness"].clone())
            .expect("readiness must decode");
    assert_eq!(readiness.channel_id, pending.channel_id);

    for drifted in [
        json!({
            "channel_id": "live-ch-1",
            "target_identity": "identity:example",
            "execution_mode": "responses",
            "activation_receipt": "receipt"
        }),
        json!({
            "channel_id": "live-ch-1",
            "target_identity": "identity:example",
            "execution_mode": "function_bridge",
            "activation_receipt": "receipt",
            "responses_model": "gpt-5.5"
        }),
        json!({
            "channel_id": "live-ch-1",
            "target_identity": "identity:example",
            "execution_mode": "function_bridge",
            "activation_receipt": "  "
        }),
    ] {
        assert!(serde_json::from_value::<ActiveLiveChannelHandle>(drifted).is_err());
    }

    assert!(
        serde_json::from_value::<LivePlaybackOwnerReadiness>(json!({
            "channel_id": pending.channel_id,
            "readiness_receipt": ""
        }))
        .is_err()
    );

    for key in [
        "pending_status",
        "active_status",
        "revoked_status",
        "closed_status",
    ] {
        let status: ExperimentalLiveChannelStatus =
            serde_json::from_value(fixture()[key].clone()).expect("phase status must decode");
        assert_eq!(
            serde_json::to_value(status).expect("phase status must encode"),
            fixture()[key]
        );
    }
}

#[test]
fn strict_experimental_surface_is_identity_only_and_provider_neutral() {
    validate_experimental_live_open_surface(&json!({
        "identity": "identity:example",
        "execution_identity": {
            "version": "v1",
            "profile_id": "gpt-live-function-bridge-v1"
        }
    }))
    .expect("identity-only target must pass surface validation");

    for invalid in [
        json!({"session_id": "session:raw"}),
        json!({"member_id": "rt:example:0"}),
        json!({"identity": "identity:example", "session_id": "session:raw"}),
        json!({"identity": "identity:example", "delegation_type": "responses"}),
        json!({"identity": "identity:example", "responses_model": "gpt-5.5"}),
        json!({"identity": "identity:example", "responses_tools": []}),
        json!({"identity": "identity:example", "responses_instructions": "delegate"}),
        json!({"identity": "identity:example", "profile_id": "gpt-live-function-bridge-v1"}),
        json!({"identity": "identity:example", "execution_profile": "function_bridge"}),
        json!({"identity": "identity:example", "auth_binding": {"realm": "example", "binding": "other"}}),
        json!({"identity": "identity:example", "self_hosted_server_id": "server"}),
        json!({"identity": "identity:example", "provider_params": {}}),
        json!({"identity": "identity:example", "tools": []}),
        json!({"identity": "identity:example", "instructions": "delegate"}),
    ] {
        assert!(validate_experimental_live_open_surface(&invalid).is_err());
    }

    for invalid in [
        json!({
            "identity": "identity:example",
            "session_id": "session:raw",
            "pending_receipt": "pending"
        }),
        json!({
            "identity": "identity:example",
            "member_id": "member:raw",
            "activation_receipt": "active"
        }),
        json!({
            "identity": "rt:identity:example:0",
            "pending_receipt": "pending"
        }),
    ] {
        assert!(validate_experimental_live_target_surface(&invalid).is_err());
    }
}

#[test]
fn feature_capabilities_are_typed_and_default_empty() {
    let parsed: RpcCapabilities = serde_json::from_value(json!({"contract_version": "0.5.0"}))
        .expect("legacy capability payload must decode");
    assert!(parsed.feature_capabilities.is_empty());
    assert!(!parsed.supports_live_execution_identity_v1());

    let capability = FeatureCapability::live_execution_identity_v1();
    assert_eq!(capability.as_str(), LIVE_EXECUTION_IDENTITY_V1);
    let encoded = serde_json::to_value(capability).expect("capability must encode");
    assert_eq!(encoded, json!("live.execution_identity.v1"));

    let admitted: RpcCapabilities = serde_json::from_value(json!({
        "contract_version": "0.5.0",
        "feature_capabilities": ["live.execution_identity.v1"]
    }))
    .expect("typed feature capability must decode");
    assert!(admitted.supports_live_execution_identity_v1());

    let modes: RpcCapabilities = serde_json::from_value(json!({
        "contract_version": "0.5.0",
        "feature_capabilities": [
            "live.execution_identity.v1",
            "live.execution.function_bridge.v1"
        ]
    }))
    .expect("mode capabilities must decode");
    assert!(modes.supports_live_execution_mode(LiveExecutionMode::FunctionBridge));
    assert!(!modes.supports_live_execution_mode(LiveExecutionMode::ClientContext));
    assert_eq!(
        FeatureCapability::live_execution_function_bridge_v1().as_str(),
        LIVE_EXECUTION_FUNCTION_BRIDGE_V1
    );
    assert_eq!(
        FeatureCapability::live_execution_client_context_v1().as_str(),
        LIVE_EXECUTION_CLIENT_CONTEXT_V1
    );
}
