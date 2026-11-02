"""Meta 0.9.7 pair byte admission and inert readback; no native or published product proof."""
from dataclasses import replace

import pytest

from launchkit import runtime_candidates
from launchkit import sdk_wheel_integrity as integrity
from launchkit.templates import install_candidate as installed
from test_sdk_wheel_integrity import InertWheel, check_pair
from test_installed_sdk_wheel_proof import compare, inert_wheel, sdk_files


def selected_pair(version, *, required=None):
    return (InertWheel("meerkat-meta", version=version),
            InertWheel("meerkat-toolkit", version="0.9.5", meta_version=required or version))


@pytest.mark.parametrize("version", ["0.9.6", "0.9.7"])
def test_exact_meta_pair_keeps_both_supported_formats(version):
    pair = selected_pair(version)
    facts = check_pair(pair)
    assert tuple(f.version for f in facts) == (version, "0.9.5")
    assert tuple(f.sha256 for f in facts) == tuple(w.selected()[1].sha256 for w in pair)


@pytest.mark.parametrize(("version", "required"), [("0.9.6", "0.9.7"), ("0.9.7", "0.9.6")])
def test_cross_version_pair_refuses_before_pip(tmp_path, monkeypatch, version, required):
    pair = selected_pair(version, required=required)
    root = tmp_path / "deployment"
    root.mkdir(mode=0o700)
    selected = []
    for wheel in pair:
        data, expectation = wheel.selected()
        path = root / (wheel.package + ".whl")
        path.write_bytes(data)
        selected.append((path, expectation))
    original = {p.name: p.read_bytes() for p in root.iterdir()}

    def forbidden(*args, **kwargs):
        pytest.fail("A cross-version pair must refuse before staging or any command.")

    monkeypatch.setattr(installed, "_selected_staging", forbidden)
    monkeypatch.setattr(installed.subprocess, "run", forbidden)
    with pytest.raises(integrity.WheelIntegrityError, match="exactly the selected Meta"):
        installed.install_meta_toolkit_wheels(selected[0][0], selected[1][0],
            meta=selected[0][1], toolkit=selected[1][1], deployment_root=root,
            site_packages=root / ".venv/lib/python-fixture/site-packages",
            sdk_python=root / ".venv/bin/python", require_current=lambda: True,
            execute_command=forbidden)
    assert {p.name: p.read_bytes() for p in root.iterdir()} == original


@pytest.mark.parametrize("defect", ["relabel-096", "metadata-version", "source-hash"])
def test_meta097_admission_still_requires_exact_original_bytes(defect):
    pair = selected_pair("0.9.7")
    meta = pair[0]
    if defect == "relabel-096":
        content, expectation = InertWheel("meerkat-meta", version="0.9.6").selected()
        expectation = replace(expectation, version="0.9.7")
    else:
        if defect == "metadata-version":
            meta.metadata["METADATA"] = meta.metadata["METADATA"].replace(
                b"Version: 0.9.7", b"Version: 0.9.6")
        content, expectation = meta.selected()
        if defect == "source-hash":
            payload = list(expectation.source_payload)
            name, _, size = payload[0]
            payload[0] = (name, "f" * 64, size)
            expectation = replace(expectation, source_payload=tuple(payload))
    with pytest.raises(integrity.WheelIntegrityError):
        check_pair(pair, meta=(content, expectation))


@pytest.mark.parametrize("sdk_files", [("0.9.6", "0.9.5"), ("0.9.7", "0.9.5")], indirect=True)
def test_readback_preserves_selected_version_and_original_bytes(sdk_files):
    original = {p: p.read_bytes() for p in sdk_files.site.rglob("*") if p.is_file()}
    facts = compare(sdk_files)
    assert facts[0] is sdk_files.facts[0] and facts[1] is sdk_files.facts[1]
    assert {p: p.read_bytes() for p in sdk_files.site.rglob("*") if p.is_file()} == original
    assert sdk_files.calls == []


@pytest.mark.parametrize("sdk_files", [("0.9.7", "0.9.5")], indirect=True)
def test_inert_separate_096_environment_cannot_satisfy_097_readback(sdk_files):
    root = sdk_files.root.parent / "retained-ordinary"
    site = root / ".venv/lib/python-fixture/site-packages"
    site.mkdir(parents=True)
    python = root / ".venv/bin/python"
    python.parent.mkdir()
    pair = (inert_wheel("meerkat-meta", version="0.9.6"),
            inert_wheel("meerkat-toolkit", version="0.9.5", meta_version="0.9.6"))
    facts = integrity.check_meta_toolkit_wheels(pair[0][0], pair[1][0],
        meta=pair[0][1], toolkit=pair[1][1])
    for _, _, values in pair:
        for name, value in values.items():
            path = site / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(value)
    before = {p: p.read_bytes() for p in root.rglob("*") if p.is_file()}
    assert installed.compare_installed_meta_toolkit_wheels(*facts,
        deployment_root=root, site_packages=site, sdk_python=python) == facts
    with pytest.raises(runtime_candidates.CandidateArtifactError):
        installed.compare_installed_meta_toolkit_wheels(*sdk_files.facts,
            deployment_root=root, site_packages=site, sdk_python=python)
    assert compare(sdk_files) == sdk_files.facts
    assert {p: p.read_bytes() for p in root.rglob("*") if p.is_file()} == before
    assert sdk_files.calls == []
