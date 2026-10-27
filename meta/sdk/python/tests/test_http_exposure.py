"""HTTP exposure: the init result's advertised base URL reaches the runtime."""
import json
import sys

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.runtime import MetaRuntime


def _write_echo_gateway(tmp_path, result: dict):
    """A stand-in gateway that answers every request with ``result``."""
    gateway = tmp_path / "echo_gateway.py"
    gateway.write_text(
        f"#!{sys.executable}\n"
        "import json, sys\n"
        f"RESULT = json.loads({json.dumps(json.dumps(result))})\n"
        "for raw_line in sys.stdin:\n"
        "    request = json.loads(raw_line)\n"
        "    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': RESULT}), flush=True)\n"
    )
    gateway.chmod(0o755)
    return gateway


class TestHttpExposureInitResult:
    @pytest.mark.asyncio
    async def test_connect_records_local_and_advertised_base_urls(self, tmp_path):
        gateway = _write_echo_gateway(
            tmp_path,
            {
                "http_base_url": "http://127.0.0.1:1",
                "http_public_base_url": "https://mob.example.com",
            },
        )
        runtime = MetaRuntime(Meta.builder().gateway(str(gateway))._config)
        try:
            await runtime.connect()
            assert runtime.rust_http_base_url == "http://127.0.0.1:1"
            assert runtime.rust_http_public_base_url == "https://mob.example.com"
        finally:
            await runtime.shutdown()

    @pytest.mark.asyncio
    async def test_public_base_url_falls_back_to_the_local_base_when_undeclared(
        self, tmp_path
    ):
        gateway = _write_echo_gateway(
            tmp_path,
            {"http_base_url": "http://127.0.0.1:1", "http_public_base_url": None},
        )
        runtime = MetaRuntime(Meta.builder().gateway(str(gateway))._config)
        try:
            await runtime.connect()
            assert runtime.rust_http_base_url == "http://127.0.0.1:1"
            assert runtime.rust_http_public_base_url == "http://127.0.0.1:1"
        finally:
            await runtime.shutdown()
