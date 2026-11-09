"""Refuse changed source after an inert awaited native observation."""
import asyncio

import pytest

from test_native_definition_continuity import host_fixture


@pytest.mark.parametrize("operation", ["send", "dispatch"])
@pytest.mark.parametrize("observation", ["capabilities", "continuity-history"])
async def test_changed_configuration_during_observation_never_admits_a_turn(
        tmp_path, monkeypatch, operation, observation):
    from meerkat_mobkit.identity_first_models import DispatchInput

    carrier = observation == "capabilities"
    runtime, deployment, _, calls, admissions, _, native_path = await host_fixture(
        tmp_path, monkeypatch, carrier=carrier)
    identity = deployment["agents"][0]["identity"]
    message = DispatchInput(content="Keep this private case", origin="connector",
                            correlation_id="same-case", idempotency_key="same-key")

    # Use the existing fixture's admissions with explicit shipping signature
    # parameters so both carrier and inherited paths keep tracking intact.
    native_runtime = type(runtime).__mro__[1]
    original_send, original_dispatch = native_runtime.send, native_runtime.dispatch

    async def send(self, selected, content, *, system_prompt=None):
        kwargs = {} if system_prompt is None else {"system_prompt": system_prompt}
        return await original_send(self, selected, content, **kwargs)

    async def dispatch(self, selected, value, *, system_prompt=None, track_turn=False):
        kwargs = {"track_turn": True} if track_turn else {}
        if system_prompt is not None:
            kwargs["system_prompt"] = system_prompt
        return await original_dispatch(self, selected, value, **kwargs)

    monkeypatch.setattr(native_runtime, "send", send)
    monkeypatch.setattr(native_runtime, "dispatch", dispatch)
    original_rpc = runtime._rpc
    observed_method = "mobkit/capabilities" if carrier else "mobkit/mob_events/query"

    try:
        for changed in (False, True):
            entered, release = asyncio.Event(), asyncio.Event()
            if carrier:
                # Arrange the real first-observation branch. Restored host
                # callbacks may reach it while connection is still observing.
                runtime._instruction_observation = None

            async def observed_rpc(method, params):
                if method == observed_method:
                    entered.set()
                    await release.wait()
                return await original_rpc(method, params)

            monkeypatch.setattr(runtime, "_rpc", observed_rpc)
            before_calls = len(calls)
            pending = asyncio.create_task(runtime.send(identity, "Private follow-up")
                if operation == "send" else runtime.dispatch(identity, message, track_turn=True))
            try:
                await asyncio.wait_for(entered.wait(), timeout=30)
                assert not pending.done()
                assert len(admissions) == (1 if changed else 0)
                if changed:
                    native_path.write_bytes(native_path.read_bytes() + b"\n# changed while observation waited\n")
                release.set()
                if changed:
                    with pytest.raises(RuntimeError, match="changed while running"):
                        await pending
                else:
                    assert await pending == ("native send admission" if operation == "send" else "native keyed admission")
            finally:
                release.set()
                if not pending.done():
                    pending.cancel()
                    await asyncio.gather(pending, return_exceptions=True)
            assert len(calls) == before_calls + 1
            assert len(admissions) == 1
            expected_content = "Private follow-up" if operation == "send" else message.to_dict()
            assert admissions[0][:3] == (operation, identity, expected_content)
            kwargs = admissions[0][3]
            assert (kwargs.get("track_turn") is True) == (operation == "dispatch")
            if carrier:
                assert deployment["classes"][0]["instructions"] in kwargs["system_prompt"]
            else:
                assert "system_prompt" not in kwargs
    finally:
        await runtime.shutdown()
