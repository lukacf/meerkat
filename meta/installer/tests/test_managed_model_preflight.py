"""Generated app preflight controls with an inert selected-startup boundary.

No model, native SDK, credential store, gateway or deployment is exercised.
The genuine startup owners' custody controls are checked separately.
"""
from pathlib import Path
from types import SimpleNamespace
import subprocess
import socket

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.connect_installer_caller import CustodyError
from test_application_extension import load_host
import json


@pytest.fixture(autouse=True)
def no_external_operations(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Managed preflight cannot execute processes or use sockets.")
    for owner, name in ((subprocess, "Popen"), (subprocess, "run"),
                        (socket, "create_connection")):
        monkeypatch.setattr(owner, name, forbidden)
    for key in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"):
        monkeypatch.delenv(key, raising=False)


def managed_spec(*, mixed=False):
    proposal = starter_blueprint()
    classes = []
    for index, original in enumerate(proposal.agent_classes):
        classes.append(original.model_copy(update={
            "provider": "anthropic", "model": "claude-sonnet-5-5",
            "model_auth": "api_key" if mixed and index == 0 else "managed_host"}))
    proposal = proposal.model_copy(update={"agent_classes": classes})
    return json.loads(generate_files(proposal)["deployment.json"])


@pytest.mark.asyncio
@pytest.mark.parametrize("startup", [None, SimpleNamespace()])
async def test_managed_generated_run_refuses_missing_owned_selection_before_import_or_settings(
        tmp_path, monkeypatch, startup):
    host = load_host(monkeypatch, tmp_path)
    monkeypatch.setattr(host, "console_setup_source_bytes", lambda *args: pytest.fail(
        "Private settings must not be read before managed startup selection."))
    with pytest.raises(ValueError, match="original installed startup selection"):
        await host.run(managed_spec(), SimpleNamespace(), application_startup=startup)


@pytest.mark.asyncio
async def test_managed_generated_run_preserves_owner_refusal_before_gateway_or_private_settings(
        tmp_path, monkeypatch):
    host = load_host(monkeypatch, tmp_path)
    calls = []
    def refuse(spec):
        calls.append(spec)
        raise CustodyError("Synthetic selected model config changed.")
    startup = SimpleNamespace(require_model_configuration=refuse)
    monkeypatch.setattr(host, "console_setup_source_bytes", lambda *args: pytest.fail(
        "No settings access is permitted after an owner refusal."))
    spec = managed_spec()
    with pytest.raises(CustodyError, match="model config changed"):
        await host.run(spec, SimpleNamespace(), application_startup=startup)
    assert calls == [spec]


@pytest.mark.asyncio
async def test_managed_choice_cannot_skip_another_class_required_key_in_actual_generated_preflight(
        tmp_path, monkeypatch):
    host = load_host(monkeypatch, tmp_path)
    gateway = tmp_path / "inert-gateway"
    gateway.write_text("Never executed.\n")
    spec = managed_spec(mixed=True)
    calls = []
    startup = SimpleNamespace(gateway=SimpleNamespace(path=gateway),
        require_model_configuration=lambda selected: calls.append(selected))
    with pytest.raises(ValueError, match="API key"):
        await host.run(spec, SimpleNamespace(), application_startup=startup)
    assert calls == [spec]
    fields = {row["key"]: row for row in spec["environment"]}
    assert fields["ANTHROPIC_API_KEY"]["required"] is True
    assert fields["ANTHROPIC_API_KEY"]["secret"] is True


@pytest.mark.asyncio
async def test_ordinary_key_class_does_not_require_managed_owner_hook(tmp_path, monkeypatch):
    host = load_host(monkeypatch, tmp_path)
    gateway = tmp_path / "inert-gateway"
    gateway.write_text("Never executed.\n")
    spec = json.loads(generate_files(starter_blueprint())["deployment.json"])
    startup = SimpleNamespace(gateway=SimpleNamespace(path=gateway))
    with pytest.raises(ValueError, match="API key"):
        await host.run(spec, SimpleNamespace(), application_startup=startup)
