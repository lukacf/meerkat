#!/usr/bin/env python3
"""Install an offline setup client, then prepare the existing host configuration."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import tempfile
import threading
import time
import unicodedata
from types import ModuleType, SimpleNamespace


# The invoking host trusts this entry point and its adjacent preparation owner.
# The selected release's owner is loaded only after complete archive validation.
_path = Path(__file__).with_name("prepare_host.py")
host = ModuleType("bootstrap_preparation")
host.__file__ = str(_path)
exec(compile(_path.read_bytes(), str(_path), "exec"), host.__dict__)
OUTPUT_LIMIT = 2 * 1024 * 1024
ENV = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "PYTHONDONTWRITEBYTECODE": "1",
       "PYTHONNOUSERSITE": "1", "PIP_NO_INDEX": "1", "PIP_CONFIG_FILE": os.devnull,
       "PIP_DISABLE_PIP_VERSION_CHECK": "1", "PIP_NO_INPUT": "1"}


class BootstrapError(Exception):
    """A fixed diagnostic; partial destinations remain available to their owner."""


def overlaps(left, right):
    # Fresh paths cannot be compared with samefile(). Refuse ambiguous case or
    # Unicode spellings conservatively, including on case-sensitive volumes.
    normalized = [Path(unicodedata.normalize("NFC", str(path)).casefold())
                  for path in (left, right)]
    return host.overlaps(*normalized)


def checkpoint(deadline, stopped):
    if stopped.is_set():
        raise BootstrapError("bootstrap_cancelled")
    if time.monotonic() >= deadline:
        raise BootstrapError("bootstrap_timeout")


def settle(process):
    """Let the installer settle its own child sessions before forced cleanup."""
    if process.returncode is not None:
        return
    try:
        process.send_signal(signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=4)
        return
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        raise BootstrapError("bootstrap_cleanup_unconfirmed") from None


def run_owner(argv, cwd, deadline, stopped):
    """Bound output and lifetime without exposing the caller's environment."""
    checkpoint(deadline, stopped)
    process = subprocess.Popen(argv, cwd=cwd, env=ENV, shell=False,
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, start_new_session=True)
    output, errors = bytearray(), bytearray()
    try:
        with selectors.DefaultSelector() as selector:
            for stream, buffer in ((process.stdout, output), (process.stderr, errors)):
                os.set_blocking(stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ, buffer)
            while selector.get_map():
                checkpoint(deadline, stopped)
                for key, _ in selector.select(min(0.1, max(0, deadline - time.monotonic()))):
                    chunk = os.read(key.fd, min(65536, OUTPUT_LIMIT + 1 - len(key.data)))
                    if not chunk:
                        selector.unregister(key.fileobj)
                    else:
                        key.data.extend(chunk)
                        if len(key.data) > OUTPUT_LIMIT:
                            raise BootstrapError("owner_output_limit")
            while True:
                checkpoint(deadline, stopped)
                try:
                    code = process.wait(timeout=min(0.1, max(0, deadline - time.monotonic())))
                    break
                except subprocess.TimeoutExpired:
                    pass
        if code != 0:
            raise BootstrapError("owner_refused")
        try:
            result = json.loads(output)
        except (ValueError, UnicodeError):
            raise BootstrapError("invalid_owner_result") from None
        if not isinstance(result, dict):
            raise BootstrapError("invalid_owner_result")
        return result
    except BaseException:
        settle(process)
        raise
    finally:
        process.stdout.close()
        process.stderr.close()


def bootstrap(args, *, stopped=None):
    """Compose existing owners once; never resume or repair partial output."""
    args = SimpleNamespace(**vars(args))
    recovery = host.recovery_options(args)
    prior_manifest = host.prior_manifest_options(args)
    stopped = stopped if stopped is not None else threading.Event()
    if type(args.timeout_seconds) is not int or not 1 <= args.timeout_seconds <= 900:
        raise BootstrapError("invalid_timeout")
    if any(getattr(args, name, None) is not None for name in ("mcp_python", "mcp_python_sha256")):
        raise BootstrapError("mcp_runtime_is_generated")
    deadline = time.monotonic() + args.timeout_seconds
    checkpoint(deadline, stopped)
    release = host.directory(args.release_dir)
    client = host.new_path(args.client_directory)
    output, workspace = host.new_path(args.output), host.new_path(args.workspace)
    staging = host.staging_path(args.staging_root, recovering=bool(recovery))
    locations = [release, client, output, workspace, staging]
    if any(overlaps(a, b) for i, a in enumerate(locations) for b in locations[i + 1:]):
        raise host.PreparationError("overlapping_directories")
    compose_profiles, compose_execution, _ = host.compose_options(args)
    if "backend" in compose_profiles:
        graph = compose_execution["graph_workspace"]
        if any(overlaps(graph, path) for path in locations):
            raise host.PreparationError("compose_graph_overlaps_host_paths")
        locations.append(graph)
    for profile in compose_profiles.values():
        if any(overlaps(Path(profile["path"]), path) for path in locations):
            raise host.PreparationError("compose_profile_overlaps_host_paths")
    if getattr(args, "recipes_database", None) is not None:
        host.recipe_database_layout(args.recipes_database, locations)
        if any(overlaps(Path(args.recipes_database + suffix), location)
               for suffix in ("", "-wal", "-shm", "-journal") for location in locations):
            raise host.PreparationError("recipes_database_overlaps_host_paths")
    prior = getattr(args, "prior_request", None)
    if prior and any(overlaps(Path(prior), path) for path in locations):
        raise host.PreparationError("prior_request_overlaps_host_paths")
    mcpb_profile = getattr(args, "mcpb_acquisition_profile", None)
    mcpb_sha256 = getattr(args, "mcpb_acquisition_profile_sha256", None)
    acquire_mcpb = mcpb_profile is not None or mcpb_sha256 is not None
    if acquire_mcpb and (not mcpb_profile or not mcpb_sha256):
        raise host.PreparationError("mcpb_acquisition_profile_pair_required")
    if mcpb_profile and any(overlaps(Path(mcpb_profile), path) for path in locations):
        raise host.PreparationError("mcpb_acquisition_profile_overlaps_host_paths")
    node_build = host.node_build_options(args)
    if node_build is not None and any(overlaps(node_build[0], path) for path in locations):
        raise host.PreparationError("node_build_profile_overlaps_host_paths")
    node_prebuilt_profile = getattr(args, "node_prebuilt_profile", None)
    node_prebuilt_sha256 = getattr(args, "node_prebuilt_profile_sha256", None)
    acquire_node_prebuilt = node_prebuilt_profile is not None or node_prebuilt_sha256 is not None
    if acquire_node_prebuilt and (not node_prebuilt_profile or not node_prebuilt_sha256):
        raise host.PreparationError("node_prebuilt_profile_pair_required")
    if node_prebuilt_profile and any(overlaps(Path(node_prebuilt_profile), path) for path in locations):
        raise host.PreparationError("node_prebuilt_profile_overlaps_host_paths")
    node_prebuilt_policy = getattr(args, "node_prebuilt_policy", None)
    node_prebuilt_policy_sha256 = getattr(args, "node_prebuilt_policy_sha256", None)
    if node_prebuilt_policy is not None or node_prebuilt_policy_sha256 is not None:
        if not node_prebuilt_policy or not node_prebuilt_policy_sha256:
            raise host.PreparationError("node_prebuilt_policy_pair_required")
        if acquire_node_prebuilt:
            raise host.PreparationError("ambiguous_node_prebuilt_selection")
        if any(overlaps(Path(node_prebuilt_policy), path) for path in locations):
            raise host.PreparationError("node_prebuilt_policy_overlaps_host_paths")
    pin = host.pinned_path(args.python, args.python_sha256, executable=True)
    snapshot, retained = host.release_snapshot(args.release_archive, args.release_sha256, release, args.target)
    prior_source = None
    if prior_manifest:
        prior_backend = host.owner_module("bootstrap_prior_backend", release / "setup/backend.py",
                                          retained["setup/backend.py"])
        prior_source = host.read_prior_manifest(prior_backend, prior_manifest)
        protected = (prior_manifest[0], prior_source["root"])
        if any(overlaps(source, destination) for source in protected for destination in locations):
            raise host.PreparationError("prior_manifest_overlaps_host_paths")
        if getattr(args, "recipes_database", None) is not None and any(
                overlaps(Path(args.recipes_database + suffix), source)
                for suffix in ("", "-wal", "-shm", "-journal") for source in protected):
            raise host.PreparationError("recipes_database_overlaps_host_paths")
    if recovery:
        backend = host.owner_module("bootstrap_recovery_backend", release / "setup/backend.py",
                                    retained["setup/backend.py"])
        _, original_workspace, entries = backend.load_recovery_selection(*recovery)
        protected = [recovery[0], original_workspace]
        for entry in entries:
            if entry["kind"] == backend.ACQUISITION_REQUEST:
                record, _ = backend.load_request_recovery(original_workspace, entry["id"],
                    entry["sha256"], kind=entry["kind"])
                protected.append(Path(record["staging_root"]))
        if any(overlaps(source, destination) for source in protected
               for destination in (client, output, workspace)):
            raise host.PreparationError("recovery_overlaps_host_paths")
        if getattr(args, "recipes_database", None) is not None and any(
                overlaps(Path(args.recipes_database + suffix), source)
                for suffix in ("", "-wal", "-shm", "-journal") for source in protected):
            raise host.PreparationError("recipes_database_overlaps_host_paths")
    owner_name = "setup/prepare_host.py"
    if owner_name not in snapshot["hashes"]:
        raise BootstrapError("preparation_owner_not_in_release")
    raw, _ = host.read_regular(release / owner_name)
    if host.digest(raw) != snapshot["hashes"][owner_name]:
        raise host.PreparationError("extracted_release_changed")
    owner = host.owner_module("selected_bootstrap_preparation", release / owner_name, raw)
    def prepare(selection):
        try:
            return owner.prepare(selection)
        except owner.PreparationError as error:
            raise host.PreparationError(str(error)) from None
    args.release_dir = str(release)
    args.client_directory, args.output, args.workspace = str(client), str(output), str(workspace)
    args.staging_root, args.python = str(staging), pin["path"]
    args.mcp_python, args.mcp_python_sha256 = pin["path"], pin["sha256"]
    # Only the private preview is disposable. Real client/configuration output
    # is never removed, including when final preparation fails.
    with tempfile.TemporaryDirectory(prefix=".toolkit-bootstrap-", dir=output.parent) as temporary:
        preview = SimpleNamespace(**vars(args))
        preview.output = str(Path(temporary) / "validated")
        prepare(preview)
        prepared_raw, _ = host.read_regular(Path(preview.output) / "prepared.json")
        preview_receipt = json.loads(prepared_raw)
        if recovery and (preview_receipt.get("recovery", {}).get("mode") != "readback_only"
                or preview_receipt["recovery"].get("manifest_sha256") != recovery[1]
                or preview_receipt["files"].get("recover-manifest.json") != recovery[1]):
            raise BootstrapError("recovery_not_prepared")
        if prior_manifest and (preview_receipt.get("prior_selection") != {
                "source": "recovery_manifest", "manifest_sha256": prior_manifest[1],
                "request_id": prior_manifest[2], "request_sha256": prior_source["sha256"]}
                or preview_receipt["files"].get("prior-request.json") != prior_source["sha256"]):
            raise BootstrapError("prior_manifest_not_prepared")
        pins = preview_receipt["runtimes"]
        if node_build is not None and (
                preview_receipt["files"].get("node-build-profile.json") != node_build[1]
                or pins.get("node_build_profile", {}).get("sha256") != node_build[1]):
            raise BootstrapError("node_build_profile_not_prepared")
        if acquire_mcpb and (preview_receipt["files"].get("mcpb-acquisition.json") != mcpb_sha256
                             or pins.get("mcpb_acquisition_profile", {}).get("sha256") != mcpb_sha256):
            raise BootstrapError("mcpb_acquisition_not_prepared")
        if acquire_node_prebuilt and (preview_receipt["files"].get("node-prebuilt.json") != node_prebuilt_sha256
                or pins.get("node_prebuilt_profile", {}).get("sha256") != node_prebuilt_sha256):
            raise BootstrapError("node_prebuilt_not_prepared")
        if node_prebuilt_policy is not None and (
                "node-prebuilt.json" not in preview_receipt["files"]
                or pins.get("node_prebuilt_policy", {}).get("sha256") != node_prebuilt_policy_sha256):
            raise BootstrapError("node_prebuilt_policy_not_prepared")
        for name, profile in compose_profiles.items():
            inner = "compose-" + name.replace("_", "-") + ".json"
            outer = "compose-acquisition.json" if name == "acquisition_host" else "compose-execution.json"
            if (preview_receipt["files"].get(inner) != profile["sha256"]
                    or outer not in preview_receipt["files"]
                    or pins.get("compose_" + name + "_profile", {}).get("sha256") != profile["sha256"]):
                raise BootstrapError("compose_not_prepared")
        for name, item in pins.items():
            if name.startswith("compose_") and any(overlaps(Path(item["path"]), path) for path in locations):
                raise host.PreparationError("compose_input_overlaps_host_paths")
    checkpoint(deadline, stopped)

    def recheck():
        checkpoint(deadline, stopped)
        host.recheck_release(release, snapshot)
        host.recheck_pins(pins)
        if (host.directory(args.release_dir) != release or host.new_path(args.output) != output
                or host.new_path(args.workspace) != workspace
                or host.staging_path(args.staging_root, recovering=bool(recovery)) != staging
                or host.directory(client.parent) != client.parent):
            raise host.PreparationError("host_paths_changed")
        if prior_manifest and host.read_prior_manifest(prior_backend, prior_manifest) != prior_source:
            raise host.PreparationError("prior_manifest_changed")
        if recovery:
            # Reuse the complete archived preparation path, including all
            # per-kind immutable request readers, immediately before effects.
            # Only this validation output is disposable; original work and
            # real bootstrap destinations are never rewritten or removed.
            with tempfile.TemporaryDirectory(prefix=".toolkit-recovery-check-", dir=output.parent) as temporary:
                checked = SimpleNamespace(**vars(args))
                checked.output = str(Path(temporary) / "validated")
                prepare(checked)

    recheck()
    if host.new_path(client) != client:
        raise host.PreparationError("host_paths_changed")
    contents = snapshot["hashes"]["consumers/CONTENTS.json"]
    command = [pin["path"], "-I", "-B", str(release / "consumers/install-library-consumer.py"),
               "--bundle", str(release / "consumers"), "--contents-sha256", contents,
               "--family", "services", "--python", pin["path"], "--destination", str(client)]
    installed = run_owner(command, release, deadline, stopped)
    recheck()
    receipt_raw, _ = host.read_regular(client / "installed.json", 1024 * 1024)
    try:
        receipt = json.loads(receipt_raw)
    except (ValueError, UnicodeError):
        raise BootstrapError("invalid_installation_receipt") from None
    if (receipt != installed or type(receipt.get("schema_version")) is not int
            or receipt.get("schema_version") != 1
            or receipt.get("scope") != "observed_software_installation_only"
            or receipt.get("family") != "services" or receipt.get("destination") != str(client)
            or receipt.get("contents_sha256") != contents
            or any(key in receipt for key in ("configuration", "mcp_configuration", "launch_environment"))):
        raise BootstrapError("installation_receipt_mismatch")
    verified = run_owner(command + ["--verify-existing"], release, deadline, stopped)
    recheck()
    if (verified.get("verified") is not True
            or verified.get("scope") != "retained_payload_and_launch_binding_only"
            or verified.get("family") != "services" or verified.get("destination") != str(client)
            or verified.get("contents_sha256") != contents or verified.get("configuration_sha256") is not None
            or verified.get("command") != receipt.get("command")
            or verified.get("selected_runtime_path") != pin["resolved_path"]
            or verified.get("historical_receipt", {}).get("sha256") != host.digest(receipt_raw)
            or host.read_regular(client / "installed.json", 1024 * 1024)[0] != receipt_raw):
        raise BootstrapError("verification_binding_mismatch")
    interpreter = client / "runtime/bin/python"
    mcp_pin = host.pinned_path(interpreter, pin["sha256"], executable=True, preserve_link=True)
    if mcp_pin["resolved_path"] != pin["resolved_path"]:
        raise BootstrapError("client_interpreter_mismatch")
    args.mcp_python, args.mcp_python_sha256 = str(interpreter), pin["sha256"]
    recheck()
    prepared = prepare(args)
    checkpoint(deadline, stopped)
    host.recheck_release(release, snapshot)
    host.recheck_pins({**pins, "client_python": mcp_pin})
    if host.new_path(workspace) != workspace:
        raise host.PreparationError("host_paths_changed")
    checkpoint(deadline, stopped)
    return {"status": "prepared", "scope": "offline_setup_client_and_host_configuration_only",
            "client": {"directory": str(client), "receipt": str(client / "installed.json"),
                       "receipt_sha256": host.digest(receipt_raw),
                       "verification_scope": verified["scope"]},
            "host": prepared, "release_sha256": args.release_sha256}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    for name in ("release-archive", "release-sha256", "release-dir", "client-directory", "output",
                 "workspace", "staging-root", "python", "python-sha256"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--target", choices=host.TARGETS, required=True)
    parser.add_argument("--timeout-seconds", type=int, default=300)
    parser.add_argument("--families", default="agno,services,activepieces")
    for name in ("allow-public-pypi", "allow-public-npm", "enable-node-search", "enable-public-github-research", "allow-mcpb", "enable-package-inspection",
                 "enable-plugin-inspection", "enable-acquired-inspection", "enable-mcpb-inspection",
                 "allow-remote-recipes"):
        parser.add_argument("--" + name, action="store_true")
    parser.add_argument("--recipes-scope", choices=("read", "retain"))
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
    stopped = threading.Event()
    handlers = {number: signal.signal(number, lambda *_: stopped.set())
                for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        try:
            result = bootstrap(args, stopped=stopped)
        except (BootstrapError, host.PreparationError) as error:
            parser.exit(1, "Host bootstrap unconfirmed: " + str(error) + ". Preserve partial destinations.\n")
        except Exception:
            parser.exit(1, "Host bootstrap unconfirmed: input_or_output_unavailable. Preserve partial destinations.\n")
    finally:
        for number, handler in handlers.items():
            signal.signal(number, handler)
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
