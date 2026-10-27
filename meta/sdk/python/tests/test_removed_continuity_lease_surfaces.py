"""#306 (0.9.4): Meta hosts no continuity or leases, so neither does the SDK.

Core's HeadCanonical store pair is the one session authority, its mob machine
and journal are the one identity authority, and Core's realm lock and hosting
claim give host exclusivity. The SDK-hosted continuity store, lease provider
and scratch directory, their gateway callbacks and init flags, the continuity
repair trigger and the lease-loss error code were Meta's second copy of that
truth. They are gone. These guards keep them gone and pin that an old call
fails loudly with a typed error instead of being accepted and ignored.

The session store ``session_service`` used to accept, and the config helpers
that built one, are gone for the same reason: nothing ever read them, and
Core's own session store holds every session.
"""

from __future__ import annotations

import asyncio
import dataclasses
import importlib
import re
from pathlib import Path

import pytest

import meerkat_meta
import meerkat_meta.config
from meerkat_meta import errors
from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.builder import Meta, MetaBuilderConfig
from meerkat_meta.errors import RpcError
from meerkat_meta.identity_first_models import IdentityStatus
from meerkat_meta.runtime import MetaRuntime, _rpc_error_from_payload

PACKAGE_ROOT = Path(meerkat_meta.__file__).resolve().parent

# Whole-word identifiers of the removed surfaces. Docstrings and comments
# count: a mention left behind documents an API that no longer exists.
REMOVED_NAMES = (
    "continuity_store",
    "lease_provider",
    "scratch_dir",
    "has_continuity_store",
    "has_lease_provider",
    "register_continuity_store",
    "register_lease_provider",
    "request_continuity_repair",
    "LEASE_LOST_CODE",
    "LeaseLostError",
    "ContinuityRecord",
    "ContinuityFailure",
    "ContinuityResolveState",
    "SessionSnapshot",
    "ContinuityStoreProvider",
    "SessionOwnershipProvider",
    "LeaseProviderProtocol",
    "LeaseGrant",
    "LeaseAcquireResult",
    "LeaseRenewResult",
    "LeaseInfo",
    "ContinuityHealth",
    "continuity_health",
    "DurabilityPolicy",
    "JsonSessionStoreConfig",
    "BigQuerySessionStoreConfig",
)

REMOVED_CALLBACKS = (
    "callback/continuity_store/resolve_many",
    "callback/continuity_store/load_session_snapshot",
    "callback/continuity_store/resolve_record_by_session",
    "callback/continuity_store/session_owner",
    "callback/continuity_store/delete_session_snapshot_if_current_revision",
    "callback/continuity_store/save_session_snapshot",
    "callback/continuity_store/upsert_continuity_record",
    "callback/continuity_store/delete_continuity_record",
    "callback/lease_provider/acquire_leases",
    "callback/lease_provider/renew_leases",
    "callback/lease_provider/release_leases",
)


def _source_hits(pattern: re.Pattern[str]) -> list[str]:
    return [
        f"{path.relative_to(PACKAGE_ROOT)}:{lineno}: {line.strip()}"
        for path in sorted(PACKAGE_ROOT.rglob("*.py"))
        for lineno, line in enumerate(
            path.read_text(encoding="utf-8").splitlines(), start=1
        )
        if pattern.search(line)
    ]


@pytest.mark.parametrize("name", REMOVED_NAMES)
def test_package_source_never_names_a_removed_surface(name: str) -> None:
    assert _source_hits(re.compile(rf"\b{re.escape(name)}\b")) == []


def test_package_source_never_uses_the_retired_lease_lost_code() -> None:
    assert _source_hits(re.compile(r"\b32005\b")) == []


@pytest.mark.parametrize("option", ["continuity_store", "lease_provider", "scratch_dir"])
def test_builder_offers_no_continuity_or_lease_option(option: str) -> None:
    assert option not in {f.name for f in dataclasses.fields(MetaBuilderConfig)}
    # An old host's call fails at once, typed, before anything starts.
    with pytest.raises(AttributeError):
        getattr(Meta.builder(), option)


def test_runtime_has_no_continuity_repair_trigger() -> None:
    assert not hasattr(MetaRuntime, "request_continuity_repair")


@pytest.mark.parametrize("name", ["LEASE_LOST_CODE", "LeaseLostError"])
def test_lease_loss_error_surface_is_gone(name: str) -> None:
    assert not hasattr(meerkat_meta, name)
    assert not hasattr(errors, name)
    assert name not in meerkat_meta.__all__


def test_retired_lease_lost_code_maps_to_the_plain_rpc_error() -> None:
    err = _rpc_error_from_payload(
        {"code": -32005, "message": "lease lost"},
        request_id="meta/send:1",
        method="meta/send",
    )
    assert type(err) is RpcError
    assert err.code == -32005


@pytest.mark.parametrize("name", ["register_continuity_store", "register_lease_provider"])
def test_dispatcher_registers_no_continuity_or_lease_provider(name: str) -> None:
    assert not hasattr(CallbackDispatcher(), name)


@pytest.mark.parametrize("method", REMOVED_CALLBACKS)
def test_dispatcher_refuses_removed_provider_callbacks(method: str) -> None:
    # An older gateway that still calls a provider gets the dispatcher's
    # typed unknown-method refusal, never an empty answer.
    with pytest.raises(ValueError, match="unknown callback method"):
        asyncio.run(CallbackDispatcher().handle_callback(method, {}))


def test_session_service_takes_no_session_store() -> None:
    # The store was accepted and never read; an old call now fails at once.
    assert "session_store" not in {f.name for f in dataclasses.fields(MetaBuilderConfig)}
    with pytest.raises(TypeError):
        Meta.builder().session_service(object(), object())


def test_session_store_config_helpers_are_gone() -> None:
    # session_store.json() and session_store.bigquery() built a store
    # declaration that nothing ever sent.
    for module in (meerkat_meta, meerkat_meta.config):
        assert not hasattr(module, "session_store")
        assert "session_store" not in module.__all__
    with pytest.raises(ModuleNotFoundError):
        importlib.import_module("meerkat_meta.config.session_store")


def test_identity_first_init_params_carry_no_continuity_or_lease_flags() -> None:
    class Roster:
        async def roster(self, context):  # noqa: ARG002
            return []

    params = MetaRuntime(Meta.builder().roster(Roster())._config)._build_init_params()
    assert params["has_roster_provider"] is True
    assert not {"has_continuity_store", "has_lease_provider", "scratch_dir"} & params.keys()


def test_identity_status_has_no_lease_or_continuity_health() -> None:
    assert not {"lease", "continuity_health"} & {
        f.name for f in dataclasses.fields(IdentityStatus)
    }
    # A gateway that still sends them is read without them.
    status = IdentityStatus.from_dict(
        {
            "identity": "w",
            "state": "active",
            "lease": {"fencing_token": 3, "ttl_remaining_ms": 10, "healthy": True},
            "continuity_health": {
                "store_reachable": True,
                "durability_policy": {"kind": "sync_write_through"},
            },
        }
    )
    assert not {"lease", "continuity_health"} & status.to_dict().keys()
