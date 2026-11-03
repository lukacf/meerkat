"""Gating for the real-API end-to-end lane.

This directory is SKIPPED unless ``META_E2E_LIVE=1``, so the ordinary
``make test-python`` run stays hermetic. When the lane IS requested, every
missing precondition is a FAILURE, never a skip: a live lane that silently
skips on a missing key or binary is a green check that gates nothing, which is
exactly the failure mode this lane exists to catch one layer down.

Preconditions when requested:
  - ``ANTHROPIC_API_KEY`` (or ``RKAT_ANTHROPIC_API_KEY``) in the environment
  - the ``rpc_gateway`` binary at ``META_GATEWAY_BIN`` (``make e2e-live``
    builds it and sets this)
"""
from __future__ import annotations

import os

import pytest

LIVE_FLAG = "META_E2E_LIVE"


def _requested() -> bool:
    return os.environ.get(LIVE_FLAG, "").strip() == "1"


def anthropic_key() -> str:
    return os.environ.get("RKAT_ANTHROPIC_API_KEY") or os.environ.get("ANTHROPIC_API_KEY") or ""


def gateway_bin() -> str:
    return os.environ.get("META_GATEWAY_BIN", "").strip()


def pytest_collection_modifyitems(config, items):
    if _requested():
        return
    skip = pytest.mark.skip(reason=f"real-API lane not requested; set {LIVE_FLAG}=1 (make e2e-live)")
    for item in items:
        if "e2e_live" in str(item.fspath):
            item.add_marker(skip)


@pytest.fixture(scope="session")
def live_preconditions() -> dict[str, str]:
    """Fail loudly on any missing precondition once the lane is requested."""
    missing = []
    if not anthropic_key():
        missing.append("ANTHROPIC_API_KEY (or RKAT_ANTHROPIC_API_KEY) is not set")
    binary = gateway_bin()
    if not binary or not os.path.isfile(binary) or not os.access(binary, os.X_OK):
        missing.append(
            f"META_GATEWAY_BIN does not name an executable rpc_gateway (got {binary!r}); "
            "run `make e2e-live`, which builds and exports it"
        )
    if missing:
        pytest.fail("real-API lane requested but cannot run:\n  - " + "\n  - ".join(missing), pytrace=False)
    return {"gateway_bin": binary}
