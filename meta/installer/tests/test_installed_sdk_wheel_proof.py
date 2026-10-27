"""Installed inert SDK readback counterexamples; no SDK or process executes."""
import base64
import csv
from dataclasses import asdict, replace
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import stat
import sys
from types import SimpleNamespace
import zipfile

import pytest

from launchkit import runtime_candidates
from launchkit import sdk_wheel_integrity as integrity
from launchkit.templates import install_candidate as installed


def digest(value):
    return hashlib.sha256(value).hexdigest()


def inert_wheel(distribution, *, top_level=True, version="0.9.0", meta_version="0.9.0"):
    package = distribution.replace("-", "_")
    prefix = f"{package}-{version}.dist-info/"
    modules = ("builder.py", "runtime.py", "_transport.py") if distribution == "meerkat-meta" else (
        "application_runtime.py", "host_control.py")
    payload = {f"{package}/{name}": b"raise RuntimeError('inert SDK must never execute')\n"
               for name in ("__init__.py", *modules)}
    payload[f"{package}/py.typed"] = b""
    payload[f"{package}/data/config.json"] = b'{"inert":true}\n'
    metadata = (f"Metadata-Version: 2.4\nName: {distribution}\nVersion: {version}\n"
                "License-File: LICENSE-MIT\nLicense-File: vendor/NOTICE\n")
    if distribution == "meerkat-toolkit":
        metadata += f'Provides-Extra: native-host\nRequires-Dist: meerkat-meta=={meta_version}; extra == "native-host"\n'
    declared = {"METADATA": (metadata + "\n").encode(),
                "WHEEL": b"Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n\n",
                "licenses/LICENSE-MIT": b"Inert original license.\n",
                "licenses/vendor/NOTICE": b"Inert nested notice.\n"}
    if top_level:
        declared["top_level.txt"] = (package + "\n").encode()
    values = {**payload, **{prefix + name: value for name, value in declared.items()}}
    rows = [[name, "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(value).digest()).decode().rstrip("="),
             str(len(value))] for name, value in values.items()]
    rows.append([prefix + "RECORD", "", ""])
    record = io.StringIO()
    csv.writer(record, lineterminator="\n").writerows(rows)
    values[prefix + "RECORD"] = record.getvalue().encode()
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        for name, value in values.items():
            entry = zipfile.ZipInfo(name)
            entry.external_attr = (stat.S_IFREG | 0o644) << 16
            archive.writestr(entry, value)
    content = output.getvalue()
    expectation = integrity.WheelByteExpectation(distribution, version, digest(content), len(content),
        tuple((name, digest(value), len(value)) for name, value in payload.items()),
        tuple((name, digest(value), len(value)) for name, value in declared.items()))
    return content, expectation, values


@pytest.fixture
def sdk_files(tmp_path, monkeypatch, request):
    root = tmp_path / "deployment"
    root.mkdir(mode=0o700)
    site = root / ".venv/lib/python-fixture/site-packages"
    site.mkdir(parents=True)
    python = root / ".venv/bin/python"
    python.parent.mkdir()
    versions = getattr(request, "param", "0.9.0")
    meta_version, toolkit_version = versions if isinstance(versions, tuple) else (versions, "0.9.0")
    meta = inert_wheel("meerkat-meta", version=meta_version)
    toolkit = inert_wheel("meerkat-toolkit", top_level=False, version=toolkit_version, meta_version=meta_version)
    facts = integrity.check_meta_toolkit_wheels(meta[0], toolkit[0], meta=meta[1], toolkit=toolkit[1])
    for _, _, values in (meta, toolkit):
        for name, value in values.items():
            path = site / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(value)
        prefix = next(name.split("/")[0] for name in values if ".dist-info/" in name)
        (site / prefix / "INSTALLER").write_bytes(b"inert-pip\n")
    calls = []

    def no_process(*args, **kwargs):
        calls.append((args, kwargs))
        raise AssertionError("An inert installed readback must not invoke a process.")

    monkeypatch.setattr(installed.subprocess, "run", no_process)
    return SimpleNamespace(root=root, site=site, python=python, facts=facts,
                           wheel_values=(meta[2], toolkit[2]), calls=calls)


def compare(files, *, facts=None, **options):
    return installed.compare_installed_meta_toolkit_wheels(*(files.facts if facts is None else facts),
        deployment_root=options.get("deployment_root", files.root),
        site_packages=options.get("site_packages", files.site),
        sdk_python=options.get("sdk_python", files.python))


@pytest.mark.parametrize("sdk_files", ["0.9.0", "0.9.1", "0.9.4", ("0.9.4", "0.9.4")], indirect=True)
def test_exact_installed_pair_returns_original_facts_and_no_receipt_or_process(sdk_files, monkeypatch):
    original = {path: path.read_bytes() for path in sdk_files.site.rglob("*") if path.is_file()}
    import builtins
    modules = {name: module for name, module in sys.modules.items()
               if name.split(".")[0] in {"meerkat_meta", "meerkat_toolkit"}}
    original_import = builtins.__import__
    def no_sdk_import(name, *args, **kwargs):
        if name.split(".")[0] in {"meerkat_meta", "meerkat_toolkit"}:
            pytest.fail("Installed byte readback must not import an SDK.")
        return original_import(name, *args, **kwargs)
    monkeypatch.setattr(builtins, "__import__", no_sdk_import)
    result = compare(sdk_files)
    assert result[0] is sdk_files.facts[0] and result[1] is sdk_files.facts[1]
    assert {path: path.read_bytes() for path in sdk_files.site.rglob("*") if path.is_file()} == original
    assert not (sdk_files.root / ".venv/forge-sdk.receipt.json").exists()
    assert {name: module for name, module in sys.modules.items()
            if name.split(".")[0] in {"meerkat_meta", "meerkat_toolkit"}} == modules
    assert sdk_files.calls == []


@pytest.mark.parametrize("sdk_files", [("0.9.4", "0.9.4")], indirect=True)
@pytest.mark.parametrize("defect", ["selected-version", "installed-metadata", "installed-version", "payload"])
def test_toolkit_094_readback_refuses_wrong_selected_or_installed_bytes(sdk_files, defect):
    facts = sdk_files.facts
    directory = sdk_files.site / "meerkat_toolkit-0.9.4.dist-info"
    if defect == "selected-version":
        facts = (facts[0], replace(facts[1], version="0.9.0"))
    elif defect == "installed-metadata":
        path = directory / "METADATA"
        path.write_bytes(path.read_bytes().replace(b"Version: 0.9.4", b"Version: 0.9.0"))
    elif defect == "installed-version":
        directory.rename(sdk_files.site / "meerkat_toolkit-0.9.0.dist-info")
    else:
        (sdk_files.site / "meerkat_toolkit/host_control.py").write_bytes(b"Changed source.\n")
    with pytest.raises(runtime_candidates.CandidateArtifactError):
        compare(sdk_files, facts=facts)
    assert sdk_files.calls == []


@pytest.mark.parametrize("sdk_files", ["0.9.1"], indirect=True)
@pytest.mark.parametrize("defect", ["selected-version", "installed-metadata", "installed-version"])
def test_meta_091_readback_refuses_wrong_selected_or_installed_identity(sdk_files, defect):
    facts = sdk_files.facts
    directory = sdk_files.site / "meerkat_meta-0.9.1.dist-info"
    if defect == "selected-version":
        facts = (replace(facts[0], version="0.9.2"), facts[1])
    elif defect == "installed-metadata":
        path = directory / "METADATA"
        path.write_bytes(path.read_bytes().replace(b"Version: 0.9.1", b"Version: 0.9.0"))
    else:
        directory.rename(sdk_files.site / "meerkat_meta-0.9.0.dist-info")
    with pytest.raises(runtime_candidates.CandidateArtifactError):
        compare(sdk_files, facts=facts)
    assert sdk_files.calls == []


@pytest.mark.parametrize("defect", ["license-corrupt", "license-missing", "license-extra",
    "metadata-corrupt", "metadata-extra", "metadata-empty-directory", "unexpected-top-level",
    "payload-corrupt", "payload-extra", "payload-empty-directory", "duplicate-dist", "unversioned-dist", "other-version",
    "payload-link", "payload-hardlink", "payload-writable", "license-link", "license-hardlink",
    "license-writable", "metadata-directory-writable"])
def test_actual_installed_corruption_and_custody_are_refused(sdk_files, defect):
    site = sdk_files.site
    package = site / "meerkat_toolkit"
    metadata = site / f"meerkat_toolkit-{sdk_files.facts[1].version}.dist-info"
    source = package / "host_control.py"
    license_path = metadata / "licenses/vendor/NOTICE"
    if defect == "license-corrupt":
        license_path.write_bytes(b"Changed nested notice.\n")
    elif defect == "license-missing":
        license_path.unlink()
    elif defect == "license-extra":
        (metadata / "licenses/vendor/extra").write_bytes(b"Not selected.\n")
    elif defect == "metadata-corrupt":
        (metadata / "METADATA").write_bytes(b"Changed metadata.\n")
    elif defect == "metadata-extra":
        (metadata / "startup.pth").write_bytes(b"Not selected.\n")
    elif defect == "metadata-empty-directory":
        (metadata / "unexpected").mkdir()
    elif defect == "unexpected-top-level":
        (metadata / "top_level.txt").write_bytes(b"meerkat_toolkit\n")
    elif defect == "payload-corrupt":
        source.write_bytes(b"Changed source.\n")
    elif defect == "payload-extra":
        (package / "extra.py").write_bytes(b"Not selected.\n")
    elif defect == "payload-empty-directory":
        (package / "unexpected").mkdir()
    elif defect == "duplicate-dist":
        (site / f"MeErKaT.ToOlKiT-{sdk_files.facts[1].version}.egg-info").mkdir()
    elif defect == "unversioned-dist":
        (site / "MeErKaT.ToOlKiT.egg-info").mkdir()
    elif defect == "other-version":
        (site / "meerkat_toolkit-0.0.1.dist-info").mkdir()
    elif defect.endswith("-link") or defect.endswith("-hardlink"):
        path = license_path if defect.startswith("license") else source
        retained = sdk_files.root / "outside-original"
        path.rename(retained)
        if defect.endswith("-hardlink"):
            path.hardlink_to(retained)
        else:
            path.symlink_to(retained)
    elif defect == "metadata-directory-writable":
        (metadata / "licenses").chmod(0o777)
    else:
        (license_path if defect.startswith("license") else source).chmod(0o666)
    with pytest.raises((runtime_candidates.CandidateArtifactError, FileNotFoundError)):
        compare(sdk_files)
    assert sdk_files.calls == []


@pytest.mark.parametrize("defect", ["distribution", "version", "wheel-hash", "wheel-size", "fact-type",
    "payload-absolute", "payload-traversal", "payload-duplicate", "payload-casefold", "payload-startup",
    "payload-digest", "payload-size", "payload-missing-module", "metadata-traversal", "metadata-startup",
    "metadata-duplicate", "metadata-size", "metadata-incomplete"])
def test_malformed_held_facts_refuse_before_any_installed_read(sdk_files, monkeypatch, defect):
    fact = sdk_files.facts[1]
    if defect in {"distribution", "version", "wheel-hash", "wheel-size"}:
        field, value = {"distribution": ("distribution", "meerkat-mobkit"), "version": ("version", "0.8.48"),
                        "wheel-hash": ("sha256", "not-a-digest"), "wheel-size": ("size", True)}[defect]
        fact = replace(fact, **{field: value})
    elif defect == "fact-type":
        fact = SimpleNamespace(**vars(fact))
    else:
        field = "source_payload" if defect.startswith("payload") else "metadata"
        records = list(getattr(fact, field))
        name, hashed, size = records[0]
        if defect.endswith("absolute"):
            records[0] = ("/outside/host_control.py", hashed, size)
        elif defect.endswith("traversal"):
            records[0] = ("meerkat_toolkit/../outside" if field == "source_payload" else "licenses/../../outside", hashed, size)
        elif defect.endswith("duplicate"):
            records.append(records[0])
        elif defect.endswith("casefold"):
            parent, basename = name.rsplit("/", 1)
            records.append((parent + "/" + basename.upper(), hashed, size))
        elif defect.endswith("startup"):
            records.append(("meerkat_toolkit/startup.pth" if field == "source_payload" else "startup.pth", hashed, size))
        elif defect.endswith("digest"):
            records[0] = (name, "not-a-digest", size)
        elif defect.endswith("size"):
            records[0] = (name, hashed, True)
        elif defect.endswith("missing-module"):
            records = [row for row in records if row[0] != "meerkat_toolkit/host_control.py"]
        elif defect.endswith("incomplete"):
            records = [row for row in records if row[0] != "METADATA"]
        fact = replace(fact, **{field: tuple(records)})

    def no_read(*args, **kwargs):
        raise AssertionError("Malformed held facts must refuse before any installed file read.")

    monkeypatch.setattr(installed, "read_owned", no_read)
    with pytest.raises(runtime_candidates.CandidateArtifactError):
        compare(sdk_files, facts=(sdk_files.facts[0], fact))
    assert sdk_files.calls == []


@pytest.mark.parametrize("defect", ["missing-site", "missing-python", "outside-site", "foreign-python", "linked-site"])
def test_pair_observation_keeps_the_explicit_selected_venv(sdk_files, defect):
    options = {}
    if defect == "missing-site":
        options["site_packages"] = None
    elif defect == "missing-python":
        options["sdk_python"] = None
    elif defect == "outside-site":
        outside = sdk_files.root / "outside-site"
        outside.mkdir()
        options["site_packages"] = outside
    elif defect == "foreign-python":
        options["sdk_python"] = Path(sys.executable)
    else:
        linked = sdk_files.root / ".venv/linked-site"
        linked.symlink_to(sdk_files.site, target_is_directory=True)
        options["site_packages"] = linked
    with pytest.raises(runtime_candidates.CandidateArtifactError):
        compare(sdk_files, **options)
    assert sdk_files.calls == []


@pytest.mark.parametrize("kind", ["unchanged", "corrupt", "link", "hardlink", "writable", "foreign-source"])
def test_caches_retain_explicit_interpreter_check_without_loading_sdk(sdk_files, monkeypatch, kind):
    source = sdk_files.site / "meerkat_toolkit/host_control.py"
    cache = Path(importlib.util.cache_from_source(str(source)))
    cache.parent.mkdir()
    cache.write_bytes(b"inert-selected-interpreter-cache")
    if kind == "link":
        cache.rename(sdk_files.root / "outside-cache")
        cache.symlink_to(sdk_files.root / "outside-cache")
    elif kind == "hardlink":
        cache.rename(sdk_files.root / "outside-cache")
        cache.hardlink_to(sdk_files.root / "outside-cache")
    elif kind == "writable":
        cache.chmod(0o666)
    elif kind == "foreign-source":
        cache.rename(cache.parent / cache.name.replace("host_control", "not_selected"))
    calls = []

    def selected_interpreter(argv, **kwargs):
        calls.append((argv, kwargs))
        assert argv[:3] == [str(sdk_files.python), "-I", "-S"]
        assert argv[3] == "-c" and kwargs["check"] and kwargs["capture_output"]
        request = json.loads(kwargs["input"])
        assert request["filename"] == str(source)
        assert base64.b64decode(request["source"]) == source.read_bytes()
        assert base64.b64decode(request["cache"]) == cache.read_bytes()
        return SimpleNamespace(stdout=json.dumps({"valid": kind != "corrupt"}))

    monkeypatch.setattr(installed.subprocess, "run", selected_interpreter)
    if kind == "unchanged":
        assert compare(sdk_files) == sdk_files.facts
        assert len(calls) == 1
    else:
        with pytest.raises(runtime_candidates.CandidateArtifactError):
            compare(sdk_files)
        assert len(calls) == (1 if kind == "corrupt" else 0)


@pytest.mark.parametrize("bound", ["count", "bytes"])
def test_pair_cache_cost_is_bounded_before_interpreter_checks(sdk_files, monkeypatch, bound):
    cache_directory = sdk_files.site / "meerkat_toolkit/__pycache__"
    cache_directory.mkdir()
    if bound == "count":
        monkeypatch.setattr(integrity, "MAX_WHEEL_FILES", 32)
        for index in range(33):
            (cache_directory / f"host_control.inert-{index}.pyc").write_bytes(b"inert")
    else:
        monkeypatch.setattr(integrity, "MAX_WHEEL_TOTAL", 4096)
        (cache_directory / "host_control.inert.pyc").write_bytes(b"x" * 4097)
    with pytest.raises(runtime_candidates.CandidateArtifactError, match="bytecode exceeds"):
        compare(sdk_files)
    assert sdk_files.calls == []


def test_pair_unexpected_file_refuses_before_further_traversal(sdk_files, monkeypatch):
    package = sdk_files.site / "meerkat_toolkit"
    unexpected = package / "unexpected.py"
    unexpected.write_bytes(b"inert")
    original = Path.rglob

    def bounded(path, pattern):
        if path == package:
            yield unexpected
            raise AssertionError("Unexpected files must refuse before further traversal.")
        yield from original(path, pattern)

    monkeypatch.setattr(Path, "rglob", bounded)
    with pytest.raises(runtime_candidates.CandidateArtifactError, match="inventory differs"):
        compare(sdk_files)
    assert sdk_files.calls == []


def test_fact_size_mismatch_is_checked_even_with_an_unchanged_digest(sdk_files):
    fact = sdk_files.facts[1]
    for field in ("source_payload", "metadata"):
        records = list(getattr(fact, field))
        name, hashed, size = records[0]
        records[0] = name, hashed, size + 1
        with pytest.raises(runtime_candidates.CandidateArtifactError):
            compare(sdk_files, facts=(sdk_files.facts[0], replace(fact, **{field: tuple(records)})))
    assert sdk_files.calls == []


def test_legacy_proof_keeps_exact_output_without_importing_the_new_helper(tmp_path, monkeypatch):
    root = tmp_path / "historical"
    root.mkdir(mode=0o700)
    source = Path(installed.__file__).read_bytes()
    (root / "install_candidate.py").write_bytes(source)
    # Historical readers can be self-contained. The legacy installer must not
    # acquire a new eager import merely because pair observation exists.
    reader = SimpleNamespace(**{name: getattr(runtime_candidates, name) for name in (
        "ArtifactFile", "CandidateArtifactError", "CandidateBinding", "read_candidate_artifacts", "MAX_ARCHIVE_BYTES",
        "MAX_BINARY_BYTES", "MAX_TAR_OVERHEAD", "MAX_METADATA_BYTES", "MAX_WHEEL_BYTES")})
    monkeypatch.setitem(sys.modules, "runtime_candidates", reader)
    monkeypatch.setitem(sys.modules, "sdk_wheel_integrity", None)
    spec = importlib.util.spec_from_file_location("historical_installed_sdk_proof", root / "install_candidate.py")
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, spec.name, module)
    spec.loader.exec_module(module)
    site = root / ".venv/lib/python-fixture/site-packages"
    package = site / "meerkat_mobkit"
    package.mkdir(parents=True)
    payload = {"meerkat_mobkit/__init__.py": b"raise RuntimeError('legacy SDK stays inert')\n",
               "meerkat_mobkit/py.typed": b""}
    prefix = "meerkat_mobkit-0.8.48.dist-info"
    metadata = {"METADATA": b"Metadata-Version: 2.1\nName: meerkat-mobkit\nVersion: 0.8.48\n\n",
                "WHEEL": b"Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n\n",
                "top_level.txt": b"meerkat_mobkit\n"}
    wheel = io.BytesIO()
    with zipfile.ZipFile(wheel, "w") as archive:
        for name, value in {**payload, **{prefix + "/" + name: value for name, value in metadata.items()}}.items():
            path = site / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(value)
            archive.writestr(name, value)
    (site / prefix / "REQUESTED").write_bytes(b"")
    data = wheel.getvalue()
    wheel_path = root / "legacy.whl"
    wheel_path.write_bytes(data)
    binding = runtime_candidates.CandidateBinding(repository="lukacf/meerkat-mobkit", source_sha="a" * 40,
        native_version="0.8.48", core_version="0.8.51", platform="aarch64-apple-darwin",
        native_run_id=23, native_run_attempt=1, tuple_sha256="b" * 64,
        native_manifest_sha256="c" * 64, sdk_wheel_sha256=digest(data))
    context = SimpleNamespace(root=root, accepted=binding, artifacts=SimpleNamespace(sdk=SimpleNamespace(
        payload=tuple((name, digest(value)) for name, value in payload.items()),
        wheel=SimpleNamespace(path=wheel_path, size=len(data), sha256=digest(data)))))

    def no_process(*args, **kwargs):
        raise AssertionError("Legacy installed source readback must not execute a process.")

    monkeypatch.setattr(module.subprocess, "run", no_process)
    assert module.sdk_proof(context, site, root / ".venv/bin/python") == {
        "format": "forge-installed-candidate-sdk-v1", "source": "local-build-from-exact-frozen-source",
        "signed": False, "published": False, "version": "0.8.48", "site_packages": str(site),
        "payload_sha256": {name: digest(value) for name, value in payload.items()},
        "metadata_sha256": {name: digest(value) for name, value in metadata.items()},
        "candidate_binding": asdict(binding), "candidate_tuple_sha256": binding.tuple_sha256,
        "sdk_wheel_sha256": binding.sdk_wheel_sha256, "source_sha": binding.source_sha,
        "native_run_id": binding.native_run_id, "native_run_attempt": binding.native_run_attempt,
    }
    assert sys.modules["sdk_wheel_integrity"] is None
