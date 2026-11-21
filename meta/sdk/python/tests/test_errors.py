"""Tests for the Meta error hierarchy."""
import pytest

from meerkat_meta.errors import (
    CapabilityUnavailableError,
    ConsoleTimelineReplayUnavailableError,
    ContractMismatchError,
    MetaError,
    NotConnectedError,
    RpcError,
    TransportError,
)


class TestMetaErrorHierarchy:
    def test_base_is_exception(self):
        assert issubclass(MetaError, Exception)

    @pytest.mark.parametrize(
        "cls",
        [
            TransportError,
            RpcError,
            CapabilityUnavailableError,
            ConsoleTimelineReplayUnavailableError,
            ContractMismatchError,
            NotConnectedError,
        ],
    )
    def test_subclasses(self, cls):
        assert issubclass(cls, MetaError)


def test_contract_mismatch_message_only_and_optional_recovery_context():
    assert ContractMismatchError().args == ()
    assert ContractMismatchError("first", "second").args == ("first", "second")
    plain = ContractMismatchError("version mismatch")
    assert str(plain) == "version mismatch"
    assert plain.args == ("version mismatch",)
    assert plain.returned_item_id is None
    assert plain.returned_item_revision is None
    contextual = ContractMismatchError("target mismatch", returned_item_id="work_1", returned_item_revision=3)
    assert isinstance(contextual, MetaError)
    assert contextual.args == ("target mismatch",)
    assert contextual.returned_item_id == "work_1"
    assert contextual.returned_item_revision == 3


class TestRpcError:
    def test_attributes(self):
        err = RpcError(42, "bad request", request_id="req-1", method="status")
        assert err.code == 42
        assert err.request_id == "req-1"
        assert err.method == "status"

    def test_message_via_str(self):
        err = RpcError(1, "something broke")
        assert str(err) == "something broke"
