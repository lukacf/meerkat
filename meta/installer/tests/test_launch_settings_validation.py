"""Validate saved settings before any generated application resource starts."""
import json
import os
import sys
from types import SimpleNamespace

import pytest

from test_application_extension import load_host


@pytest.mark.parametrize("invalid", [None, "whitespace", "secret-length", "integer-range"],
                         ids=["valid-optional-default-exact", "required-whitespace",
                              "private-secret-length", "integer-range"])
async def test_launch_validates_saved_application_settings_before_resources(
        tmp_path, monkeypatch, invalid):
    host = load_host(monkeypatch, tmp_path)
    fields = [
        {"key": "LAUNCH_SITE_NAME", "label": "Site name", "secret": False,
         "required": True, "nonblank": True},
        {"key": "LAUNCH_ACCESS_SECRET", "label": "Site access secret", "secret": True,
         "required": True, "min_length": 16, "max_length": 80},
        {"key": "LAUNCH_LISTENER_PORT", "label": "Site listener", "secret": False,
         "required": True, "type": "integer", "minimum": 1024, "maximum": 65535},
        {"key": "LAUNCH_OPTIONAL_SECRET", "label": "Optional service secret", "secret": True,
         "required": False, "nonblank": True, "min_length": 16},
        {"key": "LAUNCH_THEME", "label": "Display theme", "secret": False,
         "required": False, "min_length": 5, "default": "quiet"},
    ]
    application = tmp_path / "application"
    application.mkdir()
    (application / "configuration.json").write_text(json.dumps({"fields": fields}))
    environment = [{"key": "OPENAI_API_KEY", "label": "Provider key",
                    "secret": True, "required": True}]
    spec = {"environment": environment, "port": 8788, "classes": []}
    values = {"OPENAI_API_KEY": "inert-provider-canary",
              "LAUNCH_SITE_NAME": "  exact chosen site  ",
              "LAUNCH_ACCESS_SECRET": " \tvalid-private-canary-token\t ",
              "LAUNCH_LISTENER_PORT": "+04111"}
    problems = {
        "whitespace": ("LAUNCH_SITE_NAME", " \t ", "not only whitespace"),
        "secret-length": ("LAUNCH_ACCESS_SECRET", "tiny-canary", "at least 16"),
        "integer-range": ("LAUNCH_LISTENER_PORT", "65536", "whole number between 1024 and 65535"),
    }
    if invalid:
        key, value, issue = problems[invalid]
        values[key] = value
    for row in [*environment, *fields]:
        # Register restoration before the real dotenv loader writes these keys.
        monkeypatch.setenv(row["key"], "inert-before-load")
        monkeypatch.delenv(row["key"])
    monkeypatch.delenv("MOBKIT_PORT", raising=False)
    monkeypatch.delenv("MOBKIT_RPC_GATEWAY_BIN", raising=False)
    saved = tmp_path / ".env"
    saved_bytes = "".join(f"{key}={json.dumps(value)}\n" for key, value in values.items()).encode()
    saved.write_bytes(saved_bytes)
    saved.chmod(0o600)
    host.load_environment(saved, override=True)
    gateway = tmp_path / ".bin/rpc_gateway"
    gateway.parent.mkdir()
    gateway.touch()

    milestones = []

    class ResourceReached(RuntimeError):
        pass

    def resource(name):
        def reached(*args, **kwargs):
            milestones.append(name)
            assert {key: os.environ[key] for key in values} == values
            assert host.configuration_value(fields[-2]) == ""
            assert host.configuration_value(fields[-1]) == "quiet"
            raise ResourceReached(name)
        return reached

    monkeypatch.setattr(host, "ConnectorTools", resource("connector-tools"))
    monkeypatch.setattr(host, "MCPPool", resource("mcp-pool"))
    monkeypatch.setattr(host, "Application", resource("application"))
    monkeypatch.setitem(sys.modules, "storage_setup",
                        SimpleNamespace(prepare_storage=resource("storage")))
    if invalid:
        with pytest.raises(ValueError, match=issue) as failure:
            await host.run(spec, SimpleNamespace())
        diagnostic = str(failure.value)
        assert key in diagnostic
        assert ".venv/bin/python setup.py" in diagnostic
        assert values[key] not in diagnostic
        assert "inert-provider-canary" not in diagnostic
        assert "valid-private-canary-token" not in diagnostic
        assert milestones == []
    else:
        with pytest.raises(ResourceReached, match="application"):
            await host.run(spec, SimpleNamespace())
        assert milestones == ["application"]
    assert saved.read_bytes() == saved_bytes
    assert not (tmp_path / "data").exists()
    assert not (tmp_path / ".mobkit").exists()
