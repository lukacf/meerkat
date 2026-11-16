"""The ``meta/init`` parameters of a builder that never sets ``max_sessions``.

Each file ``fixtures/init_params_v0_9_15_<case>.json`` holds the parameters
the SDK of the 0.9.15 release built for the builder input of that case below,
as one line in the form the transport writes a request line
(``json.dumps(..., allow_nan=False)`` plus a newline, UTF-8). A builder that
does not call ``max_sessions`` keeps building exactly those bytes, and one
that calls it adds its one key and nothing else.

The fixtures are a record of that release: they are not regenerated from this
tree. A case added here needs its line captured from the 0.9.15 SDK.
"""
import json
from pathlib import Path

import pytest
from meerkat_meta.builder import Meta, MetaBuilder
from meerkat_meta.runtime import MetaRuntime

FIXTURES = Path(__file__).parent / "fixtures"


def _bare_builder() -> MetaBuilder:
    return Meta.builder()


def _configured_builder() -> MetaBuilder:
    return (
        Meta.builder()
        .mob_inline('[mob]\nid = "fixture-mob"\n')
        .modules([{"name": "fixture-module"}])
        .member_restore("eager")
        .workgraph(True)
        .fork_instruction_policy("refuse")
        .host_runnables(["fixture-runnable"])
        .console_read_only()
        .console_fetch_timeout_ms(120_000)
        .console_auth_required(False)
        .http_listen("127.0.0.1:0")
        .http_public_base_url("https://meta.example.test")
        .allow_remote()
        .implicit_delegate_idle_retirement(30)
        .persistent_state("fixture-state")
        .shutdown_in_flight_drain_ms(1000)
    )


CASES = {
    "bare": _bare_builder,
    "configured": _configured_builder,
}


def request_line(params: dict) -> bytes:
    return (json.dumps(params, allow_nan=False) + "\n").encode("utf-8")


def init_params_line(builder: MetaBuilder) -> bytes:
    return request_line(MetaRuntime(builder._config)._build_init_params())


def _fixture(name: str) -> bytes:
    return (FIXTURES / f"init_params_v0_9_15_{name}.json").read_bytes()


@pytest.mark.parametrize("name", sorted(CASES))
def test_unset_max_sessions_keeps_the_0_9_15_init_params_bytes(name):
    assert init_params_line(CASES[name]()) == _fixture(name)


@pytest.mark.parametrize("name", sorted(CASES))
def test_set_max_sessions_adds_only_its_key(name):
    builder = CASES[name]().max_sessions(48)
    params = MetaRuntime(builder._config)._build_init_params()

    assert request_line(params) != _fixture(name)
    assert params["runtime_options"].pop("max_sessions") == 48
    assert request_line(params) == _fixture(name)


def test_fixtures_hold_no_max_sessions_key():
    for name in sorted(CASES):
        params = json.loads(_fixture(name))
        assert "max_sessions" not in params["runtime_options"]
        assert "max_sessions" not in params
