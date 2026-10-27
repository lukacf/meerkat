"""TDD tests for identity-first models: DurableAgentSpec, DispatchInput, ContentBlock (REQ-40, REQ-43)."""
import pytest

from meerkat_meta.identity_first_models import (
    ContentBlock,
    DispatchInput,
    DurableAgentSpec,
    IdentityBootstrapCounts,
    IdentityBootstrapEntry,
    IdentityBootstrapMode,
    IdentityBootstrapState,
    IdentityBootstrapStatus,
    IdentityStatus,
    ImageBlock,
    ManagedPeerEdge,
    TextBlock,
)


class TestIdentityBootstrapModels:
    @staticmethod
    def _status_payload(**overrides):
        payload = {
            "mode": {"mode": "lazy_materialize"},
            "complete": False,
            "ready": False,
            "counts": {},
            "identities": {},
        }
        payload.update(overrides)
        return payload

    def test_background_mode_round_trip(self):
        mode = IdentityBootstrapMode.lazy_with_background_warm(concurrency=2)

        assert IdentityBootstrapMode.from_dict(mode.to_dict()) == mode

    def test_mode_parser_rejects_unknown_fields(self):
        with pytest.raises(ValueError, match="unsupported field"):
            IdentityBootstrapMode.from_dict(
                {"mode": "lazy_materialize", "concurrency_hint": 2}
            )

    def test_status_parser_types_full_wait_response(self):
        status = IdentityBootstrapStatus.from_dict(
            {
                "mode": {
                    "mode": "lazy_with_background_warm",
                    "concurrency": 2,
                },
                "complete": True,
                "ready": False,
                "error": "one or more identities failed to warm",
                "counts": {
                    "dormant": 0,
                    "warming": 0,
                    "active": 1,
                    "broken": 1,
                },
                "identities": {
                    "agent:active": {"state": "active"},
                    "agent:broken": {
                        "state": "broken",
                        "error": "resume failed",
                    },
                },
                "timed_out": True,
                "target": "startup_ready",
                "startup_ready": False,
            }
        )

        assert status.mode == IdentityBootstrapMode.lazy_with_background_warm(
            concurrency=2
        )
        assert status.counts == IdentityBootstrapCounts(active=1, broken=1)
        assert status.identities["agent:active"] == (
            IdentityBootstrapEntry(
                identity="agent:active",
                state=IdentityBootstrapState.ACTIVE,
            )
        )
        assert status.identities["agent:broken"].error == "resume failed"
        assert status.error == "one or more identities failed to warm"
        assert status.timed_out is True
        assert status.target == "startup_ready"
        assert status.startup_ready is False
        assert status.to_dict()["error"] == "one or more identities failed to warm"

    def test_status_parser_reads_typed_restore_progress(self):
        from meerkat_meta import IdentityRestoreProgress

        payload = self._status_payload(
            identities={
                "agent:resuming": {"state": "warming", "restore": {"stage": "resuming"}},
                "agent:broken": {
                    "state": "broken",
                    "error": "resume refused",
                    "restore": {
                        "stage": "refused",
                        "code": -32051,
                        "kind": "identity_unresumable",
                    },
                },
                "agent:older": {"state": "active"},
            }
        )
        status = IdentityBootstrapStatus.from_dict(payload)

        assert status.identities["agent:resuming"].restore == IdentityRestoreProgress(
            stage="resuming"
        )
        assert status.identities["agent:broken"].restore == IdentityRestoreProgress(
            stage="refused", code=-32051, kind="identity_unresumable"
        )
        assert status.identities["agent:older"].restore is None
        assert status.identities["agent:broken"].to_dict()["restore"] == {
            "stage": "refused",
            "code": -32051,
            "kind": "identity_unresumable",
        }

    def test_status_parser_preserves_unknown_state_as_typed_unknown(self):
        status = IdentityBootstrapStatus.from_dict(
            {
                "mode": {"mode": "lazy_materialize"},
                "complete": False,
                "ready": False,
                "counts": {},
                "identities": {"agent:new": {"state": "future_state"}},
            }
        )

        assert (
            status.identities["agent:new"].state
            is IdentityBootstrapState.UNKNOWN
        )

    @pytest.mark.parametrize("entry", [{}, {"state": None}, {"state": 1}])
    def test_status_parser_rejects_missing_or_non_string_entry_state(self, entry):
        payload = self._status_payload(identities={"agent:bad": entry})

        with pytest.raises((TypeError, ValueError), match="state"):
            IdentityBootstrapStatus.from_dict(payload)

    def test_status_parser_rejects_non_string_entry_error(self):
        payload = self._status_payload(
            identities={"agent:bad": {"state": "broken", "error": 503}}
        )

        with pytest.raises(TypeError, match="error"):
            IdentityBootstrapStatus.from_dict(payload)

    @pytest.mark.parametrize(
        ("field", "value"),
        [
            ("complete", "false"),
            ("ready", 0),
            ("timed_out", "false"),
            ("startup_ready", 1),
        ],
    )
    def test_status_parser_rejects_non_boolean_flags(self, field, value):
        payload = self._status_payload(**{field: value})

        with pytest.raises(TypeError, match="boolean"):
            IdentityBootstrapStatus.from_dict(payload)

    @pytest.mark.parametrize("field", ["complete", "ready"])
    def test_status_parser_requires_core_boolean_flags(self, field):
        payload = self._status_payload()
        del payload[field]

        with pytest.raises(ValueError, match=field):
            IdentityBootstrapStatus.from_dict(payload)

    @pytest.mark.parametrize("value", [True, "1", 1.5])
    def test_status_parser_rejects_non_integer_counts(self, value):
        payload = self._status_payload(counts={"active": value})

        with pytest.raises(TypeError, match="integer"):
            IdentityBootstrapStatus.from_dict(payload)

    def test_status_parser_rejects_negative_counts(self):
        payload = self._status_payload(counts={"broken": -1})

        with pytest.raises(ValueError, match="non-negative"):
            IdentityBootstrapStatus.from_dict(payload)

    def test_status_parser_rejects_non_string_target(self):
        payload = self._status_payload(target=42)

        with pytest.raises(TypeError, match="target"):
            IdentityBootstrapStatus.from_dict(payload)

    def test_status_parser_rejects_non_string_pass_error(self):
        payload = self._status_payload(error={"message": "failed"})

        with pytest.raises(TypeError, match="error"):
            IdentityBootstrapStatus.from_dict(payload)


class TestDurableAgentSpec:
    """REQ-40: DurableAgentSpec dataclass with all fields, from_dict/to_dict."""

    def test_all_fields(self):
        spec = DurableAgentSpec(
            identity="triage:main",
            profile="assistant",
            addressability="addressable",
            display_name="Triage Agent",
            labels={"env": "prod"},
            context={"key": "value"},
            additional_instructions=["Be concise."],
            placement="12D3KooWExactRemoteHost",
        )
        assert spec.identity == "triage:main"
        assert spec.profile == "assistant"
        assert spec.addressability == "addressable"
        assert spec.display_name == "Triage Agent"
        assert spec.labels == {"env": "prod"}
        assert spec.context == {"key": "value"}
        assert spec.additional_instructions == ["Be concise."]
        assert spec.placement == "12D3KooWExactRemoteHost"

    def test_addressability_defaults_to_addressable(self):
        spec = DurableAgentSpec(identity="a:main", profile="default")
        assert spec.addressability == "addressable"

    def test_optional_fields_default(self):
        spec = DurableAgentSpec(identity="a:main", profile="default")
        assert spec.display_name is None
        assert spec.labels == {}
        assert spec.context is None
        assert spec.additional_instructions == []
        assert spec.placement is None

    def test_to_dict(self):
        spec = DurableAgentSpec(
            identity="triage:main",
            profile="assistant",
            display_name="Triage",
            labels={"env": "prod"},
            context={"k": "v"},
            additional_instructions=["inst1"],
            placement="12D3KooWExactRemoteHost",
        )
        d = spec.to_dict()
        assert d["identity"] == "triage:main"
        assert d["profile"] == "assistant"
        assert d["addressability"] == "addressable"
        assert d["display_name"] == "Triage"
        assert d["labels"] == {"env": "prod"}
        assert d["context"] == {"k": "v"}
        assert d["additional_instructions"] == ["inst1"]
        assert d["placement"] == "12D3KooWExactRemoteHost"

    def test_to_dict_omits_none_optional_fields(self):
        spec = DurableAgentSpec(identity="a:main", profile="default")
        d = spec.to_dict()
        assert "display_name" not in d
        assert "context" not in d
        assert "placement" not in d
        # labels and additional_instructions are always present (empty)
        assert d["labels"] == {}
        assert d["additional_instructions"] == []

    def test_from_dict_full(self):
        data = {
            "identity": "gate:main",
            "profile": "gatekeeper",
            "addressability": "internal_only",
            "display_name": "Gate",
            "labels": {"role": "gate"},
            "context": {"priority": 1},
            "additional_instructions": ["Check all."],
            "placement": "12D3KooWExactRemoteHost",
        }
        spec = DurableAgentSpec.from_dict(data)
        assert spec.identity == "gate:main"
        assert spec.profile == "gatekeeper"
        assert spec.addressability == "internal_only"
        assert spec.display_name == "Gate"
        assert spec.labels == {"role": "gate"}
        assert spec.context == {"priority": 1}
        assert spec.additional_instructions == ["Check all."]
        assert spec.placement == "12D3KooWExactRemoteHost"

    def test_from_dict_minimal(self):
        data = {"identity": "a:main", "profile": "default"}
        spec = DurableAgentSpec.from_dict(data)
        assert spec.identity == "a:main"
        assert spec.profile == "default"
        assert spec.addressability == "addressable"
        assert spec.display_name is None
        assert spec.labels == {}
        assert spec.context is None
        assert spec.additional_instructions == []
        assert spec.placement is None

    def test_round_trip(self):
        original = DurableAgentSpec(
            identity="triage:main",
            profile="assistant",
            addressability="internal_only",
            display_name="Triage",
            labels={"env": "test"},
            context={"nested": {"deep": True}},
            additional_instructions=["a", "b"],
            placement="12D3KooWExactRemoteHost",
        )
        restored = DurableAgentSpec.from_dict(original.to_dict())
        assert restored.identity == original.identity
        assert restored.profile == original.profile
        assert restored.addressability == original.addressability
        assert restored.display_name == original.display_name
        assert restored.labels == original.labels
        assert restored.context == original.context
        assert restored.additional_instructions == original.additional_instructions
        assert restored.placement == original.placement


class TestDurableAgentSpecRuntimeFields:
    """`runtime_mode_override` and `initial_message` reach the wire the way the
    Rust `DurableAgentSpec` deserializes them: `MobRuntimeMode` snake_case and
    an untagged `ContentInput` (text or block list). Unset means absent."""

    def test_unset_fields_default_to_none_and_stay_off_the_wire(self):
        spec = DurableAgentSpec(identity="a:main", profile="default")
        assert spec.runtime_mode_override is None
        assert spec.initial_message is None
        d = spec.to_dict()
        assert "runtime_mode_override" not in d
        assert "initial_message" not in d

    @pytest.mark.parametrize("mode", ["autonomous_host", "turn_driven"])
    def test_runtime_mode_override_serializes_meerkat_vocabulary(self, mode):
        spec = DurableAgentSpec(
            identity="a:main", profile="default", runtime_mode_override=mode
        )
        assert spec.to_dict()["runtime_mode_override"] == mode

    def test_runtime_mode_override_rejects_unknown_mode(self):
        with pytest.raises(ValueError, match="runtime_mode_override"):
            DurableAgentSpec(
                identity="a:main", profile="default", runtime_mode_override="eager"
            )

    def test_initial_message_text_serializes_as_a_string(self):
        spec = DurableAgentSpec(
            identity="a:main", profile="default", initial_message="Start with the digest."
        )
        assert spec.to_dict()["initial_message"] == "Start with the digest."

    def test_initial_message_blocks_serialize_as_a_block_list(self):
        spec = DurableAgentSpec(
            identity="a:main",
            profile="default",
            initial_message=[
                TextBlock(text="Look at this."),
                ImageBlock(media_type="image/png", data="AAAA"),
            ],
        )
        assert spec.to_dict()["initial_message"] == [
            {"type": "text", "text": "Look at this."},
            {"type": "image", "media_type": "image/png", "source": "inline", "data": "AAAA"},
        ]

    def test_initial_message_rejects_non_content(self):
        with pytest.raises(TypeError, match="initial_message"):
            DurableAgentSpec(identity="a:main", profile="default", initial_message=42)

    def test_from_dict_reads_both_fields(self):
        spec = DurableAgentSpec.from_dict(
            {
                "identity": "a:main",
                "profile": "default",
                "runtime_mode_override": "turn_driven",
                "initial_message": [{"type": "text", "text": "hi"}],
            }
        )
        assert spec.runtime_mode_override == "turn_driven"
        assert spec.initial_message == [TextBlock(text="hi")]

    def test_round_trip_keeps_both_fields(self):
        original = DurableAgentSpec(
            identity="a:main",
            profile="default",
            runtime_mode_override="autonomous_host",
            initial_message="kick off",
        )
        restored = DurableAgentSpec.from_dict(original.to_dict())
        assert restored.runtime_mode_override == "autonomous_host"
        assert restored.initial_message == "kick off"


class TestContentBlock:
    """REQ-43: TextBlock and ImageBlock content blocks."""

    def test_text_block(self):
        block = TextBlock(text="Hello world")
        assert block.text == "Hello world"

    def test_text_block_to_dict(self):
        block = TextBlock(text="Hello")
        d = block.to_dict()
        assert d == {"type": "text", "text": "Hello"}

    def test_text_block_from_dict(self):
        block = ContentBlock.from_dict({"type": "text", "text": "Hi"})
        assert isinstance(block, TextBlock)
        assert block.text == "Hi"

    def test_image_block(self):
        block = ImageBlock(media_type="image/png", data="base64data")
        assert block.media_type == "image/png"
        assert block.data == "base64data"
        assert block.source == "inline"
        assert block.blob_id is None

    def test_image_block_to_dict(self):
        block = ImageBlock(media_type="image/jpeg", data="abc123")
        d = block.to_dict()
        assert d == {
            "type": "image",
            "media_type": "image/jpeg",
            "source": "inline",
            "data": "abc123",
        }

    def test_blob_image_block_to_dict(self):
        block = ImageBlock(
            media_type="image/png",
            source="blob",
            blob_id="sha256:" + "a" * 64,
        )
        d = block.to_dict()
        assert d == {
            "type": "image",
            "media_type": "image/png",
            "source": "blob",
            "blob_id": "sha256:" + "a" * 64,
        }

    def test_image_block_from_dict(self):
        block = ContentBlock.from_dict(
            {"type": "image", "media_type": "image/png", "data": "xyz"}
        )
        assert isinstance(block, ImageBlock)
        assert block.media_type == "image/png"
        assert block.data == "xyz"
        assert block.source == "inline"

    def test_blob_image_block_from_dict(self):
        block = ContentBlock.from_dict(
            {
                "type": "image",
                "media_type": "image/webp",
                "source": "blob",
                "blob_id": "sha256:" + "b" * 64,
            }
        )
        assert isinstance(block, ImageBlock)
        assert block.media_type == "image/webp"
        assert block.source == "blob"
        assert block.blob_id == "sha256:" + "b" * 64

    def test_unknown_image_source_raises(self):
        with pytest.raises(ValueError, match="image source"):
            ContentBlock.from_dict(
                {
                    "type": "image",
                    "media_type": "image/png",
                    "source": "remote_url",
                    "data": "abc",
                }
            )

    def test_unknown_type_raises(self):
        with pytest.raises(ValueError, match="unknown content block type"):
            ContentBlock.from_dict({"type": "video", "url": "..."})


class TestDispatchInput:
    """REQ-43: DispatchInput with typed content, validated origin, wire format."""

    def test_string_content(self):
        di = DispatchInput(content="Hello", origin="connector")
        assert di.content == "Hello"
        assert di.origin == "connector"

    def test_block_content(self):
        blocks = [TextBlock(text="Hi"), ImageBlock(media_type="image/png", data="abc")]
        di = DispatchInput(content=blocks, origin="scheduler")
        assert len(di.content) == 2

    def test_valid_origins(self):
        for origin in ("connector", "scheduler", "policy", "flow", "system"):
            di = DispatchInput(content="x", origin=origin)
            assert di.origin == origin

    def test_invalid_origin_raises(self):
        with pytest.raises(ValueError, match="origin"):
            DispatchInput(content="x", origin="invalid")

    def test_optional_fields_default_none(self):
        di = DispatchInput(content="x", origin="system")
        assert di.correlation_id is None
        assert di.idempotency_key is None

    def test_optional_fields_set(self):
        di = DispatchInput(
            content="x",
            origin="flow",
            correlation_id="corr-1",
            idempotency_key="idem-1",
        )
        assert di.correlation_id == "corr-1"
        assert di.idempotency_key == "idem-1"

    def test_to_dict_string_content(self):
        di = DispatchInput(content="Hello", origin="connector")
        d = di.to_dict()
        assert d["content"] == "Hello"
        assert d["origin"] == "connector"
        assert "correlation_id" not in d
        assert "idempotency_key" not in d

    def test_to_dict_block_content(self):
        blocks = [TextBlock(text="Hi"), ImageBlock(media_type="image/png", data="abc")]
        di = DispatchInput(content=blocks, origin="system")
        d = di.to_dict()
        assert d["content"] == [
            {"type": "text", "text": "Hi"},
            {
                "type": "image",
                "media_type": "image/png",
                "source": "inline",
                "data": "abc",
            },
        ]

    def test_to_dict_with_optional_fields(self):
        di = DispatchInput(
            content="x",
            origin="policy",
            correlation_id="c1",
            idempotency_key="k1",
        )
        d = di.to_dict()
        assert d["correlation_id"] == "c1"
        assert d["idempotency_key"] == "k1"

    def test_from_dict_string_content(self):
        di = DispatchInput.from_dict({
            "content": "Hello",
            "origin": "connector",
        })
        assert di.content == "Hello"
        assert di.origin == "connector"

    def test_from_dict_block_content(self):
        di = DispatchInput.from_dict({
            "content": [
                {"type": "text", "text": "Hi"},
                {"type": "image", "media_type": "image/png", "data": "abc"},
            ],
            "origin": "scheduler",
        })
        assert isinstance(di.content, list)
        assert len(di.content) == 2
        assert isinstance(di.content[0], TextBlock)
        assert isinstance(di.content[1], ImageBlock)

    def test_from_dict_with_optional_fields(self):
        di = DispatchInput.from_dict({
            "content": "x",
            "origin": "flow",
            "correlation_id": "c1",
            "idempotency_key": "k1",
        })
        assert di.correlation_id == "c1"
        assert di.idempotency_key == "k1"

    def test_round_trip(self):
        original = DispatchInput(
            content=[TextBlock(text="Hi")],
            origin="policy",
            correlation_id="c-1",
            idempotency_key="k-1",
        )
        restored = DispatchInput.from_dict(original.to_dict())
        assert restored.origin == original.origin
        assert restored.correlation_id == original.correlation_id
        assert restored.idempotency_key == original.idempotency_key
        assert isinstance(restored.content, list)
        assert len(restored.content) == 1
        assert isinstance(restored.content[0], TextBlock)
        assert restored.content[0].text == "Hi"


class TestManagedPeerEdge:
    """REQ-43a: ManagedPeerEdge dataclass."""

    def test_fields(self):
        edge = ManagedPeerEdge(a="agent:alpha", b="agent:beta")
        assert edge.a == "agent:alpha"
        assert edge.b == "agent:beta"

    def test_to_dict(self):
        edge = ManagedPeerEdge(a="a:main", b="b:main")
        assert edge.to_dict() == {"a": "a:main", "b": "b:main"}

    def test_from_dict(self):
        edge = ManagedPeerEdge.from_dict({"a": "x:1", "b": "y:2"})
        assert edge.a == "x:1"
        assert edge.b == "y:2"

    def test_round_trip(self):
        original = ManagedPeerEdge(a="p:1", b="q:2")
        restored = ManagedPeerEdge.from_dict(original.to_dict())
        assert restored.a == original.a
        assert restored.b == original.b


class TestCrossSdkGracefulParsing:
    """Python from_dict must degrade gracefully (like TS) on missing keys,
    not raise KeyError. Regression for the cross-SDK divergence."""

    def test_identity_status_defaults_missing_keys(self):
        status = IdentityStatus.from_dict({})
        assert status.identity == ""
        assert status.state == ""
        assert status.addressability == "addressable"
