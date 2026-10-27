#!/usr/bin/env python3
"""Prepare existing combined-setup host files without launching any process."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import sys
import tarfile
import tempfile
import unicodedata
from types import ModuleType, SimpleNamespace


ARCHIVE_LIMIT = 256 * 1024 * 1024
FILE_LIMIT = 64 * 1024 * 1024
HOST_FILE_LIMIT = 128 * 1024 * 1024
TOTAL_LIMIT = 256 * 1024 * 1024
ENTRY_LIMIT = 4096
HEX = re.compile(r"[0-9a-f]{64}\Z")
TARGETS = ("macos-arm64", "linux-x86_64")
REQUIRED = ("setup/backend.py", "setup/mcp_adapter.py", "setup/python_acquisition.py",
            "consumers/CONTENTS.json", "consumers/install-library-consumer.py",
            "selected/bin/install-library-consumer.py", "selected/bin/toolkit-python-plan",
            "selected/provenance/selection.json")
NODE_FILES = ("setup/node_acquisition.py", "selected/bin/toolkit-node-plan")
OFFICIAL_FILES = ("official/CONTENTS.json", "official/delivery.json", "setup/install-library-consumer.py")
RECIPES_TOOL = "selected/bin/toolkit-local-recipes"
MCPB_TOOL = "selected/bin/toolkit-mcpb-install"
MCPB_ACQUISITION = "setup/mcpb_acquisition.py"
NODE_PREBUILT_ACQUISITION = "setup/node_prebuilt_acquisition.py"
COMPOSE_RUNTIME = "setup/compose_runtime.py"
COMPOSE_PLANNER = "selected/bin/toolkit-compose-plan"
COMPOSE_ACQUISITION = "setup/compose_acquisition.py"
COMPOSE_BRIDGE = "setup/compose_bridge.py"
OPTIONAL = ("setup/public_github.py", *NODE_FILES, *OFFICIAL_FILES, RECIPES_TOOL, MCPB_TOOL, "setup/local_session.py",
            "selected/bin/toolkit-package-inspect", "setup/acquired_inspection.py", MCPB_ACQUISITION, NODE_PREBUILT_ACQUISITION,
            COMPOSE_RUNTIME, COMPOSE_PLANNER, COMPOSE_ACQUISITION, COMPOSE_BRIDGE)


class PreparationError(Exception):
    """A fixed diagnostic without paths, provider output or credentials."""


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def encoded(value):
    return (json.dumps(value, sort_keys=True, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def checked_hash(value):
    if not isinstance(value, str) or not HEX.fullmatch(value):
        raise PreparationError("invalid_sha256")
    return value


def directory(value):
    path = Path(value)
    if not path.is_absolute() or not path.is_dir():
        raise PreparationError("invalid_directory")
    return path.resolve(strict=True)


def new_path(value):
    path = Path(value)
    if not path.is_absolute() or path.name in ("", ".", ".."):
        raise PreparationError("invalid_new_directory")
    path = directory(path.parent) / path.name
    if path.exists() or path.is_symlink():
        raise PreparationError("output_or_workspace_exists")
    return path


def overlaps(left, right):
    return left == right or left in right.parents or right in left.parents


def compose_overlaps(left, right):
    # Reserve ambiguous fresh paths conservatively on every supported host.
    normalized = [Path(unicodedata.normalize("NFC", str(path)).casefold())
                  for path in (left, right)]
    return overlaps(*normalized)


def recipe_database_layout(database, locations):
    # The profile owner validates the canonical database path. This host-only
    # guard reserves SQLite sidecar names without opening or modifying them.
    paths = [Path(database + suffix) for suffix in ("", "-wal", "-shm", "-journal")]
    if any(compose_overlaps(path, location) for path in paths for location in locations):
        raise PreparationError("recipes_database_overlaps_host_paths")
    for path in paths[1:]:
        try:
            mode = path.lstat().st_mode
        except FileNotFoundError:
            continue
        if not stat.S_ISREG(mode):
            raise PreparationError("invalid_recipes_database_sidecar")


def read_regular(path, limit=FILE_LIMIT):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
            raise PreparationError("invalid_input_file")
        raw = stream.read(limit + 1)
        if len(raw) > limit:
            raise PreparationError("input_too_large")
        return raw, stat.S_IMODE(info.st_mode)


def pinned_path(value, expected, *, executable=False, preserve_link=False):
    checked_hash(expected)
    path = Path(value)
    if not path.is_absolute():
        raise PreparationError("invalid_input_path")
    path = directory(path.parent) / path.name
    resolved = path.resolve(strict=True)
    raw, mode = read_regular(resolved, HOST_FILE_LIMIT)
    if digest(raw) != expected or executable and not mode & 0o111:
        raise PreparationError("host_input_changed")
    return {"path": str(path if preserve_link else resolved),
            "resolved_path": str(resolved), "sha256": expected, "executable": executable}


def recheck_pins(pins):
    for pin in pins.values():
        path = Path(pin["path"])
        raw, mode = read_regular(path.resolve(strict=True), HOST_FILE_LIMIT)
        if (path.resolve(strict=True) != Path(pin["resolved_path"]) or digest(raw) != pin["sha256"]
                or pin["executable"] and not mode & 0o111):
            raise PreparationError("host_input_changed")


def node_build_options(args):
    """Capture an optional host-selected profile without interpreting its tools."""
    value = getattr(args, "node_build_profile", None)
    expected = getattr(args, "node_build_profile_sha256", None)
    if value is None and expected is None:
        return None
    if not value or not expected:
        raise PreparationError("node_build_profile_pair_required")
    checked_hash(expected)
    path = Path(value)
    if not path.is_absolute():
        raise PreparationError("invalid_input_path")
    path = directory(path.parent) / path.name
    raw, _ = read_regular(path, 65536)
    if not raw or digest(raw) != expected:
        raise PreparationError("node_build_profile_changed")
    return path, expected, raw


def relative(value):
    path = PurePosixPath(value)
    if (not value or path.is_absolute() or path.as_posix() != value or "\\" in value
            or any(part in (".", "..") for part in path.parts)):
        raise PreparationError("invalid_archive_member")
    return value


def archive_hash(stream):
    hasher, size = hashlib.sha256(), 0
    while chunk := stream.read(min(1024 * 1024, ARCHIVE_LIMIT + 1 - size)):
        size += len(chunk)
        if size > ARCHIVE_LIMIT:
            raise PreparationError("invalid_release_archive")
        hasher.update(chunk)
    return hasher.hexdigest()


def tree(root):
    files, directories = {}, {}
    pending = [(root, "")]
    while pending:
        parent, prefix = pending.pop()
        for item in parent.iterdir():
            name = relative(prefix + item.name)
            info = item.lstat()
            mode = stat.S_IMODE(info.st_mode)
            if stat.S_ISDIR(info.st_mode):
                directories[name] = mode
                pending.append((item, name + "/"))
            elif stat.S_ISREG(info.st_mode):
                files[name] = mode
            else:
                raise PreparationError("invalid_release_member")
            if len(files) + len(directories) > ENTRY_LIMIT:
                raise PreparationError("release_too_large")
    return files, directories


def release_snapshot(archive_path, archive_sha256, release, target):
    """Anchor extracted bytes to a host-trusted whole archive, without extraction."""
    checked_hash(archive_sha256)
    archive_path = Path(archive_path)
    if not archive_path.is_absolute():
        raise PreparationError("invalid_input_path")
    fd = os.open(archive_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    files, directories, hashes, retained = {}, {}, {}, {}
    with os.fdopen(fd, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > ARCHIVE_LIMIT:
            raise PreparationError("invalid_release_archive")
        if archive_hash(source) != archive_sha256:
            raise PreparationError("release_archive_changed")
        source.seek(0)
        seen, root_name, total = set(), None, 0
        with tarfile.open(fileobj=source, mode="r|gz") as archive:
            for member in archive:
                name = relative(member.name.rstrip("/") if member.isdir() else member.name)
                head, separator, tail = name.partition("/")
                if root_name is None:
                    root_name = head
                if (head != root_name or name in seen or member.mode not in (0o644, 0o755)
                        or not (member.isdir() or member.isfile())):
                    raise PreparationError("invalid_archive_member")
                seen.add(name)
                if len(seen) > ENTRY_LIMIT:
                    raise PreparationError("release_too_large")
                if not separator:
                    if not member.isdir() or member.mode != 0o755:
                        raise PreparationError("invalid_archive_member")
                    continue
                if member.isdir():
                    directories[tail] = member.mode
                    continue
                total += member.size
                if member.size < 0 or member.size > FILE_LIMIT or total > TOTAL_LIMIT:
                    raise PreparationError("release_too_large")
                with archive.extractfile(member) as stream:
                    raw = stream.read(FILE_LIMIT + 1)
                if len(raw) != member.size:
                    raise PreparationError("invalid_archive_member")
                files[tail], hashes[tail] = member.mode, digest(raw)
                if tail in (*REQUIRED, *OPTIONAL):
                    retained[tail] = raw
        if (not root_name or not re.fullmatch(r"toolkit-library-[A-Za-z0-9-]+-" + re.escape(target), root_name)
                or not set(REQUIRED) <= set(retained)):
            raise PreparationError("unsupported_combined_release")
        source.seek(0)
        if archive_hash(source) != archive_sha256:
            raise PreparationError("release_archive_changed")
    snapshot = {"root": root_name, "files": files, "directories": directories, "hashes": hashes}
    recheck_release(release, snapshot)
    return snapshot, retained


def recheck_release(release, snapshot):
    files, directories = tree(release)
    # A host may make extracted directories private. File modes and exact
    # membership remain bound; directory permission differences are not data.
    if set(files) != set(snapshot["files"]) or set(directories) != set(snapshot["directories"]):
        raise PreparationError("extracted_release_changed")
    for name, expected in snapshot["hashes"].items():
        raw, mode = read_regular(release / name)
        allowed = snapshot["files"][name]
        required = 0o500 if allowed & 0o111 else 0o400
        if digest(raw) != expected or mode & ~allowed or mode & required != required:
            raise PreparationError("extracted_release_changed")


def owner_module(name, path, raw):
    # Load only owner bytes already bound to the host-trusted archive. compile
    # avoids creating __pycache__ inside the verified extracted release.
    module = ModuleType(name)
    module.__file__ = str(path)
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


def write_new(path, raw):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def compose_options(args):
    """Read explicit host choices; lifecycle and graph semantics stay with Compose."""
    profiles = {}
    for name in ("acquisition_host", "backend"):
        path = getattr(args, "compose_" + name + "_profile", None)
        sha256 = getattr(args, "compose_" + name + "_profile_sha256", None)
        if path is not None or sha256 is not None:
            if not path or not sha256:
                raise PreparationError("compose_" + name + "_profile_pair_required")
            checked_hash(sha256)
            profiles[name] = {"path": path, "sha256": sha256}
    for name, option in (("successor_inputs", "inputs"), ("successor_backend", "backend_profile")):
        path = getattr(args, "compose_successor_" + option, None)
        expected = getattr(args, "compose_successor_" + option + "_sha256", None)
        if path is not None or expected is not None:
            if not path or not expected:
                raise PreparationError("compose_successor_pair_required")
            checked_hash(expected)
            profiles[name] = {"path": path, "sha256": expected}
    execution_names = ("graph_workspace", "mode", "allowed_actions", "expected_plan_sha256",
                       "delete_state_namespaces")
    execution = {name: getattr(args, "compose_" + name, None) for name in execution_names}
    if "backend" not in profiles and any(value is not None for value in execution.values()):
        raise PreparationError("compose_execution_profile_required")
    if "backend" in profiles:
        if not all(execution[name] for name in ("graph_workspace", "mode", "allowed_actions")):
            raise PreparationError("compose_execution_choices_required")
        if execution["mode"] not in ("new", "reopen"):
            raise PreparationError("invalid_compose_mode")
        if execution["mode"] == "new":
            if execution["expected_plan_sha256"] is not None:
                raise PreparationError("new_compose_plan_must_be_unset")
            graph = new_path(execution["graph_workspace"])
        else:
            checked_hash(execution["expected_plan_sha256"])
            graph = directory(execution["graph_workspace"])
        if str(graph) != execution["graph_workspace"]:
            raise PreparationError("invalid_compose_graph_path")
        execution["graph_workspace"] = graph
        for key in ("allowed_actions", "delete_state_namespaces"):
            value = execution[key]
            if value is not None and not isinstance(value, str):
                raise PreparationError("invalid_compose_choices")
            execution[key] = value.split(",") if value else []
    transition = getattr(args, "compose_successor_transition_sha256", None)
    successor = {"successor_inputs", "successor_backend"} & profiles.keys()
    if successor or transition is not None:
        if (successor != {"successor_inputs", "successor_backend"} or
                "backend" not in profiles or execution["mode"] != "reopen"):
            raise PreparationError("compose_successor_reopen_required")
        if transition is not None:
            checked_hash(transition)
        execution["successor"] = {"predecessor_plan_sha256": execution["expected_plan_sha256"],
                                  "transition_sha256": transition}
    timeout = getattr(args, "compose_operation_timeout_seconds", None)
    if not profiles and timeout is not None:
        raise PreparationError("compose_profile_required")
    if timeout is None:
        timeout = 300
    if type(timeout) is not int or not 1 <= timeout <= 3600:
        raise PreparationError("invalid_compose_timeout")
    return profiles, execution, timeout


def recovery_options(args):
    """Explicit recovery opts into readback, never live acquisition or effects."""
    path = getattr(args, "recover_manifest", None)
    expected = getattr(args, "recover_manifest_sha256", None)
    if path is None and expected is None:
        return None
    if not path or not expected:
        raise PreparationError("recovery_manifest_pair_required")
    checked_hash(expected)
    if not Path(path).is_absolute():
        raise PreparationError("invalid_recovery_manifest")
    forbidden = ("allow_public_pypi", "allow_public_npm", "enable_node_search", "enable_public_github_research", "allow_remote_recipes",
                 "pip_zipapp", "pip_sha256", "npm_archive", "npm_archive_sha256",
                 "mcpb_acquisition_profile", "mcpb_acquisition_profile_sha256",
                 "node_prebuilt_profile", "node_prebuilt_profile_sha256",
                 "node_prebuilt_policy", "node_prebuilt_policy_sha256",
                 "prior_request", "prior_request_sha256",
                 "prior_manifest", "prior_manifest_sha256", "prior_request_id",
                 "compose_acquisition_host_profile", "compose_acquisition_host_profile_sha256",
                 "compose_backend_profile", "compose_backend_profile_sha256",
                 "compose_graph_workspace", "compose_mode", "compose_allowed_actions",
                 "compose_expected_plan_sha256", "compose_delete_state_namespaces",
                 "compose_operation_timeout_seconds")
    forbidden += ("compose_successor_inputs", "compose_successor_inputs_sha256",
                  "compose_successor_backend_profile", "compose_successor_backend_profile_sha256",
                  "compose_successor_transition_sha256")
    if any(getattr(args, name, None) for name in forbidden):
        raise PreparationError("recovery_read_only")
    return Path(path), expected


def prior_manifest_options(args):
    """Explicit host approval of one nonsecret request for a fresh setup."""
    values = tuple(getattr(args, name, None) for name in
                   ("prior_manifest", "prior_manifest_sha256", "prior_request_id"))
    if all(value is None for value in values):
        return None
    if any(getattr(args, name, None) is not None for name in
           ("recover_manifest", "recover_manifest_sha256")):
        raise PreparationError("recovery_read_only")
    if any(getattr(args, name, None) is not None for name in
           ("prior_request", "prior_request_sha256")):
        raise PreparationError("ambiguous_prior_selection")
    if not all(isinstance(value, str) and value for value in values):
        raise PreparationError("prior_manifest_selection_required")
    path, expected, identifier = values
    checked_hash(expected)
    if not Path(path).is_absolute():
        raise PreparationError("invalid_input_path")
    if not re.fullmatch(r"[0-9a-f]{32}", identifier):
        raise PreparationError("invalid_prior_request_id")
    return Path(path), expected, identifier


def read_prior_manifest(backend, selection):
    """Use the selected release's readers; never follow recorded runtime paths."""
    path, expected, identifier = selection
    raw, root, entries = backend.load_recovery_selection(path, expected)
    entry = next((row for row in entries if row["id"] == identifier), None)
    if entry is None:
        raise PreparationError("prior_request_not_selected")
    if entry["kind"] != "toolkit.consumer_setup.configured_install_request":
        raise PreparationError("unsupported_prior_request")
    request_path = root / "requests" / (identifier + ".json")
    request = backend.read_file(root, "requests/" + identifier + ".json", backend.MAX_BODY * 2)
    if digest(request) != entry["sha256"]:
        raise PreparationError("prior_request_changed")
    backend.prior_request(request)
    return {"manifest": raw, "root": root, "path": request_path,
            "sha256": entry["sha256"], "request": request}


def staging_path(value, *, recovering=False):
    if not recovering:
        return directory(value)
    # Recovery can observe an absent old staging tree. Preserve its exact
    # canonical name without creating it or following a replacement alias.
    path = Path(value)
    if (not path.is_absolute() or path != path.resolve(strict=False)
            or path.is_symlink() or path.exists() and not path.is_dir()):
        raise PreparationError("invalid_recovery_staging")
    return path


def prepare(args):
    recovery = recovery_options(args)
    prior_manifest = prior_manifest_options(args)
    release = directory(args.release_dir)
    output, workspace = new_path(args.output), new_path(args.workspace)
    staging = staging_path(args.staging_root, recovering=bool(recovery))
    compose_profiles, compose_execution, compose_timeout = compose_options(args)
    overlap = compose_overlaps
    locations = [release, output, workspace, staging]
    if any(overlap(a, b) for i, a in enumerate(locations) for b in locations[i + 1:]):
        raise PreparationError("overlapping_directories")
    if args.target not in TARGETS:
        raise PreparationError("unsupported_target")
    node_build = node_build_options(args)
    if node_build is not None:
        if any(overlap(node_build[0], location) for location in locations):
            raise PreparationError("node_build_profile_overlaps_host_paths")
        locations.append(node_build[0])
    # The archive owns both manifest parsing and each retained request format.
    snapshot, retained = release_snapshot(args.release_archive, args.release_sha256, release, args.target)
    backend = owner_module("prepared_setup_backend", release / "setup/backend.py", retained["setup/backend.py"])
    recovery_raw = recovery_root = recovery_entries = None
    acquisition_only = False
    recovery_acquisitions = {}
    if recovery:
        recovery_raw, recovery_root, recovery_entries = backend.load_recovery_selection(*recovery)
        if any(overlap(protected, destination) for protected in (recovery[0], recovery_root)
               for destination in (output, workspace)):
            raise PreparationError("recovery_overlaps_host_paths")
        locations.extend((recovery[0], recovery_root))
        acquisition_only = all(row["kind"] == backend.ACQUISITION_REQUEST for row in recovery_entries)
        for entry in recovery_entries:
            if entry["kind"] == backend.ACQUISITION_REQUEST:
                record, _ = backend.load_request_recovery(recovery_root, entry["id"], entry["sha256"],
                                                          kind=entry["kind"])
                kind = record["family"].removeprefix("selected-")
                if "setup/" + kind.replace("-", "_") + "_acquisition.py" not in retained:
                    raise PreparationError("recovery_owner_not_in_release")
                recovery_acquisitions[kind] = record
    if "backend" in compose_profiles:
        graph = compose_execution["graph_workspace"]
        if any(overlap(graph, location) for location in locations):
            raise PreparationError("compose_graph_overlaps_host_paths")
        locations.append(graph)
    if bool(args.pip_zipapp) != bool(args.pip_sha256) or bool(args.pip_zipapp) != args.allow_public_pypi:
        raise PreparationError("explicit_pypi_scope_required")
    allow_mcpb = getattr(args, "allow_mcpb", False)
    inspect_mcpb = getattr(args, "enable_mcpb_inspection", False)
    if type(allow_mcpb) is not bool:
        raise PreparationError("invalid_mcpb_choice")
    if type(inspect_mcpb) is not bool:
        raise PreparationError("invalid_mcpb_inspection_choice")
    if inspect_mcpb and not allow_mcpb:
        raise PreparationError("mcpb_inspection_requires_mcpb")
    mcpb_path = getattr(args, "mcpb_acquisition_profile", None)
    mcpb_sha256 = getattr(args, "mcpb_acquisition_profile_sha256", None)
    acquire_mcpb = mcpb_path is not None or mcpb_sha256 is not None
    if acquire_mcpb and (not mcpb_path or not mcpb_sha256):
        raise PreparationError("mcpb_acquisition_profile_pair_required")
    if acquire_mcpb and not inspect_mcpb:
        raise PreparationError("mcpb_acquisition_requires_inspection")
    node_prebuilt_path = getattr(args, "node_prebuilt_profile", None)
    node_prebuilt_sha256 = getattr(args, "node_prebuilt_profile_sha256", None)
    node_prebuilt_policy = getattr(args, "node_prebuilt_policy", None)
    node_prebuilt_policy_sha256 = getattr(args, "node_prebuilt_policy_sha256", None)
    policy_selected = node_prebuilt_policy is not None or node_prebuilt_policy_sha256 is not None
    if policy_selected:
        if not node_prebuilt_policy or not node_prebuilt_policy_sha256:
            raise PreparationError("node_prebuilt_policy_pair_required")
        if node_prebuilt_path is not None or node_prebuilt_sha256 is not None:
            raise PreparationError("ambiguous_node_prebuilt_selection")
        node_prebuilt_path, node_prebuilt_sha256 = node_prebuilt_policy, node_prebuilt_policy_sha256
    acquire_node_prebuilt = node_prebuilt_path is not None or node_prebuilt_sha256 is not None
    if acquire_node_prebuilt and (not node_prebuilt_path or not node_prebuilt_sha256):
        raise PreparationError("node_prebuilt_profile_pair_required")
    if acquire_node_prebuilt and not args.allow_public_npm:
        raise PreparationError("node_prebuilt_requires_node_acquisition")
    selected_node_inputs = (args.selected_node, args.selected_node_sha256)
    github_research = getattr(args, "enable_public_github_research", False)
    if type(github_research) is not bool:
        raise PreparationError("invalid_github_research_choice")
    search_node = getattr(args, "enable_node_search", False)
    if type(search_node) is not bool:
        raise PreparationError("invalid_node_search_choice")
    selected_npm_inputs = (args.selected_npm_cli, args.selected_npm_cli_sha256)
    selected_inputs = (*selected_node_inputs, *selected_npm_inputs)
    if (any(value is not None for value in selected_inputs)
            and (not all(selected_node_inputs)
                 or any(value is not None for value in selected_npm_inputs) and not all(selected_npm_inputs)
                 or not all(selected_npm_inputs) and not allow_mcpb and not search_node)):
        raise PreparationError("selected_node_runtime_pair_required")
    if allow_mcpb and not all(selected_node_inputs) and not acquisition_only:
        raise PreparationError("mcpb_runtime_required")
    if (any(value is not None for value in (args.npm_archive, args.npm_archive_sha256))
            and not all((args.npm_archive, args.npm_archive_sha256))
            or bool(args.npm_archive) != bool(args.npm_archive_sha256)
            or bool(args.npm_archive) != (args.allow_public_npm or search_node)
            or args.allow_public_npm and not all(selected_inputs)
            or search_node and not all(selected_node_inputs)):
        raise PreparationError("explicit_npm_scope_required")
    inspect_acquired = getattr(args, "enable_acquired_inspection", False)
    if type(inspect_acquired) is not bool:
        raise PreparationError("invalid_acquired_inspection_choice")
    if inspect_acquired and not (recovery or args.allow_public_pypi or args.allow_public_npm or acquire_mcpb):
        raise PreparationError("acquired_inspection_requires_acquisition")
    recipe_inputs = tuple(getattr(args, "recipes_" + name, None) for name in ("database", "realm", "owner", "scope"))
    recipes_enabled = any(value is not None for value in recipe_inputs)
    if recipes_enabled and not all(value is not None for value in recipe_inputs):
        raise PreparationError("complete_recipes_profile_required")
    if recipes_enabled and recipe_inputs[3] not in ("read", "retain"):
        raise PreparationError("invalid_recipes_scope")
    remote_recipes = getattr(args, "allow_remote_recipes", False)
    if type(remote_recipes) is not bool or remote_recipes and recipe_inputs[3] != "retain":
        raise PreparationError("remote_recipes_require_retention")
    prior_path = getattr(args, "prior_request", None)
    prior_sha256 = getattr(args, "prior_request_sha256", None)
    prior_enabled = prior_path is not None or prior_sha256 is not None
    if prior_enabled and (not prior_path or not prior_sha256):
        raise PreparationError("prior_request_pair_required")
    if (inspect_acquired and (args.allow_public_pypi or args.allow_public_npm)
            and "setup/acquired_inspection.py" not in retained):
        raise PreparationError("acquired_inspection_not_in_release")
    if acquire_mcpb and MCPB_ACQUISITION not in retained:
        raise PreparationError("mcpb_acquisition_not_in_release")
    if acquire_node_prebuilt and not {NODE_PREBUILT_ACQUISITION, MCPB_ACQUISITION} <= retained.keys():
        raise PreparationError("node_prebuilt_not_in_release")
    compose_raw = {}
    if compose_profiles:
        needed = {COMPOSE_RUNTIME, COMPOSE_PLANNER}
        needed.add(COMPOSE_ACQUISITION if "acquisition_host" in compose_profiles else COMPOSE_BRIDGE)
        if "backend" in compose_profiles:
            needed.add(COMPOSE_BRIDGE)
        if not needed <= retained.keys():
            raise PreparationError("compose_not_in_release")
        for name, selected in compose_profiles.items():
            path = Path(selected["path"])
            if not path.is_absolute():
                raise PreparationError("invalid_input_path")
            backend.canonical_directory(path.parent)
            if any(overlap(path, location) for location in locations):
                raise PreparationError("compose_profile_overlaps_host_paths")
            raw, _ = read_regular(path, 2 * 1024 * 1024 if name == "successor_inputs" else backend.MAX_BODY)
            if digest(raw) != selected["sha256"]:
                raise PreparationError("compose_profile_changed")
            compose_raw[name] = raw
            locations.append(path)
    mcpb_raw = None
    if acquire_mcpb:
        checked_hash(mcpb_sha256)
        mcpb_path = Path(mcpb_path)
        if not mcpb_path.is_absolute():
            raise PreparationError("invalid_input_path")
        if any(overlap(mcpb_path, location) for location in locations):
            raise PreparationError("mcpb_acquisition_profile_overlaps_host_paths")
        def read_mcpb():
            backend.canonical_directory(mcpb_path.parent)
            return read_regular(mcpb_path, backend.MAX_BODY)[0]
        mcpb_raw = read_mcpb()
        if digest(mcpb_raw) != mcpb_sha256:
            raise PreparationError("mcpb_acquisition_profile_changed")
        locations.append(mcpb_path)
    node_prebuilt_raw = None
    if acquire_node_prebuilt:
        checked_hash(node_prebuilt_sha256)
        node_prebuilt_path = Path(node_prebuilt_path)
        if not node_prebuilt_path.is_absolute():
            raise PreparationError("invalid_input_path")
        if any(overlap(node_prebuilt_path, location) for location in locations):
            raise PreparationError("node_prebuilt_profile_overlaps_host_paths")
        def read_node_prebuilt():
            backend.canonical_directory(node_prebuilt_path.parent)
            return read_regular(node_prebuilt_path, backend.MAX_BODY)[0]
        node_prebuilt_input_raw = node_prebuilt_raw = read_node_prebuilt()
        if digest(node_prebuilt_raw) != node_prebuilt_sha256:
            raise PreparationError("node_prebuilt_profile_changed")
        locations.append(node_prebuilt_path)
    prior_raw = None
    prior_source = None
    if prior_manifest:
        prior_source = read_prior_manifest(backend, prior_manifest)
        if any(overlap(source, location) for source in (prior_manifest[0], prior_source["root"])
               for location in locations):
            raise PreparationError("prior_manifest_overlaps_host_paths")
        prior_path, prior_sha256 = prior_source["path"], prior_source["sha256"]
        prior_enabled = True
    if prior_enabled:
        checked_hash(prior_sha256)
        prior_path = Path(prior_path)
        if not prior_path.is_absolute():
            raise PreparationError("invalid_input_path")
        if any(overlap(prior_path, location) for location in locations):
            raise PreparationError("prior_request_overlaps_host_paths")
        def read_prior():
            return backend.read_file(backend.canonical_directory(prior_path.parent),
                                     prior_path.name, backend.MAX_BODY * 2)
        prior_raw = read_prior()
        if digest(prior_raw) != prior_sha256:
            raise PreparationError("prior_request_changed")
        backend.prior_request(prior_raw)
        # Reserve the original source against writable recipe database/sidecars.
        locations.append(prior_path)
        if prior_source is not None:
            if prior_raw != prior_source["request"]:
                raise PreparationError("prior_request_changed")
            locations.extend((prior_manifest[0], prior_source["root"]))
    if recipes_enabled and RECIPES_TOOL not in retained:
        raise PreparationError("local_recipes_not_in_release")
    selection = backend.decode(retained["selected/provenance/selection.json"])
    if (selection.get("platform") != args.target
            or selection.get("scope") not in ("retained_python_tools_only", "retained_selected_tools")):
        raise PreparationError("release_target_mismatch")
    if compose_profiles:
        capabilities = selection.get("capabilities", [])
        if (not isinstance(capabilities, list) or "compose-local-execution" not in capabilities
                or snapshot["files"][COMPOSE_PLANNER] & 0o111 == 0):
            raise PreparationError("compose_not_in_release")
    recovered_node_inspection = acquisition_only and inspect_acquired and "node" in recovery_acquisitions
    if github_research and "setup/public_github.py" not in retained:
        raise PreparationError("github_research_not_in_release")
    if search_node and "setup/node_acquisition.py" not in retained:
        raise PreparationError("node_search_not_in_release")
    if (all(selected_inputs) or recovered_node_inspection) and (selection.get("scope") != "retained_selected_tools"
                                or not set(NODE_FILES) <= set(retained)):
        raise PreparationError("selected_node_not_in_release")
    if allow_mcpb and (selection.get("scope") != "retained_selected_tools" or MCPB_TOOL not in retained):
        raise PreparationError("mcpb_not_in_release")
    inspect_package = getattr(args, "enable_package_inspection", False)
    inspect_plugin = getattr(args, "enable_plugin_inspection", False)
    if type(inspect_package) is not bool:
        raise PreparationError("invalid_package_inspection_choice")
    if type(inspect_plugin) is not bool:
        raise PreparationError("invalid_plugin_inspection_choice")
    if inspect_plugin:
        capabilities = selection.get("capabilities", [])
        if (type(capabilities) is not list or any(type(value) is not str for value in capabilities)
                or "agent-plugin-files" not in capabilities):
            raise PreparationError("plugin_inspection_not_in_release")
    if inspect_mcpb:
        capabilities = selection.get("capabilities", [])
        if (type(capabilities) is not list or any(type(value) is not str for value in capabilities)
                or "mcpb-archive-inspection" not in capabilities):
            raise PreparationError("mcpb_inspection_not_in_release")
    inspect_package = inspect_package or inspect_plugin or inspect_mcpb
    if inspect_package and "selected/bin/toolkit-package-inspect" not in retained:
        raise PreparationError("package_inspector_not_in_release")
    families = args.families.split(",") if args.families else []
    if (not families and not recovery or len(families) != len(set(families))
            or not set(families) <= {*backend.FAMILIES, *backend.OFFICIAL_FAMILIES}):
        raise PreparationError("invalid_library_families")
    official = set(families) & set(backend.OFFICIAL_FAMILIES)
    if official:
        if not set(OFFICIAL_FILES) <= set(retained):
            raise PreparationError("official_setup_not_in_release")
        delivery = backend.decode(retained["official/delivery.json"])
        components = delivery.get("components") if isinstance(delivery, dict) else None
        if (not isinstance(delivery, dict) or delivery.get("format") != "meerkat-toolkit.official-consumers.v1"
                or not isinstance(components, list) or not all(isinstance(row, dict) for row in components)):
            raise PreparationError("invalid_official_delivery")
        for family in official:
            rows = [row for row in components if row.get("catalog_id") == family.removeprefix("official-")]
            if len(rows) != 1:
                raise PreparationError("official_family_not_in_release")
            # Omitted targets have the historical macOS meaning used by the
            # release packager. Runtime compatibility remains installer-owned.
            if rows[0].get("target", "macos-arm64") != args.target:
                raise PreparationError("official_target_mismatch")
    playwright = backend.PLAYWRIGHT_FAMILY in families
    playwright_path = getattr(args, "playwright_host_profile", None)
    playwright_sha256 = getattr(args, "playwright_host_profile_sha256", None)
    playwright_raw = None
    if playwright:
        if not playwright_path or not playwright_sha256:
            raise PreparationError("playwright_host_profile_required")
        checked_hash(playwright_sha256)
        playwright_path = Path(playwright_path)
        if not playwright_path.is_absolute():
            raise PreparationError("invalid_input_path")
        playwright_path = directory(playwright_path.parent) / playwright_path.name
        playwright_raw, _ = read_regular(playwright_path, backend.MAX_BODY)
        if digest(playwright_raw) != playwright_sha256:
            raise PreparationError("playwright_host_profile_changed")
        browser_profile = backend.decode(playwright_raw)
        backend.keys(browser_profile, ("schema_version", "chrome_executable", "chrome_sha256", "process_parent"))
        if type(browser_profile["schema_version"]) is not int or browser_profile["schema_version"] != 1:
            raise PreparationError("invalid_playwright_host_profile")
    elif playwright_path is not None or playwright_sha256 is not None:
        raise PreparationError("unused_playwright_host_profile")
    pins = {"python": pinned_path(args.python, args.python_sha256, executable=True),
            "mcp_python": pinned_path(args.mcp_python, args.mcp_python_sha256, executable=True, preserve_link=True)}
    for name, selected in compose_profiles.items():
        pins["compose_" + name + "_profile"] = pinned_path(selected["path"], selected["sha256"])
        if name == "successor_inputs":
            continue
        value = backend.decode(compose_raw[name])
        # These are host file bindings, not a second graph/profile validator.
        # The Compose owner checks the rest before planning or engine effects.
        try:
            engine = value["engine"]
            for tool_name in ("docker", "compose"):
                item = engine[tool_name]
                path = Path(item["path"])
                backend.canonical_directory(path.parent)
                if str(path) != str(path.resolve(strict=True)):
                    raise PreparationError("invalid_compose_engine_binding")
                if any(overlap(path, location) for location in locations):
                    raise PreparationError("compose_tool_overlaps_host_paths")
                pins["compose_" + name + "_" + tool_name] = pinned_path(
                    path, item["sha256"], executable=True)
        except (KeyError, TypeError, ValueError):
            raise PreparationError("invalid_compose_engine_binding") from None
    if compose_profiles:
        # The same checks apply to the explicitly chosen interpreters.
        for name in ("python", "mcp_python"):
            if any(overlap(Path(pins[name]["resolved_path"]), location) for location in locations):
                raise PreparationError("compose_tool_overlaps_host_paths")
        if "acquisition_host" in compose_profiles:
            runtime_owner = owner_module("prepared_compose_runtime", release / COMPOSE_RUNTIME,
                                         retained[COMPOSE_RUNTIME])
            runtime_owner.validate_proposal_host(compose_raw["acquisition_host"], acquire_images=True)
        locations.extend(Path(pin["path"]) for name, pin in pins.items() if name.startswith("compose_"))
    if acquire_mcpb:
        pins["mcpb_acquisition_profile"] = pinned_path(mcpb_path, mcpb_sha256)
    if acquire_node_prebuilt:
        pins["node_prebuilt_policy" if policy_selected else "node_prebuilt_profile"] = pinned_path(node_prebuilt_path, node_prebuilt_sha256)
    if "activepieces" in families or playwright:
        if not all((args.node, args.node_sha256, args.npm_cli, args.npm_cli_sha256)):
            raise PreparationError("playwright_runtime_required" if playwright else "activepieces_runtime_required")
        pins["node"] = pinned_path(args.node, args.node_sha256, executable=True)
        pins["npm_cli"] = pinned_path(args.npm_cli, args.npm_cli_sha256)
    elif any((args.node, args.node_sha256, args.npm_cli, args.npm_cli_sha256)):
        raise PreparationError("unused_node_runtime")
    if args.allow_public_pypi:
        pins["pip"] = pinned_path(args.pip_zipapp, args.pip_sha256)
    if all(selected_node_inputs):
        pins["selected_node"] = pinned_path(args.selected_node, args.selected_node_sha256, executable=True)
    if all(selected_npm_inputs):
        pins["selected_npm_cli"] = pinned_path(args.selected_npm_cli, args.selected_npm_cli_sha256)
    if node_build is not None:
        if not {"selected_node", "selected_npm_cli"} <= pins.keys():
            raise PreparationError("node_build_requires_selected_node_validator")
        pins["node_build_profile"] = pinned_path(node_build[0], node_build[1])
    if args.allow_public_npm or search_node:
        pins["npm_archive"] = pinned_path(args.npm_archive, args.npm_archive_sha256)
    def tool(name):
        return {"path": str(release / name), "sha256": snapshot["hashes"][name]}
    profile = {"schema_version": 1, "scope": backend.SELECTED_SCOPE, "staging_root": str(staging),
               "installer": tool("selected/bin/install-library-consumer.py"),
               "validators": {"python": tool("selected/bin/toolkit-python-plan")}}
    if inspect_package:
        profile["package_inspector"] = tool("selected/bin/toolkit-package-inspect")
    if inspect_plugin:
        profile["plugin_inspection"] = True
    if inspect_mcpb:
        profile["mcpb_inspection"] = True
    if args.target == "linux-x86_64":
        profile["python_target"] = args.target
    if "selected_npm_cli" in pins or recovered_node_inspection:
        profile["validators"]["node"] = tool("selected/bin/toolkit-node-plan")
    if allow_mcpb:
        profile["validators"]["mcpb"] = tool(MCPB_TOOL)
    selected_enabled = (not recovery or inspect_acquired or inspect_package or inspect_plugin or allow_mcpb
                        or any(row["kind"] == backend.SELECTED_REQUEST for row in recovery_entries))
    if node_build is not None:
        if not selected_enabled or "node" not in profile["validators"]:
            raise PreparationError("node_build_requires_selected_node_validator")
        profile["node_build"] = {"path": str(output / "node-build-profile.json"), "sha256": node_build[1]}
    files = {"selected-install.json": encoded(profile)} if selected_enabled else {}
    if node_build is not None:
        files["node-build-profile.json"] = node_build[2]
    compose_wrappers = {}
    for name in ("successor_inputs", "successor_backend"):
        if name in compose_raw:
            files["compose-" + name.replace("_", "-") + ".json"] = compose_raw[name]
    for name, raw in compose_raw.items():
        if name in ("successor_inputs", "successor_backend"):
            continue
        inner = "compose-" + name.replace("_", "-") + ".json"
        files[inner] = raw
        common = {"schema_version": 1, "staging_root": str(staging),
                  "runtime": tool(COMPOSE_RUNTIME), "planner": tool(COMPOSE_PLANNER),
                  "operation_timeout_seconds": compose_timeout}
        reference = {"path": str(output / inner), "sha256": digest(raw)}
        if name == "acquisition_host":
            outer = {**common, "scope": "acquire_agent_selected_container_images",
                     "python_sha256": pins["python"]["sha256"], "host_profile": reference}
            filename = "compose-acquisition.json"
        else:
            outer = {**common, "scope": "execute_agent_selected_local_container_plan",
                     "python": {key: pins["python"][key] for key in ("path", "sha256")},
                     "backend_profile": reference, **compose_execution,
                     "graph_workspace": str(compose_execution["graph_workspace"])}
            if "successor" in compose_execution:
                outer["successor"] = {**compose_execution["successor"], **{
                    key: {"path": str(output / ("compose-" + selected.replace("_", "-") + ".json")),
                          "sha256": digest(compose_raw[selected])}
                    for key, selected in (("inputs", "successor_inputs"), ("backend_profile", "successor_backend"))}}
            filename = "compose-execution.json"
        compose_wrappers[name] = (filename, inner, outer)
        files[filename] = encoded(outer)
    if recipes_enabled:
        database, realm, owner, scope = recipe_inputs
        files["local-recipes.json"] = encoded({"schema_version": 1,
            "scope": backend.LOCAL_RECIPES_SCOPE if scope == "read" else backend.LOCAL_RECIPES_RETAIN_SCOPE,
            "database": database, "realm": realm, "owner": owner, "tool": tool(RECIPES_TOOL)})
    verifier = "setup/install-library-consumer.py" if official else "consumers/install-library-consumer.py"
    argv = ["-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1", pins["mcp_python"]["path"], "-I", "-B",
            str(release / "setup/mcp_adapter.py"), "--bundle", str(release / "consumers"),
            "--contents-sha256", snapshot["hashes"]["consumers/CONTENTS.json"],
            "--workspace", str(workspace), "--families", ",".join(families),
            "--verifier", str(release / verifier),
            "--verifier-sha256", snapshot["hashes"][verifier],
            "--python", pins["python"]["path"]]
    if selected_enabled:
        argv += ["--selected-install-profile", str(output / "selected-install.json"),
                 "--selected-install-profile-sha256", digest(files["selected-install.json"])]
    if recovery:
        files["recover-manifest.json"] = recovery_raw
        argv += ["--recover-manifest", str(output / "recover-manifest.json"),
                 "--recover-manifest-sha256", recovery[1]]
    for name, (filename, _, _) in compose_wrappers.items():
        flag = "--compose-acquisition-profile" if name == "acquisition_host" else "--compose-profile"
        argv += [flag, str(output / filename), flag + "-sha256", digest(files[filename])]
    if recipes_enabled:
        argv += ["--local-recipes-profile", str(output / "local-recipes.json"),
                 "--local-recipes-profile-sha256", digest(files["local-recipes.json"])]
    if prior_enabled:
        files["prior-request.json"] = prior_raw
        argv += ["--prior-request", str(output / "prior-request.json"),
                 "--prior-request-sha256", prior_sha256]
    if remote_recipes:
        argv += ["--allow-remote-recipes"]
    if official:
        argv += ["--official-bundle", str(release / "official"),
                 "--official-contents-sha256", snapshot["hashes"]["official/CONTENTS.json"],
                 "--official-installer", str(release / "setup/install-library-consumer.py"),
                 "--official-installer-sha256", snapshot["hashes"]["setup/install-library-consumer.py"]]
    if playwright:
        files["playwright-host.json"] = playwright_raw
        argv += ["--playwright-host-profile", str(output / "playwright-host.json"),
                 "--playwright-host-profile-sha256", playwright_sha256]
    if "node" in pins:
        argv += ["--node", pins["node"]["path"], "--npm-cli", pins["npm_cli"]["path"]]
    if "selected_node" in pins:
        argv += ["--selected-node", pins["selected_node"]["path"]]
    if "selected_npm_cli" in pins:
        argv += ["--selected-npm-cli", pins["selected_npm_cli"]["path"]]
    if args.allow_public_pypi:
        acquisition = {"schema_version": 1, "scope": "trusted_pypi_wheel_acquisition",
                       "python_sha256": pins["python"]["sha256"],
                       "pip": {k: pins["pip"][k] for k in ("path", "sha256")}}
        if args.target == "linux-x86_64":
            acquisition["target"] = args.target
        files["python-acquisition.json"] = encoded(acquisition)
        argv += ["--python-acquisition-profile", str(output / "python-acquisition.json"),
                 "--python-acquisition-profile-sha256", digest(files["python-acquisition.json"])]
    if args.allow_public_npm or search_node:
        acquisition = {"schema_version": 1, "scope": "trusted_npm_registry_acquisition",
                       "allow_acquisition": args.allow_public_npm,
                       "python_sha256": pins["python"]["sha256"],
                       "node_sha256": pins["selected_node"]["sha256"],
                       "npm": {k: pins["npm_archive"][k] for k in ("path", "sha256")}}
        if args.target == "linux-x86_64":
            acquisition["target"] = args.target
        files["node-acquisition.json"] = encoded(acquisition)
        argv += ["--node-acquisition-profile", str(output / "node-acquisition.json"),
                 "--node-acquisition-profile-sha256", digest(files["node-acquisition.json"])]
    if acquire_node_prebuilt:
        if policy_selected:
            policy = backend.decode(node_prebuilt_input_raw)
            backend.keys(policy, ("schema_version", "scope", "target", "allowed_origins", "allowed_redirect_origins"))
            if (type(policy.get("schema_version")) is not int or policy["schema_version"] != 1
                    or policy.get("scope") != "trusted_public_node_prebuilt_resources"):
                raise PreparationError("invalid_node_prebuilt_resource_policy")
            node_prebuilt_raw = encoded({**policy, "scope": "trusted_public_node_prebuilt_acquisition",
                "python": {name: pins["python"][name] for name in ("path", "sha256")}})
            node_prebuilt_sha256 = digest(node_prebuilt_raw)
        files["node-prebuilt.json"] = node_prebuilt_raw
        argv += ["--node-prebuilt-profile", str(output / "node-prebuilt.json"),
                 "--node-prebuilt-profile-sha256", node_prebuilt_sha256]
    if search_node:
        argv += ["--allow-node-search"]
    if github_research:
        argv += ["--allow-public-github-research"]
    if acquire_mcpb:
        files["mcpb-acquisition.json"] = mcpb_raw
        argv += ["--mcpb-acquisition-profile", str(output / "mcpb-acquisition.json"),
                 "--mcpb-acquisition-profile-sha256", mcpb_sha256]
    if inspect_acquired:
        argv += ["--allow-acquired-inspection"]
    files["mcp.json"] = encoded({"mcpServers": {"toolkit-setup": {"command": "/usr/bin/env", "args": argv}}})
    with tempfile.TemporaryDirectory(prefix=".toolkit-host-prepare-", dir=output.parent) as temporary:
        temp = Path(temporary)
        for name, raw in files.items():
            write_new(temp / name, raw)
        selected_binding = None
        if selected_enabled:
            checked_profile = temp / "selected-install.json"
            checked_profile_raw = files["selected-install.json"]
            if node_build is not None:
                # Only the outer profile locator differs before publication.
                # The toolchain's internal paths and exact bytes stay intact.
                checked_profile = temp / "check-selected-install.json"
                checked_profile_raw = encoded({**profile, "node_build": {
                    "path": str(temp / "node-build-profile.json"), "sha256": node_build[1]}})
                write_new(checked_profile, checked_profile_raw)
            _, _, payloads = backend.selected_profile(checked_profile,
                digest(checked_profile_raw), recovering=bool(recovery))
            selected_node = pins.get("selected_node", pins.get("node"))
            selected_npm = pins.get("selected_npm_cli", None if "selected_node" in pins else pins.get("npm_cli"))
            selected_binding = {"profile_sha256": digest(files["selected-install.json"]),
                "tools": {name: digest(raw) for name, raw in payloads.items()},
                "runtimes": {"python": pins["python"]["resolved_path"],
                    "node": selected_node["resolved_path"] if selected_node and not acquisition_only else None,
                    "npm_cli": selected_npm["resolved_path"] if selected_npm and not acquisition_only else None}}
        def check_recovery():
            if not recovery:
                return
            current = backend.load_recovery_selection(*recovery)
            if current != (recovery_raw, recovery_root, recovery_entries):
                raise PreparationError("recovery_manifest_changed")
            identities = {}
            for entry in recovery_entries:
                record, _ = backend.load_request_recovery(recovery_root, entry["id"], entry["sha256"],
                    kind=entry["kind"], selected_binding=selected_binding,
                    recipes_sha=digest(files["local-recipes.json"]) if recipes_enabled else None,
                    verifier=True, contents_sha=snapshot["hashes"]["consumers/CONTENTS.json"],
                    installer_sha=snapshot["hashes"]["consumers/install-library-consumer.py"],
                    runtimes={"python": pins["python"]["resolved_path"],
                        "node": pins["node"]["resolved_path"] if "node" in pins and not acquisition_only else None,
                        "npm_cli": pins["npm_cli"]["resolved_path"] if "npm_cli" in pins and not acquisition_only else None},
                    families=tuple(families), official_selection=(snapshot["hashes"]["official/CONTENTS.json"],
                        snapshot["hashes"]["setup/install-library-consumer.py"]) if official else None,
                    playwright_profile_sha=playwright_sha256)
                if entry["kind"] == backend.ACQUISITION_REQUEST:
                    kind = record["family"].removeprefix("selected-")
                    identity = (record["staging_root"], record["host"])
                    if kind in identities and identities[kind] != identity:
                        raise PreparationError("recovery_profile_changed")
                    identities[kind] = identity
                    if any(overlap(Path(record["staging_root"]), path) for path in (output, workspace)):
                        raise PreparationError("recovery_overlaps_host_paths")
                    if selected_enabled and kind != "compose":
                        backend.check_recovery_profile(temp / "selected-install.json",
                            digest(files["selected-install.json"]), record["host"]["selected_profile_sha256"])
            if inspect_acquired:
                package_reads = any(kind in identities for kind in ("python", "node"))
                if (not identities or not package_reads and "mcpb" not in identities
                        or "mcpb" in identities and not inspect_mcpb):
                    raise PreparationError("invalid_acquired_inspection_scope")
                if package_reads and "setup/acquired_inspection.py" not in retained:
                    raise PreparationError("acquired_inspection_not_in_release")
            # Detect anchor changes even when another request was being read.
            if backend.load_recovery_selection(*recovery) != current:
                raise PreparationError("recovery_manifest_changed")
        check_recovery()
        def check_recipes():
            if recipes_enabled:
                try:
                    value, _, _ = backend.load_local_recipes_profile(temp / "local-recipes.json", digest(files["local-recipes.json"]))
                except backend.SetupError:
                    raise PreparationError("invalid_local_recipes_profile") from None
                recipe_database_layout(value["database"], locations)
        check_recipes()
        io = SimpleNamespace(Error=backend.SetupError, read_file=backend.read_file, write_new=backend.write_new,
                             canonical_directory=backend.canonical_directory, decode=backend.decode, keys=backend.keys)
        for name, (filename, inner, outer) in compose_wrappers.items():
            # Output is not published yet. Check equivalent private references
            # to the exact retained inner bytes, with the same owner constructors.
            reference = "host_profile" if name == "acquisition_host" else "backend_profile"
            private = {**outer, reference: {**outer[reference], "path": str(temp / inner)}}
            if "successor" in outer:
                private["successor"] = {**outer["successor"], **{
                    key: {**outer["successor"][key], "path": str(temp / Path(outer["successor"][key]["path"]).name)}
                    for key in ("inputs", "backend_profile")}}
            private_raw = encoded(private)
            private_path = temp / ("check-" + filename)
            write_new(private_path, private_raw)
            if name == "acquisition_host":
                owner = owner_module("prepared_compose_acquisition", release / COMPOSE_ACQUISITION,
                                     retained[COMPOSE_ACQUISITION])
                owner.ComposeAcquisition(io, private_path, digest(private_raw), pins["python"]["path"])
            else:
                owner = owner_module("prepared_compose_bridge", release / COMPOSE_BRIDGE,
                                     retained[COMPOSE_BRIDGE])
                owner.ComposeBridge(io, private_path, digest(private_raw), workspace)
        if args.allow_public_pypi:
            owner = owner_module("prepared_python_acquisition", release / "setup/python_acquisition.py", retained["setup/python_acquisition.py"])
            observed = owner.PythonAcquisition(io, temp / "python-acquisition.json", digest(files["python-acquisition.json"]),
                pins["python"]["path"], staging, Path(profile["installer"]["path"]), profile["installer"]["sha256"])
            if observed.target != profile.get("python_target", "macos-arm64"):
                raise PreparationError("python_target_mismatch")
        if args.allow_public_npm or search_node:
            owner = owner_module("prepared_node_acquisition", release / "setup/node_acquisition.py", retained["setup/node_acquisition.py"])
            observed = owner.NodeAcquisition(io, temp / "node-acquisition.json", digest(files["node-acquisition.json"]),
                pins["python"]["path"], staging, Path(profile["installer"]["path"]), profile["installer"]["sha256"],
                pins["selected_node"]["path"])
            if observed.target != args.target:
                raise PreparationError("node_target_mismatch")
            if acquire_node_prebuilt:
                node_owner = observed
                owner = owner_module("prepared_node_prebuilt_acquisition", release / NODE_PREBUILT_ACQUISITION,
                                     retained[NODE_PREBUILT_ACQUISITION])
                def no_source_before_acquisition(_):
                    raise PreparationError("node_prebuilt_source_not_admitted")
                observed = owner.NodePrebuiltAcquisition(temp / "node-prebuilt.json", node_prebuilt_sha256,
                    staging, io=io, node_owner=node_owner, source_reader=no_source_before_acquisition,
                    selected_node=pins["selected_node"]["path"])
                if (str(observed.python) != pins["python"]["path"]
                        or digest(observed.inputs[observed.python]) != pins["python"]["sha256"]):
                    raise PreparationError("node_prebuilt_python_mismatch")
        if acquire_mcpb:
            owner = owner_module("prepared_mcpb_acquisition", release / MCPB_ACQUISITION, retained[MCPB_ACQUISITION])
            observed = owner.McpbAcquisition(temp / "mcpb-acquisition.json", mcpb_sha256, staging, io=io)
            if (str(observed.python) != pins["python"]["path"]
                    or digest(observed.python_raw) != pins["python"]["sha256"]):
                raise PreparationError("mcpb_acquisition_python_mismatch")
        recheck_release(release, snapshot)
        recheck_pins(pins)
        if node_build is not None and node_build_options(args) != node_build:
            raise PreparationError("node_build_profile_changed")
        if playwright and digest(read_regular(playwright_path, backend.MAX_BODY)[0]) != playwright_sha256:
            raise PreparationError("playwright_host_profile_changed")
        if prior_enabled and digest(read_prior()) != prior_sha256:
            raise PreparationError("prior_request_changed")
        if acquire_mcpb and read_mcpb() != mcpb_raw:
            raise PreparationError("mcpb_acquisition_profile_changed")
        if acquire_node_prebuilt and read_node_prebuilt() != node_prebuilt_input_raw:
            raise PreparationError("node_prebuilt_profile_changed")
        if compose_profiles:
            # Recheck freshness after all private validation, before publication.
            current_profiles, current_execution, _ = compose_options(args)
            if current_profiles != compose_profiles or current_execution != compose_execution:
                raise PreparationError("compose_host_changed")
        if (new_path(workspace) != workspace or staging_path(staging, recovering=bool(recovery)) != staging
                or directory(release) != release or directory(output.parent) != output.parent):
            raise PreparationError("host_paths_changed")
        check_recipes()
        check_recovery()
        if prior_manifest and read_prior_manifest(backend, prior_manifest) != prior_source:
            raise PreparationError("prior_manifest_changed")
        os.mkdir(output, 0o700)
        for name, raw in files.items():
            write_new(output / name, raw)
        receipt = {"schema_version": 1, "status": "prepared", "scope": "host_setup_configuration_only",
                   "release": {"archive_sha256": args.release_sha256, "archive_root": snapshot["root"], "directory": str(release)},
                   "target": args.target, "workspace": str(workspace), "runtimes": pins,
                   "public_pypi_enabled": args.allow_public_pypi,
                   "public_npm_enabled": args.allow_public_npm,
                   "public_github_research_enabled": github_research,
                   "node_search_enabled": search_node,
                   "files": {name: digest(raw) for name, raw in files.items()}}
        if recovery:
            receipt["recovery"] = {"mode": "readback_only", "manifest_sha256": recovery[1],
                                   "workspace": str(recovery_root), "requests": len(recovery_entries)}
        if prior_manifest:
            receipt["prior_selection"] = {"source": "recovery_manifest", "manifest_sha256": prior_manifest[1],
                                          "request_id": prior_manifest[2], "request_sha256": prior_sha256}
        write_new(output / "prepared.json", encoded(receipt))
        sync_directory(output)
        sync_directory(output.parent)
    return {"status": "prepared", "scope": "host_setup_configuration_only", "directory": str(output),
            "receipt_sha256": digest(encoded(receipt))}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    for name in ("release-archive", "release-sha256", "release-dir", "output", "workspace", "staging-root",
                 "python", "python-sha256", "mcp-python", "mcp-python-sha256"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--families", default="agno,services,activepieces",
                        help="comma-separated agno, services, activepieces, official-github, official-playwright; official families require shipped target support")
    parser.add_argument("--allow-public-pypi", action="store_true")
    parser.add_argument("--allow-public-npm", action="store_true")
    parser.add_argument("--enable-public-github-research", action="store_true")
    parser.add_argument("--enable-node-search", action="store_true",
                        help="enable bounded public npm search with pinned selected Node and npm archive; acquisition remains a separate opt-in")
    parser.add_argument("--allow-mcpb", action="store_true",
                        help="enable the shipped MCPB installer with an explicitly pinned selected Node runtime; npm is not required")
    parser.add_argument("--enable-package-inspection", action="store_true")
    parser.add_argument("--enable-plugin-inspection", action="store_true",
                        help="allow bounded public Agent Plugins file reads below the staging root")
    parser.add_argument("--enable-acquired-inspection", action="store_true")
    parser.add_argument("--enable-mcpb-inspection", action="store_true",
                        help="allow inert staged MCPB inspection; requires --allow-mcpb and a supporting release")
    parser.add_argument("--recipes-scope", choices=("read", "retain"),
                        help="explicit existing recipe disclosure or retention scope; requires database, realm and owner")
    parser.add_argument("--allow-remote-recipes", action="store_true",
                        help="enable existing hosted recipe tools; requires --recipes-scope retain")
    parser.add_argument("--compose-mode", choices=("new", "reopen"))
    parser.add_argument("--compose-operation-timeout-seconds", type=int)
    for name in ("pip-zipapp", "pip-sha256", "node", "node-sha256", "npm-cli", "npm-cli-sha256",
                 "selected-node", "selected-node-sha256", "selected-npm-cli", "selected-npm-cli-sha256",
                 "npm-archive", "npm-archive-sha256", "playwright-host-profile", "playwright-host-profile-sha256",
                 "recipes-database", "recipes-realm", "recipes-owner", "prior-request", "prior-request-sha256",
                 "prior-manifest", "prior-manifest-sha256", "prior-request-id",
                 "recover-manifest", "recover-manifest-sha256",
                 "mcpb-acquisition-profile", "mcpb-acquisition-profile-sha256",
                 "node-prebuilt-profile", "node-prebuilt-profile-sha256",
                 "node-prebuilt-policy", "node-prebuilt-policy-sha256",
                 "node-build-profile", "node-build-profile-sha256",
                 "compose-acquisition-host-profile", "compose-acquisition-host-profile-sha256",
                 "compose-backend-profile", "compose-backend-profile-sha256", "compose-graph-workspace",
                 "compose-allowed-actions", "compose-expected-plan-sha256", "compose-delete-state-namespaces",
                 "compose-successor-inputs", "compose-successor-inputs-sha256",
                 "compose-successor-backend-profile", "compose-successor-backend-profile-sha256",
                 "compose-successor-transition-sha256"):
        parser.add_argument("--" + name)
    args = parser.parse_args(argv)
    try:
        result = prepare(args)
    except PreparationError as error:
        parser.exit(1, "Host preparation refused: " + str(error) + ".\n")
    except Exception:
        parser.exit(1, "Host preparation refused: input_or_output_unavailable.\n")
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
