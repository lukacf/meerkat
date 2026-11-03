"""Inert application custody controls, without starting native or resources.

The executor below writes synthetic receipts and artifacts. It tests only the
trusted caller boundary, not native schema validation or helper projection.
"""
import asyncio
import hashlib
import json
from pathlib import Path

import pytest

from launchkit.connect_installer_caller import (
    CustodyError, ExactFile, NativeInstallerCaller, ProcessOutcome,
)


def content(value):
    return (json.dumps(value, separators=(",", ":")) + "\n").encode()


def write(path, data, mode=0o600):
    path.write_bytes(data)
    path.chmod(mode)
    return ExactFile(path, hashlib.sha256(data).hexdigest())


class Fixture:
    def __init__(self, root, *, toml=False):
        self.root = root.resolve()
        self.root.chmod(0o700)
        self.output = self.root / "setup-bundle"
        self.attempt = self.root / "setup-attempt.json"
        self.calls = []
        self.changed = False
        self.mutate = lambda setup, policy, receipt: None
        self.after_files = lambda receipt: None
        self.failure = None
        self.gateway = write(self.root / "toolkit_gateway", b"inert, never executed\n", 0o700)
        # Inert custody specimens only, never native decoding or permission proof.
        self.helper_source_surface = write(self.root / "helper-surface.json",
            b'[{"name":"research"}]')
        self.helper_surface_bytes = b'[{"name":"research"},{"name":"setup_report"}]'
        definition = ({"format": "toml", "text": "# exact raw comment\nid = 'fixture'\n"}
                      if toml else {"format": "native", "definition": {"id": "fixture"}})
        self.input_value = {"schema": 3, "definition": definition,
            "realm": {"realm_id": "fixture", "context_root": str(self.root),
                "state_root": str(self.root / "state")},
            "helper": {"member": {"mob_id": "fixture", "role": "research", "member": "helper"}},
            "helper_source_surface": self.helper_source_surface.pin(),
            "native_members": [{"member": {"mob_id": "fixture", "role": "research", "member": "helper"},
                "logical_owner": "5a5f9973-3a12-4977-9c69-b1002c559d51", "context_generation": 7,
                "audience": {"audience": "principal", "principal": "5a5f9973-3a12-4977-9c69-b1002c559d51"},
                "initial_surface": {"name": self.helper_source_surface.path.name,
                    "sha256": self.helper_source_surface.sha256}}]}
        self.input = write(self.root / "setup-input.json", content(self.input_value))
        self.source = write(self.root / "selected-source.txt", b"selected application source\n")
        self.caller = NativeInstallerCaller(self.gateway, execute=self.execute,
            require_current=self.require_current,
            provision_attempt_path=self.root / "stable-provision-attempt.json")

    def require_current(self):
        if self.changed:
            raise CustodyError("The existing owner's stopped-host lease changed.")

    async def execute(self, argv):
        self.calls.append(argv)
        await asyncio.sleep(0)
        if self.failure is not None:
            return ProcessOutcome(1, content({"kind": "application_installer_failed",
                "command": argv[1], "phase": self.failure, "effects_may_have_occurred": True,
                "detail": "synthetic exited resource owner"}), b"retained stderr\n")
        resources = self.root / "prepared-resources"
        resources.mkdir(mode=0o700)
        handoff = write(resources / "prepare.stdout", b'{"mcpServers":{"inert":{}}}\n')
        policy_bytes = b'{"synthetic_compiled_policy":"opaque to Python"}\n'
        policy_digest = hashlib.sha256(policy_bytes).hexdigest()
        # Native typed serialization can include defaults missing from the
        # source object. Python must keep byte custody, not recreate that model.
        definition_source = json.loads(json.dumps(self.input_value["definition"]))
        if definition_source["format"] == "native":
            definition_source["definition"]["source_identity"] = None
        # Native application_setup hashes typed canonical input,
        # which can differ from the exact supplied input file's byte hash.
        # This synthetic value is opaque native output, not a Python replica
        # of canonical_json or proof of actual native schema qualification.
        native_input_hash = hashlib.sha256(b"synthetic typed canonical input with defaults").hexdigest()
        native_members = json.loads(json.dumps(self.input_value["native_members"]))
        native_members[0]["member"]["role"] = "prepared-role"
        native_members[0]["initial_surface"] = {"name": "helper-initial-surface.json",
            "sha256": hashlib.sha256(self.helper_surface_bytes).hexdigest()}
        setup = {"schema": 3, "input_sha256": native_input_hash,
            "definition_source": definition_source,
            "definition": {"id": "fixture", "profiles": {"prepared-role": {}}},
            "realm": {**self.input_value["realm"], "user_config_root": None},
            "source_helper_roster": {"identity": "helper", "profile": "research", "binding": None},
            "source_helper_member": self.input_value["helper"]["member"].copy(),
            "helper_source_surface": self.helper_source_surface.pin(),
            "native_members": native_members, "helper": {"member": {
                "mob_id": "fixture", "role": "prepared-role", "member": "helper"}},
            "resources": {"artifact_directory": str(resources), "handoff": handoff.pin()},
            "application_tool_policy": {"name": "application-tool-policy.json", "sha256": policy_digest}}
        receipt = {"kind": "application_setup_prepared", "helper": setup["helper"]["member"].copy(),
            "prepared_artifact_directory": str(resources)}
        self.mutate(setup, policy_bytes, receipt)
        self.output.mkdir(mode=0o700)
        setup_pin = write(self.output / "application-setup.json", content(setup))
        policy_pin = write(self.output / "application-tool-policy.json", policy_bytes)
        surface_pin = write(self.output / "helper-initial-surface.json", self.helper_surface_bytes)
        manifest_pin = write(self.output / "manifest.json", content({"schema": 1,
            "kind": "application_setup_preparation", "artifacts": [
                {"path": setup_pin.path.name, "sha256": setup_pin.sha256},
                {"path": policy_pin.path.name, "sha256": policy_pin.sha256},
                {"path": surface_pin.path.name, "sha256": surface_pin.sha256}]}))
        receipt.update(manifest=manifest_pin.pin(), setup=setup_pin.pin(),
                       application_tool_policy=policy_pin.pin(), helper_initial_surface=surface_pin.pin())
        self.after_files(receipt)
        return ProcessOutcome(0, content(receipt), b"retained stderr\n")

    def run(self):
        return asyncio.run(self.caller.prepare_application_setup(self.input,
            output_directory=self.output, attempt_path=self.attempt, source_files=(self.source,)))

    def completion(self):
        return json.loads(Path(str(self.attempt) + ".outcome.json").read_bytes())


def test_success_returns_exact_opaque_tuple_and_retains_restart_pins(tmp_path):
    f = Fixture(tmp_path)
    result = f.run()
    assert result.status == "receipt_verified"
    bundle = result.setup_bundle
    setup_bytes, policy_bytes = bundle.authenticate()
    assert policy_bytes == b'{"synthetic_compiled_policy":"opaque to Python"}\n'
    setup = json.loads(setup_bytes)
    assert setup["source_helper_roster"]["binding"] is None
    assert setup["realm"]["user_config_root"] is None
    assert "source_identity" in setup["definition_source"]["definition"]
    assert setup["input_sha256"] != f.input.sha256
    assert f.calls == [(str(f.gateway.path), "--prepare-application-setup",
        "--input", str(f.input.path), "--output", str(f.output))]
    saved = f.completion()
    assert saved["setup_bundle"] == bundle.pins()
    assert saved["setup_bundle"]["resource_handoff"]["sha256"] == hashlib.sha256(
        (tmp_path.resolve() / "prepared-resources/prepare.stdout").read_bytes()).hexdigest()
    assert saved["setup_bundle"]["directory"]["inode"] == f.output.stat().st_ino
    assert saved["process"]["returncode"] == 0
    assert saved["error_type"] is None


@pytest.mark.parametrize("name", ["attempt", "completion", "provision", "provision_completion"])
def test_fresh_output_aliases_cannot_consume_attempt_custody(tmp_path, name):
    f = Fixture(tmp_path)
    f.output = {"attempt": f.attempt, "completion": Path(str(f.attempt) + ".outcome.json"),
        "provision": f.caller.provision_attempt_path,
        "provision_completion": Path(str(f.caller.provision_attempt_path) + ".outcome.json")}[name]
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls
    assert not f.attempt.exists()


def test_setup_attempt_does_not_claim_future_native_provisioning_attempt(tmp_path):
    f = Fixture(tmp_path)
    f.attempt = f.caller.provision_attempt_path
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls
    assert not f.attempt.exists()


def test_symlink_output_parent_is_refused_before_executor_entry(tmp_path):
    f = Fixture(tmp_path)
    target = f.root / "actual-parent"
    target.mkdir(mode=0o700)
    link = f.root / "linked-parent"
    link.symlink_to(target, target_is_directory=True)
    f.output = link / "bundle"
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls


def test_unknown_partial_failure_never_replays_same_entered_attempt(tmp_path):
    f = Fixture(tmp_path)
    def partial(receipt):
        (f.output / "application-tool-policy.json").unlink()
    f.after_files = partial
    result = f.run()
    assert result.status == "outcome_unknown"
    assert result.setup_bundle is None
    assert f.output.exists()
    retained = f.attempt.read_bytes()
    # Output is already occupied, and independently the retained entered
    # attempt cannot be reused even if the owner selects another fresh output.
    f.output = f.root / "another-fresh-output"
    with pytest.raises(CustodyError):
        f.run()
    assert len(f.calls) == 1
    assert f.attempt.read_bytes() == retained
    assert f.completion()["process"]["returncode"] == 0


@pytest.mark.parametrize("change", ["helper", "resources", "input", "policy_reference", "handoff_path"])
def test_cross_bundle_substitutions_refuse_opaque_result(tmp_path, change):
    f = Fixture(tmp_path)
    def mutate(setup, policy, receipt):
        if change == "helper":
            receipt["helper"]["member"] = "someone-else"
        elif change == "resources":
            receipt["prepared_artifact_directory"] = str(f.root)
        elif change == "input":
            setup["input_sha256"] = "not-a-native-sha256"
        elif change == "policy_reference":
            setup["application_tool_policy"]["sha256"] = "0" * 64
        else:
            setup["resources"]["handoff"]["path"] = str(f.source.path)
    f.mutate = mutate
    result = f.run()
    assert result.status == "outcome_unknown"
    assert result.setup_bundle is None
    assert f.completion()["setup_bundle"] is None
    assert f.completion()["receipt"] is not None


def test_native_canonical_input_hash_is_preserved_separately_from_raw_pretty_input(tmp_path):
    f = Fixture(tmp_path)
    pretty = json.dumps(f.input_value, indent=2).encode() + b"\n"
    f.input = write(f.input.path, pretty)
    result = f.run()
    assert result.status == "receipt_verified"
    bundle = result.setup_bundle
    setup_bytes, _ = bundle.authenticate()
    setup = json.loads(setup_bytes)
    assert setup["input_sha256"] != hashlib.sha256(pretty).hexdigest()
    assert bundle.setup_input.sha256 == hashlib.sha256(pretty).hexdigest()
    assert bundle.setup_input.read() == pretty
    assert f.completion()["setup_bundle"]["setup_input"]["sha256"] == bundle.setup_input.sha256


def test_exact_raw_toml_cannot_be_normalized_or_changed(tmp_path):
    f = Fixture(tmp_path, toml=True)
    def mutate(setup, policy, receipt):
        setup["definition_source"]["text"] = "id = 'fixture'\n"
    f.mutate = mutate
    result = f.run()
    assert result.status == "outcome_unknown"
    assert result.setup_bundle is None


def test_handoff_symlink_or_changed_bytes_do_not_receive_authenticated_result(tmp_path):
    f = Fixture(tmp_path)
    def substitute(receipt):
        handoff = f.root / "prepared-resources/prepare.stdout"
        handoff.unlink()
        handoff.symlink_to(f.source.path)
    f.after_files = substitute
    result = f.run()
    assert result.status == "outcome_unknown"
    assert result.setup_bundle is None


@pytest.mark.parametrize("change", ["extra_file", "manifest_bool", "wrong_hash", "duplicate_row", "receipt_path"])
def test_manifest_and_receipt_boundaries_refuse_changed_closure(tmp_path, change):
    f = Fixture(tmp_path)
    def change_files(receipt):
        if change == "extra_file":
            write(f.output / "untracked.json", b"{}")
        elif change == "receipt_path":
            receipt["setup"]["path"] = str(f.source.path)
        else:
            manifest_path = f.output / "manifest.json"
            manifest = json.loads(manifest_path.read_bytes())
            if change == "manifest_bool":
                manifest["schema"] = True
            elif change == "wrong_hash":
                manifest["artifacts"][1]["sha256"] = "0" * 64
            else:
                manifest["artifacts"].append(manifest["artifacts"][0].copy())
            # An internally dishonest but byte-pinned manifest must still be
            # rejected; the receipt hash alone cannot prove bundle membership.
            receipt["manifest"] = write(manifest_path, content(manifest)).pin()
    f.after_files = change_files
    result = f.run()
    assert result.status == "outcome_unknown"
    assert result.setup_bundle is None


@pytest.mark.parametrize("change", ["lease", "input", "source", "output_parent"])
def test_source_currentness_changes_after_await_keep_receipt_historical(tmp_path, change):
    f = Fixture(tmp_path)
    if change == "output_parent":
        parent = f.root / "bundle-parent"
        parent.mkdir(mode=0o700)
        f.output = parent / "setup-bundle"
    def change_source(receipt):
        if change == "lease":
            f.changed = True
        elif change == "input":
            write(f.input.path, b'{"changed":true}')
        elif change == "source":
            write(f.source.path, b"changed\n")
        else:
            f.output.parent.chmod(0o755)
    f.after_files = change_source
    result = f.run()
    assert result.status == "source_changed"
    assert result.setup_bundle is None
    assert f.completion()["receipt"]["kind"] == "application_setup_prepared"
    assert f.completion()["process"]["returncode"] == 0


@pytest.mark.parametrize("phase", ["resource_preparation", "bundle_write"])
def test_new_failure_phases_are_retained_only_for_setup_command(tmp_path, phase):
    f = Fixture(tmp_path)
    f.failure = phase
    result = f.run()
    assert result.status == "native_failure"
    assert result.setup_bundle is None
    assert result.receipt["effects_may_have_occurred"] is True
    assert not f.output.exists()
    compiler_result = asyncio.run(f.caller.compile_application_tool_policy(f.input,
        output_path=f.root / "compiler-output.json", attempt_path=f.root / "compiler-attempt.json"))
    assert compiler_result.status == "outcome_unknown"
    assert compiler_result.setup_bundle is None


@pytest.mark.parametrize("change", ["policy_bytes", "handoff_bytes", "resource_directory", "output_directory"])
def test_restart_authentication_refuses_changed_retained_tuple(tmp_path, change):
    f = Fixture(tmp_path)
    bundle = f.run().setup_bundle
    if change == "policy_bytes":
        write(bundle.application_tool_policy.path, b"substituted policy\n")
    elif change == "handoff_bytes":
        write(bundle.resource_handoff.path, b"substituted resources\n")
    else:
        directory = bundle.resources_directory[0] if change == "resource_directory" else bundle.directory[0]
        retained = directory.with_name(directory.name + "-original")
        directory.rename(retained)
        directory.mkdir(mode=0o700)
        for old in retained.iterdir():
            write(directory / old.name, old.read_bytes())
    with pytest.raises(CustodyError):
        bundle.authenticate()
