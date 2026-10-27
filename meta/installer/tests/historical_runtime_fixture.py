"""Pinned original v1 compiler inputs for inert historical contract coverage.

The .46 emission is a synthetic unit fixture from the retained original source.
It is not an installed .46 runtime or a historical deployment receipt.
"""
import hashlib
import importlib
import importlib.util
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import zipfile

import pytest

from launchkit import compiler, generator


@pytest.fixture(autouse=True, params=["historical-v1-046", "current-v3-047"])
def qualified_runtime_compiler(monkeypatch, request):
    if request.param == "current-v3-047":
        assert compiler.MOBKIT_VERSION == generator.MOBKIT_VERSION == "0.8.47"
        yield
        return
    namespace = "_forge_historical_runtime_v1"
    path = Path(__file__).parent / "fixtures/runtime_history_v1/launchkit"
    spec = importlib.util.spec_from_file_location(namespace, path / "__init__.py",
        submodule_search_locations=[str(path)])
    package = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, namespace, package)
    spec.loader.exec_module(package)
    historical = __import__(namespace + ".compiler", fromlist=["compiler"])
    monkeypatch.setattr(compiler, "MOBKIT_VERSION", "0.8.46")
    monkeypatch.setattr(generator, "MOBKIT_VERSION", "0.8.46")

    def export(value, **kwargs):
        # Existing fixtures also emit their .42 predecessor. Use that exact
        # selected version with the original compiler and original templates.
        historical.MOBKIT_VERSION = compiler.MOBKIT_VERSION
        return [generator.PackageFile(item.path, item.content, item.mode)
                for item in historical.generate_package(value, **kwargs)]

    monkeypatch.setattr(compiler, "generate_package", export)
    try:
        yield
    finally:
        for name in list(sys.modules):
            if name.startswith(namespace + "."):
                sys.modules.pop(name, None)


@pytest.fixture
def retained_runtime_v2(tmp_path, monkeypatch):
    """Exact public original v2 sources for synthetic closed-history controls."""
    fixture = Path(__file__).parent / "fixtures/runtime_history_v2"
    metadata = json.loads((fixture / "SOURCE.json").read_text())
    payload = (fixture / metadata["archive"]["path"]).read_bytes()
    assert len(payload) == metadata["archive"]["bytes"]
    assert hashlib.sha256(payload).hexdigest() == metadata["archive"]["sha256"]
    source = tmp_path / "retained-v2-sources"
    with zipfile.ZipFile(fixture / metadata["archive"]["path"]) as archive:
        assert len(archive.infolist()) == len(metadata["files"]) == 105
        assert set(archive.namelist()) == set(metadata["files"])
        for name, expected in metadata["files"].items():
            data = archive.read(name)
            assert len(data) == expected["bytes"]
            assert hashlib.sha256(data).hexdigest() == expected["sha256"]
            assert archive.getinfo(name).external_attr >> 16 & 0o777 == 0o644
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    namespace = "_forge_retained_runtime_v2"
    package = source / "launchkit"
    spec = importlib.util.spec_from_file_location(namespace, package / "__init__.py",
        submodule_search_locations=[str(package)])
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, namespace, module)
    spec.loader.exec_module(module)
    try:
        yield SimpleNamespace(root=source, metadata=metadata,
            compiler=importlib.import_module(namespace + ".compiler"),
            generator=importlib.import_module(namespace + ".generator"),
            updates=importlib.import_module(namespace + ".deployment_updates"))
    finally:
        for name in list(sys.modules):
            if name.startswith(namespace + "."):
                sys.modules.pop(name, None)


@pytest.fixture
def retained_candidate_v1(tmp_path, monkeypatch):
    """Original public candidate-v1 sources, never Reading runtime authority."""
    fixture = Path(__file__).parent / "fixtures/runtime_history_v1_candidate"
    metadata = json.loads((fixture / "SOURCE.json").read_text())
    payload = (fixture / metadata["archive"]["path"]).read_bytes()
    assert metadata["archive"]["sha256"] == "f0b48f54883f80d7a52e338318bce984b3b4b9fe16c7a1ea1b7faf53bb352829"
    assert len(payload) == metadata["archive"]["bytes"]
    assert hashlib.sha256(payload).hexdigest() == metadata["archive"]["sha256"]
    original = set(metadata["original_supervisor"]["source_paths"])
    scripts = set(metadata["additional_raw_scripts"]["paths"])
    assert len(original) == 110 and len(scripts) == 4 and not original & scripts
    assert set(metadata["files"]) == original | scripts
    source = tmp_path / "retained-candidate-v1-sources"
    with zipfile.ZipFile(fixture / metadata["archive"]["path"]) as archive:
        assert len(archive.infolist()) == len(metadata["files"]) == 114
        assert set(archive.namelist()) == set(metadata["files"])
        for name, expected in metadata["files"].items():
            data = archive.read(name)
            assert len(data) == expected["bytes"]
            assert hashlib.sha256(data).hexdigest() == expected["sha256"]
            assert archive.getinfo(name).external_attr >> 16 & 0o777 == 0o644
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    namespace = "_forge_retained_candidate_v1"
    package = source / "launchkit"
    spec = importlib.util.spec_from_file_location(namespace, package / "__init__.py",
        submodule_search_locations=[str(package)])
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, namespace, module)
    spec.loader.exec_module(module)
    try:
        yield SimpleNamespace(root=source, metadata=metadata,
            source_files={name: (source / name).read_bytes() for name in sorted(original)},
            compiler=importlib.import_module(namespace + ".compiler"),
            generator=importlib.import_module(namespace + ".generator"),
            blueprint=importlib.import_module(namespace + ".blueprint"),
            updates=importlib.import_module(namespace + ".deployment_updates"),
            services=importlib.import_module(namespace + ".deployment_service"),
            selection=importlib.import_module(namespace + ".runtime_selection"),
            candidates=importlib.import_module(namespace + ".runtime_candidates"))
    finally:
        for name in list(sys.modules):
            if name.startswith(namespace + "."):
                sys.modules.pop(name, None)
