"""Inert supplemental ELF transport and refusal controls, never native qualification."""
from dataclasses import replace
import json
import os
import stat
import zipfile

import pytest

from launchkit import distribution, product_distribution as packaging
from test_distribution import bundle_source
from test_development_product_selection import guide_product_inputs
from test_meta_toolkit_runtime_selection import product, no_product_execution, linux_product, put


def inert_elf(label):
    header = bytearray(64)
    header[:7] = b"\x7fELF\x02\x01\x01"
    header[16:24] = b"\x03\x00\x3e\x00\x01\x00\x00\x00"
    header[52:54] = b"\x40\x00"
    return bytes(header) + b"NOT EXECUTABLE: INERT TEST FIXTURE " + label.encode()


@pytest.fixture
def inputs(monkeypatch, request, tmp_path):
    linux_product(monkeypatch)
    specimen = request.getfixturevalue("product")
    bundle = request.getfixturevalue("bundle_source")
    root, _, _, guide = guide_product_inputs(specimen, bundle, monkeypatch)
    target = specimen.target
    programs = {role: {
        "executable": put(specimen.root / role / "program", inert_elf(role)),
        "evidence": put(specimen.root / role / "original.json", b'{"scope":"inert source-only", "producer_path":"/original/path"}\n')}
        for role in ("collection", "provider")}
    return root, target, guide, programs


def package(inputs, tmp_path):
    root, target, guide, programs = inputs
    archive = tmp_path / "native.zip"
    result = packaging.package(root, target, guide, archive, native_programs=programs)
    extracted = tmp_path / "extracted"
    with zipfile.ZipFile(archive) as source:
        source.extractall(extracted)
    return result, extracted


def test_package_preserves_separate_program_bytes_and_original_evidence(inputs, tmp_path):
    result, extracted = package(inputs, tmp_path)
    assert result["native_executed"] is False and result["installed_use_accepted"] is False
    observed = packaging.verify(extracted)
    assert observed["support"]["installed_use_accepted"] is False
    for role, pins in inputs[3].items():
        for kind, pin in pins.items():
            binding = observed["native_programs"][role][kind]
            assert binding == {"path": packaging._native_paths(role)[kind], "sha256": pin.sha256, "bytes": pin.size}
            path = extracted / binding["path"]
            assert path.read_bytes() == pin.path.read_bytes()
            assert stat.S_IMODE(path.stat().st_mode) == (0o700 if kind == "executable" else 0o600)
            assert stat.S_IMODE(path.parent.stat().st_mode) == 0o700


@pytest.mark.parametrize("change", ["executable", "evidence", "record", "mode", "missing-binding", "missing-evidence", "unknown-role", "swap-roles", "wrong-target"])
def test_unpacked_native_bindings_refuse_tampering(inputs, tmp_path, change):
    _, extracted = package(inputs, tmp_path)
    value = packaging.verify(extracted)
    row = value["native_programs"]["collection"]
    if change in {"executable", "evidence"}:
        (extracted / row[change]["path"]).write_bytes(b"tampered")
    elif change == "record":
        row["executable"]["sha256"] = "a" * 64
    elif change == "mode":
        value["file_modes"][row["executable"]["path"]] = 0o755
    elif change == "missing-binding":
        value.pop("native_programs")
    elif change == "missing-evidence":
        row.pop("evidence")
    elif change == "unknown-role":
        value["native_programs"]["unknown"] = row
    elif change == "swap-roles":
        value["native_programs"]["collection"] = value["native_programs"]["provider"]
    else:
        value["product"]["selection"]["target"] = "macos-arm64"
    (extracted / distribution.MANIFEST).write_text(json.dumps(value))
    with pytest.raises(distribution.DistributionError):
        packaging.verify(extracted)


@pytest.mark.parametrize("change", ["pin", "size", "missing-evidence", "alias", "symlink", "hardlink", "script", "arm-elf", "wrong-target", "unknown-role", "empty"])
def test_native_package_inputs_refuse_invalid_pins_before_archive_creation(inputs, tmp_path, change):
    root, target, guide, programs = inputs
    row = programs["collection"]
    if change == "pin":
        row["executable"] = replace(row["executable"], sha256="a" * 64)
    elif change == "size":
        row["evidence"] = replace(row["evidence"], size=1)
    elif change == "missing-evidence":
        row.pop("evidence")
    elif change == "alias":
        programs["provider"]["executable"] = row["executable"]
    elif change in {"symlink", "hardlink"}:
        path = row["executable"].path
        original = path.with_name("original")
        path.rename(original)
        if change == "symlink":
            path.symlink_to(original)
        else:
            os.link(original, path)
    elif change in {"script", "arm-elf"}:
        content = b"#!/bin/sh\nexit 0\n" if change == "script" else inert_elf("arm")[:18] + b"\xb7\x00" + inert_elf("arm")[20:]
        row["executable"] = put(row["executable"].path, content)
    elif change == "wrong-target":
        with pytest.raises(distribution.DistributionError):
            packaging._native_inputs(programs, "macos-arm64")
        return
    elif change == "unknown-role":
        programs["unknown"] = row
    else:
        programs = {}
    output = tmp_path / "refused.zip"
    with pytest.raises(distribution.DistributionError):
        packaging.package(root, target, guide, output, native_programs=programs)
    assert not output.exists()
