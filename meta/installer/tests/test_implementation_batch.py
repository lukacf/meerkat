"""Offline source transactions only; no authored code or native runs execute."""
from concurrent.futures import ThreadPoolExecutor
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import stat

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.implementation_worker import BuildTools
from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError
from launchkit.implementation_timing import ImplementationTiming, TimedDraft
from launchkit.templates.storage_setup import write_private
import launchkit.implementation_workspace as workspace_module
import launchkit.implementation_worker as worker_module


@pytest.fixture
def workspace(tmp_path):
    workspace = ImplementationWorkspace(tmp_path / "workspace", starter_blueprint())
    workspace.seed()
    workspace.write_file("application/first.txt", "alpha\r\nbeta\r\n")
    workspace.write_file("application/second.sh", "#!/bin/sh\nsecond\n")
    return workspace


def sha(text):
    return hashlib.sha256(text.encode()).hexdigest()


def write(path, content, expected=None):
    return {"kind": "write", "path": path, "content": content, "expected_sha256": expected}


def edit(path, expected, *replacements):
    return {"kind": "edit", "path": path, "expected_sha256": expected,
            "edits": [{"old_text": old, "new_text": new} for old, new in replacements]}


def changes(workspace):
    return [edit("application/first.txt", sha(workspace.read_file("application/first.txt")),
                 ("alpha", "ALPHA"), ("beta", "BETA")),
            write("application/second.sh", "#!/bin/sh\nupdated\n",
                  sha(workspace.read_file("application/second.sh"))),
            write("assets/new/page.html", "<p>Fresh batch source.</p>\n")]


def snapshot(workspace):
    return {"state": workspace._state_path.read_bytes(),
            "files": {path.relative_to(workspace.project).as_posix():
                      (path.read_bytes(), stat.S_IMODE(path.stat().st_mode))
                      for path in workspace.project.rglob("*") if path.is_file()},
            "directories": sorted(path.relative_to(workspace.project).as_posix()
                                  for path in workspace.project.rglob("*") if path.is_dir())}


def check_fixture(workspace):
    # Host-signed fixture tests invalidation without executing application checks.
    with workspace._locked():
        state = workspace._state()
        receipt = {"status": "pass", "tests_run": 1, "ready_to_package": True,
                   "digest": workspace._digest(workspace._files(state)),
                   "checks": [], "fixture": "synthetic-not-executed"}
        receipt["signature"] = workspace._signed(receipt)
        state["last_check"] = receipt
        workspace._save_state(state)
    assert workspace.verification() == receipt
    return receipt


def assert_no_transaction(workspace):
    assert not workspace._batch_path.exists()
    assert not list(workspace.root.glob("batch-stage-*"))
    assert not list(workspace.root.glob("batch-backup-*"))


def test_successful_batch_preserves_framework_modes_and_fresh_digest_invalidates_once(workspace, monkeypatch):
    receipt = check_fixture(workspace)
    before = snapshot(workspace)
    provenance = workspace.framework_provenance()
    digest = workspace.digest()
    saves = []
    save = workspace._save_state
    monkeypatch.setattr(workspace, "_save_state", lambda state: (saves.append(deepcopy(state)), save(state))[1])
    result = workspace.apply_batch(changes(workspace))
    assert len(saves) == 1 and saves[0]["last_check"] is None
    assert result["digest"] == workspace.digest() != digest == receipt["digest"]
    assert workspace.read_file("application/first.txt") == "ALPHA\nBETA\n"
    assert workspace.read_file("application/second.sh") == "#!/bin/sh\nupdated\n"
    assert workspace.read_file("assets/new/page.html") == "<p>Fresh batch source.</p>\n"
    assert len(result["files"]) == 3
    for row in result["files"]:
        assert row["sha256"] == sha(workspace.read_file(row["path"]))
        assert row["sha256"] + "  " + row["path"] + "\n" in workspace.read_file("SHA256SUMS")
    assert stat.S_IMODE((workspace.project / "application/second.sh").stat().st_mode) == 0o755
    assert stat.S_IMODE((workspace.project / "assets/new/page.html").stat().st_mode) == 0o644
    assert workspace.verification() is None
    assert workspace.framework_provenance() == provenance
    after = snapshot(workspace)
    for name in workspace._state()["baseline"]:
        assert after["files"][name] == before["files"][name]
    state = workspace._state()
    assert [row["path"] for row in state["changes"][-3:]] == [row["path"] for row in result["files"]]
    assert len({row["at"] for row in state["changes"][-3:]}) == 1
    assert_no_transaction(workspace)


@pytest.mark.parametrize("position", [0, 1, 2])
def test_one_stale_hash_in_batch_preserves_all_bytes_modes_and_check(workspace, position):
    receipt = check_fixture(workspace)
    before = snapshot(workspace)
    operations = changes(workspace)
    operations[position]["expected_sha256"] = "0" * 64
    with pytest.raises(WorkspaceError, match="changed or already exists"):
        workspace.apply_batch(operations)
    assert snapshot(workspace) == before
    assert workspace.verification() == receipt
    assert_no_transaction(workspace)


@pytest.mark.parametrize("path", ["app.py", "SHA256SUMS", "config/console.toml", "acceptance-tests/test_fake.py",
                                   "acceptance-report.json", "implementation-report.json", "../outside.txt", ".env", "helper.py"])
def test_protected_or_invalid_batch_path_prevents_other_changes(workspace, path):
    before = snapshot(workspace)
    with pytest.raises(WorkspaceError):
        workspace.apply_batch([write("assets/allowed.txt", "allowed"), write(path, "blocked")])
    assert snapshot(workspace) == before
    assert_no_transaction(workspace)


@pytest.mark.parametrize("paths", [["assets/same.txt", "assets/same.txt"],
    ["assets/same.txt", "assets/SAME.txt"], ["assets/new", "assets/new/nested.txt"],
    ["assets/new/nested.txt", "assets/new"], ["assets/Folder/one.txt", "assets/folder/two.txt"],
    ["application/first.txt", "application/FIRST.txt/child.txt"]])
def test_duplicate_alias_or_conflicting_paths_are_rejected_before_staging(workspace, paths):
    before = snapshot(workspace)
    with pytest.raises(WorkspaceError):
        workspace.apply_batch([write(path, "new") for path in paths])
    assert snapshot(workspace) == before
    assert_no_transaction(workspace)


@pytest.mark.parametrize("invalid", [None, [], {}, [None], [{"kind": {}}],
    [write("assets/new.txt", "new", True)], [{**write("assets/new.txt", "new"), "extra": "ignored"}],
    [edit("application/first.txt", None, ("alpha", "changed"))],
    [edit("application/first.txt", "0" * 64)], [edit("application/first.txt", "0" * 64, ("", "changed"))],
    [edit("application/first.txt", "0" * 64, ("alpha", "alpha"))],
    [write("assets/new.txt", "bad\x00text")], [write("assets/new.txt", "\ud800")]])
def test_malformed_batch_arguments_have_no_effect(workspace, invalid):
    before = snapshot(workspace)
    with pytest.raises(WorkspaceError):
        workspace.apply_batch(invalid)
    assert snapshot(workspace) == before
    assert_no_transaction(workspace)


@pytest.mark.parametrize("case", ["missing", "ambiguous", "overlapping", "late-missing", "net-noop", "null-overwrite"])
def test_invalid_exact_replacement_or_creation_fence_rejects_entire_batch(workspace, case):
    workspace.write_file("application/first.txt", "alpha\nrepeat repeat\naaa\n")
    current = workspace.read_file("application/first.txt")
    replacements = {"missing": [("missing", "new")], "ambiguous": [("repeat", "new")],
        "overlapping": [("aa", "new")], "late-missing": [("alpha", "changed"), ("missing", "new")],
        "net-noop": [("alpha", "changed"), ("changed", "alpha")]}
    operations = [write("assets/first-valid.txt", "valid")]
    operations.append(write("application/first.txt", "new") if case == "null-overwrite" else
                      edit("application/first.txt", sha(current), *replacements[case]))
    before = snapshot(workspace)
    with pytest.raises(WorkspaceError):
        workspace.apply_batch(operations)
    assert snapshot(workspace) == before
    assert_no_transaction(workspace)


@pytest.mark.parametrize("limit", ["files", "arguments", "replacements", "project-bytes", "project-files", "final-file"])
def test_batch_limits_validate_aggregate_and_final_sizes_without_source_changes(workspace, monkeypatch, limit):
    operations = [write("assets/new1.txt", "a" * 15), write("assets/new2.txt", "b" * 15)]
    if limit == "files":
        monkeypatch.setattr(workspace_module, "MAX_BATCH_FILES", 1)
    elif limit == "arguments":
        monkeypatch.setattr(workspace_module, "MAX_BATCH_ARGUMENT_BYTES", 160)
    elif limit == "replacements":
        monkeypatch.setattr(workspace_module, "MAX_BATCH_EDITS", 1)
        operations = [edit("application/first.txt", sha(workspace.read_file("application/first.txt")),
                           ("alpha", "first"), ("beta", "second"))]
    elif limit == "project-bytes":
        raw = sum(path.stat().st_size for path in workspace.project.rglob("*") if path.is_file())
        monkeypatch.setattr(workspace_module, "MAX_PROJECT_BYTES", raw + 20)
    elif limit == "project-files":
        monkeypatch.setattr(workspace_module, "MAX_PROJECT_FILES", len(workspace.list_files()) + 1)
    else:
        workspace.write_file("application/first.txt", "a" * 90 + "end")
        original = workspace_module._text_size
        def bounded(text):
            size = original(text)
            if size > 100:
                raise WorkspaceError("Synthetic final file limit.")
            return size
        monkeypatch.setattr(workspace_module, "_text_size", bounded)
        operations = [edit("application/first.txt", sha(workspace.read_file("application/first.txt")), ("end", "x" * 20))]
    before = snapshot(workspace)
    with pytest.raises(WorkspaceError):
        workspace.apply_batch(operations)
    assert snapshot(workspace) == before
    assert_no_transaction(workspace)


@pytest.mark.parametrize("point", ["stage-write", "install-stage", "state-save"])
def test_io_failure_before_state_commit_rolls_back_source_and_checks(workspace, monkeypatch, point):
    receipt = check_fixture(workspace)
    before = snapshot(workspace)
    if point == "stage-write":
        original = workspace_module._atomic_write
        def failed(path, content, mode=0o600):
            if path.name == "second.sh" and path.parent.parent.name.startswith("batch-stage-"):
                raise OSError("Synthetic stage write failure")
            return original(path, content, mode)
        monkeypatch.setattr(workspace_module, "_atomic_write", failed)
    elif point == "install-stage":
        original = workspace_module.os.replace
        def failed(source, target):
            if Path(source).name.startswith("batch-stage-") and Path(target) == workspace.project:
                raise OSError("Synthetic project swap failure")
            return original(source, target)
        monkeypatch.setattr(workspace_module.os, "replace", failed)
    else:
        monkeypatch.setattr(workspace, "_save_state", lambda state: (_ for _ in ()).throw(OSError("Synthetic state write failure")))
    with pytest.raises(OSError):
        workspace.apply_batch(changes(workspace))
    assert snapshot(workspace) == before
    assert workspace.verification() == receipt
    assert_no_transaction(workspace)


@pytest.mark.parametrize("point", ["journal", "partial-stage", "old-project-moved", "new-project-moved", "state-committed", "cleanup"])
def test_process_interruption_recovers_exact_old_or_complete_new_batch_on_next_lock(workspace, monkeypatch, point):
    receipt = check_fixture(workspace)
    before = snapshot(workspace)
    operations = changes(workspace)
    if point == "journal":
        sync = workspace._sync_directory
        def interrupted(path):
            sync(path)
            if path == workspace.root:
                raise SystemExit("Synthetic interruption after durable journal")
        monkeypatch.setattr(workspace, "_sync_directory", interrupted)
    elif point == "partial-stage":
        def interrupted(source, target, **kwargs):
            Path(target).mkdir()
            (Path(target) / "partial.txt").write_text("partial staging only")
            raise SystemExit("Synthetic interruption during source copy")
        monkeypatch.setattr(workspace_module.shutil, "copytree", interrupted)
    elif point in {"old-project-moved", "new-project-moved"}:
        replace = workspace_module.os.replace
        def interrupted(source, target):
            result = replace(source, target)
            if (point == "old-project-moved" and Path(source) == workspace.project or
                    point == "new-project-moved" and Path(source).name.startswith("batch-stage-")):
                raise SystemExit("Synthetic interruption after directory move")
            return result
        monkeypatch.setattr(workspace_module.os, "replace", interrupted)
    elif point == "state-committed":
        save = workspace._save_state
        def interrupted(state):
            save(state)
            raise SystemExit("Synthetic interruption after state commit")
        monkeypatch.setattr(workspace, "_save_state", interrupted)
    else:
        monkeypatch.setattr(workspace, "_remove_batch_tree", lambda path: (_ for _ in ()).throw(SystemExit("Synthetic cleanup interruption")))
    with pytest.raises(SystemExit):
        workspace.apply_batch(operations)
    assert workspace._batch_path.is_file()
    monkeypatch.undo()
    restored = ImplementationWorkspace(workspace.root, workspace.blueprint)
    restored.list_files()
    if point in {"state-committed", "cleanup"}:
        assert restored.read_file("application/first.txt") == "ALPHA\nBETA\n"
        assert restored.read_file("application/second.sh") == "#!/bin/sh\nupdated\n"
        assert restored.read_file("assets/new/page.html") == "<p>Fresh batch source.</p>\n"
        assert restored.verification() is None
        assert len(restored._state()["changes"]) == len(json.loads(before["state"])["changes"]) + 3
    else:
        assert snapshot(restored) == before
        assert restored.verification() == receipt
    assert_no_transaction(restored)


def test_bad_journal_fails_closed_without_inventing_source_or_verification(workspace, monkeypatch):
    save = workspace._save_state
    monkeypatch.setattr(workspace, "_save_state", lambda state: (_ for _ in ()).throw(SystemExit("Synthetic interruption")))
    with pytest.raises(SystemExit):
        workspace.apply_batch(changes(workspace))
    journal = json.loads(workspace._batch_path.read_text())
    journal["after_digest"] = "0" * 64
    workspace._batch_path.write_text(json.dumps(journal))
    before = snapshot(workspace)
    monkeypatch.setattr(workspace, "_save_state", save)
    with pytest.raises(WorkspaceError, match="cannot be safely recovered"):
        workspace.list_files()
    assert snapshot(workspace) == before
    assert workspace._batch_path.exists()


def test_two_concurrent_batches_with_same_base_cannot_mix_source(workspace):
    expected_first = sha(workspace.read_file("application/first.txt"))
    expected_second = sha(workspace.read_file("application/second.sh"))
    def mutate(value):
        try:
            return workspace.apply_batch([write("application/first.txt", value, expected_first),
                write("application/second.sh", value, expected_second)])
        except WorkspaceError:
            return None
    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(mutate, ["first", "second"]))
    assert sum(result is not None for result in results) == 1
    assert workspace.read_file("application/first.txt") == workspace.read_file("application/second.sh")
    assert_no_transaction(workspace)


class Draft:
    def __init__(self):
        self.tools = {}
    def register_tool(self, name, handler, **options):
        self.tools[name] = {"handler": handler, **options}


@pytest.mark.asyncio
async def test_registered_batch_tool_updates_one_receipt_and_clears_publication_once(tmp_path, monkeypatch):
    write_private(tmp_path / "input.json", {"blueprint": starter_blueprint().model_dump(mode="json")})
    write_private(tmp_path / "receipt.json", {"status": "running", "progress": [], "ready_to_package": True})
    tools = BuildTools(tmp_path)
    tools.workspace.seed()
    tools.publication = {"fixture": "previous-publication"}
    draft = Draft()
    timing = ImplementationTiming(tmp_path)
    tools.register(TimedDraft(draft, timing, "implementation:main"))
    assert {"write_project_file", "edit_project_file", "check_implementation", "publish_implementation", "apply_project_batch"} <= set(draft.tools)
    updates = []
    original = worker_module.update_receipt
    def observed(directory, **fields):
        updates.append(fields)
        return original(directory, **fields)
    monkeypatch.setattr(worker_module, "update_receipt", observed)
    result = await draft.tools["apply_project_batch"]["handler"]({"operations": [
        write("application/new.txt", "first"), write("assets/new.txt", "second")]})
    assert result["ok"] and result["digest"] == tools.workspace.digest()
    assert tools.publication is None
    assert len(updates) == 1 and updates[0]["ready_to_package"] is False
    assert len(result["files"]) == 2
    assert [row["tool"] for row in timing.state["spans"] if row.get("tool")] == ["apply_project_batch"]
    prior = snapshot(tools.workspace)
    updates.clear()
    tools.publication = {"fixture": "retained-publication"}
    rejected = await tools.batch({"operations": [write("assets/new.txt", "conflict")]})
    assert rejected["ok"] is False and tools.publication == {"fixture": "retained-publication"}
    assert snapshot(tools.workspace) == prior and updates == []
    assert (await tools.batch({"operations": [], "extra": True}))["ok"] is False


def test_interrupted_rollback_remains_recoverable_with_complete_original_backup(workspace, monkeypatch):
    receipt = check_fixture(workspace)
    before = snapshot(workspace)
    monkeypatch.setattr(workspace, "_save_state", lambda state: (_ for _ in ()).throw(OSError("Synthetic failed state save")))
    remove = workspace._remove_batch_tree
    def interrupted(path):
        if path == workspace.project:
            (path / "application/first.txt").unlink()
            raise SystemExit("Synthetic interruption during rollback removal")
        return remove(path)
    monkeypatch.setattr(workspace, "_remove_batch_tree", interrupted)
    with pytest.raises(SystemExit):
        workspace.apply_batch(changes(workspace))
    monkeypatch.undo()
    restored = ImplementationWorkspace(workspace.root, workspace.blueprint)
    assert restored.verification() == receipt
    assert snapshot(restored) == before
    assert_no_transaction(restored)


def test_failed_post_state_fsync_retains_journal_and_backup_until_recovery(workspace, monkeypatch):
    check_fixture(workspace)
    old_changes = len(workspace._state()["changes"])
    sync = workspace._sync_directory
    def failed(path):
        if path == workspace.root and len(json.loads(workspace._state_path.read_text())["changes"]) > old_changes:
            raise OSError("Synthetic commit durability failure")
        return sync(path)
    monkeypatch.setattr(workspace, "_sync_directory", failed)
    with pytest.raises(OSError):
        workspace.apply_batch(changes(workspace))
    assert workspace._batch_path.exists()
    assert len(list(workspace.root.glob("batch-backup-*"))) == 1
    monkeypatch.undo()
    restored = ImplementationWorkspace(workspace.root, workspace.blueprint)
    assert restored.verification() is None
    assert restored.read_file("application/first.txt") == "ALPHA\nBETA\n"
    assert restored.read_file("assets/new/page.html") == "<p>Fresh batch source.</p>\n"
    assert len(restored._state()["changes"]) == old_changes + 3
    assert_no_transaction(restored)


@pytest.mark.parametrize("kind", ["symlink", "hardlink"])
def test_batch_does_not_follow_external_source_links(workspace, tmp_path, kind):
    outside = tmp_path / "outside.txt"
    outside.write_text("external source must stay intact")
    target = workspace.project / "assets/linked.txt"
    target.parent.mkdir(exist_ok=True)
    if kind == "symlink":
        target.symlink_to(outside)
    else:
        workspace_module.os.link(outside, target)
    state = workspace._state_path.read_bytes()
    with pytest.raises(WorkspaceError):
        workspace.apply_batch([write("application/first.txt", "changed", sha("alpha\nbeta\n"))])
    assert outside.read_text() == "external source must stay intact"
    assert workspace._state_path.read_bytes() == state
    assert (workspace.project / "application/first.txt").read_bytes() == b"alpha\r\nbeta\r\n"
    assert_no_transaction(workspace)


@pytest.mark.asyncio
async def test_post_commit_error_cannot_retain_a_cached_publication(tmp_path, monkeypatch):
    write_private(tmp_path / "input.json", {"blueprint": starter_blueprint().model_dump(mode="json")})
    write_private(tmp_path / "receipt.json", {"status": "running", "progress": [], "ready_to_package": True})
    tools = BuildTools(tmp_path)
    tools.workspace.seed()
    check_fixture(tools.workspace)
    tools.publication = {"checked_digest": tools.workspace.digest(), "fixture": "prior-publication"}
    save = tools.workspace._save_state
    def committed_but_lost(state):
        save(state)
        raise OSError("Synthetic post-commit reply loss")
    monkeypatch.setattr(tools.workspace, "_save_state", committed_but_lost)
    result = await tools.batch({"operations": [write("application/first.txt", "committed first"),
                                               write("application/second.txt", "committed second")]})
    assert result["ok"] is False
    assert tools.workspace.read_file("application/first.txt") == "committed first"
    assert tools.workspace.read_file("application/second.txt") == "committed second"
    assert tools.workspace.verification() is None and tools.publication is None
    assert json.loads((tmp_path / "receipt.json").read_text())["ready_to_package"] is False
    assert_no_transaction(tools.workspace)


@pytest.mark.asyncio
async def test_actual_sdk_batch_schema_and_handler_preserve_hash_and_creation_fences(tmp_path):
    from jsonschema import ValidationError, validate
    from meerkat_mobkit.models import SessionBuildOptions

    write_private(tmp_path / "input.json", {"blueprint": starter_blueprint().model_dump(mode="json")})
    write_private(tmp_path / "receipt.json", {"status": "running", "progress": [], "ready_to_package": False})
    tools = BuildTools(tmp_path)
    tools.workspace.seed()
    tools.workspace.write_file("application/current.txt", "before")
    draft = SessionBuildOptions()
    tools.register(draft)
    wire = next(row for row in draft.to_dict()["tools"] if row["name"] == "apply_project_batch")
    body = {"operations": [edit("application/current.txt", sha("before"), ("before", "after")),
                           write("assets/new.txt", "created")]}
    validate(body, wire["input_schema"])
    for bad in ({"operations": []}, {"operations": [{**body["operations"][0], "extra": True}]},
                {"operations": [write("assets/new.txt", "created", True)]}):
        with pytest.raises(ValidationError):
            validate(bad, wire["input_schema"])
    result = await draft.tool_handlers["apply_project_batch"](body)
    assert result["ok"] and result["digest"] == tools.workspace.digest()
    assert tools.workspace.read_file("application/current.txt") == "after"
    assert tools.workspace.read_file("assets/new.txt") == "created"
    assert (await draft.tool_handlers["apply_project_batch"](body))["ok"] is False
    assert tools.workspace.read_file("application/current.txt") == "after"
