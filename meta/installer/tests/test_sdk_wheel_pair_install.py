"""Exact pair installation controls with inert pip and SDK source bytes only."""
from dataclasses import replace
import hashlib
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
from test_installed_sdk_wheel_proof import inert_wheel

from sdk_source_import_guard import no_selected_sdk_import


def hashed(value):
    return hashlib.sha256(value).hexdigest()


def put_installed(files, *, include_toolkit=True):
    """Simulate copying archive members; never execute pip or SDK source."""
    wheels = (files.meta, files.toolkit) if include_toolkit else (files.meta,)
    for _, _, values in wheels:
        for name, content in values.items():
            path = files.site / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            path.chmod(0o644)
        prefix = next(name.split("/")[0] for name in values if ".dist-info/" in name)
        (files.site / prefix / "INSTALLER").write_bytes(b"inert-pip\n")


@pytest.fixture
def pair_files(tmp_path, monkeypatch, request):
    root = tmp_path / "selected deployment"
    root.mkdir(mode=0o700)
    site = root / ".venv/lib/python-fixture/site-packages"
    site.mkdir(parents=True)
    python = root / ".venv/bin/python"
    python.parent.mkdir()
    python.write_bytes(b"inert interpreter, never executed\n")
    python.chmod(0o700)
    (root / "retained.txt").write_bytes(b"application state stays owned\n")
    meta_version = getattr(request, "param", "0.9.0")
    meta = inert_wheel("meerkat-meta", version=meta_version)
    toolkit = inert_wheel("meerkat-toolkit", top_level=False, meta_version=meta_version)
    paths = tmp_path / "selected input bytes"
    paths.mkdir(mode=0o700)
    meta_path, toolkit_path = paths / "original-meta.bin", paths / "original-toolkit.bin"
    for path, content in ((meta_path, meta[0]), (toolkit_path, toolkit[0])):
        path.write_bytes(content)
        path.chmod(0o600)
    files = SimpleNamespace(root=root, site=site, python=python, meta=meta, toolkit=toolkit,
        meta_path=meta_path, toolkit_path=toolkit_path, calls=[], checks=[], current=True,
        verified=None, copied=None)

    def require_current():
        files.checks.append(files.current)
        if not files.current:
            raise installed.CandidateArtifactError("Fixture installation owner changed.")

    files.require_current = require_current
    original_check = integrity.check_meta_toolkit_wheels

    def checked(*args, **kwargs):
        files.verified = original_check(*args, **kwargs)
        return files.verified

    def inert_pip(argv, **kwargs):
        files.calls.append((argv, kwargs))
        assert argv == [str(python), "-I", "-m", "pip", "--isolated", "install",
            "--disable-pip-version-check", "--no-index", "--no-deps", "--no-compile",
            "--force-reinstall", "--require-hashes", "-r", argv[-1]]
        assert set(kwargs) == {"check", "env"} and kwargs["check"] is True
        environment = kwargs["env"]
        assert type(environment) is dict
        assert set(environment) == set(os.environ) | {"PIP_CONFIG_FILE"}
        assert environment.get("PIP_CONFIG_FILE") == os.devnull
        assert all(environment[name] == value for name, value in os.environ.items()
                   if name != "PIP_CONFIG_FILE")
        assert all(str(path) not in argument for path in (meta_path, toolkit_path) for argument in argv)
        requirements = Path(argv[-1])
        assert requirements.parent.parent == root / ".venv"
        assert requirements.parent.name.startswith(".forge-selected-sdk-pair-")
        assert stat.S_IMODE(requirements.parent.stat().st_mode) == 0o700
        assert stat.S_IMODE(requirements.stat().st_mode) == 0o600
        lines = requirements.read_text().splitlines()
        assert len(lines) == 2
        copied = []
        for line, selected in zip(lines, (meta, toolkit)):
            content, expectation, _ = selected
            match = re.fullmatch(r"(meerkat-(?:meta|toolkit)) @ (file://\S+) --hash=sha256:([a-f0-9]{64})", line)
            assert match and match[1] == expectation.distribution
            parsed = urlsplit(match[2])
            assert parsed.scheme == "file" and not parsed.netloc and not parsed.query and not parsed.fragment
            path = Path(unquote(parsed.path))
            assert path.parent == requirements.parent
            assert path.name == f"{expectation.distribution.replace('-', '_')}-{expectation.version}-py3-none-any.whl"
            assert stat.S_IMODE(path.stat().st_mode) == 0o600
            assert path.read_bytes() == content and hashed(content) == match[3] == expectation.sha256
            assert str(meta_path) not in line and str(toolkit_path) not in line
            copied.append(path)
        assert {path.name for path in requirements.parent.iterdir()} == {
            "requirements.txt", *(path.name for path in copied)}
        files.copied = tuple(copied)
        put_installed(files)
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(integrity, "check_meta_toolkit_wheels", checked)
    monkeypatch.setattr(installed.subprocess, "run", inert_pip)
    return files


def install(files, **options):
    args = dict(meta=files.meta[1], toolkit=files.toolkit[1], deployment_root=files.root,
        site_packages=files.site, sdk_python=files.python, require_current=files.require_current)
    args.update(options)
    return installed.install_meta_toolkit_wheels(files.meta_path, files.toolkit_path, **args)


def assert_no_success_receipt_or_private_copy(files):
    assert not (files.root / ".venv/forge-sdk.receipt.json").exists()
    assert not list((files.root / ".venv").glob(".forge-selected-sdk-pair-*"))
    assert (files.root / "retained.txt").read_bytes() == b"application state stays owned\n"


def forbid_private_copy(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Invalid or stale inputs must fail before any installation staging.")
    monkeypatch.setattr(installed.tempfile, "TemporaryDirectory", forbidden)


@pytest.mark.parametrize("pair_files", ["0.9.0", "0.9.1", "0.9.4"], indirect=True)
def test_exact_pair_private_copies_hashes_isolation_readback_and_original_facts(pair_files, monkeypatch):
    monkeypatch.setenv("PIP_CONFIG_FILE", "/not-selected/pip.conf")
    monkeypatch.setenv("PIP_TARGET", "/not-selected/target")
    monkeypatch.setenv("PIP_PREFIX", "/not-selected/prefix")
    monkeypatch.setenv("PIP_FIND_LINKS", "https://not-selected.invalid/wheels")
    monkeypatch.setenv("PYTHONPATH", "/not-selected/python")
    before = {path: path.read_bytes() for path in (pair_files.meta_path, pair_files.toolkit_path)}
    result = install(pair_files)
    assert result is pair_files.verified
    assert result[0].distribution == "meerkat-meta" and result[1].distribution == "meerkat-toolkit"
    assert len(pair_files.calls) == 1 and all(pair_files.checks)
    assert {path: path.read_bytes() for path in before} == before
    assert pair_files.copied and all(not path.exists() for path in pair_files.copied)
    assert_no_success_receipt_or_private_copy(pair_files)


@pytest.mark.parametrize("defect", ["meta-bytes", "toolkit-bytes", "selected-hash", "selected-type",
    "source-link", "source-hardlink", "source-writable"])
def test_source_bytes_expectations_and_custody_refuse_before_any_mutation(pair_files, monkeypatch, defect):
    options = {}
    if defect in {"meta-bytes", "toolkit-bytes"}:
        path = pair_files.meta_path if defect == "meta-bytes" else pair_files.toolkit_path
        path.write_bytes(path.read_bytes() + b"changed after selection")
    elif defect == "selected-hash":
        options["toolkit"] = replace(pair_files.toolkit[1], sha256="0" * 64)
    elif defect == "selected-type":
        options["toolkit"] = SimpleNamespace(**vars(pair_files.toolkit[1]))
    elif defect in {"source-link", "source-hardlink"}:
        retained = pair_files.toolkit_path.with_name("retained-toolkit.bin")
        pair_files.toolkit_path.rename(retained)
        if defect == "source-link":
            pair_files.toolkit_path.symlink_to(retained)
        else:
            pair_files.toolkit_path.hardlink_to(retained)
    else:
        pair_files.toolkit_path.chmod(0o666)
    forbid_private_copy(monkeypatch)
    with pytest.raises((installed.CandidateArtifactError, integrity.WheelIntegrityError)):
        install(pair_files, **options)
    assert pair_files.calls == [] and list(pair_files.site.iterdir()) == []
    assert_no_success_receipt_or_private_copy(pair_files)


@pytest.mark.parametrize("defect", ["missing-site", "missing-python", "other-python", "outside-site", "linked-venv"])
def test_explicit_selected_venv_paths_required_before_private_copy(pair_files, monkeypatch, defect):
    options = {}
    if defect in {"missing-site", "missing-python", "other-python"}:
        field, value = {"missing-site": ("site_packages", None), "missing-python": ("sdk_python", None),
            "other-python": ("sdk_python", pair_files.python.with_name("different-python"))}[defect]
        options[field] = value
    elif defect == "outside-site":
        outside = pair_files.root.parent / "outside-site"
        outside.mkdir()
        options["site_packages"] = outside
    else:
        venv = pair_files.root / ".venv"
        venv.rename(pair_files.root / "retained-venv")
        venv.symlink_to(pair_files.root / "retained-venv", target_is_directory=True)
    forbid_private_copy(monkeypatch)
    with pytest.raises(installed.CandidateArtifactError):
        install(pair_files, **options)
    assert pair_files.calls == [] and list(pair_files.site.iterdir()) == []
    assert_no_success_receipt_or_private_copy(pair_files)


@pytest.mark.parametrize("defect", ["not-callable", "false", "raised", "async"])
def test_original_synchronous_owner_fence_required_before_reads(pair_files, monkeypatch, defect):
    def no_read(*args, **kwargs):
        raise AssertionError("The owner fence must run before any source read.")
    monkeypatch.setattr(installed, "read_owned", no_read)
    async def asynchronous():
        return True
    def raised():
        raise installed.CandidateArtifactError("Original caller fence refuses.")
    fence = {"not-callable": None, "false": lambda: False, "raised": raised, "async": asynchronous}[defect]
    with pytest.raises(installed.CandidateArtifactError):
        install(pair_files, require_current=fence)
    assert pair_files.calls == [] and pair_files.verified is None
    assert_no_success_receipt_or_private_copy(pair_files)


def test_owner_change_after_exact_byte_check_refuses_before_staging(pair_files, monkeypatch):
    original = integrity.check_meta_toolkit_wheels
    def changed(*args, **kwargs):
        facts = original(*args, **kwargs)
        pair_files.current = False
        return facts
    monkeypatch.setattr(integrity, "check_meta_toolkit_wheels", changed)
    forbid_private_copy(monkeypatch)
    with pytest.raises(installed.CandidateArtifactError, match="owner changed"):
        install(pair_files)
    assert pair_files.calls == [] and list(pair_files.site.iterdir()) == []
    assert_no_success_receipt_or_private_copy(pair_files)


def test_owner_change_after_private_copy_refuses_before_pip(pair_files, monkeypatch):
    original = installed.atomic_write
    def changed(path, *args, **kwargs):
        original(path, *args, **kwargs)
        if path.name == "requirements.txt":
            pair_files.current = False
    monkeypatch.setattr(installed, "atomic_write", changed)
    with pytest.raises(installed.CandidateArtifactError, match="owner changed"):
        install(pair_files)
    assert pair_files.calls == [] and list(pair_files.site.iterdir()) == []
    assert_no_success_receipt_or_private_copy(pair_files)


def test_owner_change_during_pip_refuses_before_readback_without_claiming_rollback(pair_files, monkeypatch):
    original = installed.subprocess.run
    def changed(*args, **kwargs):
        result = original(*args, **kwargs)
        pair_files.current = False
        return result
    def no_readback(*args, **kwargs):
        raise AssertionError("Lost ownership must refuse before installed readback.")
    monkeypatch.setattr(installed.subprocess, "run", changed)
    monkeypatch.setattr(installed, "compare_installed_meta_toolkit_wheels", no_readback)
    with pytest.raises(installed.CandidateArtifactError, match="owner changed"):
        install(pair_files)
    assert len(pair_files.calls) == 1 and (pair_files.site / "meerkat_toolkit/host_control.py").exists()
    assert_no_success_receipt_or_private_copy(pair_files)


def test_owner_change_during_readback_never_returns_facts(pair_files, monkeypatch):
    original = installed.compare_installed_meta_toolkit_wheels
    def changed(*args, **kwargs):
        result = original(*args, **kwargs)
        pair_files.current = False
        return result
    monkeypatch.setattr(installed, "compare_installed_meta_toolkit_wheels", changed)
    with pytest.raises(installed.CandidateArtifactError, match="owner changed"):
        install(pair_files)
    assert len(pair_files.calls) == 1
    assert_no_success_receipt_or_private_copy(pair_files)


def test_owner_change_during_cleanup_never_returns_facts(pair_files, monkeypatch):
    original = installed.tempfile.TemporaryDirectory
    class ChangedCleanup(original):
        def __exit__(self, *args):
            result = super().__exit__(*args)
            pair_files.current = False
            return result
    monkeypatch.setattr(installed.tempfile, "TemporaryDirectory", ChangedCleanup)
    with pytest.raises(installed.CandidateArtifactError, match="owner changed"):
        install(pair_files)
    assert len(pair_files.calls) == 1
    assert_no_success_receipt_or_private_copy(pair_files)


def test_original_source_change_after_copy_does_not_enter_pip_or_installed_payload(pair_files, monkeypatch):
    original = installed.subprocess.run
    def changed(*args, **kwargs):
        pair_files.toolkit_path.write_bytes(b"unselected replacement source")
        return original(*args, **kwargs)
    monkeypatch.setattr(installed.subprocess, "run", changed)
    result = install(pair_files)
    assert result is pair_files.verified and len(pair_files.calls) == 1
    assert (pair_files.site / "meerkat_toolkit/host_control.py").read_bytes() == pair_files.toolkit[2][
        "meerkat_toolkit/host_control.py"]
    assert_no_success_receipt_or_private_copy(pair_files)


def test_pip_failure_keeps_partial_installed_bytes_and_returns_no_success(pair_files, monkeypatch):
    def partial_pip(argv, **kwargs):
        pair_files.calls.append((argv, kwargs))
        put_installed(pair_files, include_toolkit=False)
        raise subprocess.CalledProcessError(1, argv)
    monkeypatch.setattr(installed.subprocess, "run", partial_pip)
    with pytest.raises(subprocess.CalledProcessError):
        install(pair_files)
    assert len(pair_files.calls) == 1
    assert (pair_files.site / "meerkat_meta/runtime.py").exists()
    assert not (pair_files.site / "meerkat_toolkit").exists()
    assert_no_success_receipt_or_private_copy(pair_files)


def test_actual_mismatched_installed_bytes_refuse_after_successful_inert_pip(pair_files, monkeypatch):
    original = installed.subprocess.run
    def corrupted(*args, **kwargs):
        result = original(*args, **kwargs)
        (pair_files.site / "meerkat_toolkit/host_control.py").write_bytes(b"unselected installed code\n")
        return result
    monkeypatch.setattr(installed.subprocess, "run", corrupted)
    with pytest.raises(installed.CandidateArtifactError, match="differs from the exact source wheel"):
        install(pair_files)
    assert len(pair_files.calls) == 1
    assert (pair_files.site / "meerkat_toolkit/host_control.py").read_bytes() == b"unselected installed code\n"
    assert_no_success_receipt_or_private_copy(pair_files)
