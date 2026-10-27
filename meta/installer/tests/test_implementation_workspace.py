import hashlib
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from dataclasses import replace
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_package
from launchkit.generator import PackageFile
from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError
import launchkit.implementation_workspace as implementation


def test_linux_runtime_keeps_shared_library_loader_name_without_exposing_its_directory(tmp_path, monkeypatch):
    from launchkit import linux_sandbox
    library_directory = tmp_path / "private-runtime" / "lib"
    library_directory.mkdir(parents=True)
    actual = library_directory / "libpython3.12.so.1.0"
    actual.write_bytes(b"inert shared-library fixture, never executed")
    alias = library_directory / "libpython3.12.so"
    alias.symlink_to(actual.name)
    (library_directory / "private-config").write_bytes(b"must not be mounted")
    values = {"LDLIBRARY": alias.name, "LIBDIR": str(library_directory), "MULTIARCH": None}
    monkeypatch.setattr(linux_sandbox.sysconfig, "get_config_var", values.get)
    reads = set(linux_sandbox._runtime_reads())
    assert str(alias) in reads
    assert str(actual) in reads, "The ELF loader uses the resolved library's versioned filename."
    assert str(library_directory) not in reads


@pytest.fixture
def workspace(tmp_path):
    result = ImplementationWorkspace(tmp_path / "implementation", starter_blueprint())
    result.seed()
    return result


def authored_feature(workspace, broken=False):
    workspace.write_file("application/__init__.py", "class Integration:\n    pass\n\ndef build(context):\n    return Integration()\n")
    workspace.write_file("application/feature.py", "def normalize(value):\n    return value.strip()" + ("\n" if broken else ".lower()\n"))
    workspace.write_file("tests/test_feature.py", """import unittest
from application.feature import normalize

class NormalizeTests(unittest.TestCase):
    def test_normalizes_mixed_case_whitespace(self):
        self.assertEqual(normalize("  Hello, MOBKIT  "), "hello, mobkit")

    def test_preserves_the_empty_case(self):
        self.assertEqual(normalize("  "), "")
""")


def require_sandbox():
    if not implementation._sandbox_executable():
        pytest.skip("Executable checks deliberately have no unsandboxed fallback.")


def test_linux_missing_seccomp_refuses_before_launch(tmp_path, monkeypatch):
    from launchkit import linux_sandbox
    monkeypatch.setattr(implementation.sys, "platform", "linux")
    def missing_library(*args, **kwargs):
        raise OSError("Missing fixed libseccomp prerequisite")
    monkeypatch.setattr(linux_sandbox.ctypes, "CDLL", missing_library)
    def forbidden_launch(*args, **kwargs):
        pytest.fail("Missing mandatory isolation must not launch a child")
    monkeypatch.setattr(implementation.subprocess, "Popen", forbidden_launch)
    project, scratch = tmp_path / "project", tmp_path / "scratch"
    project.mkdir()
    scratch.mkdir()
    with pytest.raises(WorkspaceError, match="libseccomp"):
        implementation._run_sandboxed(
            ["/usr/bin/python3", "-I", "-S", "-c", "print('must not execute')"], "/usr/bin/bwrap",
            implementation._sandbox_profile(project, scratch, tmp_path), project,
            implementation._check_environment(scratch))


@pytest.mark.skipif(sys.platform != "linux", reason="Linux kernel sealing contract")
def test_linux_seccomp_seals_work_without_python_constant_exports(monkeypatch):
    import errno
    import fcntl
    from launchkit import linux_sandbox
    for name in ("F_ADD_SEALS", "F_GET_SEALS", "F_SEAL_SEAL", "F_SEAL_SHRINK",
                 "F_SEAL_GROW", "F_SEAL_WRITE"):
        monkeypatch.delattr(fcntl, name, raising=False)
    descriptor = linux_sandbox._filter()
    try:
        assert fcntl.fcntl(descriptor, 1034) & 0x000f == 0x000f
        size = os.fstat(descriptor).st_size
        assert size > 0 and os.read(descriptor, size)
        for mutate in (lambda: os.write(descriptor, b"changed"),
                       lambda: os.ftruncate(descriptor, 0),
                       lambda: os.ftruncate(descriptor, size + 1)):
            with pytest.raises(OSError) as caught:
                mutate()
            assert caught.value.errno == errno.EPERM
    finally:
        os.close(descriptor)


@pytest.mark.skipif(sys.platform != "linux", reason="Linux kernel sealing contract")
@pytest.mark.parametrize("failure", ["add_refused", "readback_missing"])
def test_linux_seccomp_seal_failure_closes_the_filter(monkeypatch, failure):
    import errno
    import fcntl
    from launchkit import linux_sandbox
    original = fcntl.fcntl
    add_seals = getattr(fcntl, "F_ADD_SEALS", 1033)
    get_seals = getattr(fcntl, "F_GET_SEALS", 1034)
    descriptors = []
    def observe(descriptor, command, *arguments):
        if command == add_seals:
            descriptors.append(descriptor)
            if failure == "add_refused":
                raise OSError(errno.EPERM, "Kernel refused the required seals")
        result = original(descriptor, command, *arguments)
        return 0 if command == get_seals and failure == "readback_missing" else result
    monkeypatch.setattr(fcntl, "fcntl", observe)
    with pytest.raises(linux_sandbox.LinuxSandboxError):
        linux_sandbox._filter()
    assert len(descriptors) == 1
    with pytest.raises(OSError) as caught:
        os.fstat(descriptors[0])
    assert caught.value.errno == errno.EBADF


def test_authored_dom_suite_is_executable_signed_source_bound_and_whole_suite(workspace):
    require_sandbox()
    workspace.write_file("assets/page.html", '<button id="increment">Add</button><output id="count">0</output><script>document.querySelector("button").onclick=()=>document.querySelector("output").textContent="1";</script>')
    workspace.write_file("tests/test_page.dom.cjs", '''const fs=require("node:fs"),path=require("node:path");
module.exports=({test,createDOM,assert,sourceRoot})=>test("click production control",()=>{
 const dom=createDOM(fs.readFileSync(path.join(sourceRoot,"assets/page.html"),"utf8"));
 dom.window.document.querySelector("button").click();
 assert.equal(dom.window.document.querySelector("output").textContent,"1");
});''')
    workspace.write_file("tests/test_python.py", "import unittest\nclass Contract(unittest.TestCase):\n def test_preserved(self): self.assertTrue(True)\n")
    subset = workspace.check(["tests/test_page.dom.cjs"])
    assert subset["status"] == "pass" and subset["tests_run"] == 1, failed_checks(subset)
    assert not subset["complete_test_suite"] and not subset["ready_to_package"]
    with pytest.raises(WorkspaceError, match="passing executable checks"):
        workspace.export_files()
    checked = workspace.check()
    assert checked["status"] == "pass" and checked["tests_run"] == 2 and checked["ready_to_package"], checked
    assert checked == workspace.verification()
    assert any(row["name"] == "authored DOM tests" and "not browser form encoding" in row["detail"] for row in checked["checks"])
    workspace.write_file("assets/page.html", '<button id="increment">Add</button><output>0</output>')
    assert workspace.verification() is None
    failed = workspace.check()
    assert failed["status"] == "fail" and not failed["ready_to_package"]


def test_dom_failure_cannot_be_hidden_by_python_pass(workspace):
    require_sandbox()
    workspace.write_file("tests/test_python.py", "import unittest\nclass Contract(unittest.TestCase):\n def test_ok(self): self.assertTrue(True)\n")
    workspace.write_file("tests/test_page.dom.cjs", "module.exports=({test,assert})=>test('actual failure',()=>assert.equal(1,2));")
    checked = workspace.check()
    assert checked["status"] == "fail" and checked["tests_run"] == 2 and not checked["ready_to_package"], checked


def test_requested_dom_dependency_unavailable_has_no_execution_fallback(workspace, monkeypatch):
    require_sandbox()
    workspace.write_file("tests/test_page.dom.cjs", "module.exports=({test})=>test('ok',()=>{});")
    import launchkit.dom_check as dom_check
    monkeypatch.setattr(dom_check,"prepare_dependencies",lambda *args: (_ for _ in ()).throw(dom_check.DOMUnavailable("Pinned dependencies unavailable.")))
    checked = workspace.check()
    assert checked["status"] == "unavailable" and not checked["ready_to_package"]
    assert next(row for row in checked["checks"] if row["name"] == "authored DOM tests")["tests_run"] == 0


def test_private_checker_syntax_uses_exact_read_grant_without_source_write(workspace, monkeypatch):
    require_sandbox()
    from launchkit import checker_runtime
    root = Path(os.environ.get("MOBKIT_TEST_CHECKER_ROOT", Path(__file__).resolve().parents[1]))
    if checker_runtime.prepared_node(root) is None:
        pytest.skip("Explicitly prepare a private checker; checks do not download it.")
    monkeypatch.setattr(implementation, "resolve_node", lambda configured: checker_runtime.resolve_node(configured, root=root))
    authored_feature(workspace)
    workspace.write_file("assets/client.mjs", "const unfinished = ;\n")
    failed = workspace.check()
    assert "SyntaxError" in failed_checks(failed)["node syntax: assets/client.mjs"]
    assert not failed["ready_to_package"]
    workspace.write_file("assets/client.mjs", "export const accepted = true;\n")
    checked = workspace.check()
    assert checked["ready_to_package"], failed_checks(checked)
    assert workspace.read_file("assets/client.mjs") == "export const accepted = true;\n"


def failed_checks(result):
    return {item["name"]: item["detail"] for item in result["checks"] if item["status"] != "pass"}


def test_seed_matches_compiler_and_preserves_additive_files_on_reopen(workspace):
    expected = {item.path: item.content for item in generate_package(starter_blueprint())}
    assert {row["path"] for row in workspace.list_files()} == set(expected)
    assert all(workspace.read_file(path) == content for path, content in expected.items())
    workspace.write_file("application/routing.py", "CHANNELS = ['web', 'telegram']\n")
    before = workspace.digest()
    reopened = ImplementationWorkspace(workspace.root, starter_blueprint())
    reopened.seed()
    assert reopened.digest() == before
    assert reopened.read_file("application/routing.py") == "CHANNELS = ['web', 'telegram']\n"
    state = json.loads(reopened._state_path.read_text())
    assert state["changes"][-1]["path"] == "application/routing.py"
    assert state["changes"][-1]["after"] == hashlib.sha256(b"CHANNELS = ['web', 'telegram']\n").hexdigest()


def accepted_framework_snapshot(workspace, build_id="prior-build"):
    # The service selects actual independent acceptance. Workspace tests exercise
    # only its host snapshot boundary, never create a readiness grant.
    with workspace._locked():
        stored = workspace._state()
        files = workspace._files(stored)
        return workspace._framework_snapshot(stored, files, build_id=build_id,
                                             accepted_source_digest=workspace._digest(files))


def compiler_change(monkeypatch, paths):
    original = implementation.generate_package
    monkeypatch.setattr(implementation, "generate_package", lambda blueprint: [
        PackageFile(item.path, item.content + "\n# New compiler template.\n", item.mode)
        if item.path in paths else item for item in original(blueprint)])


def test_retained_framework_executes_current_checks_without_inheriting_receipts(workspace, tmp_path, monkeypatch):
    require_sandbox()
    authored_feature(workspace)
    previous_check = workspace.check()
    assert previous_check["status"] == "pass", previous_check
    snapshot = accepted_framework_snapshot(workspace)
    old_files = {item.path: item for item in snapshot.files}
    compiler_change(monkeypatch, {"app.py", "connectors.py", "remote_operations.py", "setup.py"})
    repaired = ImplementationWorkspace(tmp_path / "repair", starter_blueprint())
    repaired.seed(snapshot)
    for path, item in old_files.items():
        assert repaired.read_file(path) == item.content
        assert (repaired.project / path).stat().st_mode & 0o777 == item.mode
    assert repaired.verification() is None
    assert repaired._key_path.read_bytes() != workspace._key_path.read_bytes()
    provenance = repaired.framework_provenance()
    assert "signature" not in provenance
    assert provenance["basis"] == "accepted-baseline"
    assert provenance["origin"]["source_digest"] == previous_check["digest"]
    assert provenance["previous"]["source_digest"] == workspace.digest()
    assert provenance["retained_template_paths"] == ["app.py", "connectors.py", "remote_operations.py", "setup.py"]
    assert provenance["current_compiler_baseline_digest"] != provenance["baseline_digest"]
    assert "no current-template upgrade" in provenance["scope"]
    authored_feature(repaired)
    repaired.write_file("application/repair.txt", "A new source change.\n")
    checked = repaired.check()
    assert checked["status"] == "pass" and checked["tests_run"] == 2, checked
    assert checked["digest"] == repaired.digest() != previous_check["digest"]
    assert checked["signature"] != previous_check["signature"]
    exported = {item.path: item for item in repaired.export_files()}
    for line in exported["SHA256SUMS"].content.splitlines():
        sha, path = line.split("  ", 1)
        assert hashlib.sha256(exported[path].content.encode()).hexdigest() == sha
    with pytest.raises(WorkspaceError, match="read-only"):
        repaired.write_file("app.py", "pass\n")


def test_retained_framework_rejects_changed_snapshot_and_forged_provenance(workspace, tmp_path):
    snapshot = accepted_framework_snapshot(workspace)
    tampered = replace(snapshot, files=tuple(PackageFile(item.path, "pass\n", item.mode)
                       if item.path == "app.py" else item for item in snapshot.files))
    with pytest.raises(WorkspaceError, match="snapshot changed"):
        ImplementationWorkspace(tmp_path / "tampered", starter_blueprint()).seed(tampered)
    repaired = ImplementationWorkspace(tmp_path / "repair", starter_blueprint())
    repaired.seed(snapshot)
    state = json.loads(repaired._state_path.read_text())
    state["framework"]["origin"]["source_digest"] = "b" * 64
    repaired._save_state(state)
    with pytest.raises(WorkspaceError, match="framework provenance"):
        repaired.seed()
    with pytest.raises(WorkspaceError, match="framework provenance"):
        repaired.export_files(require_verified=False)


@pytest.mark.parametrize("path", ["requirements.txt", "config/mob.toml", "deployment.json", ".env.example"])
def test_compiler_contract_drift_retains_accepted_framework_until_explicit_refresh(workspace, tmp_path, monkeypatch, path):
    snapshot = accepted_framework_snapshot(workspace)
    compiler_change(monkeypatch, {"app.py", path})
    repaired = ImplementationWorkspace(tmp_path / "repair", starter_blueprint())
    repaired.seed(snapshot)
    for item in snapshot.files:
        assert repaired.read_file(item.path) == item.content
        assert (repaired.project / item.path).stat().st_mode & 0o777 == item.mode
    provenance = repaired.framework_provenance()
    assert provenance["basis"] == "accepted-baseline" and provenance["origin"] == snapshot.origin
    assert provenance["reason"] == "unchanged-specification"
    assert provenance["changed_contract_paths"] == [path]
    assert provenance["retained_template_paths"] == sorted(["app.py", path])
    assert provenance["baseline_digest"] != provenance["current_compiler_baseline_digest"]
    assert repaired.verification() is None
    assert repaired._key_path.read_bytes() != workspace._key_path.read_bytes()
    refreshed = ImplementationWorkspace(tmp_path / "refresh", starter_blueprint())
    refreshed.seed(fresh_reason="explicit-framework-refresh")
    assert refreshed.read_file("app.py") != workspace.read_file("app.py")
    assert refreshed.read_file(path) != workspace.read_file(path)
    current = refreshed.framework_provenance()
    assert current["basis"] == "current-compiler" and current["origin"] is None
    assert current["reason"] == "explicit-framework-refresh"
    assert refreshed.verification() is None


def test_retained_framework_needs_identical_blueprint_and_host_accepted_origin(workspace, tmp_path):
    snapshot = accepted_framework_snapshot(workspace)
    changed = starter_blueprint().model_dump(mode="json")
    changed["project"]["description"] += " New deployment needs."
    with pytest.raises(WorkspaceError, match="identical frozen Blueprint"):
        ImplementationWorkspace(tmp_path / "changed", changed).seed(snapshot)
    with workspace._locked():
        state = workspace._state()
        assert workspace._framework_snapshot(state, workspace._files(state), build_id="unaccepted") is None
    (workspace.project / "app.py").write_text("pass\n")
    with pytest.raises(WorkspaceError, match="changed outside the host"):
        accepted_framework_snapshot(workspace)


@pytest.mark.parametrize("path", ["../outside.py", "/tmp/outside.py", "./file.py", "a/../file.py", "a//file.py", "a\\file.py", "file\x00.py", "a\nfile.py", ".env", "knowledge/.env", ".mobkit/state.json", ".git/config", "a/.private/file", "node_modules/a.js"])
def test_paths_cannot_escape_or_access_private_storage(workspace, path):
    with pytest.raises(WorkspaceError):
        workspace.write_file(path, "sensitive = False\n")
    with pytest.raises(WorkspaceError):
        workspace.read_file(path)


@pytest.mark.parametrize("path", ["app.py", "setup.py", "install.sh", "deployment.json", "blueprint.json", "config/mob.toml", "config/connections.json", "config/console.toml", "deploy/deploy-gcp.sh", "SHA256SUMS", ".env.example"])
def test_seeded_framework_and_deployment_files_cannot_be_overwritten(workspace, path):
    before = workspace.read_file(path)
    with pytest.raises(WorkspaceError, match="read-only"):
        workspace.write_file(path, "agent claimed completion\n")
    assert workspace.read_file(path) == before


def test_symlinks_and_hard_links_cannot_import_user_files(workspace, tmp_path):
    outside = tmp_path / "private-key"
    outside.write_text("DO NOT READ")
    (workspace.project / "linked.py").symlink_to(outside)
    with pytest.raises(WorkspaceError, match="symlinks"):
        workspace.read_file("linked.py")
    with pytest.raises(WorkspaceError):
        workspace.list_files()
    (workspace.project / "linked.py").unlink()
    (workspace.project / "application").symlink_to(tmp_path, target_is_directory=True)
    with pytest.raises(WorkspaceError):
        workspace.write_file("application/exploit.py", "pass\n")
    (workspace.project / "application").unlink()
    os.link(outside, workspace.project / "hardlink.py")
    with pytest.raises(WorkspaceError, match="hard links"):
        workspace.read_file("hardlink.py")
    assert outside.read_text() == "DO NOT READ"


def test_reopening_refuses_a_different_frozen_blueprint(workspace):
    changed = starter_blueprint().model_dump()
    changed["project"]["description"] += " Different desired deployment."
    with pytest.raises(WorkspaceError, match="different frozen Blueprint"):
        ImplementationWorkspace(workspace.root, changed).seed()


@pytest.mark.parametrize("mode", ["remote", "local"])
def test_optional_elephant_packages_keep_public_nested_gitignore_but_no_private_environment(tmp_path, mode):
    blueprint = starter_blueprint().model_dump()
    blueprint["knowledge"]["mode"] = mode
    result = ImplementationWorkspace(tmp_path / mode, blueprint)
    result.seed()
    assert "knowledge/manage.py" in {row["path"] for row in result.list_files()}
    assert ".env" in result.read_file("knowledge/.gitignore")
    with pytest.raises(WorkspaceError):
        result.read_file("knowledge/.env")
    with pytest.raises(WorkspaceError, match="read-only"):
        result.write_file("knowledge/.gitignore", "include credentials\n")


def test_direct_framework_tampering_is_detected_before_export(workspace):
    (workspace.project / "deployment.json").write_text('{"verified":true}')
    with pytest.raises(WorkspaceError, match="changed outside the host"):
        workspace.check()
    with pytest.raises(WorkspaceError):
        workspace.export_files(require_verified=False)


def test_host_manifest_covers_custom_assets_without_allowing_agent_edits(workspace):
    workspace.write_file("assets/user-interface.html", "<!doctype html><title>Family inbox</title>\n")
    files = {item.path: item.content for item in workspace.export_files(require_verified=False)}
    checksums = dict(line.split("  ", 1)[::-1] for line in files["SHA256SUMS"].splitlines())
    assert set(checksums) == set(files) - {"SHA256SUMS"}
    assert all(checksums[path] == hashlib.sha256(files[path].encode()).hexdigest() for path in checksums)
    manifest = next(row for row in workspace.list_files() if row["path"] == "SHA256SUMS")
    assert manifest["derived"] and not manifest["modified"]
    assert workspace.read_file("SHA256SUMS") == files["SHA256SUMS"]


def test_exact_edit_preserves_mode_provenance_and_invalidates_real_checks(workspace, monkeypatch):
    authored_feature(workspace)
    workspace.write_file("application/helper.sh", "#!/bin/sh\nprintf 'original'\n")
    monkeypatch.setattr(implementation, "_sandbox_executable", lambda: None)
    receipt = workspace.check()
    assert workspace.verification() == receipt
    prior = workspace.digest()
    provenance = workspace.framework_provenance()
    before = workspace.read_file("application/helper.sh")
    result = workspace.edit_file("application/helper.sh", hashlib.sha256(before.encode()).hexdigest(),
                                 "'original'", "'repaired'\n# Unicode: \u00e9")
    assert workspace.read_file("application/helper.sh") == "#!/bin/sh\nprintf 'repaired'\n# Unicode: \u00e9\n"
    assert (workspace.project / "application/helper.sh").stat().st_mode & 0o777 == 0o755
    assert result["digest"] == workspace.digest() != prior
    assert result["sha256"] == hashlib.sha256(workspace.read_file(result["path"]).encode()).hexdigest()
    assert workspace.verification() is None
    assert workspace.framework_provenance() == provenance
    state = json.loads(workspace._state_path.read_text())
    assert state["last_check"] is None
    assert state["changes"][-1]["before"] == hashlib.sha256(before.encode()).hexdigest()
    assert state["changes"][-1]["after"] == result["sha256"]
    assert result["sha256"] + "  application/helper.sh\n" in workspace.read_file("SHA256SUMS")


@pytest.mark.parametrize("case", ["stale", "bad-hash", "missing", "ambiguous", "overlapping", "empty", "noop", "invalid-utf8", "nul", "oversize", "absent"])
def test_rejected_edits_preserve_source_and_authentic_check_receipt(workspace, monkeypatch, case):
    workspace.write_file("application/edit.txt", "alpha\nrepeat repeat\naaa\n")
    monkeypatch.setattr(implementation, "_sandbox_executable", lambda: None)
    receipt = workspace.check()
    before = (workspace.project / "application/edit.txt").read_bytes()
    state = workspace._state_path.read_bytes()
    key = workspace._key_path.read_bytes()
    sha = hashlib.sha256(before).hexdigest()
    path, old, new = "application/edit.txt", "alpha", "changed"
    if case == "stale": sha = "0" * 64
    if case == "bad-hash": sha = True
    if case == "missing": old = "not present"
    if case == "ambiguous": old = "repeat"
    if case == "overlapping": old = "aa"
    if case == "empty": old = ""
    if case == "noop": new = old
    if case == "invalid-utf8": new = "\ud800"
    if case == "nul": new = "not\x00text"
    if case == "oversize": new = "a" * (implementation.MAX_FILE_BYTES + 1)
    if case == "absent": path = "application/absent.txt"
    with pytest.raises(WorkspaceError):
        workspace.edit_file(path, sha, old, new)
    assert (workspace.project / "application/edit.txt").read_bytes() == before
    assert workspace._state_path.read_bytes() == state
    assert workspace._key_path.read_bytes() == key
    assert not (workspace.project / "application/absent.txt").exists()
    assert workspace.verification() == receipt


@pytest.mark.parametrize("path", ["app.py", "config/console.toml", "SHA256SUMS", "acceptance-report.json", "acceptance-tests/test_fake.py", "helper.py", "../outside.txt", ".env"])
def test_exact_edits_cannot_bypass_protected_paths(workspace, path):
    state = workspace._state_path.read_bytes()
    files = workspace.export_files(require_verified=False)
    with pytest.raises(WorkspaceError):
        workspace.edit_file(path, "0" * 64, "old", "new")
    assert workspace._state_path.read_bytes() == state
    assert workspace.export_files(require_verified=False) == files


def test_exact_edit_rejects_links_and_total_size_without_writes(workspace, monkeypatch, tmp_path):
    workspace.write_file("application/edit.txt", "old")
    state = workspace._state_path.read_bytes()
    source = workspace.project / "application/edit.txt"
    external = tmp_path / "external"
    external.write_text("old")
    for make_link in (lambda: source.symlink_to(external), lambda: os.link(external, source)):
        source.unlink(); make_link()
        with pytest.raises(WorkspaceError):
            workspace.edit_file("application/edit.txt", hashlib.sha256(b"old").hexdigest(), "old", "new")
        assert external.read_bytes() == b"old"
        assert workspace._state_path.read_bytes() == state
    source.unlink(); source.write_text("old"); source.chmod(0o644)
    monkeypatch.setattr(implementation, "_sandbox_executable", lambda: None)
    receipt = workspace.check()
    state = workspace._state_path.read_bytes()
    total = sum(p.stat().st_size for p in workspace.project.rglob("*") if p.is_file())
    monkeypatch.setattr(implementation, "MAX_PROJECT_BYTES", total + 10)
    with pytest.raises(WorkspaceError, match="total size"):
        workspace.edit_file("application/edit.txt", hashlib.sha256(b"old").hexdigest(), "old", "new" * 100)
    assert source.read_bytes() == b"old"
    assert workspace._state_path.read_bytes() == state
    assert workspace.verification() == receipt
    # Full writes use the same preflight boundary, not a separate bypass.
    with pytest.raises(WorkspaceError, match="total size"):
        workspace.write_file("application/edit.txt", "new" * 100)
    assert source.read_bytes() == b"old"
    assert workspace._state_path.read_bytes() == state
    assert workspace.verification() == receipt


def test_edit_size_limit_counts_unchanged_raw_crlf_bytes(workspace, monkeypatch):
    workspace.write_file("application/unchanged.txt", "line\r\n" * 100)
    workspace.write_file("application/edit.txt", "old")
    unchanged = (workspace.project / "application/unchanged.txt").read_bytes()
    source = workspace.project / "application/edit.txt"
    state = workspace._state_path.read_bytes()
    total = sum(p.stat().st_size for p in workspace.project.rglob("*") if p.is_file())
    monkeypatch.setattr(implementation, "MAX_PROJECT_BYTES", total + 5)
    with pytest.raises(WorkspaceError, match="total size"):
        workspace.edit_file("application/edit.txt", hashlib.sha256(b"old").hexdigest(), "old", "x" * 10)
    assert source.read_bytes() == b"old"
    assert (workspace.project / "application/unchanged.txt").read_bytes() == unchanged
    assert workspace._state_path.read_bytes() == state


def test_edit_uses_one_lock_and_concurrent_stale_hash_cannot_overwrite(workspace, monkeypatch):
    workspace.write_file("application/edit.txt", "original")
    original_lock = workspace._locked
    active = False
    @contextmanager
    def guarded_lock():
        nonlocal active
        assert not active, "Source editing must not reacquire its workspace lock."
        with original_lock():
            active = True
            try: yield
            finally: active = False
    with monkeypatch.context() as scoped:
        scoped.setattr(workspace, "_locked", guarded_lock)
        workspace.edit_file("application/edit.txt", hashlib.sha256(b"original").hexdigest(), "original", "next")
    barrier = threading.Barrier(2)
    sha = hashlib.sha256(b"next").hexdigest()
    def edit(value):
        barrier.wait(timeout=5)
        try: return workspace.edit_file("application/edit.txt", sha, "next", value)
        except WorkspaceError as error: return str(error)
    with ThreadPoolExecutor(max_workers=2) as pool:
        outcomes = list(pool.map(edit, ["first", "second"]))
    assert sum(isinstance(outcome, dict) for outcome in outcomes) == 1
    assert "changed" in next(outcome for outcome in outcomes if isinstance(outcome, str))
    assert workspace.read_file("application/edit.txt") in {"first", "second"}


def test_edit_precondition_matches_advertised_crlf_source_text_hash(workspace):
    workspace.write_file("application/edit.txt", "before\r\nafter\r\n")
    content = workspace.read_file("application/edit.txt")
    assert content == "before\nafter\n"
    metadata = next(row for row in workspace.list_files() if row["path"] == "application/edit.txt")
    assert metadata["sha256"] == hashlib.sha256(content.encode()).hexdigest()
    assert metadata["sha256"] != hashlib.sha256((workspace.project / "application/edit.txt").read_bytes()).hexdigest()
    workspace.edit_file("application/edit.txt", metadata["sha256"], "before\n", "changed\n")
    assert (workspace.project / "application/edit.txt").read_bytes() == b"changed\nafter\n"


def test_exact_edit_repairs_behavior_only_after_a_fresh_host_check(workspace):
    require_sandbox()
    authored_feature(workspace, broken=True)
    failed = workspace.check()
    assert failed["status"] == "fail" and failed["tests_run"] == 2
    current = next(row for row in workspace.list_files() if row["path"] == "application/feature.py")
    result = workspace.edit_file(current["path"], current["sha256"], "value.strip()", "value.strip().lower()")
    assert workspace.verification() is None
    with pytest.raises(WorkspaceError):
        workspace.export_files()
    checked = workspace.check()
    assert checked["status"] == "pass" and checked["tests_run"] == 2, checked
    assert checked["id"] != failed["id"]
    assert checked["digest"] == result["digest"] == workspace._digest(workspace.export_files())


def test_missing_sandbox_never_executes_generated_code_and_blocks_packaging(workspace, monkeypatch):
    authored_feature(workspace)
    monkeypatch.setattr(implementation, "_sandbox_executable", lambda: None)
    def unexpected_execution(*args, **kwargs):
        raise AssertionError("Generated code must not run without an OS sandbox.")
    monkeypatch.setattr(implementation.subprocess, "Popen", unexpected_execution)
    result = workspace.check()
    assert result["status"] == "unavailable"
    assert result["tests_run"] == 0
    assert not result["ready_to_package"]
    assert workspace.verification() == result
    with pytest.raises(WorkspaceError):
        workspace.export_files()


def test_python_syntax_is_checked_without_executing_top_level_code(workspace, monkeypatch, tmp_path):
    marker = tmp_path / "must-not-exist"
    workspace.write_file("application/danger.py", f"from pathlib import Path\nPath({str(marker)!r}).write_text('escaped')\ninvalid syntax here\n")
    monkeypatch.setattr(implementation, "_sandbox_executable", lambda: None)
    result = workspace.check()
    assert result["status"] == "fail"
    assert "python syntax: application/danger.py" in failed_checks(result)
    assert not marker.exists()


def test_failed_feature_requires_repair_and_a_fresh_host_check(workspace):
    require_sandbox()
    authored_feature(workspace, broken=True)
    failed = workspace.check()
    assert failed["status"] == "fail", failed_checks(failed)
    assert failed["tests_run"] == 2
    assert "authored tests" in failed_checks(failed)
    assert not failed["ready_to_package"]
    with pytest.raises(WorkspaceError):
        workspace.export_files()
    workspace.write_file("application/feature.py", "def normalize(value):\n    return value.strip().lower()\n")
    assert workspace.verification() is None
    passed = workspace.check()
    assert passed["status"] == "pass", failed_checks(passed)
    assert passed["tests_run"] == 2 and passed["ready_to_package"]
    assert passed["digest"] != failed["digest"] and passed["id"] != failed["id"]
    assert workspace._digest(workspace.export_files()) == passed["digest"]
    assert ImplementationWorkspace(workspace.root, starter_blueprint()).verification() == passed
    workspace.write_file("assets/index.html", "<p>New user UI</p>\n")
    assert workspace.verification() is None
    with pytest.raises(WorkspaceError):
        workspace.export_files()


def test_direct_custom_edits_and_modes_also_invalidate_receipts(workspace):
    require_sandbox()
    authored_feature(workspace)
    assert workspace.check()["ready_to_package"]
    feature = workspace.project / "application/feature.py"
    feature.write_text(feature.read_text() + "\n# changed without a tool\n")
    assert workspace.verification() is None
    assert workspace.check()["ready_to_package"]
    feature.chmod(0o755)
    assert workspace.verification() is None


def test_agent_cannot_forge_a_passing_receipt_by_editing_a_manifest(workspace):
    require_sandbox()
    authored_feature(workspace)
    assert workspace.check()["ready_to_package"]
    with pytest.raises(WorkspaceError):
        workspace.write_file("../workspace-state.json", '{"ready_to_package":true}')
    state = json.loads(workspace._state_path.read_text())
    state["last_check"]["tests_run"] = 999
    workspace._state_path.write_text(json.dumps(state))
    assert workspace.verification() is None
    with pytest.raises(WorkspaceError):
        workspace.export_files()


def test_tests_selector_cannot_execute_a_command_or_skip_failing_tests_for_publish(workspace):
    require_sandbox()
    authored_feature(workspace)
    workspace.write_file("tests/test_other.py", "import unittest\nclass Other(unittest.TestCase):\n    def test_real_requirement(self):\n        self.fail('Still unimplemented')\n")
    for selected in ["application/feature.py", "../../outside.py", "tests/no-such-test.py", "bash -c true", []]:
        with pytest.raises(WorkspaceError):
            workspace.check(selected)
    partial = workspace.check(["tests/test_feature.py"])
    assert partial["status"] == "pass", failed_checks(partial)
    assert partial["tests_run"] == 2
    assert not partial["complete_test_suite"] and not partial["ready_to_package"]
    with pytest.raises(WorkspaceError):
        workspace.export_files()
    complete = workspace.check()
    assert complete["status"] == "fail" and complete["tests_run"] == 3


def test_zero_and_all_skipped_tests_do_not_certify_a_project(workspace):
    require_sandbox()
    workspace.write_file("tests/test_empty.py", "import unittest\n")
    result = workspace.check()
    assert result["tests_run"] == 0 and not result["ready_to_package"]
    workspace.write_file("tests/test_empty.py", "import unittest\nclass NotReady(unittest.TestCase):\n    @unittest.skip('Not implemented')\n    def test_feature(self):\n        pass\n")
    result = workspace.check()
    assert result["tests_run"] == 1 and result["status"] == "fail"
    assert not result["ready_to_package"]


def test_sandbox_denies_host_credentials_network_and_source_mutation(workspace, tmp_path, monkeypatch):
    require_sandbox()
    secret = tmp_path / "host-secret"
    secret.write_text("private credential")
    monkeypatch.setenv("OPENAI_API_KEY", "must-not-reach-generated-code")
    monkeypatch.setenv("BASH_ENV", str(secret))
    monkeypatch.setenv("NODE_OPTIONS", "--require=" + str(secret))
    workspace.write_file("tests/test_boundary.py", f"""import os, pathlib, socket, unittest
class BoundaryTests(unittest.TestCase):
    def test_environment_has_no_provider_credentials_or_hooks(self):
        for name in ('OPENAI_API_KEY', 'BASH_ENV', 'NODE_OPTIONS'):
            self.assertNotIn(name, os.environ)
    def test_outside_user_file_cannot_be_read(self):
        with self.assertRaises(OSError):
            pathlib.Path({str(secret)!r}).read_text()
    def test_network_is_denied(self):
        with self.assertRaises(OSError):
            socket.socket().connect(('127.0.0.1', 9))
    def test_project_is_read_only(self):
        with self.assertRaises(OSError):
            pathlib.Path('deployment.json').write_text('{{}}')
    def test_private_host_receipt_cannot_be_read(self):
        with self.assertRaises(OSError):
            pathlib.Path({str(workspace._state_path)!r}).read_text()
""")
    result = workspace.check()
    assert result["status"] == "pass", failed_checks(result)
    assert result["tests_run"] == 5
    assert secret.read_text() == "private credential"


def test_node_and_shell_syntax_are_real_parser_checks(workspace):
    require_sandbox()
    if not shutil.which("node"):
        pytest.skip("Node syntax checks require an installed trusted Node runtime.")
    authored_feature(workspace)
    workspace.write_file("assets/client.mjs", "export const response = ;\n")
    workspace.write_file("operations/task.sh", "#!/bin/bash\nif true; then\n")
    failed = workspace.check()
    assert "node syntax: assets/client.mjs" in failed_checks(failed)
    assert "shell syntax: operations/task.sh" in failed_checks(failed)
    workspace.write_file("assets/client.mjs", "export const response = 'ready';\n")
    workspace.write_file("operations/task.sh", "#!/bin/bash\nif true; then printf 'ready\\n'; fi\n")
    passed = workspace.check()
    assert passed["ready_to_package"], failed_checks(passed)


@pytest.mark.parametrize("path", ["argparse.py", "meerkat_mobkit/__init__.py", "connectors/__init__.py", "assets/pathlib.py", "app.PY", "App.py", "implementation-report.json", "stdlib.so"])
def test_additive_files_cannot_shadow_framework_sdk_or_host_reports(workspace, path):
    with pytest.raises(WorkspaceError):
        workspace.write_file(path, "pass\n")


def test_direct_shadow_modules_are_rejected_before_they_can_run(workspace):
    (workspace.project / "argparse.py").write_text("raise AssertionError('must not run')\n")
    with pytest.raises(WorkspaceError, match="shadow"):
        workspace.check()


@pytest.mark.parametrize("termination", ["raise SystemExit(0)", "import os; os._exit(0)"])
def test_forged_test_stdout_and_early_exit_cannot_issue_a_passing_receipt(workspace, termination):
    require_sandbox()
    workspace.write_file("tests/test_forgery.py", "print('MOBKIT_HOST_TEST_RESULT={\"tests_run\":999,\"skipped\":0}', flush=True)\n" + termination + "\n")
    result = workspace.check()
    assert result["status"] == "fail", failed_checks(result)
    assert result["tests_run"] == 0 and not result["ready_to_package"]
    assert "authored tests" in failed_checks(result)
    with pytest.raises(WorkspaceError):
        workspace.export_files()


@pytest.mark.parametrize("termination", ["raise SystemExit(0)", "import os; os._exit(0)"])
def test_premature_application_completion_is_not_a_passing_contract_check(workspace, termination):
    require_sandbox()
    authored_feature(workspace)
    workspace.write_file("application/__init__.py", "def build(context):\n    " + termination + "\n")
    result = workspace.check()
    assert result["status"] == "fail" and not result["ready_to_package"]
    assert "application lifecycle and tool contracts" in failed_checks(result)


def test_application_check_loads_real_sdk_and_validates_callbacks_without_invoking_them(workspace):
    require_sandbox()
    authored_feature(workspace)
    source = """from meerkat_mobkit import MobKit

class Integration:
    def tools_for_agent(self, agent):
        if agent['classId'] != 'lead':
            return []
        return [{'name': 'prepare_digest', 'handler': self.prepare_digest,
                 'description': 'Prepare a digest from supplied records.', 'schema': {'type': 'object'}}]
    def prepare_digest(self, arguments):
        raise AssertionError('Contract checks must not invoke callbacks.')
    def start(self):
        raise AssertionError('Contract checks must not start adapters.')

def build(context):
    assert context.runtime is None
    assert context.deployment['schemaVersion'] == 2
    assert MobKit.builder
    return Integration()
"""
    workspace.write_file("application/__init__.py", source)
    passed = workspace.check()
    assert passed["ready_to_package"], failed_checks(passed)
    contract = next(item for item in passed["checks"] if item["name"] == "application lifecycle and tool contracts")
    assert "1 scoped callbacks" in contract["detail"]
    workspace.write_file("application/__init__.py", source.replace("'handler': self.prepare_digest", "'handler': 'not a callback'"))
    failed = workspace.check()
    assert failed["status"] == "fail" and not failed["ready_to_package"]
    assert "Malformed application callback" in failed_checks(failed)["application lifecycle and tool contracts"]


def test_complete_check_has_a_budget_across_all_executable_steps(workspace, monkeypatch):
    require_sandbox()
    authored_feature(workspace)
    monkeypatch.setattr(implementation, "CHECK_TOTAL_SECONDS", 0.05)
    result = workspace.check()
    assert result["status"] in ("fail", "unavailable") and not result["ready_to_package"]
    assert any("time limit" in item["detail"] for item in result["checks"])
    assert result["limits"]["execution_total_seconds"] == 0.05


@pytest.mark.parametrize("source, limit_name", [("while True: pass", "time limit"), ("print('x' * 20000)", "output limit")])
def test_sandboxed_checks_have_time_and_output_limits(tmp_path, monkeypatch, source, limit_name):
    require_sandbox()
    snapshot = tmp_path / "project"
    snapshot.mkdir()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    monkeypatch.setattr(implementation, "CHECK_TIMEOUT_SECONDS", 0.25)
    monkeypatch.setattr(implementation, "CHECK_OUTPUT_BYTES", 1024)
    result = implementation._run_sandboxed(
        [str(Path(sys.executable).resolve()), "-I", "-S", "-c", source],
        implementation._sandbox_executable(), implementation._sandbox_profile(snapshot, scratch, tmp_path),
        snapshot, implementation._check_environment(scratch),
    )
    assert result["status"] == "fail" and limit_name in result["detail"]
    assert len(result["detail"]) < 1200


class TerminationProcess:
    pid = 987654

    def __init__(self, polls, kill_error=None, wait_error=None):
        self.polls = iter(polls)
        self.kill_error = kill_error
        self.wait_error = wait_error
        self.kills = 0
        self.waits = []

    def poll(self):
        return next(self.polls)

    def kill(self):
        self.kills += 1
        if self.kill_error:
            raise self.kill_error

    def wait(self, timeout):
        self.waits.append(timeout)
        if self.wait_error:
            raise self.wait_error
        return -9


def test_termination_does_not_signal_an_already_reaped_child(monkeypatch):
    process = TerminationProcess([0])
    def unexpected_signal(*args):
        raise AssertionError("Do not signal a PID whose child has already exited.")
    monkeypatch.setattr(implementation.os, "killpg", unexpected_signal)
    implementation._terminate_process(process)
    assert process.kills == 0 and not process.waits


@pytest.mark.parametrize("group_error", [PermissionError(1, "startup race"), ProcessLookupError(3, "group exited")])
def test_termination_falls_back_to_the_exact_live_child_when_group_signal_races(monkeypatch, group_error):
    process = TerminationProcess([None, None])
    def group_signal(*args):
        raise group_error
    monkeypatch.setattr(implementation.os, "killpg", group_signal)
    implementation._terminate_process(process)
    assert process.kills == 1 and process.waits == [implementation.TERMINATION_WAIT_SECONDS]


def test_termination_observes_a_child_that_exited_during_the_group_signal(monkeypatch):
    process = TerminationProcess([None, 0])
    def group_signal(*args):
        raise PermissionError(1, "startup race")
    monkeypatch.setattr(implementation.os, "killpg", group_signal)
    implementation._terminate_process(process)
    assert process.kills == 0 and not process.waits


def test_denied_signals_can_only_be_accepted_after_a_bounded_observed_exit(monkeypatch):
    process = TerminationProcess([None, None], kill_error=PermissionError(1, "signal denied"))
    def group_signal(*args):
        raise PermissionError(1, "group signal denied")
    monkeypatch.setattr(implementation.os, "killpg", group_signal)
    implementation._terminate_process(process)
    assert process.kills == 1 and process.waits == [implementation.TERMINATION_WAIT_SECONDS]


@pytest.mark.parametrize("group_denied", [True, False])
def test_real_inability_to_stop_a_live_child_is_reported_as_an_error(monkeypatch, group_denied):
    process = TerminationProcess([None, None], kill_error=PermissionError(1, "signal denied"),
                                 wait_error=subprocess.TimeoutExpired("sandbox child", 1))
    def group_signal(*args):
        if group_denied:
            raise PermissionError(1, "group signal denied")
    monkeypatch.setattr(implementation.os, "killpg", group_signal)
    with pytest.raises(WorkspaceError, match="no verification receipt"):
        implementation._terminate_process(process)
    assert process.waits == [implementation.TERMINATION_WAIT_SECONDS]


def test_actual_timeout_reaps_the_child_even_if_group_signal_is_denied(tmp_path, monkeypatch):
    require_sandbox()
    project = tmp_path / "project"
    project.mkdir()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    started = []
    original_popen = implementation.subprocess.Popen
    def record_process(*args, **kwargs):
        process = original_popen(*args, **kwargs)
        started.append(process)
        return process
    def denied_group_signal(*args):
        raise PermissionError(1, "simulated sandbox-exec group race")
    monkeypatch.setattr(implementation.subprocess, "Popen", record_process)
    monkeypatch.setattr(implementation.os, "killpg", denied_group_signal)
    result = implementation._run_sandboxed(
        [str(Path(sys.executable).resolve()), "-I", "-S", "-c", "while True: pass"],
        implementation._sandbox_executable(), implementation._sandbox_profile(project, scratch, tmp_path),
        project, implementation._check_environment(scratch), timeout=0.05,
    )
    assert result["status"] == "fail" and "time limit" in result["detail"]
    assert len(started) == 1 and started[0].poll() is not None


def test_cleanup_failure_cannot_leave_an_earlier_receipt_current(workspace, monkeypatch):
    require_sandbox()
    authored_feature(workspace)
    assert workspace.check()["ready_to_package"]
    def cleanup_failure(*args, **kwargs):
        raise WorkspaceError("Sandbox child could not be terminated; no verification receipt can be issued.")
    monkeypatch.setattr(implementation, "_run_sandboxed", cleanup_failure)
    with pytest.raises(WorkspaceError, match="could not be terminated"):
        workspace.check()
    assert workspace.verification() is None
    with pytest.raises(WorkspaceError):
        workspace.export_files()


def test_sandbox_cannot_spawn_child_processes_outside_termination_accounting(workspace):
    require_sandbox()
    workspace.write_file("tests/test_no_spawn.py", """import subprocess, unittest
class NoSpawn(unittest.TestCase):
    def test_child_creation_is_denied(self):
        with self.assertRaises(OSError):
            subprocess.run(['/usr/bin/true'], check=True)
""")
    result = workspace.check()
    assert result["ready_to_package"], failed_checks(result)
