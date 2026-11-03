"""Inert retained inventory readback with exact selected product prefix custody.

The process receipt is synthetic. No gateway, SDK or provider is executed and
these opaque member strings make no claim about native authority or readiness.
"""
import asyncio
import base64
from dataclasses import replace
import hashlib
import json
from types import SimpleNamespace

import pytest

from launchkit import connect_installer_caller as caller
from launchkit import runtime_candidates
from launchkit.templates import install_candidate
from test_meta_toolkit_runtime_selection import product, no_product_execution


def encoded(value):
    return (json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def private_file(root, name, value):
    parent = root / name
    parent.mkdir(mode=0o700)
    path = parent / "retained.json"
    data = value if isinstance(value, bytes) else encoded(value)
    path.write_bytes(data)
    path.chmod(0o600)
    return caller.ExactFile(path, hashlib.sha256(data).hexdigest())


@pytest.fixture
def retained(tmp_path, product):
    artifacts = runtime_candidates.read_meta_toolkit_artifacts(product.target)
    enclosing = tmp_path / "selected-product"
    enclosing.mkdir(mode=0o700)
    path = install_candidate.install_meta_toolkit_prefix(artifacts,
        deployment_root=enclosing, require_current=lambda: None)
    gateway = caller.ProductGatewayFile.observe(
        caller.ExactFile(path, artifacts.candidate["binary"]["sha256"]), artifacts)
    root = tmp_path / "retained-observation"
    root.mkdir(mode=0o700)
    source = {"schema": 1, "mob_id": "inert-selected-mob", "helper_identity": "helper:chosen",
        "roster": [{"identity": "domain:original", "profile": "research",
            "instructions": "Original retained instructions.\n",
            "binding": {"opaque_native_binding": ["selected", 7]}},
            {"identity": "helper:chosen", "profile": "setup"}]}
    receipt = {"schema": 1, "mob_id": source["mob_id"], "realm_id": "inert-native-realm",
        "helper_identity": source["helper_identity"], "members": [
            {"identity": row["identity"], "member": {"mob_id": source["mob_id"],
                "role": row["profile"], "member": "inert-native-member-" + str(index)}}
            for index, row in enumerate(source["roster"])]}
    compiler_input = private_file(root, "compiler", source)
    source_file = private_file(root, "source", {"retained_original_source": [1, False, None]})
    attempt = private_file(root, "attempt", {"status": "entered",
        "argv": [str(gateway.path), caller.INVENTORY_COMMAND, "--input", str(compiler_input.path)],
        "input_files": [pin.pin() for pin in (gateway, compiler_input, source_file)], "output_path": None})
    completion = private_file(root, "completion", {"status": "receipt_verified", "receipt": receipt,
        "artifact": None, "error_type": None, "process": {"returncode": 0,
            "stdout_base64": base64.b64encode(encoded(receipt)).decode(),
            "stderr_base64": base64.b64encode(b"inert retained stderr\n").decode()}})
    checks = []
    async def never_execute(argv):
        pytest.fail("Retained inventory readback must not execute any command.")
    owner = caller.NativeInstallerCaller(gateway, execute=never_execute,
        require_current=lambda: checks.append("original-owner"),
        provision_attempt_path=root / "original-provision-attempt.json")
    inventory = caller.NativeApplicationInventory(owner, gateway, compiler_input,
        (source_file,), attempt, completion)
    # Exercise the actual retained file/receipt path before every counterexample.
    assert inventory.authenticate() == (source, receipt)
    return SimpleNamespace(inventory=inventory, owner=owner, source=source,
        receipt=receipt, gateway=gateway, checks=checks)


def test_exact_product_gateway_authenticates_full_prefix_and_original_receipt(retained):
    assert type(retained.gateway) is caller.ProductGatewayFile
    assert retained.gateway.path.parent.stat().st_mode & 0o777 == 0o755
    assert retained.gateway.path.stat().st_mode & 0o777 == 0o755
    assert retained.inventory.authenticate() == (retained.source, retained.receipt)
    assert retained.checks == ["original-owner"] * 4


def test_original_inventory_runner_retains_product_readback_without_native_execution(retained):
    calls = []
    async def exited_observation(argv):
        calls.append(argv)
        return caller.ProcessOutcome(0, encoded(retained.receipt), b"inert retained stderr\n")
    retained.owner.execute = exited_observation
    entered = retained.inventory.attempt.path.parent / "new-inert-inventory-attempt.json"
    result = asyncio.run(retained.owner.lower_application_inventory(
        retained.inventory.compiler_input, attempt_path=entered,
        source_files=retained.inventory.source_files))
    assert result.status == "receipt_verified" and result.receipt == retained.receipt
    assert type(result.inventory.gateway) is caller.ProductGatewayFile
    assert result.inventory.authenticate() == (retained.source, retained.receipt)
    assert calls == [(str(retained.gateway.path), caller.INVENTORY_COMMAND,
        "--input", str(retained.inventory.compiler_input.path))]


@pytest.mark.parametrize("gateway_type", [caller.ExactFile, type("ExtendedProductGatewayFile", (caller.ProductGatewayFile,), {})])
def test_public_gateway_parent_requires_the_exact_product_wrapper(retained, gateway_type):
    original = retained.gateway
    gateway = (gateway_type(original.path, original.sha256) if gateway_type is caller.ExactFile
        else gateway_type(original.path, original.sha256, original.artifacts,
            original.enclosing, original.enclosing_mode, original.entries))
    retained.owner.gateway = gateway
    inventory = replace(retained.inventory, gateway=gateway)
    with pytest.raises(caller.CustodyError, match="parent directory is not private"):
        inventory.authenticate()


@pytest.mark.parametrize("field", ["compiler_input", "source_files", "attempt", "completion"])
@pytest.mark.parametrize("when", ["before-read", "after-receipt"])
def test_every_private_inventory_input_parent_stays_private(retained, monkeypatch, field, when):
    inventory = retained.inventory
    pin = inventory.source_files[0] if field == "source_files" else getattr(inventory, field)
    def publish_parent():
        pin.path.parent.chmod(0o755)
    if when == "before-read":
        publish_parent()
    else:
        original = caller._receipt
        def joined(*args, **kwargs):
            result = original(*args, **kwargs)
            publish_parent()
            return result
        monkeypatch.setattr(caller, "_receipt", joined)
    with pytest.raises(caller.CustodyError, match="parent directory is not private"):
        inventory.authenticate()


@pytest.mark.parametrize("when", ["before-read", "after-receipt"])
def test_changed_selected_full_prefix_refuses_retained_inventory(retained, monkeypatch, when):
    resource = retained.gateway.path.parent.parent / "libexec/meerkat-connect/real-file"
    def change_prefix():
        resource.chmod(0o644)
        resource.write_bytes(b"changed inert selected resource\n")
        resource.chmod(0o444)
    if when == "before-read":
        change_prefix()
    else:
        original = caller._receipt
        def joined(*args, **kwargs):
            result = original(*args, **kwargs)
            change_prefix()
            return result
        monkeypatch.setattr(caller, "_receipt", joined)
    with pytest.raises(caller.CustodyError, match="original selected product prefix"):
        retained.inventory.authenticate()
