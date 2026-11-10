"""Forge runs the workspace's own Meta release.

The Guide, the protected application receiver and the development environment
name this repository's Meta version, Meta and Core of one release share it,
and the release inputs and compiled agent teams take Meta from the monorepo's
own releases, never from an older separately pinned release or the standalone
repository. The Guide applies an explicit profile auth binding. Inert files
only; nothing runs.
"""
import hashlib
import json
from pathlib import Path
import tomllib

import pytest

from launchkit import bootstrap, guide_runtime, guide_sdk, public_meta_runtime, receiver_sdk
from launchkit.connect_installer_caller import ExactFile

INSTALLER = Path(__file__).resolve().parents[1]
WORKSPACE_META = tomllib.loads((INSTALLER.parent / "sdk/python/pyproject.toml").read_text())["project"]["version"]
MONOREPO_META = "https://github.com/lukacf/meerkat/releases/download/meta-v"
TRIPLES = {"linux-x86_64": "x86_64-unknown-linux-gnu", "macos-arm64": "aarch64-apple-darwin"}
RELEASE_INPUTS = ("release/forge-release-inputs.json", "release/inputs/forge-release-inputs.skeleton.json")


def test_the_guide_runs_the_workspace_meta_release():
    assert guide_sdk.SELECTED_VERSION == WORKSPACE_META
    assert guide_sdk.declared_core_version() == WORKSPACE_META


def test_the_application_receiver_runs_the_workspace_meta_release():
    assert receiver_sdk.SELECTED_VERSION == WORKSPACE_META
    assert receiver_sdk.declared_core_version() == WORKSPACE_META


def test_the_development_environment_installs_this_commits_meta_sdk():
    dependencies = tomllib.loads((INSTALLER / "pyproject.toml").read_text())["project"]["dependencies"]
    assert [item for item in dependencies if item.startswith("meerkat-meta")] == [f"meerkat-meta=={WORKSPACE_META}"]
    assert not (INSTALLER / "vendor").exists(), "no vendored Meta wheel of a published release"
    assert not (INSTALLER / "requirements-guide-sdk.txt").exists()


@pytest.mark.parametrize("name", RELEASE_INPUTS)
def test_release_inputs_take_the_guide_from_one_monorepo_meta_release(name):
    value = json.loads((INSTALLER / name).read_text())
    version = value["guide"]["version"]
    for target, entry in value["targets"].items():
        gateway = entry["files"][entry["gateway"]]
        assert gateway["url"] == f"{MONOREPO_META}{version}/meta-rpc-gateway-{version}-{TRIPLES[target]}.tar.gz"
        assert gateway["member"] == "rpc_gateway"
        assert "guide_sdk" in entry, "the Guide's client is the release's published wheel, not a vendored one"
        client = entry["files"][entry["guide_sdk"]]
        assert client["url"].startswith("https://files.pythonhosted.org/packages/")
        assert client["url"].endswith(f"/meerkat_meta-{version}-py3-none-any.whl")
    # Meta and Core of one release share its version and its release commit.
    assert value["guide"]["core_version"] == version
    assert value["guide"]["meta_source"] == value["guide"]["core_source"]


def test_compiled_agent_teams_install_the_release_the_packaged_guide_carries():
    guide = json.loads((INSTALLER / RELEASE_INPUTS[0]).read_text())["guide"]
    assert public_meta_runtime.VERSION == guide["version"]
    assert public_meta_runtime.RELEASE_URL == f"{MONOREPO_META}{public_meta_runtime.VERSION}"


@pytest.fixture
def gateway(tmp_path):
    tmp_path.chmod(0o700)
    path = tmp_path / "inert-rpc-gateway"
    path.write_bytes(b"inert test fixture, never execute\n")
    path.chmod(0o700)
    return ExactFile(path, hashlib.sha256(path.read_bytes()).hexdigest())


@pytest.mark.parametrize("binding", [
    {"realm": "global", "binding": "copilot_openai"},
    {"realm": "global", "binding": "default_openai", "profile": "work"},
])
def test_the_guide_applies_an_explicit_profile_auth_binding(gateway, binding):
    route = {"provider": "openai", "model": "gpt-5.5", "auth_binding": binding}
    selected = bootstrap.SelectedGuideRuntime(gateway, guide_sdk.SELECTED_VERSION,
        guide_sdk.declared_core_version(), "a" * 40, "a" * 40, route)
    assert selected.model_configuration == route
    lines = guide_runtime.model_profile_lines("openai", "gpt-5.5", selected.model_configuration)
    fields = ", ".join(f"{key} = {json.dumps(binding[key])}" for key in ("realm", "binding", "profile") if key in binding)
    assert lines == ['model = "gpt-5.5"', 'provider = "openai"', f"auth_binding = {{ {fields} }}"]
    parsed = tomllib.loads("[profiles.guide]\n" + "\n".join(lines) + "\n")
    assert parsed["profiles"]["guide"]["auth_binding"] == binding
