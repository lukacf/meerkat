"""Inert native inventory command and custody controls.

The executor never starts the synthetic gateway. Member and realm strings are
opaque fixtures, not examples or assertions of native identifier algorithms.
"""
import asyncio
import base64
from copy import deepcopy
import hashlib
import json
from pathlib import Path

import pytest

from launchkit.connect_installer_caller import (
    CustodyError, ExactFile, NativeInstallerCaller, ProcessOutcome,
)


def encoded(value):
    return (json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def pinned(path, data, mode=0o600):
    path.write_bytes(data)
    path.chmod(mode)
    return ExactFile(path, hashlib.sha256(data).hexdigest())


class Fixture:
    def __init__(self, root):
        self.root = root.resolve()
        self.root.chmod(0o700)
        self.calls = []
        self.current = True
        self.after_await = lambda: None
        self.gateway = pinned(self.root / "toolkit_gateway", b"inert; never executed\n", 0o700)
        # The exact roster is retained for the later compiler, including fields
        # this inventory adapter must not deserialize or reconstruct itself.
        self.input = {"schema": 1, "mob_id": "fixture-app", "helper_identity": "helper:chosen",
            "roster": [
                {"identity": "domain:original", "profile": "research",
                 "binding": {"opaque_native_binding": ["selected", 7]},
                 "backend": {"opaque_native_backend": "retained"},
                 "context": {"opaque_native_context": [True, None]},
                 "instructions": "Exact original instructions.\n"},
                {"identity": "helper:chosen", "profile": "setup", "instructions": "Exact helper instructions."},
                {"identity": "domain:unselected", "profile": "research"}]}
        self.input_bytes = encoded(self.input)
        self.compiler_input = pinned(self.root / "compiler-input.json", self.input_bytes)
        self.source = pinned(self.root / "source.json", encoded({"selected_source": [1, False, None]}))
        self.receipt = {"schema": 1, "mob_id": self.input["mob_id"],
            "realm_id": "opaque-native-realm-17", "helper_identity": self.input["helper_identity"],
            "members": [{"identity": row["identity"], "member": {"mob_id": self.input["mob_id"],
                "role": row["profile"], "member": f"opaque-native-member-{index}"}}
                for index, row in enumerate(self.input["roster"])]}
        self.process = ProcessOutcome(0, encoded(self.receipt), b"retained inert stderr\n")
        self.attempt = self.root / "inventory-attempt.json"
        self.caller = NativeInstallerCaller(self.gateway, execute=self.execute,
            require_current=self.require_current,
            provision_attempt_path=self.root / "stable-provision-attempt.json")

    def require_current(self):
        if not self.current:
            raise CustodyError("The existing selected owner changed.")

    async def execute(self, argv):
        self.calls.append(argv)
        assert json.loads(self.attempt.read_bytes())["status"] == "entered"
        await asyncio.sleep(0)
        self.after_await()
        if isinstance(self.process, BaseException):
            raise self.process
        return self.process

    def run(self, *, attempt=None):
        return asyncio.run(self.caller.lower_application_inventory(self.compiler_input,
            attempt_path=attempt or self.attempt, source_files=(self.source,)))

    def completion(self):
        return json.loads(Path(str(self.attempt) + ".outcome.json").read_bytes())


def test_exact_inventory_command_retains_order_helper_and_original_roster(tmp_path):
    f = Fixture(tmp_path)
    result = f.run()
    assert f.calls == [(str(f.gateway.path), "--lower-application-inventory", "--input", str(f.compiler_input.path))]
    assert result.status == "receipt_verified" and result.receipt == f.receipt
    assert result.artifact is None and result.selection is None and result.setup_bundle is None
    assert result.receipt["members"][1]["identity"] == result.receipt["helper_identity"]
    assert f.compiler_input.read() == f.input_bytes
    entered = json.loads(f.attempt.read_bytes())
    assert entered["input_files"] == [pin.pin() for pin in (f.gateway, f.compiler_input, f.source)]
    saved = f.completion()
    assert saved["receipt"] == f.receipt and saved["status"] == "receipt_verified"
    assert base64.b64decode(saved["process"]["stdout_base64"]) == f.process.stdout
    assert base64.b64decode(saved["process"]["stderr_base64"]) == f.process.stderr
    assert set(f.root.iterdir()) == {f.gateway.path, f.compiler_input.path, f.source.path,
        f.attempt, Path(str(f.attempt) + ".outcome.json")}
    assert not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("phase", ["arguments", "validation", "receipt"])
def test_known_inventory_refusal_has_exact_command_and_no_effects(tmp_path, phase):
    f = Fixture(tmp_path)
    refusal = {"kind": "application_installer_failed", "command": "--lower-application-inventory",
        "phase": phase, "effects_may_have_occurred": False, "detail": "native inventory refused"}
    f.process = ProcessOutcome(1, encoded(refusal), b"native detail\n")
    result = f.run()
    assert result.status == "native_failure" and result.receipt == refusal
    assert result.artifact is None and f.compiler_input.read() == f.input_bytes
    assert f.completion()["process"]["returncode"] == 1


@pytest.mark.parametrize("change", ["schema_bool", "unknown_field", "wrong_mob", "helper_alias",
    "empty_realm", "reordered_rows", "changed_role", "changed_member_mob", "duplicate_member", "extra_binding", "missing_row"])
def test_changed_or_ambiguous_inventory_response_remains_unknown(tmp_path, change):
    f = Fixture(tmp_path)
    receipt = deepcopy(f.receipt)
    if change == "schema_bool":
        receipt["schema"] = True
    elif change == "unknown_field":
        receipt["original_routes"] = "inventory cannot select originals"
    elif change == "wrong_mob":
        receipt["mob_id"] = "different-app"
    elif change == "helper_alias":
        receipt["helper_identity"] = receipt["members"][1]["member"]["member"]
    elif change == "empty_realm":
        receipt["realm_id"] = ""
    elif change == "reordered_rows":
        receipt["members"].reverse()
    elif change == "changed_role":
        receipt["members"][0]["member"]["role"] = "setup"
    elif change == "changed_member_mob":
        receipt["members"][0]["member"]["mob_id"] = "different-app"
    elif change == "duplicate_member":
        receipt["members"][2]["member"]["member"] = receipt["members"][0]["member"]["member"]
    elif change == "extra_binding":
        receipt["members"][0]["member"]["grant"] = "not an inventory field"
    else:
        receipt["members"].pop()
    f.process = ProcessOutcome(0, encoded(receipt), b"")
    result = f.run()
    assert result.status == "outcome_unknown" and result.receipt is None
    assert f.completion()["error_type"] == "CustodyError"
    assert len(f.calls) == 1 and not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("change", ["duplicate_json", "extra_document", "wrong_exit", "oversize_stdout",
    "failure_effects", "failure_phase", "failure_command"])
def test_unmatched_process_or_refusal_cannot_produce_verified_inventory(tmp_path, change):
    f = Fixture(tmp_path)
    if change == "duplicate_json":
        f.process = ProcessOutcome(0, encoded(f.receipt).replace(b'{"schema":1,', b'{"schema":1,"schema":1,'), b"")
    elif change == "extra_document":
        f.process = ProcessOutcome(0, encoded(f.receipt) + b"{}\n", b"")
    elif change == "wrong_exit":
        f.process = ProcessOutcome(1, encoded(f.receipt), b"")
    elif change == "oversize_stdout":
        f.process = ProcessOutcome(0, b" " * (1024 * 1024) + b"\n", b"")
    else:
        refusal = {"kind": "application_installer_failed", "command": "--lower-application-inventory",
            "phase": "validation", "effects_may_have_occurred": False, "detail": "refused"}
        refusal[{"failure_effects": "effects_may_have_occurred", "failure_phase": "phase", "failure_command": "command"}[change]] = {
            "failure_effects": True, "failure_phase": "owner_initialization", "failure_command": "--prepare-application-setup"}[change]
        f.process = ProcessOutcome(1, encoded(refusal), b"")
    result = f.run()
    assert result.status == "outcome_unknown" and result.receipt is None
    assert f.completion()["error_type"] == "CustodyError"


@pytest.mark.parametrize("change", ["schema_bool", "unknown_field", "empty_roster", "absent_helper", "duplicate_identity", "duplicate_json", "oversize"])
def test_invalid_selected_inventory_input_refuses_before_entry(tmp_path, change):
    f = Fixture(tmp_path)
    value = deepcopy(f.input)
    if change == "schema_bool":
        value["schema"] = True
    elif change == "unknown_field":
        value["acquisition"] = {"not": "inventory authority"}
    elif change == "empty_roster":
        value["roster"] = []
    elif change == "absent_helper":
        value["helper_identity"] = "not:selected"
    elif change == "duplicate_identity":
        value["roster"].append(deepcopy(value["roster"][0]))
    raw = encoded(value)
    if change == "duplicate_json":
        raw = raw.replace(b'{"schema":1,', b'{"schema":1,"schema":1,')
    elif change == "oversize":
        raw = b" " * (256 * 1024) + raw
    f.compiler_input = pinned(f.compiler_input.path, raw)
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls and not f.attempt.exists()


@pytest.mark.parametrize("target", ["input", "source", "owner", "private_parent"])
def test_late_changed_custody_keeps_inventory_historical(tmp_path, target):
    f = Fixture(tmp_path)
    if target == "input":
        f.after_await = lambda: f.compiler_input.path.write_bytes(f.input_bytes + b" ")
    elif target == "source":
        f.after_await = lambda: f.source.path.write_bytes(b"changed")
    elif target == "owner":
        f.after_await = lambda: setattr(f, "current", False)
    else:
        source_parent = f.root / "private-source"
        source_parent.mkdir(mode=0o700)
        f.source = pinned(source_parent / f.source.path.name, f.source.read())
        f.after_await = lambda: source_parent.chmod(0o755)
    result = f.run()
    assert result.status == "source_changed" and result.receipt == f.receipt
    assert result.artifact is None and result.selection is None and result.setup_bundle is None
    assert f.completion()["receipt"] == f.receipt and len(f.calls) == 1


def test_public_input_parent_refuses_before_entry(tmp_path):
    f = Fixture(tmp_path)
    input_parent = f.root / "public-input"
    input_parent.mkdir(mode=0o755)
    input_parent.chmod(0o755)
    f.compiler_input = pinned(input_parent / f.compiler_input.path.name, f.input_bytes)
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls and not f.attempt.exists()


def test_existing_attempt_and_stable_provision_claim_are_not_replayed_or_overwritten(tmp_path):
    f = Fixture(tmp_path)
    f.run()
    before = f.attempt.read_bytes()
    with pytest.raises(CustodyError):
        f.run()
    assert len(f.calls) == 1 and f.attempt.read_bytes() == before
    with pytest.raises(CustodyError):
        f.run(attempt=f.caller.provision_attempt_path)
    assert len(f.calls) == 1 and not f.caller.provision_attempt_path.exists()


def test_unknown_executor_outcome_retains_input_and_does_not_retry(tmp_path):
    f = Fixture(tmp_path)
    f.process = RuntimeError("inert executor uncertainty")
    result = f.run()
    assert result.status == "outcome_unknown" and result.receipt is None
    assert f.completion()["error_type"] == "RuntimeError"
    assert f.compiler_input.read() == f.input_bytes and len(f.calls) == 1


def helperless_fixture(root, *, empty=False):
    f = Fixture(root)
    del f.input["helper_identity"]
    del f.receipt["helper_identity"]
    # Remove the formerly selected helper from this application entirely.
    f.input["roster"].pop(1)
    f.receipt["members"].pop(1)
    f.input["member_classes"] = [{"id": "project", "profile": "application__project",
        "factory": "application:project", "callbacks": [], "grants": []}]
    if empty:
        f.input["roster"] = []
        f.receipt["members"] = []
    f.input_bytes = encoded(f.input)
    f.compiler_input = pinned(f.compiler_input.path, f.input_bytes)
    f.process = ProcessOutcome(0, encoded(f.receipt), b"")
    return f


@pytest.mark.parametrize("empty", [False, True])
def test_helperless_inventory_retains_classes_and_exact_ordered_receipt(tmp_path, empty):
    f = helperless_fixture(tmp_path, empty=empty)
    result = f.run()
    assert result.status == "receipt_verified" and result.receipt == f.receipt
    assert "helper_identity" not in result.receipt
    source, receipt = result.inventory.authenticate()
    assert source == f.input and receipt == f.receipt
    assert [row["identity"] for row in receipt["members"]] == [
        row["identity"] for row in source["roster"]]
    assert f.compiler_input.read() == f.input_bytes
    assert len(f.calls) == 1 and not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("omitted", [False, True])
def test_fixed_helperless_inventory_does_not_require_a_dynamic_class(tmp_path, omitted):
    f = helperless_fixture(tmp_path)
    f.input["member_classes"] = []
    if omitted:
        del f.input["member_classes"]
    f.compiler_input = pinned(f.compiler_input.path, encoded(f.input))
    result = f.run()
    assert result.status == "receipt_verified"
    assert result.inventory.authenticate() == (f.input, f.receipt)


@pytest.mark.parametrize("change", ["invented_helper", "null_helper", "invented_member", "echoed_classes"])
def test_helperless_inventory_rejects_an_invented_receipt(tmp_path, change):
    f = helperless_fixture(tmp_path, empty=True)
    if change == "invented_helper":
        f.receipt["helper_identity"] = "synthetic:helper"
    elif change == "null_helper":
        f.receipt["helper_identity"] = None
    elif change == "invented_member":
        f.receipt["members"].append({"identity": "project:placeholder", "member": {
            "mob_id": f.input["mob_id"], "role": "application__project", "member": "invented"}})
    else:
        f.receipt["member_classes"] = f.input["member_classes"]
    f.process = ProcessOutcome(0, encoded(f.receipt), b"")
    result = f.run()
    assert result.status == "outcome_unknown" and result.receipt is None and result.inventory is None
    assert f.completion()["error_type"] == "CustodyError"
    assert len(f.calls) == 1 and not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("change", ["missing_classes", "empty_classes", "null_classes", "null_helper"])
def test_invalid_helperless_inventory_refuses_before_command_entry(tmp_path, change):
    f = helperless_fixture(tmp_path, empty=True)
    if change == "missing_classes":
        del f.input["member_classes"]
    elif change == "empty_classes":
        f.input["member_classes"] = []
    elif change == "null_classes":
        f.input["member_classes"] = None
    else:
        f.input["helper_identity"] = None
    f.compiler_input = pinned(f.compiler_input.path, encoded(f.input))
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls and not f.attempt.exists()
    assert not f.caller.provision_attempt_path.exists()
