#!/usr/bin/env python3
"""Prepare Toolkit from the selected local distribution and a trusted Python."""
from __future__ import annotations

import argparse
import io
import json
import os
from pathlib import Path
import platform
import signal
import sys
import tarfile
import threading
import time
from types import ModuleType
import unicodedata


# This starter and its adjacent owners come from the same trusted delivery.
# Archive directories never supply the selection manifest or executable code
# until their complete bytes have been checked against that selection.
HERE = Path(__file__).resolve().parent
runner = ModuleType("starter_bootstrap")
runner.__file__ = str(HERE / "bootstrap_host.py")
exec(compile((HERE / "bootstrap_host.py").read_bytes(), runner.__file__, "exec"), runner.__dict__)
host = runner.host


def distribution(selector=HERE / "current-distribution.json"):
    pointer = json.loads(host.read_regular(selector, 65536)[0])
    if (type(pointer.get("schema_version")) is not int or pointer["schema_version"] != 1
            or not isinstance(pointer.get("manifest"), str)
            or Path(pointer["manifest"]).is_absolute()):
        raise host.PreparationError("invalid_distribution_selection")
    raw = host.read_regular(selector.parent / pointer["manifest"], 1024 * 1024)[0]
    if host.digest(raw) != host.checked_hash(pointer["manifest_sha256"]):
        raise host.PreparationError("distribution_selection_changed")
    manifest = json.loads(raw)
    if type(manifest.get("schema_version")) is not int or manifest["schema_version"] != 1:
        raise host.PreparationError("invalid_distribution_manifest")
    return manifest


def selected_asset(manifest, target):
    suffix = "-" + target + ".tar.gz"
    rows = [row for row in manifest["assets"] if row["upload_name"].endswith(suffix)]
    if len(rows) != 1:
        raise host.PreparationError("distribution_target_unavailable")
    row = rows[0]
    name = host.relative(row["upload_name"])
    if ("/" in name or not name.startswith("toolkit-library-")
            or type(row["bytes"]) is not int or not 0 < row["bytes"] <= host.ARCHIVE_LIMIT):
        raise host.PreparationError("invalid_distribution_asset")
    host.checked_hash(row["sha256"])
    return row


def current_target():
    target = {("Darwin", "arm64"): "macos-arm64", ("Linux", "x86_64"): "linux-x86_64"}.get(
        (platform.system(), platform.machine()))
    if not target:
        raise host.PreparationError("unsupported_host_platform")
    if (sys.implementation.name != "cpython" or sys.version_info[:2] != (3, 11)
            or sys.prefix != sys.base_prefix):
        raise host.PreparationError("invoke_with_native_cpython_3_11")
    return target


def runtime_pin(value, destination, *, executable=True):
    path = Path(value).resolve(strict=True)
    if host.compose_overlaps(path, destination):
        raise host.PreparationError("runtime_overlaps_destination")
    raw, _ = host.read_regular(path, host.HOST_FILE_LIMIT)
    return host.pinned_path(str(path), host.digest(raw), executable=executable)


def checked_members(archive, root):
    """Preflight a bounded, regular-file tree before creating any output."""
    members, seen, kinds, total = [], set(), {}, 0
    for member in archive:
        name = host.relative(member.name.rstrip("/") if member.isdir() else member.name)
        normalized = unicodedata.normalize("NFC", name).casefold()
        if (normalized in seen or name.split("/")[0] != root
                or member.mode not in (0o644, 0o755) or member.sparse is not None
                or not (member.isdir() or member.isfile())):
            raise host.PreparationError("invalid_archive_member")
        if name == root and (not member.isdir() or member.mode != 0o755):
            raise host.PreparationError("invalid_archive_member")
        seen.add(normalized)
        kinds[normalized] = member.isdir()
        if len(seen) > host.ENTRY_LIMIT:
            raise host.PreparationError("release_too_large")
        if member.isfile():
            total += member.size
            if member.size < 0 or member.size > host.FILE_LIMIT or total > host.TOTAL_LIMIT:
                raise host.PreparationError("release_too_large")
        members.append(member)
    if not members or root.casefold() not in kinds:
        raise host.PreparationError("invalid_archive_member")
    for name in kinds:
        if any(kinds.get(str(parent)) is not True for parent in Path(name).parents if str(parent) != "."):
            raise host.PreparationError("invalid_archive_parent")
    return members


def start(args, *, stopped=None):
    stopped = stopped if stopped is not None else threading.Event()
    deadline = time.monotonic() + args.timeout_seconds
    runner.checkpoint(deadline, stopped)
    target = current_target()
    asset = selected_asset(distribution(), target)
    destination = host.new_path(args.destination)
    downloads = host.directory(args.archive_dir)
    if any(host.compose_overlaps(destination, path) for path in (downloads, HERE)):
        raise host.PreparationError("destination_overlaps_inputs")
    # On case-sensitive hosts, a fresh spelling must not alias an occupied name
    # on another supported host. Never reuse or remove a partial destination.
    if any(host.compose_overlaps(destination, item) for item in destination.parent.iterdir()):
        raise host.PreparationError("destination_alias_exists")
    python = runtime_pin(sys.executable, destination)
    pins = {"python": python}
    extra = []
    if getattr(args, "enable_public_github_research", False):
        extra += ["--enable-public-github-research"]
    families = "agno,services"
    if bool(args.node) != bool(args.npm_cli):
        raise host.PreparationError("node_and_npm_cli_required_together")
    if args.node:
        for name in ("node", "npm_cli"):
            pins[name] = runtime_pin(getattr(args, name), destination, executable=name == "node")
            flag = name.replace("_", "-")
            extra += ["--" + flag, pins[name]["path"], "--" + flag + "-sha256", pins[name]["sha256"]]
        families += ",activepieces"
    if args.allow_public_pypi != bool(args.pip_zipapp):
        raise host.PreparationError("public_pypi_requires_explicit_choice_and_pip_zipapp")
    if args.allow_public_pypi:
        pins["pip"] = runtime_pin(args.pip_zipapp, destination, executable=False)
        extra += ["--allow-public-pypi", "--pip-zipapp", pins["pip"]["path"],
                  "--pip-sha256", pins["pip"]["sha256"], "--enable-acquired-inspection"]

    public_npm = getattr(args, "allow_public_npm", False)
    search_node = getattr(args, "enable_node_search", False)
    selected_inputs = tuple(getattr(args, name, None) for name in
                            ("selected_node", "selected_npm_cli", "npm_archive"))
    if (type(public_npm) is not bool or type(search_node) is not bool
            or any(value is not None for value in selected_inputs) != (public_npm or search_node)
            or (public_npm or search_node) and not all(selected_inputs)):
        raise host.PreparationError("complete_explicit_npm_resources_and_scope_required")
    if public_npm or search_node:
        for name, value in zip(("selected_node", "selected_npm_cli", "npm_archive"), selected_inputs):
            # These are independently selected inputs, never the library pair
            # or a Node/npm executable discovered on PATH.
            if Path(value).is_symlink():
                raise host.PreparationError("selected_npm_resource_must_be_regular")
            pins[name] = runtime_pin(value, destination, executable=name == "selected_node")
            flag = name.replace("_", "-")
            extra += ["--" + flag, pins[name]["path"], "--" + flag + "-sha256", pins[name]["sha256"]]
        if public_npm:
            extra += ["--allow-public-npm"]
            if "--enable-acquired-inspection" not in extra:
                extra += ["--enable-acquired-inspection"]
        if search_node:
            extra += ["--enable-node-search"]

    node_build = host.node_build_options(args)
    if node_build is not None:
        if not {"selected_node", "selected_npm_cli"} <= pins.keys():
            raise host.PreparationError("node_build_requires_selected_node_validator")
        if host.compose_overlaps(node_build[0], destination):
            raise host.PreparationError("node_build_profile_overlaps_host_paths")
        pins["node_build_profile"] = host.pinned_path(node_build[0], node_build[1])
        extra += ["--node-build-profile", str(node_build[0]),
                  "--node-build-profile-sha256", node_build[1]]

    node_prebuilt_profile = getattr(args, "node_prebuilt_profile", None)
    node_prebuilt_sha256 = getattr(args, "node_prebuilt_profile_sha256", None)
    acquire_node_prebuilt = node_prebuilt_profile is not None or node_prebuilt_sha256 is not None
    if acquire_node_prebuilt:
        if not node_prebuilt_profile or not node_prebuilt_sha256:
            raise host.PreparationError("node_prebuilt_profile_pair_required")
        host.checked_hash(node_prebuilt_sha256)
        if not public_npm:
            raise host.PreparationError("node_prebuilt_requires_node_acquisition")
        if Path(node_prebuilt_profile).is_symlink():
            raise host.PreparationError("node_prebuilt_profile_must_be_regular")
        pins["node_prebuilt_profile"] = runtime_pin(node_prebuilt_profile, destination, executable=False)
        if pins["node_prebuilt_profile"]["sha256"] != node_prebuilt_sha256:
            raise host.PreparationError("node_prebuilt_profile_changed")
        extra += ["--node-prebuilt-profile", pins["node_prebuilt_profile"]["path"],
                  "--node-prebuilt-profile-sha256", node_prebuilt_sha256]

    node_prebuilt_policy = getattr(args, "node_prebuilt_policy", None)
    node_prebuilt_policy_sha256 = getattr(args, "node_prebuilt_policy_sha256", None)
    if node_prebuilt_policy is not None or node_prebuilt_policy_sha256 is not None:
        if not node_prebuilt_policy or not node_prebuilt_policy_sha256:
            raise host.PreparationError("node_prebuilt_policy_pair_required")
        if acquire_node_prebuilt:
            raise host.PreparationError("ambiguous_node_prebuilt_selection")
        host.checked_hash(node_prebuilt_policy_sha256)
        if not public_npm:
            raise host.PreparationError("node_prebuilt_requires_node_acquisition")
        if Path(node_prebuilt_policy).is_symlink():
            raise host.PreparationError("node_prebuilt_policy_must_be_regular")
        pins["node_prebuilt_policy"] = runtime_pin(node_prebuilt_policy, destination, executable=False)
        if pins["node_prebuilt_policy"]["sha256"] != node_prebuilt_policy_sha256:
            raise host.PreparationError("node_prebuilt_policy_changed")
        extra += ["--node-prebuilt-policy", pins["node_prebuilt_policy"]["path"],
                  "--node-prebuilt-policy-sha256", node_prebuilt_policy_sha256]

    recipe_inputs = tuple(getattr(args, "recipes_" + key, None) for key in ("database", "realm", "owner", "scope"))
    recipes_enabled = any(value is not None for value in recipe_inputs)
    remote_recipes = getattr(args, "allow_remote_recipes", False)
    if recipes_enabled and (not all(recipe_inputs) or recipe_inputs[3] not in ("read", "retain")):
        raise host.PreparationError("complete_recipes_profile_required")
    if type(remote_recipes) is not bool or remote_recipes and recipe_inputs[3] != "retain":
        raise host.PreparationError("remote_recipes_require_retention")
    if recipes_enabled:
        database, realm, owner, scope = recipe_inputs
        if not Path(database).is_absolute() or Path(database).parent.resolve(strict=True) != Path(database).parent:
            raise host.PreparationError("invalid_recipes_database_path")
        host.recipe_database_layout(database, (destination, downloads, HERE))
        for key, value in zip(("database", "realm", "owner", "scope"), recipe_inputs):
            extra += ["--recipes-" + key, value]
        if remote_recipes:
            extra += ["--allow-remote-recipes"]

    archive_path = downloads / asset["upload_name"]
    raw, _ = host.read_regular(archive_path, host.ARCHIVE_LIMIT)
    if len(raw) != asset["bytes"] or host.digest(raw) != asset["sha256"]:
        raise host.PreparationError("selected_archive_changed")
    root = asset["upload_name"].removesuffix(".tar.gz")
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as archive:
        members = checked_members(archive, root)
        runner.checkpoint(deadline, stopped)
        host.recheck_pins(pins)
        destination.mkdir(mode=0o700)
        extracted = destination / "distribution"
        extracted.mkdir(mode=0o700)
        for member in sorted(members, key=lambda item: (len(Path(item.name).parts), not item.isdir())):
            runner.checkpoint(deadline, stopped)
            path = extracted / member.name
            if member.isdir():
                path.mkdir(mode=0o700)
            else:
                with archive.extractfile(member) as source:
                    payload = source.read(host.FILE_LIMIT + 1)
                if len(payload) != member.size:
                    raise host.PreparationError("invalid_archive_member")
                host.write_new(path, payload)
                path.chmod(member.mode)
    release = extracted / root
    # The canonical owner validates the complete tree and the retained archive
    # again. Only then may the selected archived bootstrap execute.
    snapshot, _ = host.release_snapshot(str(archive_path), asset["sha256"], release, target)
    host.recheck_pins(pins)
    if recipes_enabled:
        recipe_tool = release / host.RECIPES_TOOL
        if (host.RECIPES_TOOL not in snapshot["hashes"]
                or not snapshot["files"][host.RECIPES_TOOL] & 0o111):
            raise host.PreparationError("local_recipes_owner_unavailable")
        # Initialization and replay belong to the existing durable store owner.
        # The process is joined by the same preparation runner and cancellation.
        initialized = runner.run_owner([str(recipe_tool), "init", "--database", database,
            "--realm", realm, "--owner", owner], release, deadline, stopped)
        if initialized != {"schema_version": 1, "scope": "retained_recipe_metadata", "status": "initialized"}:
            raise host.PreparationError("local_recipes_initialization_unconfirmed")
        host.recheck_release(release, snapshot)
    (destination / "staging").mkdir(mode=0o700)
    command = [python["path"], "-I", "-B", str(release / "setup/bootstrap_host.py"),
        "--release-archive", str(archive_path), "--release-sha256", asset["sha256"],
        "--release-dir", str(release), "--target", target,
        "--python", python["path"], "--python-sha256", python["sha256"],
        "--client-directory", str(destination / "client"), "--output", str(destination / "config"),
        "--workspace", str(destination / "session"), "--staging-root", str(destination / "staging"),
        "--families", families, "--enable-package-inspection", "--timeout-seconds", str(args.timeout_seconds), *extra]
    result = runner.run_owner(command, release, deadline, stopped)
    if (result.get("status") != "prepared" or result.get("release_sha256") != asset["sha256"]
            or result.get("scope") != "offline_setup_client_and_host_configuration_only"
            or result.get("host", {}).get("directory") != str(destination / "config")):
        raise host.PreparationError("bootstrap_result_mismatch")
    return {"status": "prepared", "scope": result["scope"], "target": target,
            "configuration": str(destination / "config/mcp.json"), "families": families.split(","),
            "public_pypi_enabled": args.allow_public_pypi,
            "public_npm_enabled": public_npm, "node_search_enabled": search_node,
            "next": "Register this configuration in your existing MCP host, then ask the agent for the original task.",
            "bootstrap": result}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--archive-dir", required=True, help="directory containing the selected Mac or Linux archive")
    parser.add_argument("--destination", required=True, help="new absolute directory; its parent must exist")
    parser.add_argument("--node", help="trusted compatible Node, paired with --npm-cli; enables Activepieces")
    parser.add_argument("--npm-cli", help="trusted npm CLI script for the selected library Node")
    parser.add_argument("--allow-public-pypi", action="store_true", help="allow agent-selected Python package acquisition")
    parser.add_argument("--pip-zipapp", help="trusted pip zipapp; required with --allow-public-pypi")
    parser.add_argument("--allow-public-npm", action="store_true", help="allow exact agent-selected npm acquisition")
    parser.add_argument("--enable-node-search", action="store_true", help="allow literal public npm research")
    parser.add_argument("--enable-public-github-research", action="store_true",
                        help="allow anonymous public GitHub repository research")
    parser.add_argument("--selected-node", help="trusted acquisition Node, separate from the library runtime")
    parser.add_argument("--selected-npm-cli", help="trusted npm CLI paired with the acquisition Node")
    parser.add_argument("--npm-archive", help="trusted npm distribution required by the acquisition owner")
    parser.add_argument("--node-prebuilt-profile", help="explicit pinned public Node prebuilt acquisition profile")
    parser.add_argument("--node-prebuilt-profile-sha256")
    parser.add_argument("--node-prebuilt-policy", help="explicit relocatable target/origin policy; host binds its selected Python")
    parser.add_argument("--node-prebuilt-policy-sha256")
    parser.add_argument("--node-build-profile", help="optional host-selected build profile; external tools remain host-owned")
    parser.add_argument("--node-build-profile-sha256")
    parser.add_argument("--timeout-seconds", type=int, default=300)
    for key in ("database", "realm", "owner"):
        parser.add_argument("--recipes-" + key)
    parser.add_argument("--recipes-scope", choices=("read", "retain"))
    parser.add_argument("--allow-remote-recipes", action="store_true")
    args = parser.parse_args(argv)
    if not 1 <= args.timeout_seconds <= 900:
        parser.error("timeout must be 1-900 seconds")
    stopped = threading.Event()
    handlers = {number: signal.signal(number, lambda *_: stopped.set())
                for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        try:
            result = start(args, stopped=stopped)
        except (host.PreparationError, runner.BootstrapError) as error:
            parser.exit(1, "Setup unconfirmed: " + str(error) + ". Preserve any partial destination; use a fresh path.\n")
        except Exception:
            parser.exit(1, "Setup unconfirmed: input_or_output_unavailable. Preserve any partial destination; use a fresh path.\n")
    finally:
        for number, handler in handlers.items():
            signal.signal(number, handler)
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
