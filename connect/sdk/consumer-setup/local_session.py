#!/usr/bin/env python3
"""Product-selected local setup entry; the native MCP host owns this process."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import select
import socket
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


def offer_next_round(backend, root, binding, host_input, host_output, cancelled_fd):
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


def controller_factory(backend, root, record, reopened):
    """Keep old exact requests with Controller; only live custody permits setup."""
    original = root / record["workspace"]
    selected_record_raw = read_session_record(root, record["binding"])[1]
    selections = round_snapshot(backend, root, record, allow_empty=True) if reopened else ()
    offer = offered_round(root, record["binding"], selections) if reopened else None
    live = offer is not None
    if reopened and not live and not selections:
        # An empty receipt is history, never permission for a saved descriptor
        # to reopen an effect-capable Controller without its live host owner.
        raise LocalSetupError("local_setup_previous_session_needs_review")
    current = root / offer[1] if live else original
    next_record = next_session_record(record, offer[1], selections) if live else record
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
        positional[2] = root / ("readback-" + uuid4().hex) if reopened and not live else current
        if reopened:
            kwargs.update(_retained_selections=selections, _local_read_only=not live)
            if not live:
                kwargs["allow_node_search"] = False
                kwargs["allow_public_github_research"] = False
        if not reopened or live:
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
            if live:
                connection, workspace = offer
                if (round_fingerprint(root, record["binding"], selections)["session_sha256"] != digest(selected_record_raw)
                        or round_snapshot(backend, root, record, allow_empty=True) != selections):
                    raise LocalSetupError("local_setup_round_changed")
                old_checkpoint = original / "local-recovery.json"
                if record["schema_version"] == 1 and selections:
                    write_new(old_checkpoint, selections[-1])
                write_new(current / "local-empty.json", encoded({"schema_version": 1, "workspace": workspace, "empty": True}))
                published = encoded(next_record)
                replace(root / "session.json", published)
                send_frame(connection, {"published_sha256": digest(published)})
                if read_frame(connection) != {"accepted": True}:
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


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--configuration", type=Path, required=True)
    parser.add_argument("--configuration-sha256", required=True)
    parser.add_argument("--helper-sha256", required=True)
    parser.add_argument("--adapter-sha256", required=True)
    parser.add_argument("--backend-sha256", required=True)
    parser.add_argument("--state-root", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        helper = Path(__file__).resolve()
        # Reapply the same pure selection checks at the native-owned launch.
        prepare_server(configuration=args.configuration, configuration_sha256=args.configuration_sha256,
                       helper=helper, helper_sha256=args.helper_sha256, adapter_sha256=args.adapter_sha256,
                       backend_sha256=args.backend_sha256, state_root=args.state_root)
        server = prepared_configuration(args.configuration, args.configuration_sha256, helper, args.helper_sha256)
        adapter = retained_adapter(helper, args.adapter_sha256, args.backend_sha256)
        binding = {"configuration_sha256": args.configuration_sha256, "helper_sha256": args.helper_sha256,
                   "adapter_sha256": args.adapter_sha256, "backend_sha256": args.backend_sha256}
        with session_owner(args.state_root, binding) as (root, record, reopened):
            factory = controller_factory(adapter.backend, root, record, reopened)
            return adapter.main(server["args"][7:], controller_factory=factory)
    except LocalSetupError as error:
        print(str(error), file=sys.stderr)
        return 2
    except Exception:
        print("local_setup_unavailable", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
