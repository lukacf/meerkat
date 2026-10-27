import json
from pathlib import Path

import pytest

from meerkat_meta.live import (
    LIVE_EXECUTION_CLIENT_CONTEXT_V1,
    LIVE_EXECUTION_FUNCTION_BRIDGE_V1,
    LIVE_EXECUTION_IDENTITY_V1,
    OPENAI_GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID,
    ActiveLiveChannelHandle,
    ExperimentalLiveChannelStatus,
    ExperimentalLiveExecutionProfileConfig,
    ExperimentalLiveGatewayConfig,
    LiveAssistantOutputAddress,
    LiveAuthBindingRef,
    LiveChannelHandle,
    LiveExecutionIdentityV1,
    LivePlaybackCompleteResult,
    LivePlaybackOwnerReadiness,
    PendingLiveChannelHandle,
    LiveReplacementRequired,
    OpenAiLiveGatewayConfig,
    OpenAiLiveSummaryConfig,
    live_open_execution_identity_params,
    live_execution_mode_capability,
    supports_live_execution_mode,
    supports_live_execution_identity_v1,
)
from meerkat_meta.types import CapabilitiesResult


@pytest.fixture(scope="module")
def contracts_fixture():
    path = Path(__file__).parents[3] / "crates/meerkat-meta/tests/fixtures/live_contracts_v1.json"
    return json.loads(path.read_text(encoding="utf-8"))


def test_openai_live_gateway_registration_matches_shared_fixture(contracts_fixture):
    config = OpenAiLiveGatewayConfig(
        principal="user:example",
        realm="example",
        auth_binding=LiveAuthBindingRef(
            realm="example", binding="openai-api-key", profile="example"
        ),
        voice="marin",
        session_instructions="You are a test voice agent.",
    )
    assert config.to_dict() == contracts_fixture["openai_live_gateway_config"]
    assert (
        OPENAI_GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID
        == contracts_fixture["openai_live_public_profile_id"]
    )
    minimal = OpenAiLiveGatewayConfig(
        principal="user:example",
        realm="example",
        auth_binding=LiveAuthBindingRef(realm="example", binding="openai-api-key"),
        voice="marin",
    )
    assert minimal.to_dict() == {
        "principal": "user:example",
        "realm": "example",
        "auth_binding": {"realm": "example", "binding": "openai-api-key"},
        "voice": "marin",
    }


def test_openai_live_gateway_registration_rejects_drift():
    with pytest.raises(ValueError, match="realm must equal"):
        OpenAiLiveGatewayConfig(
            principal="user:example",
            realm="example",
            auth_binding=LiveAuthBindingRef(realm="other", binding="openai-api-key"),
            voice="marin",
        ).to_dict()
    with pytest.raises(ValueError, match="voice"):
        OpenAiLiveGatewayConfig(
            principal="user:example",
            realm="example",
            auth_binding=LiveAuthBindingRef(realm="example", binding="openai-api-key"),
            voice=" ",
        ).to_dict()
    with pytest.raises(ValueError, match="session_instructions"):
        OpenAiLiveGatewayConfig(
            principal="user:example",
            realm="example",
            auth_binding=LiveAuthBindingRef(realm="example", binding="openai-api-key"),
            voice="marin",
            session_instructions=" ",
        ).to_dict()
    with pytest.raises(TypeError):
        OpenAiLiveGatewayConfig(
            principal="user:example",
            realm="example",
            auth_binding={"realm": "example", "binding": "openai-api-key"},
            voice="marin",
        ).to_dict()
    for field in ["model", "provider", "factory_kind", "gate0_qualification"]:
        with pytest.raises(TypeError):
            OpenAiLiveGatewayConfig(
                principal="user:example",
                realm="example",
                auth_binding=LiveAuthBindingRef(realm="example", binding="openai-api-key"),
                voice="marin",
                **{field: "forbidden"},
            )


def test_experimental_live_gateway_registration_is_explicit_and_strict(
    contracts_fixture,
):
    config = ExperimentalLiveGatewayConfig(
        principal="user:example",
        realm="example",
        factory_kind="openai-gpt-live",
        factory_version="v1",
        gate0_qualification="gate0-v1",
        auth_binding=LiveAuthBindingRef(
            realm="example", binding="chatgpt-oauth", profile="example"
        ),
        voice="marin",
        execution_profiles=(
            ExperimentalLiveExecutionProfileConfig(
                profile_id="example.device.open-room.v1",
                session_instructions="You are a test voice agent.",
            ),
        ),
    )
    assert config.to_dict() == contracts_fixture["experimental_gateway_config"]
    with pytest.raises(ValueError, match="realm must equal"):
        ExperimentalLiveGatewayConfig(
            principal="user:example",
            realm="example",
            factory_kind="openai-gpt-live",
            factory_version="v1",
            gate0_qualification="gate0-v1",
            auth_binding=LiveAuthBindingRef(
                realm="other", binding="chatgpt-oauth"
            ),
            voice="marin",
        ).to_dict()


def test_experimental_live_execution_profiles_reject_authority_and_drift():
    def config_with(*profiles):
        return ExperimentalLiveGatewayConfig(
            principal="user:example",
            realm="example",
            factory_kind="openai-gpt-live",
            factory_version="v1",
            gate0_qualification="gate0-v1",
            auth_binding=LiveAuthBindingRef(
                realm="example", binding="chatgpt-oauth"
            ),
            voice="marin",
            execution_profiles=profiles,
        )

    for profile in [
        ExperimentalLiveExecutionProfileConfig(
            profile_id=" ", session_instructions="voice"
        ),
        ExperimentalLiveExecutionProfileConfig(
            profile_id="robot", session_instructions=" "
        ),
        *[
            ExperimentalLiveExecutionProfileConfig(
                profile_id=profile_id,
                session_instructions="override canonical policy",
            )
            for profile_id in [
                "openai.gpt-live-1-codex.client-context.v1",
                "openai.gpt-live-1-codex.function-bridge.v1",
                OPENAI_GPT_LIVE_PUBLIC_CLIENT_CONTEXT_PROFILE_ID,
            ]
        ],
    ]:
        with pytest.raises(ValueError):
            config_with(profile).to_dict()

    duplicate = ExperimentalLiveExecutionProfileConfig(
        profile_id="robot", session_instructions="voice"
    )
    with pytest.raises(ValueError, match="duplicate"):
        config_with(
            duplicate,
            ExperimentalLiveExecutionProfileConfig(
                profile_id=" robot ", session_instructions="other"
            ),
        ).to_dict()
    with pytest.raises(TypeError, match="entries"):
        config_with({"profile_id": "robot"}).to_dict()
    for field in ["mode", "model", "provider", "tools", "responses", "capabilities"]:
        with pytest.raises(TypeError):
            ExperimentalLiveExecutionProfileConfig(
                profile_id="robot",
                session_instructions="voice",
                **{field: "forbidden"},
            )


def test_execution_identity_matches_shared_profile_fixture(contracts_fixture):
    selected = LiveExecutionIdentityV1(
        profile_id="example.device.open-room.v1",
    )
    assert selected.to_dict() == contracts_fixture["execution_identity"]
    assert LiveExecutionIdentityV1.from_dict(selected.to_dict()) == selected


def test_execution_identity_rejects_unknown_fields_null_and_legacy_conflicts():
    with pytest.raises(ValueError, match="unknown field"):
        LiveExecutionIdentityV1.from_dict(
            {"version": "v1", "model": "gpt-live-1-codex", "extra": True}
        )
    with pytest.raises(ValueError, match="profile_id must be a non-empty string"):
        LiveExecutionIdentityV1(profile_id=" ").to_dict()
    for field, value in [
        ("model", "legacy"),
        ("provider", "openai"),
        ("auth_binding", {"realm": "example", "binding": "other"}),
        ("self_hosted_server_id", "server"),
        ("provider_params", {}),
    ]:
        with pytest.raises(ValueError, match=f"legacy top-level {field}"):
            live_open_execution_identity_params(
                LiveExecutionIdentityV1(
                    profile_id="example.device.open-room.v1"
                ),
                **{field: value},
            )
    with pytest.raises(ValueError, match="must be v1"):
        LiveExecutionIdentityV1.from_dict(
            {"version": "v2", "profile_id": "example.device.open-room.v1"}
        )
    with pytest.raises(ValueError, match="must be v1"):
        LiveExecutionIdentityV1.from_dict(
            {"profile_id": "example.device.open-room.v1"}
        )
    for field, value in [
        ("model", "gpt-live-1-codex"),
        ("provider", "openai"),
        ("self_hosted_server_id", "server"),
        ("auth_binding", {"action": "clear"}),
    ]:
        with pytest.raises(ValueError):
            LiveExecutionIdentityV1.from_dict(
                {
                    "version": "v1",
                    "profile_id": "example.device.open-room.v1",
                    field: value,
                }
            )


def test_capability_is_off_when_absent():
    old_gateway = CapabilitiesResult.from_dict(
        {"contract_version": "0.5.0", "methods": [], "loaded_modules": []}
    )
    assert old_gateway.feature_capabilities == []
    assert not supports_live_execution_identity_v1(old_gateway.feature_capabilities)
    assert supports_live_execution_identity_v1([LIVE_EXECUTION_IDENTITY_V1])


def test_channel_handle_matches_shared_fixture(contracts_fixture):
    wire = contracts_fixture["channel_handle"]
    handle = LiveChannelHandle.from_dict(wire)
    assert handle.channel_id == "live-ch-1"
    assert handle.target_identity == "identity:example"
    assert handle.transport.transport == "websocket"
    assert handle.continuity.mode == "transcript_only"
    assert handle.to_dict() == wire


def test_handles_carry_open_advisories_and_omit_them_when_empty(contracts_fixture):
    # What a live open went ahead without (a refused staged create-time
    # prompt) rides both handles; an advisory this SDK does not know is kept
    # as the server sent it, and no advisories leave the wire unchanged.
    expected = ("staged_prompt_refused", "a_future_advisory")
    for key, parser in [
        ("channel_handle_with_advisories", LiveChannelHandle.from_dict),
        ("pending_channel_handle_with_advisories", PendingLiveChannelHandle.from_dict),
    ]:
        wire = contracts_fixture[key]
        handle = parser(wire)
        assert handle.advisories == expected
        assert handle.to_dict() == wire

    for key, parser in [
        ("channel_handle", LiveChannelHandle.from_dict),
        ("pending_channel_handle", PendingLiveChannelHandle.from_dict),
    ]:
        wire = contracts_fixture[key]
        handle = parser(wire)
        assert handle.advisories == ()
        assert "advisories" not in handle.to_dict()
        assert parser({**wire, "advisories": []}).advisories == ()
        for malformed in ["staged_prompt_refused", [1], [""], None]:
            with pytest.raises(ValueError):
                parser({**wire, "advisories": malformed})


def test_pending_active_and_readiness_handles_are_distinct_and_strict(
    contracts_fixture,
):
    pending = PendingLiveChannelHandle.from_dict(
        contracts_fixture["pending_channel_handle"]
    )
    active = ActiveLiveChannelHandle.from_dict(
        contracts_fixture["active_channel_handle"]
    )
    readiness = LivePlaybackOwnerReadiness.from_dict(
        contracts_fixture["playback_owner_readiness"]
    )
    assert pending.execution_mode == "client_context"
    assert active.channel_id == pending.channel_id == readiness.channel_id
    assert active.activation_receipt != pending.pending_receipt
    assert pending.to_dict() == contracts_fixture["pending_channel_handle"]
    assert active.to_dict() == contracts_fixture["active_channel_handle"]
    assert readiness.to_dict() == contracts_fixture["playback_owner_readiness"]
    assert ExperimentalLiveChannelStatus.from_dict(
        {"phase": "active", "handle": active.to_dict()}
    ).handle == active
    for key in [
        "pending_status",
        "active_status",
        "revoked_status",
        "closed_status",
    ]:
        assert (
            ExperimentalLiveChannelStatus.from_dict(contracts_fixture[key]).to_dict()
            == contracts_fixture[key]
        )

    for drifted in [
        {**active.to_dict(), "execution_mode": "responses"},
        {**active.to_dict(), "responses_model": "gpt-5.5"},
        {**pending.to_dict(), "tools": []},
        {**active.to_dict(), "activation_receipt": "  "},
    ]:
        parser = (
            PendingLiveChannelHandle.from_dict
            if "pending_receipt" in drifted
            else ActiveLiveChannelHandle.from_dict
        )
        with pytest.raises(ValueError):
            parser(drifted)


def test_provider_neutral_mode_capabilities_are_independent():
    advertised = [LIVE_EXECUTION_IDENTITY_V1, LIVE_EXECUTION_FUNCTION_BRIDGE_V1]
    assert supports_live_execution_mode(advertised, "function_bridge")
    assert not supports_live_execution_mode(advertised, "client_context")
    assert (
        live_execution_mode_capability("client_context")
        == LIVE_EXECUTION_CLIENT_CONTEXT_V1
    )
    with pytest.raises(ValueError):
        live_execution_mode_capability("responses")  # type: ignore[arg-type]


def test_replacement_required_is_strict_and_preserves_fresh_handle(contracts_fixture):
    wire = {
        "required": True,
        "reason": "canonical_context",
        "replacement": contracts_fixture["channel_handle"],
        "canonical_seed_cursor": 17,
    }
    result = LiveReplacementRequired.from_dict(wire)
    assert result.required is True
    assert result.replacement is not None
    assert result.replacement.channel_id == "live-ch-1"
    assert result.to_dict() == wire
    assert LiveReplacementRequired.from_dict({"required": False}).to_dict() == {
        "required": False
    }
    with pytest.raises(ValueError, match="unknown field"):
        LiveReplacementRequired.from_dict(
            {"required": False, "channel_id": "stale-old-channel"}
        )


def test_playback_complete_result_is_strict():
    assert LivePlaybackCompleteResult.from_dict(
        {"status": "completed"}
    ).to_dict() == {"status": "completed"}
    with pytest.raises(ValueError):
        LivePlaybackCompleteResult.from_dict(
            {"status": "completed", "interaction_id": "caller-minted"}
        )


def test_assistant_output_address_is_strict_and_opaque():
    wire = {
        "channel_id": "live-ch-1",
        "output_id": "opaque-output-1",
        "content_index": 0,
    }
    assert LiveAssistantOutputAddress.from_dict(wire).to_dict() == wire
    with pytest.raises(ValueError, match="unknown field"):
        LiveAssistantOutputAddress.from_dict({**wire, "item_id": "provider-item"})
    with pytest.raises(ValueError, match="non-negative integer"):
        LiveAssistantOutputAddress.from_dict({**wire, "content_index": -1})


def test_openai_live_gateway_summary_bounds_match_shared_fixture(contracts_fixture):
    config = OpenAiLiveGatewayConfig(
        principal="user:example",
        realm="example",
        auth_binding=LiveAuthBindingRef(
            realm="example", binding="openai-api-key", profile="example"
        ),
        voice="marin",
        session_instructions="You are a test voice agent.",
        summary=OpenAiLiveSummaryConfig(
            model="gpt-5.4-mini", max_input_bytes=32768, max_output_bytes=2048
        ),
    )
    assert config.to_dict() == contracts_fixture["openai_live_gateway_config_with_summary"]
    partial = OpenAiLiveGatewayConfig(
        principal="user:example",
        realm="example",
        auth_binding=LiveAuthBindingRef(realm="example", binding="openai-api-key"),
        voice="marin",
        summary=OpenAiLiveSummaryConfig(max_output_bytes=1024),
    )
    assert partial.to_dict()["summary"] == {"max_output_bytes": 1024}
    assert "summary" not in OpenAiLiveGatewayConfig(
        principal="user:example",
        realm="example",
        auth_binding=LiveAuthBindingRef(realm="example", binding="openai-api-key"),
        voice="marin",
    ).to_dict()
    for invalid in [
        OpenAiLiveSummaryConfig(model=" "),
        OpenAiLiveSummaryConfig(max_input_bytes=0),
        OpenAiLiveSummaryConfig(max_output_bytes=-1),
        OpenAiLiveSummaryConfig(max_output_bytes=True),
        OpenAiLiveSummaryConfig(max_input_bytes="65536"),  # type: ignore[arg-type]
    ]:
        with pytest.raises(ValueError):
            OpenAiLiveGatewayConfig(
                principal="user:example",
                realm="example",
                auth_binding=LiveAuthBindingRef(realm="example", binding="openai-api-key"),
                voice="marin",
                summary=invalid,
            ).to_dict()
    with pytest.raises(TypeError):
        OpenAiLiveGatewayConfig(
            principal="user:example",
            realm="example",
            auth_binding=LiveAuthBindingRef(realm="example", binding="openai-api-key"),
            voice="marin",
            summary={"max_output_bytes": 1024},  # type: ignore[arg-type]
        ).to_dict()
