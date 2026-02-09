"""Conformance tests for generated Python SDK types."""

import json

from meerkat.generated.types import (
    CONTRACT_VERSION,
    CapabilitiesResponse,
    CapabilityEntry,
    WireEvent,
    WireRunResult,
    WireUsage,
)
from meerkat.generated.errors import (
    CapabilityUnavailableError,
    MeerkatError,
    SessionNotFoundError,
    SkillNotFoundError,
)


def test_contract_version():
    """Contract version should be 0.1.0."""
    assert CONTRACT_VERSION == "0.1.0"


def test_wire_usage_defaults():
    """WireUsage should have sensible defaults."""
    usage = WireUsage()
    assert usage.input_tokens == 0
    assert usage.output_tokens == 0
    assert usage.total_tokens == 0
    assert usage.cache_creation_tokens is None
    assert usage.cache_read_tokens is None


def test_wire_run_result_defaults():
    """WireRunResult should have sensible defaults."""
    result = WireRunResult()
    assert result.session_id == ""
    assert result.text == ""
    assert result.turns == 0
    assert result.tool_calls == 0
    assert result.usage is None


def test_wire_event_defaults():
    """WireEvent should have sensible defaults."""
    event = WireEvent()
    assert event.session_id == ""
    assert event.sequence == 0
    assert event.event is None


def test_capabilities_response():
    """CapabilitiesResponse should work with entries."""
    entry = CapabilityEntry(id="sessions", description="Session lifecycle", status="available")
    response = CapabilitiesResponse(contract_version="0.1.0", capabilities=[entry])
    assert response.contract_version == "0.1.0"
    assert len(response.capabilities) == 1
    assert response.capabilities[0].id == "sessions"


def test_error_hierarchy():
    """Error types should inherit from MeerkatError."""
    assert issubclass(CapabilityUnavailableError, MeerkatError)
    assert issubclass(SessionNotFoundError, MeerkatError)
    assert issubclass(SkillNotFoundError, MeerkatError)


def test_error_fields():
    """MeerkatError should capture code and message."""
    err = MeerkatError("TEST_CODE", "test message", details={"key": "value"})
    assert err.code == "TEST_CODE"
    assert err.message == "test message"
    assert err.details == {"key": "value"}
    assert str(err) == "test message"


def test_version_compatibility():
    """Version format should be semver-compatible."""
    parts = CONTRACT_VERSION.split(".")
    assert len(parts) == 3
    for part in parts:
        int(part)  # Should not raise
