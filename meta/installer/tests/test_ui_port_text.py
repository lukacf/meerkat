"""UI port projection agrees with private configuration integer text."""
import json
import os

import pytest

from launchkit.application_experiences import project_application_experiences
from launchkit.templates_v2.application_configuration import configuration_issue, MAX_VALUE_LENGTH


@pytest.mark.parametrize("raw, expected", [
    pytest.param("+04111", 4111, id="signed-leading-zero"),
    pytest.param("0004111", 4111, id="unsigned-leading-zeros"),
    pytest.param("0" * (MAX_VALUE_LENGTH - 4) + "4111", 4111, id="maximum-length-leading-zeros"),
    pytest.param("4111", 4111, id="ordinary-text"),
    pytest.param(4111, 4111, id="ordinary-integer"),
    pytest.param("+01024", 1024, id="minimum-port"),
    pytest.param("+065535", 65535, id="maximum-port"),
    pytest.param("--4111", None, id="malformed-sign"),
    pytest.param(" +4111", None, id="leading-whitespace"),
    pytest.param("4111 ", None, id="trailing-whitespace"),
    pytest.param("4_111", None, id="underscore"),
    pytest.param("4111.0", None, id="decimal-text"),
    pytest.param("\u0664\u0661\u0661\u0661", None, id="non-ascii-digits"),
    pytest.param("4111?token=private-port-canary", None, id="private-invalid-text"),
    pytest.param("-04111", None, id="negative"),
    pytest.param("+01023", None, id="below-range"),
    pytest.param("+065536", None, id="above-range"),
    pytest.param("9" * 17, None, id="excess-significant-digits"),
    pytest.param("0" * (MAX_VALUE_LENGTH - 3) + "4111", None, id="excess-text-length"),
    pytest.param(True, None, id="boolean"),
    pytest.param(4111.0, None, id="float"),
    pytest.param(float("inf"), None, id="infinity"),
    pytest.param(float("nan"), None, id="nan"),
    pytest.param(None, None, id="absent"),
])
def test_declared_ui_port_preserves_valid_integer_text_and_safe_refusals(raw, expected, monkeypatch):
    field = {"key": "APP_UI_PORT", "label": "Application UI port", "secret": False,
             "required": True, "type": "integer", "minimum": 1024, "maximum": 65535}
    files = {"application/configuration.json": json.dumps({"fields": [field]}).encode(),
             "application/experience.json": json.dumps({"entries": [{"id": "workspace", "label": "Workspace",
                 "path": "/app", "port_field": "APP_UI_PORT"}]}).encode()}
    original_files = dict(files)
    settings = {"APP_UI_PORT": raw, "UNDECLARED_SECRET": "private-port-canary"}
    monkeypatch.setenv("APP_UI_PORT", "unchanged-owned-env-canary")
    # The configuration owner admits the same bounded integer string forms.
    if isinstance(raw, str):
        assert (configuration_issue(field, raw) is None) is (expected is not None)
    definition = {"port": 8787, "environment": []}
    arguments = {"source_digest": "a" * 64, "configuration_values": settings,
                 "console_url": "http://127.0.0.1:8787/console"}
    local = project_application_experiences(files, definition, **arguments)
    remote = project_application_experiences(files, definition, **arguments,
        remote=True, tunnel_mappings={expected: 18880} if expected is not None else {})
    for result in (local, remote):
        entry = result["entries"][0]
        assert entry["destination_port"] == expected
        assert result["verified"] is False and entry["verified"] is False
        assert result["source_digest"] == "a" * 64
        assert "private-port-canary" not in json.dumps(result)
        if expected is None:
            assert entry["url"] is None
            assert entry["reason"] == "Configure this declared nonsecret port through private setup."
        else:
            assert entry["reason"] is None
    if expected is not None:
        assert local["entries"][0]["url"] == f"http://127.0.0.1:{expected}/app"
        assert remote["entries"][0]["url"] == "http://127.0.0.1:18880/app"
        assert remote["entries"][0]["tunnel_mapping"] == {"destination_port": expected, "local_port": 18880}
    assert settings["APP_UI_PORT"] is raw
    assert settings["UNDECLARED_SECRET"] == "private-port-canary"
    assert files == original_files
    assert os.environ["APP_UI_PORT"] == "unchanged-owned-env-canary"
