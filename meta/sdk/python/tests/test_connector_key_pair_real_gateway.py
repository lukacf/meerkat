"""The connector helper's key pair against a real gateway.

One ``rpc_gateway`` process and one long-term agent seated from a roster and
built through the self-hosted model route with no credential, so the row
needs no provider key. The model is the local chat-completions fake of the
cross-mob rows.

``DispatchInput.connector`` checks the pair where the input is built; the
gateway checks it again before anything is delivered. This row is about the
gateway's side of a plain ``meta/dispatch`` (no ``expected_scope``) with the
connector origin and a canonical UUID as the correlation id, so nothing in
it depends on how another route or another origin treats the pair:

- the gateway admits the helper's keyed input (a result, not a refusal);
- it kept the idempotency key: a repeat of the pair is answered with the
  first admission's turn ticket, and another key under the same correlation
  id with a ticket of its own;
- it reads each key at admission: a half pair, which only the full
  constructor can build, is refused with the gateway's own words for the
  missing half.

Run:
    META_GATEWAY_BIN=<path to an rpc_gateway> PYTHONPATH=sdk/python \
        python3 -m pytest sdk/python/tests/test_connector_key_pair_real_gateway.py -q -s
"""
from __future__ import annotations

import contextlib
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
    _MODEL,
    _FakeModel,
    _config_toml,
)

_needs_a_gateway = pytest.mark.skipif(
    not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN)),
    reason="META_GATEWAY_BIN not set to a built rpc_gateway",
)

_MOB_ID = "connector-key-pair"
_IDENTITY = "domain:intake"
# A canonical UUID: the gateway carries it as given on every route.
_CORRELATION = "7c2d9a4b-3e18-4b7a-8f55-1a9e0c6b2d10"
_KEY = "source-event-42"
_OTHER_KEY = "source-event-43"

# Turn-driven, so a dispatch that asks for it gets a turn ticket.
_MOB_TOML = f"""\
[mob]
id = "{_MOB_ID}"

[profiles.worker]
model = "{_MODEL}"
provider = "self_hosted"
self_hosted_server_id = "local"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"""

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
                identity=_IDENTITY,
                profile="worker",
                addressability="addressable",
                runtime_mode_override="turn_driven",
            )
        ]


async def _gateway(tmp_path: Path, base_url: str):
    root = tmp_path / _MOB_ID
    root.mkdir()
    (root / "mob.toml").write_text(_MOB_TOML)
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


async def _stop_gateway_and_model(runtime, fake, first: BaseException | None) -> None:
    """Stop the gateway and then the stand-in model.

    Both are always attempted, and the first error is the one that surfaces:
    ``first``, the test's own, when there is one; else the gateway's shutdown
    error; else the model's close error. A later error never replaces an
    earlier one: it is printed and dropped.
    """
    cleanup_error: Exception | None = None
    try:
        if runtime is not None:
            await runtime.shutdown()
    except Exception as error:
        cleanup_error = error
    finally:
        try:
            fake.close()
        except Exception as error:
            if cleanup_error is None:
                cleanup_error = error
            else:
                print(f"dropped, a later cleanup error: the model's close failed: {error!r}")
    if cleanup_error is None:
        return
    if first is not None:
        print(f"dropped, a later cleanup error: {cleanup_error!r}")
        return
    raise cleanup_error


@contextlib.asynccontextmanager
async def _running(fake, start):
    """The runtime ``start()`` gives, stopped afterwards together with ``fake``
    (:func:`_stop_gateway_and_model`). An error of the body, or of ``start()``
    itself, is the one that surfaces."""
    runtime = None
    first: BaseException | None = None
    try:
        runtime = await start()
        yield runtime
    except BaseException as error:
        first = error
        raise
    finally:
        await _stop_gateway_and_model(runtime, fake, first)


@_needs_a_gateway
@pytest.mark.asyncio
@pytest.mark.timeout(120)
async def test_the_gateway_admits_the_helpers_pair_and_refuses_each_half(tmp_path, monkeypatch):
    # The gateway process keeps each agent's comms key pair under its state
    # home (on macOS, under its home). It inherits this process's
    # environment, so give it a state home and a home inside this test's
    # directory: a run then leaves nothing outside it and does not depend on
    # the caller's environment.
    state_home = tmp_path / "state-home"
    home = tmp_path / "home"
    state_home.mkdir()
    home.mkdir()
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))

    fake = _FakeModel()
    async with _running(fake, lambda: _gateway(tmp_path, fake.base_url)) as runtime:
        capabilities = await runtime.mob_handle().capabilities()
        print(
            f"GATEWAY contract_version={capabilities.contract_version}"
            f" serves_meta_dispatch={'meta/dispatch' in capabilities.methods}"
            f" feature_capabilities={sorted(capabilities.feature_capabilities)}"
        )

        keyed = DispatchInput.connector(
            "connector event", correlation_id=_CORRELATION, idempotency_key=_KEY
        )
        # The answer to `meta/dispatch` is the gateway's admission of the
        # input: the pair passed its check and the delivery was taken. A
        # refusal is an error, never this result.
        admitted = await runtime.dispatch(_IDENTITY, keyed, track_turn=True)
        assert isinstance(admitted, DispatchResult), admitted
        assert isinstance(admitted.fencing_token, int), admitted
        assert isinstance(admitted.durable, bool), admitted
        assert admitted.turn_ticket, admitted.turn_unavailable
        print(
            "ADMITTED the helper's pair:"
            f" fencing_token={admitted.fencing_token} durable={admitted.durable}"
            f" completion_baseline={admitted.completion_baseline}"
            f" turn_ticket={admitted.turn_ticket}"
        )

        # The gateway kept the idempotency key: the same pair again is the
        # first admission, and another key is an admission of its own.
        repeated = await runtime.dispatch(_IDENTITY, keyed, track_turn=True)
        assert repeated.turn_ticket == admitted.turn_ticket, (repeated, admitted)
        other = await runtime.dispatch(
            _IDENTITY,
            DispatchInput.connector(
                "connector event", correlation_id=_CORRELATION, idempotency_key=_OTHER_KEY
            ),
            track_turn=True,
        )
        assert other.turn_ticket, other.turn_unavailable
        assert other.turn_ticket != admitted.turn_ticket, (other, admitted)
        print(
            f"SAME PAIR AGAIN: turn_ticket={repeated.turn_ticket} (the first admission's);"
            f" ANOTHER KEY, same correlation id: turn_ticket={other.turn_ticket}"
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

    # The isolation held: the gateway kept the agent's session identity in a
    # directory under this test's own state home (under its own home on macOS).
    identity_roots = [
        state_home / "meerkat" / "session_comms_identity",
        home / "Library" / "Application Support" / "meerkat" / "session_comms_identity",
    ]
    kept = [
        entry
        for root in identity_roots
        if root.is_dir()
        for entry in root.iterdir()
        if entry.is_dir()
    ]
    assert kept, f"no session identity directory under {state_home} or {home}"


class _ShutdownFails:
    """A runtime stand-in whose shutdown raises, as a real one does when the
    gateway's cleanup does not settle."""

    def __init__(self, error: Exception) -> None:
        self.error = error
        self.shutdowns = 0

    async def shutdown(self) -> None:
        self.shutdowns += 1
        raise self.error


class _CountsCloses:
    """A model stand-in: no listener, it counts its closes."""

    def __init__(self, error: Exception | None = None) -> None:
        self.error = error
        self.closes = 0

    def close(self) -> None:
        self.closes += 1
        if self.error is not None:
            raise self.error


# No gateway, no network: the cleanup of the row above, driven by stand-ins.
@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("own_fails", "close_fails"),
    [(False, False), (True, False), (False, True)],
    ids=["shutdown-fails-alone", "after-the-tests-own-error", "and-the-close-fails-too"],
)
async def test_a_failed_shutdown_still_closes_the_model_and_the_first_error_surfaces(
    own_fails, close_fails, capsys
):
    own_error = AssertionError("the test's own failure") if own_fails else None
    shutdown_error = RuntimeError("the gateway's cleanup did not settle")
    close_error = OSError("the model's close failed") if close_fails else None
    runtime = _ShutdownFails(shutdown_error)
    fake = _CountsCloses(close_error)

    async def start():
        return runtime

    with pytest.raises(Exception) as surfaced:
        async with _running(fake, start):
            if own_error is not None:
                raise own_error

    assert runtime.shutdowns == 1
    assert fake.closes == 1, "the model was not closed after the failed shutdown"
    assert surfaced.value is (own_error or shutdown_error), surfaced.value
    # The later error is printed, not raised.
    later = close_error if close_fails else shutdown_error if own_fails else None
    printed = capsys.readouterr().out
    assert ("dropped, a later cleanup error" in printed) == (later is not None), printed
    if later is not None:
        assert repr(later) in printed, printed
