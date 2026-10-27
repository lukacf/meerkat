"""Retained native inventory to compiler custody, using an inert executor.

No native policy validation, grant, host, provider or installation is exercised.
"""
import asyncio
import base64
from copy import deepcopy
import hashlib
import json
from pathlib import Path

import pytest

from launchkit.connect_installer_caller import CustodyError, InstallerOutcome, ProcessOutcome
from test_application_inventory import Fixture, encoded, pinned


class PolicyFixture(Fixture):
    def __init__(self, root):
        super().__init__(root)
        self.policy_value = {"provider_id": "application", "policy_id": "selected-tools", "revision": 3,
            "members": [{"member_identity": row["identity"], "grants": [
                {"tool_name": "setup_report", "action": "invoke", "consequence": "r3"}]
                if row["identity"] == self.input["helper_identity"] else []}
                for row in reversed(self.input["roster"])]}
        self.policy = pinned(self.root / "tool-policy.json", b"  " + encoded(self.policy_value))
        self.compile_input = self.root / "policy-compiler-input.json"
        self.compile_output = self.root / "compiled-policy.json"
        self.compile_attempt = self.root / "compile-attempt.json"
        self.compile_started = []

    async def execute(self, argv):
        if argv[1] != "--compile-application-tool-policy":
            return await super().execute(argv)
        self.calls.append(argv)
        self.compile_started.append(json.loads(self.compile_input.read_bytes()))
        await asyncio.sleep(0)
        self.after_await()
        output = pinned(self.compile_output, b'{"opaque_inert_compiled_specimen":true}\n')
        return ProcessOutcome(0, encoded({"kind": "application_tool_policy_compiled",
            "artifact": output.pin()}), b"inert compiler stderr\n")

    async def compile(self, inventory, **extra):
        return await self.caller.compile_selected_application_tool_policy(self.policy,
            source_id="accepted-application/tool-policy.json", inventory=inventory,
            compiler_input_path=self.compile_input, output_path=self.compile_output,
            attempt_path=self.compile_attempt, **extra)


async def inventory(f):
    return await f.caller.lower_application_inventory(f.compiler_input,
        attempt_path=f.attempt, source_files=(f.source,))


async def test_actual_returned_inventory_joins_only_exact_native_bindings_and_original_digest(tmp_path):
    f = PolicyFixture(tmp_path)
    before = f.policy.read()
    observed = await inventory(f)
    assert observed.inventory is not None
    # A mutable returned receipt is not the source of compiler member facts.
    observed.receipt["members"][0]["member"]["member"] = "forged-in-memory"
    result = await f.compile(observed.inventory)
    assert result.status == "receipt_verified"
    value = f.compile_started[0]
    native_by_identity = {row["identity"]: row["member"]["member"] for row in f.receipt["members"]}
    expected = deepcopy(f.policy_value)
    for row in expected["members"]:
        row["member_identity"] = native_by_identity[row["member_identity"]]
    assert value == {**expected, "source": {"source_id": "accepted-application/tool-policy.json",
        "source_digest": "sha256:" + hashlib.sha256(before).hexdigest()}}
    assert f.policy.read() == before
    assert f.compiler_input.read() == f.input_bytes
    assert len(f.calls) == 2 and f.calls[0][1] == "--lower-application-inventory"
    retained = json.loads(f.compile_attempt.read_bytes())
    assert retained["input_files"] == [pin.pin() for pin in (
        f.gateway, pinned_existing(f.compile_input),
        f.policy, f.compiler_input, f.source, observed.inventory.attempt, observed.inventory.completion)]
    assert result.inventory is None and not f.caller.provision_attempt_path.exists()


def pinned_existing(path):
    from launchkit.connect_installer_caller import ExactFile
    return ExactFile(path, hashlib.sha256(path.read_bytes()).hexdigest())


@pytest.mark.parametrize("target", ["input", "source", "attempt", "completion", "gateway", "owner"])
async def test_changed_inventory_original_refuses_before_compiler_input_or_admission(tmp_path, target):
    f = PolicyFixture(tmp_path)
    result = await inventory(f)
    if target == "owner":
        f.current = False
    else:
        pin = {"input": f.compiler_input, "source": f.source, "attempt": result.inventory.attempt,
            "completion": result.inventory.completion, "gateway": f.gateway}[target]
        pin.path.write_bytes(pin.path.read_bytes() + b" ")
    with pytest.raises(CustodyError):
        await f.compile(result.inventory)
    assert len(f.calls) == 1
    assert not f.compile_input.exists() and not f.compile_attempt.exists()


@pytest.mark.parametrize("change", ["foreign_caller", "competing_identities", "plain_receipt"])
async def test_inventory_cannot_be_a_browser_dict_or_other_owner_observation(tmp_path, change):
    f = PolicyFixture(tmp_path)
    result = await inventory(f)
    selected = result.inventory
    extra = {}
    if change == "foreign_caller":
        other_root = tmp_path / "other-owner"
        other_root.mkdir(mode=0o700)
        other = PolicyFixture(other_root)
        selected = (await inventory(other)).inventory
    elif change == "competing_identities":
        extra["selected_member_identities"] = [row["identity"] for row in f.input["roster"]]
    else:
        selected = result.receipt
    with pytest.raises(CustodyError):
        await f.compile(selected, **extra)
    assert len(f.calls) == 1 and not f.compile_attempt.exists()


async def test_unknown_inventory_result_never_has_a_compiler_carrier(tmp_path):
    f = PolicyFixture(tmp_path)
    f.process = RuntimeError("uncertain process entry")
    result = await inventory(f)
    assert result.status == "outcome_unknown" and result.inventory is None
    assert isinstance(result, InstallerOutcome)
    with pytest.raises(CustodyError, match="identities or retained inventory"):
        await f.compile(None)
    assert len(f.calls) == 1


async def test_changed_retained_inventory_after_projection_refuses_at_compiler_admission(tmp_path, monkeypatch):
    f = PolicyFixture(tmp_path)
    result = await inventory(f)
    compile_native = f.caller.compile_application_tool_policy
    async def change_then_admit(*args, **kwargs):
        result.inventory.completion.path.write_bytes(b"changed original completion")
        return await compile_native(*args, **kwargs)
    monkeypatch.setattr(f.caller, "compile_application_tool_policy", change_then_admit)
    with pytest.raises(CustodyError):
        await f.compile(result.inventory)
    assert f.compile_input.exists() and not f.compile_attempt.exists()
    assert len(f.calls) == 1


async def test_inventory_change_during_compiler_stays_historical_without_artifact(tmp_path):
    f = PolicyFixture(tmp_path)
    result = await inventory(f)
    f.after_await = lambda: f.compiler_input.path.write_bytes(b"changed source roster")
    compiled = await f.compile(result.inventory)
    assert compiled.status == "source_changed" and compiled.artifact is None
    assert len(f.calls) == 2
    assert json.loads(Path(str(f.compile_attempt) + ".outcome.json").read_bytes())["status"] == "source_changed"


async def test_original_command_receipt_and_raw_process_are_all_authenticated(tmp_path):
    f = PolicyFixture(tmp_path)
    result = await inventory(f)
    source, observed = result.inventory.authenticate()
    assert source == f.input and observed == f.receipt
    completion = json.loads(result.inventory.completion.read())
    assert base64.b64decode(completion["process"]["stdout_base64"]) == f.process.stdout
    assert not f.compile_input.exists() and len(f.calls) == 1


async def test_actual_owner_callback_stays_on_its_event_loop_during_inventory_and_compile(tmp_path):
    f = PolicyFixture(tmp_path)
    loop = asyncio.get_running_loop()
    current = f.caller.require_current
    def loop_owner():
        assert asyncio.get_running_loop() is loop
        current()
    f.caller.require_current = loop_owner
    result = await inventory(f)
    compiled = await f.compile(result.inventory)
    assert compiled.status == "receipt_verified" and len(f.calls) == 2


async def test_inventory_gateway_is_executable_custody_once_not_a_small_input(tmp_path, monkeypatch):
    f = PolicyFixture(tmp_path)
    result = await inventory(f)
    from launchkit.connect_installer_caller import ExactFile
    read = ExactFile.read
    def small_input_only(pin, **kwargs):
        if pin == f.gateway:
            pytest.fail("Gateway executable must be verified, not reread as a small input.")
        return read(pin, **kwargs)
    monkeypatch.setattr(ExactFile, "read", small_input_only)
    compiled = await f.compile(result.inventory)
    assert compiled.status == "receipt_verified"
    pins = json.loads(f.compile_attempt.read_bytes())["input_files"]
    assert pins.count(f.gateway.pin()) == 1


async def test_gateway_swap_during_output_validation_cannot_change_compiler_admission(tmp_path, monkeypatch):
    f = PolicyFixture(tmp_path)
    result = await inventory(f)
    alternate = pinned(f.root / "alternate-gateway", b"different inert gateway\n", 0o700)
    from launchkit import connect_installer_caller as caller
    private_parent = caller._private_parent
    def swap_during_output_check(path):
        private_parent(path)
        if path == f.compile_output:
            f.caller.gateway = alternate
    monkeypatch.setattr(caller, "_private_parent", swap_during_output_check)
    with pytest.raises(CustodyError, match="inventory gateway"):
        await f.compile(result.inventory)
    assert f.compile_input.exists() and not f.compile_attempt.exists()
    assert len(f.calls) == 1


@pytest.mark.parametrize("with_inventory", [True, False], ids=["retained-inventory", "explicit-native-identities"])
@pytest.mark.parametrize("changed_input", ["selection", "compiler-input"])
async def test_original_change_during_completion_fsync_cannot_return_current_artifact(
        tmp_path, monkeypatch, with_inventory, changed_input):
    f = PolicyFixture(tmp_path)
    selected_inventory, extra = None, {}
    if with_inventory:
        selected_inventory = (await inventory(f)).inventory
    else:
        # These opaque bindings are trusted-owner specimens. This branch does
        # not run inventory or infer native identifiers from durable source.
        bindings = {row["identity"]: row["member"]["member"] for row in f.receipt["members"]}
        for row in f.policy_value["members"]:
            row["member_identity"] = bindings[row["member_identity"]]
        f.policy = pinned(f.policy.path, b"  " + encoded(f.policy_value))
        extra["selected_member_identities"] = tuple(bindings.values())
    from launchkit import connect_installer_caller as caller
    write = caller._write_new
    changed_path = f.policy.path if changed_input == "selection" else f.compile_input
    def after_completion(path, value):
        write(path, value)
        if path == Path(str(f.compile_attempt) + ".outcome.json"):
            changed_path.write_bytes(b"changed original input during completion")
    monkeypatch.setattr(caller, "_write_new", after_completion)
    compiled = await f.compile(selected_inventory, **extra)
    assert compiled.status == "source_changed" and compiled.artifact is None
    assert compiled.selection is None and compiled.setup_bundle is None
    assert len(f.calls) == (2 if with_inventory else 1)
    historical = json.loads(Path(str(f.compile_attempt) + ".outcome.json").read_bytes())
    assert historical["status"] == "receipt_verified" and historical["artifact"] is not None


async def test_gateway_change_during_final_input_worker_withholds_artifact(tmp_path, monkeypatch):
    f = PolicyFixture(tmp_path)
    result = await inventory(f)
    alternate = pinned(f.root / "alternate-gateway", b"different inert gateway\n", 0o700)
    from launchkit import connect_installer_caller as caller
    read = caller.ExactFile.read
    completion_path = Path(str(f.compile_attempt) + ".outcome.json")
    def swap_after_last_original_read(pin, **kwargs):
        content = read(pin, **kwargs)
        if pin == result.inventory.completion and completion_path.exists():
            f.caller.gateway = alternate
        return content
    monkeypatch.setattr(caller.ExactFile, "read", swap_after_last_original_read)
    compiled = await f.compile(result.inventory)
    assert compiled.status == "source_changed" and compiled.artifact is None
    assert len(f.calls) == 2
    assert json.loads(completion_path.read_bytes())["status"] == "receipt_verified"
