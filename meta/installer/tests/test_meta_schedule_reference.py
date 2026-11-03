"""Current schedule documentation is usable offline and never a tool grant."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from jsonschema import Draft202012Validator
import pytest

import launchkit.native_schedule_reference as reference


def bundled():
    return json.loads(Path(reference.__file__).with_name("meta_schedule_reference_0_9_4.json").read_text())


def test_094_supports_host_runnable_creation_and_flat_update_with_native_constraints():
    schemas = bundled()["tool_schemas"]
    create = Draft202012Validator(schemas["meerkat_schedule_create"])
    request = {"trigger": {"type": "calendar", "timezone": "Europe/Stockholm",
                           "hour": {"kind": "values", "values": [9]}},
               "target": {"target_kind": "host_runnable", "runnable": "refresh-projects",
                          "params": {"scope": "selected-projects"}},
               "misfire_policy": {"type": "skip"}, "overlap_policy": "skip_if_running",
               "missing_target_policy": "mark_misfired"}
    create.validate(request)
    update = Draft202012Validator(schemas["meerkat_schedule_update"])
    update.validate({"schedule_id": "12345678-1234-1234-1234-123456789abc", "target": request["target"]})
    assert list(update.iter_errors({"schedule_id": "saved-id", "update": {"target": request["target"]}}))
    assert list(create.iter_errors({**request, "commissioning_agent": "caller-cannot-set-this"}))
    for invalid in ({"target_kind": "host_runnable", "runnable": ""},
                    {"target_kind": "host_runnable"},
                    {**request["target"], "type": "host_runnable"}):
        assert list(create.iter_errors({**request, "target": invalid}))
    create.validate({**request, "target": {"target_kind": "session", "type": "current_session",
                                         "action": {"type": "prompt", "prompt": "Refresh the project evidence."}}})


@pytest.mark.parametrize("version", [None, "0.8.42", "0.9.0", "0.9.5", "0.9.4+unverified"])
def test_other_versions_receive_guidance_without_unverified_schemas(version):
    result = reference.meta_schedule_reference(version)
    assert "schedule_reference_version" not in result
    assert "meerkat_schedule_create:" not in result["content"]
    assert "No bundled contract matches" in result["native_authoring_schema"]


def test_current_reference_is_portable_without_sdk_network_or_checkout(tmp_path):
    package = tmp_path / "portable_reference"
    package.mkdir()
    (package / "__init__.py").write_text("")
    origin = Path(reference.__file__).parent
    for name in ("native_schedule_reference.py", "meta_schedule_reference_0_9_4.json"):
        shutil.copyfile(origin / name, package / name)
    script = """import importlib.util, json, sys
sys.path.insert(0, sys.argv[1])
assert importlib.util.find_spec('meerkat_meta') is None
from portable_reference.native_schedule_reference import meta_schedule_reference
r = meta_schedule_reference('0.9.4')
assert not r['observed_destination']
assert 'meerkat_schedule_create:' in r['content'] and '"host_runnable"' in r['content']
print(json.dumps({'bytes': len(r['content'].encode()), 'version': r['schedule_reference_version']}))
"""
    result = subprocess.run([sys.executable, "-I", "-S", "-c", script, str(tmp_path)],
                            env={"PATH": os.defpath}, cwd=tmp_path, capture_output=True,
                            text=True, timeout=10, check=True)
    proof = json.loads(result.stdout)
    assert proof["version"] == "0.9.4" and proof["bytes"] < reference.MAX_REFERENCE_BYTES


def test_invalid_current_reference_fails_without_silently_falling_back(tmp_path, monkeypatch):
    data = bundled()
    data["version"] = "0.9.5"
    (tmp_path / "meta_schedule_reference_0_9_4.json").write_text(json.dumps(data))
    monkeypatch.setattr(reference.resources, "files", lambda package: tmp_path)
    result = reference.meta_schedule_reference("0.9.4")
    assert result["ok"] is False and not result["observed_destination"]
    assert "content" not in result
