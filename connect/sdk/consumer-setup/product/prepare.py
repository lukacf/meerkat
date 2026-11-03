#!/usr/bin/env python3
"""Internal installed-product preparation. The native caller verifies us first."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import signal
import stat
import sys
import threading
from types import ModuleType, SimpleNamespace

MANIFEST_LIMIT = 16 * 1024 * 1024
ENTRY_LIMIT = 50000
HOST_TARGETS = {("Darwin", "arm64"): "macos-arm64", ("Linux", "x86_64"): "linux-x86_64"}


class ResourceError(ValueError):
    pass


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ResourceError("duplicate_document_key")
        value[key] = item
    return value


def decode(raw):
    return json.loads(raw, object_pairs_hook=unique_object)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def relative(value):
    if (not isinstance(value, str) or not value or "\\" in value
            or PurePosixPath(value).is_absolute()
            or any(part in ("", ".", "..") for part in value.split("/"))):
        raise ResourceError("invalid_resource_path")
    return value


def tree_entries(root):
    """Exact immutable closure. Never traverse a symlink as a directory."""
    rows = []
    for directory, subdirs, files in os.walk(root, followlinks=False):
        for name in sorted(subdirs + files):
            path = Path(directory) / name
            rel = path.relative_to(root).as_posix()
            if rel == "resources.json":
                continue
            relative(rel)
            info = path.lstat()
            row = {"path": rel, "mode": stat.S_IMODE(info.st_mode)}
            if stat.S_ISLNK(info.st_mode):
                target = os.readlink(path)
                if os.path.isabs(target) or not path.resolve(strict=True).is_relative_to(root):
                    raise ResourceError("resource_link_escapes_root")
                row.update(kind="symlink", target=target)
            elif stat.S_ISDIR(info.st_mode):
                row.update(kind="directory")
            elif stat.S_ISREG(info.st_mode):
                row.update(kind="file", size=info.st_size, sha256=sha256(path))
            else:
                raise ResourceError("unsupported_resource_kind")
            rows.append(row)
            if len(rows) > ENTRY_LIMIT:
                raise ResourceError("resource_entry_limit")
    return sorted(rows, key=lambda row: row["path"])


def verify_resources(root):
    root = root.resolve(strict=True)
    manifest_path = root / "resources.json"
    if not stat.S_ISREG(manifest_path.lstat().st_mode):
        raise ResourceError("invalid_resource_manifest")
    with manifest_path.open("rb") as stream:
        raw = stream.read(MANIFEST_LIMIT + 1)
    if len(raw) > MANIFEST_LIMIT:
        raise ResourceError("resource_manifest_limit")
    value = decode(raw)
    if (value.get("schema_version") != 1 or value.get("kind") != "meerkat_connect_resources"
            or value.get("target") not in HOST_TARGETS.values()
            or not isinstance(value.get("entries"), list)):
        raise ResourceError("unsupported_resource_manifest")
    for key in ("prepare", "python", "node", "npm_cli", "starter", "archive"):
        relative(value[key])
    npm_resources(root, value)
    node_prebuilt_resources(root, value)
    node_build_resources(root, value)
    if tree_entries(root) != value["entries"]:
        raise ResourceError("resource_closure_changed")
    return value, hashlib.sha256(raw).hexdigest()


def npm_resources(root, manifest):
    """Optional supplied capability, with no partial or implicit runtime fallback."""
    if "npm_acquisition" not in manifest:
        return None
    value = manifest["npm_acquisition"]
    if not isinstance(value, dict) or set(value) != {"node", "npm_cli", "npm_archive"}:
        raise ResourceError("invalid_bundled_npm_acquisition_group")
    selected = {name: root / relative(path) for name, path in value.items()}
    if any(path.is_symlink() or not path.is_file() for path in selected.values()):
        raise ResourceError("bundled_npm_acquisition_unavailable")
    return selected


def node_prebuilt_resources(root, manifest):
    """Only an explicitly packaged profile enables this optional acquisition."""
    name = "node-prebuilt-policy.json"
    entries = [entry for entry in manifest.get("entries", []) if entry.get("path") == name]
    path = root / name
    if not entries and not path.exists() and not path.is_symlink():
        return None
    if (len(entries) != 1 or entries[0].get("kind") != "file"
            or path.is_symlink() or not path.is_file()
            or not 0 < path.stat().st_size <= 65536
            or entries[0].get("size") != path.stat().st_size
            or entries[0].get("sha256") != sha256(path)
            or npm_resources(root, manifest) is None):
        raise ResourceError("bundled_node_prebuilt_profile_unavailable")
    return {"path": path, "sha256": entries[0]["sha256"]}


def node_build_resources(root, manifest):
    """Discover exact optional profile bytes; external tool paths stay host-owned."""
    name = "node-build-profile.json"
    entries = [entry for entry in manifest.get("entries", []) if entry.get("path") == name]
    path = root / name
    if not entries and not path.exists() and not path.is_symlink():
        return None
    if (len(entries) != 1 or entries[0].get("kind") != "file"
            or npm_resources(root, manifest) is None):
        raise ResourceError("bundled_node_build_profile_unavailable")
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= 65536:
            raise ResourceError("bundled_node_build_profile_unavailable")
        raw = source.read(65537)
    if (len(raw) != info.st_size or entries[0].get("size") != len(raw)
            or entries[0].get("sha256") != hashlib.sha256(raw).hexdigest()):
        raise ResourceError("bundled_node_build_profile_unavailable")
    return {"path": path, "sha256": entries[0]["sha256"]}


def checked_receipt(host, path, expected):
    raw = host.read_regular(path, MANIFEST_LIMIT)[0]
    if host.digest(raw) != expected:
        raise ResourceError("preparation_receipt_changed")
    return decode(raw)


def local_projection(host, prepared, release, retained, configuration_sha256):
    """Project the verified preparer's command through its selected launcher."""
    helper_path = release / "setup/local_session.py"
    helper_raw = retained.get("setup/local_session.py")
    if helper_raw is None:
        raise ResourceError("local_setup_launcher_not_selected")
    helper = host.owner_module("product_local_setup", helper_path, helper_raw)
    binding = {"configuration_sha256": configuration_sha256,
               "helper_sha256": host.digest(helper_raw),
               "adapter_sha256": host.digest(retained["setup/mcp_adapter.py"]),
               "backend_sha256": host.digest(retained["setup/backend.py"])}
    selected = helper.prepare_server(configuration=prepared / "config/mcp.json",
        helper=helper_path, state_root=prepared / "local-session", **binding)
    descriptor = {"mcpServers": {"toolkit-setup": {
        "command": selected["command"], "args": selected["args"]}}}
    return helper, binding, descriptor


def local_workspace_evidence(helper, backend, prepared, workspace, binding, selected):
    """Observe the same selected attempt while its native MCP lease may be held."""
    state_root = prepared / "local-session"
    record, record_raw = helper.read_session_record(state_root, binding)
    try:
        checkpoint = helper.selected_checkpoint(backend, state_root, record, workspace)
    except helper.LocalSetupError as error:
        if str(error) == "local_setup_attempt_mismatch":
            raise ResourceError("readback_local_attempt_mismatch") from None
        raise
    _, entries = backend.read_recovery_selection(checkpoint)
    if not any(entry["id"] == selected["job_id"]
               and entry["sha256"] == selected["recovery_request_sha256"] for entry in entries):
        raise ResourceError("readback_local_request_not_selected")
    return record_raw, checkpoint


def prepare(root, destination, stopped, *, realm_state_root=None, realm=None):
    manifest, manifest_sha = verify_resources(root)
    if HOST_TARGETS.get((platform.system(), platform.machine())) != manifest["target"]:
        raise ResourceError("unsupported_product_target")
    python = (root / manifest["python"]).resolve(strict=True)
    if Path(sys.executable).resolve(strict=True) != python:
        raise ResourceError("invoke_with_bundled_python")
    destination = Path(destination)
    if (not destination.is_absolute() or destination.exists() or destination.is_symlink()
            or destination.parent.resolve(strict=True) != destination.parent
            or destination.is_relative_to(root) or root.is_relative_to(destination)):
        raise ResourceError("fresh_disjoint_destination_required")
    # Product-owned acquisition uses the pinned trusted pip already in this
    # immutable closure. It downloads inert wheels only; the native tool ceiling
    # and the separate checked-install decision still govern agent actions.
    pip = root / "runtime/pip.pyz"
    if pip.is_symlink() or not pip.is_file():
        raise ResourceError("bundled_python_acquisition_unavailable")
    npm = npm_resources(root, manifest)
    # These exact starter bytes belong to the selected distribution. Native
    # verification precedes Python startup; this recheck precedes owner entry.
    starter = ModuleType("product_starter")
    starter.__file__ = str(root / manifest["starter"])
    exec(compile(Path(starter.__file__).read_bytes(), starter.__file__, "exec"), starter.__dict__)
    args = SimpleNamespace(archive_dir=str((root / manifest["archive"]).parent),
        destination=str(destination), node=str(root / manifest["node"]),
        npm_cli=str(root / manifest["npm_cli"]), allow_public_pypi=True,
        pip_zipapp=str(pip), timeout_seconds=300, enable_public_github_research=True)
    if npm is not None:
        args.allow_public_npm = args.enable_node_search = True
        args.selected_node = str(npm["node"])
        args.selected_npm_cli = str(npm["npm_cli"])
        args.npm_archive = str(npm["npm_archive"])
    node_prebuilt = node_prebuilt_resources(root, manifest)
    if node_prebuilt is not None:
        args.node_prebuilt_policy = str(node_prebuilt["path"])
        args.node_prebuilt_policy_sha256 = node_prebuilt["sha256"]
    node_build = node_build_resources(root, manifest)
    if node_build is not None:
        args.node_build_profile = str(node_build["path"])
        args.node_build_profile_sha256 = node_build["sha256"]
    if (realm is None) != (realm_state_root is None):
        raise ResourceError("complete_native_realm_required")
    if realm is not None:
        if (not isinstance(realm, str) or not realm or len(realm) > 256
                or any(character not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.-" for character in realm)
                or realm in (".", "..")):
            raise ResourceError("invalid_native_realm")
        state_root = starter.host.directory(realm_state_root)
        realm_parent = state_root / ".meerkat-connect" / realm
        if (destination.name != "prepared" or destination.parent.parent != realm_parent
                or not destination.parent.name.startswith("setup-")):
            raise ResourceError("preparation_realm_mismatch")
        args.recipes_database = str(realm_parent / "recipes.sqlite")
        args.recipes_realm = realm
        args.recipes_owner = "meerkat-connect"
        args.recipes_scope = "retain"
        args.allow_remote_recipes = True
    result = starter.start(args, stopped=stopped)
    host = starter.host
    bootstrap = result["bootstrap"]
    receipt = checked_receipt(host, destination / "config/prepared.json", bootstrap["host"]["receipt_sha256"])
    checked_receipt(host, destination / "client/installed.json", bootstrap["client"]["receipt_sha256"])
    if (receipt.get("status") != "prepared" or receipt.get("scope") != "host_setup_configuration_only"
            or receipt.get("target") != manifest["target"]
            or receipt.get("public_pypi_enabled") is not True
            or receipt.get("runtimes", {}).get("pip", {}).get("sha256") != sha256(pip)
            or receipt.get("workspace") != str(destination / "session")
            or receipt.get("release", {}).get("archive_sha256") != sha256(root / manifest["archive"])):
        raise ResourceError("preparation_scope_mismatch")
    if node_prebuilt is not None and (
            "node-prebuilt.json" not in receipt.get("files", {})
            or receipt.get("runtimes", {}).get("node_prebuilt_policy", {}).get("sha256") != node_prebuilt["sha256"]):
        raise ResourceError("node_prebuilt_preparation_scope_mismatch")
    if node_build is not None and (
            receipt.get("files", {}).get("node-build-profile.json") != node_build["sha256"]
            or receipt.get("runtimes", {}).get("node_build_profile", {}).get("sha256") != node_build["sha256"]):
        raise ResourceError("node_build_preparation_scope_mismatch")
    files = receipt.get("files", {})
    for name, digest in files.items():
        relative(name)
        checked_receipt(host, destination / "config" / name, digest)
    config = decode(host.read_regular(destination / "config/mcp.json", MANIFEST_LIMIT)[0])
    if (set(config) != {"mcpServers"} or set(config["mcpServers"]) != {"toolkit-setup"}
            or set(config["mcpServers"]["toolkit-setup"]) != {"command", "args"}
            or config["mcpServers"]["toolkit-setup"]["command"] != "/usr/bin/env"):
        raise ResourceError("invalid_prepared_mcp_projection")
    if "--allow-acquired-inspection" not in config["mcpServers"]["toolkit-setup"]["args"]:
        raise ResourceError("acquired_inspection_not_prepared")
    if (receipt.get("public_npm_enabled", False) is not (npm is not None)
            or receipt.get("node_search_enabled", False) is not (npm is not None)):
        raise ResourceError("npm_acquisition_scope_mismatch")
    if npm is not None:
        profile = checked_receipt(host, destination / "config/node-acquisition.json", files["node-acquisition.json"])
        selected = checked_receipt(host, destination / "config/selected-install.json", files["selected-install.json"])
        runtime_names = {"node": "selected_node", "npm_cli": "selected_npm_cli", "npm_archive": "npm_archive"}
        if (profile.get("scope") != "trusted_npm_registry_acquisition"
                or profile.get("allow_acquisition") is not True
                or profile.get("target", "macos-arm64") != manifest["target"]
                or profile.get("node_sha256") != sha256(npm["node"])
                or profile.get("npm") != {"path": str(npm["npm_archive"]), "sha256": sha256(npm["npm_archive"])}
                or any(receipt.get("runtimes", {}).get(runtime_names[name], {}).get("sha256") != sha256(path)
                       for name, path in npm.items())
                or "node" not in selected.get("validators", {})
                or "--allow-node-search" not in config["mcpServers"]["toolkit-setup"]["args"]):
            raise ResourceError("npm_acquisition_not_prepared")
        if node_build is not None and selected.get("node_build") != {
                "path": str(destination / "config/node-build-profile.json"),
                "sha256": node_build["sha256"]}:
            raise ResourceError("node_build_not_prepared")
    if realm is not None:
        profile = checked_receipt(host, destination / "config/local-recipes.json", files["local-recipes.json"])
        if (profile.get("database") != args.recipes_database or profile.get("realm") != realm
                or profile.get("owner") != "meerkat-connect"
                or profile.get("scope") != "retain_and_disclose_nonsecret_local_recipe_metadata"
                or "--allow-remote-recipes" not in config["mcpServers"]["toolkit-setup"]["args"]):
            raise ResourceError("remote_recipes_not_prepared")
    archive = root / manifest["archive"]
    release = destination / "distribution" / archive.name.removesuffix(".tar.gz")
    snapshot, retained = host.release_snapshot(str(archive), sha256(archive), release, manifest["target"])
    _, binding, config = local_projection(host, destination, release, retained, files["mcp.json"])
    descriptor_raw = (json.dumps(config, sort_keys=True) + "\n").encode()
    if stopped.is_set():
        raise ResourceError("preparation_cancelled")
    host.recheck_release(release, snapshot)
    verify_resources(root)
    host.write_new(destination / "product-mcp.json", descriptor_raw)
    # Keep the owner's result on disk, not mixed with the MCP command on stdout.
    host.write_new(destination / "product-prepared.json", (json.dumps({
        "resources_sha256": manifest_sha, "preparation": result,
        "mcp_sha256": files["mcp.json"], "local_setup": {
            "descriptor_sha256": host.digest(descriptor_raw),
            "helper_sha256": binding["helper_sha256"]}}, indent=2) + "\n").encode())
    return config


# Private transport for the existing Rust host. These locators are never agent
# selection arguments; the host presents its own current-session handles.
RETAINED_PAGE = 32
RETAINED_SETUPS = 4096
SETUP_NAME = re.compile(r"setup-[A-Za-z0-9_-]{1,128}\Z")
WORK_NAME = re.compile(r"work-[0-9a-f]{32}\Z")
JOB_NAME = re.compile(r"[0-9a-f]{32}\Z")
SHA_NAME = re.compile(r"[0-9a-f]{64}\Z")


def retained_name(value, pattern):
    return isinstance(value, str) and pattern.fullmatch(value) is not None


def private_directory(path):
    info = path.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077
            or path.resolve(strict=True) != path):
        raise ResourceError("retained_private_directory_required")
    return path


def retained_parent(host, request):
    realm = request["realm"]
    if not isinstance(realm, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,256}", realm) or realm in (".", ".."):
        raise ResourceError("invalid_retained_realm")
    state = Path(request["realm_state_root"])
    if not state.is_absolute():
        raise ResourceError("invalid_retained_realm")
    # Native keeps its selected root spelling, including filesystem aliases.
    # Match preparation and direct readback; product descendants remain private
    # canonical directories checked below, not additional alias entry points.
    state = host.directory(str(state))
    parent = state / ".meerkat-connect" / realm
    # A realm with no setup artifacts has no retained requests. Symlinks and
    # nonprivate artifact parents do not become an alternate discovery root.
    if not parent.exists() and not parent.is_symlink():
        return None
    private_directory(parent.parent)
    return private_directory(parent)


def retained_preparation(root, manifest, manifest_sha, host, parent, setup):
    if not retained_name(setup, SETUP_NAME):
        raise ResourceError("invalid_retained_locator")
    prepared = private_directory(parent / setup) / "prepared"
    if not stat.S_ISDIR(prepared.lstat().st_mode) or prepared.resolve(strict=True) != prepared:
        raise ResourceError("retained_preparation_unconfirmed")
    archive = root / manifest["archive"]
    release = prepared / "distribution" / archive.name.removesuffix(".tar.gz")
    if host.directory(str(release)) != release:
        raise ResourceError("retained_preparation_changed")
    snapshot, retained = host.release_snapshot(str(archive), sha256(archive), release, manifest["target"])
    backend = host.owner_module("product_readback_backend", release / "setup/backend.py", retained["setup/backend.py"])
    product_raw = host.read_regular(prepared / "product-prepared.json", MANIFEST_LIMIT)[0]
    product = decode(product_raw)
    receipt = checked_receipt(host, prepared / "config/prepared.json",
        product["preparation"]["bootstrap"]["host"]["receipt_sha256"])
    if (product["resources_sha256"] != manifest_sha or receipt.get("target") != manifest["target"]
            or receipt.get("workspace") != str(prepared / "session")
            or receipt.get("release", {}).get("archive_sha256") != sha256(archive)
            or product["mcp_sha256"] != receipt["files"]["mcp.json"]):
        raise ResourceError("retained_preparation_changed")
    helper, binding, descriptor = local_projection(host, prepared, release, retained, product["mcp_sha256"])
    descriptor_raw = host.read_regular(prepared / "product-mcp.json", MANIFEST_LIMIT)[0]
    if (decode(descriptor_raw) != descriptor
            or product["local_setup"] != {"descriptor_sha256": host.digest(descriptor_raw),
                                          "helper_sha256": binding["helper_sha256"]}):
        raise ResourceError("retained_preparation_changed")
    record, record_raw = helper.read_session_record(prepared / "local-session", binding)
    return SimpleNamespace(prepared=prepared, release=release, snapshot=snapshot, backend=backend,
        helper=helper, binding=binding, record=record, record_raw=record_raw, product_raw=product_raw,
        descriptor_raw=descriptor_raw, receipt=receipt)


def retained_checkpoint(context, workspace):
    # Read one record-declared round. Do not acquire a lease, advance a round,
    # reopen a runtime, or scan work directories for omitted requests.
    root = context.prepared / "local-session"
    record = context.record
    if workspace != record["workspace"] and not any(row["workspace"] == workspace for row in record.get("prior", [])):
        raise ResourceError("retained_workspace_not_selected")
    requests = root / workspace / "requests"
    if not stat.S_ISDIR(requests.lstat().st_mode) or requests.resolve(strict=True) != requests:
        raise ResourceError("retained_requests_unconfirmed")
    # The canonical closure check below is kept intact. Bound its directory
    # under the same exclusive host custody before it enumerates child names.
    with os.scandir(requests) as children:
        for number, _ in enumerate(children):
            if number >= 256:
                raise ResourceError("retained_request_limit")
    if workspace == record["workspace"]:
        raw = context.helper.read_recovery_checkpoint(context.backend, root, record)
    else:
        prior = next((row for row in record.get("prior", []) if row["workspace"] == workspace), None)
        if prior is None:
            raise ResourceError("retained_workspace_not_selected")
        raw = context.helper.read_recovery_checkpoint(context.backend, root,
            {"schema_version": 2, "workspace": workspace})
        if context.helper.digest(raw) != prior["recovery_sha256"]:
            raise ResourceError("retained_checkpoint_changed")
    selected_root, entries = context.backend.read_recovery_selection(raw)
    if selected_root != root / workspace:
        raise ResourceError("retained_workspace_changed")
    return raw, selected_root, entries


def retained_recheck(root, manifest, manifest_sha, host, parent, setup, context, workspace, checkpoint):
    current = retained_preparation(root, manifest, manifest_sha, host, parent, setup)
    if (current.record_raw != context.record_raw or current.product_raw != context.product_raw
            or current.descriptor_raw != context.descriptor_raw
            or retained_checkpoint(current, workspace)[0] != checkpoint):
        raise ResourceError("retained_selection_changed")
    host.recheck_release(context.release, context.snapshot)


def retained_page(root, manifest, manifest_sha, host, request, stopped):
    query = request["retained_installations"]
    if set(query) != {"action", "cursor"}:
        raise ResourceError("invalid_retained_request")
    cursor = query["cursor"]
    if cursor is not None and (not isinstance(cursor, dict) or set(cursor) != {"setup", "workspace", "after_job"}
            or not retained_name(cursor["setup"], SETUP_NAME)
            or cursor["workspace"] != "" and not retained_name(cursor["workspace"], WORK_NAME)
            or cursor["after_job"] != "" and not retained_name(cursor["after_job"], JOB_NAME)):
        raise ResourceError("invalid_retained_cursor")
    page = {"kind": "retained_installation_page", "candidates": [], "next_cursor": None, "unconfirmed": False}
    parent = retained_parent(host, request)
    if parent is None:
        return page
    names = []
    with os.scandir(parent) as entries:
        for number, entry in enumerate(entries):
            if stopped.is_set():
                raise ResourceError("readback_cancelled")
            if number >= RETAINED_SETUPS:
                raise ResourceError("retained_enumeration_limit")
            if retained_name(entry.name, SETUP_NAME):
                names.append(entry.name)
    names.sort()
    setup = names[0] if cursor is None and names else cursor["setup"] if cursor is not None else None
    if setup is None:
        return page
    later = next((name for name in names if name > setup), None)
    if later is not None:
        page["next_cursor"] = {"setup": later, "workspace": "", "after_job": ""}
    # A readback-only artifact directory has no prepared owner. A partial or
    # incompatible preparation yields a fixed omission marker, not a global
    # failure that hides the next setup. Exact selection never falls back.
    try:
        directory = private_directory(parent / setup)
        if not (directory / "prepared").exists() and not (directory / "prepared").is_symlink():
            return page
        context = retained_preparation(root, manifest, manifest_sha, host, parent, setup)
        workspaces = sorted([context.record["workspace"], *[row["workspace"] for row in context.record.get("prior", [])]])
        workspace = cursor["workspace"] if cursor is not None and cursor["workspace"] else workspaces[0]
        if workspace not in workspaces:
            raise ResourceError("invalid_retained_cursor")
        following = next((name for name in workspaces if name > workspace), None)
        if following is not None:
            page["next_cursor"] = {"setup": setup, "workspace": following, "after_job": ""}
        checkpoint, selected_root, entries = retained_checkpoint(context, workspace)
        after = cursor["after_job"] if cursor is not None and cursor["workspace"] else ""
        remaining = sorted((entry for entry in entries if entry["id"] > after), key=lambda entry: entry["id"])
        window = remaining[:RETAINED_PAGE]
        if len(remaining) > len(window):
            page["next_cursor"] = {"setup": setup, "workspace": workspace, "after_job": window[-1]["id"]}
        for entry in window:
            if stopped.is_set():
                raise ResourceError("readback_cancelled")
            try:
                summary = context.backend.retained_request_summary(selected_root, entry)
                if summary is not None:
                    page["candidates"].append({"locator": {"setup": setup, "workspace": workspace,
                        "job_id": entry["id"], "request_sha256": entry["sha256"], "checkpoint_sha256": host.digest(checkpoint)},
                        "family": summary["family"], "profile": summary["profile"]})
            except Exception:
                page["unconfirmed"] = True
        retained_recheck(root, manifest, manifest_sha, host, parent, setup, context, workspace, checkpoint)
    except Exception:
        # All preparation/checkpoint errors remain fixed and private. A selected
        # readback uses the strict branch below and cannot skip to another item.
        if stopped.is_set():
            raise ResourceError("readback_cancelled") from None
        page["candidates"] = []
        page["unconfirmed"] = True
    return page


def retained_selection(root, manifest, manifest_sha, host, request):
    query = request["retained_installations"]
    if set(query) != {"action", "locator"} or not isinstance(query["locator"], dict):
        raise ResourceError("invalid_retained_request")
    locator = query["locator"]
    if (set(locator) != {"setup", "workspace", "job_id", "request_sha256", "checkpoint_sha256"}
            or not retained_name(locator["setup"], SETUP_NAME) or not retained_name(locator["workspace"], WORK_NAME)
            or not retained_name(locator["job_id"], JOB_NAME)
            or any(not retained_name(locator[name], SHA_NAME) for name in ("request_sha256", "checkpoint_sha256"))):
        raise ResourceError("invalid_retained_locator")
    parent = retained_parent(host, request)
    if parent is None:
        raise ResourceError("retained_selection_unconfirmed")
    context = retained_preparation(root, manifest, manifest_sha, host, parent, locator["setup"])
    checkpoint, workspace, entries = retained_checkpoint(context, locator["workspace"])
    if host.digest(checkpoint) != locator["checkpoint_sha256"]:
        raise ResourceError("retained_checkpoint_changed")
    entry = next((entry for entry in entries if entry["id"] == locator["job_id"] and entry["sha256"] == locator["request_sha256"]), None)
    if entry is None:
        raise ResourceError("retained_request_not_selected")
    summary = context.backend.retained_request_summary(workspace, entry)
    if summary is None:
        raise ResourceError("retained_metadata_unavailable")
    mcp = workspace / "installations" / summary["validation_id"] / "ready/mcp.json"
    return {**summary, "job_id": locator["job_id"], "recovery_request_sha256": locator["request_sha256"],
        "mcp_path": str(mcp), "receipt_path": str(mcp.with_name("installed.json"))}, context, checkpoint, parent


def credential_readback_slots(evidence, selected_package):
    if (not isinstance(evidence, dict)
            or set(evidence) != {"operation_basis", "operations", "credential_slots"}
            or not isinstance(evidence["operation_basis"], dict)):
        raise ResourceError("invalid_credential_readback")
    operations = evidence["operations"]
    if (not isinstance(operations, list) or not 1 <= len(operations) <= 128
            or any(not isinstance(name, str) or re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", name) is None for name in operations)
            or len(set(operations)) != len(operations)):
        raise ResourceError("invalid_credential_readback")
    slots = evidence["credential_slots"]
    if not isinstance(slots, list) or not 1 <= len(slots) <= 256:
        raise ResourceError("invalid_credential_readback")
    names, paths = set(), set()
    valid_name = lambda value: isinstance(value, str) and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,127}", value) is not None
    for slot in slots:
        if not isinstance(slot, dict) or set(slot) != {"source", "declaration"}:
            raise ResourceError("invalid_credential_readback")
        declaration = slot["declaration"]
        if selected_package:
            if slot["source"] != "selected_environment":
                raise ResourceError("invalid_credential_readback")
            name = declaration
        else:
            if (slot["source"] != "library_field" or not isinstance(declaration, dict)
                    or set(declaration) != {"field_path", "environment_variable"}):
                raise ResourceError("invalid_credential_readback")
            path = declaration["field_path"]
            if (not isinstance(path, list) or not 1 <= len(path) <= 2
                    or not all(valid_name(key) for key in path) or tuple(path) in paths):
                raise ResourceError("invalid_credential_readback")
            paths.add(tuple(path))
            name = declaration["environment_variable"]
        if not valid_name(name) or name in names:
            raise ResourceError("invalid_credential_readback")
        names.add(name)
    if not selected_package and evidence["operation_basis"] != {"kind": "package_published_metadata"}:
        raise ResourceError("invalid_credential_readback")
    if selected_package:
        authentication = evidence["operation_basis"].get("authentication")
        if not isinstance(authentication, dict) or authentication.get("kind") != "binding":
            raise ResourceError("invalid_credential_readback")
    return names


def installation_readback(root, destination, request_path, stopped):
    """One package-owner read, using current product code and an exact old request.

    Ordinary account/public/credential readback starts no acquired code. The explicit
    retained selection branch additionally observes library config-help/check-config
    metadata from the verified existing adapter. Neither path starts an install,
    agent, authentication or MCP serving process. The canonical backend owns
    request decoding and installed-artifact checking.
    """
    manifest, manifest_sha = verify_resources(root)
    if HOST_TARGETS.get((platform.system(), platform.machine())) != manifest["target"]:
        raise ResourceError("unsupported_product_target")
    python = (root / manifest["python"]).resolve(strict=True)
    if Path(sys.executable).resolve(strict=True) != python:
        raise ResourceError("invoke_with_bundled_python")
    starter = ModuleType("product_readback_starter")
    starter.__file__ = str(root / manifest["starter"])
    exec(compile(Path(starter.__file__).read_bytes(), starter.__file__, "exec"), starter.__dict__)
    host = starter.host
    raw = host.read_regular(Path(request_path), 1024 * 1024)[0]
    request = decode(raw)
    retained_query = request.get("retained_installations")
    retained_context = None
    if retained_query is not None:
        if (set(request) != {"retained_installations", "realm_state_root", "realm"}
                or not isinstance(retained_query, dict)):
            raise ResourceError("invalid_retained_request")
        if stopped.is_set():
            raise ResourceError("readback_cancelled")
        if retained_query.get("action") == "list":
            page = retained_page(root, manifest, manifest_sha, host, request, stopped)
            if stopped.is_set() or host.read_regular(Path(request_path), 1024 * 1024)[0] != raw:
                raise ResourceError("readback_cancelled_or_changed")
            verify_resources(root)
            return page
        if retained_query.get("action") != "select":
            raise ResourceError("invalid_retained_request")
        selected, retained_context, retained_checkpoint_raw, retained_realm_parent = retained_selection(
            root, manifest, manifest_sha, host, request)
        request = {"software": selected, "realm_state_root": request["realm_state_root"], "realm": request["realm"]}
    public = request.get("public_service")
    credential = request.get("credential_service")
    remote = "remote_export" in request
    source_key = "remote_export" if remote else "software"
    evidence_key = "credential_service" if credential is not None else "public_service"
    service = credential if credential is not None else public
    if (set(request) != {source_key, "realm_state_root", "realm", *([evidence_key] if service is not None else [])}
            or not isinstance(request[source_key], dict)):
        raise ResourceError("invalid_readback_request")
    selected = request[source_key]
    selected_package = selected.get("family") in ("selected-python", "selected-node")
    # The protected native caller selects the account, public or credential
    # branch. Each reuses the canonical installed-artifact owner.
    if (retained_context is None and (remote and (public is None or credential is not None)
            or not remote and service is None and selected.get("family") != "services"
            and not (selected.get("family") == "agno" and selected.get("profile") == "salesforce")
            or service is not None and (not isinstance(service, dict)
                or credential is None and set(service) != {"operation_basis", "operations"}
                or not remote and selected.get("family") not in ("agno", "services", "activepieces", "selected-python", "selected-node")))):
        raise ResourceError("unsupported_readback_owner")
    credential_slots = credential_readback_slots(credential, selected_package) if credential is not None else None
    if remote:
        if (set(selected) != {"job_id", "request_sha256", "recovery_request_sha256", "export"}
                or not isinstance(selected["export"], dict)):
            raise ResourceError("invalid_remote_readback_request")
        exported = selected["export"]
        mcp = Path(exported["mcp_path"])
        workspace = host.directory(str(mcp.parents[3]))
        expected_directory = workspace / "remote-recipes" / ("export-" + selected["job_id"]) / "output"
        if (mcp != expected_directory / "mcp.json" or exported["destination"] != str(expected_directory)
                or exported["plugin_path"] != str(expected_directory / "plugin.json")):
            raise ResourceError("readback_owner_mismatch")
    else:
        mcp = Path(selected["mcp_path"])
        workspace = host.directory(str(mcp.parents[3]))
        expected_mcp = workspace / "installations" / selected["validation_id"] / "ready/mcp.json"
        if mcp != expected_mcp or Path(selected["receipt_path"]) != mcp.with_name("installed.json"):
            raise ResourceError("readback_owner_mismatch")
    local_attempt = workspace.parent.name == "local-session"
    prepared = workspace.parent.parent if local_attempt else workspace.parent
    realm_parent = host.directory(request["realm_state_root"]) / ".meerkat-connect" / request["realm"]
    if ((not local_attempt and workspace.name != "session") or prepared.name != "prepared"
            or prepared.parent.parent != realm_parent
            or not prepared.parent.name.startswith("setup-")):
        raise ResourceError("readback_realm_mismatch")
    destination = host.new_path(destination)
    if (destination.parent.resolve(strict=True) != destination.parent
            or any(host.compose_overlaps(destination, path) for path in (root, workspace, prepared))):
        raise ResourceError("readback_destination_mismatch")
    archive = root / manifest["archive"]
    release = prepared / "distribution" / archive.name.removesuffix(".tar.gz")
    snapshot, retained = host.release_snapshot(str(archive), sha256(archive), release, manifest["target"])
    backend = host.owner_module("product_readback_backend", release / "setup/backend.py", retained["setup/backend.py"])
    if local_attempt or selected_package or remote:
        product_raw = host.read_regular(prepared / "product-prepared.json", MANIFEST_LIMIT)[0]
        product = decode(product_raw)
        receipt = checked_receipt(host, prepared / "config/prepared.json",
            product["preparation"]["bootstrap"]["host"]["receipt_sha256"])
        if (product["resources_sha256"] != manifest_sha
                or receipt.get("target") != manifest["target"]
                or receipt.get("workspace") != str(prepared / "session")
                or receipt.get("release", {}).get("archive_sha256") != sha256(archive)
                or product["mcp_sha256"] != receipt["files"]["mcp.json"]):
            raise ResourceError("readback_preparation_changed")
    if local_attempt:
        helper, binding, descriptor = local_projection(
            host, prepared, release, retained, product["mcp_sha256"])
        descriptor_raw = host.read_regular(prepared / "product-mcp.json", MANIFEST_LIMIT)[0]
        if (product["local_setup"] != {"descriptor_sha256": host.digest(descriptor_raw),
                                      "helper_sha256": binding["helper_sha256"]}
                or decode(descriptor_raw) != descriptor):
            raise ResourceError("readback_local_descriptor_changed")
        evidence = (local_workspace_evidence(helper, backend, prepared, workspace, binding, selected)
            if retained_context is None else (retained_context.record_raw, retained_checkpoint_raw))
    selected_options = {}
    if selected_package:
        # The original product receipt pins this fixed preparation profile and
        # its installer/validator closure. A request cannot choose a new owner.
        profile_path = prepared / "config/selected-install.json"
        profile_sha = receipt["files"]["selected-install.json"]
        checked_receipt(host, profile_path, profile_sha)
        selected_options = {"selected_install_profile": profile_path,
                            "selected_install_profile_sha256": profile_sha}
        # Reconstruct the same selected host that preparation bound, including
        # the separate acquisition runtime for both Node and Python packages.
        npm = npm_resources(root, manifest)
        if npm is not None:
            selected_options.update(selected_node=str(npm["node"]),
                                    selected_npm_cli=str(npm["npm_cli"]))
    if remote:
        profile_path = prepared / "config/local-recipes.json"
        profile_sha = receipt["files"]["local-recipes.json"]
        profile = checked_receipt(host, profile_path, profile_sha)
        if (profile.get("database") != str(realm_parent / "recipes.sqlite")
                or profile.get("realm") != request["realm"] or profile.get("owner") != "meerkat-connect"
                or profile.get("scope") != backend.LOCAL_RECIPES_RETAIN_SCOPE):
            raise ResourceError("remote_readback_profile_changed")
        selected_options = {"local_recipes_profile": profile_path,
                            "local_recipes_profile_sha256": profile_sha}
    if stopped.is_set():
        raise ResourceError("readback_cancelled")
    owner = backend.Controller(
        bundle=release / "consumers", contents_sha256=snapshot["hashes"]["consumers/CONTENTS.json"],
        workspace=destination, families=() if selected_package or remote else (selected["family"],), python=str(python),
        node=str((root / manifest["node"]).resolve(strict=True)),
        npm_cli=str((root / manifest["npm_cli"]).resolve(strict=True)),
        verifier=release / "consumers/install-library-consumer.py",
        verifier_sha256=snapshot["hashes"]["consumers/install-library-consumer.py"],
        recover_workspace=workspace, recover_request=selected["job_id"],
        recover_request_sha256=selected["recovery_request_sha256"],
        **selected_options,
    )
    # Reuse the runner's actual cancellation owner for this synchronous read.
    # It checks before spawn and while waiting, then joins its owned child.
    control = {"requested": stopped, "spawn_lock": threading.Lock()}
    owner._job_context.control = control
    try:
        if remote:
            detail = owner.job_detail({"job_id": selected["job_id"]})
            if (detail["job"]["kind"] != "export" or detail["job"]["family"] != "remote-recipe"
                    or detail["job"]["request_sha256"] != selected["request_sha256"]
                    or detail["job"]["recovery_request"] != {"id": selected["job_id"], "sha256": selected["recovery_request_sha256"]}):
                raise ResourceError("remote_readback_request_changed")
        value = owner.reconcile({"job_id": selected["job_id"]},
            **({"retained_setup_metadata": True} if retained_context is not None else {}))
        if selected_package and retained_context is None and value.get("observation", {}).get("status") == "matched":
            result = value["observation"]["result"]
            basis = service["operation_basis"]
            _, effect, plan = backend.selected_proposal(workspace, selected["validation_id"],
                                                        basis["effect_sha256"], owner.selected_binding)
            identity = backend.selected_identity(plan, effect["request"])
            expected_kind = "selected_software_declarations" if "document_kind" in identity else "selected_manifest_declarations"
            if (basis.get("kind") != expected_kind
                    or backend.selected_identity(basis, effect["request"]) != identity
                    or backend.selected_identity(result, effect["request"]) != identity
                    or basis.get("authentication") != plan.get("declared_service", {}).get("authentication")
                    or (basis.get("authentication", {}).get("kind") != "binding" if credential is not None
                        else basis.get("authentication") != {"kind": "none"})
                    or result.get("declared_service") != plan.get("declared_service")
                    or basis["package_sha256"] != effect["files"]["package.json"]
                    or any(result.get(key) != basis.get(key) for key in
                           ("selection_sha256", "effect_sha256"))
                    or basis.get("source") != {"plugin_source": result.get("plugin_source"),
                                               "acquired_source": result.get("acquired_source")}):
                raise ResourceError("readback_selected_declaration_changed")
            if credential is not None:
                declared = plan["declared_service"]
                slots = declared.get("environment_secret_slots")
                if (not isinstance(slots, list) or len(slots) != len(credential_slots)
                        or any(not isinstance(name, str) for name in slots) or set(slots) != credential_slots
                        or not set(credential["operations"]) <= set(declared["operation_tools"].values())):
                    raise ResourceError("readback_selected_declaration_changed")
            # This input hash comes from the same canonically checked proposal;
            # the actual --verify-existing read above remains mandatory.
            result["package_sha256"] = effect["files"]["package.json"]
    finally:
        owner.close_admission()
        del owner._job_context.control
    if stopped.is_set() or host.read_regular(Path(request_path), 1024 * 1024)[0] != raw:
        raise ResourceError("readback_cancelled_or_changed")
    host.recheck_release(release, snapshot)
    verify_resources(root)
    if local_attempt or selected_package or remote:
        if host.read_regular(prepared / "product-prepared.json", MANIFEST_LIMIT)[0] != product_raw:
            raise ResourceError("readback_local_attempt_changed")
        checked_receipt(host, prepared / "config/prepared.json",
            product["preparation"]["bootstrap"]["host"]["receipt_sha256"])
    if selected_package or remote:
        checked_receipt(host, profile_path, profile_sha)
    if local_attempt:
        if (host.read_regular(prepared / "product-mcp.json", MANIFEST_LIMIT)[0] != descriptor_raw
                or (local_workspace_evidence(helper, backend, prepared, workspace, binding, selected)
                    if retained_context is None else (
                        helper.read_session_record(prepared / "local-session", binding)[1],
                        retained_checkpoint(retained_context, retained_query["locator"]["workspace"])[0])) != evidence):
            raise ResourceError("readback_local_attempt_changed")
        local_projection(host, prepared, release, retained, product["mcp_sha256"])
    observation = value.get("observation", {})
    if observation.get("status") != "matched" or observation.get("job_id") != selected["job_id"]:
        raise ResourceError("installation_unconfirmed")
    if remote:
        result = observation["result"]
        basis = public["operation_basis"]
        current = result.get("configuration")
        if (observation.get("scope") != backend.REMOTE_EXPORT_READBACK
                or result.get("scope") != backend.REMOTE_EXPORT_READBACK
                or result.get("export_check", {}).get("state") != "matches"
                or current != exported
                or basis != {"kind": "remote_recipe_declarations", "recipe": exported["recipe"],
                    "selection_digest": exported["selection_digest"],
                    "declared_remote_service": exported.get("declared_remote_service")}):
            raise ResourceError("remote_export_changed")
        declaration = current["declared_remote_service"]
        operations = public["operations"]
        if (declaration.get("authentication") != {"kind": "none"}
                or not isinstance(operations, list) or not operations or len(operations) > 128
                or len(set(operations)) != len(operations)
                or not set(operations) <= set(declaration["operation_tools"].values())):
            raise ResourceError("remote_public_declaration_changed")
        return current
    if retained_context is not None:
        retained_recheck(root, manifest, manifest_sha, host, retained_realm_parent,
            retained_query["locator"]["setup"], retained_context,
            retained_query["locator"]["workspace"], retained_checkpoint_raw)
        if "setup_metadata" not in observation:
            raise ResourceError("retained_metadata_unavailable")
        result = observation["result"]
        return {"kind": "retained_installation_readback", "locator": retained_query["locator"],
            "installation": {**result, **{name: selected[name] for name in
                ("job_id", "validation_id", "recovery_request_sha256", "family", "profile")}},
            "metadata": observation["setup_metadata"]}
    return observation["result"]


def local_round_owner(root, request_path, stopped, cancelled_fd):
    """A single live continuation offer from the verified preparation owner."""
    manifest, manifest_sha = verify_resources(root)
    if (HOST_TARGETS.get((platform.system(), platform.machine())) != manifest["target"]
            or Path(sys.executable).resolve(strict=True) != (root / manifest["python"]).resolve(strict=True)):
        raise ResourceError("invoke_with_bundled_python")
    starter = ModuleType("product_round_starter")
    starter.__file__ = str(root / manifest["starter"])
    exec(compile(Path(starter.__file__).read_bytes(), starter.__file__, "exec"), starter.__dict__)
    host = starter.host
    raw = host.read_regular(Path(request_path), 1024 * 1024)[0]
    request = decode(raw)
    if (type(request) is not dict or set(request) != {"prepared", "realm_state_root", "realm",
            "descriptor_sha256", "preparation_sha256"}):
        raise ResourceError("invalid_local_round_request")
    prepared = host.directory(request["prepared"])
    realm_parent = host.directory(request["realm_state_root"]) / ".meerkat-connect" / request["realm"]
    if (prepared.name != "prepared" or prepared.parent.parent != realm_parent
            or not prepared.parent.name.startswith("setup-")):
        raise ResourceError("local_round_realm_changed")
    product = checked_receipt(host, prepared / "product-prepared.json", request["preparation_sha256"])
    receipt = checked_receipt(host, prepared / "config/prepared.json",
        product["preparation"]["bootstrap"]["host"]["receipt_sha256"])
    archive = root / manifest["archive"]
    release = prepared / "distribution" / archive.name.removesuffix(".tar.gz")
    snapshot, retained = host.release_snapshot(str(archive), sha256(archive), release, manifest["target"])
    if (product["resources_sha256"] != manifest_sha or receipt.get("target") != manifest["target"]
            or receipt.get("workspace") != str(prepared / "session")
            or receipt.get("release", {}).get("archive_sha256") != sha256(archive)
            or product["mcp_sha256"] != receipt["files"]["mcp.json"]):
        raise ResourceError("local_round_preparation_changed")
    helper, binding, descriptor = local_projection(host, prepared, release, retained, product["mcp_sha256"])
    descriptor_raw = host.read_regular(prepared / "product-mcp.json", MANIFEST_LIMIT)[0]
    if (host.digest(descriptor_raw) != request["descriptor_sha256"]
            or decode(descriptor_raw) != descriptor
            or product["local_setup"] != {"descriptor_sha256": request["descriptor_sha256"],
                                          "helper_sha256": binding["helper_sha256"]}):
        raise ResourceError("local_round_descriptor_changed")
    backend = host.owner_module("product_round_backend", release / "setup/backend.py", retained["setup/backend.py"])
    if stopped.is_set():
        raise ResourceError("local_round_cancelled")
    helper.offer_next_round(backend, prepared / "local-session", binding,
                           sys.stdin.buffer, sys.stdout.buffer, cancelled_fd)
    if (stopped.is_set() or host.read_regular(Path(request_path), 1024 * 1024)[0] != raw
            or host.read_regular(prepared / "product-mcp.json", MANIFEST_LIMIT)[0] != descriptor_raw):
        raise ResourceError("local_round_changed")
    checked_receipt(host, prepared / "product-prepared.json", request["preparation_sha256"])
    host.recheck_release(release, snapshot)
    verify_resources(root)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--destination", required=True)
    parser.add_argument("--installation-readback", help="private exact owner request; readback only")
    parser.add_argument("--local-round-owner", help="private live preparation-owner request")
    parser.add_argument("--realm-state-root")
    parser.add_argument("--realm")
    args = parser.parse_args(argv)
    stopped = threading.Event()
    wake_read, wake_write = os.pipe()
    os.set_blocking(wake_write, False)
    def cancel(*_):
        stopped.set()
        try:
            os.write(wake_write, b"x")
        except BlockingIOError:
            pass
    handlers = {number: signal.signal(number, cancel)
                for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        try:
            root = Path(__file__).resolve().parent
            if args.local_round_owner:
                if args.installation_readback or args.realm_state_root or args.realm:
                    raise ResourceError("invalid_local_round_request")
                local_round_owner(root, args.local_round_owner, stopped, wake_read)
                return 0
            value = (installation_readback(root, args.destination, args.installation_readback, stopped)
                     if args.installation_readback else prepare(root, args.destination, stopped,
                         realm_state_root=args.realm_state_root, realm=args.realm))
        except Exception:
            # Do not echo data, paths or owner exceptions into an agent turn.
            parser.exit(1, "Setup preparation unconfirmed. Preserve the setup directory for recovery.\n")
    finally:
        for number, handler in handlers.items():
            signal.signal(number, handler)
        os.close(wake_read); os.close(wake_write)
    print(json.dumps(value, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
