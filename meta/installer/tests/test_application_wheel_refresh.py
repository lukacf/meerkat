"""Offline retained-dependency refresh controls, never a real runtime upgrade."""
import hashlib
import os
from pathlib import Path

import pytest

from launchkit import compiler, deployment_updates as updates
from test_application_wheel_assets import ASSET, wheel_bytes
from test_reading_runtime_refresh import refresh_pair


def assert_retained_wheel(tmp_path, monkeypatch, content):
    previous, candidate, _, _ = refresh_pair(tmp_path, monkeypatch)
    previous[ASSET] = candidate[ASSET] = (content, 0o644)
    previous["requirements.application.txt"] = candidate["requirements.application.txt"] = (
        b"./application/vendor/example_dependency-1.0-py3-none-any.whl\n", 0o644)
    original = compiler.generate_runtime_refresh
    observed = []

    def inspect(value, source, **kwargs):
        selected = next(item for item in source if item.path == ASSET)
        assert selected.content == content and selected.as_bytes() == content
        assert selected.mode == 0o644
        observed.append(selected)
        return original(value, source, **kwargs)

    monkeypatch.setattr(compiler, "generate_runtime_refresh", inspect)
    transition = updates.runtime_dependency_transition(previous, candidate, {})
    assert transition and transition["to_version"] == "0.8.47"
    assert ASSET not in transition["protected_paths"]
    assert observed and updates.compatibility(previous, candidate, {})[1] == []
    assert candidate[ASSET] == previous[ASSET]
    return previous, candidate


def test_runtime_refresh_preserves_wheel_without_promoting_application_into_framework(tmp_path, monkeypatch):
    assert_retained_wheel(tmp_path, monkeypatch, wheel_bytes())


@pytest.mark.parametrize("mutation", ["bytes", "mode", "new-wheel", "dependency-pin", "other-binary"])
def test_runtime_refresh_refuses_dependency_changes_or_unrecognized_binary(tmp_path, monkeypatch, mutation):
    previous, candidate, _, _ = refresh_pair(tmp_path, monkeypatch)
    previous[ASSET] = candidate[ASSET] = (wheel_bytes(), 0o644)
    if mutation == "bytes":
        candidate[ASSET] = (candidate[ASSET][0] + b"changed", 0o644)
    elif mutation == "mode":
        candidate[ASSET] = (candidate[ASSET][0], 0o755)
    elif mutation == "new-wheel":
        candidate["application/vendor/other-1.0-py3-none-any.whl"] = (wheel_bytes(), 0o644)
    elif mutation == "dependency-pin":
        previous["requirements.application.txt"] = (b"python-telegram-bot==22.7\n", 0o644)
        candidate["requirements.application.txt"] = (b"python-telegram-bot==22.8\n", 0o644)
    else:
        previous["application/payload.bin"] = candidate["application/payload.bin"] = (b"\x00\xff", 0o644)
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None


def test_actual_qualified_wheel_retains_exact_bytes_in_offline_refresh(tmp_path, monkeypatch):
    path, expected = os.environ.get("FORGE_TEST_WHEEL_PATH"), os.environ.get("FORGE_TEST_WHEEL_SHA256")
    if not path or not expected:
        pytest.skip("Optional developer-owned qualified wheel fixture was not supplied.")
    content = Path(path).read_bytes()
    assert hashlib.sha256(content).hexdigest() == expected
    assert_retained_wheel(tmp_path, monkeypatch, content)
