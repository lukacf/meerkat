"""Actual provider projection functions with isolated, inert service owners."""
import ast
import asyncio
import json
from pathlib import Path
from types import SimpleNamespace

import pytest
from fastapi import HTTPException

from test_meta_guide_runtime import selected
from launchkit.provider_settings import ProviderSettings, ProviderSettingsUnavailable


def provider_functions(tmp_path, locked=None):
    source = Path(__file__).parents[1] / "launchkit/server.py"
    functions = [node for node in ast.parse(source.read_text()).body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        and node.name in {"provider_public_status", "save_provider_settings"}]
    for function in functions:
        function.decorator_list = []
    environment = {}
    settings = ProviderSettings(tmp_path / "private-profile", environ=environment)
    scope = {"__package__": "launchkit", "ProviderSettingsBody": object,
        "provider_settings": settings, "STUDIO_ROOT": settings.root,
        "canonical_setup_provider": lambda root: locked,
        "ProviderSettingsUnavailable": ProviderSettingsUnavailable, "HTTPException": HTTPException,
        "architect": SimpleNamespace(runtime=None, state="stopped", provider_stop_failed=False,
            start_lock=asyncio.Lock()), "provider_owned_work": lambda: False}
    exec(compile(ast.Module(body=functions, type_ignores=[]), str(source), "exec"), scope)
    return scope, settings, environment


def test_host_projection_describes_original_model_without_exposing_credentials(selected, tmp_path):
    scope, settings, environment = provider_functions(tmp_path)
    public = scope["provider_public_status"]()
    assert public["effective_provider"] == public["locked_provider"] == "openai"
    assert public["effective_model"] == "gpt-6.1-sol"
    row = next(item for item in public["providers"] if item["provider"] == "openai")
    assert row == {"provider": "openai", "configured": True, "source": "native_host_config", "editable": False}
    assert public["runtime_active"] is False and public["runtime_status"] == "stopped"
    assert public["provider_binding_error"] is None
    assert not environment and not settings.root.exists()
    encoded = json.dumps(public)
    for private in ["fixture-host-route", str(selected.gateway.path), "private-profile"]:
        assert private not in encoded


@pytest.mark.asyncio
async def test_host_credentials_cannot_be_replaced_or_switch_provider_through_setup(selected, tmp_path):
    scope, settings, environment = provider_functions(tmp_path)
    for provider in ["openai", "anthropic"]:
        with pytest.raises(HTTPException) as caught:
            await scope["save_provider_settings"](SimpleNamespace(provider=provider, api_key="fixture-not-saved"))
        assert caught.value.status_code == 409
    assert not environment and not settings.root.exists()


def test_saved_provider_conflict_is_exposed_without_rebinding_native_history(selected, tmp_path):
    scope, settings, environment = provider_functions(tmp_path, locked="anthropic")
    public = scope["provider_public_status"]()
    assert public["locked_provider"] == "openai"
    assert "differs from this selected model host" in public["provider_binding_error"]
    assert not environment and not settings.root.exists()


def test_previous_setup_generation_lends_settings_without_binding_the_fresh_one(selected, tmp_path, monkeypatch):
    from launchkit import guide_sdk
    scope, settings, environment = provider_functions(tmp_path)
    reads = []
    def unreadable(root):
        reads.append(root.name)
        raise ProviderSettingsUnavailable("fictional unreadable saved definition")
    scope["canonical_setup_provider"] = unreadable
    (settings.root / "architect-runtime-meta-v1").mkdir(parents=True)
    monkeypatch.setattr(guide_sdk, "SELECTED_VERSION", "0.9.4")
    public = scope["provider_public_status"]()
    assert reads == ["architect-runtime-meta-v1"]
    assert public["locked_provider"] == "openai" and public["provider_binding_error"] is None
    # The fresh generation's own saved definition still binds its provider.
    (settings.root / "architect-runtime-meta-v2" / ".mobkit" / "state").mkdir(parents=True)
    (settings.root / "architect-runtime-meta-v2" / ".mobkit" / "state" / "event_log.sqlite3").write_bytes(b"")
    public = scope["provider_public_status"]()
    assert reads[-1] == "architect-runtime-meta-v2"
    assert public["provider_binding_error"] == "fictional unreadable saved definition"
    assert not environment


def test_lent_provider_never_conflicts_with_the_selected_model_host(selected, tmp_path, monkeypatch):
    from launchkit import guide_sdk
    scope, settings, environment = provider_functions(tmp_path, locked="anthropic")
    (settings.root / "architect-runtime-meta-v1").mkdir(parents=True)
    monkeypatch.setattr(guide_sdk, "SELECTED_VERSION", "0.9.4")
    public = scope["provider_public_status"]()
    assert public["provider_binding_error"] is None
    assert public["locked_provider"] == public["effective_provider"] == "openai"
    # The fresh generation's own saved definition still reports a real conflict.
    state = settings.root / "architect-runtime-meta-v2" / ".mobkit" / "state"
    state.mkdir(parents=True)
    (state / "event_log.sqlite3").write_bytes(b"")
    public = scope["provider_public_status"]()
    assert "differs from this selected model host" in public["provider_binding_error"]
    assert public["locked_provider"] == "openai"
    assert not environment
