"""Real private file/lease controls without native or application execution."""
import asyncio
from copy import deepcopy
import hashlib
import os
from pathlib import Path
import stat
import subprocess
import threading
import uuid

import pytest

from launchkit import connect_accepted_source as custody
from launchkit import deployment_updates as updates
from launchkit.connect_installer_caller import CustodyError, MAX_INPUT_BYTES
from launchkit.deployment import materialize, read_receipt, update_receipt
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_deployment import Guide, Implementation, Service, submit


class AdmissionOnly(Service):
    def _launch(self, directory, mode):
        # Exercise production submit admission, but do not enter a worker.
        pass


async def admitted(tmp_path, *, extra_files=None):
    tmp_path.chmod(0o700)
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "b" * 64
    implementation.extra_files = extra_files or {}
    service = AdmissionOnly(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    receipt = await submit(service, destination)
    directory = service._directory(guide.identity)
    materialize(directory)
    return directory, receipt, destination


async def retain(directory, receipt, lease, *, name="native-source", current=lambda: None):
    return await custody.retain_accepted_application_source(directory, receipt,
        project_lease=lease, require_owner_current=current, custody_directory=directory / name)


@pytest.fixture
def no_source_update_effects(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Source update binding controls cannot start or signal a real process.")
    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    monkeypatch.setattr(asyncio, "create_subprocess_shell", forbidden)
    monkeypatch.setattr(subprocess, "Popen", forbidden)
    monkeypatch.setattr(os, "kill", forbidden)
    monkeypatch.setattr(os, "killpg", forbidden)


async def admitted_source_update(tmp_path):
    """Stage through the existing owner, without entering any worker/runtime."""
    tmp_path.chmod(0o700)
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "b" * 64
    implementation.extra_files = {"application/logic.py": "VERSION = 'original'\n"}
    service = AdmissionOnly(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    previous = await submit(service, destination)
    directory = service._directory(guide.identity)
    materialize(directory)
    update_receipt(directory, status="stopped")
    implementation.identity = str(uuid.uuid4())
    implementation.digest = "c" * 64
    implementation.extra_files = {"application/logic.py": "VERSION = 'candidate'\n"}
    receipt = await service.stage_update(guide.identity, previous["deployment_id"],
                                        guide.state["version"], implementation.identity)
    return directory, receipt, destination


async def test_retained_source_survives_genuine_update_checkpoints_without_relabeling(
        tmp_path, no_source_update_effects):
    directory, receipt, destination = await admitted_source_update(tmp_path)
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease)
        original_receipt, original_source = source.receipt, source.source
        original_projection = {name: (source.root / name).read_bytes() for name in source.files}
        selected = updates.update_binding(receipt["update"])
        root = updates.update_directory(directory, receipt["update"]["update_id"])
        try:
            observed = updates.checkpoint(directory, root, receipt["update"],
                expected_binding=selected, status="activating",
                detail="Inert source test checkpoint, no runtime admitted.",
                native_activation_expected_revision=3)
            await source.require_current_async()
            assert not source._invalidated
            observed = updates.checkpoint(directory, root, observed,
                expected_binding=selected, status="completed",
                detail="Inert completed journal projection; no native acceptance claimed.",
                rollback_available=False)
            source.require_current()
            assert read_receipt(directory)["update"] == observed
            assert source.receipt == original_receipt and source.source == original_source
            assert source.receipt["update"]["status"] == "staged"
            assert source.exact_file("application/logic.py").read() == b"VERSION = 'original'\n"
            assert original_projection == {name: (source.root / name).read_bytes() for name in source.files}
        finally:
            source.close()
        assert not source._invalidated and source._closed
        with pytest.raises(CustodyError, match="no longer current"):
            source.require_current()


@pytest.mark.parametrize("changed", ["update_id", "package_sha256", "changes"])
async def test_retained_source_refuses_changed_original_update_selection_permanently(
        tmp_path, no_source_update_effects, changed):
    directory, receipt, destination = await admitted_source_update(tmp_path)
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease)
        try:
            selected = deepcopy(receipt["update"])
            if changed == "changes":
                selected[changed] = [*selected[changed], {"path": "application/unselected.py", "kind": "added"}]
            else:
                selected[changed] = str(uuid.uuid4()) if changed == "update_id" else "d" * 64
            update_receipt(directory, update=selected)
            with pytest.raises(CustodyError, match="admitted application source"):
                await source.require_current_async()
            update_receipt(directory, update=receipt["update"])
            with pytest.raises(CustodyError, match="no longer current"):
                source.require_current()
        finally:
            source.close()


def test_source_binding_update_projection_retains_all_selection_fields_and_types():
    selected = {key: {"selected": key} for key in updates.UPDATE_BINDING_KEYS}
    original = {"build_id": "same original build", "update": {**selected, "status": "staged"}}
    advanced = {**original, "update": {**selected, "status": "completed", "detail": "progress"}}
    before = custody._binding(original)
    after = custody._binding(advanced)
    assert before == after
    assert before["update"] == selected
    # Preserve the existing no-update value rather than treating it as an empty
    # update object, and never coerce malformed scalar projections to dictionaries.
    for left, right in [(None, {}), (None, False), (False, 0), ({}, [])]:
        assert not updates._same_toml(custody._binding({"update": left}),
                                     custody._binding({"update": right}))
    assert not updates._same_toml(before, custody._binding({**advanced, "build_id": "different source"}))
    original["update"]["changes"]["selected"] = "later caller mutation"
    assert before["update"]["changes"] == {"selected": "changes"}


async def test_production_admission_exact_private_files_and_existing_caller_pin(tmp_path):
    raw = b'{ "members": [], "literal": "\\u0061" }\n'
    directory, receipt, destination = await admitted(tmp_path,
        extra_files={"application/tool-policy.json": raw, "assets/nested/empty": b""})
    callback_threads = []
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease,
            current=lambda: callback_threads.append(threading.get_ident()))
        try:
            source.require_current()
            assert set(source.files) == set(custody.package_files((directory / "package.zip").read_bytes()))
            assert (source.root / "application/tool-policy.json").read_bytes() == raw
            assert (source.root / "assets/nested/empty").read_bytes() == b""
            assert source.exact_file("application/tool-policy.json").read() == raw
            assert all(t == threading.get_ident() for t in callback_threads)
            for path in [source.root, *source.root.rglob("*")]:
                assert stat.S_IMODE(path.stat().st_mode) == (0o700 if path.is_dir() else 0o600)
            detached = source.source
            detached["source_digest"] = "changed"
            assert source.source["source_digest"] == "b" * 64
            update_receipt(directory, status="checking", phase="source-custody", detail="Journal progress")
            source.require_current()
        finally:
            source.close()
        with pytest.raises(CustodyError):
            source.require_current()


async def test_full_source_keeps_large_and_empty_resources_but_native_selection_is_bounded(tmp_path):
    directory, receipt, destination = await admitted(tmp_path,
        extra_files={"assets/large": b"x" * (MAX_INPUT_BYTES + 1), "assets/empty": b""})
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease)
        try:
            assert (source.root / "assets/large").stat().st_size == MAX_INPUT_BYTES + 1
            for name in ["assets/large", "assets/empty", "application/__init__.py"]:
                with pytest.raises(CustodyError, match="native input"):
                    source.exact_file(name)
            source.require_current()
        finally:
            source.close()


@pytest.mark.parametrize("change", ["package", "installed", "private", "extra", "mode", "owner", "source", "brief"])
async def test_currentness_refuses_original_or_private_source_changes(tmp_path, change):
    directory, receipt, destination = await admitted(tmp_path)
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease)
        try:
            if change == "package":
                path = directory / "package.zip"
                content = path.read_bytes()
                path.unlink()
                path.write_bytes(content)
                path.chmod(0o600)
            elif change == "installed":
                (destination / "app.py").write_bytes(b"changed\n")
            elif change == "private":
                path = source.root / "app.py"
                content = path.read_bytes()
                path.unlink()
                path.write_bytes(content)
                path.chmod(0o600)
            elif change == "extra":
                (source.root / "extra").write_bytes(b"unselected")
            elif change == "mode":
                (source.root / "application").chmod(0o755)
            elif change == "owner":
                (destination / custody.OWNER_FILE).write_bytes(b"{}")
            elif change == "source":
                update_receipt(directory, build_id="other-selected-build")
            else:
                update_receipt(directory, brief_hash="c" * 64)
            with pytest.raises((CustodyError, OSError)):
                source.require_current()
        finally:
            source.close()


async def test_closed_original_lease_invalidates_retained_carrier(tmp_path):
    directory, receipt, destination = await admitted(tmp_path)
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease)
    try:
        with pytest.raises(RuntimeError, match="lock is no longer current"):
            source.exact_file("app.py")
    finally:
        source.close()


async def test_generated_lease_protocol_and_wrong_project_refusal(tmp_path):
    directory, receipt, destination = await admitted(tmp_path)
    with project_lock(destination) as original:
        class GeneratedLease:
            root = original.root
            require_current = staticmethod(original.require_current)
        source = await retain(directory, receipt, GeneratedLease())
        source.close()
        other = tmp_path / "other"
        other.mkdir(mode=0o700)
        with project_lock(other) as wrong:
            with pytest.raises(CustodyError, match="held project lease"):
                await retain(directory, receipt, wrong, name="wrong-project")
        assert not (directory / "wrong-project").exists()


@pytest.mark.parametrize("change", ["replacement", "fifo", "hardlink", "symlink"])
async def test_original_zip_observation_cannot_follow_replacement_or_block(tmp_path, monkeypatch, change):
    directory, receipt, destination = await admitted(tmp_path)
    path = directory / "package.zip"
    original_read = custody.os.read
    altered = False

    def read_then_replace(descriptor, size):
        nonlocal altered
        content = original_read(descriptor, size)
        info = os.fstat(descriptor)
        if not altered and info.st_size == path.stat().st_size:
            altered = True
            payload = path.read_bytes()
            alternate = directory / "other.zip"
            if change == "hardlink":
                os.link(path, alternate)
            else:
                path.unlink()
                if change == "fifo":
                    os.mkfifo(path, 0o600)
                elif change == "symlink":
                    alternate.write_bytes(payload)
                    path.symlink_to(alternate)
                else:
                    path.write_bytes(payload)
                    path.chmod(0o600)
        return content

    monkeypatch.setattr(custody.os, "read", read_then_replace)
    with project_lock(destination) as lease:
        with pytest.raises(CustodyError):
            await asyncio.wait_for(retain(directory, receipt, lease), timeout=2)
    assert not (directory / "native-source").exists()


async def test_fifo_at_package_open_is_nonblocking(tmp_path, monkeypatch):
    directory, receipt, destination = await admitted(tmp_path)
    original_open = custody.os.open
    changed = False

    def replace_before_open(path, flags, *args, **kwargs):
        nonlocal changed
        if path == "package.zip" and not changed:
            changed = True
            (directory / path).unlink()
            os.mkfifo(directory / path, 0o600)
            assert flags & os.O_NONBLOCK
        return original_open(path, flags, *args, **kwargs)

    monkeypatch.setattr(custody.os, "open", replace_before_open)
    with project_lock(destination) as lease:
        with pytest.raises(CustodyError):
            await asyncio.wait_for(retain(directory, receipt, lease), timeout=2)


async def test_installed_fifo_at_open_is_nonblocking(tmp_path, monkeypatch):
    directory, receipt, destination = await admitted(tmp_path)
    original_open = custody.os.open
    changed = False

    def replace_before_open(path, flags, *args, **kwargs):
        nonlocal changed
        if path == "app.py" and not flags & os.O_CREAT and not changed:
            changed = True
            (destination / path).unlink()
            os.mkfifo(destination / path, 0o600)
            assert flags & os.O_NONBLOCK
        return original_open(path, flags, *args, **kwargs)

    monkeypatch.setattr(custody.os, "open", replace_before_open)
    with project_lock(destination) as lease:
        with pytest.raises(CustodyError):
            await asyncio.wait_for(retain(directory, receipt, lease), timeout=2)


async def test_existing_custody_is_preserved_and_no_new_acceptance_is_created(tmp_path):
    directory, receipt, destination = await admitted(tmp_path)
    root = directory / "native-source"
    root.mkdir(mode=0o700)
    (root / "retained").write_bytes(b"existing evidence")
    with project_lock(destination) as lease:
        with pytest.raises(FileExistsError):
            await retain(directory, receipt, lease)
        def not_current():
            raise DraftConflict("Accepted owner selection changed")
        with pytest.raises(DraftConflict):
            await retain(directory, receipt, lease, name="not-current", current=not_current)
    assert (root / "retained").read_bytes() == b"existing evidence"
    assert not (directory / "not-current").exists()


async def test_owner_change_after_worker_prevents_returned_carrier(tmp_path):
    directory, receipt, destination = await admitted(tmp_path)
    calls = 0
    def owner_current():
        nonlocal calls
        calls += 1
        if calls > 1:
            raise DraftConflict("Current owner selection changed after projection")
    with project_lock(destination) as lease:
        with pytest.raises(DraftConflict):
            await retain(directory, receipt, lease, current=owner_current)
    assert (directory / "native-source/app.py").is_file()


async def test_cancelled_projection_joins_worker_and_preserves_private_evidence(tmp_path, monkeypatch):
    directory, receipt, destination = await admitted(tmp_path)
    entered, release, finished = threading.Event(), threading.Event(), threading.Event()
    original = custody._retain
    def delayed(*args):
        entered.set()
        assert release.wait(3)
        try:
            return original(*args)
        finally:
            finished.set()
    monkeypatch.setattr(custody, "_retain", delayed)
    with project_lock(destination) as lease:
        task = asyncio.create_task(retain(directory, receipt, lease))
        for _ in range(30000):
            if entered.is_set():
                break
            await asyncio.sleep(0.001)
        assert entered.is_set()
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        assert not task.done()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert finished.is_set()
        lease.require_current()
    assert (directory / "native-source/app.py").is_file()


async def test_directory_replacement_does_not_chmod_or_write_foreign_state(tmp_path, monkeypatch):
    directory, receipt, destination = await admitted(tmp_path)
    original_open = custody.os.open
    replaced = False
    def replace_after_mkdir(path, flags, *args, **kwargs):
        nonlocal replaced
        if path == "native-source" and flags & os.O_DIRECTORY and not replaced:
            replaced = True
            root = directory / path
            root.rename(directory / "created-root-evidence")
            root.mkdir(mode=0o755)
            root.chmod(0o755)
            (root / "foreign").write_bytes(b"preserve this directory")
        return original_open(path, flags, *args, **kwargs)
    monkeypatch.setattr(custody.os, "open", replace_after_mkdir)
    with project_lock(destination) as lease:
        with pytest.raises(CustodyError, match="root was replaced"):
            await retain(directory, receipt, lease)
    assert stat.S_IMODE((directory / "native-source").stat().st_mode) == 0o755
    assert (directory / "native-source/foreign").read_bytes() == b"preserve this directory"
    assert set((directory / "native-source").iterdir()) == {directory / "native-source/foreign"}


@pytest.mark.parametrize("nested", [False, True], ids=["root", "nested"])
async def test_same_private_mode_directory_replacement_after_creation_observation_is_refused(tmp_path, monkeypatch, nested):
    directory, receipt, destination = await admitted(tmp_path)
    original_open = custody.os.open
    replaced = False
    selected_name = "application" if nested else "native-source"
    selected_path = directory / "native-source" / "application" if nested else directory / "native-source"
    def replace_observed_creation(path, flags, *args, **kwargs):
        nonlocal replaced
        if path == selected_name and flags & os.O_DIRECTORY and not replaced:
            # Root traversal also opens installed application directories by
            # component. Select the actual private creation's parent descriptor.
            parent = kwargs.get("dir_fd")
            expected_parent = selected_path.parent
            info = os.fstat(parent)
            if expected_parent.exists() and (info.st_dev, info.st_ino) == (expected_parent.stat().st_dev, expected_parent.stat().st_ino):
                replaced = True
                selected_path.rename(selected_path.with_name(selected_path.name + "-created-evidence"))
                selected_path.mkdir(mode=0o700)
                (selected_path / "foreign").write_bytes(b"private foreign state")
        return original_open(path, flags, *args, **kwargs)
    monkeypatch.setattr(custody.os, "open", replace_observed_creation)
    with project_lock(destination) as lease:
        with pytest.raises(CustodyError, match="was replaced"):
            await retain(directory, receipt, lease)
    assert replaced
    assert stat.S_IMODE(selected_path.stat().st_mode) == 0o700
    assert (selected_path / "foreign").read_bytes() == b"private foreign state"
    assert set(selected_path.iterdir()) == {selected_path / "foreign"}


async def test_type_changed_receipt_binding_fails_before_private_copy(tmp_path):
    directory, receipt, destination = await admitted(tmp_path)
    update_receipt(directory, guide_version=True)
    with project_lock(destination) as lease:
        with pytest.raises(CustodyError, match="different application"):
            await retain(directory, receipt, lease)
    assert not (directory / "native-source").exists()


async def test_source_mutation_during_private_write_is_refused_after_handoff(tmp_path, monkeypatch):
    directory, receipt, destination = await admitted(tmp_path)
    original_write = custody._write
    def replace_source_after_write(*args):
        result = original_write(*args)
        (destination / "app.py").write_bytes(b"newer installed source\n")
        return result
    monkeypatch.setattr(custody, "_write", replace_source_after_write)
    with project_lock(destination) as lease:
        with pytest.raises(CustodyError, match="source file changed"):
            await retain(directory, receipt, lease)
    assert (directory / "native-source/app.py").is_file()


async def test_original_package_replacement_at_directory_fsync_is_refused(tmp_path, monkeypatch):
    directory, receipt, destination = await admitted(tmp_path)
    owner_identity = (directory.stat().st_dev, directory.stat().st_ino)
    original_sync = custody.os.fsync
    replaced = False
    def replace_at_final_sync(descriptor):
        nonlocal replaced
        info = os.fstat(descriptor)
        if (info.st_dev, info.st_ino) == owner_identity and not replaced:
            replaced = True
            path = directory / "package.zip"
            payload = path.read_bytes()
            path.unlink()
            path.write_bytes(payload)
            path.chmod(0o600)
        return original_sync(descriptor)
    monkeypatch.setattr(custody.os, "fsync", replace_at_final_sync)
    with project_lock(destination) as lease:
        with pytest.raises(CustodyError, match="source file changed"):
            await retain(directory, receipt, lease)
    assert replaced


async def test_final_filesystem_check_is_off_loop_and_cancellation_joins_it(tmp_path, monkeypatch):
    directory, receipt, destination = await admitted(tmp_path)
    entered, release, finished = threading.Event(), threading.Event(), threading.Event()
    original = custody.AcceptedApplicationSource._filesystem
    checks = 0
    def blocked_final_check(self):
        nonlocal checks
        checks += 1
        if checks == 2:
            entered.set()
            assert release.wait(3)
        try:
            return original(self)
        finally:
            if checks == 2:
                finished.set()
    monkeypatch.setattr(custody.AcceptedApplicationSource, "_filesystem", blocked_final_check)
    with project_lock(destination) as lease:
        task = asyncio.create_task(retain(directory, receipt, lease))
        for _ in range(30000):
            if entered.is_set():
                break
            await asyncio.sleep(0.001)
        assert entered.is_set(), "The event loop must remain responsive during final source validation"
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        assert not task.done()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert finished.is_set()
        lease.require_current()
    assert (directory / "native-source/app.py").is_file()


async def test_split_currentness_keeps_owner_callback_on_loop_and_filesystem_off_loop(tmp_path, monkeypatch):
    directory, receipt, destination = await admitted(tmp_path)
    owner_thread, owner_loop = threading.get_ident(), asyncio.get_running_loop()
    owner_calls, filesystem_threads = [], []
    original = custody.AcceptedApplicationSource._filesystem

    def owner_current():
        assert threading.get_ident() == owner_thread
        assert asyncio.get_running_loop() is owner_loop
        owner_calls.append(threading.get_ident())

    def filesystem(self):
        filesystem_threads.append(threading.get_ident())
        assert threading.get_ident() != owner_thread
        return original(self)

    monkeypatch.setattr(custody.AcceptedApplicationSource, "_filesystem", filesystem)
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease, current=owner_current)
        try:
            before = len(filesystem_threads)
            source.require_owner_current()
            assert len(filesystem_threads) == before
            callbacks = len(owner_calls)
            _, cancelled = await custody._joined_filesystem(source.require_filesystem_current)
            assert not cancelled
            assert len(filesystem_threads) == before + 1
            assert len(owner_calls) == callbacks
            source.require_owner_current()
            callbacks = len(owner_calls)
            await source.require_current_async()
            assert len(owner_calls) == callbacks + 2
            assert len(filesystem_threads) == before + 2
        finally:
            source.close()


async def test_filesystem_component_refuses_unrelated_installed_mutation_with_unchanged_native_input(tmp_path):
    raw = b'{"policy":"unchanged"}\n'
    directory, receipt, destination = await admitted(tmp_path,
        extra_files={"application/tool-policy.json": raw})
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease)
        try:
            native_input = source.exact_file("application/tool-policy.json")
            installed = destination / "app.py"
            original = installed.read_bytes()
            installed.write_bytes(b"unrelated installed source changed\n")
            assert native_input.read() == raw
            source.require_owner_current()
            with pytest.raises(CustodyError, match="source file changed"):
                await custody._joined_filesystem(source.require_filesystem_current)
            installed.write_bytes(original)
            assert native_input.read() == raw
            for check in (source.require_owner_current, source.require_filesystem_current,
                          source.require_current, source.require_current_async):
                with pytest.raises(CustodyError, match="no longer current"):
                    if asyncio.iscoroutinefunction(check):
                        await check()
                    elif check == source.require_filesystem_current:
                        await custody._joined_filesystem(check)
                    else:
                        check()
        finally:
            source.close()


@pytest.mark.parametrize("component", ["require_owner_current", "require_filesystem_current"])
@pytest.mark.parametrize("change", ["closed", "lease", "root"])
async def test_split_components_refuse_closed_lease_or_changed_root(tmp_path, component, change):
    directory, receipt, destination = await admitted(tmp_path)
    source = None

    async def observe():
        check = getattr(source, component)
        if component == "require_filesystem_current":
            await custody._joined_filesystem(check)
        else:
            check()

    try:
        with project_lock(destination) as original:
            class GeneratedLease:
                root = original.root
                require_current = staticmethod(original.require_current)
            lease = GeneratedLease()
            source = await retain(directory, receipt, lease)
            if change == "closed":
                source.close()
            elif change == "root":
                lease.root = tmp_path / "another-project"
            if change != "lease":
                with pytest.raises(CustodyError):
                    await observe()
                lease.root = original.root
                with pytest.raises(CustodyError, match="no longer current"):
                    await observe()
        if change == "lease":
            with pytest.raises(RuntimeError, match="lock is no longer current"):
                await observe()
            with pytest.raises(CustodyError, match="no longer current"):
                await observe()
    finally:
        if source is not None:
            source.close()


@pytest.mark.parametrize("mutate", [False, True], ids=["current", "invalidated"])
async def test_cancelled_currentness_joins_worker_and_keeps_its_invalidation(tmp_path, monkeypatch, mutate):
    directory, receipt, destination = await admitted(tmp_path)
    entered, release, finished = threading.Event(), threading.Event(), threading.Event()
    with project_lock(destination) as lease:
        source = await retain(directory, receipt, lease)
        original = source._filesystem

        def delayed():
            entered.set()
            assert release.wait(3)
            try:
                return original()
            finally:
                finished.set()

        monkeypatch.setattr(source, "_filesystem", delayed)
        try:
            task = asyncio.create_task(source.require_current_async())
            for _ in range(30000):
                if entered.is_set():
                    break
                await asyncio.sleep(0.001)
            assert entered.is_set()
            task.cancel()
            await asyncio.sleep(0)
            task.cancel()
            assert not task.done()
            if mutate:
                (destination / "app.py").write_bytes(b"changed during cancelled validation\n")
            release.set()
            with pytest.raises(asyncio.CancelledError):
                await task
            assert finished.is_set()
            assert source._invalidated is mutate
            if mutate:
                with pytest.raises(CustodyError, match="no longer current"):
                    source.require_owner_current()
            else:
                source.require_owner_current()
                await source.require_current_async()
            lease.require_current()
        finally:
            release.set()
            source.close()
