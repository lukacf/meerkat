"""Inert source-only product fixtures, never qualified native or SDK packages.

The JSON models the existing producer's final assembly and verifier relationships.
Synthetic ``passed`` fields exercise consumption of held records only. No package
assembly, qualification, native program, selected SDK or dependency is executed.
"""
from dataclasses import replace
import gzip
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tarfile
from types import SimpleNamespace

import pytest

from launchkit import runtime_candidates as candidates
from launchkit import runtime_selection as selection
from launchkit.templates import install_candidate as installed
from test_sdk_wheel_integrity import InertWheel, archive_bytes, digest


FIXTURE = "INERT_SOURCE_ONLY_FIXTURE_NOT_QUALIFIED"
TARGET = "macos-arm64"
PREFIX = "meerkat-connect-" + TARGET
RESOURCE = PREFIX + "/libexec/meerkat-connect"
# Further fields of the resource component's build record, such as the
# supported host that the Connect resource packager declares.
RESOURCE_RECORD = {}
GATEWAY_BYTES = b"INERT gateway bytes, must never execute\n"
SDK_BYTES = b"raise RuntimeError('inert fixture must never import or execute')\n"


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def put(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    path.chmod(0o600)
    return candidates.ArtifactFile(path, len(content), digest(content))


def pin(artifact, *, sized=False):
    result = {"path": str(artifact.path), "sha256": artifact.sha256}
    if sized:
        result["bytes"] = artifact.size
    return result


def prefix_bytes(nodes):
    """Encode inert files and confined links in the producer's PAX/gzip layout."""
    output = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for name, (kind, mode, value) in sorted(nodes.items()):
                member = tarfile.TarInfo(name)
                member.mode = mode
                if kind == "directory":
                    member.type = tarfile.DIRTYPE
                    archive.addfile(member)
                elif kind == "link":
                    member.type = tarfile.SYMTYPE
                    member.linkname = value
                    archive.addfile(member)
                else:
                    member.size = len(value)
                    archive.addfile(member, io.BytesIO(value))
    return output.getvalue()


def dependency_bytes():
    prefix = "fixture_dependency-1.0.0.dist-info/"
    values = {"fixture_dependency/__init__.py": SDK_BYTES,
        prefix + "METADATA": b"Metadata-Version: 2.4\nName: fixture-dependency\nVersion: 1.0.0\n\n",
        prefix + "WHEEL": b"Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n\n"}
    return archive_bytes(values, prefix)


@pytest.fixture(autouse=True)
def no_product_execution(monkeypatch):
    def forbidden(*args, **kwargs):
        pytest.fail("An inert source-only fixture attempted to execute a process.")
    monkeypatch.setattr(subprocess, "Popen", forbidden)
    monkeypatch.setattr(subprocess, "run", forbidden)
    monkeypatch.setattr(os, "system", forbidden)
    before = {name for name in sys.modules if name.split(".")[0] in {
        "meerkat_meta", "meerkat_toolkit", "fixture_dependency"}}
    yield
    after = {name for name in sys.modules if name.split(".")[0] in {
        "meerkat_meta", "meerkat_toolkit", "fixture_dependency"}}
    assert after == before, "The integrity and copy path must not import selected product code."


@pytest.fixture
def product(tmp_path):
    root = tmp_path.resolve() / "inert source-only inputs"
    root.mkdir(mode=0o700)
    final = root / "final"
    final.mkdir(mode=0o700)
    wheels = (InertWheel("meerkat-meta"), InertWheel("meerkat-toolkit"))
    for name in ("agent_builder.py", "identity_first_models.py", "identity_first_providers.py", "types.py"):
        wheels[0].payload["meerkat_meta/" + name] = SDK_BYTES
    selected_wheels = [wheel.selected() for wheel in wheels]
    meta_wheel = put(root / "meerkat_meta-0.9.0-py3-none-any.whl", selected_wheels[0][0])
    toolkit_wheel = put(root / f"meerkat_toolkit-{wheels[1].version}-py3-none-any.whl", selected_wheels[1][0])
    dependency = put(root / "fixture_dependency-1.0.0-py3-none-any.whl", dependency_bytes())
    requirements = put(root / "dependencies.hashes.txt",
        f"# {FIXTURE}\nfixture-dependency==1.0.0 --hash=sha256:{dependency.sha256}\n".encode())
    source = put(root / "source-manifest.json", json_bytes({"fixture": FIXTURE,
        "core_source_sha": "1" * 40, "meta_source_sha": "2" * 40}))
    resource_bytes = json_bytes({"fixture": FIXTURE, "kind": "meerkat_connect_resources",
        "target": TARGET, "prepare": "prepare.py", "files": []})
    resource_sha = digest(resource_bytes)
    nodes = {PREFIX: ("directory", 0o755, None), PREFIX + "/bin": ("directory", 0o755, None),
        PREFIX + "/bin/toolkit_gateway": ("file", 0o755, GATEWAY_BYTES),
        PREFIX + "/libexec": ("directory", 0o755, None), RESOURCE: ("directory", 0o555, None),
        RESOURCE + "/resources.json": ("file", 0o444, resource_bytes),
        RESOURCE + "/prepare.py": ("file", 0o444, SDK_BYTES),
        RESOURCE + "/real-file": ("file", 0o444, b"inert resource\n"),
        RESOURCE + "/python-runtime": ("directory", 0o555, None),
        RESOURCE + "/python-runtime/bin": ("directory", 0o555, None),
        RESOURCE + "/python-runtime/bin/python": ("file", 0o555, b"inert interpreter, never execute\n"),
        RESOURCE + "/runtime-link": ("link", 0o777, "python-runtime"),
        RESOURCE + "/python": ("link", 0o777, "runtime-link/bin/python"),
        RESOURCE + "/python-current": ("link", 0o777, "python"),
        PREFIX + "/LICENSE-MIT": ("file", 0o444, b"inert license\n"),
        PREFIX + "/README.md": ("file", 0o444, (FIXTURE + "\n").encode())}
    archive = put(final / ("meerkat-toolkit-gateway-preview-" + TARGET + ".tar.gz"), prefix_bytes(nodes))
    resource_component_sha = "3" * 64
    resources = {"fixture": FIXTURE, "schema_version": 1, "target": TARGET,
        "resources_sha256": resource_sha, "archive_sha256": resource_component_sha,
        "archive_bytes": 1, "prepare_source_sha256": digest(SDK_BYTES),
        "input_selection": {"kind": "explicit_pinned_connect_resource_inputs"}, **RESOURCE_RECORD}
    resource_record = put(final / "resource-build-record.json", json_bytes(resources))
    candidate = {"target": TARGET, "source_manifest_sha256": source.sha256,
        "resources_sha256": resource_sha, "binary_kind": "toolkit_gateway",
        "binary": {"sha256": digest(GATEWAY_BYTES), "size": len(GATEWAY_BYTES)},
        "resource_build_record_sha256": resource_record.sha256}
    build = {"fixture": FIXTURE, "schema_version": 1, "target": TARGET,
        "binary_kind": "toolkit_gateway", "binary": {"path": str(root / "native/toolkit_gateway"),
            **candidate["binary"]}, "source_manifest": pin(source), "resources_sha256": resource_sha,
        "resource_build_record_sha256": resource_record.sha256,
        "cargo": {"argv": ["inert-cargo", "--bin", "toolkit_gateway"], "exit_code": 0,
            "env": {"MEERKAT_CONNECT_RESOURCES_SHA256": resource_sha}}}
    build_record = put(final / "build-record.json", json_bytes(build))
    catalog = {"fixture": FIXTURE, "models": [], "provider_defaults": {}}
    model_catalog = put(root / "verifier/installed-product-success/model-catalog.json", json_bytes(catalog))
    sdk_sources = []
    for wheel in wheels:
        for name, content in wheel.payload.items():
            if name.endswith(".py") and (wheel.distribution == "meerkat-meta" or not name.endswith("/__init__.py")):
                sdk_sources.append(pin(put(root / "sdk-source" / name, content)))
    report = {"fixture": FIXTURE, "schema": 1, "status": "passed",
        "execution_claim": "Inert record only; no native execution or qualification occurred.",
        "installed_candidate": candidate, "toolkit_gateway_sha256": candidate["binary"]["sha256"],
        "raw_gateway": None, "raw_gateway_sha256": None, "sdk_sources": sdk_sources,
        "installed_inputs": {"staging": {"path": str(root / "staging/staging.json"), "sha256": "4" * 64},
            "native_build": pin(build_record, sized=True), "resource_build": pin(resource_record, sized=True),
            "source_manifest": pin(source, sized=True),
            "resource_manifest": {"path": str(root / "staging" / RESOURCE / "resources.json"),
                "sha256": resource_sha, "bytes": len(resource_bytes)}},
        "cases": [{"case": "installed-product-success", "status": "passed", "candidate": candidate,
                "model_catalog": pin(model_catalog)},
            {"case": "installed-product-resource-corruption", "status": "passed", "candidate": candidate}]}
    verification = put(final / "gateway-verification.json", json_bytes(report))
    assembly = {"fixture": FIXTURE, "schema_version": 1, "target": TARGET,
        "scope": "installed_gateway_schema4_resource_startup_and_shutdown_preview",
        "binary_kind": "toolkit_gateway", "not_qualified": ["model_execution", "account_sign_in",
            "acquisition", "component_install", "installed_integration_realm_persistence", "frontend_activation", "other_targets"],
        "archive": archive.path.name, "archive_sha256": archive.sha256, "archive_bytes": archive.size,
        "binary_sha256": candidate["binary"]["sha256"], "binary_bytes": candidate["binary"]["size"],
        "resources_sha256": resource_sha, "resource_component_sha256": resource_component_sha,
        "resource_build_record_sha256": resource_record.sha256, "build_record_sha256": build_record.sha256,
        "qualification": {"candidate": candidate, "installed_gateway_verification_sha256": verification.sha256,
            "cli_tty_qualified": False}, "licenses": {"LICENSE-MIT": digest(b"inert license\n")},
        "readme_sha256": digest((FIXTURE + "\n").encode()), "resource_closure_unchanged": True}
    assembly_file = put(final / "assembly.json", json_bytes(assembly))
    target = candidates.MetaToolkitRuntimeTarget(root, TARGET, assembly_file, meta_wheel,
        selected_wheels[0][1], toolkit_wheel, selected_wheels[1][1], requirements, (dependency,), model_catalog)
    yield SimpleNamespace(root=root, final=final, wheels=wheels, nodes=nodes, archive=archive,
        source=source, build=build, resources=resources, report=report, catalog=catalog,
        assembly=assembly, target=target)
    # Only this fixture's temporary owned tree is made removable. Exact modes
    # are asserted during the test; no retained/shared artifact is touched.
    for directory, children, files in os.walk(tmp_path, followlinks=False):
        for name in [*children, *files]:
            path = Path(directory) / name
            if not path.is_symlink() and path.stat().st_uid == os.getuid():
                path.chmod(stat.S_IMODE(path.stat().st_mode) | stat.S_IWUSR)


def changed_assembly(product):
    return replace(product.target, assembly=put(product.target.assembly.path, json_bytes(product.assembly)))


LINUX_HOST = {"platform": "linux", "machine": "x86_64", "pointer_bits": 64, "glibc": "glibc 2.39"}


def linux_product(monkeypatch, supported_host=LINUX_HOST):
    """Build the next product fixture for Linux x86_64.

    Its resource build record declares supported_host as the Connect resource
    packager does; None leaves that declaration out.
    """
    module = sys.modules[__name__]
    prefix = "meerkat-connect-linux-x86_64"
    monkeypatch.setattr(module, "TARGET", "linux-x86_64")
    monkeypatch.setattr(module, "PREFIX", prefix)
    monkeypatch.setattr(module, "RESOURCE", prefix + "/libexec/meerkat-connect")
    monkeypatch.setattr(module, "RESOURCE_RECORD", {} if supported_host is None else {"runtime_compatibility": {
        "supported_host": supported_host,
        "scope": "hash_bound_archive_metadata_and_declared_host_only_not_runtime_qualification"}})


def test_final_product_reader_preserves_complete_inert_prefix_without_admission(product):
    observed = candidates.read_meta_toolkit_artifacts(product.target)
    assert observed.selection == product.target and observed.archive == product.archive
    assert observed.catalog == product.catalog and len(observed.entries) == len(product.nodes)
    for entry in observed.entries:
        kind, mode, value = product.nodes[entry.name]
        assert (entry.kind, entry.mode) == (kind, mode)
        if kind == "file":
            assert (entry.size, entry.sha256) == (len(value), digest(value))
        elif kind == "link":
            assert entry.link == value
    assert not hasattr(observed, "admitted") and not hasattr(observed, "qualified")
    assert candidates.recheck_meta_toolkit_artifacts(observed, product.target) is observed


def test_protected_serialization_roundtrip_retains_every_selected_byte_contract(product):
    rendered = selection.candidate_selection(product.target, deployment_target="mac")
    decoded = json.loads(json.dumps(rendered))
    assert decoded["format"] == selection.PRODUCT_SELECTION_FORMAT
    assert decoded["meta"]["version"] == decoded["toolkit"]["version"] == "0.9.0"
    assert selection.parse_candidate_selection(decoded) == product.target
    assert selection.selected_runtime_marker(product.target)["assembly_sha256"] == product.target.assembly.sha256
    assert rendered["dependency_requirements"]["sha256"] == product.target.dependency_requirements.sha256
    assert rendered["meta"]["source_payload"] and rendered["toolkit"]["metadata"]
    assert "source_manifest" not in rendered and product.target.source_manifest is None
    assert selection.candidate_selection(selection.parse_candidate_selection(decoded)) == rendered
    with pytest.raises(ValueError, match="platform"):
        selection.candidate_selection(product.target, deployment_target="linux")


def test_pure_protected_parser_does_not_qualify_or_open_selected_artifacts(product, monkeypatch):
    rendered = selection.candidate_selection(product.target)
    def forbidden(*args, **kwargs):
        pytest.fail("Historical identity parsing must not perform artifact admission.")
    monkeypatch.setattr(selection, "read_meta_toolkit_artifacts", forbidden)
    assert selection.parse_candidate_selection(rendered) == product.target


@pytest.mark.parametrize("target", [[], {}])
def test_protected_product_parser_refuses_container_target_without_artifact_reads(product, monkeypatch, target):
    rendered = selection.candidate_selection(product.target)
    rendered["target"] = target

    def forbidden(*args, **kwargs):
        pytest.fail("Malformed historical selection must not open artifact inputs.")

    monkeypatch.setattr(selection, "read_meta_toolkit_artifacts", forbidden)
    with pytest.raises(ValueError, match="selected native product target is invalid"):
        selection.parse_candidate_selection(json.loads(json.dumps(rendered)))


@pytest.fixture
def portable_product(product, tmp_path):
    """Copy selected inert bytes; retain the producer's original record paths."""
    root = tmp_path.resolve() / "relocated exact artifact inputs"
    root.mkdir(mode=0o700)
    final = root / "final"
    final.mkdir(mode=0o700)
    for name in ("build-record.json", "resource-build-record.json", "gateway-verification.json",
            product.archive.path.name):
        put(final / name, (product.final / name).read_bytes())
    assembly = put(final / product.target.assembly.path.name, product.target.assembly.path.read_bytes())
    def copy_wheel(artifact):
        return put(root / "wheels" / artifact.path.name, artifact.path.read_bytes())
    source = put(root / "evidence/selected-native-source.json", product.source.path.read_bytes())
    catalog = put(root / "evidence/selected-model-catalog.json", product.target.model_catalog.path.read_bytes())
    target = replace(product.target, artifact_root=root, assembly=assembly,
        meta_wheel=copy_wheel(product.target.meta_wheel),
        toolkit_wheel=copy_wheel(product.target.toolkit_wheel),
        dependency_requirements=put(root / "offline.hashes.txt", product.target.dependency_requirements.path.read_bytes()),
        dependency_wheels=tuple(copy_wheel(item) for item in product.target.dependency_wheels),
        model_catalog=catalog, source_manifest=source)
    return SimpleNamespace(root=root, final=final, source=source, target=target, original=product)


def test_explicit_local_pins_admit_relocated_exact_bytes_without_rewriting_provenance(portable_product):
    f = portable_product
    observed = candidates.read_meta_toolkit_artifacts(f.target)
    assert observed.catalog == f.original.catalog
    assert observed.entries == candidates.read_meta_toolkit_artifacts(f.original.target).entries
    assert all(item.path.is_relative_to(f.root) for item in observed.evidence)
    assert f.source in observed.evidence and f.target.model_catalog in observed.evidence
    for name in ("assembly.json", "build-record.json", "resource-build-record.json", "gateway-verification.json"):
        assert (f.final / name).read_bytes() == (f.original.final / name).read_bytes()
    assert not Path(f.original.build["source_manifest"]["path"]).is_relative_to(f.root)
    assert f.original.report["cases"][0]["model_catalog"]["path"] != str(f.target.model_catalog.path)
    rendered = selection.candidate_selection(f.target)
    assert rendered["source_manifest"] == {"path": str(f.source.path),
        "size": f.source.size, "sha256": f.source.sha256}
    assert selection.parse_candidate_selection(json.loads(json.dumps(rendered))) == f.target
    assert candidates.recheck_meta_toolkit_artifacts(observed, f.target) is observed


def test_relocated_legacy_selection_refuses_instead_of_searching_for_source(portable_product):
    with pytest.raises(candidates.CandidateArtifactError, match="artifact root"):
        candidates.read_meta_toolkit_artifacts(replace(portable_product.target, source_manifest=None))


@pytest.mark.parametrize("defect", ["changed-bytes", "reselected-digest", "wrong-size", "outside-root"])
def test_explicit_source_pin_requires_exact_same_owned_native_source(portable_product, defect):
    f = portable_product
    target = f.target
    if defect == "changed-bytes":
        f.source.path.write_bytes(f.source.path.read_bytes() + b"changed")
    elif defect == "reselected-digest":
        target = replace(target, source_manifest=put(f.source.path, json_bytes({"fixture": FIXTURE, "changed": True})))
    elif defect == "wrong-size":
        target = replace(target, source_manifest=replace(f.source, size=f.source.size + 1))
    else:
        target = replace(target, source_manifest=f.original.source)
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.read_meta_toolkit_artifacts(target)


@pytest.mark.parametrize("defect", ["build-digest", "verifier-digest", "verifier-size", "verifier-size-missing"])
def test_explicit_source_joins_unchanged_build_and_verifier_bindings(portable_product, defect):
    f = portable_product
    assembly = json.loads(f.target.assembly.path.read_bytes())
    if defect == "build-digest":
        build = json.loads((f.final / "build-record.json").read_bytes())
        build["source_manifest"]["sha256"] = "f" * 64
        changed = put(f.final / "build-record.json", json_bytes(build))
        assembly["build_record_sha256"] = changed.sha256
    else:
        report = json.loads((f.final / "gateway-verification.json").read_bytes())
        source = report["installed_inputs"]["source_manifest"]
        if defect == "verifier-digest":
            source["sha256"] = "f" * 64
        elif defect == "verifier-size":
            source["bytes"] += 1
        else:
            source.pop("bytes")
        changed = put(f.final / "gateway-verification.json", json_bytes(report))
        assembly["qualification"]["installed_gateway_verification_sha256"] = changed.sha256
    target = replace(f.target, assembly=put(f.target.assembly.path, json_bytes(assembly)))
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.read_meta_toolkit_artifacts(target)


def test_relocated_catalog_cannot_substitute_a_new_digest(portable_product):
    f = portable_product
    catalog = put(f.target.model_catalog.path, json_bytes({"fixture": FIXTURE, "changed": True}))
    with pytest.raises(candidates.CandidateArtifactError, match="catalog"):
        candidates.read_meta_toolkit_artifacts(replace(f.target, model_catalog=catalog))


@pytest.mark.parametrize("defect", ["null", "string", "extra-pin-field", "extra-selection-field", "bad-hash", "relative-path"])
def test_portable_protected_parser_refuses_null_ambiguous_or_malformed_selection(portable_product, monkeypatch, defect):
    rendered = selection.candidate_selection(portable_product.target)
    if defect == "null":
        rendered["source_manifest"] = None
    elif defect == "string":
        rendered["source_manifest"] = rendered["source_manifest"]["path"]
    elif defect == "extra-pin-field":
        rendered["source_manifest"]["original_path"] = "/unused/producer/path"
    elif defect == "extra-selection-field":
        rendered["source_manifest_alias"] = rendered["source_manifest"]
    elif defect == "bad-hash":
        rendered["source_manifest"]["sha256"] = "not-a-digest"
    else:
        rendered["source_manifest"]["path"] = "relative-source.json"
    def forbidden(*args, **kwargs):
        pytest.fail("Malformed historical selection must not open artifact inputs.")
    monkeypatch.setattr(selection, "read_meta_toolkit_artifacts", forbidden)
    with pytest.raises(ValueError):
        selection.parse_candidate_selection(rendered)


@pytest.mark.parametrize("defect", ["staging", "platform", "binary", "resource"])
def test_relocated_selection_keeps_existing_final_candidate_checks(portable_product, defect):
    f = portable_product
    target = f.target
    if defect == "platform":
        target = replace(target, target="linux-x86_64")
    else:
        assembly = json.loads(target.assembly.path.read_bytes())
        if defect == "staging":
            assembly["scope"] = "unqualified_product_prefix_for_verification"
        elif defect == "binary":
            assembly["binary_sha256"] = "f" * 64
        else:
            assembly["resources_sha256"] = "f" * 64
        target = replace(target, assembly=put(target.assembly.path, json_bytes(assembly)))
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.read_meta_toolkit_artifacts(target)


@pytest.mark.parametrize("defect", ["staging", "candidate", "native-build", "resource-build", "verifier"])
def test_changed_producer_relationships_refuse_even_when_outer_assembly_is_reselected(product, defect):
    if defect == "staging":
        product.assembly["scope"] = "unqualified_product_prefix_for_verification"
    elif defect == "candidate":
        product.assembly["qualification"]["candidate"] = {**product.assembly["qualification"]["candidate"],
            "resources_sha256": "f" * 64}
    elif defect == "native-build":
        product.build["resources_sha256"] = "f" * 64
        changed = put(product.final / "build-record.json", json_bytes(product.build))
        product.assembly["build_record_sha256"] = changed.sha256
    elif defect == "resource-build":
        product.resources["resources_sha256"] = "f" * 64
        changed = put(product.final / "resource-build-record.json", json_bytes(product.resources))
        product.assembly["resource_build_record_sha256"] = changed.sha256
        product.assembly["qualification"]["candidate"]["resource_build_record_sha256"] = changed.sha256
    else:
        product.report["installed_inputs"]["resource_manifest"]["sha256"] = "f" * 64
        changed = put(product.final / "gateway-verification.json", json_bytes(product.report))
        product.assembly["qualification"]["installed_gateway_verification_sha256"] = changed.sha256
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.read_meta_toolkit_artifacts(changed_assembly(product))


def test_reselected_changed_sdk_source_cannot_reuse_previous_native_verifier(product):
    toolkit = product.wheels[1]
    toolkit.payload["meerkat_toolkit/host_control.py"] = SDK_BYTES
    content, expectation = toolkit.selected()
    target = replace(product.target, toolkit_wheel=put(product.target.toolkit_wheel.path, content), toolkit=expectation)
    with pytest.raises(candidates.CandidateArtifactError, match="imported SDK source"):
        candidates.read_meta_toolkit_artifacts(target)


@pytest.mark.parametrize("defect", ["empty", "missing", "duplicate"])
def test_product_reader_requires_original_verifier_ten_module_source_closure(product, defect):
    sources = product.report["sdk_sources"]
    if defect == "empty":
        product.report["sdk_sources"] = []
    elif defect == "missing":
        product.report["sdk_sources"] = sources[:-1]
    else:
        product.report["sdk_sources"] = [*sources[:-1], sources[0]]
    report = put(product.final / "gateway-verification.json", json_bytes(product.report))
    product.assembly["qualification"]["installed_gateway_verification_sha256"] = report.sha256
    with pytest.raises(candidates.CandidateArtifactError, match="source closure is incomplete"):
        candidates.read_meta_toolkit_artifacts(changed_assembly(product))


@pytest.mark.parametrize("field", ["meta_wheel", "toolkit_wheel", "model_catalog", "dependency_requirements", "assembly"])
def test_mutated_selected_bytes_are_rejected_before_they_can_be_used(product, field):
    artifact = getattr(product.target, field)
    artifact.path.write_bytes(artifact.path.read_bytes() + b"changed")
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.read_meta_toolkit_artifacts(product.target)


def test_reselected_catalog_cannot_replace_actual_verifier_catalog(product):
    catalog = put(product.target.model_catalog.path, json_bytes({"fixture": FIXTURE, "changed": True}))
    with pytest.raises(candidates.CandidateArtifactError, match="catalog"):
        candidates.read_meta_toolkit_artifacts(replace(product.target, model_catalog=catalog))


def test_archive_manifest_must_match_native_compiled_resource_pin(product):
    product.nodes[RESOURCE + "/resources.json"] = ("file", 0o444, b"changed inert resource manifest\n")
    archive = put(product.archive.path, prefix_bytes(product.nodes))
    product.assembly.update(archive_sha256=archive.sha256, archive_bytes=archive.size)
    with pytest.raises(candidates.CandidateArtifactError, match="resource manifest"):
        candidates.read_meta_toolkit_artifacts(changed_assembly(product))


@pytest.mark.parametrize("supported_host, floor", [
    (None, None),
    (LINUX_HOST, (2, 39)),
    ({**LINUX_HOST, "glibc": "glibc 2.41"}, (2, 41)),
    ({key: value for key, value in LINUX_HOST.items() if key != "glibc"}, "refused"),
    ({**LINUX_HOST, "glibc": "musl 1.2"}, "refused"),
    ({**LINUX_HOST, "glibc": "glibc 2"}, "refused"),
    ({**LINUX_HOST, "glibc": 2.39}, "refused"),
    ("glibc 2.39", "refused")],
    ids=["undeclared", "glibc-2.39", "glibc-2.41", "no-glibc", "musl", "no-minor", "number", "not-a-host"])
def test_linux_product_reader_carries_its_declared_glibc_floor(monkeypatch, request, supported_host, floor):
    # The resource build record is pinned through the assembly. The floor is
    # the supported host that record declares, not a Forge constant.
    linux_product(monkeypatch, supported_host)
    product = request.getfixturevalue("product")
    if floor == "refused":
        with pytest.raises(candidates.CandidateArtifactError, match="supported host"):
            candidates.read_meta_toolkit_artifacts(product.target)
        return
    observed = candidates.read_meta_toolkit_artifacts(product.target)
    assert observed.glibc_floor == floor
    assert candidates.recheck_meta_toolkit_artifacts(observed, product.target) is observed
    object.__setattr__(observed, "glibc_floor", (2, 17))
    with pytest.raises(candidates.CandidateArtifactError, match="metadata changed"):
        candidates.recheck_meta_toolkit_artifacts(observed, product.target)


@pytest.mark.parametrize("supported_host, refused", [
    ({"platform": "darwin", "machine": "arm64", "pointer_bits": 64}, False),
    ({"platform": "darwin", "machine": "arm64", "pointer_bits": 64, "glibc": "glibc 2.39"}, True)],
    ids=["declared-host", "glibc-on-macos"])
def test_macos_product_reader_declares_no_glibc_floor(monkeypatch, request, supported_host, refused):
    monkeypatch.setattr(sys.modules[__name__], "RESOURCE_RECORD", {"runtime_compatibility": {
        "supported_host": supported_host,
        "scope": "hash_bound_archive_metadata_and_declared_host_only_not_runtime_qualification"}})
    product = request.getfixturevalue("product")
    if refused:
        with pytest.raises(candidates.CandidateArtifactError, match="supported host"):
            candidates.read_meta_toolkit_artifacts(product.target)
    else:
        assert candidates.read_meta_toolkit_artifacts(product.target).glibc_floor is None


@pytest.mark.parametrize("supported_host, floor", [
    (None, None), (LINUX_HOST, (2, 39)), ({**LINUX_HOST, "glibc": "glibc 2"}, "refused")],
    ids=["undeclared", "glibc-2.39", "no-minor"])
def test_declared_glibc_floor_reads_only_the_pinned_assembly_and_resource_record(
        monkeypatch, request, supported_host, floor):
    # The early read for Stage: the floor without hashing the whole product.
    linux_product(monkeypatch, supported_host)
    product = request.getfixturevalue("product")
    product.archive.path.write_bytes(b"changed archive that this read never opens\n")
    if floor == "refused":
        with pytest.raises(candidates.CandidateArtifactError, match="supported host"):
            candidates.declared_glibc_floor(product.target)
    else:
        assert candidates.declared_glibc_floor(product.target) == floor
    record = product.final / "resource-build-record.json"
    record.write_bytes(record.read_bytes() + b"\n")
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.declared_glibc_floor(product.target)


@pytest.mark.parametrize("link", ["prepare.py/../real-file", "missing", "../../../../escape", "/absolute", "python-current"])
def test_prefix_refuses_file_traversal_dangling_escape_absolute_and_cycle(product, link):
    product.nodes[RESOURCE + "/python"] = ("link", 0o777, link)
    with pytest.raises(candidates.CandidateArtifactError):
        candidates.product_prefix_entries(prefix_bytes(product.nodes), PREFIX)


def test_prefix_preserves_valid_intermediate_directory_and_chained_links(product):
    entries = {entry.name: entry for entry in candidates.product_prefix_entries(prefix_bytes(product.nodes), PREFIX)}
    assert entries[RESOURCE + "/python"].link == "runtime-link/bin/python"
    assert entries[RESOURCE + "/python-current"].link == "python"
    assert entries[RESOURCE + "/runtime-link"].link == "python-runtime"


def test_prefix_copy_preserves_selected_files_directories_and_link_modes(product, tmp_path):
    artifacts = candidates.read_meta_toolkit_artifacts(product.target)
    deployment = tmp_path.resolve() / "inert byte-copy destination"
    deployment.mkdir(mode=0o700)
    checks = []
    def current():
        checks.append(True)
    old_umask = os.umask(0o077)
    try:
        gateway = installed.install_meta_toolkit_prefix(artifacts, deployment_root=deployment, require_current=current)
    finally:
        os.umask(old_umask)
    assert gateway == deployment / ".bin" / PREFIX / "bin/toolkit_gateway"
    assert gateway.read_bytes() == GATEWAY_BYTES and checks
    for entry in artifacts.entries:
        path = deployment / ".bin" / entry.name
        assert stat.S_IMODE(path.lstat().st_mode) == entry.mode
        if entry.kind == "link":
            assert os.readlink(path) == entry.link
    assert installed.verify_meta_toolkit_prefix(artifacts, deployment_root=deployment) == gateway


@pytest.mark.parametrize("disposition", ["false", "raises"])
def test_original_owner_refusal_precedes_any_prefix_capture_or_copy(product, tmp_path, monkeypatch, disposition):
    artifacts = candidates.read_meta_toolkit_artifacts(product.target)
    deployment = tmp_path.resolve() / "untouched destination"
    deployment.mkdir(mode=0o700)
    def forbidden(*args, **kwargs):
        pytest.fail("A refused original owner must precede any prefix read or copy.")
    monkeypatch.setattr(installed, "read_owned", forbidden)
    def current():
        if disposition == "raises":
            raise candidates.CandidateArtifactError("Original inert fixture owner changed.")
        return False
    with pytest.raises(candidates.CandidateArtifactError):
        installed.install_meta_toolkit_prefix(artifacts, deployment_root=deployment, require_current=current)
    assert list(deployment.iterdir()) == []


def test_native_product_bounds_do_not_reuse_legacy_flat_archive_limits(product, monkeypatch):
    monkeypatch.setattr(candidates, "MAX_ARCHIVE_BYTES", 1)
    monkeypatch.setattr(candidates, "MAX_BINARY_BYTES", 1)
    assert candidates.read_meta_toolkit_artifacts(product.target).archive == product.archive
