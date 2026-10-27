"""Compiler roster bytes against the actual generated host and genuine SDK types.

No gateway, provider, native owner or installation is started by these controls.
"""
import asyncio
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys

import pytest

from launchkit import compiler
from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.connect_installer_caller import MAX_INVENTORY_BYTES, _inventory_input


# Exercise this checkout's SDK, rather than silently skipping on machines
# without a developer's historical source-intake directory.
META_SOURCE = Path(__file__).resolve().parents[2] / "sdk/python"


def selected_blueprint(*, named=False, helper="research:2", instances=2):
    data = starter_blueprint().model_dump(mode="json")
    data["flows"] = []
    research = data["agent_classes"][1]
    research["instances"] = instances
    if named:
        research["members"] = [
            {"key": "first", "name": "First Å member", "instructions": "Keep the selected first instructions."},
            {"key": "chosen", "name": "Chosen second member", "instructions": "Keep the selected second instructions."},
        ]
        helper = "research:chosen"
    data["acquisition"] = {"helper_identity": helper, "helper_profile": "dedicated",
                           "helper_tools": "preparation"}
    return Blueprint.model_validate(data)


def generated_application(spec, tmp_path, monkeypatch):
    # Acquisition package admission stays closed. The roster itself is the
    # unchanged fixed host contract, generated through its ordinary package path.
    ordinary = spec.model_dump(mode="json")
    ordinary.pop("acquisition")
    files = compiler.generate_files(ordinary)
    for name, content in files.items():
        output = tmp_path / name
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(content)
    monkeypatch.syspath_prepend(str(tmp_path))
    for name in ("connectors", "mcp_connections", "application_configuration", "console_configuration",
                 "console_auth", "native_definition_continuity", "storage_setup"):
        monkeypatch.delitem(sys.modules, name, raising=False)
    loader = importlib.util.spec_from_file_location("inventory_generated_host", tmp_path / "app.py")
    host = importlib.util.module_from_spec(loader)
    loader.loader.exec_module(host)
    return host.Application(host.ApplicationContext(
        tmp_path, json.loads(files["deployment.json"]), asyncio.Event()))


@pytest.mark.parametrize("sdk", ["ordinary", "meta"])
@pytest.mark.parametrize("named,helper", [(False, "lead:main"), (False, "research:2"),
                                          (True, "research:chosen")])
def test_compiler_inventory_matches_actual_generated_roster_types(
        tmp_path, monkeypatch, sdk, named, helper):
    if sdk == "ordinary":
        return assert_compiler_inventory(tmp_path, monkeypatch, sdk, named, helper)
    # The full suite has already imported the installed Guide SDK during
    # collection. A fresh interpreter selects this checkout's SDK without
    # replacing modules still referenced by other tests in the parent process.
    source = META_SOURCE / "meerkat_meta/identity_first_models.py"
    assert source.is_file(), "The compiler contract tests require this checkout's Meta SDK source."
    before = {name: module for name, module in sys.modules.items()
              if name == "meerkat_meta" or name.startswith("meerkat_meta.")}
    env = dict(os.environ, PYTHONPATH=os.pathsep.join((str(META_SOURCE), str(META_SOURCE.parents[1] / "installer"))))
    env.pop("PYTHONOPTIMIZE", None)
    program = """import json, runpy, sys
from pathlib import Path
case = runpy.run_path(sys.argv[1])
with case["pytest"].MonkeyPatch.context() as patch:
    case["assert_compiler_inventory"](Path(sys.argv[2]), patch, "meta", json.loads(sys.argv[3]), sys.argv[4])
"""
    result = subprocess.run([sys.executable, "-c", program, str(Path(__file__).resolve()),
                             str(tmp_path), json.dumps(named), helper],
                            cwd=META_SOURCE.parents[1], env=env, capture_output=True, text=True, timeout=30)
    assert {name: module for name, module in sys.modules.items()
            if name == "meerkat_meta" or name.startswith("meerkat_meta.")} == before
    assert result.returncode == 0, result.stdout + result.stderr


def assert_compiler_inventory(tmp_path, monkeypatch, sdk, named, helper):
    spec = selected_blueprint(named=named, helper=helper)
    original = spec.model_dump(mode="json")
    payload = compiler.application_inventory_input(spec)
    application = generated_application(spec, tmp_path, monkeypatch)
    if sdk == "meta":
        source = META_SOURCE / "meerkat_meta/identity_first_models.py"
        assert source.is_file(), "The compiler contract tests require this checkout's Meta SDK source."
        from meerkat_meta.identity_first_models import DurableAgentSpec
        assert Path(sys.modules[DurableAgentSpec.__module__].__file__).resolve() == source.resolve()
        application._application_callbacks = type("NoNativeObservation", (), {
            "observe_roster_context": lambda self, context: None})()
    else:
        from meerkat_mobkit.identity_first_models import DurableAgentSpec
    roster = asyncio.run(application.roster(None))
    assert all(type(row) is DurableAgentSpec for row in roster)
    value = _inventory_input(payload)
    assert value == {"schema": 1, "mob_id": spec.project.slug,
                     "helper_identity": spec.acquisition.helper_identity,
                     "roster": [row.to_dict() for row in roster]}
    assert spec.model_dump(mode="json") == original
    if spec.acquisition.helper_identity != spec.topology.entrypoint:
        assert value["helper_identity"] != spec.topology.entrypoint
    if named:
        assert [(row["identity"], row["profile"], row["display_name"]) for row in value["roster"][1:3]] == [
            ("research:first", "research__first", "First Å member"),
            ("research:chosen", "research__chosen", "Chosen second member")]


def test_inventory_requires_explicit_acquisition_and_keeps_package_guard():
    with pytest.raises(ValueError, match="explicit.*helper"):
        compiler.application_inventory_input(starter_blueprint())
    spec = selected_blueprint()
    assert compiler.package_availability(spec)["available"] is False
    with pytest.raises(ValueError, match="connection setup helper.*compatible selected runtime"):
        compiler.generate_package(spec)


def test_inventory_has_exact_roster_fields_without_private_settings_or_instructions(monkeypatch):
    data = selected_blueprint().model_dump(mode="json")
    sentinel = "PRIVATE_SETTINGS_AND_INSTRUCTION_SENTINEL"
    monkeypatch.setenv("OPENAI_API_KEY", sentinel)
    monkeypatch.setenv("PRIVATE_TOKEN", sentinel)
    data["project"]["description"] = sentinel
    data["agent_classes"][1]["instructions"] = sentinel
    data["connections"] = [{"id": "private", "name": "Private connection", "transport": "stdio",
                            "command": "example", "args": [], "env": {"TOKEN": "${PRIVATE_TOKEN}"}}]
    # A reusable class without initial identities must not become an inventory member.
    empty = copy.deepcopy(data["agent_classes"][2])
    empty.update(id="empty", instances=0)
    data["agent_classes"].append(empty)
    payload = compiler.application_inventory_input(Blueprint.model_validate(data))
    value = json.loads(payload)
    assert set(value) == {"schema", "mob_id", "helper_identity", "roster"}
    assert sentinel.encode() not in payload and b"PRIVATE_TOKEN" not in payload
    assert b"application_parent" not in payload and b"realm_id" not in payload
    assert all(set(row) == {"identity", "profile", "addressability", "display_name", "labels",
                            "additional_instructions", "runtime_mode_override"} for row in value["roster"])
    assert all(row["additional_instructions"] == [] and row["addressability"] == "addressable"
               and row["runtime_mode_override"] == "turn_driven" for row in value["roster"])
    assert all(not row["identity"].startswith("empty:") for row in value["roster"])


def test_large_finite_inventory_preserves_every_row_and_fits_native_carrier():
    spec = selected_blueprint(instances=512, helper="research:512")
    payload = compiler.application_inventory_input(spec)
    value = _inventory_input(payload)
    assert len(value["roster"]) == 514 and len(payload) <= MAX_INVENTORY_BYTES
    assert [row["identity"] for row in value["roster"]][1:513] == [f"research:{i}" for i in range(1, 513)]
    assert payload == compiler.application_inventory_input(spec)


def test_inventory_rejects_valid_blueprint_that_exceeds_native_byte_limit():
    data = selected_blueprint(instances=512, helper="research:512").model_dump(mode="json")
    data["agent_classes"][1]["description"] = "Å" * 1000
    spec = Blueprint.model_validate(data)
    with pytest.raises(ValueError, match="256 KiB"):
        compiler.application_inventory_input(spec)


def test_helperless_inventory_preserves_classes_without_creating_initial_members():
    data = starter_blueprint().model_dump(mode="json")
    data["flows"] = []
    project = copy.deepcopy(data["agent_classes"][1])
    project.update(id="project", instances=0)
    data["agent_classes"].append(project)
    spec = Blueprint.model_validate(data)
    declarations = [{"id": "project", "profile": "application__project",
        "factory": "application:project", "callbacks": [], "grants": []}]
    original = copy.deepcopy(declarations)
    payload = compiler.application_inventory_input(spec, member_classes=declarations)
    value = _inventory_input(payload)
    assert set(value) == {"schema", "mob_id", "roster", "member_classes"}
    assert value["member_classes"] == original and declarations == original
    assert [row["identity"] for row in value["roster"]] == [
        "lead:main", "research:1", "research:2", "review:main"]
    assert not any(row["identity"].startswith("project:") for row in value["roster"])
    declarations[0]["grants"].append({"changed": "after serialization"})
    assert json.loads(payload)["member_classes"] == original


def test_legacy_inventory_bytes_keep_helper_field_order_and_no_class_default():
    spec = selected_blueprint()
    payload = compiler.application_inventory_input(spec)
    source = json.loads(payload)
    expected = {"schema": 1, "mob_id": spec.project.slug,
        "helper_identity": spec.acquisition.helper_identity, "roster": source["roster"]}
    assert payload == (json.dumps(expected, ensure_ascii=True,
        separators=(",", ":")) + "\n").encode()


@pytest.mark.parametrize("declarations", [{}, [None], ["project"], True])
def test_compiler_refuses_nonobject_native_class_selection(declarations):
    with pytest.raises(ValueError, match="native member-class list"):
        compiler.application_inventory_input(starter_blueprint(), member_classes=declarations)


def test_selected_class_bytes_count_toward_native_inventory_limit():
    with pytest.raises(ValueError, match="256 KiB"):
        compiler.application_inventory_input(starter_blueprint(), member_classes=[
            {"id": "project", "opaque_native_input": "x" * MAX_INVENTORY_BYTES}])
