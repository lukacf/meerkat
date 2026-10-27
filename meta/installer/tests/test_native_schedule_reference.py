"""The offline reference is versioned documentation, never mutation authority."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

import launchkit.native_schedule_reference as reference_module
from launchkit.native_schedule_reference import (MAX_REFERENCE_BYTES, REFERENCE_VERSION,
                                                 load_schedule_reference, schedule_reference)


def test_pinned_authoring_schema_covers_actual_mutation_and_readback_fields():
    reference, digest = load_schedule_reference()
    assert reference["version"] == "0.8.42"
    assert reference["observed_destination"] is False
    assert len(digest) == 64
    schemas = reference["tool_schemas"]
    assert set(schemas) == {"meerkat_schedule_" + operation for operation in (
        "create", "get", "list", "update", "pause", "resume", "delete", "occurrences")}
    create = schemas["meerkat_schedule_create"]
    assert create["required"] == ["trigger", "target", "misfire_policy", "overlap_policy", "missing_target_policy"]
    assert create["additionalProperties"] is False
    assert set(create["properties"]) == {
        "name", "description", "trigger", "target", "misfire_policy", "overlap_policy",
        "missing_target_policy", "labels", "planning_horizon_days", "planning_horizon_occurrences"}
    update = schemas["meerkat_schedule_update"]
    assert update["required"] == ["schedule_id"]
    assert update["additionalProperties"] is False
    assert {key: value for key, value in update["properties"].items() if key != "schedule_id"} == create["properties"]
    assert update["properties"]["schedule_id"]["type"] == "string"
    for name in ("get", "pause", "resume", "delete", "occurrences"):
        schema = schemas["meerkat_schedule_" + name]
        assert schema["required"] == ["schedule_id"]
        assert set(schema["properties"]) == {"schedule_id"}
        assert schema["additionalProperties"] is False
    assert schemas["meerkat_schedule_list"]["properties"] == {}
    assert schemas["meerkat_schedule_list"]["additionalProperties"] is False
    assert set(reference["result_fields"]) == {"schedule_id", "phase", "revision", "labels"}
    assert reference["result_fields"]["phase"] == ["active", "paused", "deleted"]


def test_current_session_is_the_real_host_injected_shortcut_not_a_new_target():
    reference, _ = load_schedule_reference()
    for name in ("meerkat_schedule_create", "meerkat_schedule_update"):
        variants = reference["tool_schemas"][name]["properties"]["target"]["oneOf"]
        session = variants[0]
        assert session["properties"]["target_kind"] == {"const": "session"}
        assert session["properties"]["type"]["enum"] == [
            "exact_session", "resumable_session", "materialize_on_demand_session", "current_session"]
        assert session["required"] == ["target_kind", "type", "action"]
        assert "host_runnable" not in json.dumps(variants)
    sources = reference["sources"]
    assert all(source["version"] == REFERENCE_VERSION for source in sources)
    assert all(len(source["sha256"]) == 64 and len(source["commit"]) == 40 for source in sources)
    mobkit = [source for source in sources if source["project"] == "meerkat-mobkit"]
    assert mobkit and all(source["commit"] == "d328c2dd398e755293749884ab16326dffcec604" for source in mobkit)


def test_native_calendar_readback_documents_default_fields_without_inventing_year():
    reference, _ = load_schedule_reference()
    serialization = reference["result_serialization"]["calendar_trigger"]
    assert reference["reference_revision"] == reference_module.REFERENCE_REVISION
    assert serialization["serialized_default_any_fields"] == [
        "minute", "hour", "day_of_month", "month", "day_of_week"]
    assert serialization["default_any_value"] == {"kind": "any"}
    assert serialization["optional_year_omitted_when_none"] is True
    authored = {"type": "calendar", "timezone": "UTC",
                "minute": {"kind": "values", "values": [0]},
                "hour": {"kind": "values", "values": [18]}}
    native = dict(authored, day_of_month={"kind": "any"},
                  month={"kind": "any"}, day_of_week={"kind": "any"})
    # These are source-contract examples, not an executed destination receipt.
    documented = dict(authored)
    for field in serialization["serialized_default_any_fields"]:
        documented.setdefault(field, serialization["default_any_value"])
    assert authored != native
    assert documented == native
    assert "year" not in documented


@pytest.mark.asyncio
@pytest.mark.parametrize("include_nullable_data", [False, True])
async def test_pinned_inspect_error_decodes_through_real_sdk_without_losing_wire_context(
        include_nullable_data):
    from meerkat_mobkit.errors import RpcError
    from meerkat_mobkit.runtime import MobKitRuntime

    reference, _ = load_schedule_reference()
    contract = reference["observation_error_contract"]
    message = ("internal: inspect: session bridge mob error: mob lifecycle operation "
               "admission is still pending at observation_lane_saturated: member_status_observation")
    assert contract["rpc_error"] == {"code": -32603, "message": message}
    assert contract["native_data_omitted_when_none"] is True
    assert contract["sdk_exception"]["omitted_or_null_data_decodes_to_none"] is True
    wire_error = dict(contract["rpc_error"])
    if include_nullable_data:
        wire_error["data"] = None
    calls = []

    class OfflineTransport:
        async def send_async(self, request):
            calls.append(request)
            return {"jsonrpc": "2.0", "id": request["id"],
                    "error": wire_error}

    # Execute the actual SDK RPC decoder with an explicit offline transport;
    # no gateway, credentials, source application or agent work is admitted.
    runtime = object.__new__(MobKitRuntime)
    runtime._transport = OfflineTransport()
    with pytest.raises(RpcError) as raised:
        await runtime._rpc(contract["rpc_method"], {"identity": "synthetic:owner"})
    error = raised.value
    assert type(error) is RpcError
    assert error.code == -32603
    assert error.method == "mobkit/inspect_identity"
    assert len(calls) == 1
    assert error.request_id == calls[0]["id"]
    assert calls[0]["method"] == "mobkit/inspect_identity"
    assert calls[0]["params"] == {"identity": "synthetic:owner"}
    assert error.data is None
    assert str(error) == message
    # The failed generated classifier removed only the first wrapper. This
    # actual wire fixture cannot pass that shorthand comparison.
    assert str(error).split(": ", 1)[-1] != contract["native_stage"]
    assert contract["caller_stage"] == "observation"
    assert contract["native_intent"] == "member_status_observation"
    content = schedule_reference("0.8.42")["content"]
    assert message in content
    assert "RpcError(code, message" in content


@pytest.mark.parametrize("stage,method,code,message", [
    ("admission", "mobkit/inspect_identity", -32603, None),
    ("observation", "mobkit/dispatch", -32603, None),
    ("observation", "mobkit/inspect_identity", -32000, None),
    ("observation", "mobkit/inspect_identity", -32603, "observation_lane_saturated"),
    ("observation", "mobkit/inspect_identity", -32603, "provider observation_lane_saturated detail"),
])
def test_documented_exact_observation_category_does_not_match_unrelated_rpc_context(
        stage, method, code, message):
    from meerkat_mobkit.runtime import _rpc_error_from_payload

    reference, _ = load_schedule_reference()
    contract = reference["observation_error_contract"]
    expected = contract["rpc_error"]
    error = _rpc_error_from_payload(
        {"code": code, "message": expected["message"] if message is None else message},
        request_id="synthetic-unrelated-context", method=method)
    observed_context = (stage, error.method, error.code, str(error))
    documented_context = (contract["caller_stage"], contract["rpc_method"],
                          expected["code"], expected["message"])
    assert observed_context != documented_context


@pytest.mark.parametrize("installed", [None, "0.8.41", "0.8.43", "0.9.0"])
def test_mismatched_or_missing_sdk_cannot_receive_a_pretend_current_contract(installed):
    result = schedule_reference(installed)
    assert result["ok"] is False
    assert result["version_matches"] is False
    assert result["installed_sdk_version"] == installed
    assert result["reference_version"] == "0.8.42"
    assert result["observed_destination"] is False
    assert "content" not in result
    assert "sources" not in result
    assert "0.8.42" in result["error"]


def test_reference_is_portable_without_sdk_rust_checkout_or_network(tmp_path):
    package = tmp_path / "portable_reference"
    package.mkdir()
    (package / "__init__.py").write_text("")
    origin = Path(reference_module.__file__).parent
    for name in ("native_schedule_reference.py", "native_schedule_reference.json"):
        shutil.copyfile(origin / name, package / name)
    # Isolated Python cannot inherit the project/PYTHONPATH or import the SDK.
    script = """import hashlib, importlib.util, json, sys
sys.path.insert(0, sys.argv[1])
assert importlib.util.find_spec('meerkat_mobkit') is None
from portable_reference.native_schedule_reference import schedule_reference
result = schedule_reference('0.8.42')
assert result['ok'] and not result['observed_destination']
print(json.dumps({'digest':result['reference_sha256'],'bytes':len(result['content'].encode()),'sources':len(result['sources'])}))
"""
    child = subprocess.run([sys.executable, "-I", "-S", "-c", script, str(tmp_path)],
                           cwd=tmp_path, env={"PATH": os.defpath}, capture_output=True,
                           text=True, timeout=10, check=True)
    proof = json.loads(child.stdout)
    assert proof["digest"] == hashlib.sha256((package / "native_schedule_reference.json").read_bytes()).hexdigest()
    assert proof["bytes"] < MAX_REFERENCE_BYTES
    assert proof["sources"] >= 5
    assert "/Users/" not in (package / "native_schedule_reference.json").read_text()
