"""Stopped runtime byte retention and official native receipt boundaries."""
import hashlib
import json
import os
from pathlib import Path
import stat
from types import SimpleNamespace

import pytest

from launchkit import runtime_dependencies as runtime


TARGET = "aarch64-apple-darwin"


def native(directory, version="0.8.42"):
    directory.mkdir(mode=0o700, exist_ok=True)
    for name, prefix in runtime.BINARIES.items():
        payload = (version + ":" + name).encode()
        executable = directory / name
        executable.write_bytes(payload)
        executable.chmod(0o700)
        receipt = directory / (name + ".receipt.json")
        receipt.write_text(json.dumps({"version": version, "archive": f"{prefix}-{version}-{TARGET}.tar.gz",
            "release_url": f"https://github.com/lukacf/meerkat-mobkit/releases/download/v{version}",
            "archive_sha256": "a" * 64, "binary_sha256": hashlib.sha256(payload).hexdigest()}))
        receipt.chmod(0o600)


@pytest.fixture
def installed(tmp_path, monkeypatch):
    monkeypatch.setattr(runtime, "platform_target", lambda: TARGET)
    destination, transaction = tmp_path / "application", tmp_path / "transaction"
    destination.mkdir(mode=0o700)
    transaction.mkdir(mode=0o700)
    native(destination / ".bin")
    venv = destination / ".venv"
    (venv / "bin").mkdir(parents=True)
    (venv / "lib/site-packages/meerkat_mobkit").mkdir(parents=True)
    (venv / "lib/site-packages/meerkat_mobkit/__init__.py").write_text("# Previous installed SDK bytes.\n")
    (venv / "lib/site-packages/empty").touch()
    (venv / "pyvenv.cfg").write_text("version = 3.14.7\n")
    for path in (venv, *venv.rglob("*")):
        path.chmod(0o755 if path.is_dir() else 0o644)
    interpreter = tmp_path / "python3.14"
    interpreter.write_text("Test executable bytes, never executed.\n")
    interpreter.chmod(0o700)
    (venv / "bin/python3.14").symlink_to(interpreter)
    (venv / "bin/python3").symlink_to("python3.14")
    (venv / "bin/python").symlink_to("python3")
    return destination, transaction


def read_inventory(transaction):
    return json.loads((transaction / runtime.INVENTORY).read_text())


def update_receipt(destination, name, change):
    path = destination / ".bin" / (name + ".receipt.json")
    value = json.loads(path.read_text())
    change(value)
    path.write_text(json.dumps(value))


def test_retain_preserves_exact_inodes_bytes_modes_and_private_inventory(installed):
    destination, transaction = installed
    previous = runtime.verify_runtime(destination, "0.8.42")
    inodes = {name: (destination / name).stat().st_ino for name in runtime.ROOTS}
    proof = runtime.retain_runtime(destination, transaction)
    assert proof["retained"] is True and proof["previous_native"] == previous
    assert proof["inventory_sha256"] == hashlib.sha256((transaction / runtime.INVENTORY).read_bytes()).hexdigest()
    inventory = read_inventory(transaction)
    assert inventory["previous_native"] == previous
    assert set(inventory["roots"]) == set(runtime.ROOTS)
    for name in runtime.ROOTS:
        assert not (destination / name).exists()
        assert (transaction / runtime.BACKUP / name).stat().st_ino == inodes[name]
    assert runtime.verify_runtime(transaction / runtime.BACKUP, "0.8.42") == previous
    assert inventory["roots"][".venv"][".venv/bin/python"]["target"] == "python3"
    assert (transaction / runtime.INVENTORY).stat().st_mode & 0o777 == 0o600
    assert (transaction / runtime.COMPLETED).stat().st_mode & 0o777 == 0o600
    assert (transaction / runtime.BACKUP).stat().st_mode & 0o777 == 0o700


def test_repeat_keeps_previous_backup_and_never_labels_candidate_as_previous(installed):
    destination, transaction = installed
    first = runtime.retain_runtime(destination, transaction)
    journal = {name: (transaction / name).read_bytes() for name in (runtime.INVENTORY, runtime.COMPLETED)}
    previous_inode = (transaction / runtime.BACKUP / ".bin").stat().st_ino
    native(destination / ".bin", "0.8.46")
    (destination / ".venv").mkdir()
    (destination / ".venv/candidate").write_text("New SDK installation.")
    assert runtime.retain_runtime(destination, transaction) == first
    assert runtime.verify_runtime(destination, "0.8.46")["core_version"] == "0.8.51"
    assert (transaction / runtime.BACKUP / ".bin").stat().st_ino == previous_inode
    assert all((transaction / name).read_bytes() == value for name, value in journal.items())
    assert (destination / ".venv/candidate").read_text() == "New SDK installation."


@pytest.mark.parametrize("interrupt_after", [0, 1, 2])
def test_interrupted_retention_resumes_only_exact_old_locations(installed, monkeypatch, interrupt_after):
    destination, transaction = installed
    original = runtime._rename_root
    calls = []
    def interrupted(source, retained):
        if len(calls) == interrupt_after:
            raise InterruptedError("Simulated worker interruption.")
        original(source, retained)
        calls.append(source.name)
    if interrupt_after < 2:
        monkeypatch.setattr(runtime, "_rename_root", interrupted)
    else:
        create = runtime._create_json
        def before_complete(path, value):
            if path.name == runtime.COMPLETED:
                raise InterruptedError("Simulated after both moves.")
            return create(path, value)
        monkeypatch.setattr(runtime, "_create_json", before_complete)
    with pytest.raises(InterruptedError):
        runtime.retain_runtime(destination, transaction)
    inventory = (transaction / runtime.INVENTORY).read_bytes()
    assert not (transaction / runtime.COMPLETED).exists()
    monkeypatch.undo()
    monkeypatch.setattr(runtime, "platform_target", lambda: TARGET)
    proof = runtime.retain_runtime(destination, transaction)
    assert proof["retained"] is True
    assert (transaction / runtime.INVENTORY).read_bytes() == inventory
    assert all((transaction / runtime.BACKUP / name).exists() and not (destination / name).exists() for name in runtime.ROOTS)


@pytest.mark.parametrize("authority", [runtime.INVENTORY, runtime.COMPLETED])
def test_failed_publication_write_leaves_no_partial_authority_and_retry_converges(installed, monkeypatch, authority):
    destination, transaction = installed
    original_create, original_fsync = runtime._create_json, runtime.os.fsync
    writing = []
    def create(path, value):
        writing.append(path.name)
        try:
            return original_create(path, value)
        finally:
            writing.pop()
    def fail_write(descriptor):
        if writing == [authority] and stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise InterruptedError("Simulated interrupted authority write.")
        return original_fsync(descriptor)
    monkeypatch.setattr(runtime, "_create_json", create)
    monkeypatch.setattr(runtime.os, "fsync", fail_write)
    with pytest.raises(InterruptedError):
        runtime.retain_runtime(destination, transaction)
    assert not (transaction / authority).exists()
    assert not list(transaction.glob(".*.publish-*"))
    if authority == runtime.INVENTORY:
        assert all((destination / name).exists() for name in runtime.ROOTS)
    else:
        assert all((transaction / runtime.BACKUP / name).exists() for name in runtime.ROOTS)
        assert (transaction / runtime.INVENTORY).exists()
    monkeypatch.setattr(runtime, "_create_json", original_create)
    monkeypatch.setattr(runtime.os, "fsync", original_fsync)
    assert runtime.retain_runtime(destination, transaction)["retained"] is True


@pytest.mark.parametrize("authority", [runtime.INVENTORY, runtime.COMPLETED])
def test_visible_authority_with_exact_leftover_publication_temp_resumes(installed, monkeypatch, authority):
    destination, transaction = installed
    original = runtime._unlink_publication
    def after_publish(path, identity):
        if path.name.startswith("." + authority + ".publish-"):
            raise InterruptedError("Simulated crash after atomic publication.")
        return original(path, identity)
    monkeypatch.setattr(runtime, "_unlink_publication", after_publish)
    with pytest.raises(InterruptedError):
        runtime.retain_runtime(destination, transaction)
    canonical = transaction / authority
    content = canonical.read_bytes()
    assert canonical.stat().st_nlink == 2
    assert len(list(transaction.glob("." + authority + ".publish-*"))) == 1
    monkeypatch.setattr(runtime, "_unlink_publication", original)
    assert runtime.retain_runtime(destination, transaction)["retained"] is True
    assert canonical.read_bytes() == content and canonical.stat().st_nlink == 1
    assert not list(transaction.glob(".*.publish-*"))


@pytest.mark.parametrize("link_name", ["unrelated-hardlink", ".runtime-retained.json.publish-" + "a" * 32 + "-" + "f" * 64])
def test_unrelated_authority_hardlink_is_preserved_and_never_reconciled(installed, link_name):
    destination, transaction = installed
    runtime.retain_runtime(destination, transaction)
    canonical, link = transaction / runtime.COMPLETED, transaction / link_name
    os.link(canonical, link)
    content = canonical.read_bytes()
    with pytest.raises(runtime.RuntimeDependencyError, match="hardlink"):
        runtime.retain_runtime(destination, transaction)
    assert link.exists() and canonical.read_bytes() == content and canonical.stat().st_nlink == 2


def test_atomic_authority_publication_never_overwrites_existing_bytes(installed):
    _, transaction = installed
    path = transaction / runtime.INVENTORY
    path.write_text("Previously saved authority bytes.")
    path.chmod(0o600)
    with pytest.raises(FileExistsError):
        runtime._create_json(path, {"new": "unaccepted"})
    assert path.read_text() == "Previously saved authority bytes."
    assert not list(transaction.glob(".*.publish-*"))


def test_partial_backup_refuses_premature_candidate_root(installed, monkeypatch):
    destination, transaction = installed
    original = runtime._rename_root
    def first_only(source, retained):
        if source.name == ".venv":
            raise InterruptedError
        original(source, retained)
    monkeypatch.setattr(runtime, "_rename_root", first_only)
    with pytest.raises(InterruptedError):
        runtime.retain_runtime(destination, transaction)
    native(destination / ".bin", "0.8.46")
    with pytest.raises(runtime.RuntimeDependencyError, match="distinguish"):
        runtime.retain_runtime(destination, transaction)
    assert runtime.verify_runtime(transaction / runtime.BACKUP, "0.8.42")["version"] == "0.8.42"
    assert (destination / ".venv").exists()


def test_unmoved_original_mutation_refuses_without_discarding_any_root(installed, monkeypatch):
    destination, transaction = installed
    original = runtime._rename_root
    def first_only(source, retained):
        if source.name == ".venv":
            raise InterruptedError
        original(source, retained)
    monkeypatch.setattr(runtime, "_rename_root", first_only)
    with pytest.raises(InterruptedError):
        runtime.retain_runtime(destination, transaction)
    (destination / ".venv/pyvenv.cfg").write_text("Changed original SDK context.")
    with pytest.raises(runtime.RuntimeDependencyError, match="inventory changed"):
        runtime.retain_runtime(destination, transaction)
    assert (transaction / runtime.BACKUP / ".bin/rpc_gateway").read_bytes().startswith(b"0.8.42")
    assert (destination / ".venv/pyvenv.cfg").read_text() == "Changed original SDK context."


@pytest.mark.parametrize("change", ["file", "mode", "hardlink", "missing"])
def test_changed_completed_backup_never_replaces_candidate(installed, change):
    destination, transaction = installed
    runtime.retain_runtime(destination, transaction)
    original = transaction / runtime.BACKUP / ".venv/pyvenv.cfg"
    if change == "file":
        original.write_text("Altered old runtime.")
    elif change == "mode":
        original.chmod(0o600)
    elif change == "hardlink":
        os.link(original, transaction / "unrelated-hardlink")
    else:
        original.unlink()
    native(destination / ".bin", "0.8.46")
    (destination / ".venv").mkdir()
    with pytest.raises(runtime.RuntimeDependencyError):
        runtime.retain_runtime(destination, transaction)
    assert runtime.verify_runtime(destination, "0.8.46")["version"] == "0.8.46"
    assert (transaction / runtime.BACKUP / ".bin").exists()


@pytest.mark.parametrize("extra", ["hardlink", "fifo", "directory-link", "arbitrary-link", "escaped-interpreter"])
def test_unexpected_runtime_paths_refuse_before_inventory_or_moves(installed, tmp_path, extra):
    destination, transaction = installed
    path = destination / ".venv" / extra
    if extra == "hardlink":
        os.link(destination / ".venv/pyvenv.cfg", path)
    elif extra == "fifo":
        os.mkfifo(path)
    elif extra == "directory-link":
        path.symlink_to(destination / ".venv/lib", target_is_directory=True)
    elif extra == "arbitrary-link":
        path.symlink_to(tmp_path / "python3.14")
    else:
        path = destination / ".venv/bin/python3.14"
        path.unlink()
        path.symlink_to("../../python3.14")
    with pytest.raises(runtime.RuntimeDependencyError):
        runtime.retain_runtime(destination, transaction)
    assert not (transaction / runtime.INVENTORY).exists()
    assert all((destination / name).exists() for name in runtime.ROOTS)


@pytest.mark.parametrize("target", ["unowned", "other-filesystem"])
def test_unowned_runtime_and_cross_filesystem_transaction_refuse_before_moves(installed, monkeypatch, target):
    destination, transaction = installed
    selected = destination / ".venv" if target == "unowned" else transaction
    original = Path.lstat
    def changed(path):
        info = original(path)
        if path == selected:
            fields = {name: getattr(info, name) for name in dir(info) if name.startswith("st_")}
            fields["st_uid" if target == "unowned" else "st_dev"] += 1
            return SimpleNamespace(**fields)
        return info
    monkeypatch.setattr(Path, "lstat", changed)
    with pytest.raises(runtime.RuntimeDependencyError, match="owned"):
        runtime.retain_runtime(destination, transaction)
    assert not (transaction / runtime.INVENTORY).exists()
    assert all((destination / name).exists() for name in runtime.ROOTS)


def test_standard_linux_lib64_alias_is_retained_without_following_it(installed):
    destination, transaction = installed
    (destination / ".venv/lib64").symlink_to("lib", target_is_directory=True)
    runtime.retain_runtime(destination, transaction)
    entries = read_inventory(transaction)["roots"][".venv"]
    assert entries[".venv/lib64"]["kind"] == "venv-library-link"
    assert entries[".venv/lib64"]["target"] == "lib"
    assert not any(name.startswith(".venv/lib64/") for name in entries)


def test_python314_utf8_alias_is_retained_and_verified_on_resume(installed):
    destination, transaction = installed
    alias = destination / ".venv/bin" / runtime.PYTHON314_ALIAS
    alias.symlink_to("python3.14")
    original_inode = alias.lstat().st_ino
    first = runtime.retain_runtime(destination, transaction)
    entry = read_inventory(transaction)["roots"][".venv"][".venv/bin/" + runtime.PYTHON314_ALIAS]
    assert entry["kind"] == "interpreter-link" and entry["target"] == "python3.14"
    retained = transaction / runtime.BACKUP / ".venv/bin" / runtime.PYTHON314_ALIAS
    assert retained.lstat().st_ino == original_inode
    assert retained.resolve(strict=True).name == "python3.14"
    assert runtime.retain_runtime(destination, transaction) == first


@pytest.mark.parametrize("target", ["python3", "python3.13", "./python3.14", "../bin/python3.14", "absolute"])
def test_python314_utf8_alias_wrong_target_refuses_before_move(installed, tmp_path, target):
    destination, transaction = installed
    selected = str(tmp_path / "python3.14") if target == "absolute" else target
    (destination / ".venv/bin" / runtime.PYTHON314_ALIAS).symlink_to(selected)
    with pytest.raises(runtime.RuntimeDependencyError, match="point exactly"):
        runtime.retain_runtime(destination, transaction)
    assert not (transaction / runtime.INVENTORY).exists()
    assert all((destination / name).exists() for name in runtime.ROOTS)


@pytest.mark.parametrize("change", ["missing", "directory", "not-executable"])
def test_python314_utf8_alias_requires_an_ordinary_final_executable(installed, tmp_path, change):
    destination, transaction = installed
    (destination / ".venv/bin" / runtime.PYTHON314_ALIAS).symlink_to("python3.14")
    interpreter = tmp_path / "python3.14"
    if change == "missing":
        interpreter.unlink()
    elif change == "directory":
        interpreter.unlink()
        interpreter.mkdir()
    else:
        interpreter.chmod(0o600)
    with pytest.raises(runtime.RuntimeDependencyError, match="ordinary Python executable"):
        runtime.retain_runtime(destination, transaction)
    assert not (transaction / runtime.INVENTORY).exists()
    assert all((destination / name).exists() for name in runtime.ROOTS)


def test_similar_unicode_interpreter_alias_is_not_permitted(installed):
    destination, transaction = installed
    (destination / ".venv/bin/\u03c0thon").symlink_to("python3.14")
    with pytest.raises(runtime.RuntimeDependencyError, match="normal Python interpreter links"):
        runtime.retain_runtime(destination, transaction)
    assert not (transaction / runtime.INVENTORY).exists()


@pytest.mark.parametrize("target", ["../lib", "lib/site-packages", "/tmp", "bin"])
def test_lib64_alias_wrong_target_refuses_before_move(installed, target):
    destination, transaction = installed
    (destination / ".venv/lib64").symlink_to(target, target_is_directory=True)
    with pytest.raises(runtime.RuntimeDependencyError, match="exactly"):
        runtime.retain_runtime(destination, transaction)
    assert not (transaction / runtime.INVENTORY).exists()


def test_precreated_nonempty_backup_is_not_adopted_as_previous(installed):
    destination, transaction = installed
    backup = transaction / runtime.BACKUP
    backup.mkdir(mode=0o700)
    (backup / ".bin").mkdir()
    with pytest.raises(runtime.RuntimeDependencyError, match="unbound retained runtime"):
        runtime.retain_runtime(destination, transaction)
    assert not (transaction / runtime.INVENTORY).exists()
    assert all((destination / name).exists() for name in runtime.ROOTS)


def test_empty_precreated_backup_does_not_bypass_previous_native_proof(installed):
    destination, transaction = installed
    (transaction / runtime.BACKUP).mkdir(mode=0o700)
    update_receipt(destination, "rpc_gateway", lambda row: row.update(source="explicit_override"))
    with pytest.raises(runtime.RuntimeDependencyError, match="exact official release"):
        runtime.retain_runtime(destination, transaction)
    assert not (transaction / runtime.INVENTORY).exists()
    assert all((destination / name).exists() for name in runtime.ROOTS)


@pytest.mark.parametrize("bound,value", [("MAX_RUNTIME_BYTES", 1), ("MAX_RUNTIME_ENTRIES", 1), ("MAX_INVENTORY_BYTES", 1)])
def test_bounds_refuse_without_moving_previous_runtime(installed, monkeypatch, bound, value):
    destination, transaction = installed
    monkeypatch.setattr(runtime, bound, value)
    with pytest.raises(runtime.RuntimeDependencyError, match="bounded"):
        runtime.retain_runtime(destination, transaction)
    assert all((destination / name).exists() for name in runtime.ROOTS)


@pytest.mark.parametrize("corruption", ["duplicate", "malformed", "foreign-destination", "changed-completion", "private-mode"])
def test_retained_journal_corruption_refuses_and_preserves_backup(installed, corruption):
    destination, transaction = installed
    runtime.retain_runtime(destination, transaction)
    path = transaction / runtime.INVENTORY
    original_native = (transaction / runtime.BACKUP / ".bin/rpc_gateway").read_bytes()
    if corruption == "duplicate":
        path.write_text(path.read_text().replace('"format":', '"format":"other","format":', 1))
    elif corruption == "malformed":
        path.write_text("{bad JSON")
    elif corruption == "foreign-destination":
        value = json.loads(path.read_text())
        value["destination"] += "/other"
        path.write_text(json.dumps(value))
    elif corruption == "changed-completion":
        marker = transaction / runtime.COMPLETED
        value = json.loads(marker.read_text())
        value["inventory_sha256"] = "f" * 64
        marker.write_text(json.dumps(value))
    else:
        path.chmod(0o644)
    with pytest.raises(runtime.RuntimeDependencyError):
        runtime.retain_runtime(destination, transaction)
    assert (transaction / runtime.BACKUP / ".bin/rpc_gateway").read_bytes() == original_native


@pytest.mark.parametrize("name", runtime.BINARIES)
@pytest.mark.parametrize("field,value", [("source", "explicit_override"), ("version", "0.8.46"),
    ("archive", "mobkit-rpc-gateway-0.8.42-x86_64-apple-darwin.tar.gz"),
    ("release_url", "https://other.test/release"), ("archive_sha256", "broken")])
def test_both_native_receipts_need_exact_official_version_platform_and_source(installed, name, field, value):
    destination, _ = installed
    update_receipt(destination, name, lambda row: row.update({field: value}))
    with pytest.raises(runtime.RuntimeDependencyError, match="exact official release"):
        runtime.verify_runtime(destination, "0.8.42")


@pytest.mark.parametrize("name", runtime.BINARIES)
@pytest.mark.parametrize("change", ["bytes", "private-receipt", "executable-mode", "hardlink", "symlink"])
def test_both_native_executables_and_receipts_require_owned_private_exact_bytes(installed, tmp_path, name, change):
    destination, _ = installed
    binary = destination / ".bin" / name
    receipt = binary.with_name(name + ".receipt.json")
    if change == "bytes":
        binary.write_bytes(b"Changed native bytes.")
    elif change == "private-receipt":
        receipt.chmod(0o644)
    elif change == "executable-mode":
        binary.chmod(0o600)
    elif change == "hardlink":
        os.link(binary, tmp_path / "other-binary")
    else:
        target = tmp_path / "other-binary"
        binary.rename(target)
        binary.symlink_to(target)
    with pytest.raises((runtime.RuntimeDependencyError, OSError)):
        runtime.verify_runtime(destination, "0.8.42")


@pytest.mark.parametrize("entry", ["rpc_gateway", "rpc_gateway.receipt.json", "mobkit_gateway", "mobkit_gateway.receipt.json"])
def test_special_native_files_refuse_without_opening_or_waiting(installed, monkeypatch, entry):
    destination, _ = installed
    path = destination / ".bin" / entry
    path.unlink()
    os.mkfifo(path)
    original = runtime.os.open
    def forbid_special_open(value, *args, **kwargs):
        assert Path(value) != path, "A special native file must be rejected before open."
        return original(value, *args, **kwargs)
    monkeypatch.setattr(runtime.os, "open", forbid_special_open)
    with pytest.raises(runtime.RuntimeDependencyError, match="ordinary file"):
        runtime.verify_runtime(destination, "0.8.42")


def test_qualified_pair_accepts_generated_and_seeded_receipts_but_refuses_wrong_core(installed):
    destination, _ = installed
    native(destination / ".bin", "0.8.46")
    assert runtime.verify_runtime(destination, "0.8.46")["core_version"] == "0.8.51"
    update_receipt(destination, "rpc_gateway", lambda row: row.update(core_version="0.8.51"))
    assert runtime.verify_runtime(destination, "0.8.46")["core_version"] == "0.8.51"
    update_receipt(destination, "rpc_gateway", lambda row: row.update(core_version="0.8.50"))
    with pytest.raises(runtime.RuntimeDependencyError, match="exact official release"):
        runtime.verify_runtime(destination, "0.8.46")


@pytest.mark.parametrize("version", ["0.8.46", "0.8.47", "0.8.48"])
def test_same_core_native_receipts_remain_exact_and_do_not_guess_a_version(installed, version):
    destination, _ = installed
    native(destination / ".bin", version)
    proof = runtime.verify_runtime(destination, version)
    assert proof["version"] == version and proof["core_version"] == "0.8.51"
    with pytest.raises(runtime.RuntimeDependencyError):
        runtime.verify_runtime(destination, "0.8.47" if version == "0.8.46" else "0.8.46")
    update_receipt(destination, "rpc_gateway", lambda value: value.update(core_version="0.8.50"))
    with pytest.raises(runtime.RuntimeDependencyError):
        runtime.verify_runtime(destination, version)


def test_same_core_046_predecessor_retention_never_relabels_candidate_bytes(installed):
    destination, transaction = installed
    native(destination / ".bin", "0.8.46")
    previous = runtime.verify_runtime(destination, "0.8.46")
    inodes = {name: (destination / name).stat().st_ino for name in runtime.ROOTS}
    saved = runtime.retain_runtime(destination, transaction, expected_version="0.8.46")
    assert saved["previous_native"] == previous
    inventory = (transaction / runtime.INVENTORY).read_bytes()
    native(destination / ".bin", "0.8.47")
    (destination / ".venv").mkdir(mode=0o700)
    assert runtime.retain_runtime(destination, transaction, expected_version="0.8.46") == saved
    assert (transaction / runtime.INVENTORY).read_bytes() == inventory
    assert all((transaction / runtime.BACKUP / name).stat().st_ino == inode for name, inode in inodes.items())
    assert runtime.verify_runtime(destination, "0.8.47")["version"] == "0.8.47"
    with pytest.raises(runtime.RuntimeDependencyError):
        runtime.retain_runtime(destination, transaction)


def test_047_predecessor_retention_keeps_exact_original_bytes_after_048_candidate_exists(installed):
    destination, transaction = installed
    native(destination / ".bin", "0.8.47")
    previous = runtime.verify_runtime(destination, "0.8.47")
    inodes = {name: (destination / name).stat().st_ino for name in runtime.ROOTS}
    original = {path.relative_to(destination): path.read_bytes()
        for path in (destination / ".bin").iterdir()}
    saved = runtime.retain_runtime(destination, transaction, expected_version="0.8.47")
    assert saved["previous_native"] == previous
    authority = {name: (transaction / name).read_bytes()
        for name in (runtime.INVENTORY, runtime.COMPLETED)}
    native(destination / ".bin", "0.8.48")
    (destination / ".venv").mkdir(mode=0o700)
    assert runtime.retain_runtime(destination, transaction, expected_version="0.8.47") == saved
    assert all((transaction / name).read_bytes() == content for name, content in authority.items())
    assert all((transaction / runtime.BACKUP / name).stat().st_ino == inode
        for name, inode in inodes.items())
    assert all((transaction / runtime.BACKUP / path).read_bytes() == content
        for path, content in original.items())
    assert runtime.verify_runtime(destination, "0.8.48")["version"] == "0.8.48"
    with pytest.raises(runtime.RuntimeDependencyError):
        runtime.retain_runtime(destination, transaction, expected_version="0.8.48")


@pytest.mark.parametrize("change", ["candidate-source", "candidate-url", "foreign-platform",
    "wrong-core", "typed-core", "binary", "public-receipt"])
def test_048_support_never_admits_candidate_receipts_or_mismatched_runtime(installed, change):
    destination, _ = installed
    native(destination / ".bin", "0.8.48")
    receipt = destination / ".bin/rpc_gateway.receipt.json"
    if change == "binary":
        (destination / ".bin/rpc_gateway").write_bytes(b"Different unverified native bytes.")
    elif change == "public-receipt":
        receipt.chmod(0o644)
    else:
        key, value = {
            "candidate-source": ("source", "accepted-candidate"),
            "candidate-url": ("release_url", "https://api.github.com/repos/lukacf/meerkat-mobkit/actions/artifacts/11504111244/zip"),
            "foreign-platform": ("archive", "mobkit-rpc-gateway-0.8.48-x86_64-unknown-linux-gnu.tar.gz"),
            "wrong-core": ("core_version", "0.8.52"),
            "typed-core": ("core_version", True),
        }[change]
        update_receipt(destination, "rpc_gateway", lambda row: row.update({key: value}))
    with pytest.raises(runtime.RuntimeDependencyError):
        runtime.verify_runtime(destination, "0.8.48")


@pytest.mark.parametrize("version", ["0.8.49", "0.9.0", None, True])
def test_unlisted_native_release_refuses_before_reading_a_runtime(tmp_path, version):
    with pytest.raises(runtime.RuntimeDependencyError, match="not supported"):
        runtime.verify_runtime(tmp_path / "absent", version)
