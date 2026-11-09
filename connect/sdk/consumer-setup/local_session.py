#!/usr/bin/env python3
"""Product-selected local setup entry; the native MCP host owns this process."""
from __future__ import annotations

import argparse
import asyncio
from contextlib import contextmanager
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import select
import socket
import signal
import tempfile
import stat
import sys
from types import ModuleType
from uuid import uuid4


LIMIT = 2 * 1024 * 1024
HASH = re.compile(r"[0-9a-f]{64}\Z")


class LocalSetupError(ValueError):
    """A fixed product diagnostic, without package output or credentials."""


def encoded(value):
    return (json.dumps(value, sort_keys=True, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def absolute(path):
    path = Path(path)
    if not path.is_absolute() or path.parent.resolve(strict=True) != path.parent or path.name in ("", ".", ".."):
        raise LocalSetupError("local_setup_path_unavailable")
    return path


def read(path, expected=None):
    path = absolute(path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_size > LIMIT:
            raise LocalSetupError("local_setup_selection_unavailable")
        with os.fdopen(fd, "rb", closefd=False) as stream:
            raw = stream.read(LIMIT + 1)
        if len(raw) > LIMIT or expected is not None and (not isinstance(expected, str)
                or not HASH.fullmatch(expected) or digest(raw) != expected):
            raise LocalSetupError("local_setup_selection_changed")
        return raw
    finally:
        os.close(fd)


def decode(raw):
    def unique(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise LocalSetupError("local_setup_selection_unavailable")
            value[key] = item
        return value
    try:
        return json.loads(raw, object_pairs_hook=unique)
    except (ValueError, UnicodeError):
        raise LocalSetupError("local_setup_selection_unavailable") from None


def prepared_configuration(configuration, configuration_sha256, helper, helper_sha256):
    """Consume the exact existing preparer's local stdio launch, without effects."""
    configuration, helper = absolute(configuration), absolute(helper)
    read(helper, helper_sha256)
    value = decode(read(configuration, configuration_sha256))
    if (type(value) is not dict or set(value) != {"mcpServers"}
            or type(value["mcpServers"]) is not dict or set(value["mcpServers"]) != {"toolkit-setup"}):
        raise LocalSetupError("local_setup_configuration_unavailable")
    server = value["mcpServers"]["toolkit-setup"]
    if (type(server) is not dict or set(server) != {"command", "args"}
            or server["command"] != "/usr/bin/env" or type(server["args"]) is not list
            or not 7 < len(server["args"]) <= 128
            or any(type(arg) is not str or not arg or "\x00" in arg or len(arg) > 8192 for arg in server["args"])):
        raise LocalSetupError("local_setup_configuration_unavailable")
    args = server["args"]
    if (args[:3] != ["-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1"]
            or not Path(args[3]).is_absolute() or args[4:6] != ["-I", "-B"]
            or Path(args[6]) != helper.with_name("mcp_adapter.py")):
        raise LocalSetupError("local_setup_configuration_unavailable")
    flags = [arg for arg in args[7:] if arg.startswith("--")]
    if (len(flags) != len(set(flags)) or "--workspace" not in flags
            or any(flag.startswith(("--recover-", "--prior-", "--compose-"))
                   or flag == "--browser-port" for flag in flags)):
        raise LocalSetupError("local_setup_requires_fresh_local_selection")
    if "--allow-remote-recipes" in flags and not {"--local-recipes-profile", "--local-recipes-profile-sha256"} <= set(flags):
        raise LocalSetupError("local_setup_requires_pinned_recipe_profile")
    return server


def prepare_server(*, configuration, configuration_sha256, helper, helper_sha256,
                   adapter_sha256, backend_sha256, state_root):
    """Return one public native MCP selection; create no process or state."""
    server = prepared_configuration(configuration, configuration_sha256, helper, helper_sha256)
    read(absolute(helper).with_name("mcp_adapter.py"), adapter_sha256)
    read(absolute(helper).with_name("backend.py"), backend_sha256)
    root = absolute(state_root)
    if any(root == directory or root in directory.parents or directory in root.parents
           for directory in (absolute(configuration).parent, absolute(helper).parent)):
        raise LocalSetupError("local_setup_state_overlaps_selection")
    return {"name": "toolkit-setup", "command": server["command"], "args": [
        *server["args"][:6], str(helper), "--configuration", str(configuration),
        "--configuration-sha256", configuration_sha256, "--helper-sha256", helper_sha256,
        "--adapter-sha256", adapter_sha256, "--backend-sha256", backend_sha256,
        "--state-root", str(root)]}


def retained_module(name, path, raw, namespace=None):
    # The same retained-byte loading pattern as prepare_host.owner_module.
    module = ModuleType(name)
    module.__file__ = str(path)
    module.__dict__.update(namespace or {})
    previous = sys.modules.get(name)
    sys.modules[name] = module
    try:
        exec(compile(raw, str(path), "exec"), module.__dict__)
    finally:
        if previous is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = previous
    return module


def retained_adapter(helper, adapter_sha256, backend_sha256):
    """Verify both entry modules before importing either; never reopen them."""
    adapter_path, backend_path = helper.with_name("mcp_adapter.py"), helper.with_name("backend.py")
    adapter_raw = read(adapter_path, adapter_sha256)
    backend_raw = read(backend_path, backend_sha256)
    backend = retained_module("consumer_setup_local_backend", backend_path, backend_raw)
    return retained_module("consumer_setup_local_adapter", adapter_path, adapter_raw,
                           {"_retained_backend": backend})


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write_new(path, raw):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(fd, "wb", closefd=False) as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(fd)
    finally:
        os.close(fd)
    sync_directory(path.parent)


def replace(path, raw):
    temporary = path.with_name(".checkpoint-" + uuid4().hex)
    try:
        write_new(temporary, raw)
        os.replace(temporary, path)
        sync_directory(path.parent)
        if read(path) != raw:
            raise LocalSetupError("local_setup_checkpoint_changed")
    finally:
        temporary.unlink(missing_ok=True)


@contextmanager
def session_owner(root, binding):
    """One local helper lease, held through the existing adapter's full close."""
    root = absolute(root)
    try:
        root.mkdir(mode=0o700)
        sync_directory(root.parent)
        created = True
    except FileExistsError:
        created = False
    info = root.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077):
        raise LocalSetupError("local_setup_private_directory_required")
    fd = os.open(root / "owner.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise LocalSetupError("local_setup_owner_unavailable")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise LocalSetupError("local_setup_already_open") from None
        record_path = root / "session.json"
        reopened = record_path.exists() or record_path.is_symlink()
        if reopened:
            record, _ = read_session_record(root, binding)
        else:
            if not created:
                raise LocalSetupError("local_setup_previous_session_needs_review")
            record = {"schema_version": 1, "binding": binding, "workspace": "work-" + uuid4().hex}
            write_new(record_path, encoded(record))
        yield root, record, reopened
    finally:
        os.close(fd)


def read_session_record(root, binding):
    """Read the selected attempt without creating state or taking its lease."""
    root = absolute(root)
    info = root.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077):
        raise LocalSetupError("local_setup_private_directory_required")
    raw = read(root / "session.json")
    record = decode(raw)
    if (type(record) is not dict or type(record.get("schema_version")) is not int
            or record["schema_version"] not in (1, 2)
            or set(record) != ({"schema_version", "binding", "workspace"}
                              | ({"prior"} if record["schema_version"] == 2 else set()))
            or record["binding"] != binding or type(record["workspace"]) is not str
            or not re.fullmatch(r"work-[0-9a-f]{32}", record["workspace"])):
        raise LocalSetupError("local_setup_session_changed")
    if record["schema_version"] == 2:
        prior = record["prior"]
        if type(prior) is not list or not 1 <= len(prior) <= 256:
            raise LocalSetupError("local_setup_session_changed")
        names = {record["workspace"]}
        for entry in prior:
            if (type(entry) is not dict or set(entry) != {"workspace", "recovery_sha256"}
                    or type(entry["workspace"]) is not str
                    or not re.fullmatch(r"work-[0-9a-f]{32}", entry["workspace"])
                    or entry["workspace"] in names or type(entry["recovery_sha256"]) is not str
                    or not HASH.fullmatch(entry["recovery_sha256"])):
                raise LocalSetupError("local_setup_session_changed")
            names.add(entry["workspace"])
    return record, raw


def read_recovery_checkpoint(backend, root, record):
    """Validate the existing Controller checkpoint, without selecting work."""
    original = root / record["workspace"]
    recovery = checkpoint_path(root, record)
    if not recovery.exists() and not recovery.is_symlink():
        # Never scan old workspaces, infer no effects, or silently replace
        # an interrupted selection. The product may explicitly start a
        # new local session while retaining this one for diagnosis.
        raise LocalSetupError("local_setup_previous_session_needs_review")
    raw = read(recovery)
    recovered_root, entries = backend.read_recovery_selection(raw)
    if recovered_root != original:
        raise LocalSetupError("local_setup_session_changed")
    request_directory = original / "requests"
    if (not stat.S_ISDIR(request_directory.lstat().st_mode)
            or {entry.name for entry in request_directory.iterdir()} !=
            {entry["id"] + ".json" for entry in entries}):
        # Check closure only. Never discover/select a missing request from
        # the directory or silently reopen a checkpoint that omitted one
        # after failed publication. Existing owners still validate bytes.
        raise LocalSetupError("local_setup_recovery_selection_incomplete")
    return raw


def checkpoint_path(root, record):
    return root / "recovery.json" if record["schema_version"] == 1 else root / record["workspace"] / "local-recovery.json"


def prior_selections(backend, root, record):
    selections = []
    for prior in record.get("prior", []):
        raw = read_recovery_checkpoint(backend, root, {"schema_version": 2, "workspace": prior["workspace"]})
        selected_root, _ = backend.read_recovery_selection(raw)
        if digest(raw) != prior["recovery_sha256"] or selected_root != root / prior["workspace"]:
            raise LocalSetupError("local_setup_session_changed")
        selections.append(raw)
    return selections


def selected_checkpoint(backend, root, record, workspace):
    """An explicitly named round only; never discover a workspace or request."""
    if workspace == root / record["workspace"]:
        return read_recovery_checkpoint(backend, root, record)
    for prior, raw in zip(record.get("prior", []), prior_selections(backend, root, record)):
        if workspace == root / prior["workspace"]:
            return raw
    raise LocalSetupError("local_setup_attempt_mismatch")


def round_snapshot(backend, root, record, *, allow_empty=False):
    selections = prior_selections(backend, root, record)
    current = checkpoint_path(root, record)
    if current.exists() or current.is_symlink():
        selections.append(read_recovery_checkpoint(backend, root, record))
    elif allow_empty and ((root / record["workspace"] / "local-empty.json").exists()
                         or (root / record["workspace"] / "local-empty.json").is_symlink()):
        # A durable explicit empty-round receipt precedes admission. A retained
        # request without its checkpoint is an interrupted publication, not empty.
        expected = encoded({"schema_version": 1, "workspace": record["workspace"], "empty": True})
        requests = root / record["workspace"] / "requests"
        if (read(root / record["workspace"] / "local-empty.json") != expected
                or not stat.S_ISDIR(requests.lstat().st_mode) or any(requests.iterdir())):
            raise LocalSetupError("local_setup_recovery_selection_incomplete")
    else:
        raise LocalSetupError("local_setup_previous_session_needs_review")
    return tuple(selections)


def next_session_record(record, workspace, selections):
    """Retain request-bearing rounds; explicit empty rounds have no checkpoint."""
    prior = list(record.get("prior", []))
    if len(selections) == len(prior) + 1:
        prior.append({"workspace": record["workspace"],
                      "recovery_sha256": digest(selections[-1])})
    elif len(selections) != len(prior):
        raise LocalSetupError("local_setup_session_changed")
    return {"schema_version": 2 if prior else 1, "binding": record["binding"],
            "workspace": workspace, **({"prior": prior} if prior else {})}


def round_fingerprint(root, binding, selections):
    _, raw = read_session_record(root, binding)
    return {"binding": binding, "session_sha256": digest(raw),
            "checkpoints": [digest(item) for item in selections]}


def read_frame(stream, stop_fds=()):
    raw = bytearray()
    while True:
        readable, _, _ = select.select([stream, *stop_fds], [], [])
        if any(fd in readable for fd in stop_fds):
            raise LocalSetupError("local_setup_round_cancelled")
        block = os.read(stream.fileno(), 1)
        if not block or len(raw) >= LIMIT:
            raise LocalSetupError("local_setup_round_disconnected")
        raw.extend(block)
        if block == b"\n":
            return decode(bytes(raw))


def send_frame(stream, value):
    stream.sendall(encoded(value))


def offer_next_round(backend, root, binding, host_input, host_output, cancelled_fd, *, expected_round=None):
    """One live resource-owner rendezvous, joined by its invoking CLI scope.

    The CLI must already have joined the old native setup. The existing helper
    lock is an additional overlap fence. It is released before the next helper
    acquires it; no lock transfer or background service is involved.
    """
    with session_owner(root, binding) as (root, record, reopened):
        if not reopened:
            raise LocalSetupError("local_setup_previous_session_required")
        selections = round_snapshot(backend, root, record, allow_empty=True)
        # One invocation retains at most the existing Controller selection
        # limit in total. Refuse a successor before creating an unusable round;
        # exact old readback remains available at the limit.
        if sum(len(backend.read_recovery_selection(raw)[1]) for raw in selections) >= 256:
            raise LocalSetupError("local_setup_history_full")
        offer = round_fingerprint(root, binding, selections)
        # A restored host carries an observation of the exact previously closed
        # empty round. Validate it under this same lease, before any publication.
        if expected_round is not None and (selections or offer != expected_round):
            raise LocalSetupError("local_setup_round_changed")
    locator = root / "round-handoff.json"
    if locator.exists() or locator.is_symlink():
        # A killed owner can leave a locator, never reusable authority. A fresh
        # CLI invocation reserves a new preparation directory; this old saved
        # descriptor remains recovery-only and is not silently repurposed.
        raise LocalSetupError("local_setup_round_already_offered")
    locator_raw = None
    with tempfile.TemporaryDirectory(prefix="mc-round-", dir="/tmp") as temporary:
        socket_path = Path(temporary).resolve() / "owner.sock"
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            listener.bind(str(socket_path))
            os.chmod(socket_path, 0o600)
            listener.listen(1)
            locator_raw = encoded({"schema_version": 1, "socket": str(socket_path)})
            try:
                write_new(locator, locator_raw)
                host_output.write(encoded({"ready": True})); host_output.flush()
                readable, _, _ = select.select([listener, host_input, cancelled_fd], [], [])
                if listener not in readable or host_input in readable or cancelled_fd in readable:
                    raise LocalSetupError("local_setup_round_cancelled")
                connection, _ = listener.accept()
                # Exactly one connection may consume this offer, even if its
                # initialization later fails. No automatic retry is available.
                listener.close()
                with connection:
                    request = read_frame(connection, (host_input, cancelled_fd))
                    if request != offer or round_fingerprint(root, binding, selections) != offer:
                        raise LocalSetupError("local_setup_round_changed")
                    for raw in selections:
                        backend.read_recovery_selection(raw)
                    workspace = "work-" + uuid4().hex
                    send_frame(connection, {"workspace": workspace})
                    publication = read_frame(connection, (host_input, cancelled_fd))
                    current, current_raw = read_session_record(root, binding)
                    if (publication != {"published_sha256": digest(current_raw)}
                            or current != next_session_record(record, workspace, selections)
                            or round_snapshot(backend, root, current, allow_empty=True) != selections):
                        raise LocalSetupError("local_setup_round_publication_changed")
                    send_frame(connection, {"accepted": True})
                    host_output.write(encoded({"consumed": True})); host_output.flush()
                    if read_frame(host_input, (cancelled_fd,)) != {"settle": True}:
                        raise LocalSetupError("local_setup_round_cancelled")
                    host_output.write(encoded({"settled": True})); host_output.flush()
            finally:
                # Remove only the exact locator this scope published. It is
                # never an authorization token, and a stale socket cannot grant.
                if locator_raw is not None and locator.exists() and read(locator) == locator_raw:
                    locator.unlink(); sync_directory(root)


def offered_round(root, binding, selections):
    """Connect to a live offer while session_owner holds the helper lease."""
    locator = root / "round-handoff.json"
    if not locator.exists() and not locator.is_symlink():
        return None
    value = decode(read(locator))
    if (type(value) is not dict or set(value) != {"schema_version", "socket"}
            or value["schema_version"] != 1 or type(value["socket"]) is not str):
        raise LocalSetupError("local_setup_round_locator_changed")
    try:
        path = absolute(value["socket"])
        info = path.parent.lstat()
        socket_info = path.lstat()
    except FileNotFoundError:
        return None
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077
            or not stat.S_ISSOCK(socket_info.st_mode) or socket_info.st_uid != os.getuid()
            or socket_info.st_mode & 0o077):
        raise LocalSetupError("local_setup_round_locator_changed")
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connection.settimeout(30)
    try:
        connection.connect(str(path))
    except (ConnectionRefusedError, FileNotFoundError):
        connection.close()
        return None
    try:
        send_frame(connection, round_fingerprint(root, binding, selections))
        value = read_frame(connection)
        if (type(value) is not dict or set(value) != {"workspace"}
                or not re.fullmatch(r"work-[0-9a-f]{32}", value["workspace"])):
            raise LocalSetupError("local_setup_round_changed")
        return connection, value["workspace"]
    except BaseException:
        connection.close()
        raise


def controller_factory(backend, root, record, reopened, *, _shared_restore=None):
    """Keep exact requests; native-selected resource custody permits new setup."""
    original = root / record["workspace"]
    selected_record_raw = read_session_record(root, record["binding"])[1]
    selections = round_snapshot(backend, root, record, allow_empty=True) if reopened else ()
    offer = offered_round(root, record["binding"], selections) if reopened and _shared_restore is None else None
    live = offer is not None
    if _shared_restore is not None and (not reopened
            or _shared_restore["round"] != round_fingerprint(root, record["binding"], selections)):
        raise LocalSetupError("local_setup_session_changed")
    advancing = live or _shared_restore is not None
    if reopened and not advancing and not selections:
        # An empty receipt is history, never permission for a saved descriptor
        # to reopen an effect-capable Controller without its live host owner.
        raise LocalSetupError("local_setup_previous_session_needs_review")
    current = root / (offer[1] if live else "work-" + uuid4().hex) if advancing else original
    next_record = next_session_record(record, current.name, selections) if advancing else record
    recovery = checkpoint_path(root, next_record)

    def checkpoint(selection):
        raw = selection["text"].encode("utf-8")
        if selection["sha256"] != digest(raw) or selection["bytes"] != len(raw):
            raise LocalSetupError("local_setup_checkpoint_changed")
        selected_root, _ = backend.read_recovery_selection(raw)
        if selected_root != current:
            raise LocalSetupError("local_setup_session_changed")
        replace(recovery, raw)

    consumed = False
    def create(*args, **kwargs):
        nonlocal consumed
        if consumed:
            raise LocalSetupError("local_setup_round_already_consumed")
        consumed = True
        positional = list(args)
        positional[2] = root / ("readback-" + uuid4().hex) if reopened and not advancing else current
        if reopened:
            kwargs.update(_retained_selections=selections, _local_read_only=not advancing)
            if not advancing:
                kwargs["allow_node_search"] = False
                kwargs["allow_public_github_research"] = False
        if not reopened or advancing:
            kwargs["recovery_selection_writer"] = checkpoint
        owner = None
        try:
            owner = backend.Controller(*positional, **kwargs)
            if not reopened:
                # Publish before the fresh Controller is returned for admission.
                # Later missing checkpoints can be empty only while this exact
                # receipt remains intact and no retained request exists.
                write_new(current / "local-empty.json", encoded({"schema_version": 1,
                    "workspace": record["workspace"], "empty": True}))
            if advancing:
                if (round_fingerprint(root, record["binding"], selections)["session_sha256"] != digest(selected_record_raw)
                        or round_snapshot(backend, root, record, allow_empty=True) != selections):
                    raise LocalSetupError("local_setup_round_changed")
                old_checkpoint = original / "local-recovery.json"
                if record["schema_version"] == 1 and selections:
                    write_new(old_checkpoint, selections[-1])
                write_new(current / "local-empty.json", encoded({"schema_version": 1, "workspace": current.name, "empty": True}))
                published = encoded(next_record)
                replace(root / "session.json", published)
                if live:
                    send_frame(offer[0], {"published_sha256": digest(published)})
                    if read_frame(offer[0]) != {"accepted": True}:
                        raise LocalSetupError("local_setup_round_unconfirmed")
            return owner
        except BaseException:
            if owner is not None:
                owner.close_admission(); owner.join()
            raise
        finally:
            if offer is not None:
                offer[0].close()
    return create


def observe_empty_close(backend, root, binding):
    """Observation after Controller join, never native close or admission proof.

    The caller still holds session_owner. Only the existing snapshot reader
    decides whether the current round is empty; incomplete history is refused.
    """
    record, _ = read_session_record(root, binding)
    try:
        selections = round_snapshot(backend, root, record, allow_empty=True)
    except LocalSetupError:
        return
    if selections:
        return
    write_new(root / "empty-close.json", encoded({"schema_version": 1,
        "round": round_fingerprint(root, binding, selections)}))


class PhysicalLease:
    """One real process's shared custody, acquired before initialization/I/O.

    Each stdio bridge opens this independently; close_fds and arbitrary worker
    descendants are not an inheritance proof. Any entered operation remains
    outside the cold nonentry constituent, regardless of lease availability.
    """
    def __init__(self, path, expected):
        self.root_fd = self.directory_fd = self.lease_fd = None
        try:
            self.path = absolute(path)
            self.raw = read(self.path, expected)
            self.value = decode(self.raw)
            value = self.value
            if (value.get("schema_version") != 2 or not HASH.fullmatch(value.get("owner", ""))
                    or self.path.name != "shared-lifetime-" + value["owner"] + ".json"
                    or value["artifact_directory"] != str(self.path.parent)
                    or set(value["fence"]) != {"root", "directory", "lease"}):
                raise LocalSetupError("local_setup_physical_unavailable")
            self.sha = digest(self.raw)
            self.root_fd = os.open(self.path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            self.directory_name = "shared-physical-" + value["owner"]
            self.directory_path = self.path.parent / self.directory_name
            self.directory_fd = os.open(self.directory_name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                        dir_fd=self.root_fd)
            self.lease_fd = os.open("lease", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                                    dir_fd=self.directory_fd)
            fcntl.flock(self.lease_fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
            self.current()
        except BaseException:
            self.close()
            raise

    @staticmethod
    def identity(info, directory):
        if (info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != (0o700 if directory else 0o600)
                or not (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode))
                or not directory and info.st_nlink != 1):
            raise LocalSetupError("local_setup_physical_unavailable")
        return {"device": info.st_dev, "inode": info.st_ino}

    def current(self):
        if (self.identity(os.fstat(self.root_fd), True) != self.value["fence"]["root"]
                or self.identity(self.path.parent.lstat(), True) != self.value["fence"]["root"]
                or self.path.parent.resolve(strict=True) != self.path.parent
                or self.identity(os.fstat(self.directory_fd), True) != self.value["fence"]["directory"]
                or self.identity(os.stat(self.directory_name, dir_fd=self.root_fd, follow_symlinks=False), True)
                    != self.value["fence"]["directory"]
                or self.identity(os.fstat(self.lease_fd), False) != self.value["fence"]["lease"]
                or self.identity(os.stat("lease", dir_fd=self.directory_fd, follow_symlinks=False), False)
                    != self.value["fence"]["lease"] or os.fstat(self.lease_fd).st_size != 0
                or self.record_read_root(self.path.name) != self.raw):
            raise LocalSetupError("local_setup_physical_changed")
        try:
            os.stat("sealed.json", dir_fd=self.directory_fd, follow_symlinks=False)
        except FileNotFoundError:
            return
        raise LocalSetupError("local_setup_physical_closed")

    def record_read_root(self, name):
        return self._read(self.root_fd, name)

    def _read(self, directory, name):
        if not name or name in (".", "..") or "/" in name:
            raise LocalSetupError("local_setup_physical_unavailable")
        fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        try:
            info = os.fstat(fd)
            self.identity(info, False)
            if info.st_size > LIMIT:
                raise LocalSetupError("local_setup_physical_unavailable")
            with os.fdopen(os.dup(fd), "rb") as source:
                raw = source.read(LIMIT + 1)
            if len(raw) > LIMIT:
                raise LocalSetupError("local_setup_physical_unavailable")
            return raw
        finally:
            os.close(fd)

    def read(self, name):
        self.current()
        raw = self._read(self.directory_fd, name)
        self.current()
        return raw

    def save_raw(self, name, raw):
        self.current()
        if not name or name in (".", "..") or "/" in name or len(raw) > LIMIT:
            raise LocalSetupError("local_setup_physical_unavailable")
        fd = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                     0o600, dir_fd=self.directory_fd)
        try:
            self.identity(os.fstat(fd), False)
            with os.fdopen(os.dup(fd), "wb") as output:
                output.write(raw)
                output.flush()
                os.fsync(output.fileno())
            os.fsync(self.directory_fd)
        finally:
            os.close(fd)
        if self.read(name) != raw:
            raise LocalSetupError("local_setup_physical_changed")

    def save(self, name, value):
        self.save_raw(name, encoded(value))

    def startup(self, arguments, reopened):
        # This conservative eligibility guard executes before adapter imports
        # and Controller construction. Unsupported initialization still follows
        # ordinary launch semantics but can never yield cold nonentry evidence.
        file_only = {"--bundle", "--contents-sha256", "--workspace", "--families",
                     "--python", "--node", "--npm-cli", "--verifier", "--verifier-sha256"}
        flags = {arg.split("=", 1)[0] for arg in arguments if arg.startswith("--")}
        eligible = not reopened and self.value["restored"] is None and flags <= file_only
        self.save("startup-begin.json", {"schema_version": 1,
            "scope": "fresh_scoped_workspace_initialization" if eligible else "initialization_outcome_unresolved",
            "lifetime_sha256": self.sha})

    def peer(self, index, path, info):
        self.save("peer-" + str(index) + ".json", {"peer": index, "path": str(path),
            "device": info.st_dev, "inode": info.st_ino, "lifetime_sha256": self.sha})

    def ready(self, owner, count):
        controller = owner.controller
        with owner._dispatch_lock, controller.lock:
            if (controller.request_retention_failed or controller.jobs or controller.validations
                    or controller.threads or controller.operation_lock.locked() or owner._closed_readback):
                scope = "shared_physical_initialization_unresolved"
            else:
                scope = "shared_physical_ready"
            self.save("startup.json", {"scope": scope, "peers": count,
                "controller": owner._identity, "lifetime_sha256": self.sha,
                "startup_sha256": digest(self.read("startup-begin.json"))})

    def census_directory(self, path):
        path = absolute(path)
        if path != self.path.parent / ("shared-entries-" + self.value["owner"]):
            raise LocalSetupError("local_setup_physical_census_changed")
        self.save("census-directory.json", self.identity(path.lstat(), True))

    def accept(self, token, request, request_raw):
        pin = request.get("physical_intent")
        if (type(pin) is not dict or set(pin) != {"id", "sha256"}
                or not HASH.fullmatch(pin.get("id", "")) or not HASH.fullmatch(pin.get("sha256", ""))):
            raise LocalSetupError("local_setup_physical_intent_unavailable")
        raw = self.read("operation-" + pin["id"] + ".json")
        operation = decode(raw)
        base = dict(request)
        del base["physical_intent"]
        member_sha256 = operation.get("member_sha256")
        if member_sha256 is None:
            # This preserves the public ordinary shared path without turning
            # missing native member provenance into a fabricated association.
            unresolved = decode(self.read("unresolved-member-" + str(request["peer"]) + ".json"))
            if unresolved != {"peer": request["peer"], "scope": "member_provenance_unavailable",
                              "lifetime_sha256": self.sha}:
                raise LocalSetupError("local_setup_physical_intent_changed")
        else:
            member_raw = self.read("member-" + str(request["peer"]) + ".json")
            member = decode(member_raw)
            if digest(member_raw) != member_sha256 or member["session"] != base["native_request"]["session_id"]:
                raise LocalSetupError("local_setup_physical_intent_changed")
        if (digest(raw) != pin["sha256"] or operation != {"id": pin["id"], "peer": request["peer"],
                "request": base, "member_sha256": member_sha256, "lifetime_sha256": self.sha}):
            raise LocalSetupError("local_setup_physical_intent_changed")
        self.save("accepted-" + pin["id"] + ".json", {"id": pin["id"], "entry": token,
            "operation_sha256": digest(raw), "request_sha256": digest(request_raw)})

    def bridge(self, path):
        path = absolute(path)
        index = path.stem
        if not index.isdecimal() or str(int(index)) != index or not 0 <= int(index) < 8:
            raise LocalSetupError("local_setup_physical_peer_unavailable")
        raw = self.read("peer-" + index + ".json")
        peer = decode(raw)
        info = path.lstat()
        if peer != {"peer": int(index), "path": str(path), "device": info.st_dev,
                    "inode": info.st_ino, "lifetime_sha256": self.sha}:
            raise LocalSetupError("local_setup_physical_peer_changed")
        self.save("bridge-" + uuid4().hex + ".json", {"peer": int(index),
            "peer_sha256": digest(raw), "lifetime_sha256": self.sha})

    def close(self):
        failure = None
        for name in ("lease_fd", "directory_fd", "root_fd"):
            fd = getattr(self, name, None)
            if fd is not None:
                try:
                    os.close(fd)
                except OSError as error:
                    failure = failure or error
                finally:
                    setattr(self, name, None)
        if failure is not None:
            raise failure

    def __enter__(self):
        return self

    def __exit__(self, *exception):
        self.close()


def selected_shared_lifetime(args):
    """Select the Rust owner's immutable pre-spawn record, not a native claim."""
    if args.shared_lifetime is None and args.shared_lifetime_sha256 is None:
        return None
    if (args.shared_lifetime is None or args.shared_lifetime_sha256 is None
            or args.shared_peers is None or not HASH.fullmatch(args.shared_close_id or "")):
        raise LocalSetupError("local_setup_lifetime_unavailable")
    path = absolute(args.shared_lifetime)
    raw = read(path, args.shared_lifetime_sha256)
    value = decode(raw)
    required = {"schema_version", "owner", "realm", "state_root", "artifact_directory",
                "resources_sha256", "command", "arguments", "peers", "restored", "preparer"}
    if type(value) is dict and value.get("schema_version") == 2:
        required.add("fence")
    if (type(value) is not dict or set(value) != required
            or type(value["schema_version"]) is not int or value["schema_version"] not in (1, 2)
            or value["owner"] != args.shared_close_id or value["peers"] != args.shared_peers
            or type(value["peers"]) is not int or value["restored"] != args.shared_restore_sha256
            or value["artifact_directory"] != str(path.parent)
            or path.name != "shared-lifetime-" + args.shared_close_id + ".json"
            or Path(args.state_root) != path.parent / "prepared/local-session"
            or type(value["realm"]) is not str or not value["realm"]
            or type(value["state_root"]) is not str or not Path(value["state_root"]).is_absolute()
            or type(value["resources_sha256"]) is not str or not HASH.fullmatch(value["resources_sha256"])
            or value["command"] != "/usr/bin/env" or type(value["arguments"]) is not list
            or any(type(arg) is not str for arg in value["arguments"])
            or type(value["preparer"]) is not dict or set(value["preparer"]) != {
                "prepare.stdout"}):
        raise LocalSetupError("local_setup_lifetime_unavailable")
    for name, expected in value["preparer"].items():
        read(path.parent / name, expected)
    return path, raw


def observe_shared_abandonment(root, owner, lifetime):
    """Optional constituent from the actual joined owner; never G admission.

    Failure retains evidence and produces no disposition. Ordinary successful
    close selection remains governed by its separate existing owner contract.
    """
    if lifetime is None:
        return
    try:
        snapshot = owner.abandoned_snapshot()
        if snapshot is None or snapshot["lifetime_sha256"] != digest(lifetime[1]):
            return
        if read(lifetime[0], digest(lifetime[1])) != lifetime[1]:
            return
        identity = decode(lifetime[1])["owner"]
        write_new(root / ("shared-abandoned-" + identity + ".json"), encoded(snapshot))
    except Exception:
        return

def select_shared_close(adapter, root, binding, expected):
    """Read exactly one native-selected close record under session_owner."""
    raw = read(root / "shared-close.json", expected)
    value = decode(raw)
    if (type(value) is not dict or set(value) != {"schema_version", "owner", "round", "controller"}
            or type(value["schema_version"]) is not int or value["schema_version"] != 1
            or type(value["owner"]) is not str or not HASH.fullmatch(value["owner"])):
        raise LocalSetupError("local_setup_shared_close_unavailable")
    record, _ = read_session_record(root, binding)
    selections = round_snapshot(adapter.backend, root, record, allow_empty=True)
    if value["round"] != round_fingerprint(root, binding, selections):
        raise LocalSetupError("local_setup_round_changed")
    rows = adapter.ControllerConnections.read_closed_snapshot(value["controller"], selections)
    if rows[-1]["workspace"] != str(root / record["workspace"]):
        raise LocalSetupError("local_setup_round_changed")
    return value, raw


def claim_shared_close(adapter, root, binding, expected):
    # Native has independently consumed its exact close/admission selection.
    # This second claim protects the physical round while its lease is held.
    # Any crash after the claim preserves refusal; no replacement is selected.
    value, raw = select_shared_close(adapter, root, binding, expected)
    write_new(root / "shared-restore-claim.json", raw)
    if read(root / "shared-close.json", expected) != raw:
        raise LocalSetupError("local_setup_round_changed")
    (root / "shared-close.json").unlink(); sync_directory(root)
    return value, raw


def observe_shared_job_audit(root, owner, identity):
    """Best-effort evidence after join; never decides reuse or changes cleanup."""
    try:
        if type(identity) is not str or not HASH.fullmatch(identity):
            return
        value = owner.joined_job_audit()
        if value is None:
            return
        raw = encoded({"schema_version": 1, "owner": identity, "audit": value})
        if len(raw) > LIMIT:
            return
        write_new(root / ("shared-jobs-audit-" + identity + ".json"), raw)
    except Exception:
        # Preserve the original close result and leave unavailable diagnostics
        # unclaimed. Process cancellation is not swallowed here.
        return


def observe_shared_close(adapter, root, binding, owner, identity, claimed=None):
    """Only ControllerConnections decides whether joined work is reusable."""
    try:
        snapshot = owner.closed_snapshot()
        record, _ = read_session_record(root, binding)
        selections = round_snapshot(adapter.backend, root, record, allow_empty=True)
        adapter.ControllerConnections.read_closed_snapshot(snapshot, selections)
    except (adapter.backend.SetupError, LocalSetupError):
        # Unknown effect/publication or an incomplete request selection stays
        # retained and unresolved. Process exit alone cannot publish reuse.
        return
    if type(identity) is not str or not HASH.fullmatch(identity):
        return
    value = {"schema_version": 1, "owner": identity, "round": round_fingerprint(root, binding, selections),
             "controller": snapshot}
    write_new(root / "shared-close.json", encoded(value))
    if claimed is not None:
        path = root / "shared-restore-claim.json"
        if read(path) != claimed:
            raise LocalSetupError("local_setup_round_changed")
        path.unlink(); sync_directory(root)


class _SharedPeerTransport:
    """The existing peer's listener, serve tasks and one retained close task."""
    def __init__(self, adapter_module, peer):
        self.module, self.peer = adapter_module, peer
        self.loop = asyncio.get_running_loop()
        self.listener = None
        self.openings = set()
        self.tasks = set()
        self.writers = set()
        self.accepted = False
        self.closing = False
        self.failed = False
        self.close_task = None
        if peer._transport_owner is not None:
            raise LocalSetupError("local_setup_owner_changed")
        peer._transport_owner = self

    def listen(self, listener):
        if self.listener is not None or self.closing:
            raise LocalSetupError("local_setup_owner_changed")
        # Publish only after successful registration. On construction failure
        # the caller still closes its raw socket and drains all earlier peers.
        self.loop.add_reader(listener.fileno(), self.accept_ready)
        self.listener = listener

    def accept_ready(self):
        # Acceptance and ownership transfer are synchronous on this loop. A
        # queued readiness callback after fencing cannot create a late stream.
        if self.closing:
            return
        try:
            connection, _ = self.listener.accept()
        except (BlockingIOError, InterruptedError):
            return
        except OSError:
            self.failed = True
            self.retirement(True)
            return
        if self.accepted:
            connection.close()
            return
        self.accepted = True
        connection.setblocking(False)
        task = asyncio.create_task(self.open_socket(connection))
        self.openings.add(task)
        task.add_done_callback(self.completed_opening)

    async def open_socket(self, connection):
        # The close task joins this retained handoff without cancelling it.
        # Stream construction may have accepted the socket before returning.
        try:
            reader, writer = await asyncio.open_unix_connection(sock=connection,
                limit=2 * self.module.backend.MAX_BODY)
        except BaseException:
            connection.close()
            raise
        self.start(reader, writer)

    def completed_opening(self, task):
        self.openings.discard(task)
        if task.cancelled() or task.exception() is not None:
            self.failed = True

    def start(self, reader, writer):
        # Retain the actual stream before scheduling its coroutine. A close can
        # cancel a serve task before its first poll; that still must join the
        # transport instead of relying on a never-entered finally.
        self.writers.add(writer)
        accepted = not self.closing
        if not accepted:
            writer.close()
        task = asyncio.create_task(self.serve(reader, writer, accepted))
        self.tasks.add(task)
        task.add_done_callback(self.completed)

    def completed(self, task):
        self.tasks.discard(task)
        if not task.cancelled() and task.exception() is not None:
            self.failed = True

    async def serve(self, reader, writer, accepted):
        try:
            if accepted:
                await self.module.socket_stdio(self.peer, reader, writer)
        finally:
            try:
                if accepted:
                    await self.peer.aclose()
            finally:
                writer.close()
                await writer.wait_closed()
                self.writers.discard(writer)

    def retirement(self, close=False):
        # No asyncio task, Event or stream is manipulated from the inherited
        # owner's worker-thread dispatch path.
        if asyncio.get_running_loop() is not self.loop:
            raise self.module.backend.SetupError("shared_peer_unavailable", 409)
        if close and self.close_task is None:
            self.closing = True
            self.peer.close_admission()
            if self.listener is not None:
                self.loop.remove_reader(self.listener.fileno())
                self.listener.close()
            self.close_task = asyncio.create_task(self.drain())
            # Keep the actual task and retrieve its result even if the command
            # receiver disappears. Readback never starts a replacement drain.
            self.close_task.add_done_callback(self.completed_close)
        if self.close_task is None:
            return "not_requested"
        if not self.close_task.done():
            return "pending"
        if self.close_task.cancelled() or self.close_task.exception() is not None or self.failed:
            return "failed"
        return "joined"

    def completed_close(self, task):
        if not task.cancelled():
            task.exception()

    def joined(self):
        # Read-only observation also used by the inherited owner's worker.
        return (self.close_task is not None and self.close_task.done()
                and not self.close_task.cancelled()
                and self.close_task.exception() is None and not self.failed)

    async def drain(self):
        # Only this peer is fenced. An entered selected-install dispatch drains,
        # but its physical worker remains with ControllerConnections.job_wait.
        # Keep socket cleanup owned even if the adapter drain reports failure.
        # Reader removal fences all future acceptance. Every socket already
        # accepted is represented by one opening task before any await.
        if self.openings:
            opened = await asyncio.gather(*tuple(self.openings), return_exceptions=True)
            if any(isinstance(result, BaseException) for result in opened):
                self.failed = True
        try:
            await self.peer.aclose()
        except BaseException:
            self.failed = True
        while self.tasks or self.writers:
            # Close the actual streams as well as cancelling their serve tasks.
            # This includes a queued accept whose coroutine never started.
            writers, active = tuple(self.writers), tuple(self.tasks)
            for writer in writers:
                writer.close()
            for task in active:
                task.cancel()
            await asyncio.gather(*active, return_exceptions=True)
            closed = await asyncio.gather(*(writer.wait_closed() for writer in writers),
                                          return_exceptions=True)
            if any(isinstance(result, BaseException) for result in closed):
                self.failed = True
            self.writers.difference_update(writers)
        if self.failed:
            raise LocalSetupError("local_setup_connection_failed")

    async def aclose(self):
        self.retirement(True)
        await self.module._joined(self.close_task)


async def shared_connections(adapter_module, controller, count, stopped, ready, *, _owner=None):
    """One live host owns Controller and a distinct MCP connection per helper.

    Native attachment and resolved-call admission remain per helper. These
    private sockets carry bytes only; their paths are not caller authority.
    """
    if type(count) is not int or not 1 <= count <= 8:
        raise LocalSetupError("local_setup_connection_count")
    owner = _owner if _owner is not None else adapter_module.ControllerConnections(controller)
    if owner.controller is not controller:
        raise LocalSetupError("local_setup_owner_changed")
    # AF_UNIX paths are short even when the installed resource root is long.
    directory = Path(tempfile.mkdtemp(prefix="connect-setup-", dir="/tmp")).resolve()
    transports, paths = [], []
    failures = []
    def open_peer(peer):
        transport = _SharedPeerTransport(adapter_module, peer)
        transports.append(transport)
        path = directory / (str(len(transports) - 1) + ".sock")
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            listener.bind(str(path))
            info = path.lstat()
            paths.append((path, info.st_dev, info.st_ino))
            os.chmod(path, 0o600)
            physical = getattr(controller, "_shared_physical", None)
            if physical is not None:
                physical.peer(len(transports) - 1, path, info)
            listener.listen(1)
            listener.setblocking(False)
            transport.listen(listener)
        except BaseException:
            listener.close()
            raise
        return str(path)
    try:
        if owner._peer_factory is not None:
            raise LocalSetupError("local_setup_owner_changed")
        owner._peer_factory = open_peer
        for _ in range(count):
            open_peer(owner.open())
        physical = getattr(controller, "_shared_physical", None)
        if physical is not None:
            physical.ready(owner, count)
        await ready([str(path) for path, _, _ in paths])
        await stopped()
    finally:
        # Global shutdown still belongs to this host. Explicit per-peer close
        # above never calls either of these Controller owner methods.
        owner.close_admission()
        owner._peer_factory = None
        for transport in transports:
            transport.retirement(True)
        failures = await asyncio.gather(*(transport.aclose() for transport in transports),
                                        return_exceptions=True)
        await owner.aclose()
        for path, device, inode in paths:
            info = path.lstat()
            if not stat.S_ISSOCK(info.st_mode) or (info.st_dev, info.st_ino) != (device, inode):
                raise LocalSetupError("local_setup_connection_changed")
            path.unlink()
        directory.rmdir()
    if any(isinstance(result, BaseException) for result in failures):
        raise LocalSetupError("local_setup_connection_failed")


async def shared_owner_requests(adapter_module, owner, reader, writer):
    """Bounded inherited owner frames. MCP peers never receive this channel."""
    sequence = 0
    while True:
        try:
            raw = await reader.readline()
        except (ValueError, asyncio.LimitOverrunError):
            raise LocalSetupError("local_setup_owner_input") from None
        if not raw:
            return
        if len(raw) > 256 * 1024 or not raw.endswith(b"\n"):
            raise LocalSetupError("local_setup_owner_input")
        value = decode(raw)
        if (type(value) is not dict or set(value) != {"id", "command"}
                or type(value["id"]) is not int or value["id"] != sequence + 1):
            raise LocalSetupError("local_setup_owner_input")
        sequence = value["id"]
        try:
            # A cancelled pipe reader still joins the finite private owner
            # operation. Its response may be unknown; no effect is replayed.
            command = value["command"]
            if isinstance(command, dict) and command.get("operation") in ("close_peer", "observe_peer", "renew_peer"):
                # Fencing and task retention run on the owning event loop.
                # The command returns a bounded observation, never waits for
                # drain while holding the serialized inherited channel.
                result = owner.peer_control(command)
            else:
                task = asyncio.create_task(asyncio.to_thread(owner.owner_control, command))
                result = await adapter_module._joined(task)
            response = {"id": sequence, "result": result}
        except adapter_module.backend.SetupError:
            response = {"id": sequence, "error": "shared_selection_unavailable"}
        payload = encoded(response)
        if len(payload) > LIMIT:
            raise LocalSetupError("local_setup_owner_output")
        writer.write(payload)
        await asyncio.wait_for(writer.drain(), 30)


async def shared_stdio_owner(adapter_module, controller, count):
    """The inherited owner pipe retains lifetime and exact private handoffs."""
    loop = asyncio.get_running_loop()
    reader = asyncio.StreamReader(limit=256 * 1024)
    pipe = os.fdopen(os.dup(sys.stdin.fileno()), "rb", buffering=0)
    output = os.fdopen(os.dup(sys.stdout.fileno()), "wb", buffering=0)
    transport = write_transport = writer = None
    task = asyncio.current_task()
    owner = adapter_module.ControllerConnections(controller,
        _closed_readback=getattr(controller, "_shared_closed_readback", ()),
        _entry_census=getattr(controller, "_shared_entry_census", None))
    def stop():
        task.cancel()
    async def stopped():
        await shared_owner_requests(adapter_module, owner, reader, writer)
    async def ready(paths):
        writer.write(encoded({"shared_setup": 1, "peers": paths}))
        await asyncio.wait_for(writer.drain(), 30)
    try:
        for signum in (signal.SIGTERM, signal.SIGINT):
            loop.add_signal_handler(signum, stop)
        transport, _ = await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), pipe)
        protocol = asyncio.StreamReaderProtocol(asyncio.StreamReader())
        write_transport, _ = await loop.connect_write_pipe(lambda: protocol, output)
        writer = asyncio.StreamWriter(write_transport, protocol, None, loop)
        await shared_connections(adapter_module, controller, count, stopped, ready, _owner=owner)
    finally:
        if transport is not None:
            transport.close()
        else:
            pipe.close()
        if writer is not None:
            writer.close()
            await writer.wait_closed()
        elif write_transport is not None:
            write_transport.close()
        else:
            output.close()
        for signum in (signal.SIGTERM, signal.SIGINT):
            loop.remove_signal_handler(signum)
    return owner


def _shared_peer_failure(phase, error):
    """Last attempted phase and closed categories, never exception text or paths."""
    phases = {
        "arguments", "physical_lease", "physical_bridge", "physical_close", "path_validation",
        "socket_validation", "socket_create", "socket_connect", "socket_recheck",
        "socket_nonblocking", "select", "stdin_read", "socket_read",
        "stdout_write", "socket_write", "socket_half_close", "socket_close",
    }
    classes = {
        LocalSetupError: "LocalSetupError", OSError: "OSError",
        FileNotFoundError: "FileNotFoundError", PermissionError: "PermissionError",
        ConnectionRefusedError: "ConnectionRefusedError",
        ConnectionResetError: "ConnectionResetError",
        ConnectionAbortedError: "ConnectionAbortedError", BrokenPipeError: "BrokenPipeError",
        BlockingIOError: "BlockingIOError", InterruptedError: "InterruptedError",
        TimeoutError: "TimeoutError", ValueError: "ValueError", TypeError: "TypeError",
    }
    codes = {getattr(errno, name): name for name in (
        "ENOENT", "EACCES", "EPERM", "ECONNREFUSED", "ECONNRESET", "ECONNABORTED",
        "ENOTCONN", "EPIPE", "EBADF", "EINVAL", "EAGAIN", "EINTR", "ETIMEDOUT",
    )}
    code = codes.get(error.errno, "other") if type(error) in classes and isinstance(error, OSError) else "none"
    return ("local_setup_connection_diagnostic phase=" + (phase if phase in phases else "other")
            + " exception=" + classes.get(type(error), "Other") + " errno=" + code)


def stdio_bridge(path, *, _phase=None):
    """Copy native-owned stdio to one exact live host-selected peer, once."""
    phase = _phase if _phase is not None else ["path_validation"]
    phase[0] = "path_validation"
    path = absolute(path)
    phase[0] = "socket_validation"
    directory, info = path.parent.lstat(), path.lstat()
    if (not stat.S_ISDIR(directory.st_mode) or directory.st_uid != os.getuid()
            or directory.st_mode & 0o077 or not stat.S_ISSOCK(info.st_mode)
            or info.st_uid != os.getuid() or info.st_mode & 0o077):
        raise LocalSetupError("local_setup_connection_unavailable")
    phase[0] = "socket_create"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        phase[0] = "socket_connect"
        connection.connect(str(path))
        phase[0] = "socket_recheck"
        selected = path.lstat()
        if (selected.st_dev, selected.st_ino) != (info.st_dev, info.st_ino):
            raise LocalSetupError("local_setup_connection_changed")
        incoming = outgoing = b""
        input_open = output_open = True
        half_closed = False
        phase[0] = "socket_nonblocking"
        connection.setblocking(False)
        while output_open or incoming:
            reads = []
            writes = []
            if input_open and len(outgoing) < 65536:
                reads.append(sys.stdin.fileno())
            if output_open and len(incoming) < 65536:
                reads.append(connection)
            if incoming:
                writes.append(sys.stdout.fileno())
            if outgoing:
                writes.append(connection)
            phase[0] = "select"
            readable, writable, _ = select.select(reads, writes, [])
            if sys.stdin.fileno() in readable:
                phase[0] = "stdin_read"
                value = os.read(sys.stdin.fileno(), 65536 - len(outgoing))
                outgoing += value
                input_open = bool(value)
            if connection in readable:
                phase[0] = "socket_read"
                value = connection.recv(65536 - len(incoming))
                incoming += value
                output_open = bool(value)
                if not output_open:
                    input_open = False
                    outgoing = b""
            if sys.stdout.fileno() in writable:
                phase[0] = "stdout_write"
                incoming = incoming[os.write(sys.stdout.fileno(), incoming):]
            if connection in writable and outgoing:
                phase[0] = "socket_write"
                outgoing = outgoing[connection.send(outgoing):]
            if not input_open and not outgoing and not half_closed:
                phase[0] = "socket_half_close"
                connection.shutdown(socket.SHUT_WR)
                half_closed = True
        phase[0] = "socket_close"
    return 0


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv[:1] == ["--shared-peer"]:
        phase = ["arguments"]
        try:
            if len(argv) == 2:
                return stdio_bridge(argv[1], _phase=phase)
            if len(argv) != 6 or argv[2] != "--shared-lifetime" or argv[4] != "--shared-lifetime-sha256":
                raise LocalSetupError("local_setup_connection_unavailable")
            phase[0] = "physical_lease"
            with PhysicalLease(argv[3], argv[5]) as physical:
                phase[0] = "physical_bridge"
                physical.bridge(argv[1])
                result = stdio_bridge(argv[1], _phase=phase)
                phase[0] = "physical_close"
                return result
        except Exception as error:
            print("local_setup_connection_unavailable", file=sys.stderr)
            try:
                print(_shared_peer_failure(phase[0], error), file=sys.stderr)
            except Exception:
                # Added diagnostics cannot replace the original exit status.
                pass
            return 2
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--configuration", type=Path, required=True)
    parser.add_argument("--configuration-sha256", required=True)
    parser.add_argument("--helper-sha256", required=True)
    parser.add_argument("--adapter-sha256", required=True)
    parser.add_argument("--backend-sha256", required=True)
    parser.add_argument("--state-root", type=Path, required=True)
    parser.add_argument("--shared-peers", type=int, choices=range(1, 9))
    parser.add_argument("--shared-restore-sha256")
    parser.add_argument("--shared-close-id")
    parser.add_argument("--shared-lifetime", type=Path)
    parser.add_argument("--shared-lifetime-sha256")
    args = parser.parse_args(argv)
    physical = None
    try:
        helper = Path(__file__).resolve()
        # Reapply the same pure selection checks at the native-owned launch.
        prepare_server(configuration=args.configuration, configuration_sha256=args.configuration_sha256,
                       helper=helper, helper_sha256=args.helper_sha256, adapter_sha256=args.adapter_sha256,
                       backend_sha256=args.backend_sha256, state_root=args.state_root)
        server = prepared_configuration(args.configuration, args.configuration_sha256, helper, args.helper_sha256)
        lifetime = selected_shared_lifetime(args)
        if lifetime is not None and decode(lifetime[1])["schema_version"] == 2:
            physical = PhysicalLease(lifetime[0], digest(lifetime[1]))
            # This file-only check precedes loading any optional adapter code.
            physical.startup(server["args"][7:], Path(args.state_root).exists())
        adapter = retained_adapter(helper, args.adapter_sha256, args.backend_sha256)
        binding = {"configuration_sha256": args.configuration_sha256, "helper_sha256": args.helper_sha256,
                   "adapter_sha256": args.adapter_sha256, "backend_sha256": args.backend_sha256}
        with session_owner(args.state_root, binding) as (root, record, reopened):
            if args.shared_close_id is not None and (args.shared_peers is None
                    or not HASH.fullmatch(args.shared_close_id)):
                raise LocalSetupError("local_setup_shared_close_unavailable")
            # No later failed launch may leave an earlier successful observation
            # available to its host. Remove only this private, regular record.
            observation = root / "empty-close.json"
            if observation.exists() or observation.is_symlink():
                read(observation)
                observation.unlink(); sync_directory(root)
            selected = claimed = None
            if args.shared_restore_sha256 is not None:
                if args.shared_peers != 1 or not reopened:
                    raise LocalSetupError("local_setup_shared_close_unavailable")
                selected, claimed = claim_shared_close(adapter, root, binding, args.shared_restore_sha256)
            elif args.shared_peers is not None and reopened:
                raise LocalSetupError("local_setup_previous_session_needs_review")
            factory = controller_factory(adapter.backend, root, record, reopened, _shared_restore=selected)
            runner = None
            ordinary_closed = False
            shared_closed = None
            if args.shared_peers is not None:
                async def runner(controller):
                    nonlocal shared_closed
                    if lifetime is not None:
                        controller._shared_entry_census = adapter.SharedEntryCensus(
                            lifetime[0].parent / ("shared-entries-" + args.shared_close_id), digest(lifetime[1]),
                            _physical=physical)
                        controller._shared_physical = physical
                    if selected is not None:
                        # Preserve old facts separately. No old view, validation,
                        # entry token or effect permission reaches the new owner.
                        controller._shared_closed_readback = tuple(row for row in
                            selected["controller"]["rounds"] if row["requests"])
                    shared_closed = await shared_stdio_owner(adapter, controller, args.shared_peers)
            elif "--browser-port" not in server["args"][7:]:
                async def runner(controller):
                    nonlocal ordinary_closed
                    await adapter.stdio(adapter.SetupAdapter(controller))
                    # Signal cancellation and protocol errors do not reach this
                    # point. adapter.main also joins Controller in its finally.
                    ordinary_closed = True
            result = adapter.main(server["args"][7:], controller_factory=factory, run_controller=runner)
            if result == 0 and ordinary_closed:
                observe_empty_close(adapter.backend, root, binding)
            if result == 0 and shared_closed is not None:
                observe_shared_close(adapter, root, binding, shared_closed, args.shared_close_id, claimed)
                observe_shared_abandonment(root, shared_closed, lifetime)
                observe_shared_job_audit(root, shared_closed, args.shared_close_id)
            return result
    except LocalSetupError as error:
        print(str(error), file=sys.stderr)
        return 2
    except Exception:
        print("local_setup_unavailable", file=sys.stderr)
        return 2
    finally:
        if physical is not None:
            physical.close()


if __name__ == "__main__":
    raise SystemExit(main())
