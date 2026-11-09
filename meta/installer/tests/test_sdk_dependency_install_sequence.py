"""Selected SDK and dependency action order with inert pip and wheel bytes."""
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
from types import SimpleNamespace
from urllib.parse import unquote, urlsplit

import pytest

from launchkit import sdk_wheel_integrity as integrity
from launchkit.templates import install_candidate as installed
from test_sdk_wheel_pair_install import pair_files, install as install_pair, assert_no_success_receipt_or_private_copy
from test_selected_dependency_install import inert_wheel, put

from sdk_source_import_guard import no_selected_sdk_import


@pytest.fixture
def sequence_files(pair_files, monkeypatch):
    files = pair_files
    files.inputs = files.meta_path.parent
    files.dependencies = (inert_wheel("fixture-sequence", "1.0.0"),)
    files.dependency_wheels = tuple(put(files.inputs / item.filename, item.content) for item in files.dependencies)
    lines = [f"{item.name}=={item.version} --hash=sha256:{ref.sha256}"
             for item, ref in zip(files.dependencies, files.dependency_wheels)]
    files.dependency_requirements = put(files.inputs / "dependency.hashes.txt", ("\n".join(lines) + "\n").encode())
    files.events, files.dependency_calls = [], []
    sdk_pip = installed.subprocess.run
    actual_compare = installed.compare_installed_meta_toolkit_wheels

    def readback(*args, **kwargs):
        files.events.append("sdk-readback")
        return actual_compare(*args, **kwargs)

    def sequence_pip(argv, **kwargs):
        selected = Path(argv[-1])
        if selected.parent.name.startswith(".forge-selected-sdk-pair-"):
            files.events.append("sdk-pip")
            return sdk_pip(argv, **kwargs)
        files.events.append("dependency-pip")
        files.dependency_calls.append((argv, kwargs))
        assert argv == [str(files.python), "-I", "-m", "pip", "--isolated", "install",
            "--disable-pip-version-check", "--no-index", "--no-deps", "--no-compile",
            "--force-reinstall", "--require-hashes", "--only-binary=:all:", "-r", str(selected)]
        assert set(kwargs) == {"check", "env"} and kwargs["check"] is True
        assert kwargs["env"]["PIP_CONFIG_FILE"] == os.devnull
        assert selected.parent.parent == files.root / ".venv"
        assert selected.parent.name.startswith(".forge-selected-dependencies-")
        assert stat.S_IMODE(selected.stat().st_mode) == 0o600
        lines = selected.read_text().splitlines()
        assert len(lines) == len(files.dependencies)
        for line, item, ref in zip(lines, files.dependencies, files.dependency_wheels):
            match = re.fullmatch(r"([a-z0-9-]+) @ (file://\S+) --hash=sha256:([a-f0-9]{64})", line)
            assert match and match[1] == item.name and match[3] == ref.sha256
            url = urlsplit(match[2])
            assert not url.netloc and not url.query and not url.fragment
            path = Path(unquote(url.path))
            assert path.parent == selected.parent and path.name == item.filename
            assert path.read_bytes() == item.content
            assert str(files.inputs) not in line
            for name, content in item.values.items():
                installed_path = files.site / name
                installed_path.parent.mkdir(parents=True, exist_ok=True)
                installed_path.write_bytes(content)
                installed_path.chmod(0o644)
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(installed, "compare_installed_meta_toolkit_wheels", readback)
    monkeypatch.setattr(installed.subprocess, "run", sequence_pip)
    return files


def install(files, **options):
    arguments = dict(dependency_requirements=files.dependency_requirements,
        dependency_wheels=files.dependency_wheels)
    arguments.update(options)
    return install_pair(files, **arguments)


def assert_retained(files):
    assert_no_success_receipt_or_private_copy(files)
    assert not list((files.root / ".venv").glob(".forge-selected-dependencies-*"))


def test_selected_batch_sdk_then_dependencies_then_final_sdk_readback(sequence_files, monkeypatch):
    files = sequence_files
    original_install = installed._install_selected_dependency_bytes
    invocations = []
    def dependency_action(pending, **kwargs):
        before = len(files.checks)
        kwargs["current"]()
        assert len(files.checks) == before + 1, "The shared action fence invokes the original owner callback."
        invocations.append((pending, kwargs, before))
        result = original_install(pending, **kwargs)
        assert len(files.checks) > before + 1
        return result
    monkeypatch.setattr(installed, "_install_selected_dependency_bytes", dependency_action)
    result = install(files)
    assert result is files.verified
    assert files.events == ["sdk-pip", "sdk-readback", "dependency-pip", "sdk-readback"]
    assert len(files.calls) == len(files.dependency_calls) == len(invocations) == 1
    pending, kwargs, before = invocations[0]
    assert pending == tuple((item.name, item.filename, item.content, ref.sha256)
                            for item, ref in zip(files.dependencies, files.dependency_wheels))
    assert kwargs["root"] == files.root and kwargs["python"] == files.python
    assert set(kwargs) == {"root", "python", "current"} and callable(kwargs["current"])
    assert before > 0 and all(files.checks), "Both SDK and dependency phases consult the same original owner."
    assert (files.site / "fixture_sequence/__init__.py").read_bytes() == files.dependencies[0].values["fixture_sequence/__init__.py"]
    assert "fixture_sequence" not in sys.modules
    assert_retained(files)


def test_pair_only_default_keeps_one_sdk_action_and_one_readback(sequence_files):
    files = sequence_files
    assert install_pair(files) is files.verified
    assert files.events == ["sdk-pip", "sdk-readback"]
    assert files.dependency_calls == []
    assert not (files.site / "fixture_sequence").exists()
    assert_retained(files)


@pytest.mark.parametrize("missing", ["requirements", "wheels"])
def test_one_sided_dependency_arguments_refuse_before_sdk_mutation(sequence_files, monkeypatch, missing):
    files = sequence_files
    def forbidden(*args, **kwargs):
        raise AssertionError("Incomplete dependency selection must not stage SDK bytes.")
    monkeypatch.setattr(installed.tempfile, "TemporaryDirectory", forbidden)
    options = {"dependency_requirements" if missing == "requirements" else "dependency_wheels": None}
    with pytest.raises(installed.CandidateArtifactError):
        install(files, **options)
    assert files.events == [] and files.calls == files.dependency_calls == []
    assert list(files.site.iterdir()) == []
    assert_retained(files)


@pytest.mark.parametrize("defect", ["malformed", "missing-wheel", "wrong-hash", "sdk-pin", "metadata-name"])
def test_bad_dependency_preflight_precedes_all_sdk_private_staging(sequence_files, monkeypatch, defect):
    files = sequence_files
    if defect == "missing-wheel":
        files.dependency_wheels = ()
    elif defect == "metadata-name":
        item = inert_wheel("fixture-sequence", "1.0.0", defect="metadata-name")
        files.dependencies = (item,)
        files.dependency_wheels = (put(files.inputs / item.filename, item.content),)
        files.dependency_requirements = put(files.dependency_requirements.path,
            f"fixture-sequence==1.0.0 --hash=sha256:{files.dependency_wheels[0].sha256}\n".encode())
    else:
        text = files.dependency_requirements.path.read_text()
        if defect == "malformed":
            text = "--index-url https://unselected.invalid\n"
        elif defect == "wrong-hash":
            text = text.replace(files.dependency_wheels[0].sha256, "0" * 64)
        else:
            text = text.replace("fixture-sequence", "meerkat-meta")
        files.dependency_requirements = put(files.dependency_requirements.path, text.encode())
    def forbidden(*args, **kwargs):
        raise AssertionError("Invalid dependency preflight must not stage SDK bytes.")
    monkeypatch.setattr(installed.tempfile, "TemporaryDirectory", forbidden)
    with pytest.raises((installed.CandidateArtifactError, integrity.WheelIntegrityError)):
        install(files)
    assert files.events == [] and files.calls == files.dependency_calls == []
    assert list(files.site.iterdir()) == []
    assert_retained(files)


@pytest.mark.parametrize("target", ["source", "metadata"])
def test_dependency_action_overwriting_sdk_bytes_is_caught_by_final_readback(sequence_files, monkeypatch, target):
    files = sequence_files
    original = installed.subprocess.run
    path = files.site / ("meerkat_meta/runtime.py" if target == "source" else "meerkat_meta-0.9.0.dist-info/METADATA")
    def changed(argv, **kwargs):
        result = original(argv, **kwargs)
        if "--only-binary=:all:" in argv:
            path.write_bytes(b"dependency overwrote selected SDK bytes\n")
        return result
    monkeypatch.setattr(installed.subprocess, "run", changed)
    with pytest.raises(installed.CandidateArtifactError, match="differs from the exact|differs from its selected"):
        install(files)
    assert files.events == ["sdk-pip", "sdk-readback", "dependency-pip", "sdk-readback"]
    assert path.read_bytes() == b"dependency overwrote selected SDK bytes\n"
    assert (files.site / "fixture_sequence/__init__.py").exists()
    assert_retained(files)


def test_dependency_pip_failure_keeps_sdk_and_partial_dependency_state_without_success(sequence_files, monkeypatch):
    files = sequence_files
    original = installed.subprocess.run
    def failed(argv, **kwargs):
        result = original(argv, **kwargs)
        if "--only-binary=:all:" in argv:
            (files.site / "partial.txt").write_bytes(b"partial dependency install\n")
            raise subprocess.CalledProcessError(1, argv)
        return result
    monkeypatch.setattr(installed.subprocess, "run", failed)
    with pytest.raises(subprocess.CalledProcessError):
        install(files)
    assert files.events == ["sdk-pip", "sdk-readback", "dependency-pip"]
    assert (files.site / "meerkat_meta/runtime.py").read_bytes() == files.meta[2]["meerkat_meta/runtime.py"]
    assert (files.site / "partial.txt").read_bytes() == b"partial dependency install\n"
    assert len(files.dependency_calls) == 1
    assert_retained(files)


def test_dependency_sources_replaced_during_sdk_pip_use_original_preflighted_bytes(sequence_files, monkeypatch):
    files = sequence_files
    original = installed.subprocess.run
    def replaced(argv, **kwargs):
        if "--only-binary=:all:" not in argv:
            for ref in (files.dependency_requirements, *files.dependency_wheels):
                ref.path.write_bytes(b"unselected mutable source replacement\n")
        return original(argv, **kwargs)
    monkeypatch.setattr(installed.subprocess, "run", replaced)
    assert install(files) is files.verified
    assert files.events == ["sdk-pip", "sdk-readback", "dependency-pip", "sdk-readback"]
    assert (files.site / "fixture_sequence/__init__.py").read_bytes() == files.dependencies[0].values["fixture_sequence/__init__.py"]
    assert all(ref.path.read_bytes() == b"unselected mutable source replacement\n"
               for ref in (files.dependency_requirements, *files.dependency_wheels))
    assert_retained(files)


@pytest.mark.parametrize("phase", ["before", "after"])
def test_stale_original_owner_at_dependency_boundary_refuses_final_success(sequence_files, monkeypatch, phase):
    files = sequence_files
    if phase == "before":
        original = installed._install_selected_dependency_bytes
        def changed(*args, **kwargs):
            files.current = False
            return original(*args, **kwargs)
        monkeypatch.setattr(installed, "_install_selected_dependency_bytes", changed)
    else:
        original = installed.subprocess.run
        def changed(argv, **kwargs):
            result = original(argv, **kwargs)
            if "--only-binary=:all:" in argv:
                files.current = False
            return result
        monkeypatch.setattr(installed.subprocess, "run", changed)
    with pytest.raises(installed.CandidateArtifactError, match="owner changed"):
        install(files)
    expected = ["sdk-pip", "sdk-readback"] + (["dependency-pip"] if phase == "after" else [])
    assert files.events == expected
    assert (files.site / "meerkat_meta/runtime.py").exists()
    assert_retained(files)
