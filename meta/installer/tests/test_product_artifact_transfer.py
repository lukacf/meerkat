"""Complete inert artifact custody, not native setup or cloud qualification.

The existing source-only fixture carries poisoned SDK/native bytes. Tests call
the real framed receiver and trusted readers, with no process/provider/native
operation and no accepted application source rewrite.
"""
import asyncio
from contextlib import contextmanager
from dataclasses import replace
import hashlib
import io
import json
import os
from pathlib import Path
import pwd
import struct
import sys
from types import SimpleNamespace
import uuid

import pytest

from launchkit import remote_product_artifact_receiver as receiver
from launchkit import remote_product_artifacts as transfer
from launchkit.remote_interface_bundle import controller_bytes, product_artifact_receiver_bytes
from launchkit.runtime_selection import parse_candidate_selection, selected_runtime_artifacts
from test_meta_toolkit_runtime_selection import product, no_product_execution, digest
from test_product_artifact_transport import ENDPOINT, FINGERPRINT


GUIDE = "11111111-1111-4111-8111-111111111111"
BUILD = "22222222-2222-4222-8222-222222222222"
SOURCE = "a" * 64


def frame(request, content):
    header = transfer._json({**request,
        "transfer_file": {"size": len(content), "sha256": digest(content)}})
    return struct.pack("!I", len(header)) + header + content


@pytest.fixture
def custody(product, tmp_path, monkeypatch):
    home = tmp_path.resolve() / "destination account"
    home.mkdir(mode=0o700)
    host = {"uid": os.getuid(), "home": str(home), "destination": str(home / "project")}
    monkeypatch.setattr(pwd, "getpwuid", lambda uid: SimpleNamespace(pw_dir=str(home)))
    source = product_artifact_receiver_bytes()
    plan = transfer.prepare_product_artifact_transfer(product.target, require_current=lambda: True)
    context = {"schema": 1, "transaction_id": str(uuid.uuid4()), "guide_id": GUIDE,
        "build_id": BUILD, "source_digest": SOURCE, "host": host,
        "manifest_sha256": plan.manifest_sha256, "manifest_size": len(plan.manifest),
        "receiver_sha256": digest(source)}
    store = home / ".local/state/meta-forge/product-artifacts" / context["transaction_id"]
    return SimpleNamespace(product=product, home=home, host=host, source=source,
        plan=plan, context=context, store=store)


def call(custody, action, content=None, *, name=None, module=receiver, context=None):
    request = {"action": action, "context": context or custody.context}
    if name is not None:
        request["name"] = name
    payload = transfer._json(request) if content is None else frame(request, content)
    parsed, stream = module.read_request(io.BytesIO(payload))
    return module.receive(parsed, stream, receiver_source=custody.source)


def send_complete(custody, *, module=receiver):
    call(custody, "begin", module=module)
    call(custody, "manifest", custody.plan.manifest, module=module)
    for pin in custody.plan.files:
        call(custody, "file", pin.path.read_bytes(),
            name=str(pin.path.relative_to(custody.plan.origin.artifact_root)), module=module)


@contextmanager
def frozen_receiver(content):
    """Run the exact trusted bundle in memory, without importing received code."""
    original = {key: value for key, value in sys.modules.items()
        if key == "launchkit" or key.startswith("launchkit.")}
    namespace = {"__name__": "inert_receiver_fixture", "CONTROLLER_SOURCE": content}
    try:
        exec(compile(content, "<trusted-frozen-artifact-receiver>", "exec"), namespace)
        yield SimpleNamespace(**namespace)
    finally:
        for key in list(sys.modules):
            if key == "launchkit" or key.startswith("launchkit."):
                sys.modules.pop(key)
        sys.modules.update(original)


def test_actual_framed_frozen_receiver_preserves_whole_prefix_and_producer_bytes(custody):
    original = {pin.path: pin.path.read_bytes() for pin in custody.plan.files}
    with frozen_receiver(custody.source) as frozen:
        send_complete(custody, module=frozen)
        final = call(custody, "finish", module=frozen)
        selected = json.loads((custody.store / "selection.json").read_bytes())
        target = sys.modules["launchkit.runtime_selection"].parse_candidate_selection(selected)
        observed = sys.modules["launchkit.runtime_selection"].selected_runtime_artifacts(target)
        assert {entry.name for entry in observed.entries} == set(custody.product.nodes)
        assert observed.archive.path.read_bytes() == custody.product.archive.path.read_bytes()
        assert target.source_manifest is not None
        assert target.source_manifest.path.read_bytes() == custody.product.source.path.read_bytes()
        assert target.artifact_root == custody.store / "inputs"
        assert not hasattr(observed, "qualified") and not hasattr(observed, "admitted")
    assert final["status"] == "runtime-artifacts-transferred"
    assert final["artifact_root"] == str(custody.store / "inputs")
    assert len(transfer._json(final)) < 65536
    assert set(final) == {"transaction_id", "host", "manifest_sha256", "receiver_sha256",
        "status", "artifact_root", "selection_sha256"}
    for path, content in original.items():
        assert path.read_bytes() == content
        assert (custody.store / "inputs" / path.relative_to(custody.plan.origin.artifact_root)).read_bytes() == content
    assert not Path(custody.host["destination"]).exists()
    assert not (custody.store / "inputs" / observed.prefix).exists()
    assert json.loads((custody.store / "custody.json").read_bytes()) == final


def test_relocated_exact_selection_reread_and_retry_do_not_change_original_identity(custody):
    send_complete(custody)
    first = call(custody, "finish")
    target = transfer.relocate_product_target(custody.plan.manifest, custody.store / "inputs")
    assert parse_candidate_selection(json.loads((custody.store / "selection.json").read_bytes())) == target
    assert selected_runtime_artifacts(target).selection == target
    assert target != custody.plan.origin and target.target == custody.plan.origin.target
    assert target.meta == custody.plan.origin.meta and target.toolkit == custody.plan.origin.toolkit
    assert custody.plan.origin.source_manifest is None
    assert call(custody, "finish") == first
    before = (custody.store / "custody.json").read_bytes()
    send_complete(custody)
    assert call(custody, "finish") == first and (custody.store / "custody.json").read_bytes() == before


@pytest.mark.parametrize("change", ["truncated", "bytes", "trailing", "header-hash", "header-size"])
def test_partial_or_mismatched_stream_has_no_complete_artifact_or_custody(custody, change):
    call(custody, "begin")
    call(custody, "manifest", custody.plan.manifest)
    pin = custody.plan.files[0]
    name = str(pin.path.relative_to(custody.plan.origin.artifact_root))
    payload = frame({"action": "file", "context": custody.context, "name": name}, pin.path.read_bytes())
    if change == "truncated": payload = payload[:-1]
    elif change == "bytes": payload = payload[:-1] + bytes([payload[-1] ^ 1])
    elif change == "trailing": payload += b"unowned trailer"
    else:
        request, stream = receiver.read_request(io.BytesIO(payload))
        request["transfer_file"]["sha256" if change == "header-hash" else "size"] = (
            "0" * 64 if change == "header-hash" else pin.size + 1)
        payload = frame({"action": "file", "context": custody.context, "name": name}, pin.path.read_bytes())
        with pytest.raises(ValueError):
            receiver.receive(request, stream, receiver_source=custody.source)
        assert not (custody.store / "inputs" / name).exists()
        assert not (custody.store / "custody.json").exists()
        return
    request, stream = receiver.read_request(io.BytesIO(payload))
    with pytest.raises(ValueError): receiver.receive(request, stream, receiver_source=custody.source)
    assert not (custody.store / "inputs" / name).exists()
    assert not (custody.store / "custody.json").exists()
    assert list((custody.store / "partial").iterdir())


@pytest.mark.parametrize("change", ["missing", "extra", "changed", "linked", "manifest", "owner", "receiver"])
def test_fresh_finish_rejects_incomplete_changed_or_foreign_input_closure(custody, change):
    send_complete(custody)
    pin = custody.plan.files[0]
    path = custody.store / "inputs" / pin.path.relative_to(custody.plan.origin.artifact_root)
    if change == "missing": path.unlink()
    elif change == "extra": (custody.store / "inputs" / "unexpected.py").write_bytes(b"raise RuntimeError('never import')")
    elif change == "changed": path.write_bytes(b"x" * pin.size)
    elif change == "linked":
        path.unlink(); path.symlink_to(pin.path)
    else:
        selected = custody.store / {"manifest": "manifest.json", "owner": "owner.json", "receiver": "receiver.py"}[change]
        selected.write_bytes(selected.read_bytes() + b"changed")
    with pytest.raises(ValueError): call(custody, "finish")
    assert not (custody.store / "custody.json").exists()
    assert not Path(custody.host["destination"]).exists()


@pytest.mark.parametrize("change", ["build", "source", "uid", "home", "manifest", "bundle"])
def test_original_receiver_ownership_cannot_be_replaced_by_request(custody, change):
    call(custody, "begin")
    context = json.loads(json.dumps(custody.context))
    if change == "build": context["build_id"] = str(uuid.uuid4())
    elif change == "source": context["source_digest"] = "f" * 64
    elif change == "uid": context["host"]["uid"] += 1
    elif change == "home": context["host"]["home"] += "-other"
    elif change == "manifest": context["manifest_sha256"] = "f" * 64
    else: context["receiver_sha256"] = "f" * 64
    before = (custody.store / "owner.json").read_bytes()
    with pytest.raises((ValueError, OSError)):
        call(custody, "manifest", custody.plan.manifest, context=context)
    assert (custody.store / "owner.json").read_bytes() == before
    assert not (custody.store / "custody.json").exists()


def test_transferred_filenames_cannot_shadow_frozen_reader_or_expand_exact_manifest(custody):
    call(custody, "begin")
    call(custody, "manifest", custody.plan.manifest)
    for name in ("runtime_candidates.py", "sdk_wheel_integrity.py", "templates/install_candidate.py", "../owner.json"):
        with pytest.raises(ValueError):
            call(custody, "file", b"raise RuntimeError('native import poison')", name=name)
    assert not list((custody.store / "inputs").iterdir())
    assert not (custody.store / "custody.json").exists()


def test_extra_manifest_inputs_cannot_become_complete_custody(custody):
    document = json.loads(custody.plan.manifest)
    content = b"raise RuntimeError('never import transferred helper')\n"
    document["files"].append({"name": "runtime_candidates.py", "size": len(content), "sha256": digest(content)})
    manifest = transfer._json(document)
    custody.context = {**custody.context, "manifest_sha256": digest(manifest), "manifest_size": len(manifest)}
    call(custody, "begin")
    call(custody, "manifest", manifest)
    for pin in custody.plan.files:
        call(custody, "file", pin.path.read_bytes(), name=str(pin.path.relative_to(custody.plan.origin.artifact_root)))
    call(custody, "file", content, name="runtime_candidates.py")
    with frozen_receiver(custody.source) as frozen:
        with pytest.raises(ValueError, match="exact complete artifact closure"):
            call(custody, "finish", module=frozen)
    assert not (custody.store / "custody.json").exists()


class TrustedImplementation:
    def __init__(self, target): self.target, self.calls = target, []
    def selected_runtime_target(self, guide_id, *, build_id, source_digest):
        assert (guide_id, build_id, source_digest) == (GUIDE, BUILD, SOURCE)
        self.calls.append((guide_id, build_id, source_digest))
        return self.target


class FramedTransport:
    """Trusted pinned transport seam that exercises actual receiver wire bytes."""
    def __init__(self, custody): self.custody, self.calls, self.deadlines = custody, [], set()
    def admitted(self, endpoint, payload, timeout, expected_fingerprint, deadline):
        assert endpoint == ENDPOINT and expected_fingerprint == FINGERPRINT
        assert 0 < timeout <= 600 and deadline > asyncio.get_running_loop().time()
        self.deadlines.add(deadline)
        self.calls.append(payload["action"])
    async def request(self, endpoint, program, payload, timeout, *, expected_fingerprint, deadline):
        self.admitted(endpoint, payload, timeout, expected_fingerprint, deadline)
        request, stream = receiver.read_request(io.BytesIO(transfer._json(payload)))
        result = receiver.receive(request, stream, receiver_source=self.custody.source)
        return {**result, "_transport": {"host_key_fingerprint": expected_fingerprint}}
    async def transfer_file(self, endpoint, program, payload, artifact, timeout, *,
            expected_fingerprint, deadline, require_current):
        self.admitted(endpoint, payload, timeout, expected_fingerprint, deadline)
        require_current()
        request, stream = receiver.read_request(io.BytesIO(frame(payload, artifact.path.read_bytes())))
        result = receiver.receive(request, stream, receiver_source=self.custody.source)
        require_current()
        return {**result, "_transport": {"host_key_fingerprint": expected_fingerprint}}


def owner_arguments(custody, tmp_path):
    return {"build_id": BUILD, "source_digest": SOURCE,
        "transaction_id": custody.context["transaction_id"], "transaction_directory": tmp_path.resolve() / "host transfer",
        "destination_host": custody.host, "transport": FramedTransport(custody), "endpoint": ENDPOINT,
        "expected_fingerprint": FINGERPRINT, "require_current": lambda: True}


async def test_concrete_trusted_implementation_to_complete_receiver_path(custody, tmp_path):
    owner = TrustedImplementation(custody.product.target)
    kwargs = owner_arguments(custody, tmp_path)
    loop, owner_task, observations = asyncio.get_running_loop(), asyncio.current_task(), []
    def original_owner():
        assert asyncio.get_running_loop() is loop and asyncio.current_task() is owner_task
        observations.append(True)
    kwargs["require_current"] = original_owner
    result = await transfer.transfer_product_artifacts(owner, GUIDE, **kwargs)
    assert result.origin == custody.product.target
    assert result.relocated == transfer.relocate_product_target(custody.plan.manifest, custody.store / "inputs")
    assert result.selection_sha256 == digest((custody.store / "selection.json").read_bytes())
    assert len(owner.calls) > 2 and len(observations) > 2 and len(kwargs["transport"].deadlines) == 1
    assert kwargs["transport"].calls == ["begin", "manifest", *(["file"] * len(custody.plan.files)), "finish"]
    local = json.loads((kwargs["transaction_directory"] / "received.json").read_bytes())
    assert local["status"] == "runtime-artifacts-transferred"
    assert local["context"] == custody.context and not Path(custody.host["destination"]).exists()
    assert not hasattr(result, "installed") and not hasattr(result, "ready")


@pytest.mark.parametrize("change", ["source", "owner", "cancel", "timeout", "fingerprint", "receipt"])
async def test_changed_owner_or_uncertain_transport_never_publishes_host_custody(custody, tmp_path, change):
    owner = TrustedImplementation(custody.product.target)
    kwargs = owner_arguments(custody, tmp_path)
    original = kwargs["transport"].transfer_file
    async def interrupted(*args, **options):
        result = await original(*args, **options)
        if change == "source": custody.plan.files[-1].path.write_bytes(b"x" * custody.plan.files[-1].size)
        elif change == "owner": owner.target = replace(owner.target, target="linux-x86_64")
        elif change == "cancel": raise asyncio.CancelledError
        elif change == "timeout": raise TimeoutError
        elif change == "fingerprint": result["_transport"]["host_key_fingerprint"] = "SHA256:" + "A" * 43
        else: result["transaction_id"] = str(uuid.uuid4())
        return result
    kwargs["transport"].transfer_file = interrupted
    with pytest.raises((ValueError, TimeoutError, asyncio.CancelledError)):
        await transfer.transfer_product_artifacts(owner, GUIDE, **kwargs)
    assert (kwargs["transaction_directory"] / "intent.json").exists()
    assert not (kwargs["transaction_directory"] / "received.json").exists()
    assert not (custody.store / "custody.json").exists()
    assert "finish" not in kwargs["transport"].calls


def test_existing_ordinary_controller_bundle_is_unchanged_by_separate_receiver():
    root = Path(receiver.__file__).parent
    canonical = (root / "remote_destination.py").read_bytes()
    marker = b"\nif __name__ == '__main__':\n"
    payload = b"\nAPPLICATION_EXPERIENCES_SOURCE = " + repr((root / "application_experiences.py").read_bytes()).encode("ascii") + b"\n"
    payload += b"CONSOLE_CONFIGURATION_SOURCE = " + repr((root / "templates_v2/console_configuration.py").read_bytes()).encode("ascii") + b"\n"
    assert controller_bytes() == canonical.replace(marker, payload + marker)
    assert product_artifact_receiver_bytes() != controller_bytes()
