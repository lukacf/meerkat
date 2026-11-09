"""Selected dependency batch controls with inert wheel and pip bytes only."""
import base64
import csv
from dataclasses import replace
import hashlib
import io
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
from types import SimpleNamespace
from urllib.parse import unquote, urlsplit
import zipfile

import pytest

from launchkit import sdk_wheel_integrity as integrity
from launchkit.runtime_candidates import ArtifactFile
from launchkit.templates import install_candidate as installed


def hashed(content):
    return hashlib.sha256(content).hexdigest()


def normalized(name):
    return re.sub(r"[-_.]+", "-", name).lower()


def inert_wheel(name, version, *, compiled=False, defect=None):
    """Construct bytes, including ordinary compiled-wheel directories, never code."""
    package = name.replace("-", "_")
    prefix = f"{package}-{version}.dist-info/"
    tags = (["cp312-abi3-manylinux_2_17_x86_64", "cp312-abi3-manylinux2014_x86_64"]
            if compiled else ["py2-none-any", "py3-none-any"])
    filename = (f"{package}-{version}-1build-cp312-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl"
                if compiled else f"{package}-{version}-py2.py3-none-any.whl")
    metadata = f"Metadata-Version: 2.4\nName: {name}\nVersion: {version}\nRequires-Python: >=3.11\n"
    if defect == "metadata-name":
        metadata = metadata.replace(f"Name: {name}", "Name: other-package")
    elif defect == "metadata-version":
        metadata = metadata.replace(f"Version: {version}", "Version: 9.9.9")
    elif defect == "duplicate-name":
        metadata += f"Name: {name}\n"
    wheel = "Wheel-Version: 1.0\nGenerator: inert-fixture\nRoot-Is-Purelib: " + ("false" if compiled else "true") + "\n"
    if compiled:
        wheel += "Build: 1build\n"
    wheel += "".join(f"Tag: {tag}\n" for tag in tags)
    if defect == "metadata-tag":
        wheel = wheel.replace(tags[0], "py3-none-any")
    elif defect == "duplicate-tag":
        wheel += f"Tag: {tags[0]}\n"
    values = {
        package + ("/_inert.abi3.so" if compiled else "/__init__.py"): b"inert payload, never imported\n",
        prefix + "METADATA": metadata.encode(),
        prefix + "WHEEL": wheel.encode(),
        prefix + "licenses/LICENSE": b"inert license text\n",
    }
    if defect == "second-distribution":
        values["other_package-1.0.dist-info/METADATA"] = b"Name: other-package\nVersion: 1.0\n"
    record = prefix + "RECORD"
    rows = [[path, "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(value).digest()).decode().rstrip("="), str(len(value))]
            for path, value in values.items()]
    rows.append([record, "", ""])
    if defect == "record-digest":
        rows[0][1] = "sha256=" + "A" * 43
    elif defect == "record-missing":
        rows.pop(0)
    output = io.StringIO(newline="")
    csv.writer(output, lineterminator="\n").writerows(rows)
    values[record] = output.getvalue().encode()
    directories = [package + "/", prefix, prefix + "licenses/"] if compiled else []
    directory_extra = None
    if defect and defect.startswith("directory-"):
        directory_extra = {
            "directory-traversal": ("../escape/", stat.S_IFDIR | 0o755, b""),
            "directory-absolute": ("/escape/", stat.S_IFDIR | 0o755, b""),
            "directory-backslash": ("escape\\folder/", stat.S_IFDIR | 0o755, b""),
            "directory-link": ("linked/", stat.S_IFLNK | 0o777, b""),
            "directory-file-mode": ("ordinary/", stat.S_IFREG | 0o644, b""),
            "directory-payload": ("nonempty/", stat.S_IFDIR | 0o755, b"unexpected"),
            "directory-noncanonical": ("nested//path/", stat.S_IFDIR | 0o755, b""),
        }.get(defect)
    data = io.BytesIO()
    with zipfile.ZipFile(data, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for directory in directories:
            info = zipfile.ZipInfo(directory)
            info.external_attr = ((stat.S_IFDIR | 0o755) << 16) | 0x10
            archive.writestr(info, b"")
        if directory_extra:
            path, mode, payload = directory_extra
            info = zipfile.ZipInfo(path)
            info.external_attr = (mode << 16) | 0x10
            archive.writestr(info, payload)
        for path, value in values.items():
            info = zipfile.ZipInfo(path)
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            archive.writestr(info, value)
    return SimpleNamespace(name=normalized(name), version=version, filename=filename,
        content=data.getvalue(), values=values)


def put(path, content):
    path.write_bytes(content)
    path.chmod(0o600)
    return ArtifactFile(path, len(content), hashed(content))


def select(files, batch, *, requirements=None):
    files.batch = tuple(batch)
    files.wheels = tuple(put(files.inputs / item.filename, item.content) for item in files.batch)
    lines = [f"{item.name}=={item.version} --hash=sha256:{ref.sha256}"
             for item, ref in zip(files.batch, files.wheels)]
    content = requirements if requirements is not None else ("# Held fixture graph\n\n" + "\n".join(lines) + "\n").encode()
    files.requirements = put(files.inputs / "held.hashes.txt", content)


@pytest.fixture
def dependency_files(tmp_path, monkeypatch):
    root = tmp_path / "selected deployment with spaces"
    root.mkdir(mode=0o700)
    site = root / ".venv/lib/python-fixture/site-packages"
    site.mkdir(parents=True)
    python = root / ".venv/bin/python"
    python.parent.mkdir()
    python.write_bytes(b"inert selected interpreter, never executed\n")
    python.chmod(0o700)
    inputs = tmp_path / "held input bytes"
    inputs.mkdir(mode=0o700)
    (root / "retained.txt").write_bytes(b"retained application bytes\n")
    files = SimpleNamespace(root=root, site=site, python=python, inputs=inputs,
        calls=[], checks=[], current=True, copied=())
    select(files, [inert_wheel("fixture-portable", "1.2.0"), inert_wheel("fixture-native", "2.1.0", compiled=True)])

    def require_current():
        files.checks.append(files.current)
        if not files.current:
            raise installed.CandidateArtifactError("Original fixture owner changed.")

    def inert_pip(argv, **kwargs):
        files.calls.append((argv, kwargs))
        assert argv == [str(python), "-I", "-m", "pip", "--isolated", "install",
            "--disable-pip-version-check", "--no-index", "--no-deps", "--no-compile",
            "--force-reinstall", "--require-hashes", "--only-binary=:all:", "-r", argv[-1]]
        assert set(kwargs) == {"check", "env"} and kwargs["check"] is True
        environment = kwargs["env"]
        assert type(environment) is dict and environment.get("PIP_CONFIG_FILE") == os.devnull
        assert set(environment) == set(os.environ) | {"PIP_CONFIG_FILE"}
        assert all(environment[key] == value for key, value in os.environ.items() if key != "PIP_CONFIG_FILE")
        requirements = Path(argv[-1])
        assert requirements.parent.parent == root / ".venv"
        assert requirements.parent.name.startswith(".forge-selected-dependencies-")
        assert stat.S_IMODE(requirements.parent.stat().st_mode) == 0o700
        assert stat.S_IMODE(requirements.stat().st_mode) == 0o600
        lines = requirements.read_text().splitlines()
        assert len(lines) == len(files.batch)
        copied = []
        for line, item, ref in zip(lines, files.batch, files.wheels):
            match = re.fullmatch(r"([a-z0-9-]+) @ (file://\S+) --hash=sha256:([a-f0-9]{64})", line)
            assert match and match[1] == item.name and match[3] == ref.sha256
            parsed = urlsplit(match[2])
            assert parsed.scheme == "file" and not parsed.netloc and not parsed.query and not parsed.fragment
            path = Path(unquote(parsed.path))
            assert path.parent == requirements.parent and path.name == item.filename
            assert stat.S_IMODE(path.stat().st_mode) == 0o600
            assert path.read_bytes() == item.content and hashed(item.content) == ref.sha256
            assert str(files.inputs) not in line
            copied.append(path)
        assert {path.name for path in requirements.parent.iterdir()} == {"requirements.txt", *(path.name for path in copied)}
        assert all(str(ref.path) not in argument for ref in (files.requirements, *files.wheels) for argument in argv)
        files.copied = tuple(copied)
        return SimpleNamespace(returncode=0)

    files.require_current = require_current
    monkeypatch.setattr(installed.subprocess, "run", inert_pip)
    return files


def install(files, **options):
    arguments = dict(deployment_root=files.root, site_packages=files.site,
        sdk_python=files.python, require_current=files.require_current)
    arguments.update(options)
    return installed.install_selected_dependency_wheels(files.requirements, files.wheels, **arguments)


def assert_retained(files):
    assert not list((files.root / ".venv").glob(".forge-selected-dependencies-*"))
    assert not (files.root / ".venv/forge-sdk.receipt.json").exists()
    assert (files.root / "retained.txt").read_bytes() == b"retained application bytes\n"


def forbid_staging(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Unselected inputs must fail before private staging.")
    monkeypatch.setattr(installed.tempfile, "TemporaryDirectory", forbidden)


def test_exact_batch_preserves_portable_compiled_build_and_compressed_tags(dependency_files, monkeypatch):
    files = dependency_files
    for key, value in {"PIP_INDEX_URL": "https://unselected.invalid/simple", "PIP_EXTRA_INDEX_URL": "https://unselected.invalid/extra",
            "PIP_FIND_LINKS": "/unselected/wheels", "PIP_TARGET": "/unselected/target", "PIP_PREFIX": "/unselected/prefix",
            "PIP_CONFIG_FILE": "/unselected/pip.conf", "PYTHONPATH": "/unselected/python"}.items():
        monkeypatch.setenv(key, value)
    before = {ref.path: ref.path.read_bytes() for ref in (files.requirements, *files.wheels)}
    result = install(files)
    assert result is files.wheels and len(files.calls) == 1 and all(files.checks)
    assert {path: path.read_bytes() for path in before} == before
    assert files.copied and all(not path.exists() for path in files.copied)
    assert list(files.site.iterdir()) == [], "A pip action result must not invent installed-byte readback."
    assert not any(name in sys.modules for name in ("fixture_portable", "fixture_native"))
    assert_retained(files)


@pytest.mark.parametrize("line", ["--index-url https://unselected.invalid", "--find-links /unselected", "-r other.txt", "-c other.txt",
    "-e .", "fixture-portable @ https://unselected.invalid/wheel.whl", "fixture-portable[extra]==1.2.0 --hash=sha256:{hash}",
    "fixture-portable>=1.2.0 --hash=sha256:{hash}", "fixture-portable==1.* --hash=sha256:{hash}",
    "fixture-portable==1.2.0; python_version>'3' --hash=sha256:{hash}", "fixture-portable==1.2.0 --hash=sha256:{hash} # discard me",
    "fixture-portable==1.2.0 --hash=sha256:{hash} --hash=sha256:{hash}", "fixture-portable==1.2.0 \\",
    "fixture-portable==1.2.0 --hash=sha512:{hash}"])
def test_active_requirements_directives_never_reach_staging(dependency_files, monkeypatch, line):
    files = dependency_files
    files.requirements = put(files.requirements.path, (line.format(hash=files.wheels[0].sha256) + "\n").encode())
    forbid_staging(monkeypatch)
    with pytest.raises(installed.CandidateArtifactError):
        install(files)
    assert files.calls == [] and list(files.site.iterdir()) == []
    assert_retained(files)


@pytest.mark.parametrize("defect", ["missing", "extra", "duplicate-normalized", "wrong-version", "wrong-hash", "meta", "toolkit", "legacy-sdk"])
def test_one_to_one_selected_pins_cannot_be_missing_extra_or_replace_sdks(dependency_files, monkeypatch, defect):
    files = dependency_files
    lines = files.requirements.path.read_text().splitlines()
    if defect == "missing":
        files.wheels = files.wheels[:-1]
    elif defect == "extra":
        extra = inert_wheel("fixture-extra", "1.0.0")
        files.wheels += (put(files.inputs / extra.filename, extra.content),)
    elif defect == "duplicate-normalized":
        lines[-1] = f"Fixture_Portable==1.2.0 --hash=sha256:{files.wheels[0].sha256}"
    elif defect == "wrong-version":
        lines[-1] = lines[-1].replace("==2.1.0", "==2.2.0")
    elif defect == "wrong-hash":
        lines[-1] = lines[-1].replace(files.wheels[-1].sha256, "0" * 64)
    else:
        sdk = {"meta": "Meerkat.Meta", "toolkit": "meerkat_toolkit", "legacy-sdk": "meerkat-mobkit"}[defect]
        lines[-1] = f"{sdk}==2.1.0 --hash=sha256:{files.wheels[-1].sha256}"
    files.requirements = put(files.requirements.path, ("\n".join(lines) + "\n").encode())
    forbid_staging(monkeypatch)
    with pytest.raises(installed.CandidateArtifactError):
        install(files)
    assert files.calls == []
    assert_retained(files)


@pytest.mark.parametrize("defect", ["metadata-name", "metadata-version", "duplicate-name", "metadata-tag", "duplicate-tag",
    "second-distribution", "record-digest", "record-missing", "directory-traversal", "directory-absolute", "directory-backslash",
    "directory-link", "directory-file-mode", "directory-payload", "directory-noncanonical"])
def test_wheel_metadata_record_and_directory_shape_refuse_before_staging(dependency_files, monkeypatch, defect):
    files = dependency_files
    select(files, [inert_wheel("fixture-native", "2.1.0", compiled=True, defect=defect)])
    forbid_staging(monkeypatch)
    with pytest.raises((installed.CandidateArtifactError, integrity.WheelIntegrityError)):
        install(files)
    assert files.calls == []
    assert_retained(files)


@pytest.mark.parametrize("defect", ["duplicate-wheel", "source-archive", "invalid-filename", "invalid-encoding", "invalid-zip"])
def test_nonwheel_duplicate_or_unparseable_inputs_cannot_be_installed(dependency_files, monkeypatch, defect):
    files = dependency_files
    if defect == "duplicate-wheel":
        files.wheels = (files.wheels[0], files.wheels[0])
    elif defect in {"source-archive", "invalid-filename"}:
        ref = files.wheels[0]
        path = ref.path.with_name("fixture_portable-1.2.0.tar.gz" if defect == "source-archive" else "no-wheel-tags.whl")
        ref.path.rename(path)
        files.wheels = (replace(ref, path=path), *files.wheels[1:])
    elif defect == "invalid-encoding":
        files.requirements = put(files.requirements.path, b"\xff\n")
    else:
        files.wheels = (put(files.wheels[0].path, b"not a ZIP wheel\n"), *files.wheels[1:])
        lines = [f"{item.name}=={item.version} --hash=sha256:{ref.sha256}" for item, ref in zip(files.batch, files.wheels)]
        files.requirements = put(files.requirements.path, ("\n".join(lines) + "\n").encode())
    forbid_staging(monkeypatch)
    with pytest.raises((installed.CandidateArtifactError, UnicodeDecodeError, zipfile.BadZipFile)):
        install(files)
    assert files.calls == []
    assert_retained(files)


@pytest.mark.parametrize("source", ["requirements", "wheel"])
@pytest.mark.parametrize("defect", ["changed-bytes", "changed-size", "invalid-reference", "symlink", "hardlink", "writable"])
def test_exact_owned_source_integrity_and_paths_before_private_mutation(dependency_files, monkeypatch, source, defect):
    files = dependency_files
    ref = files.requirements if source == "requirements" else files.wheels[0]
    changed = ref
    if defect == "changed-bytes":
        ref.path.write_bytes(ref.path.read_bytes() + b"changed")
    elif defect == "changed-size":
        changed = replace(ref, size=ref.size + 1)
    elif defect == "invalid-reference":
        changed = SimpleNamespace(**vars(ref))
    elif defect in {"symlink", "hardlink"}:
        original = ref.path.with_name("retained-" + ref.path.name)
        ref.path.rename(original)
        ref.path.symlink_to(original) if defect == "symlink" else ref.path.hardlink_to(original)
    else:
        ref.path.chmod(0o666)
    if source == "requirements":
        files.requirements = changed
    else:
        files.wheels = (changed, *files.wheels[1:])
    forbid_staging(monkeypatch)
    with pytest.raises(installed.CandidateArtifactError):
        install(files)
    assert files.calls == [] and list(files.site.iterdir()) == []
    assert_retained(files)


@pytest.mark.parametrize("defect", ["missing-site", "missing-python", "other-python", "outside-site", "linked-venv"])
def test_explicit_owned_target_venv_required(dependency_files, monkeypatch, defect):
    files = dependency_files
    options = {}
    if defect in {"missing-site", "missing-python", "other-python"}:
        key, value = {"missing-site": ("site_packages", None), "missing-python": ("sdk_python", None),
            "other-python": ("sdk_python", files.python.with_name("other-python"))}[defect]
        options[key] = value
    elif defect == "outside-site":
        outside = files.root.parent / "outside-site"
        outside.mkdir(); options["site_packages"] = outside
    else:
        (files.root / ".venv").rename(files.root / "retained-venv")
        (files.root / ".venv").symlink_to(files.root / "retained-venv", target_is_directory=True)
    forbid_staging(monkeypatch)
    with pytest.raises(installed.CandidateArtifactError):
        install(files, **options)
    assert files.calls == []
    assert_retained(files)


def test_selected_venv_python_link_remains_an_ordinary_supported_target(dependency_files):
    files = dependency_files
    base = files.root.parent / "inert-base-python"
    files.python.rename(base)
    files.python.symlink_to(base)
    assert install(files) is files.wheels
    assert len(files.calls) == 1
    assert_retained(files)


@pytest.mark.parametrize("defect", ["not-callable", "false", "raised", "async"])
def test_original_synchronous_owner_required_before_reads(dependency_files, monkeypatch, defect):
    def no_read(*args, **kwargs):
        raise AssertionError("Original owner must be checked before any source read.")
    monkeypatch.setattr(installed, "read_owned", no_read)
    async def asynchronous():
        return True
    def raised():
        raise installed.CandidateArtifactError("Original fixture refuses.")
    fence = {"not-callable": None, "false": lambda: False, "raised": raised, "async": asynchronous}[defect]
    with pytest.raises(installed.CandidateArtifactError):
        install(dependency_files, require_current=fence)
    assert dependency_files.calls == []
    assert_retained(dependency_files)


@pytest.mark.parametrize("phase", ["read", "staging", "pip", "cleanup"])
def test_owner_change_at_mutation_boundaries_never_returns_selected_success(dependency_files, monkeypatch, phase):
    files = dependency_files
    if phase == "read":
        original = installed.read_owned
        def changed(path, *args, **kwargs):
            content = original(path, *args, **kwargs)
            if path == files.wheels[-1].path:
                files.current = False
            return content
        monkeypatch.setattr(installed, "read_owned", changed)
        forbid_staging(monkeypatch)
    elif phase == "staging":
        original = installed.atomic_write
        def changed(path, *args, **kwargs):
            original(path, *args, **kwargs)
            if path.name == "requirements.txt":
                files.current = False
        monkeypatch.setattr(installed, "atomic_write", changed)
    elif phase == "pip":
        original = installed.subprocess.run
        def changed(*args, **kwargs):
            result = original(*args, **kwargs)
            (files.site / "partial.txt").write_bytes(b"possibly installed state\n")
            files.current = False
            return result
        monkeypatch.setattr(installed.subprocess, "run", changed)
    else:
        original = installed.tempfile.TemporaryDirectory
        class ChangedCleanup(original):
            def __exit__(self, *args):
                result = super().__exit__(*args)
                files.current = False
                return result
        monkeypatch.setattr(installed.tempfile, "TemporaryDirectory", ChangedCleanup)
    with pytest.raises(installed.CandidateArtifactError, match="owner changed"):
        install(files)
    assert len(files.calls) == (1 if phase in {"pip", "cleanup"} else 0)
    if phase == "pip":
        assert (files.site / "partial.txt").read_bytes() == b"possibly installed state\n"
    assert_retained(files)


def test_replaced_original_sources_cannot_enter_the_private_pip_input(dependency_files, monkeypatch):
    files = dependency_files
    original = installed.subprocess.run
    def replaced(*args, **kwargs):
        for ref in (files.requirements, *files.wheels):
            ref.path.write_bytes(b"unselected original replacement\n")
        return original(*args, **kwargs)
    monkeypatch.setattr(installed.subprocess, "run", replaced)
    assert install(files) is files.wheels
    assert len(files.calls) == 1
    assert all(ref.path.read_bytes() == b"unselected original replacement\n" for ref in (files.requirements, *files.wheels))
    assert_retained(files)


def test_incompatible_or_failed_pip_preserves_partial_state_without_success_or_retry(dependency_files, monkeypatch):
    files = dependency_files
    original = installed.subprocess.run
    def failed(argv, **kwargs):
        original(argv, **kwargs)
        (files.site / "partial.txt").write_bytes(b"partial install, no rollback claimed\n")
        raise subprocess.CalledProcessError(1, argv, stderr="inert incompatible wheel")
    monkeypatch.setattr(installed.subprocess, "run", failed)
    with pytest.raises(subprocess.CalledProcessError, match="returned non-zero exit status 1"):
        install(files)
    assert len(files.calls) == 1
    assert (files.site / "partial.txt").read_bytes() == b"partial install, no rollback claimed\n"
    assert_retained(files)


def test_canonical_file_directory_collision_refuses_before_staging(dependency_files, monkeypatch):
    files = dependency_files
    item = inert_wheel("fixture-native", "2.1.0", compiled=True)
    content = io.BytesIO(item.content)
    with zipfile.ZipFile(content, "a") as archive:
        info = zipfile.ZipInfo("Fixture_Native")
        info.external_attr = (stat.S_IFREG | 0o644) << 16
        archive.writestr(info, b"conflicting ordinary file\n")
    item.content = content.getvalue()
    select(files, [item])
    forbid_staging(monkeypatch)
    with pytest.raises(integrity.WheelIntegrityError, match="conflicting file or directory path"):
        install(files)
    assert files.calls == [] and list(files.site.iterdir()) == []
    assert_retained(files)


def test_directory_bearing_wheel_requires_explicit_shared_reader_opt_in():
    item = inert_wheel("fixture-native", "2.1.0", compiled=True)
    with zipfile.ZipFile(io.BytesIO(item.content)) as archive:
        inventory = archive.namelist()
    options = dict(expected_names=inventory, max_file_bytes=installed.MAX_WHEEL_BYTES,
        max_total_bytes=integrity.MAX_WHEEL_TOTAL, max_files=integrity.MAX_WHEEL_FILES)
    with pytest.raises(integrity.WheelIntegrityError, match="invalid path, link, or oversized member"):
        integrity.read_wheel_members(item.content, **options)
    values = integrity.read_wheel_members(item.content, allow_directories=True, **options)
    assert values == item.values
    assert not any(name.endswith("/") for name in values)
    integrity.verify_wheel_record(values, "fixture_native-2.1.0.dist-info/RECORD")
