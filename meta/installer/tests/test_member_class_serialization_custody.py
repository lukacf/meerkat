"""Connect class serde custody, with inert completed bytes and no native effects."""
from copy import deepcopy
import json

import pytest

from launchkit.connect_application_plan import _assemble
from launchkit.connect_application_policy import completed_application_policy_inputs
from launchkit.connect_installer_caller import CustodyError, _prepared_plan_source
from test_helperless_completed_bundle import fixture
from test_prepared_helperless_source import completed_specimen
from test_helperless_application_callbacks import HelperlessFixture
from test_selected_member_classes import startup_fixture


@pytest.mark.parametrize("defaults", [{"purpose": "ordinary"}, {"callback_account": None},
    {"purpose": "ordinary", "callback_account": None}])
@pytest.mark.parametrize("empty", [False, True])
def test_explicit_native_defaults_survive_source_selection_completion_and_policy_join(tmp_path, defaults, empty):
    f = fixture(tmp_path, empty=empty, class_defaults=defaults)
    before = f.plan.read()
    selected = _assemble(deepcopy(f.plan_value))
    assert selected["application"]["member_classes"][0].items() >= defaults.items()
    assert _prepared_plan_source(f.plan_value, f.setup_value) is f.plan_value["application"]
    f.bundle.authenticate()
    staged = completed_application_policy_inputs(f.bundle, f.source_policy)
    assert json.loads(staged["local_policy_input"])["native_host"]["member_classes"] == (
        f.setup_value["native_owner"]["native_host"]["member_classes"])
    assert f.plan.read() == before and not f.calls


@pytest.mark.parametrize("field,value", [("purpose", "shared_workflow"),
    ("callback_account", {"kind": "declared_none"}),
    ("callback_account", {"kind": "selected", "authority": "original-authority", "subject": "original-subject"})])
def test_nondefault_class_fields_must_survive_the_completed_owner(tmp_path, field, value):
    plan, setup = completed_specimen(tmp_path)
    for classes in (plan["application"]["member_classes"], plan["owner"]["native_host"]["member_classes"],
            setup["native_owner"]["native_host"]["member_classes"]):
        classes[0][field] = deepcopy(value)
    _assemble(deepcopy(plan))
    assert _prepared_plan_source(plan, setup) is plan["application"]
    setup["native_owner"]["native_host"]["member_classes"][0].pop(field)
    with pytest.raises(CustodyError, match="selected member classes"):
        _prepared_plan_source(plan, setup)


@pytest.mark.parametrize("source_side", ["application", "owner"])
def test_plan_owner_and_application_can_differ_only_in_native_default_spelling(tmp_path, source_side):
    plan, setup = completed_specimen(tmp_path)
    classes = (plan["application"]["member_classes"] if source_side == "application"
        else plan["owner"]["native_host"]["member_classes"])
    classes[0].update(purpose="ordinary", callback_account=None)
    before = deepcopy(plan)
    _assemble(plan)
    assert _prepared_plan_source(plan, setup) is plan["application"]
    assert plan == before


@pytest.mark.parametrize("case", ["missing", "purpose", "account", "factory", "callbacks", "grants"])
def test_callback_classes_cannot_diverge_from_retained_completed_owner(tmp_path, case):
    f = HelperlessFixture(tmp_path, empty=True)
    classes = deepcopy(list(f.callbacks.member_classes.values()))
    completed = {"native_owner": {"native_host": {"member_classes": classes}}}
    f.callbacks.require_completed_classes(json.dumps(completed).encode())
    if case == "missing":
        completed["native_owner"]["native_host"].pop("member_classes")
    elif case == "purpose":
        classes[0]["purpose"] = "shared_workflow"
    elif case == "account":
        classes[0]["callback_account"] = {"kind": "declared_none"}
    elif case == "factory":
        classes[0]["factory"] = "foreign"
    elif case == "callbacks":
        classes[0]["callbacks"] = []
    else:
        classes[0]["grants"] = []
    with pytest.raises(CustodyError, match="selected callback classes"):
        f.callbacks.require_completed_classes(json.dumps(completed).encode())


@pytest.mark.parametrize("side", ["source", "completed"])
def test_callback_owner_join_preserves_explicit_native_defaults_without_rewriting(tmp_path, side):
    f = HelperlessFixture(tmp_path, empty=True)
    classes = deepcopy(list(f.callbacks.member_classes.values()))
    if side == "source":
        f.value["application"]["member_classes"][0].update(purpose="ordinary", callback_account=None)
        f.select()
    else:
        classes[0].update(purpose="ordinary", callback_account=None)
    completed = json.dumps({"native_owner": {"native_host": {"member_classes": classes}}}).encode()
    before = f.pin.read()
    f.callbacks.require_completed_classes(completed)
    assert f.pin.read() == before


def test_class_free_callback_selection_refuses_added_completed_classes(tmp_path):
    f = HelperlessFixture(tmp_path)
    classes = f.value["application"].pop("member_classes")
    f.select()
    f.callbacks.require_completed_classes(b'{"native_owner":{"native_host":{"member_classes":[]}}}')
    with pytest.raises(CustodyError, match="selected callback classes"):
        f.callbacks.require_completed_classes(json.dumps({
            "native_owner": {"native_host": {"member_classes": classes}}}).encode())


@pytest.mark.parametrize("helperless", [False, True])
@pytest.mark.asyncio
async def test_restore_refuses_foreign_completed_classes_before_runtime_construction(tmp_path, monkeypatch, helperless):
    f = startup_fixture(tmp_path, monkeypatch, "restored", helperless=helperless)
    from test_application_startup import pin
    setup = json.loads(f.setup.read())
    setup["native_owner"]["native_host"]["member_classes"][0]["factory"] = "foreign"
    f.setup = pin(f.setup.path, json.dumps(setup).encode())
    manifest = json.loads(f.manifest.read())
    for row in manifest["artifacts"]:
        if row["path"] == f.setup.path.name:
            row["sha256"] = f.setup.sha256
    f.manifest = pin(f.manifest.path, json.dumps(manifest).encode())
    f.selection["policy_manifest"] = f.manifest.pin()
    f.selection_file = pin(f.selection_file.path, json.dumps(f.selection).encode())
    f.owner = f.make_owner()
    with pytest.raises(CustodyError, match="selected callback classes"):
        await f.owner.prepare(f.app)
    assert not f.events and f.app.context.runtime is None and f.app._application_callbacks is None
