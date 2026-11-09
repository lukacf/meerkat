"""Compiler configuration joins with inert same-attempt output specimens.

These controls do not issue grants, finish native resources, publish an
activation or demonstrate Ready. Native preparation remains mandatory.
"""
import copy
import asyncio
import hashlib
import json
import threading

import pytest

from launchkit.compiler import completed_application_policy_inputs
from launchkit.connect_installer_caller import CustodyError
from test_application_preparation_custody import PreparationFixture
from test_connect_completed_application_setup import write


def fixture(tmp_path, *, source_policy_name="policy.json"):
    f = PreparationFixture(tmp_path)
    f.plan_value["acquisition"] = {
        "console_setup": {"origin": "https://console.example.test",
            "access_management": "requester_issued_leaves"},
        "backend_generation": "retained-workgraph-generation",
        "proposal_ceiling": {"actions": [
            {"action": "find_and_install", "resource": f.plan_value["application"]["members"][0]["member"]["member"]},
            {"action": "delegate", "resource": f.plan_value["application"]["members"][0]["member"]["member"]}],
            "services": ["selected-service"], "frontends": [], "output_audiences": ["actual-audience"]},
        "output_audience": ["actual-audience"],
        "limits": {"setup_timeout_ms": 30000, "max_workers": 4,
            "max_effect_attempts": 2, "admission_timeout_ms": 10000}}
    f.repin_plan()
    policy_path = f.root / source_policy_name
    if policy_path.parent != f.root:
        policy_path.parent.mkdir(mode=0o700)
    f.source_policy = write(policy_path, b'{ "retained_application_policy" : true }\n')
    f.bundle = f.caller.retain_completed_application_preparation(f.manifest,
        preparation_plan=f.plan, source_files=(f.source, f.source_policy))
    return f


def test_final_policy_uses_actual_finished_native_facts_and_original_artifacts(tmp_path):
    f = fixture(tmp_path)
    before = {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    result = completed_application_policy_inputs(f.bundle, f.source_policy)
    policy = json.loads(result["local_policy_input"])
    acquisition = json.loads(result["acquisition"].content)
    owner = f.setup_value["native_owner"]
    native = policy["native_host"]
    assert policy["schema"] == 7
    assert policy["deployment"] == owner["deployment"]
    assert policy["components"] == owner["components"] and policy["channels"] == owner["channels"]
    assert policy["source_policy_sha256"] == f.source_policy.sha256
    for name in ("realm", "grant_administrator", "remote_operator", "ingress_domain", "routes"):
        assert native[name] == owner["native_host"][name]
    assert native["members"] == f.setup_value["native_members"]
    assert native["members"] != f.plan_value["application"]["members"]
    assert native["members"][0]["setup_capability"]["parent"] == {"opaque_native_parent": "actual-agent-setup"}
    assert acquisition == {"schema": 1, **f.plan_value["acquisition"],
        "prepared_application_setup": {"name": "application-setup.json", "sha256": f.bundle.setup.sha256}}
    assert acquisition["console_setup"] == {
        "origin": "https://console.example.test", "access_management": "requester_issued_leaves"}
    assert native["setup"] == {"name": "native-setup.json",
        "sha256": hashlib.sha256(result["acquisition"].content.encode()).hexdigest()}
    assert native["application_tool_policy"] == {"name": "application-tool-policy.json",
        "sha256": f.bundle.application_tool_policy.sha256}
    assert result["source_artifacts"] == (f.source_policy, f.bundle.setup,
        f.bundle.application_tool_policy, *(output for _, output in f.bundle.member_surfaces))
    assert before == {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    assert not f.calls and not f.caller.provision_attempt_path.exists()


def test_retained_noncanonical_source_policy_stages_alias_without_changing_original_pin(tmp_path):
    from launchkit.connect_application_finalizer import ApplicationPolicyFinalizer

    f = fixture(tmp_path, source_policy_name="application/tool-policy.json")
    original = f.source_policy
    selected_pin, selected_bytes = original.pin(), original.read()
    inputs = completed_application_policy_inputs(f.bundle, original)
    assert inputs["source_artifacts"][0] is original
    assert inputs["source_artifact_names"] == ("policy.json",
        *(pin.path.name for pin in inputs["source_artifacts"][1:]))
    assert json.loads(inputs["local_policy_input"])["source_policy_sha256"] == original.sha256

    workspace = f.root / "finalizer"
    workspace.mkdir(mode=0o700)
    template = write(f.root / "activation-template.json", b'{"source":"retained"}\n')
    before = {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    finalizer = ApplicationPolicyFinalizer(f.caller, preparation_plan=f.plan,
        source_policy=original, activation_template=template, working_directory=workspace,
        bundle_path=f.root / "policy-bundle", activation_path=f.root / "activation.json",
        source_files=(f.source,))
    bundle, policy, staged, _ = finalizer._stage(f.manifest)

    assert original in bundle.source_files and finalizer.policy is original
    assert json.loads((workspace / "finalization.json").read_bytes())["source_files"][0] == selected_pin
    assert tuple(pin.path.name for pin in staged) == (
        *inputs["source_artifact_names"], inputs["acquisition"].path)
    assert staged[0].path == workspace / "artifacts/policy.json"
    assert staged[0].sha256 == original.sha256 and staged[0].read() == selected_bytes
    assert json.loads(policy.read())["source_policy_sha256"] == original.sha256
    assert original.path == f.root / "application/tool-policy.json"
    assert original.pin() == selected_pin and original.read() == selected_bytes
    assert all(path.read_bytes() == content for path, content in before.items())
    assert not (workspace / "artifacts/tool-policy.json").exists()
    assert not f.calls and not f.caller.provision_attempt_path.exists()
    assert not finalizer.bundle_path.exists() and not finalizer.activation_path.exists()


def test_foreign_same_byte_policy_pin_cannot_replace_retained_noncanonical_source(tmp_path):
    f = fixture(tmp_path, source_policy_name="application/tool-policy.json")
    original = f.source_policy
    admitted = completed_application_policy_inputs(f.bundle, original)
    foreign = write(f.root / "policy.json", original.read())
    assert foreign.sha256 == original.sha256 and foreign not in f.bundle.source_files
    before = {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    with pytest.raises(CustodyError, match="retained driver bundle and source policy"):
        completed_application_policy_inputs(f.bundle, foreign)
    assert admitted["source_artifacts"][0] is original
    assert f.bundle.source_files == (f.source, original)
    assert before == {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    assert not f.calls and not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("change", ["source-policy", "plan", "setup", "tool-policy", "surface", "owner"])
def test_late_changes_refuse_before_staging_or_native_execution(tmp_path, change):
    f = fixture(tmp_path)
    original = f.bundle._require_current
    count = 0
    def current():
        nonlocal count
        count += 1
        if count == 4:
            if change == "owner":
                f.current = False
            else:
                pin = {"source-policy": f.source_policy, "plan": f.plan,
                    "setup": f.bundle.setup, "tool-policy": f.bundle.application_tool_policy,
                    "surface": f.bundle.member_surfaces[0][1]}[change]
                pin.path.write_bytes(pin.path.read_bytes() + b"\n")
        original()
    object.__setattr__(f.bundle, "_require_current", current)
    with pytest.raises(CustodyError):
        completed_application_policy_inputs(f.bundle, f.source_policy)
    assert not f.calls and not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("change", ["missing-field", "added-schema", "missing-pin", "wrong-name", "raw-input"])
def test_incomplete_or_competing_configuration_is_not_completed(tmp_path, change):
    f = fixture(tmp_path)
    if change in {"missing-field", "added-schema"}:
        if change == "missing-field":
            del f.plan_value["acquisition"]["backend_generation"]
        else:
            f.plan_value["acquisition"]["schema"] = 1
        f.repin_plan()
        object.__setattr__(f.bundle, "preparation_plan", f.plan)
    elif change == "missing-pin":
        object.__setattr__(f.bundle, "source_files", (f.source,))
    elif change == "wrong-name":
        f.source_policy = write(f.root / "other-policy.json", f.source_policy.read())
    else:
        object.__setattr__(f.bundle, "preparation_plan", None)
    with pytest.raises(CustodyError):
        completed_application_policy_inputs(f.bundle, f.source_policy)
    assert not f.calls


def test_optional_console_absence_remains_absent_and_parameters_are_not_widened(tmp_path):
    f = fixture(tmp_path)
    del f.plan_value["acquisition"]["console_setup"]
    f.plan_value["acquisition"]["proposal_ceiling"]["actions"] = []
    f.repin_plan()
    object.__setattr__(f.bundle, "preparation_plan", f.plan)
    retained = copy.deepcopy(f.plan_value["acquisition"])
    result = completed_application_policy_inputs(f.bundle, f.source_policy)
    acquisition = json.loads(result["acquisition"].content)
    assert "console_setup" not in acquisition
    assert acquisition["proposal_ceiling"]["actions"] == []
    assert {k: v for k, v in acquisition.items() if k not in {"schema", "prepared_application_setup"}} == retained
    assert f.plan_value["acquisition"] == retained


def test_owner_loss_during_the_last_unchanged_source_read_is_not_returned(tmp_path):
    f = fixture(tmp_path)
    authenticate = f.bundle.authenticate
    read = f.source_policy.read
    completed_checks = 0
    def checked():
        nonlocal completed_checks
        result = authenticate()
        completed_checks += 1
        return result
    def late_read():
        result = read()
        if completed_checks == 2:
            f.current = False
        return result
    object.__setattr__(f.bundle, "authenticate", checked)
    object.__setattr__(f.source_policy, "read", late_read)
    with pytest.raises(CustodyError, match="stopped-owner lease"):
        completed_application_policy_inputs(f.bundle, f.source_policy)
    assert completed_checks == 2 and not f.calls


@pytest.mark.asyncio
async def test_policy_worker_projection_preserves_original_owner_and_file_guards(tmp_path):
    from launchkit.connect_application_policy import _completed_application_policy_inputs_files
    f = fixture(tmp_path)
    expected = completed_application_policy_inputs(f.bundle, f.source_policy)
    owner_thread = threading.get_ident()
    owner = f.bundle._require_current
    checks = []
    def require_owner():
        assert threading.get_ident() == owner_thread
        owner()
    def require_filesystem():
        assert threading.get_ident() != owner_thread
        f.source.read()
        checks.append(threading.get_ident())
    object.__setattr__(f.bundle, "_require_current", require_owner)
    object.__setattr__(f.bundle, "_require_filesystem_current", require_filesystem)
    result = await asyncio.to_thread(_completed_application_policy_inputs_files, f.bundle, f.source_policy)
    assert result["local_policy_input"] == expected["local_policy_input"]
    assert result["source_artifacts"] == expected["source_artifacts"]
    assert (result["acquisition"].path, result["acquisition"].as_bytes(), result["acquisition"].mode) == (
        expected["acquisition"].path, expected["acquisition"].as_bytes(), expected["acquisition"].mode)
    assert checks
    assert f.bundle._require_current is require_owner and not f.calls


@pytest.mark.asyncio
async def test_policy_worker_projection_rechecks_full_source_after_last_input_read(tmp_path):
    from launchkit.connect_application_policy import _completed_application_policy_inputs_files
    f = fixture(tmp_path)
    other = write(f.root / "other-accepted-resource.txt", b"original accepted resource\n")
    checks = 0
    completed_checks = 0
    def require_filesystem():
        nonlocal checks
        checks += 1
        other.read()
    object.__setattr__(f.bundle, "_require_filesystem_current", require_filesystem)
    authenticate = f.bundle._authenticate_files
    def authenticated():
        nonlocal completed_checks
        result = authenticate()
        completed_checks += 1
        return result
    object.__setattr__(f.bundle, "_authenticate_files", authenticated)
    read = f.source_policy.read
    def last_read():
        content = read()
        if completed_checks == 2:
            other.path.write_bytes(b"changed original accepted resource\n")
        return content
    object.__setattr__(f.source_policy, "read", last_read)
    with pytest.raises(CustodyError, match="selected bytes"):
        await asyncio.to_thread(_completed_application_policy_inputs_files, f.bundle, f.source_policy)
    assert completed_checks == 2 and checks > 0 and not f.calls
