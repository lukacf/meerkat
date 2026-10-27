"""session_service() registration refuses a non-conforming builder typed.

``meta/init`` carries ``has_session_builder``, which makes the gateway call
back into this process for every build. An object without ``build_agent`` (a
bare function, a partial, a misnamed method) used to be skipped in silence
while that flag was still sent, so the first spawn failed later with "no
SessionAgentBuilder registered". It must fail at connect(), before any gateway
starts, with a TypeError that names the contract.
"""
import json
import sys

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime


class _ConformingBuilder:
    async def build_agent(self, options) -> None:
        return None


def _bare_function_builder(options):
    return None


class _MisnamedMethodBuilder:
    async def build(self, options) -> None:
        return None


def _write_marking_gateway(tmp_path):
    """A stand-in gateway that records its own start and answers every request."""
    marker = tmp_path / "gateway-started"
    gateway = tmp_path / "marking_gateway.py"
    gateway.write_text(
        f"#!{sys.executable}\n"
        "import json, pathlib, sys\n"
        f"pathlib.Path({json.dumps(str(marker))}).write_text('started')\n"
        "for raw_line in sys.stdin:\n"
        "    request = json.loads(raw_line)\n"
        "    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'],"
        " 'result': {'http_base_url': 'http://127.0.0.1:1'}}), flush=True)\n"
    )
    gateway.chmod(0o755)
    return gateway, marker


class TestSessionBuilderRegistration:
    @pytest.mark.asyncio
    async def test_conforming_builder_is_registered_without_a_gateway(self):
        builder = _ConformingBuilder()
        runtime = MetaRuntime(Meta.builder().session_service(builder)._config)
        await runtime.connect()
        assert runtime.is_running
        assert runtime._dispatcher._builder is builder

    @pytest.mark.asyncio
    async def test_conforming_builder_is_registered_before_init(self, tmp_path):
        gateway, marker = _write_marking_gateway(tmp_path)
        builder = _ConformingBuilder()
        runtime = MetaRuntime(
            Meta.builder().gateway(str(gateway)).session_service(builder)._config
        )
        try:
            await runtime.connect()
            assert runtime.is_running
            assert marker.exists()
            assert runtime._dispatcher._builder is builder
        finally:
            await runtime.shutdown()

    @pytest.mark.asyncio
    async def test_bare_function_is_refused_typed_without_a_gateway(self):
        runtime = MetaRuntime(
            Meta.builder().session_service(_bare_function_builder)._config
        )
        with pytest.raises(TypeError, match="SessionAgentBuilder") as refused:
            await runtime.connect()
        assert "build_agent" in str(refused.value)
        assert "function" in str(refused.value)
        assert not runtime.is_running

    @pytest.mark.asyncio
    async def test_misnamed_method_is_refused_before_the_gateway_starts(self, tmp_path):
        gateway, marker = _write_marking_gateway(tmp_path)
        runtime = MetaRuntime(
            Meta.builder()
            .gateway(str(gateway))
            .session_service(_MisnamedMethodBuilder())
            ._config
        )
        with pytest.raises(TypeError, match="SessionAgentBuilder") as refused:
            await runtime.connect()
        assert "_MisnamedMethodBuilder" in str(refused.value)
        assert not runtime.is_running
        # The refusal precedes the spawn: no transport was created and the
        # gateway never recorded a start.
        assert runtime._transport is None
        assert not marker.exists()
