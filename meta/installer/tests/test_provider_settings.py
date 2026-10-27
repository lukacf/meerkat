"""Private credentials with isolated fictional keys, no native/provider calls."""
import json
import os
import sqlite3
import stat
import sys

import pytest

from launchkit import provider_settings as module
from launchkit.provider_settings import (ProviderSettings, ProviderSettingsUnavailable,
    canonical_setup_provider, canonical_setup_settings)


def test_save_restart_and_replacement_use_existing_native_environment(tmp_path):
    root = tmp_path / "studio"
    environment = {}
    settings = ProviderSettings(root, environ=environment)
    assert settings.restore()["effective_provider"] is None
    public = settings.save("openai", "fictional-first-key")
    assert environment == {"OPENAI_API_KEY": "fictional-first-key"}
    assert public["effective_provider"] == "openai"
    assert stat.S_IMODE(root.stat().st_mode) == 0o700
    assert stat.S_IMODE((root / "provider-settings.json").stat().st_mode) == 0o600
    assert "fictional-first-key" not in json.dumps(public)
    restarted_environment = {}
    restarted = ProviderSettings(root, environ=restarted_environment)
    assert restarted.restore()["providers"][0]["source"] == "saved"
    assert restarted_environment["OPENAI_API_KEY"] == "fictional-first-key"
    restarted.save("openai", "fictional-corrected-key")
    assert restarted_environment["OPENAI_API_KEY"] == "fictional-corrected-key"
    assert "fictional-corrected-key" not in json.dumps(restarted.status())


def test_inherited_and_later_external_keys_win_without_unused_save(tmp_path):
    environment = {"OPENAI_API_KEY": "fictional-inherited-key"}
    settings = ProviderSettings(tmp_path / "studio", environ=environment)
    assert settings.status()["providers"][0] == {
        "provider": "openai", "configured": True, "source": "environment", "editable": False}
    with pytest.raises(ProviderSettingsUnavailable, match="server environment"):
        settings.save("openai", "fictional-replacement-key")
    assert not settings.root.exists()
    settings.save("gemini", "fictional-gemini-key")
    assert settings.status()["effective_provider"] == "openai"
    assert environment["OPENAI_API_KEY"] == "fictional-inherited-key"
    environment["ANTHROPIC_API_KEY"] = "fictional-later-key"
    with pytest.raises(ProviderSettingsUnavailable, match="server environment"):
        settings.save("anthropic", "fictional-ignored-key")


def test_saved_key_cannot_override_inherited_key_after_restart(tmp_path):
    ProviderSettings(tmp_path / "studio", environ={}).save("openai", "fictional-saved-key")
    environment = {"OPENAI_API_KEY": "fictional-host-key"}
    settings = ProviderSettings(tmp_path / "studio", environ=environment)
    public = settings.restore()
    assert environment == {"OPENAI_API_KEY": "fictional-host-key"}
    assert public["providers"][0]["source"] == "environment"
    assert "fictional" not in json.dumps(public)


@pytest.mark.parametrize("provider,name", [
    ("openai", "RKAT_OPENAI_API_KEY"), ("anthropic", "RKAT_ANTHROPIC_API_KEY"),
    ("gemini", "RKAT_GEMINI_API_KEY"), ("gemini", "GOOGLE_API_KEY"),
    ("gemini", "RKAT_GOOGLE_API_KEY"),
])
def test_native_alias_only_is_inherited_readonly_and_visible_to_planner(tmp_path, provider, name):
    environment = {name: "fictional-inherited-alias"}
    settings = ProviderSettings(tmp_path / "studio", environ=environment)
    public = settings.restore()
    assert environment[module.ENV_KEYS[provider]] == "fictional-inherited-alias"
    assert environment[name] == "fictional-inherited-alias"
    assert public["effective_provider"] == provider
    assert next(row for row in public["providers"] if row["provider"] == provider) == {
        "provider": provider, "configured": True, "source": "environment", "editable": False}
    with pytest.raises(ProviderSettingsUnavailable, match="server environment"):
        settings.save(provider, "fictional-saved-replacement")
    assert not settings.root.exists()
    assert "fictional" not in json.dumps(public)


def test_native_primary_precedence_and_later_alias_override_are_not_claimed_saved(tmp_path):
    environment = {"GEMINI_API_KEY": "fictional-primary", "RKAT_GOOGLE_API_KEY": "fictional-fallback"}
    settings = ProviderSettings(tmp_path / "studio", environ=environment)
    settings.restore()
    assert settings.inherited["gemini"] == "fictional-primary"
    saved = ProviderSettings(tmp_path / "other-studio", environ={})
    saved.save("openai", "fictional-saved")
    saved.environ["RKAT_OPENAI_API_KEY"] = "fictional-external-override"
    assert saved.status()["providers"][0]["source"] == "environment"
    with pytest.raises(ProviderSettingsUnavailable):
        saved.save("openai", "fictional-unused-replacement")


@pytest.mark.parametrize("key", [None, 12, "", "  ", "x" * 4097, "key\nvalue", "key\x00value", "key\x7fvalue"])
def test_invalid_keys_do_not_create_files_or_environment(tmp_path, key):
    environment = {}
    settings = ProviderSettings(tmp_path / "studio", environ=environment)
    with pytest.raises(ValueError) as error:
        settings.save("openai", key)
    assert "key" not in str(error.value).replace("API key", "")
    assert environment == {} and not settings.root.exists()


def test_active_owner_refuses_write_and_preserves_original(tmp_path):
    environment = {}
    settings = ProviderSettings(tmp_path / "studio", environ=environment)
    settings.save("openai", "fictional-original-key")
    before = (settings.root / "provider-settings.json").read_bytes()
    with pytest.raises(ProviderSettingsUnavailable, match="Stop active"):
        settings.save("openai", "fictional-other-key", runtime_active=True)
    assert (settings.root / "provider-settings.json").read_bytes() == before
    assert environment["OPENAI_API_KEY"] == "fictional-original-key"
    assert not settings.status(runtime_active=True)["providers"][0]["editable"]


@pytest.mark.parametrize("damage", ["symlink", "hardlink", "public", "invalid", "oversize", "deep_json"])
def test_damaged_file_is_nonfatal_but_not_replaced(tmp_path, damage):
    root = tmp_path / "studio"
    root.mkdir(mode=0o700)
    path = root / "provider-settings.json"
    outside = tmp_path / "original"
    outside.write_text('{"version":1,"keys":{"openai":"fictional-private-key"}}')
    outside.chmod(0o600)
    if damage == "symlink":
        path.symlink_to(outside)
    elif damage == "hardlink":
        os.link(outside, path)
    elif damage == "deep_json":
        depth = min(sys.getrecursionlimit() * 15, (module.MAX_SETTINGS_BYTES - 128) // 2)
        content = '{"version":1,"keys":{"openai":' + '[' * depth + '"fictional-deep"' + ']' * depth + '}}'
        assert len(content.encode()) < module.MAX_SETTINGS_BYTES
        with pytest.raises(RecursionError):
            json.loads(content)
        path.write_text(content)
        path.chmod(0o600)
    else:
        path.write_text("{" if damage == "invalid" else "x" * (module.MAX_SETTINGS_BYTES + 1)
                        if damage == "oversize" else outside.read_text())
        path.chmod(0o644 if damage == "public" else 0o600)
    before = path.read_bytes()
    environment = {"ANTHROPIC_API_KEY": "fictional-host-key"}
    settings = ProviderSettings(root, environ=environment)
    public = settings.restore()
    assert public["load_error"] and public["effective_provider"] == "anthropic"
    assert environment == {"ANTHROPIC_API_KEY": "fictional-host-key"}
    assert "fictional" not in json.dumps(public)
    assert settings.status() == public
    with pytest.raises(ProviderSettingsUnavailable) as error:
        settings.save("openai", "fictional-new-key")
    assert "fictional" not in str(error.value)
    assert environment == {"ANTHROPIC_API_KEY": "fictional-host-key"}
    assert path.read_bytes() == before
    assert not list(root.glob(".provider-settings-*.pending"))


def test_linked_directory_refuses_without_touching_target(tmp_path):
    original = tmp_path / "original"
    original.mkdir(mode=0o700)
    root = tmp_path / "studio"
    root.symlink_to(original, target_is_directory=True)
    settings = ProviderSettings(root, environ={})
    assert settings.restore()["load_error"]
    with pytest.raises(ProviderSettingsUnavailable):
        settings.save("openai", "fictional-new-key")
    assert list(original.iterdir()) == []


def test_failed_private_write_never_applies_new_environment(tmp_path, monkeypatch):
    settings = ProviderSettings(tmp_path / "studio", environ={})
    settings.save("openai", "fictional-original-key")
    before = (settings.root / "provider-settings.json").read_bytes()
    def refuse(*args, **kwargs):
        raise OSError("fictional-secret-error-text")
    monkeypatch.setattr(module.os, "replace", refuse)
    with pytest.raises(ProviderSettingsUnavailable) as error:
        settings.save("openai", "fictional-new-key")
    assert "fictional" not in str(error.value)
    assert (settings.root / "provider-settings.json").read_bytes() == before
    assert settings.environ["OPENAI_API_KEY"] == "fictional-original-key"
    assert not list(settings.root.glob("*.pending"))


def test_invalid_provider_never_injects_environment_name(tmp_path):
    settings = ProviderSettings(tmp_path / "studio", environ={})
    with pytest.raises(ValueError):
        settings.save("PYTHONPATH", "fictional-payload")
    assert settings.environ == {} and not settings.root.exists()


def native_spec(root, *, provider="anthropic", mutate=None):
    database = root / ".mobkit/state/event_log.sqlite3"
    database.parent.mkdir(parents=True)
    value = {"revision": 1, "definition": {"id": "launch-architect", "profiles": {
        name: {"provider": provider, "instructions": "fictional-private-instructions"}
        for name in module._SETUP_PROFILES}, "unprojected": "fictional-private-other-field"}}
    if mutate:
        mutate(value)
    with sqlite3.connect(database) as connection:
        connection.execute("CREATE TABLE mob_specs (mob_id TEXT PRIMARY KEY, spec_json BLOB NOT NULL)")
        connection.execute("INSERT INTO mob_specs VALUES (?, ?)", ("launch-architect", json.dumps(value).encode()))
        connection.execute("CREATE TABLE private_history (body TEXT)")
        connection.execute("INSERT INTO private_history VALUES ('fictional-private-history')")
        connection.execute("INSERT INTO mob_specs VALUES (?, ?)", ("other-mob", b"not setup JSON"))
    return database


def test_canonical_provider_is_native_spec_not_current_environment(tmp_path, monkeypatch):
    root = tmp_path / "runtime"
    database = native_spec(root)
    before = database.read_bytes()
    monkeypatch.setenv("OPENAI_API_KEY", "fictional-current-openai-key")
    assert canonical_setup_provider(root) == "anthropic"
    assert database.read_bytes() == before
    assert set(database.parent.iterdir()) == {database}


def test_absent_native_store_remains_absent(tmp_path):
    root = tmp_path / "runtime"
    assert canonical_setup_provider(root) is None
    assert not root.exists()


def test_canonical_tuning_retains_each_native_profile_without_history_or_store_writes(tmp_path):
    root = tmp_path / "runtime"
    thresholds = {"architect": None, "connectors": 65000, "reviewer": None, "visualizer": 200000}
    def mutate(value):
        for name, threshold in thresholds.items():
            value["definition"]["profiles"][name]["auto_compact_threshold"] = threshold
    database = native_spec(root, mutate=mutate)
    before = database.read_bytes()
    assert canonical_setup_settings(root) == {"provider": "anthropic", "auto_compact_thresholds": thresholds}
    assert database.read_bytes() == before
    assert set(database.parent.iterdir()) == {database}


@pytest.mark.parametrize("threshold", [True, False, 0, -1, 1.5, "200000", [], {}, 1 << 63])
def test_invalid_native_tuning_refuses_without_private_projection_or_store_writes(tmp_path, threshold):
    root = tmp_path / "runtime"
    def mutate(value):
        value["definition"]["profiles"]["architect"]["auto_compact_threshold"] = threshold
    database = native_spec(root, mutate=mutate)
    before = database.read_bytes()
    with pytest.raises(ProviderSettingsUnavailable) as error:
        canonical_setup_settings(root)
    assert "fictional" not in str(error.value)
    assert database.read_bytes() == before


@pytest.mark.parametrize("damage", ["different-provider", "missing-profile", "invalid-profile", "realm-ref", "wrong-id", "invalid-revision", "unknown-provider", "oversize"])
def test_unknown_canonical_provider_fails_closed_without_private_projection(tmp_path, damage):
    root = tmp_path / "runtime"
    def mutate(value):
        profiles = value["definition"]["profiles"]
        if damage == "different-provider":
            profiles["visualizer"]["provider"] = "openai"
        elif damage == "missing-profile":
            profiles.pop("reviewer")
        elif damage == "invalid-profile":
            profiles["reviewer"] = None
        elif damage == "realm-ref":
            profiles["architect"]["realm_profile"] = "fictional-private-ref"
        elif damage == "wrong-id":
            value["definition"]["id"] = "other-mob"
        elif damage == "invalid-revision":
            value["revision"] = True
        elif damage == "unknown-provider":
            profiles["architect"]["provider"] = "fictional-private-provider"
        else:
            value["definition"]["unprojected"] = "x" * (module._MAX_SPEC_BYTES + 1)
    database = native_spec(root, mutate=mutate)
    before = database.read_bytes()
    with pytest.raises(ProviderSettingsUnavailable) as error:
        canonical_setup_provider(root)
    assert "fictional" not in str(error.value)
    assert database.read_bytes() == before


def test_existing_unverified_native_store_is_not_treated_as_absent(tmp_path):
    root = tmp_path / "runtime"
    state = root / ".mobkit/state"
    state.mkdir(parents=True)
    (state / "unverified").write_text("fictional-private-state")
    with pytest.raises(ProviderSettingsUnavailable):
        canonical_setup_provider(root)
    assert not (state / "event_log.sqlite3").exists()


def test_linked_native_database_refuses_without_reading_target(tmp_path):
    original = native_spec(tmp_path / "original")
    root = tmp_path / "runtime"
    state = root / ".mobkit/state"
    state.mkdir(parents=True)
    (state / "event_log.sqlite3").symlink_to(original)
    with pytest.raises(ProviderSettingsUnavailable):
        canonical_setup_provider(root)
