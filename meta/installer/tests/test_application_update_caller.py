"""Inert update-input caller custody, without native policy validation.

Opaque private artifacts and synthetic exited receipts exercise the existing
application owner's one-shot command boundary. No gateway is executable here.
"""
import asyncio
import base64
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import threading

import pytest

from launchkit import connect_installer_caller as caller
from launchkit.templates.storage_setup import project_lock


COMMAND = "--validate-application-update"
KIND = "application_update_validated"


@pytest.fixture(autouse=True)
def forbid_native_operations(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("These source controls must never spawn or signal a real process.")

    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    monkeypatch.setattr(asyncio, "create_subprocess_shell", forbidden)
    monkeypatch.setattr(subprocess, "Popen", forbidden)
    monkeypatch.setattr(os, "kill", forbidden)
    monkeypatch.setattr(os, "killpg", forbidden)


def content(value):
    return (json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def write(path, data, mode=0o600):
    path.write_bytes(data)
    path.chmod(mode)
    return caller.ExactFile(path, hashlib.sha256(data).hexdigest())


class Fixture:
    def __init__(self, root):
        self.root = root.resolve()
        self.root.chmod(0o700)
        self.gateway = write(self.root / "toolkit_gateway", b"inert; never executed\n", 0o700)
        self.predecessor_pins = tuple(write(self.root / name, content({"opaque": name}))
            for name in ("activation.json", "manifest.json", "local-policy.json"))
        self.compilation_input = write(self.root / "candidate-input.json", b'{"opaque_input":true}\n')
        self.tool_policy = write(self.root / "candidate-policy.json", b'{"opaque_policy":true}\n')
        self.source = write(self.root / "outside-native-input.txt", b"original accepted source\n")
        self.selected = {"update_id": "original-admitted-update",
            "predecessor": {"deployment_root": str(self.root),
                **{name: pin.pin() for name, pin in zip(
                    ("activation", "policy_manifest", "local_policy"), self.predecessor_pins)}},
            "compilation_input": self.compilation_input.pin(),
            "application_tool_policy": self.tool_policy.pin()}
        self.selection = write(self.root / "update-selection.json", content(self.selected))
        self.attempt = self.root / "update-attempt.json"
        self.calls = []
        self.loop = None
        self.lease = None
        self.after_await = lambda: None
        self.receipt = {"kind": KIND, "selection": deepcopy(self.selected)}
        self.process = caller.ProcessOutcome(0, content(self.receipt), b"retained inert stderr\n")
        self.caller = caller.NativeInstallerCaller(self.gateway, execute=self.execute,
            require_current=self.require_current,
            require_filesystem_current=self.require_filesystem_current,
            provision_attempt_path=self.root / "stable-provision-attempt.json")

    def require_current(self):
        assert asyncio.get_running_loop() is self.loop
        self.lease.require_current()

    def require_filesystem_current(self):
        assert threading.current_thread() is not threading.main_thread()
        self.source.read()

    async def execute(self, argv):
        self.calls.append(argv)
        assert json.loads(self.attempt.read_bytes())["status"] == "entered"
        self.lease.require_current()
        await asyncio.sleep(0)
        self.after_await()
        if isinstance(self.process, BaseException):
            raise self.process
        return self.process

    async def invoke(self):
        self.loop = asyncio.get_running_loop()
        return await self.caller.validate_application_update(self.selection,
            attempt_path=self.attempt, source_files=(self.source,))

    def run(self):
        with project_lock(self.root) as self.lease:
            return asyncio.run(self.invoke())

    def completion(self):
        return json.loads(Path(str(self.attempt) + ".outcome.json").read_bytes())


def failure(*, phase="validation", effects=False):
    return caller.ProcessOutcome(1, content({"kind": "application_installer_failed",
        "command": COMMAND, "phase": phase, "effects_may_have_occurred": effects,
        "detail": "synthetic native input refusal"}), b"retained refusal stderr\n")


def test_dormant_validator_retains_original_selection_and_no_publication_carrier(tmp_path):
    f = Fixture(tmp_path)
    original = f.selection.read()
    result = f.run()
    assert result.status == "receipt_verified" and result.receipt == f.receipt
    assert result.artifact is None and result.selection is None and result.setup_bundle is None
    assert f.calls == [(str(f.gateway.path), COMMAND, "--selection", str(f.selection.path))]
    entered = json.loads(f.attempt.read_bytes())
    assert entered["input_files"] == [pin.pin() for pin in (
        f.gateway, f.selection, *f.predecessor_pins, f.compilation_input, f.tool_policy, f.source)]
    saved = f.completion()
    assert saved["status"] == "receipt_verified" and saved["receipt"] == f.receipt
    assert base64.b64decode(saved["process"]["stdout_base64"]) == f.process.stdout
    assert base64.b64decode(saved["process"]["stderr_base64"]) == f.process.stderr
    assert f.selection.read() == original
    assert not f.caller.provision_attempt_path.exists()
    assert not (f.root / "current").exists()


def test_read_only_validator_refuses_a_receipt_claiming_effects():
    with pytest.raises(caller.CustodyError):
        caller._receipt(failure(effects=True), COMMAND, KIND)


def test_read_only_validator_refuses_an_effectful_installer_phase():
    with pytest.raises(caller.CustodyError):
        caller._receipt(failure(phase="owner_initialization"), COMMAND, KIND)


@pytest.mark.parametrize("change", ["extra_authority", "empty_correlation", "nested_pin_shape"])
def test_closed_update_selection_refuses_before_attempt_or_executor(tmp_path, change):
    f = Fixture(tmp_path)
    selected = deepcopy(f.selected)
    if change == "extra_authority":
        selected["owner"] = {"replacement": True}
    elif change == "empty_correlation":
        selected["update_id"] = ""
    else:
        selected["predecessor"]["activation"]["sha256"] = True
    f.selection = write(f.selection.path, content(selected))
    with pytest.raises(caller.CustodyError):
        f.run()
    assert not f.calls and not f.attempt.exists()


def test_changed_candidate_bytes_refuse_before_attempt_claim(tmp_path):
    f = Fixture(tmp_path)
    f.compilation_input.path.write_bytes(b'{"changed_input":true}\n')
    with pytest.raises(caller.CustodyError):
        f.run()
    assert not f.calls and not f.attempt.exists()


@pytest.mark.parametrize("stage", ["before", "after"])
def test_update_artifact_parent_must_remain_private(tmp_path, stage):
    f = Fixture(tmp_path)
    directory = f.root / "candidate"
    directory.mkdir(mode=0o700)
    f.tool_policy = write(directory / "policy.json", f.tool_policy.read())
    f.selected["application_tool_policy"] = f.tool_policy.pin()
    f.selection = write(f.selection.path, content(f.selected))
    f.receipt = {"kind": KIND, "selection": deepcopy(f.selected)}
    f.process = caller.ProcessOutcome(0, content(f.receipt), b"exact inert receipt\n")
    if stage == "before":
        directory.chmod(0o755)
        with pytest.raises(caller.CustodyError):
            f.run()
        assert not f.calls and not f.attempt.exists()
    else:
        f.after_await = lambda: directory.chmod(0o755)
        result = f.run()
        assert result.status == "source_changed"
        assert f.completion()["status"] == "source_changed"
        assert result.receipt == f.receipt and len(f.calls) == 1


@pytest.mark.parametrize("target", ["predecessor", "source"])
def test_changes_after_receipt_keep_only_historical_validation(tmp_path, target):
    f = Fixture(tmp_path)
    pin = f.predecessor_pins[0] if target == "predecessor" else f.source
    f.after_await = lambda: pin.path.write_bytes(b"changed after original validation\n")
    result = f.run()
    assert result.status == "source_changed" and result.receipt == f.receipt
    assert result.artifact is None and result.selection is None and result.setup_bundle is None
    assert f.completion()["status"] == "source_changed"
    assert base64.b64decode(f.completion()["process"]["stdout_base64"]) == f.process.stdout
    assert len(f.calls) == 1


def test_original_project_lease_loss_withholds_current_validation(tmp_path):
    f = Fixture(tmp_path)

    def replace_lock():
        lock = f.root / ".mobkit/launch.lock"
        lock.unlink()
        write(lock, b"replacement lock\n")

    f.after_await = replace_lock
    result = f.run()
    assert result.status == "source_changed" and result.receipt == f.receipt
    assert f.completion()["status"] == "source_changed"
    assert len(f.calls) == 1
    with pytest.raises(RuntimeError):
        f.lease.require_current()


@pytest.mark.parametrize("change", ["candidate_pin", "ready_field", "wrong_kind"])
def test_substituted_or_open_success_receipt_is_retained_as_unknown(tmp_path, change):
    f = Fixture(tmp_path)
    receipt = deepcopy(f.receipt)
    if change == "candidate_pin":
        receipt["selection"]["application_tool_policy"]["sha256"] = "0" * 64
    elif change == "ready_field":
        receipt["ready"] = True
    else:
        receipt["kind"] = "application_policy_prepared"
    f.process = caller.ProcessOutcome(0, content(receipt), b"retained counterexample\n")
    result = f.run()
    assert result.status == "outcome_unknown" and result.receipt is None
    assert result.artifact is None and result.selection is None and result.setup_bundle is None
    saved = f.completion()
    assert saved["error_type"] == "CustodyError"
    assert base64.b64decode(saved["process"]["stdout_base64"]) == f.process.stdout
    assert len(f.calls) == 1


def test_selection_echo_does_not_coerce_json_value_types():
    # The common receipt boundary must preserve value types even for an
    # opaque selected object. A valid update selection is checked separately.
    process = caller.ProcessOutcome(0, content({"kind": KIND, "selection": {"opaque": True}}), b"")
    with pytest.raises(caller.CustodyError):
        caller._receipt(process, COMMAND, KIND, selection={"opaque": 1})


def test_known_read_only_refusal_retains_original_attempt_without_replay(tmp_path):
    f = Fixture(tmp_path)
    f.process = failure()
    result = f.run()
    assert result.status == "native_failure"
    assert result.receipt["effects_may_have_occurred"] is False
    assert result.artifact is None and result.selection is None and result.setup_bundle is None
    saved = f.completion()
    assert saved["status"] == "native_failure" and saved["error_type"] is None
    original_attempt = f.attempt.read_bytes()
    original_completion = Path(str(f.attempt) + ".outcome.json").read_bytes()
    with pytest.raises(caller.CustodyError):
        f.run()
    assert len(f.calls) == 1
    assert f.attempt.read_bytes() == original_attempt
    assert Path(str(f.attempt) + ".outcome.json").read_bytes() == original_completion


def test_joined_executor_failure_keeps_entered_attempt_unknown(tmp_path):
    f = Fixture(tmp_path)
    f.process = TimeoutError("Inert executor settled without a trustworthy reply.")
    result = f.run()
    assert result.status == "outcome_unknown" and result.receipt is None
    assert f.completion()["process"] is None
    assert f.completion()["error_type"] == "TimeoutError"
    with pytest.raises(caller.CustodyError):
        f.run()
    assert len(f.calls) == 1


def test_selection_change_during_completion_cannot_return_current_result(tmp_path, monkeypatch):
    f = Fixture(tmp_path)
    original = caller._write_new

    def observed_write(path, value):
        original(path, value)
        if str(path) == str(f.attempt) + ".outcome.json":
            f.selection.path.write_bytes(json.dumps(f.selected, indent=2).encode() + b"\n")

    monkeypatch.setattr(caller, "_write_new", observed_write)
    result = f.run()
    assert result.status == "source_changed" and result.receipt == f.receipt
    assert f.completion()["status"] == "receipt_verified"
    assert f.completion()["receipt"] == f.receipt
    assert len(f.calls) == 1


@pytest.mark.parametrize("phase", ["claim", "completion"])
@pytest.mark.asyncio
async def test_repeated_cancellation_joins_original_worker_and_keeps_lease(tmp_path, monkeypatch, phase):
    f = Fixture(tmp_path)
    entered, release, exited = threading.Event(), threading.Event(), threading.Event()
    original = caller._write_new

    def delayed_write(path, value):
        original(path, value)
        if (phase == "claim" and Path(path) == f.attempt
                or phase == "completion" and str(path) == str(f.attempt) + ".outcome.json"):
            entered.set()
            try:
                assert release.wait(2), "The original worker must be released by its owner."
            finally:
                exited.set()

    monkeypatch.setattr(caller, "_write_new", delayed_write)
    with project_lock(f.root) as f.lease:
        task = asyncio.create_task(f.invoke())
        try:
            assert await asyncio.to_thread(entered.wait, 30)
            task.cancel()
            await asyncio.sleep(0)
            task.cancel()
            await asyncio.sleep(0)
            assert not task.done()
            f.lease.require_current()
            release.set()
            with pytest.raises(asyncio.CancelledError):
                await task
            assert exited.is_set()
            f.lease.require_current()
        finally:
            release.set()
            if not task.done():
                await asyncio.gather(task, return_exceptions=True)
        with pytest.raises(caller.CustodyError):
            await f.invoke()
    saved = f.completion()
    assert len(f.calls) == (0 if phase == "claim" else 1)
    assert saved["status"] == ("outcome_unknown" if phase == "claim" else "receipt_verified")
    assert saved["error_type"] == ("CancelledError" if phase == "claim" else None)
    assert json.loads(f.attempt.read_bytes())["status"] == "entered"
    assert len(f.calls) == (0 if phase == "claim" else 1)
