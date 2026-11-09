"""The Guide runs the workspace's own Meta release: runtime admission, the
client wheel's identity and the development pin. Every accepted and refused
version derives from guide_sdk.SELECTED_VERSION. Inert files only; nothing runs."""
import hashlib
from importlib import metadata
import io
from pathlib import Path
import re
import tomllib
import zipfile

import pytest

from launchkit import guide_sdk
from launchkit.bootstrap import GatewayInstallError, SelectedGuideRuntime
from launchkit.connect_installer_caller import ExactFile

SELECTED = guide_sdk.SELECTED_VERSION
INSTALLER = Path(__file__).parents[1]


def neighbours(version):
    """The adjacent patch releases of an exact x.y.z version."""
    major, minor, patch = (int(part) for part in version.split("."))
    return [f"{major}.{minor}.{patch + step}" for step in (-1, 1) if patch + step >= 0]


def client_wheel(version=None, *, name="meerkat-meta", tag="py3-none-any", purelib="true", metadata_version=None):
    """An inert meerkat-meta wheel of a Meta release: its metadata only, never installed.

    metadata_version, when given, is the Version its METADATA declares in
    place of the release's own, under the release's dist-info directory.
    """
    version = SELECTED if version is None else version
    info = f"meerkat_meta-{version}.dist-info/"
    declared = version if metadata_version is None else metadata_version
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("meerkat_meta/__init__.py", "# inert test fixture, never imported\n")
        archive.writestr(info + "METADATA", f"Metadata-Version: 2.4\nName: {name}\nVersion: {declared}\n")
        archive.writestr(info + "WHEEL", f"Wheel-Version: 1.0\nRoot-Is-Purelib: {purelib}\nTag: {tag}\n")
    return buffer.getvalue()


# Both neighbours, the Guide's earlier 0.9.3 release, mixed pairs and
# near-miss spellings of the selected version.
REFUSED = list(dict.fromkeys([
    *((version, version) for version in ("0.9.3", *neighbours(SELECTED))),
    *(pair for version in neighbours(SELECTED) for pair in ((SELECTED, version), (version, SELECTED))),
    (f"{SELECTED}+meta.0bc6b4fa", SELECTED), (f"v{SELECTED}", SELECTED), (f"{SELECTED}0", SELECTED),
    (SELECTED, f"{SELECTED} "),
]))


@pytest.fixture
def gateway(tmp_path):
    path = tmp_path / "inert-rpc-gateway"
    path.write_bytes(b"inert test fixture, never execute\n")
    path.chmod(0o700)
    return ExactFile(path, hashlib.sha256(path.read_bytes()).hexdigest())


def test_neighbours_are_the_adjacent_patch_releases():
    assert neighbours("0.9.3") == ["0.9.2", "0.9.4"]
    assert neighbours("0.10.0") == ["0.10.1"]


def test_the_guide_admits_its_selected_runtime(gateway):
    assert re.fullmatch("[0-9]+\\.[0-9]+\\.[0-9]+", SELECTED)
    selected = SelectedGuideRuntime(gateway, SELECTED, SELECTED, "a" * 40, "b" * 40)
    assert (selected.version, selected.core_version) == (SELECTED, SELECTED)
    assert guide_sdk.declared_core_version() == SELECTED


@pytest.mark.parametrize("version, core_version", REFUSED)
def test_the_guide_refuses_every_other_runtime_version(gateway, version, core_version):
    with pytest.raises(GatewayInstallError, match=re.escape(f"exact selected Meta {SELECTED} runtime")):
        SelectedGuideRuntime(gateway, version, core_version, "a" * 40, "b" * 40)


@pytest.mark.parametrize("value", [None, "0.9", "v0.9.11", "0.9.11-dev", "0.9.11 ", 9])
def test_an_invalid_workspace_version_refuses_every_use(monkeypatch, value):
    monkeypatch.setattr(guide_sdk, "SELECTED_VERSION", value)
    for use in (guide_sdk.declared_core_version, guide_sdk.wheel_path,
                lambda: guide_sdk.selection("a" * 64, 1), lambda: guide_sdk.verify(client_wheel())):
        with pytest.raises(ValueError, match="exact Meta version"):
            use()


def test_the_development_environment_runs_this_commits_meta_sdk():
    """The installer project pins the workspace's Meta version, and the
    environment it runs in carries that SDK: this commit's sdk/python."""
    workspace = tomllib.loads((INSTALLER.parent / "sdk/python/pyproject.toml").read_text())["project"]["version"]
    assert SELECTED == workspace
    dependencies = tomllib.loads((INSTALLER / "pyproject.toml").read_text())["project"]["dependencies"]
    assert [value for value in dependencies if value.startswith("meerkat-meta")] == [f"meerkat-meta=={SELECTED}"]
    assert metadata.version("meerkat-meta") == SELECTED


def test_the_guide_client_is_the_published_wheel_of_its_release():
    guide_sdk.verify(client_wheel())
    assert guide_sdk.wheel_path() == f"vendor/guide/meerkat_meta-{SELECTED}-py3-none-any.whl"
    refused = {
        "another release": client_wheel(neighbours(SELECTED)[0]),
        "another release's metadata": client_wheel(metadata_version=neighbours(SELECTED)[0]),
        "another distribution": client_wheel(name="meerkat-mobkit"),
        "a platform wheel": client_wheel(tag="cp312-cp312-linux_x86_64"),
        "not purelib": client_wheel(purelib="false"),
        "not a wheel": b"inert test fixture, not a zip\n",
        "empty": b"",
        "not bytes": "text",
    }
    for label, content in refused.items():
        with pytest.raises(ValueError, match=re.escape(f"not the published Meta {SELECTED} wheel")):
            guide_sdk.verify(content)
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("meerkat_meta/__init__.py", "# no metadata\n")
    with pytest.raises(ValueError, match="not the published"):
        guide_sdk.verify(buffer.getvalue())


def test_the_package_records_the_client_path_version_and_bytes():
    content = client_wheel()
    digest = hashlib.sha256(content).hexdigest()
    assert guide_sdk.selection(digest, len(content)) == {"path": guide_sdk.wheel_path(), "version": SELECTED,
        "sha256": digest, "bytes": len(content)}
    for sha256, size in (("A" * 64, 1), ("a" * 63, 1), (None, 1), (digest, 0), (digest, -1), (digest, True),
                         (digest, "1")):
        with pytest.raises(ValueError, match="exact size and SHA-256"):
            guide_sdk.selection(sha256, size)
