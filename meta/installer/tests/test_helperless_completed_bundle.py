"""Helperless compiler artifact custody; specimens do not qualify native Ready."""
from copy import deepcopy
import hashlib
import json
from types import SimpleNamespace

import pytest

from launchkit.connect_application_policy import completed_application_policy_inputs
from launchkit.connect_installer_caller import CustodyError, NativeInstallerCaller
from test_connect_completed_application_setup import encoded, write
from test_prepared_helperless_source import completed_specimen


def fixture(tmp_path, *, empty=False, class_defaults=None):
    plan, setup = completed_specimen(tmp_path, empty=empty)
    if class_defaults is not None:
        # The native typed serializer omits these defaults in the completed
        # owner, while the original source pin must keep its selected bytes.
        for classes in (plan["application"]["member_classes"], plan["owner"]["native_host"]["member_classes"]):
            classes[0].update(class_defaults)
    f = SimpleNamespace(root=tmp_path.resolve(), plan_value=plan, setup_value=setup,
        calls=[], current=True)
    f.deployment = f.root / "installation"
    f.deployment.mkdir(mode=0o700)
    f.output = f.root / "fresh-output"
    f.output.mkdir(mode=0o700)
    f.source_policy = write(f.root / "policy.json", b'{"retained_source_policy":true}\n')
    gateway = write(f.root / "never-executed", b"inert gateway\n")
    async def execute(argv):
        f.calls.append(argv)
        raise AssertionError("File custody must not enter the native executor.")
    def current():
        if not f.current:
            raise CustodyError("The original owner changed.")
    f.caller = NativeInstallerCaller(gateway, execute=execute, require_current=current,
        provision_attempt_path=f.root / "untouched-attempt.json")
    setup["definition"] = {"opaque_native_definition": "unchanged"}
    f.policy = b'{"opaque_native_compiled_policy":true}\n'
    setup["application_tool_policy"] = {"name": "application-tool-policy.json",
        "sha256": hashlib.sha256(f.policy).hexdigest()}
    for i, row in enumerate(setup["native_members"]):
        data = encoded([{"name": f"tool-{i}", "opaque_native_identity": [i, False]}])
        reference = row["initial_surface"]
        reference["sha256"] = write(f.output / reference["name"], data).sha256
    f.plan = write(f.root / "plan.json", encoded(plan))
    def publish():
        pins = [write(f.output / "application-setup.json", encoded(setup)),
            write(f.output / "application-tool-policy.json", f.policy)]
        rows = [{"path": pin.path.name, "sha256": pin.sha256} for pin in pins]
        rows += [{"path": row["initial_surface"]["name"], "sha256": row["initial_surface"]["sha256"]}
                 for row in setup["native_members"]]
        f.manifest = write(f.output / "manifest.json", encoded({"schema": 1,
            "kind": "application_setup_preparation", "artifacts": rows}))
    def retain():
        return f.caller.retain_completed_application_preparation(f.manifest,
            preparation_plan=f.plan, source_files=(f.source_policy,))
    f.publish, f.retain = publish, retain
    publish()
    f.bundle = retain()
    return f


@pytest.mark.parametrize("empty", [False, True])
def test_helperless_output_retains_only_real_compiler_artifacts_and_schema2_carrier(tmp_path, empty):
    f = fixture(tmp_path, empty=empty)
    before = {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    bundle = f.bundle
    assert bundle.authenticate() == (encoded(f.setup_value), f.policy)
    assert bundle.resources_directory is None and bundle.resource_handoff is None
    assert "resources_directory" not in bundle.pins() and "resource_handoff" not in bundle.pins()
    assert bundle.artifact_directory == bundle.directory
    assert all(original == output for original, output in bundle.member_surfaces)
    assert len(bundle.member_surfaces) == (0 if empty else 2)
    staged = completed_application_policy_inputs(bundle, f.source_policy)
    carrier = json.loads(staged["acquisition"].as_bytes())
    assert carrier == {"schema": 2, "prepared_application_setup": {
        "name": "application-setup.json", "sha256": bundle.setup.sha256}}
    native = json.loads(staged["local_policy_input"])["native_host"]
    assert native["member_classes"] == f.plan_value["application"]["member_classes"]
    assert native["members"] == f.setup_value["native_members"]
    assert native["setup"]["sha256"] == hashlib.sha256(staged["acquisition"].as_bytes()).hexdigest()
    assert before == {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    assert not f.calls and not (f.root / "untouched-attempt.json").exists()


@pytest.mark.parametrize("change", ["directory", "relative_directory", "missing_directory", "resources", "server",
    "helper", "surface_bytes", "missing_surface", "extra_file", "classes", "extra_classes_field"])
def test_helperless_output_substitution_is_refused_with_repinned_manifest(tmp_path, change):
    f = fixture(tmp_path)
    if change == "directory":
        other = f.root / "other"
        other.mkdir(mode=0o700)
        f.setup_value["artifact_directory"] = str(other)
    elif change == "relative_directory":
        f.setup_value["artifact_directory"] = "."
    elif change == "missing_directory":
        del f.setup_value["artifact_directory"]
    elif change in {"resources", "server", "helper"}:
        f.setup_value[change] = None
    elif change in {"surface_bytes", "missing_surface"}:
        path = f.output / f.setup_value["native_members"][0]["initial_surface"]["name"]
        if change == "missing_surface":
            path.unlink()
        else:
            write(path, b"substituted native surface")
    elif change == "extra_file":
        write(f.output / "prepare.stdout", b"invented resource handoff")
    elif change == "classes":
        f.setup_value["native_owner"]["native_host"]["member_classes"][0]["factory"] = "different"
    else:
        f.setup_value["member_classes"] = deepcopy(f.plan_value["application"]["member_classes"])
    f.publish()
    with pytest.raises((CustodyError, FileNotFoundError)):
        f.retain()
    assert not f.calls


@pytest.mark.parametrize("change", ["plan", "manifest", "surface", "owner"])
def test_helperless_reuse_checks_the_same_source_files_and_owner(tmp_path, change):
    f = fixture(tmp_path)
    if change == "owner":
        f.current = False
    else:
        path = {"plan": f.plan.path, "manifest": f.manifest.path,
            "surface": f.bundle.member_surfaces[0][1].path}[change]
        write(path, path.read_bytes() + b" ")
    with pytest.raises(CustodyError):
        f.bundle.authenticate()
    assert not f.calls


@pytest.mark.parametrize("empty", [False, True])
def test_helperless_finalizer_stages_same_native_output_without_resource_handoff(tmp_path, empty):
    from launchkit.connect_application_finalizer import ApplicationPolicyFinalizer
    f = fixture(tmp_path, empty=empty)
    work = f.root / "finalizer"
    work.mkdir(mode=0o700)
    template = write(f.root / "activation-template.json", b'{"source":"retained"}\n')
    finalizer = ApplicationPolicyFinalizer(f.caller, preparation_plan=f.plan,
        source_policy=f.source_policy, activation_template=template, working_directory=work,
        bundle_path=f.deployment / "policy-bundle", activation_path=f.deployment / "activation.json",
        source_files=())
    bundle, policy, artifacts, _ = finalizer._stage(f.manifest)
    assert bundle.resource_handoff is None
    assert json.loads(policy.read())["native_host"]["members"] == f.setup_value["native_members"]
    assert json.loads(next(pin.read() for pin in artifacts if pin.path.name == "native-setup.json"))["schema"] == 2
    assert {pin.path.name for pin in artifacts} == {
        "policy.json", "application-setup.json", "application-tool-policy.json", "native-setup.json",
        *(row["initial_surface"]["name"] for row in f.setup_value["native_members"])}
    assert not f.calls and not (f.deployment / "current").exists()
