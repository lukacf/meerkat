"""Inert schema 3 byte/path custody, with no native or resource execution.

The adjacent maintained fixture emits inert member/reference custody specimens
and literal surface bytes. Ownership fields are opaque PrincipalRef specimens,
not proof of native decoding, compiler, policy or grant qualification.
"""
import hashlib
import json
from pathlib import Path

import pytest

from launchkit.connect_installer_caller import CustodyError
from test_application_setup_caller import Fixture, content, write


def save_input(f):
    f.input = write(f.input.path, content(f.input_value))


def test_schema3_preserves_source_emitted_surface_and_opaque_policy(tmp_path):
    f = Fixture(tmp_path)
    # Unrelated native metadata is opaque and remains untouched by the caller.
    ordinary = {"member": {"mob_id": "fixture", "role": "ordinary", "member": "ordinary"},
        "context_generation": 9, "initial_surface": {"name": "ordinary.json", "sha256": "1" * 64}}
    f.input_value["native_members"].append(ordinary)
    pretty = json.dumps(f.input_value, indent=2).encode() + b"\n"
    f.input = write(f.input.path, pretty)
    result = f.run()
    assert result.status == "receipt_verified"
    bundle = result.setup_bundle
    setup_bytes, policy_bytes = bundle.authenticate()
    setup = json.loads(setup_bytes)
    assert setup["schema"] == 3
    assert setup["input_sha256"] != f.input.sha256
    assert bundle.setup_input.read() == pretty
    assert policy_bytes == b'{"synthetic_compiled_policy":"opaque to Python"}\n'
    assert setup["native_members"][1] == ordinary
    assert bundle.helper_source_surface.pin() == f.helper_source_surface.pin()
    assert bundle.helper_source_surface.read() == b'[{"name":"research"}]'
    assert bundle.helper_initial_surface.read() == f.helper_surface_bytes
    assert result.receipt["helper_initial_surface"] == bundle.helper_initial_surface.pin()
    assert setup["native_members"][0]["initial_surface"] == {
        "name": bundle.helper_initial_surface.path.name, "sha256": bundle.helper_initial_surface.sha256}
    assert {p.name for p in f.output.iterdir()} == {
        "manifest.json", "application-setup.json", "application-tool-policy.json", "helper-initial-surface.json"}
    assert len(json.loads(bundle.manifest.read())["artifacts"]) == 3
    entered = json.loads(f.attempt.read_bytes())
    assert f.helper_source_surface.pin() in entered["input_files"]
    assert f.completion()["setup_bundle"] == bundle.pins()
    assert len(f.calls) == 1


@pytest.mark.parametrize("change", ["bytes", "public", "symlink", "hardlink"])
def test_original_surface_requires_private_exact_bytes_before_entry(tmp_path, change):
    f = Fixture(tmp_path)
    source = f.helper_source_surface.path
    if change == "bytes":
        source.write_bytes(b"[]")
    elif change == "public":
        source.chmod(0o644)
    elif change == "symlink":
        retained = source.with_name("real-surface.json")
        source.rename(retained)
        source.symlink_to(retained)
    else:
        import os
        os.link(source, f.root / "same-inode.json")
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls
    assert not f.attempt.exists()
    assert not f.output.exists()


@pytest.mark.parametrize("alias", ["output", "attempt", "completion", "provision", "provision_completion"])
def test_original_surface_cannot_occupy_output_or_attempt_custody(tmp_path, alias):
    f = Fixture(tmp_path)
    if alias == "output":
        f.output = f.helper_source_surface.path
    else:
        target = {"attempt": f.attempt, "completion": Path(str(f.attempt) + ".outcome.json"),
            "provision": f.caller.provision_attempt_path,
            "provision_completion": Path(str(f.caller.provision_attempt_path) + ".outcome.json")}[alias]
        f.helper_source_surface = write(target, f.helper_source_surface.read())
        f.input_value["helper_source_surface"] = f.helper_source_surface.pin()
        f.input_value["native_members"][0]["initial_surface"] = {
            "name": target.name, "sha256": f.helper_source_surface.sha256}
        save_input(f)
    before = f.helper_source_surface.read()
    with pytest.raises(CustodyError):
        f.run()
    assert not f.calls
    assert f.helper_source_surface.read() == before


def test_schema2_input_is_not_silently_upgraded_or_claimed(tmp_path):
    f = Fixture(tmp_path)
    f.input_value["schema"] = 2
    save_input(f)
    before = f.input.read()
    with pytest.raises(CustodyError):
        f.run()
    assert f.input.read() == before
    assert not f.calls
    assert not f.attempt.exists()


@pytest.mark.parametrize("change", ["source_selection", "member_reference", "duplicate_member",
    "bool_number_member", "receipt_path", "missing_receipt", "surface_bytes", "manifest_surface"])
def test_emitted_surface_needs_exact_selection_reference_and_physical_closure(tmp_path, change):
    f = Fixture(tmp_path)
    def mutate(setup, policy, receipt):
        if change == "source_selection":
            setup["helper_source_surface"] = f.source.pin()
        elif change == "member_reference":
            setup["native_members"][0]["initial_surface"]["sha256"] = "f" * 64
        elif change == "duplicate_member":
            setup["native_members"].append(json.loads(json.dumps(setup["native_members"][0])))
        elif change == "bool_number_member":
            setup["helper"]["member"]["member"] = True
            receipt["helper"]["member"] = True
            setup["native_members"][0]["member"]["member"] = 1
    def after(receipt):
        if change == "receipt_path":
            receipt["helper_initial_surface"]["path"] = str(f.helper_source_surface.path)
        elif change == "missing_receipt":
            receipt.pop("helper_initial_surface")
        elif change == "surface_bytes":
            write(f.output / "helper-initial-surface.json", b"[]")
        elif change == "manifest_surface":
            path = f.output / "manifest.json"
            manifest = json.loads(path.read_bytes())
            manifest["artifacts"].pop()
            receipt["manifest"] = write(path, content(manifest)).pin()
    f.mutate, f.after_files = mutate, after
    result = f.run()
    assert result.status == "outcome_unknown"
    assert result.setup_bundle is None
    assert f.completion()["setup_bundle"] is None
    assert len(f.calls) == 1
    assert f.output.exists()


@pytest.mark.parametrize("change", ["bytes", "symlink"])
def test_original_surface_changed_during_await_keeps_receipt_historical(tmp_path, change):
    f = Fixture(tmp_path)
    def after(receipt):
        path = f.helper_source_surface.path
        if change == "bytes":
            path.write_bytes(b"[]")
        else:
            path.unlink()
            path.symlink_to(f.source.path)
    f.after_files = after
    result = f.run()
    assert result.status == "source_changed"
    assert result.setup_bundle is None
    saved = f.completion()
    assert saved["receipt"]["helper_initial_surface"]
    assert saved["process"]["returncode"] == 0
    assert saved["setup_bundle"] is None
    assert len(f.calls) == 1
    retained_attempt = f.attempt.read_bytes()
    with pytest.raises(CustodyError):
        f.run()
    assert f.attempt.read_bytes() == retained_attempt
    assert len(f.calls) == 1


@pytest.mark.parametrize("change", ["source_bytes", "source_deleted", "emitted_bytes", "emitted_symlink"])
def test_restart_authentication_rechecks_both_retained_surfaces(tmp_path, change):
    f = Fixture(tmp_path)
    bundle = f.run().setup_bundle
    assert bundle is not None
    if change == "source_bytes":
        bundle.helper_source_surface.path.write_bytes(b"[]")
    elif change == "source_deleted":
        bundle.helper_source_surface.path.unlink()
    elif change == "emitted_bytes":
        bundle.helper_initial_surface.path.write_bytes(b"[]")
    else:
        bundle.helper_initial_surface.path.unlink()
        bundle.helper_initial_surface.path.symlink_to(bundle.helper_source_surface.path)
    with pytest.raises((CustodyError, FileNotFoundError)):
        bundle.authenticate()
