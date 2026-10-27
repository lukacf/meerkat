"""Inert completed-bundle byte custody specimens, never native qualification.

The bindings, identity bytes, policy and filenames are supplied specimens. They
do not establish native decoding, canonical binding hashes, preparation finish,
policy authority, an SDK carrier or activation. No executor is entered.
"""
import copy
import asyncio
import hashlib
import json
from pathlib import Path
import threading

import pytest

from launchkit.connect_installer_caller import CustodyError, ExactFile, NativeInstallerCaller


def encoded(value):
    return json.dumps(value, ensure_ascii=True, separators=(",", ":")).encode()


def write(path, data):
    path.write_bytes(data)
    path.chmod(0o600)
    return ExactFile(path, hashlib.sha256(data).hexdigest())


class BundleFixture:
    def __init__(self, path, count=3, *, toml=True):
        self.root = path.resolve()
        self.root.chmod(0o700)
        self.resources = self.root / "resources"
        self.output = self.root / "completed"
        self.resources.mkdir(mode=0o700)
        self.output.mkdir(mode=0o700)
        self.current = True
        self.checks = 0
        self.on_check = lambda: None
        self.calls = []
        self.source = write(self.root / "selected-source.txt", b"retained application source\n")
        self.gateway = write(self.root / "never-executed", b"inert gateway specimen\n")
        self.caller = NativeInstallerCaller(self.gateway, execute=self.execute,
            require_current=self.require_current, provision_attempt_path=self.root / "untouched-attempt.json")
        members = [{"member": {"mob_id": "fixture", "role": "research", "member": f"member-{index}"},
            "logical_owner": "5A5F9973-3A12-4977-9C69-B1002C559D51", "context_generation": index + 1,
            "audience": {"audience": "principal", "principal": "5A5F9973-3A12-4977-9C69-B1002C559D51"}}
            for index in range(count)]
        helper = {"member": copy.deepcopy(members[-1]["member"]),
            "ingress_actor": "5A5F9973-3A12-4977-9C69-B1002C559D51",
            "logical_executor": "5A5F9973-3A12-4977-9C69-B1002C559D51",
            "controller_parent": [], "operation_basis": {"opaque_native_selection": True}}
        self.input_value = {"schema": 4,
            "realm": {"realm_id": "fixture", "context_root": str(self.root / "context"),
                "state_root": str(self.root / "state")},
            "definition": ({"format": "toml", "text": "# retain this exact comment\nid = 'fixture'\n"}
                if toml else {"format": "native", "definition": {"id": "fixture"}}),
            "roster": [{"identity": f"member-{index}", "profile": "research"} for index in range(count)],
            "members": members, "helper": helper, "helper_tools": "preparation",
            "helper_profile": "dedicated", "tool_policy": {"opaque_source_policy": True}}
        self.input = write(self.root / "input.json", json.dumps(self.input_value, indent=2).encode())
        self.handoff = write(self.resources / "prepare.stdout", b'{"retained_resource_result":"opaque"}\n')
        self.policy = b'{"native_compiled_policy":"opaque, including its own digest"}\n'
        output_members = copy.deepcopy(members)
        output_members[-1]["member"]["role"] = "native-projected-helper-role"
        self.surface_bytes = []
        for index, member in enumerate(output_members):
            # A supplied name specimen, not Python's native binding hash replica.
            name = "member-surface-" + hashlib.sha256(f"supplied-name-{index}".encode()).hexdigest() + ".json"
            data = encoded([{"name": f"tool-{index}", "provenance": {"opaque": [index, False]},
                "full_native_identity": {"catalog": "complete", "hidden": True}}])
            self.surface_bytes.append(data)
            pin = write(self.resources / name, data)
            member["initial_surface"] = {"name": name, "sha256": pin.sha256}
        final_helper = copy.deepcopy(helper)
        final_helper["member"] = copy.deepcopy(output_members[-1]["member"])
        final_helper["represented_subject"] = None
        # UUID spelling/default fields are native output, not normalized here.
        for member in output_members:
            member["logical_owner"] = member["logical_owner"].lower()
            member["audience"]["principal"] = member["audience"]["principal"].lower()
        definition_source = copy.deepcopy(self.input_value["definition"])
        if not toml:
            definition_source["definition"]["source_identity"] = None
        self.setup_value = {"schema": 4, "input_sha256": hashlib.sha256(b"opaque typed native hash").hexdigest(),
            "realm": {**self.input_value["realm"], "user_config_root": None},
            "definition_source": definition_source,
            "definition": {"opaque_native_definition": {"source_identity": None}},
            "source_helper_roster": {**self.input_value["roster"][-1], "addressability": "addressable",
                "display_name": None, "labels": {}, "context": None, "additional_instructions": [],
                "initial_message": None, "runtime_mode_override": None, "backend": None,
                "binding": None, "placement": None},
            "native_members": output_members, "source_helper_member": copy.deepcopy(helper["member"]),
            "helper": final_helper, "helper_tools": "preparation", "helper_profile": "dedicated",
            "server": {"opaque_native_server": True},
            "resources": {"artifact_directory": str(self.resources), "handoff": self.handoff.pin()},
            "application_tool_policy": {"name": "application-tool-policy.json",
                "sha256": hashlib.sha256(self.policy).hexdigest()}}
        self.publish()

    def publish(self):
        setup = write(self.output / "application-setup.json", encoded(self.setup_value))
        policy = write(self.output / "application-tool-policy.json", self.policy)
        rows = [setup.pin(), policy.pin()]
        for member in self.setup_value["native_members"]:
            reference = member["initial_surface"]
            original = self.resources / reference["name"]
            output = write(self.output / reference["name"], original.read_bytes())
            rows.append(output.pin())
        self.manifest_value = {"schema": 1, "kind": "application_setup_preparation",
            "artifacts": [{"path": Path(row["path"]).name, "sha256": row["sha256"]} for row in rows]}
        self.manifest = write(self.output / "manifest.json", encoded(self.manifest_value))

    def require_current(self):
        self.checks += 1
        self.on_check()
        if not self.current:
            raise CustodyError("The existing selected source or stopped-owner lease changed.")

    async def execute(self, argv):
        self.calls.append(argv)
        raise AssertionError("Completed-bundle custody must not enter an executor.")

    def retain(self):
        return self.caller.retain_completed_application_setup(self.manifest,
            setup_input=self.input, source_files=(self.source,))

    def repin_manifest(self):
        self.manifest = write(self.output / "manifest.json", encoded(self.manifest_value))


@pytest.mark.parametrize("count,toml", [(1, True), (19, False)])
def test_completed_native_inventory_retains_exact_bytes_without_cli_or_sdk(tmp_path, count, toml):
    f = BundleFixture(tmp_path, count, toml=toml)
    before = {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    bundle = f.retain()
    setup, policy = bundle.authenticate()
    assert setup == encoded(f.setup_value)
    assert policy == f.policy
    assert bundle.manifest == f.manifest
    assert bundle.setup_input == f.input
    assert bundle.source_files == (f.source,)
    assert len(bundle.member_surfaces) == count
    assert len(f.manifest_value["artifacts"]) == count + 2
    assert len(list(f.output.iterdir())) == count + 3
    for index, (original, output) in enumerate(bundle.member_surfaces):
        assert original.read() == output.read() == f.surface_bytes[index]
    decoded = json.loads(setup)
    assert decoded["input_sha256"] != f.input.sha256
    assert decoded["source_helper_member"] != decoded["helper"]["member"]
    assert decoded["native_members"][0]["logical_owner"] != f.input_value["members"][0]["logical_owner"]
    assert bundle.pins()["member_surfaces"][0]["original"]["path"].startswith(str(f.resources))
    assert "helper_source_surface" not in decoded
    assert f.checks == 7
    assert not f.calls and not f.caller.provision_attempt_path.exists()
    assert before == {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}


@pytest.mark.asyncio
async def test_completed_worker_reader_preserves_real_owner_and_filesystem_source(tmp_path):
    f = BundleFixture(tmp_path)
    owner_thread = threading.get_ident()
    original_owner = f.require_current
    checks = []
    def require_owner():
        assert threading.get_ident() == owner_thread
        original_owner()
    def require_filesystem():
        assert threading.get_ident() != owner_thread
        f.source.read()
        checks.append(threading.get_ident())
    f.caller.require_current = require_owner
    f.caller.require_filesystem_current = require_filesystem
    bundle = await asyncio.to_thread(f.caller._retain_completed_application_setup,
        f.manifest, setup_input=f.input, preparation_plan=None, source_files=(f.source,),
        _owner_fences=False)
    result = await asyncio.to_thread(bundle._authenticate_files)
    assert result == (encoded(f.setup_value), f.policy)
    assert bundle._require_current is require_owner
    assert bundle._require_filesystem_current is require_filesystem
    assert checks and f.checks == 0 and not f.calls
    f.current = False
    with pytest.raises(CustodyError, match="stopped-owner lease"):
        bundle.authenticate()


@pytest.mark.asyncio
async def test_completed_worker_reader_does_not_omit_other_accepted_source(tmp_path):
    f = BundleFixture(tmp_path)
    other = write(f.root / "other-accepted-resource.txt", b"accepted outside selected native pins\n")
    checks = 0
    def require_filesystem():
        nonlocal checks
        checks += 1
        if checks == 2:
            other.path.write_bytes(b"changed original accepted resource\n")
        other.read()
    f.caller.require_filesystem_current = require_filesystem
    with pytest.raises(CustodyError, match="selected bytes"):
        await asyncio.to_thread(f.caller._retain_completed_application_setup,
            f.manifest, setup_input=f.input, preparation_plan=None, source_files=(f.source,),
            _owner_fences=False)
    assert checks == 2 and f.checks == 0 and not f.calls


@pytest.mark.parametrize("change", ["extra_output", "missing_original", "missing_output", "duplicate_manifest",
    "missing_manifest_row", "traversal_reference", "duplicate_surface", "source_final_helper",
    "generation_type", "raw_toml", "realm_default", "legacy_surface"])
def test_completed_bundle_counterexamples_do_not_yield_retained_result(tmp_path, change):
    f = BundleFixture(tmp_path)
    first = f.setup_value["native_members"][0]["initial_surface"]
    if change == "extra_output":
        write(f.output / "unselected.json", b"{}")
    elif change == "missing_original":
        (f.resources / first["name"]).unlink()
    elif change == "missing_output":
        (f.output / first["name"]).unlink()
    elif change == "duplicate_manifest":
        f.manifest_value["artifacts"].append(f.manifest_value["artifacts"][-1].copy())
        f.repin_manifest()
    elif change == "missing_manifest_row":
        f.manifest_value["artifacts"].pop()
        f.repin_manifest()
    else:
        if change == "traversal_reference":
            first["name"] = "../selected-source.txt"
        elif change == "duplicate_surface":
            f.setup_value["native_members"][1]["initial_surface"] = copy.deepcopy(first)
        elif change == "source_final_helper":
            f.setup_value["source_helper_member"] = copy.deepcopy(f.setup_value["helper"]["member"])
        elif change == "generation_type":
            f.setup_value["native_members"][0]["context_generation"] = True
        elif change == "raw_toml":
            f.setup_value["definition_source"]["text"] = "id = 'fixture'\n"
        elif change == "realm_default":
            f.setup_value["realm"]["user_config_root"] = str(f.root / "other-root")
        else:
            f.setup_value["helper_source_surface"] = None
        if change == "traversal_reference":
            setup = write(f.output / "application-setup.json", encoded(f.setup_value))
            f.manifest_value["artifacts"][0]["sha256"] = setup.sha256
            f.repin_manifest()
        else:
            f.publish()
    with pytest.raises((CustodyError, FileNotFoundError)):
        f.retain()
    assert not f.calls and not f.caller.provision_attempt_path.exists()


def test_mutually_rebound_output_surface_cannot_replace_retained_original(tmp_path):
    f = BundleFixture(tmp_path)
    first = f.setup_value["native_members"][0]["initial_surface"]
    changed = b'[{"name":"same-looking-tool","different_native_provenance":true}]'
    first["sha256"] = hashlib.sha256(changed).hexdigest()
    setup = write(f.output / "application-setup.json", encoded(f.setup_value))
    output = write(f.output / first["name"], changed)
    for row in f.manifest_value["artifacts"]:
        if row["path"] == setup.path.name:
            row["sha256"] = setup.sha256
        if row["path"] == output.path.name:
            row["sha256"] = output.sha256
    f.repin_manifest()
    with pytest.raises(CustodyError):
        f.retain()
    assert not f.calls


def test_matching_oversized_original_and_output_surfaces_still_refuse(tmp_path):
    f = BundleFixture(tmp_path)
    first = f.setup_value["native_members"][0]["initial_surface"]
    data = b" " * (256 * 1024 + 1)
    original = write(f.resources / first["name"], data)
    first["sha256"] = original.sha256
    f.publish()
    with pytest.raises(CustodyError, match="bounded"):
        f.retain()
    assert not f.calls


@pytest.mark.parametrize("change", ["input", "source", "handoff", "original", "output", "lease"])
def test_every_reuse_reauthenticates_whole_source_and_native_byte_closure(tmp_path, change):
    f = BundleFixture(tmp_path)
    bundle = f.retain()
    if change == "lease":
        f.current = False
    else:
        path = {"input": f.input.path, "source": f.source.path, "handoff": f.handoff.path,
            "original": bundle.member_surfaces[0][0].path,
            "output": bundle.member_surfaces[0][1].path}[change]
        write(path, path.read_bytes() + b" ")
    with pytest.raises(CustodyError):
        bundle.authenticate()
    assert not f.calls


def test_end_currentness_check_cannot_hide_a_source_change_during_capture(tmp_path):
    f = BundleFixture(tmp_path)
    def change_after_reads():
        if f.checks == 3:
            write(f.source.path, b"changed after initial observation\n")
    f.on_check = change_after_reads
    with pytest.raises(CustodyError):
        f.retain()
    assert f.checks == 3 and not f.calls


def test_current_directory_identity_is_retained_even_when_file_bytes_are_restored(tmp_path):
    f = BundleFixture(tmp_path)
    bundle = f.retain()
    retained = f.root / "old-directory"
    f.output.rename(retained)
    f.output.mkdir(mode=0o700)
    for path in retained.iterdir():
        write(f.output / path.name, path.read_bytes())
    with pytest.raises(CustodyError, match="directories changed"):
        bundle.authenticate()
    assert not f.calls


def test_last_final_surface_read_cannot_return_under_stale_existing_owner(tmp_path, monkeypatch):
    import launchkit.connect_installer_caller as caller_module

    f = BundleFixture(tmp_path)
    before = {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    original_read = caller_module._completed_surface
    reads = 0
    def invalidate_after_last_read(pin):
        nonlocal reads
        data = original_read(pin)
        reads += 1
        if reads == 4 * len(f.setup_value["native_members"]):
            f.current = False
        return data
    monkeypatch.setattr(caller_module, "_completed_surface", invalidate_after_last_read)
    with pytest.raises(CustodyError, match="existing selected source or stopped-owner lease changed"):
        f.retain()
    assert reads == 4 * len(f.setup_value["native_members"])
    assert not f.current and not f.calls
    assert before == {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
