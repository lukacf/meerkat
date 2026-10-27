"""Inert capability caller custody controls, without native activation.

Synthetic private JSON files and an inert executor exercise only Python's
existing stopped-host caller boundary. Native lineage values remain opaque.
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


MODES = {
    "agent": ("configure_agent_setup_capability", "--configure-agent-setup-capability",
              "agent_setup_capability_configured", "setup_capability"),
    "report": ("configure_helper_report_capability", "--configure-helper-report-capability",
               "helper_report_capability_configured", "report_capability"),
}


def content(value):
    return (json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def write(path, data, mode=0o600):
    path.write_bytes(data)
    path.chmod(mode)
    return ExactFile(path, hashlib.sha256(data).hexdigest())


class Fixture:
    def __init__(self, root, mode):
        self.root = root.resolve()
        self.root.chmod(0o700)
        self.mode = mode
        self.calls = []
        self.current = True
        self.after_await = lambda: None
        self.gateway = write(self.root / "toolkit_gateway", b"inert; never executed\n", 0o700)
        self.artifacts = tuple(write(self.root / name, content({"opaque_native_artifact": name}))
                               for name in ("activation.json", "policy.json", "local.json"))
        self.selected = {"deployment_root": str(self.root / "deployment"),
            **{name: pin.pin() for name, pin in zip(
                ("activation", "policy_manifest", "local_policy"), self.artifacts)}}
        self.selection = write(self.root / "selection.json", content(self.selected))
        self.member = {"mob_id": "fixture", "role": "research", "member": "helper"}
        self.request_value = {"member": self.member,
            "issuance_id": "91698950-8886-40c9-bc63-119593699686",
            **({"route": "research-route"} if mode == "agent" else {})}
        self.request = write(self.root / "request.json", content(self.request_value))
        self.source = write(self.root / "selected-source.txt", b"exact selected application source\n")
        self.parent = {"opaque_native_lineage": {"uninterpreted": [True, None, 7, {"native": "value"}]}}
        capability = ({"route": self.request_value["route"], "parent": self.parent}
                      if mode == "agent" else {"parent": [self.parent]})
        self.receipt = {"kind": MODES[mode][2], "member": deepcopy(self.member),
                        MODES[mode][3]: capability}
        self.process = ProcessOutcome(0, content(self.receipt), b"inert retained stderr\n")
        self.attempt = self.root / "capability-attempt.json"
        self.caller = NativeInstallerCaller(self.gateway, execute=self.execute,
            require_current=self.require_current,
            provision_attempt_path=self.root / "stable-provision-attempt.json")

    def require_current(self):
        if not self.current:
            raise CustodyError("The existing stopped-host owner changed.")

    async def execute(self, argv):
        self.calls.append(argv)
        assert json.loads(self.attempt.read_bytes())["status"] == "entered"
        await asyncio.sleep(0)
        self.after_await()
        if isinstance(self.process, BaseException):
            raise self.process
        return self.process

    async def invoke(self, *, attempt=None):
        return await getattr(self.caller, MODES[self.mode][0])(
            self.selection, self.request, attempt_path=attempt or self.attempt,
            source_files=(self.source,))

    def run(self):
        return asyncio.run(self.invoke())

    def completion(self):
        return json.loads(Path(str(self.attempt) + ".outcome.json").read_bytes())


@pytest.mark.parametrize("mode", MODES)
def test_capability_method_seam_preserves_exact_request_and_opaque_native_receipt(tmp_path, mode):
    f = Fixture(tmp_path, mode)
    result = f.run()
    assert result.status == "receipt_verified"
    assert result.receipt == f.receipt
    assert result.artifact is None and result.selection is None and result.setup_bundle is None
    assert f.calls == [(str(f.gateway.path), MODES[mode][1],
                        "--selection", str(f.selection.path), "--request", str(f.request.path))]
    entered = json.loads(f.attempt.read_bytes())
    expected = (f.gateway, f.selection, f.request, *f.artifacts, f.source)
    assert {tuple(pin.items()) for pin in entered["input_files"]} == {
        tuple(pin.pin().items()) for pin in expected}
    assert f.attempt.stat().st_mode & 0o777 == 0o600
    assert Path(str(f.attempt) + ".outcome.json").stat().st_mode & 0o777 == 0o600
    saved = f.completion()
    assert saved["status"] == "receipt_verified" and saved["receipt"] == f.receipt
    assert base64.b64decode(saved["process"]["stdout_base64"]) == f.process.stdout
    assert base64.b64decode(saved["process"]["stderr_base64"]) == f.process.stderr
    assert not f.caller.provision_attempt_path.exists()


def select_report_fields(f, identifiers, parents):
    # Synthetic receipts exercise only exact request/response custody.
    f.request_value.update(identifiers)
    f.request = write(f.request.path, content(f.request_value))
    f.receipt["report_capability"].update(parents)
    f.process = ProcessOutcome(0, content(f.receipt), b"inert optional report receipt\n")


@pytest.mark.parametrize("identifiers,parents", [
    ({}, {}), ({"account_issuance_id": None, "public_issuance_id": None}, {}),
    ({"account_issuance_id": "account-id"}, {"account_parent": [{"opaque_account_lineage": [7, True]}]}),
    ({"public_issuance_id": "public-id"}, {"public_parent": [{"opaque_public_lineage": [None, 9]}]}),
    ({"account_issuance_id": "account-id", "public_issuance_id": "public-id"},
     {"account_parent": [{"opaque_account_lineage": [7, True]}],
      "public_parent": [{"opaque_public_lineage": [None, 9]}]}),
])
def test_direct_report_configuration_preserves_only_selected_optional_parents(tmp_path, identifiers, parents):
    f = Fixture(tmp_path, "report")
    select_report_fields(f, identifiers, parents)
    selected_request = f.request.read()
    result = f.run()
    assert result.status == "receipt_verified" and result.receipt == f.receipt
    assert f.request.read() == selected_request
    assert f.completion()["receipt"] == f.receipt
    assert base64.b64decode(f.completion()["process"]["stdout_base64"]) == f.process.stdout
    assert f.calls == [(str(f.gateway.path), "--configure-helper-report-capability",
        "--selection", str(f.selection.path), "--request", str(f.request.path))]
    assert result.artifact is None and result.selection is None and result.setup_bundle is None
    assert not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("field", ["account_issuance_id", "public_issuance_id"])
@pytest.mark.parametrize("value", [True, 1, [], {}, "", " ", " padded "])
def test_direct_optional_report_ids_refuse_invalid_values_before_entry(tmp_path, field, value):
    f = Fixture(tmp_path, "report")
    select_report_fields(f, {field: value}, {})
    with pytest.raises(CustodyError, match="exact nonempty text or null"):
        f.run()
    assert not f.calls and not f.attempt.exists()


@pytest.mark.parametrize("field,other", [
    ("account_issuance_id", "issuance_id"), ("public_issuance_id", "issuance_id"),
    ("public_issuance_id", "account_issuance_id"),
])
def test_direct_optional_report_ids_must_be_distinct(tmp_path, field, other):
    f = Fixture(tmp_path, "report")
    identifiers = {"issuance_id": f.request_value["issuance_id"],
        "account_issuance_id": "account-id", "public_issuance_id": "public-id"}
    identifiers[field] = identifiers[other]
    select_report_fields(f, identifiers, {})
    with pytest.raises(CustodyError, match="distinct issuance IDs"):
        f.run()
    assert not f.calls and not f.attempt.exists()


@pytest.mark.parametrize("field", ["account_issuance_id", "public_issuance_id"])
def test_agent_setup_request_does_not_inherit_optional_report_fields(tmp_path, field):
    f = Fixture(tmp_path, "agent")
    f.request_value[field] = "not-an-agent-setup-field"
    f.request = write(f.request.path, content(f.request_value))
    with pytest.raises(CustodyError, match="exact native request fields"):
        f.run()
    assert not f.calls and not f.attempt.exists()


@pytest.mark.parametrize("field", ["account_parent", "public_parent"])
@pytest.mark.parametrize("change", ["missing", "unrequested", "null", "empty", "not_list", "empty_object", "not_object", "duplicate_report", "duplicate_optional"])
def test_optional_native_receipt_shape_and_selected_parent_custody_are_closed(tmp_path, field, change):
    f = Fixture(tmp_path, "report")
    identifiers = {"account_issuance_id": "account-id", "public_issuance_id": "public-id"}
    parents = {"account_parent": [{"opaque_account": "kept"}], "public_parent": [{"opaque_public": "kept"}]}
    if change == "unrequested":
        identifiers[field.replace("_parent", "_issuance_id")] = None
    elif change == "missing":
        del parents[field]
    elif change == "duplicate_report":
        parents[field] = deepcopy(f.receipt["report_capability"]["parent"])
    elif change == "duplicate_optional":
        parents[field] = deepcopy(parents["public_parent" if field == "account_parent" else "account_parent"])
    else:
        parents[field] = {"null": None, "empty": [], "not_list": {},
            "empty_object": [{}], "not_object": ["not-a-lineage"]}[change]
    select_report_fields(f, identifiers, parents)
    result = f.run()
    assert result.status == "outcome_unknown" and result.receipt is None
    assert result.artifact is None and result.setup_bundle is None
    assert f.completion()["status"] == "outcome_unknown"
    assert base64.b64decode(f.completion()["process"]["stdout_base64"]) == f.process.stdout
    assert len(f.calls) == 1


def test_optional_report_receipt_stays_historical_after_owner_change_without_replay(tmp_path):
    f = Fixture(tmp_path, "report")
    select_report_fields(f, {"public_issuance_id": "public-id"},
        {"public_parent": [{"opaque_public_lineage": "kept"}]})
    f.after_await = lambda: setattr(f, "current", False)
    result = f.run()
    assert result.status == "source_changed" and result.receipt == f.receipt
    assert result.artifact is None and result.selection is None and result.setup_bundle is None
    retained = f.attempt.read_bytes()
    completion = Path(str(f.attempt) + ".outcome.json").read_bytes()
    f.current = True
    with pytest.raises(CustodyError):
        f.run()
    assert len(f.calls) == 1 and f.attempt.read_bytes() == retained
    assert Path(str(f.attempt) + ".outcome.json").read_bytes() == completion


@pytest.mark.parametrize("mode", MODES)
@pytest.mark.parametrize("stage", ("before", "after"))
@pytest.mark.parametrize("target", ("selection", "request", "source"))
@pytest.mark.parametrize("change", ("bytes", "private_mode"))
def test_exact_private_input_custody_is_checked_before_and_after_await(tmp_path, mode, stage, target, change):
    f = Fixture(tmp_path, mode)
    pin = getattr(f, target)

    def mutate():
        if change == "private_mode":
            pin.path.chmod(0o644)
        else:
            pin.path.write_bytes(pin.path.read_bytes() + b" ")

    if stage == "before":
        mutate()
        with pytest.raises(CustodyError):
            f.run()
        assert not f.calls and not f.attempt.exists()
    else:
        f.after_await = mutate
        result = f.run()
        assert result.status == "source_changed"
        assert result.receipt == f.receipt
        assert result.artifact is None and result.setup_bundle is None
        assert f.completion()["status"] == "source_changed"
        assert base64.b64decode(f.completion()["process"]["stdout_base64"]) == f.process.stdout
        assert json.loads(f.attempt.read_bytes())["status"] == "entered"
        assert len(f.calls) == 1


@pytest.mark.parametrize("mode", MODES)
@pytest.mark.parametrize("target", ("request", "source"))
def test_capability_public_input_parent_is_refused_before_native_entry(tmp_path, mode, target):
    f = Fixture(tmp_path, mode)
    parent = f.root / "public-input-parent"
    parent.mkdir(mode=0o755)
    parent.chmod(0o755)
    original = getattr(f, target)
    moved = write(parent / original.path.name, original.read())
    setattr(f, target, moved)
    assert moved.sha256 == original.sha256
    assert moved.path.stat().st_mode & 0o777 == 0o600
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls and not f.attempt.exists()
    assert not Path(str(f.attempt) + ".outcome.json").exists()


@pytest.mark.parametrize("mode", MODES)
def test_capability_source_parent_changed_after_await_retains_only_historical_receipt(tmp_path, mode):
    f = Fixture(tmp_path, mode)
    parent = f.root / "private-source-parent"
    parent.mkdir(mode=0o700)
    original = f.source
    f.source = write(parent / original.path.name, original.read())
    assert f.source.sha256 == original.sha256
    f.after_await = lambda: parent.chmod(0o755)
    result = f.run()
    assert result.status == "source_changed" and result.receipt == f.receipt
    assert result.artifact is None and result.selection is None and result.setup_bundle is None
    assert json.loads(f.attempt.read_bytes())["status"] == "entered"
    saved = f.completion()
    assert saved["status"] == "source_changed" and saved["receipt"] == f.receipt
    assert base64.b64decode(saved["process"]["stdout_base64"]) == f.process.stdout
    assert f.source.path.stat().st_mode & 0o777 == 0o600
    assert f.source.sha256 == hashlib.sha256(f.source.path.read_bytes()).hexdigest()
    assert len(f.calls) == 1


@pytest.mark.parametrize("mode", MODES)
def test_changed_stopped_host_lease_leaves_capability_receipt_historical(tmp_path, mode):
    f = Fixture(tmp_path, mode)
    f.after_await = lambda: setattr(f, "current", False)
    result = f.run()
    assert result.status == "source_changed" and result.receipt == f.receipt
    assert result.artifact is None and result.setup_bundle is None
    assert f.completion()["receipt"] == f.receipt
    assert len(f.calls) == 1


@pytest.mark.parametrize("mode", MODES)
@pytest.mark.parametrize("change", ("unknown_top", "member_extra", "missing_issuance", "route"))
def test_closed_request_shape_is_refused_before_native_entry(tmp_path, mode, change):
    f = Fixture(tmp_path, mode)
    request = deepcopy(f.request_value)
    if change == "unknown_top":
        request["permission"] = "must not be caller-selected"
    elif change == "member_extra":
        request["member"]["logical_executor"] = "not a member wire field"
    elif change == "missing_issuance":
        del request["issuance_id"]
    else:
        request["route"] = True if mode == "agent" else "a report has no caller route"
    f.request = write(f.request.path, content(request))
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls and not f.attempt.exists()


@pytest.mark.parametrize("mode,change", [
    ("agent", "member"), ("report", "member"), ("agent", "route"),
    ("agent", "unknown_top"), ("report", "unknown_top"),
    ("agent", "unknown_capability"), ("report", "unknown_capability"),
    ("agent", "empty_parent"), ("report", "empty_parent"),
    ("report", "parent_not_object"),
    ("agent", "negative_exit"), ("report", "negative_exit"),
])
def test_substituted_or_open_receipt_cannot_become_confirmed_capability(tmp_path, mode, change):
    f = Fixture(tmp_path, mode)
    receipt = deepcopy(f.receipt)
    field = MODES[mode][3]
    if change == "member":
        receipt["member"]["member"] = "another-member"
    elif change == "route":
        receipt[field]["route"] = "another-route"
    elif change == "unknown_top":
        receipt["activated"] = True
    elif change == "unknown_capability":
        receipt[field]["audience"] = "not a capability wire field"
    elif change == "empty_parent":
        receipt[field]["parent"] = {} if mode == "agent" else []
    elif change == "parent_not_object":
        receipt[field]["parent"] = ["not a native lineage object"]
    f.process = ProcessOutcome(-9 if change == "negative_exit" else 0,
                               content(receipt), b"retained negative evidence\n")
    result = f.run()
    assert result.status == "outcome_unknown"
    assert result.artifact is None and result.setup_bundle is None
    saved = f.completion()
    assert saved["status"] == "outcome_unknown"
    assert saved["error_type"] == "CustodyError"
    assert saved["process"]["returncode"] == f.process.returncode
    assert base64.b64decode(saved["process"]["stdout_base64"]) == f.process.stdout
    assert len(f.calls) == 1


@pytest.mark.parametrize("mode", MODES)
@pytest.mark.parametrize("effects", (False, True))
def test_native_parent_issuance_failure_preserves_exact_effects_truth(tmp_path, mode, effects):
    f = Fixture(tmp_path, mode)
    failure = {"kind": "application_installer_failed", "command": MODES[mode][1],
        "phase": "native_parent_issuance", "effects_may_have_occurred": effects,
        "detail": "inert native failure evidence"}
    f.process = ProcessOutcome(1, content(failure), b"retained native failure\n")
    result = f.run()
    assert result.status == "native_failure" and result.receipt == failure
    assert result.receipt["effects_may_have_occurred"] is effects
    assert result.artifact is None and result.setup_bundle is None
    assert f.completion()["receipt"] == failure
    assert len(f.calls) == 1


@pytest.mark.parametrize("command", ("validate", "compile", "provision"))
def test_native_parent_issuance_phase_is_not_added_to_other_existing_commands(tmp_path, command):
    f = Fixture(tmp_path, "agent")
    flag = {"validate": "--validate-application-policy", "compile": "--compile-application-tool-policy",
            "provision": "--provision-application-owner"}[command]
    f.process = ProcessOutcome(1, content({"kind": "application_installer_failed", "command": flag,
        "phase": "native_parent_issuance", "effects_may_have_occurred": True,
        "detail": "not an accepted phase for this command"}), b"legacy stderr\n")
    if command == "validate":
        operation = f.caller.validate_application_policy(f.selection, attempt_path=f.attempt)
    elif command == "compile":
        operation = f.caller.compile_application_tool_policy(f.request,
            output_path=f.root / "policy-output.json", attempt_path=f.attempt)
    else:
        f.attempt = f.caller.provision_attempt_path
        operation = f.caller.provision_application_owner(f.selection, namespace="fixture", generation=1)
    result = asyncio.run(operation)
    assert result.status == "outcome_unknown" and result.artifact is None
    assert f.completion()["process"]["returncode"] == 1
    assert len(f.calls) == 1


def test_native_parent_issuance_phase_is_not_added_to_existing_setup_preparation(tmp_path):
    from test_application_setup_caller import Fixture as SetupFixture

    f = SetupFixture(tmp_path)
    f.failure = "native_parent_issuance"
    result = f.run()
    assert result.status == "outcome_unknown" and result.setup_bundle is None
    assert f.completion()["process"]["returncode"] == 1
    assert len(f.calls) == 1


@pytest.mark.parametrize("mode", MODES)
def test_uncertain_original_attempt_cannot_replay_and_explicit_fresh_owner_retains_same_request(tmp_path, mode):
    f = Fixture(tmp_path, mode)
    exact_request = json.dumps(f.request_value, indent=2).encode() + b"\n"
    f.request = write(f.request.path, exact_request)
    f.process = TimeoutError("Original executor joined but its reply is uncertain.")
    original_attempt = f.attempt
    result = f.run()
    assert result.status == "outcome_unknown" and result.receipt is None
    entered = original_attempt.read_bytes()
    completion = Path(str(original_attempt) + ".outcome.json").read_bytes()
    assert f.request.read() == exact_request
    with pytest.raises(CustodyError):
        f.run()
    assert len(f.calls) == 1
    assert original_attempt.read_bytes() == entered
    assert Path(str(original_attempt) + ".outcome.json").read_bytes() == completion

    # Only the existing trusted owner can choose this explicit fresh attempt.
    # It reuses the original request pin and issuance ID, never an auto-retry.
    f.attempt = f.root / "explicit-reconciliation-attempt.json"
    f.caller = NativeInstallerCaller(f.gateway, execute=f.execute,
        require_current=f.require_current, provision_attempt_path=f.caller.provision_attempt_path)
    f.process = ProcessOutcome(0, content(f.receipt), b"explicit inert reconciliation\n")
    reconciled = f.run()
    assert reconciled.status == "receipt_verified"
    assert len(f.calls) == 2 and f.calls[0] == f.calls[1]
    assert f.request.read() == exact_request
    assert json.loads(exact_request)["issuance_id"] == f.request_value["issuance_id"]
    assert original_attempt.read_bytes() == entered
    assert Path(str(original_attempt) + ".outcome.json").read_bytes() == completion
    assert not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("mode", MODES)
@pytest.mark.parametrize("alias", ("provision", "provision_completion"))
def test_capability_attempt_never_claims_stable_provisioning_custody(tmp_path, mode, alias):
    f = Fixture(tmp_path, mode)
    f.attempt = (f.caller.provision_attempt_path if alias == "provision"
                 else Path(str(f.caller.provision_attempt_path) + ".outcome.json"))
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls and not f.attempt.exists()
