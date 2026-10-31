"""Inert consumption of original Cargo and shipped-derivation evidence."""
from copy import deepcopy
from dataclasses import replace
from pathlib import Path
import zipfile

import pytest

from launchkit import runtime_candidates as candidates
from test_meta_toolkit_runtime_selection import (product, no_product_execution,
    linux_product, put, pin, json_bytes, changed_assembly, digest, GATEWAY_BYTES)
from test_distribution import bundle_source


@pytest.fixture(autouse=True)
def linux_inputs(monkeypatch):
    linux_product(monkeypatch)


def derivation(f, defect=None):
    # These original producer paths do not exist on this consumer. Only the
    # relocated record copies and the selected shipped archive may be opened.
    original = {"path": "/retained-producer/cargo/toolkit_gateway",
        "sha256": digest(GATEWAY_BYTES + b"inert debug information"), "size": len(GATEWAY_BYTES) + 23}
    shipped = {"path": "/retained-producer/derived/toolkit_gateway",
        **f.assembly["qualification"]["candidate"]["binary"]}
    tool = {"path": "/usr/bin/x86_64-linux-gnu-objcopy",
        "sha256": "9276e918e3c26d3aa7ef9dd2232cd8ac692f03d3c1f270bfa5144a2b5492b713",
        "version": "GNU objcopy (GNU Binutils for Ubuntu) 2.42"}
    argv = [tool["path"], "--strip-debug", original["path"], shipped["path"]]
    start = {"schema_version": 1, "kind": "meerkat_connect_strip_debug_start",
        "binary_kind": "toolkit_gateway", "argv": argv, "tool": tool,
        "input": original, "output": {"path": shipped["path"]}}
    if defect == "start-input": start["input"] = {**original, "sha256": "f" * 64}
    if defect == "start-tool": start["tool"] = {**tool, "sha256": "f" * 64}
    start_pin = put(f.final / "transformation-start.json", json_bytes(start))
    result = {"schema_version": 1, "kind": "meerkat_connect_strip_debug_result",
        "start_sha256": start_pin.sha256, "exit_code": 0, "class": "ok",
        "input": original, "output": shipped, "tool": {key: tool[key] for key in ("path", "sha256")}}
    if defect == "failed-transform": result["exit_code"] = 4
    if defect == "boolean-exit": result["exit_code"] = False
    if defect == "wrong-start": result["start_sha256"] = "f" * 64
    if defect == "changed-original": result["input"] = {**original, "size": original["size"] + 1}
    if defect == "wrong-result": result["output"] = {**shipped, "sha256": "f" * 64}
    if defect == "timeout": result["class"] = "timeout"
    result_pin = put(f.final / "transformation-result.json", json_bytes(result))
    record = {"schema_version": 1, "kind": "meerkat_connect_derived_gateway", "derivation": "strip_debug",
        "binary_kind": "toolkit_gateway", "original": original, "derived": shipped, "tool": tool,
        "argv": argv, "transformation": {
            "start": {"path": "/retained-producer/derived/START.json", "sha256": start_pin.sha256},
            "result": {"path": "/retained-producer/derived/RESULT.json", "sha256": result_pin.sha256}}}
    if defect == "strip-all": record["argv"] = [tool["path"], "--strip-all", original["path"], shipped["path"]]
    if defect == "provider-kind": record["binary_kind"] = "meerkat-connect"
    if defect == "same-path": record["derived"] = {**shipped, "path": original["path"]}
    if defect == "relative-producer": record["original"] = {**original, "path": "original/toolkit_gateway"}
    record_pin = put(f.final / "derived-binary-record.json", json_bytes(record))
    summary = {"record_sha256": record_pin.sha256, "kind": record["kind"], "derivation": "strip_debug",
        "original": {key: original[key] for key in ("sha256", "size")},
        "derived": {key: shipped[key] for key in ("sha256", "size")},
        "transformation": {"start_sha256": start_pin.sha256, "result_sha256": result_pin.sha256,
            "argv": record["argv"], "tool": tool, "exit_code": 0},
        "validation": {"fixture": "INERT - no ELF comparison or native qualification"}}
    if defect == "summary-original": summary["original"]["sha256"] = "f" * 64
    if defect == "summary-derived": summary["derived"]["sha256"] = "f" * 64
    if defect == "summary-result": summary["transformation"]["result_sha256"] = "f" * 64
    f.build["binary"] = deepcopy(original)
    if defect == "relabelled-build": f.build["binary"] = deepcopy(shipped)
    build_pin = put(f.final / "build-record.json", json_bytes(f.build))
    f.assembly["build_record_sha256"] = build_pin.sha256
    f.assembly["derived_binary"] = summary
    f.report["installed_inputs"]["native_build"] = pin(build_pin, sized=True)
    f.report["installed_inputs"]["derived_binary"] = {
        "path": str(Path(f.report["installed_inputs"]["staging"]["path"]).parent / "derived-binary-record.json"),
        "sha256": record_pin.sha256, "bytes": record_pin.size}
    if defect == "verifier-missing": f.report["installed_inputs"].pop("derived_binary")
    if defect == "verifier-wrong": f.report["installed_inputs"]["derived_binary"]["sha256"] = "f" * 64
    if defect == "verifier-size": f.report["installed_inputs"]["derived_binary"]["bytes"] += 1
    if defect == "verifier-path": f.report["installed_inputs"]["derived_binary"]["path"] = "/other/derived-binary-record.json"
    report_pin = put(f.final / "gateway-verification.json", json_bytes(f.report))
    f.assembly["qualification"]["installed_gateway_verification_sha256"] = report_pin.sha256
    if defect == "dropped-derivation": f.assembly.pop("derived_binary")
    if defect == "null-derivation": f.assembly["derived_binary"] = None
    f.target = changed_assembly(f)
    return f.target


def test_reader_joins_original_and_shipped_without_opening_producer_paths(product):
    target = derivation(product)
    build_before = (product.final / "build-record.json").read_bytes()
    observed = candidates.read_meta_toolkit_artifacts(target)
    assert observed.candidate == product.assembly["qualification"]["candidate"]
    assert observed.candidate["binary"]["sha256"] != product.build["binary"]["sha256"]
    assert (product.final / "build-record.json").read_bytes() == build_before
    assert {"derived-binary-record.json", "transformation-start.json", "transformation-result.json"} <= {
        item.path.name for item in observed.evidence}
    assert candidates.recheck_meta_toolkit_artifacts(observed, target) is observed
    assert not hasattr(observed, "qualified")


@pytest.mark.parametrize("defect", ["start-input", "start-tool", "failed-transform", "boolean-exit",
    "wrong-start", "changed-original", "wrong-result", "timeout", "strip-all", "provider-kind", "same-path",
    "relative-producer", "summary-original", "summary-derived", "summary-result", "relabelled-build",
    "verifier-missing", "verifier-wrong", "verifier-size", "verifier-path", "dropped-derivation", "null-derivation"])
def test_reader_refuses_inconsistent_original_derived_and_verifier_joins(product, defect):
    target = derivation(product, defect)
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.read_meta_toolkit_artifacts(target)


@pytest.mark.parametrize("name", ["derived-binary-record.json", "transformation-start.json", "transformation-result.json"])
@pytest.mark.parametrize("when", ["initial", "recheck"])
def test_reader_keeps_each_original_derivation_copy_pinned(product, name, when):
    target = derivation(product)
    observed = candidates.read_meta_toolkit_artifacts(target) if when == "recheck" else None
    path = product.final / name
    path.write_bytes(path.read_bytes() + b" ")
    with pytest.raises(candidates.CandidateArtifactError):
        if observed is None:
            candidates.read_meta_toolkit_artifacts(target)
        else:
            candidates.recheck_meta_toolkit_artifacts(observed, target)


def test_unstripped_path_still_requires_direct_build_identity(product):
    product.build["binary"]["sha256"] = "f" * 64
    build = put(product.final / "build-record.json", json_bytes(product.build))
    product.assembly["build_record_sha256"] = build.sha256
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.read_meta_toolkit_artifacts(changed_assembly(product))


def test_derivation_cannot_be_selected_for_another_platform(product):
    derivation(product)
    product.assembly["target"] = "macos-arm64"
    candidate = product.assembly["qualification"]["candidate"]
    candidate["target"] = "macos-arm64"
    product.target = replace(product.target, target="macos-arm64")
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.read_meta_toolkit_artifacts(changed_assembly(product))


def test_package_transports_original_evidence_and_verifies_after_relocation(product, bundle_source,
        tmp_path, monkeypatch):
    from launchkit import product_distribution
    from test_development_product_selection import guide_product_inputs

    target = derivation(product)
    root, _, _, guide = guide_product_inputs(product, bundle_source, monkeypatch)
    originals = {name: (product.final / name).read_bytes() for name in (
        "build-record.json", "derived-binary-record.json", "transformation-start.json", "transformation-result.json")}
    output = tmp_path / "inert-derived-product.zip"
    result = product_distribution.package(root, target, guide, output)
    assert result["native_executed"] is False and result["installed_use_accepted"] is False
    extracted = tmp_path / "relocated-derived-product"
    with zipfile.ZipFile(output) as archive:
        archive.extractall(extracted)
    manifest = product_distribution.verify(extracted)
    final = extracted / Path(manifest["product"]["paths"]["assembly"]).parent
    for name, raw in originals.items():
        assert (final / name).read_bytes() == raw
        assert (final / name).relative_to(extracted).as_posix() in manifest["files"]
    assert manifest["product"]["selection"]["target"] == "linux-x86_64"


@pytest.mark.parametrize("changed_stage", [False, True])
def test_development_derivation_preserves_unqualified_origin_and_staging_join(product, monkeypatch, changed_stage):
    import test_development_product_selection as development
    monkeypatch.setattr(development, "PREFIX", "meerkat-connect-linux-x86_64")
    derivation(product)
    target, descriptor, staging = development.development(product)
    descriptor["derived_binary"] = deepcopy(product.assembly["derived_binary"])
    staging["derived_binary"] = deepcopy(descriptor["derived_binary"])
    if changed_stage: staging["derived_binary"]["record_sha256"] = "f" * 64
    descriptor["staging_record_sha256"] = put(product.final / "staging.json", json_bytes(staging)).sha256
    target = replace(target, assembly=put(target.assembly.path, json_bytes(descriptor)))
    (product.final / "gateway-verification.json").unlink()
    if changed_stage:
        with pytest.raises(candidates.CandidateArtifactError):
            candidates.read_meta_toolkit_artifacts(target)
    else:
        observed = candidates.read_meta_toolkit_artifacts(target)
        assert observed.origin == "source-built-development"
        assert not any(item.path.name == "gateway-verification.json" for item in observed.evidence)
        assert not hasattr(observed, "qualified")
