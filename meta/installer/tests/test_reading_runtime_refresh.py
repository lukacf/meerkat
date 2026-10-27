"""Accepted-design runtime refreshes, with source fixtures and no native calls."""
import copy
import hashlib
import json
import re
import tomllib
from types import SimpleNamespace

import pytest

from launchkit import compiler, generator, deployment_updates as updates
from launchkit.blueprint import starter_blueprint
from launchkit.generator import PackageFile
from launchkit.implementation_workspace import FrameworkSnapshot, ImplementationWorkspace, WorkspaceError
from launchkit.projects import fingerprint
from test_runtime_dependency_updates import native_fixture_proof


def inventory(files):
    return {item.path: {"sha256": hashlib.sha256(item.content.encode()).hexdigest(), "mode": item.mode}
            for item in files}


def digest(value):
    return hashlib.sha256(json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def previous_framework(monkeypatch, version="0.8.42"):
    blueprint = starter_blueprint().model_dump(mode="json")
    for role in blueprint["agent_classes"]:
        role.update(provider="openai", model="gpt-6-astra")
        role["tools"]["memory"] = False
    with monkeypatch.context() as old:
        old.setattr(compiler, "MOBKIT_VERSION", version)
        old.setattr(generator, "MOBKIT_VERSION", version)
        files = compiler.generate_package(blueprint)
    selected = {item.path: item for item in files if item.path != "SHA256SUMS"}
    deployment = json.loads(selected["deployment.json"].content)
    deployment.pop("applicationProfiles")
    selected["deployment.json"] = PackageFile("deployment.json", json.dumps(deployment, indent=2) + "\n")
    native = selected["config/mob.toml"].content.replace("Team:", "Team purpose:")
    selected["config/mob.toml"] = PackageFile("config/mob.toml", native)
    selected["mcp_connections.py"] = PackageFile("mcp_connections.py", "# Exact accepted MCP host without new dependency imports.\n")
    selected["requirements.txt"] = PackageFile("requirements.txt",
        selected["requirements.txt"].content.replace("jsonschema==4.26.0\n", ""))
    sums = "".join(f"{hashlib.sha256(item.content.encode()).hexdigest()}  {item.path}\n"
                   for item in sorted(selected.values(), key=lambda item: item.path))
    files = tuple([*selected.values(), PackageFile("SHA256SUMS", sums)])
    baseline = digest(inventory(files))
    binding = {"build_id": "accepted-predecessor", "source_digest": "a" * 64}
    snapshot = FrameworkSnapshot(fingerprint(blueprint), files, baseline, binding["build_id"],
                                 binding["source_digest"], binding)
    return blueprint, snapshot


def package_map(files):
    return {item.path: (item.content.encode(), 0o755 if item.mode & 0o111 else 0o644) for item in files}


def report(blueprint, framework, build_id, source):
    return {"build_id": build_id, "guide_version": 4, "blueprint_hash": fingerprint(blueprint),
            "brief_hash": "b" * 64, "unresolved": [], "verification": {"digest": source},
            "framework": framework}


def refresh_pair(tmp_path, monkeypatch, version="0.8.42"):
    blueprint, snapshot = previous_framework(monkeypatch, version)
    previous = package_map(snapshot.files)
    old_framework = {"kind": "host-framework-provenance", "version": 1,
        "blueprint_digest": fingerprint(blueprint), "baseline_digest": snapshot.baseline_digest,
        "current_compiler_baseline_digest": snapshot.baseline_digest,
        "basis": "current-compiler", "reason": "native-or-dependency-contract-changed",
        "retained_template_paths": []}
    previous["implementation-report.json"] = (json.dumps(report(blueprint, old_framework,
        snapshot.previous_build_id, snapshot.previous_source_digest)).encode(), 0o644)
    previous["application/__init__.py"] = (b"# Accepted application, never imported by these source tests.\n", 0o644)
    previous["application/configuration.json"] = (b'{"fields":[]}', 0o644)
    workspace = ImplementationWorkspace(tmp_path / "successor", blueprint)
    workspace.seed(snapshot, runtime_refresh=True)
    for name, (content, _) in previous.items():
        if name.startswith("application/"):
            workspace.write_file(name, content.decode())
    candidate = package_map(workspace.export_files(require_verified=False))
    candidate["implementation-report.json"] = (json.dumps(report(blueprint, workspace.framework_provenance(),
        "checked-successor", "c" * 64)).encode(), 0o644)
    return previous, candidate, workspace, snapshot


def background_receipt():
    return {"target": "local", "execution_mode": "background", "status": "stopped",
        "service_cleanup_confirmed": True, "service": {"manager": "launchd", "update_startup_protocol": 1,
            "update_worker_sha256": "d" * 64,
            "host_files": {"launchkit/deployment_worker.py": "d" * 64}}}


@pytest.mark.parametrize("version", ["0.8.42", "0.8.46"])
@pytest.mark.parametrize("background", [False, True])
def test_runtime_refresh_preserves_accepted_design_domain_source_and_dependency_closure(tmp_path, monkeypatch, version, background):
    previous, candidate, workspace, snapshot = refresh_pair(tmp_path, monkeypatch, version)
    receipt = background_receipt() if background else {}
    transition = updates.runtime_dependency_transition(previous, candidate, receipt)
    assert transition and (transition["from_version"], transition["to_version"], transition["core_version"]) == (version, "0.8.47", "0.8.51")
    assert transition["definition_unchanged"] is True and transition["native_registry_update"] is False
    assert updates.compatibility(previous, candidate, receipt)[1] == []
    for name in ("blueprint.json", "config/mob.toml", "config/console.toml", "config/connections.json",
                 "application/__init__.py", "application/configuration.json", "mcp_connections.py", ".env.example"):
        assert candidate[name] == previous[name]
    before, after = json.loads(previous["deployment.json"][0]), json.loads(candidate["deployment.json"][0])
    assert before.pop("mobkitVersion") == version and after.pop("mobkitVersion") == "0.8.47"
    assert before == after and "applicationProfiles" not in after
    assert b"Team purpose:" in candidate["config/mob.toml"][0]
    assert b"jsonschema" not in candidate["requirements.txt"][0]
    assert candidate["requirements.txt"][0] == previous["requirements.txt"][0].replace(
        f"meerkat-mobkit=={version}".encode(), b"meerkat-mobkit==0.8.47")
    assert workspace.verification() is None
    assert workspace.framework_provenance()["previous"] == snapshot.origin
    assert workspace.framework_provenance()["basis"] == "runtime-refresh"


@pytest.mark.parametrize("mutation", ["role", "native-grant", "profile", "blueprint", "deployment", "mcp", "application", "new-application", "other-pin", "new-pin", "mode", "provenance", "origin", "predecessor"])
def test_runtime_refresh_never_licenses_authored_or_design_drift(tmp_path, monkeypatch, mutation):
    previous, candidate, _, _ = refresh_pair(tmp_path, monkeypatch)
    if mutation in {"role", "native-grant", "profile"}:
        text = candidate["config/mob.toml"][0]
        old, new = {"role": (b"Team purpose:", b"Different role:"),
                    "native-grant": (b"memory = false", b"memory = true"),
                    "profile": (b"gpt-6-astra", b"gpt-6-luna")}[mutation]
        assert old in text
        candidate["config/mob.toml"] = (text.replace(old, new, 1), 0o644)
    elif mutation in {"blueprint", "deployment"}:
        name = mutation + ".json"
        value = json.loads(candidate[name][0]); value["unexpected"] = True
        candidate[name] = (json.dumps(value).encode(), 0o644)
    elif mutation in {"provenance", "origin", "predecessor"}:
        value = json.loads(candidate["implementation-report.json"][0])
        if mutation == "provenance": value["framework"]["baseline_digest"] = "f" * 64
        else: value["framework"]["origin" if mutation == "origin" else "previous"]["source_digest"] = "f" * 64
        candidate["implementation-report.json"] = (json.dumps(value).encode(), 0o644)
    elif mutation == "new-application": candidate["application/extra.py"] = (b"# No new app behavior during runtime refresh.\n", 0o644)
    elif mutation in {"other-pin", "new-pin"}:
        text = candidate["requirements.txt"][0]
        candidate["requirements.txt"] = ((text.replace(b"httpx==0.28.1", b"httpx==0.28.2")
            if mutation == "other-pin" else text + b"jsonschema==4.26.0\n"), 0o644)
    else:
        name = {"mcp": "mcp_connections.py", "application": "application/__init__.py", "mode": "config/mob.toml"}[mutation]
        candidate[name] = (candidate[name][0] + (b"# Changed\n" if mutation != "mode" else b""),
                           0o755 if mutation == "mode" else 0o644)
    assert updates.runtime_dependency_transition(previous, candidate, {}) is None


@pytest.mark.parametrize("mutation", ["protocol", "worker", "service", "remote"])
def test_background_runtime_refresh_needs_the_existing_owned_update_protocol(tmp_path, monkeypatch, mutation):
    previous, candidate, _, _ = refresh_pair(tmp_path, monkeypatch)
    receipt = background_receipt()
    if mutation == "protocol": receipt["service"]["update_startup_protocol"] = True
    elif mutation == "worker": receipt["service"]["update_worker_sha256"] = "f" * 64
    elif mutation == "service": receipt["service"] = None
    else: receipt["target"] = "gcp"
    assert updates.runtime_dependency_transition(previous, candidate, receipt) is None


def test_runtime_refresh_requires_exact_snapshot_and_rejects_legacy_registry(tmp_path, monkeypatch):
    blueprint, snapshot = previous_framework(monkeypatch)
    workspace = ImplementationWorkspace(tmp_path / "candidate", blueprint)
    with pytest.raises(WorkspaceError, match="exact accepted"):
        workspace.seed(runtime_refresh=True)
    files = list(snapshot.files)
    files = [PackageFile(item.path, item.content + '\n[models."gpt-6.1-sol"]\nprovider="openai"\n', item.mode)
             if item.path == "config/mob.toml" else item for item in files]
    with pytest.raises(ValueError, match="registry"):
        compiler.generate_runtime_refresh(blueprint, files)


def test_runtime_refresh_refuses_an_old_pin_substring_outside_the_unique_sdk_line(monkeypatch):
    blueprint, snapshot = previous_framework(monkeypatch)
    files = [PackageFile(item.path, item.content + "# Keep meerkat-mobkit==0.8.42 as historical text\n", item.mode)
             if item.path == "requirements.txt" else item for item in snapshot.files]
    with pytest.raises(ValueError, match="unique exact"):
        compiler.generate_runtime_refresh(blueprint, files)


def test_runtime_refresh_typed_blueprint_equality_never_accepts_boolean_as_integer(monkeypatch):
    blueprint, snapshot = previous_framework(monkeypatch)
    files = []
    for item in snapshot.files:
        if item.path == "blueprint.json":
            changed = json.loads(item.content)
            changed["agent_classes"][0]["instances"] = True
            item = PackageFile(item.path, json.dumps(changed), item.mode)
        files.append(item)
    with pytest.raises(ValueError, match="identical accepted"):
        compiler.generate_runtime_refresh(blueprint, files)
