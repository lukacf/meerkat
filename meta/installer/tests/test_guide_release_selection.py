"""The Guide's one exact selected release: runtime admission, the client pin
and the release inputs skeleton. Every accepted and refused version derives
from guide_sdk.SELECTED_VERSION. Inert files only; nothing runs."""
import hashlib
import io
import json
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
SKELETON = INSTALLER / "release/inputs/forge-release-inputs.skeleton.json"
PIN = ("SOURCE", "TREE", "SHA256", "SIZE")
CLIENT = b"inert guide client wheel, never install\n"


def neighbours(version):
    """The adjacent patch releases of an exact x.y.z version."""
    major, minor, patch = (int(part) for part in version.split("."))
    return [f"{major}.{minor}.{patch + step}" for step in (-1, 1) if patch + step >= 0]


# Both neighbours, the Guide's previous 0.9.0 selection, mixed pairs and
# near-miss spellings of the selected version.
REFUSED = list(dict.fromkeys([
    *((version, version) for version in ("0.9.0", *neighbours(SELECTED))),
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


@pytest.mark.parametrize("version, core_version", REFUSED)
def test_the_guide_refuses_every_other_runtime_version(gateway, version, core_version):
    with pytest.raises(GatewayInstallError, match=re.escape(f"exact selected Meta {SELECTED} runtime")):
        SelectedGuideRuntime(gateway, version, core_version, "a" * 40, "b" * 40)


def test_the_committed_guide_client_pin_admits_exactly_the_vendored_published_wheel():
    """The selected release's published wheel is the only wheel beside the
    Guide. The pin, its bytes, its version and both installer pins agree."""
    assert guide_sdk.WHEEL == f"vendor/guide/meerkat_meta-{SELECTED}-py3-none-any.whl"
    wheel = INSTALLER / guide_sdk.WHEEL
    assert sorted(path.name for path in wheel.parent.glob("*.whl")) == [wheel.name]
    content = wheel.read_bytes()
    guide_sdk.verify(content)
    assert guide_sdk.selection() == {"path": guide_sdk.WHEEL, "version": SELECTED, "source": guide_sdk.SOURCE,
        "tree": guide_sdk.TREE, "sha256": hashlib.sha256(content).hexdigest(), "bytes": len(content)}
    with zipfile.ZipFile(io.BytesIO(content)) as archive:
        metadata = archive.read(f"meerkat_meta-{SELECTED}.dist-info/METADATA").decode()
    assert f"\nVersion: {SELECTED}\n" in metadata
    with pytest.raises(ValueError, match="differs from its reviewed wheel"):
        guide_sdk.verify(content[:-1] + bytes([content[-1] ^ 1]))
    requirements = (INSTALLER / "requirements-guide-sdk.txt").read_text().splitlines()
    assert [line for line in requirements if not line.startswith("#")] == [
        f"meerkat-meta=={SELECTED} --hash=sha256:{guide_sdk.SHA256}"]
    dependencies = tomllib.loads((INSTALLER / "pyproject.toml").read_text())["project"]["dependencies"]
    assert [value for value in dependencies if value.startswith("meerkat-meta")] == [f"meerkat-meta=={SELECTED}"]


def complete_pin(monkeypatch, content=CLIENT):
    values = ("c" * 40, "d" * 40, hashlib.sha256(content).hexdigest(), len(content))
    for name, value in zip(PIN, values):
        monkeypatch.setattr(guide_sdk, name, value)


@pytest.mark.parametrize("name, value", [
    ("SOURCE", None), ("SOURCE", "pending"), ("SOURCE", "C" * 40), ("SOURCE", "c" * 39),
    ("TREE", None), ("TREE", ""),
    ("SHA256", None), ("SHA256", "PENDING"), ("SHA256", "0" * 63), ("SHA256", "B" * 64),
    ("SIZE", None), ("SIZE", 0), ("SIZE", -1), ("SIZE", True), ("SIZE", str(len(CLIENT))),
])
def test_a_partial_or_placeholder_guide_client_pin_fails_closed(monkeypatch, name, value):
    complete_pin(monkeypatch)
    assert guide_sdk.selection() == {"path": guide_sdk.WHEEL, "version": SELECTED, "source": "c" * 40,
        "tree": "d" * 40, "sha256": hashlib.sha256(CLIENT).hexdigest(), "bytes": len(CLIENT)}
    guide_sdk.verify(CLIENT)
    monkeypatch.setattr(guide_sdk, name, value)
    for use in (guide_sdk.require_pinned, guide_sdk.selection, lambda: guide_sdk.verify(CLIENT)):
        with pytest.raises(ValueError, match="pending"):
            use()


def test_an_invented_complete_pin_still_refuses_the_actual_wheel(monkeypatch):
    complete_pin(monkeypatch)
    monkeypatch.setattr(guide_sdk, "SHA256", "0" * 64)
    with pytest.raises(ValueError, match="differs from its reviewed wheel"):
        guide_sdk.verify(CLIENT)


def test_the_release_inputs_skeleton_selects_the_admitted_guide(gateway):
    guide = json.loads(SKELETON.read_text())["guide"]
    assert (guide["version"], guide["core_version"]) == (SELECTED, SELECTED)
    # The gateway and the client come from the same Meta release commit.
    assert guide["meta_source"] == guide_sdk.SOURCE
    selected = SelectedGuideRuntime(gateway, guide["version"], guide["core_version"],
        guide["meta_source"], guide["core_source"])
    assert (selected.meta_source, selected.core_source) == (guide["meta_source"], guide["core_source"])
