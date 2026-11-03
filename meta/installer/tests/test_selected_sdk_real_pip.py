"""Actual offline pip and inert wheels in a disposable, owned virtualenv.

These exercise installation and readback, not SDK execution, native startup or
qualified package selection. Each test uses its own synthetic project lease.
"""
import base64
import csv
import hashlib
import io
import json
from pathlib import Path
import sys
from types import SimpleNamespace
from urllib.parse import unquote, urlsplit
import zipfile

import pytest

from launchkit import sdk_wheel_integrity as integrity
from launchkit.templates import install_candidate as installed
from launchkit.templates.storage_setup import project_lock
from test_installed_sdk_wheel_proof import inert_wheel as sdk_wheel
from test_selected_dependency_install import inert_wheel as dependency_wheel, put
from test_selected_sdk_supervisor import command_owner, assert_reaped

from sdk_source_import_guard import no_selected_sdk_import


@pytest.fixture
async def actual_pip(tmp_path, monkeypatch):
    root = tmp_path.resolve() / "synthetic project with spaces"
    root.mkdir(mode=0o700)
    inputs = tmp_path.resolve() / "inert selected inputs"
    inputs.mkdir(mode=0o700)
    retained = root / "retained-state.txt"
    retained.write_bytes(b"synthetic history stays unchanged\n")
    owner = command_owner(root)
    # Adversarial ambient pip/Python settings must not redirect this install.
    owner.environment.update(PIP_TARGET=str(tmp_path / "not selected"),
        PIP_PREFIX=str(tmp_path / "other prefix"), PIP_FIND_LINKS="https://not-selected.invalid/",
        PIP_CONFIG_FILE=str(tmp_path / "not-selected.conf"), PYTHONPATH=str(inputs))
    children, commands = [], []
    original_spawn = owner.spawn_child

    async def observe_spawn(*argv, **options):
        commands.append(argv)
        result = await original_spawn(*argv, **options)
        children.append(result[0])
        return result

    owner.spawn_child = observe_spawn
    await owner.command([sys.executable, "-I", "-m", "venv", "--copies", str(root / ".venv")],
        "Disposable venv", 30, raw_outcome=True)
    python = root / ".venv/bin/python"
    probe = await owner.command([str(python), "-I", "-S", "-c",
        "import json,sys,sysconfig;print(json.dumps(sysconfig.get_paths("
        "scheme='posix_venv' if 'posix_venv' in sysconfig.get_scheme_names() else 'posix_prefix',"
        "vars={'base':sys.argv[1],'platbase':sys.argv[1]})))", str(root / ".venv")],
        "Disposable interpreter paths", 5, raw_outcome=True)
    paths = json.loads(probe["stdout"])
    assert paths["purelib"] == paths["platlib"]
    site = Path(paths["purelib"])
    assert site.is_relative_to(root / ".venv") and site.is_dir()
    meta, toolkit = sdk_wheel("meerkat-meta"), sdk_wheel("meerkat-toolkit", top_level=False)
    meta_path, toolkit_path = inputs / "meta-original.bin", inputs / "toolkit-original.bin"
    put(meta_path, meta[0])
    put(toolkit_path, toolkit[0])

    def forbidden(*args, **kwargs):
        pytest.fail("Every selected interpreter command must use the original supervisor.")

    monkeypatch.setattr(installed.subprocess, "run", forbidden)
    files = SimpleNamespace(root=root, inputs=inputs, site=site, python=python,
        owner=owner, children=children, commands=commands, retained=retained,
        meta=meta, toolkit=toolkit, meta_path=meta_path, toolkit_path=toolkit_path)
    try:
        with project_lock(root) as lease:
            files.require_current = lease.require_current
            yield files
    finally:
        if owner.child is not None:
            await owner.terminate(timeout=5)
        assert owner.child is None
        assert getattr(owner, "_selected_sdk_install_task", None) is None
        for child in children:
            assert_reaped(child)


def pair_options(files):
    return dict(meta=files.meta[1], toolkit=files.toolkit[1], site_packages=files.site,
        sdk_python=files.python, require_current=files.require_current, timeout=30)


def selected_dependency(files, *, supported):
    name = "fixture-actual-pip" if supported else "fixture-unsupported-python"
    # Matching filename and WHEEL metadata make the preflight internally valid;
    # cp999 is nevertheless incompatible with the actual selected interpreter.
    original = dependency_wheel(name, "1.0.0")
    if supported:
        content, filename = original.content, original.filename
    else:
        values = dict(original.values)
        prefix = name.replace("-", "_") + "-1.0.0.dist-info/"
        values[prefix + "WHEEL"] = ("Wheel-Version: 1.0\nRoot-Is-Purelib: true\n"
            "Tag: cp999-none-any\n\n").encode()
        record = prefix + "RECORD"
        rows = [[key, "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(value).digest()).decode().rstrip("="),
            str(len(value))] for key, value in values.items() if key != record]
        rows.append([record, "", ""])
        output = io.StringIO()
        csv.writer(output, lineterminator="\n").writerows(rows)
        values[record] = output.getvalue().encode()
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, "w") as wheel:
            for key, value in values.items():
                wheel.writestr(key, value)
        content = archive.getvalue()
        filename = name.replace("-", "_") + "-1.0.0-cp999-none-any.whl"
    ref = put(files.inputs / filename, content)
    requirements = put(files.inputs / "dependency.hashes.txt",
        f"{name}==1.0.0 --hash=sha256:{ref.sha256}\n".encode())
    return dict(dependency_requirements=requirements, dependency_wheels=(ref,))


def assert_real_sdk_readback(files, facts):
    assert facts[0].sha256 == files.meta[1].sha256 and facts[1].sha256 == files.toolkit[1].sha256
    for fact, (_, _, wheel_values) in zip(facts, (files.meta, files.toolkit)):
        for key, expected in wheel_values.items():
            if not key.endswith("/RECORD"):
                assert (files.site / key).read_bytes() == expected
        metadata = files.site / (fact.distribution.replace("-", "_") + f"-{fact.version}.dist-info")
        assert (metadata / "INSTALLER").read_bytes() == b"pip\n"
        assert (metadata / "REQUESTED").read_bytes() == b""
        direct = json.loads((metadata / "direct_url.json").read_bytes())
        assert direct["archive_info"]["hashes"] == {"sha256": fact.sha256}
        url = urlsplit(direct["url"])
        assert url.scheme == "file" and not url.netloc
        staged = Path(unquote(url.path))
        assert staged.parent.parent == files.root / ".venv" and not staged.exists()
        # pip rewrites RECORD to include its generated metadata. Confirm those
        # rows describe the files it actually installed, including their bytes.
        records = list(csv.reader(io.StringIO((metadata / "RECORD").read_text())))
        indexed = {row[0]: row for row in records}
        for name in ("INSTALLER", "REQUESTED", "direct_url.json"):
            key = metadata.relative_to(files.site).as_posix() + "/" + name
            row = indexed[key]
            data = (files.site / key).read_bytes()
            assert row[1] == "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).decode().rstrip("=")
            assert int(row[2]) == len(data)
    assert files.retained.read_bytes() == b"synthetic history stays unchanged\n"
    assert files.meta_path.read_bytes() == files.meta[0] and files.toolkit_path.read_bytes() == files.toolkit[0]
    assert not list((files.root / ".venv").glob(".forge-selected-*"))
    assert not (files.root / ".venv/forge-sdk.receipt.json").exists()
    assert "fixture_actual_pip" not in sys.modules


async def test_actual_offline_pip_installs_pair_dependency_and_reads_back_generated_metadata(actual_pip):
    files = actual_pip
    facts = await files.owner.install_selected_sdk_pair(files.meta_path, files.toolkit_path,
        **pair_options(files), **selected_dependency(files, supported=True))
    assert_real_sdk_readback(files, facts)
    assert (files.site / "fixture_actual_pip/__init__.py").read_bytes() == b"inert payload, never imported\n"
    pip_commands = [args for args in files.commands if "pip" in args]
    assert len(pip_commands) == 2
    assert all(args[0] == str(files.python) and "--no-index" in args and "--no-deps" in args
        and "--require-hashes" in args for args in pip_commands)
    assert "--only-binary=:all:" in pip_commands[1]
    assert not (files.root.parent / "not selected").exists()
    assert not (files.root.parent / "other prefix").exists()


async def test_actual_pip_refuses_incompatible_dependency_and_preserves_partial_sdk_install(actual_pip):
    files = actual_pip
    with pytest.raises(installed.CandidateArtifactError, match="selected interpreter command failed"):
        await files.owner.install_selected_sdk_pair(files.meta_path, files.toolkit_path,
            **pair_options(files), **selected_dependency(files, supported=False))
    assert files.owner.child is None
    assert all(child.returncode is not None for child in files.children)
    facts = integrity.check_meta_toolkit_wheels(files.meta[0], files.toolkit[0],
        meta=files.meta[1], toolkit=files.toolkit[1])
    assert_real_sdk_readback(files, facts)
    assert not (files.site / "fixture_unsupported_python").exists()
    assert len([args for args in files.commands if "pip" in args]) == 2
    assert any("not a supported wheel" in line for _, line in files.owner.logs)
