"""First knowledge fetch failure permits an inert retry in the same project."""
import ast
import ctypes
import errno
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
from types import SimpleNamespace

import pytest

from test_elephant import claims_of, manager


SETTINGS = {"mode": "local", "source_repository": "https://example.invalid/knowledge.git",
            "source_revision": "a" * 40}


@pytest.fixture
def checkout(manager, monkeypatch):
    if os.environ.get("FORGE_TEST_KNOWLEDGE_SOURCE_BASELINE") == "1":
        source = Path(__file__).parents[1] / ".artifacts/forge-knowledge-source-recovery-20261010/manage.py.before"
        assert hashlib.sha256(source.read_bytes()).hexdigest() == "265a10314e83150c1cd7928714a768c2844b2ae8d1d76c3c498d4d2670b53585"
        node = next(item for item in ast.parse(source.read_bytes()).body
                    if isinstance(item, ast.FunctionDef) and item.name == "source_checkout")
        exec(compile(ast.Module(body=[node], type_ignores=[]), str(source), "exec"), manager.__dict__)
    private = [manager.ROOT / ".env", manager.ROOT.parent / ".env", manager.ROOT.parent / "saved-history.bin"]
    for path in private:
        path.write_bytes(b"JWT_SECRET=retained-inert-signing-value\nSURREALDB_PASS=retained-inert-database-value\n"
                         if path.name == ".env" else b"Retained synthetic private input.\n")
        path.chmod(0o600)
    before = {path: (path.read_bytes(), path.stat().st_ino) for path in private}
    observation = SimpleNamespace(manager=manager, calls=[], before=before, failure=None,
        revision=SETTINGS["source_revision"], origin=SETTINGS["source_repository"], status="",
        after_checkout=lambda: None, during_fetch=lambda: None, after_validation=lambda: None)

    def git(arguments, *, capture=False, timeout=None):
        assert arguments[:2] == ["git", "-C"] and timeout is None
        folder, operation = Path(arguments[2]), arguments[3:]
        observation.calls.append((folder, operation, capture))
        if operation == ["init"]:
            assert folder.stat().st_mode & 0o777 == 0o700
            (folder / ".git").mkdir()
        elif operation == ["remote", "add", "origin", SETTINGS["source_repository"]]:
            (folder / ".git/config").write_text("Inert source configuration.\n")
        elif operation == ["fetch", "--depth=1", "origin", SETTINGS["source_revision"]]:
            observation.during_fetch()
            if observation.failure is not None:
                raise observation.failure
            (folder / ".git/FETCH_HEAD").write_text(SETTINGS["source_revision"])
        elif operation == ["checkout", "--detach", "FETCH_HEAD"]:
            (folder / "Cargo.toml").write_text("Inert pinned source, never compiled.\n")
            (folder / ".git/HEAD").write_text(observation.revision)
            observation.after_checkout()
        elif operation == ["rev-parse", "HEAD"]:
            if not (folder / ".git/HEAD").exists():
                raise subprocess.CalledProcessError(128, arguments)
            return SimpleNamespace(stdout=observation.revision)
        elif operation == ["status", "--porcelain"]:
            return SimpleNamespace(stdout=observation.status)
        elif operation == ["remote", "get-url", "origin"]:
            observation.after_validation()
            return SimpleNamespace(stdout=observation.origin)
        else:
            pytest.fail("Unexpected inert Git operation.")
        assert not capture
        return SimpleNamespace(stdout="")

    monkeypatch.setattr(manager, "command", git)
    monkeypatch.setattr(manager.subprocess, "run", lambda *args, **kwargs: pytest.fail("No real Git or Compose command may run."))
    monkeypatch.setattr(manager.subprocess, "Popen", lambda *args, **kwargs: pytest.fail("No process may spawn."))
    return observation


def preserved(checkout):
    assert all(path.read_bytes() == data and path.stat().st_ino == inode
               for path, (data, inode) in checkout.before.items())


def existing_source(checkout):
    source = checkout.manager.ROOT / ".source"
    (source / ".git").mkdir(parents=True)
    (source / ".git/HEAD").write_text(checkout.revision)
    (source / "keep.txt").write_bytes(b"Existing unrelated source remains byte-identical.\n")
    return source


def test_first_fetch_failure_then_retry_reuses_same_project_without_poisoned_source(checkout):
    manager = checkout.manager
    failure = subprocess.CalledProcessError(128, ["inert-git-fetch"])
    checkout.failure = failure
    with pytest.raises(subprocess.CalledProcessError) as caught:
        manager.source_checkout(SETTINGS)
    assert caught.value is failure
    assert not (manager.ROOT / ".source").exists(), "A failed first fetch must not publish a final checkout."
    assert not list(manager.ROOT.glob(".source-pending-*"))
    preserved(checkout)
    checkout.failure = None
    manager.source_checkout(SETTINGS)
    source = manager.ROOT / ".source"
    assert (source / ".git/HEAD").read_text() == SETTINGS["source_revision"]
    assert (source / "Cargo.toml").is_file() and source.stat().st_mode & 0o777 == 0o700
    assert not list(manager.ROOT.glob(".source-pending-*"))
    assert sum(operation[0] == "fetch" for _, operation, _ in checkout.calls) == 2
    checkout.calls.clear()
    manager.source_checkout(SETTINGS)
    assert [operation for _, operation, _ in checkout.calls] == [
        ["rev-parse", "HEAD"], ["status", "--porcelain"], ["remote", "get-url", "origin"]]
    preserved(checkout)


@pytest.mark.parametrize("mismatch", ["revision", "origin", "dirty"])
def test_existing_unrelated_source_is_refused_without_staging_or_changes(checkout, mismatch):
    source = existing_source(checkout)
    before = {path: (path.read_bytes(), path.stat().st_ino) for path in source.rglob("*") if path.is_file()}
    if mismatch == "revision":
        checkout.revision = "b" * 40
    elif mismatch == "origin":
        checkout.origin = "https://example.invalid/unrelated.git"
    else:
        checkout.status = " M keep.txt\n"
    with pytest.raises(ValueError, match="pinned clean checkout"):
        checkout.manager.source_checkout(SETTINGS)
    assert {path: (path.read_bytes(), path.stat().st_ino) for path in source.rglob("*") if path.is_file()} == before
    assert not list(checkout.manager.ROOT.glob(".source-pending-*"))
    assert all(capture for _, _, capture in checkout.calls)
    preserved(checkout)


@pytest.mark.parametrize("mismatch", ["revision", "origin", "dirty"])
def test_new_checkout_requires_all_pinned_source_facts_before_publication(checkout, mismatch):
    if mismatch == "revision":
        checkout.revision = "b" * 40
    elif mismatch == "origin":
        checkout.origin = "https://example.invalid/unrelated.git"
    else:
        checkout.status = " M Cargo.toml\n"
    with pytest.raises(ValueError, match="pinned clean revision and repository"):
        checkout.manager.source_checkout(SETTINGS)
    assert not (checkout.manager.ROOT / ".source").exists()
    assert not list(checkout.manager.ROOT.glob(".source-pending-*"))
    preserved(checkout)


@pytest.mark.parametrize("competing", ["empty", "populated", "symlink"])
def test_publication_never_replaces_a_competing_destination(checkout, competing):
    source = checkout.manager.ROOT / ".source"
    target = checkout.manager.ROOT / "competing-source"
    target.mkdir()
    (target / "keep.txt").write_bytes(b"Competing source must remain.\n")
    snapshot = {}

    def compete():
        if competing == "symlink":
            source.symlink_to(target, target_is_directory=True)
        else:
            source.mkdir()
            if competing == "populated":
                (source / "keep.txt").write_bytes(b"Competing source must remain.\n")
        snapshot["inode"] = source.lstat().st_ino

    checkout.after_checkout = compete
    with pytest.raises(OSError) as caught:
        checkout.manager.source_checkout(SETTINGS)
    assert caught.value.errno in {errno.EEXIST, errno.ENOTEMPTY}
    assert source.lstat().st_ino == snapshot["inode"]
    if competing == "symlink":
        assert source.is_symlink() and source.readlink() == target
    elif competing == "populated":
        assert (source / "keep.txt").read_bytes() == b"Competing source must remain.\n"
    else:
        assert list(source.iterdir()) == []
    assert (target / "keep.txt").read_bytes() == b"Competing source must remain.\n"
    assert not list(checkout.manager.ROOT.glob(".source-pending-*"))
    preserved(checkout)


@pytest.mark.parametrize("platform", ["linux", "darwin"])
def test_unavailable_no_replace_primitive_refuses_publication_without_fallback(checkout, monkeypatch, platform):
    monkeypatch.setattr(checkout.manager.sys, "platform", platform)
    monkeypatch.setattr(checkout.manager.ctypes, "CDLL", lambda *args, **kwargs: SimpleNamespace())
    with pytest.raises(ValueError, match="without replacing existing source"):
        checkout.manager.source_checkout(SETTINGS)
    assert not (checkout.manager.ROOT / ".source").exists()
    assert not list(checkout.manager.ROOT.glob(".source-pending-*"))
    preserved(checkout)


def test_publication_preserves_actual_errno_and_cleans_only_own_attempt(checkout, monkeypatch):
    class Unavailable:
        def __call__(self, *args):
            ctypes.set_errno(errno.EXDEV)
            return -1

    function = Unavailable()
    monkeypatch.setattr(checkout.manager.ctypes, "CDLL", lambda *args, **kwargs:
        SimpleNamespace(renameat2=function, renamex_np=function))
    with pytest.raises(OSError) as caught:
        checkout.manager.source_checkout(SETTINGS)
    assert caught.value.errno == errno.EXDEV
    assert not (checkout.manager.ROOT / ".source").exists()
    assert not list(checkout.manager.ROOT.glob(".source-pending-*"))
    preserved(checkout)


def test_failed_stage_cleanup_leaves_final_source_absent_and_retry_can_use_new_attempt(checkout, monkeypatch):
    manager = checkout.manager
    checkout.failure = subprocess.CalledProcessError(128, ["inert-git-fetch"])
    original_cleanup = shutil.rmtree
    cleanup_error = PermissionError("Synthetic staging cleanup refused.")
    calls = []

    def refuse(path):
        calls.append(Path(path))
        raise cleanup_error

    with monkeypatch.context() as cleanup:
        cleanup.setattr(manager.shutil, "rmtree", refuse)
        with pytest.raises(PermissionError) as caught:
            manager.source_checkout(SETTINGS)
    assert caught.value is cleanup_error and caught.value.__context__ is checkout.failure
    assert len(calls) == 1 and calls[0].name.startswith(".source-pending-")
    assert calls[0].parent == manager.ROOT and calls[0].is_dir()
    assert not (manager.ROOT / ".source").exists()
    checkout.failure = None
    manager.source_checkout(SETTINGS)
    assert (manager.ROOT / ".source/Cargo.toml").is_file()
    assert calls[0].is_dir(), "A later attempt must not delete an earlier retained stage."
    preserved(checkout)
    original_cleanup(calls[0])


@pytest.mark.parametrize("when", ["after-validation", "failed-fetch"])
def test_replaced_stage_is_neither_published_nor_removed(checkout, when):
    manager = checkout.manager
    moved = manager.ROOT / "retained-original-attempt"
    snapshot = {}
    failure = subprocess.CalledProcessError(128, ["inert-git-fetch"])

    def replace():
        stage = checkout.calls[-1][0]
        stage.rename(moved)
        stage.mkdir()
        (stage / "keep.txt").write_bytes(b"Unrelated replacement must remain.\n")
        snapshot["stage"] = stage
        snapshot["inode"] = stage.lstat().st_ino
        snapshot["moved_inode"] = moved.lstat().st_ino
        snapshot["moved_files"] = {path.relative_to(moved): path.read_bytes()
                                   for path in moved.rglob("*") if path.is_file()}

    if when == "after-validation":
        checkout.after_validation = replace
    else:
        checkout.during_fetch = replace
        checkout.failure = failure
    with pytest.raises(ValueError, match="staging directory changed") as caught:
        manager.source_checkout(SETTINGS)
    if when == "failed-fetch":
        assert caught.value.__context__ is failure
    assert not (manager.ROOT / ".source").exists()
    assert snapshot["stage"].lstat().st_ino == snapshot["inode"]
    assert (snapshot["stage"] / "keep.txt").read_bytes() == b"Unrelated replacement must remain.\n"
    assert moved.lstat().st_ino == snapshot["moved_inode"]
    assert {path.relative_to(moved): path.read_bytes() for path in moved.rglob("*") if path.is_file()} == snapshot["moved_files"]
    preserved(checkout)


def test_up_fetch_retry_reaches_compose_only_after_valid_checkout(checkout, monkeypatch):
    manager = checkout.manager
    events = []
    expected_settings = {**SETTINGS, "token_env": "ELEPHANT_TOKEN", "space_env": "ELEPHANT_SPACE_ID",
                         "project": "test-project"}
    monkeypatch.setattr(manager, "load_settings", lambda: expected_settings)
    monkeypatch.setattr(manager, "local_preflight", lambda: events.append("preflight"))
    monkeypatch.setattr(manager, "setup", lambda *_: pytest.fail("Existing private credentials must remain."))

    def compose(*args):
        assert (manager.ROOT / ".source/Cargo.toml").is_file()
        # The service interpolates the extractor token, so it exists before the stack starts.
        assert manager.read_env(manager.ROOT / ".env").get("MCP_BEARER_TOKEN")
        events.append(("compose", args))

    def provision(settings, values):
        assert settings is expected_settings and values.get("MCP_BEARER_TOKEN")
        events.append("provision")
        return {"space": settings["project"]}

    def space_status(settings, *, wait=0):
        assert settings is expected_settings and wait == manager.READY_TIMEOUT
        events.append("status")
        return {"semantic_ingest_ready": True}

    async def verify(settings, operation):
        assert settings is expected_settings and operation == "verify"
        events.append("verify")
        return {"fixture_verified": True}

    monkeypatch.setattr(manager, "compose", compose)
    monkeypatch.setattr(manager, "provision", provision)
    monkeypatch.setattr(manager, "space_status", space_status)
    monkeypatch.setattr(manager, "mcp_operation", verify)
    monkeypatch.setattr(sys, "argv", ["manage.py", "up"])
    checkout.failure = subprocess.CalledProcessError(128, ["inert-git-fetch"])
    with pytest.raises(subprocess.CalledProcessError):
        manager.main()
    assert events == ["preflight"] and not (manager.ROOT / ".source").exists()
    # The folder predates the extractor token, and the failed fetch still changed no private file.
    preserved(checkout)
    checkout.failure = None
    manager.main()
    assert events == ["preflight", "preflight",
        ("compose", ("up", "--build", "--detach", "--wait", "--wait-timeout", "180")), "provision", "status", "verify"]
    # The retry adds only the extractor token; the other private files keep their bytes and inode.
    knowledge_env = manager.ROOT / ".env"
    added = manager.read_env(knowledge_env)
    assert claims_of(added.pop("MCP_BEARER_TOKEN"))["sub"] == manager.WORKER_SUBJECT
    original = checkout.before[knowledge_env][0].decode()
    assert added == dict(line.split("=", 1) for line in original.splitlines())
    assert all(path.read_bytes() == data and path.stat().st_ino == inode
               for path, (data, inode) in checkout.before.items() if path != knowledge_env)
