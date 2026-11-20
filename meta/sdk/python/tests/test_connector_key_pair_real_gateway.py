"""The connector helper's key pair against a real gateway.

One ``rpc_gateway`` process and one long-term agent seated from a roster and
built through the self-hosted model route with no credential, so the row
needs no provider key. The model is the local chat-completions fake of the
cross-mob rows.

``DispatchInput.connector`` checks the pair where the input is built; the
gateway checks it again before anything is delivered. This row dispatches the
helper's keyed input through ``runtime.dispatch`` and reads the gateway's
admission answer. It then sends each half pair, which only the full
constructor can build, and reads the gateway's own refusal.

Run:
    META_GATEWAY_BIN=<path to an rpc_gateway> PYTHONPATH=sdk/python \
        python3 -m pytest sdk/python/tests/test_connector_key_pair_real_gateway.py -q -s
"""
from __future__ import annotations

import os
from pathlib import Path

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.errors import RpcError
from meerkat_meta.identity_first_models import (
    DispatchInput,
    DispatchResult,
    DurableAgentSpec,
)

from .test_wire_cross_mob_real_gateways import (
    _GATEWAY_BIN,
    _FakeModel,
    _config_toml,
    _mob_toml,
)

pytestmark = pytest.mark.skipif(
    not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN)),
    reason="META_GATEWAY_BIN not set to a built rpc_gateway",
)

_MOB_ID = "connector-key-pair"
_IDENTITY = "domain:intake"
_CORRELATION = "7c2d9a4b-3e18-4b7a-8f55-1a9e0c6b2d10"
_KEY = "source-event-42"

# Each half pair and the gateway's words for it.
_HALF_PAIRS = [
    ({"correlation_id": _CORRELATION}, "correlation id without an idempotency key (half-pair)"),
    ({"idempotency_key": _KEY}, "idempotency key without a correlation id (half-pair)"),
]


class _Roster:
    async def roster(self, context) -> list[DurableAgentSpec]:
        del context
        return [
            DurableAgentSpec(
                identity=_IDENTITY, profile="worker", addressability="addressable"
            )
        ]


async def _gateway(tmp_path: Path, base_url: str):
    root = tmp_path / _MOB_ID
    root.mkdir()
    (root / "mob.toml").write_text(_mob_toml(_MOB_ID))
    (root / "config.toml").write_text(_config_toml(base_url))
    return await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(str(root / "mob.toml"))
        .meerkat_config(root / "config.toml")
        .persistent_state(str(root / "state"))
        .roster(_Roster())
        .build()
    )


@pytest.mark.asyncio
@pytest.mark.timeout(120)
async def test_the_gateway_admits_the_helpers_pair_and_refuses_each_half(tmp_path):
    fake = _FakeModel()
    runtime = None
    try:
        runtime = await _gateway(tmp_path, fake.base_url)

        keyed = DispatchInput.connector(
            "connector event", correlation_id=_CORRELATION, idempotency_key=_KEY
        )
        assert keyed.to_dict() == {
            "content": "connector event",
            "origin": "connector",
            "correlation_id": _CORRELATION,
            "idempotency_key": _KEY,
        }
        # The answer to `meta/dispatch` is the gateway's admission of the
        # input: the pair passed its check and the delivery was taken. A
        # refusal is an error, never this result.
        admitted = await runtime.dispatch(_IDENTITY, keyed)
        assert isinstance(admitted, DispatchResult), admitted
        assert isinstance(admitted.fencing_token, int), admitted
        assert isinstance(admitted.durable, bool), admitted
        print(
            "ADMITTED the helper's pair:"
            f" fencing_token={admitted.fencing_token} durable={admitted.durable}"
            f" completion_baseline={admitted.completion_baseline}"
        )

        # The control: the gateway refuses a half pair with its own words,
        # before anything is delivered.
        for half, words in _HALF_PAIRS:
            with pytest.raises(RpcError) as refused:
                await runtime.dispatch(
                    _IDENTITY,
                    DispatchInput(content="connector event", origin="connector", **half),
                )
            message = str(refused.value)
            assert refused.value.code == -32603, (refused.value.code, message)
            assert (
                f"refused before bridge admission: invalid delivery identity ({words})"
                in message
            ), message
            print(f"REFUSED {sorted(half)} alone: code={refused.value.code} message={message}")
    finally:
        if runtime is not None:
            await runtime.shutdown()
        fake.close()
