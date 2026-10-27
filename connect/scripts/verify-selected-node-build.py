#!/usr/bin/env python3
"""Explicit Linux qualification of selected Node compilation and fresh addon use.

This is an opt-in fixture, never a default test or installer. It uses the actual
Controller, canonical inspector/validator and selected installer. No registry
source, npm lifecycle script, MCP connection or native grant is requested.
"""
from __future__ import annotations

import argparse
import base64
import copy
import datetime
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import signal
import stat
import sys
import tarfile
import threading
import time


PACKAGE = "@toolkit-fixtures/source-addon"
VERSION = "1.0.0"
COMPONENT = "source-addon"
OPERANDS = {"left": 19, "right": 23}
OUTPUT = "node_modules/" + PACKAGE + "/build/Release/addon.node"
HEADER_NAMES = ("node_api.h", "node_api_types.h", "js_native_api.h", "js_native_api_types.h")
HEX = re.compile(r"[0-9a-f]{64}\Z")
INTERRUPTED = False


def request_interrupt(signum, frame):
    # Flag-only while an existing Controller operation owns a child. Check the
    # flag at operation boundaries, after its direct-child owner has returned.
    global INTERRUPTED
    INTERRUPTED = True


def check_interrupt():
    if INTERRUPTED:
        raise KeyboardInterrupt


def wire(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def require(value, reason):
    if not value:
        raise AssertionError(reason)


def pinned(path, expected, limit=256 * 1024 * 1024, executable=False):
    path = Path(path)
    require(path.is_absolute() and path.resolve(strict=True) == path, "noncanonical input path")
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_size <= limit, "input type or size")
    require(HEX.fullmatch(expected) is not None, "invalid input digest")
    raw = path.read_bytes()
    require(sha(raw) == expected and len(raw) == info.st_size, "input changed")
    require(not executable or os.access(path, os.X_OK), "input is not executable")
    return raw


def save(root, name, value):
    path = root / name
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with path.open("xb") as output:
        output.write(wire(value))


def snapshot(root):
    result = {}
    for path in sorted(root.rglob("*")):
        info = path.lstat()
        relative = str(path.relative_to(root))
        if stat.S_ISDIR(info.st_mode):
            result[relative] = {"kind": "directory"}
        else:
            require(stat.S_ISREG(info.st_mode), "unexpected installed nonregular object")
            result[relative] = {"sha256": sha(path.read_bytes()), "bytes": info.st_size}
    return result


def archive(files):
    # A source-only local tarball. Fixed tar/gzip metadata makes its bytes stable.
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as bundle:
        for name, data in sorted(files.items()):
            info = tarfile.TarInfo("package/" + name)
            info.size, info.mode, info.mtime = len(data), 0o644, 0
            bundle.addfile(info, io.BytesIO(data))
    return gzip.compress(raw.getvalue(), mtime=0)


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("backend", "installer", "validator", "package-inspector", "node", "npm-cli", "tool-inputs", "fixture-manifest"):
        parser.add_argument("--" + name, required=True, type=Path)
        parser.add_argument("--" + name + "-sha256", required=True)
    parser.add_argument("--python-sha256", required=True)
    parser.add_argument("--fixture-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args()


def main(args, result):
    root = args.output
    require(sys.platform == "linux", "Linux qualification only")
    python = Path(sys.executable).resolve(strict=True)
    pinned(python, args.python_sha256, executable=True)
    pins = {}
    for name in ("backend", "installer", "validator", "package_inspector", "node", "npm_cli", "tool_inputs", "fixture_manifest"):
        path, digest = getattr(args, name), getattr(args, name + "_sha256")
        raw = pinned(path, digest, executable=name in ("validator", "package_inspector", "node", "npm_cli"))
        pins[name] = {"path": str(path), "sha256": digest, "bytes": len(raw)}
    result["input_pins"] = pins
    inventory = json.loads(args.tool_inputs.read_bytes())
    target = inventory["target"]
    require(target["node_sha256"] == args.node_sha256 and inventory["node"]["path"] == str(args.node), "Node inventory mismatch")
    compiler = [row for row in inventory["executables"] if row["tool_id"] == "gcc-driver"]
    require(len(compiler) == 1, "one exact compiler driver required")
    compiler = compiler[0]
    pinned(compiler["path"], compiler["sha256"], executable=True)
    headers = {}
    for name in HEADER_NAMES:
        rows = [row for row in inventory["files"] if Path(row["path"]).name == name and row["role"] == "static_napi_header_closure"]
        require(len(rows) == 1, "one exact header required")
        row = rows[0]
        pinned(row["path"], row["sha256"], limit=1024 * 1024)
        headers[name] = {"path": row["path"], "sha256": row["sha256"]}
    fixture_root = args.fixture_root
    require(fixture_root.resolve(strict=True) == fixture_root, "fixture directory must be canonical")
    expected = json.loads(args.fixture_manifest.read_bytes())
    require(set(expected) == {"schema_version", "files"} and expected["schema_version"] == 1, "fixture manifest shape")
    require(set(expected["files"]) == {"package.json", "index.cjs", "should-not-run.cjs", "src/add.c"}, "exact source fixture inventory required")
    files = {}
    for name, row in expected["files"].items():
        data = pinned(fixture_root / name, row["sha256"], limit=65536)
        require(len(data) == row["bytes"], "fixture size mismatch")
        files[name] = data
    require(json.loads(files["package.json"])["name"] == PACKAGE, "fixture package identity")
    require(not any(name.endswith(".node") for name in files), "fixture must be source-only")
    result["fixture_files"] = expected["files"]
    result["target"] = target
    archive_raw = archive(files)
    integrity = "sha512-" + base64.b64encode(hashlib.sha512(archive_raw).digest()).decode()
    staging = root / "staging"
    staged = staging / "choice"
    closure = staged / "closure"
    closure.mkdir(mode=0o700, parents=True)
    project = {"name": "selected-source-build-proof", "version": VERSION, "private": True,
               "dependencies": {PACKAGE: "file:./artifact.tgz"}}
    lock = {"name": project["name"], "version": VERSION, "lockfileVersion": 3, "requires": True,
            "packages": {"": {"name": project["name"], "version": VERSION, "dependencies": project["dependencies"]},
                         "node_modules/" + PACKAGE: {"version": VERSION, "resolved": "file:artifact.tgz", "integrity": integrity, "hasInstallScript": True}}}
    (staged / "artifact.tgz").write_bytes(archive_raw)
    (closure / "package-json").write_bytes(wire(project))
    (closure / "package-lock").write_bytes(wire(lock))
    profile = {"schema_version": 1, "scope": "selected_node_build", "target": target,
               "executables": {"compile": {key: compiler[key] for key in ("path", "sha256")}},
               "files": {name.replace(".", "-"): row for name, row in headers.items()},
               "environment": {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"},
               "limits": {"timeout_seconds": 120, "file_bytes": 1048576, "total_output_bytes": 2097152}}
    profile_path = root / "node-build-profile.json"
    profile_path.write_bytes(wire(profile))
    plan = {"schema_version": 1, "target": target,
            "inputs": [{"source": {"npm": "node_modules/" + PACKAGE + "/src/add.c"}, "path": "src/add.c"},
                       *[{"source": {"toolchain": name.replace(".", "-")}, "path": "headers/" + name} for name in HEADER_NAMES]],
            "steps": [{"executable": "compile", "arguments": ["-shared", "-fPIC", "-Iheaders", "-DNODE_GYP_MODULE_NAME=source_addon", "src/add.c", "-o", "addon.node"]}],
            "outputs": [{"path": "addon.node", "destination": OUTPUT, "replaces_sha256": None}]}
    save(root, "BUILD-PLAN.json", plan)
    require(sha(profile_path.read_bytes()) == sha(wire(profile)), "profile bytes changed")
    spec = importlib.util.spec_from_file_location("qualification_actual_backend", args.backend)
    backend = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(backend)
    installer_spec = importlib.util.spec_from_file_location("qualification_actual_installer", args.installer)
    installer = importlib.util.module_from_spec(installer_spec)
    installer_spec.loader.exec_module(installer)
    calls = []
    call_lock = threading.Lock()

    class RecordingRunner(backend.Runner):
        """Observe the real Runner return without changing dispatch or custody."""
        def run(self, argv, **kwargs):
            check_interrupt()
            started = time.monotonic()
            with call_lock:
                index = len(calls)
                row = {"index": index, "argv": list(argv), "capture": kwargs.get("capture", False)}
                calls.append(row)
            try:
                code, raw = super().run(argv, **kwargs)
                row.update(exit_code=code, stdout_sha256=sha(raw), stdout_bytes=len(raw))
                (root / ("call-%02d.stdout" % index)).write_bytes(raw)
                check_interrupt()
                return code, raw
            except BaseException as error:
                row["error_type"] = type(error).__name__
                raise
            finally:
                row["seconds"] = round(time.monotonic() - started, 6)

    selected_profile = {"schema_version": 1, "scope": backend.SELECTED_SCOPE, "staging_root": str(staging),
                        "installer": {"path": str(args.installer), "sha256": args.installer_sha256},
                        "validators": {"node": {"path": str(args.validator), "sha256": args.validator_sha256}},
                        "package_inspector": {"path": str(args.package_inspector), "sha256": args.package_inspector_sha256},
                        "node_build": {"path": str(profile_path), "sha256": sha(profile_path.read_bytes())}}
    host_path = root / "selected-install.json"
    host_path.write_bytes(wire(selected_profile))
    controller = None
    try:
        controller = backend.Controller(None, None, root / "workspace", (), str(python), str(args.node), str(args.npm_cli),
            runner_factory=RecordingRunner, selected_install_profile=host_path,
            selected_install_profile_sha256=sha(host_path.read_bytes()))
        package = {"schema_version": 1, "identity": {"publisher": "toolkit-fixtures", "name": "source-addon", "version": VERSION},
            "display_name": "Controlled selected source addon", "description": "Local compilation and direct arithmetic only.",
            "provenance": {"source": "https://example.invalid/toolkit-fixtures/source-addon", "source_revision": "fixture-source-v1", "license": "MIT", "notices": ["Source fixture; no registry acquisition or MCP readiness is claimed."], "local_patches": []},
            "strategies": [{"id": "node-local", "kind": "local_mcp", "instructions": "Install this retained source archive with scripts disabled and the separately reviewed compile plan.", "sources": [], "known_requirements": [], "unresolved_details": []}],
            "known_exports": [{"id": "arithmetic", "role": "service_connector", "description": "Software declaration only; the qualification consumes the entrypoint directly."}], "assessments": []}
        package_raw = wire(package)
        inspection = controller.inspect_package({"package_json": package_raw.decode()})
        save(root, "PACKAGE-INSPECTION.json", inspection)
        require(inspection["status"] == "accepted" and inspection["source_digest"] == sha(package_raw), "actual package inspection required")
        schema = {"type": "object", "properties": {"left": {"type": "integer"}, "right": {"type": "integer"}}, "required": ["left", "right"], "additionalProperties": False}
        declaration = {"schema_version": 1, "document_kind": "local_software_declaration", "package": package["identity"],
            "package_content_digest": inspection["summary"]["package_content_digest"], "strategy": "node-local",
            "components": [{"id": COMPONENT, "version": VERSION, "runtime": "node", "source": {"kind": "artifact", "location": (staged / "artifact.tgz").as_uri(), "content_digest": sha(archive_raw),
                "dependencies": {"kind": "pinned_closure", "artifacts": {"package-json": sha(wire(project)), "package-lock": sha(wire(lock)), "node-build-plan": sha(wire(plan))}},
                "entrypoint": {"executable": "index.cjs", "arguments": [wire(OPERANDS).decode().strip()]}}}],
            "exports": [{"role": "service_connector", "id": "arithmetic", "components": [COMPONENT], "authentication": {"kind": "none"}, "operations": [{"id": "add", "schema_digest": sha(wire(schema)), "declared_effect": "read", "required_authority": ["read"], "resources": {"kind": "connection_wide"}, "result_limits": {"maximum_bytes": 1024, "maximum_items": 1}}]}],
            "portable_mcp_entries": [{"name": "source-addon-qualification", "component": COMPONENT}]}
        save(root, "PACKAGE.json", package)
        save(root, "DECLARATION.json", declaration)
        request = {"profile": "node", "staged_id": "choice", "component": COMPONENT,
                   "package_json": package_raw.decode(), "declaration_json": wire(declaration).decode(), "build_plan_json": wire(plan).decode()}
        checked = controller.selected_check(request)
        save(root, "CHECKED.json", checked)
        review = checked["build"]
        require(review["plan_sha256"] == sha(wire(plan)) and review["profile_sha256"] == sha(profile_path.read_bytes()), "review must bind authored plan and host profile")
        require(review["target"] == target and review["outputs"] == plan["outputs"], "review target or output mismatch")
        require(review["commands"] == [{"executable": "compile", "argv": [compiler["path"], *plan["steps"][0]["arguments"]], "executable_sha256": compiler["sha256"]}], "literal reviewed compiler command mismatch")
        require(not Path(checked["destination"]).exists(), "review cannot install")
        count = len(calls)
        require(controller.selected_check(request) == checked and len(calls) == count, "same check must reuse exact validation")
        selection = {key: checked[key] for key in ("validation_id", "effect_sha256")}
        save(root, "EXPLICIT-INSTALL-SELECTION.json", selection)
        check_interrupt()
        job = controller.selected_install(selection)["job"]
        controller.join()
        check_interrupt()
        job = copy.deepcopy(controller.jobs[job["id"]])
        save(root, "INSTALL-JOB.json", job)
        require(job["status"] == "succeeded", "actual install job did not succeed")
        installed = job["result"]
        require(installed["build_receipt_binding"] == "original_installer_reply", "live installer origin required")
        receipt_path = Path(installed["receipt_path"])
        receipt_raw = receipt_path.read_bytes()
        receipt = json.loads(receipt_raw)
        save(root, "INSTALLED-RECEIPT.json", receipt)
        origin = installed["installer_reply"]
        require(origin["receipt_sha256"] == installed["receipt_sha256"] == sha(receipt_raw), "live ready-file pin mismatch")
        require(origin["semantic_sha256"] == sha(backend.encoded(receipt)), "live semantic reply pin mismatch")
        install_calls = [row for row in calls if "--selected-node" in row["argv"] and "--verify-existing" not in row["argv"]]
        require(len(install_calls) == 1 and install_calls[0]["exit_code"] == 0, "one actual selected installer invocation required")
        require(origin["raw_sha256"] == install_calls[0]["stdout_sha256"] and origin["bytes"] == install_calls[0]["stdout_bytes"], "origin must be actual delegated stdout")
        build = receipt["node_build"]
        require(build["review"] == review and build["steps"] == [{"index": 0, "exit_code": 0}], "actual successful compiler step required")
        destination = Path(checked["destination"])
        output = destination / OUTPUT
        require(build["outputs"] == {OUTPUT: {"sha256": sha(output.read_bytes()), "bytes": output.stat().st_size}}, "actual generated addon inventory mismatch")
        require(output.read_bytes()[:4] == b"\x7fELF" and output.stat().st_size > 0, "real ELF addon required")
        require(not list(root.rglob("lifecycle-script-ran")), "npm lifecycle marker must remain absent")
        before = snapshot(destination)
        count = len(calls)
        require(controller.selected_install(selection)["job"]["id"] == job["id"] and len(calls) == count, "same install effect must not rerun")
        observed = controller.reconcile({"job_id": job["id"]})["observation"]
        save(root, "READBACK.json", observed)
        require(observed["status"] == "matched" and observed["result"]["installer_reply"] == origin, "inert readback must retain actual origin")
        added = calls[count:]
        require(len(added) == 1 and "--verify-existing" in added[0]["argv"], "readback may invoke only the actual inert verifier")
        require(snapshot(destination) == before, "readback modified installed files")
        argv = receipt["command"]
        require(argv == [str(args.node), str(destination / "node_modules" / PACKAGE / "index.cjs"), wire(OPERANDS).decode().strip()], "actual installed launch binding differs")
        consumer_root = root / "consumer"
        consumer_root.mkdir(mode=0o700)
        for name in ("home", "tmp"):
            (consumer_root / name).mkdir(mode=0o700)
        env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": str(consumer_root / "home"), "TMPDIR": str(consumer_root / "tmp")}
        result["consumer"] = {"argv": argv, "timeout_seconds": 15,
                              "owner": "pinned selected installer.run direct-child owner"}
        check_interrupt()
        raw = installer.run(argv, consumer_root, env, "fresh-consumer", timeout=15)
        check_interrupt()
        err = (consumer_root / "fresh-consumer.stderr").read_bytes()
        result["consumer"].update(exit_code=0, direct_child_joined=True,
                                  stdout_sha256=sha(raw), stderr_sha256=sha(err))
        (root / "CONSUMER.stdout").write_bytes(raw)
        (root / "CONSUMER.stderr").write_bytes(err)
        require(len(raw) <= 1024 and json.loads(raw) == {"sum": 42} and err == b"", "fresh native addon consumption failed")
        require(snapshot(destination) == before and not list(root.rglob("lifecycle-script-ran")), "consumer changed installation or lifecycle ran")
        result.update(outcome="passed", install_job_id=job["id"], validation_id=checked["validation_id"],
                      generated_outputs=build["outputs"], installer_reply=origin, same_effect_reused=True,
                      inert_readback_unchanged=True, lifecycle_marker_absent=True, consumer_sum=42)
    finally:
        # Keep primary failure and perform every owned cleanup even if one fails.
        primary_failure = sys.exc_info()[0] is not None
        cleanup_errors = []
        if controller is not None:
            try:
                controller.close_admission()
                controller.join()
                result["cleanup"]["controller_threads_alive"] = [thread.name for thread in controller.threads if thread.is_alive()]
                result["cleanup"]["controller_closed"] = controller.closing
            except BaseException as error:
                cleanup_errors.append({"owner": "controller", "type": type(error).__name__, "message": str(error)})
        result["calls"] = calls
        result["cleanup"]["direct_children"] = Path("/proc", str(os.getpid()), "task", str(os.getpid()), "children").read_text().split()
        result["cleanup"]["retained_workspace"] = str(root / "workspace")
        result["cleanup"]["errors"] = cleanup_errors
        if not primary_failure:
            require(not cleanup_errors and not result["cleanup"].get("controller_threads_alive", []) and not result["cleanup"]["direct_children"], "owned cleanup incomplete")
    for name, row in pins.items():
        pinned(row["path"], row["sha256"])
    pinned(python, args.python_sha256, executable=True)
    pinned(compiler["path"], compiler["sha256"], executable=True)
    for row in headers.values():
        pinned(row["path"], row["sha256"], limit=1024 * 1024)


if __name__ == "__main__":
    args = arguments()
    require(args.output.is_absolute() and not args.output.exists(), "fresh absolute output required")
    require(args.output.parent.resolve(strict=True) == args.output.parent, "canonical output parent required")
    args.output.mkdir(mode=0o700)
    result = {"schema_version": 1, "scope": "actual_controller_selected_node_build_and_fresh_direct_consumer", "outcome": "failed", "cleanup": {},
              "limits": ["Local synthetic source archive; no network acquisition, MCP readiness, native grant or business data.",
                         "Selected compiler and four Node headers are pinned; external system headers, compiler subprocesses, linker and dynamic dependency closure are not a hermetic toolchain.",
                         "Installer step exit is an actual direct-child wait; no universal descendant census.",
                         "Workspace and partial roots are retained for evidence; readback does not establish cold historical build provenance."]}
    for signum in (signal.SIGINT, signal.SIGTERM):
        signal.signal(signum, request_interrupt)
    try:
        check_interrupt()
        main(args, result)
        check_interrupt()
    except BaseException as error:
        result["outcome"] = "failed"
        result["failure"] = {"type": type(error).__name__, "message": str(error)}
        raise
    finally:
        result["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        save(args.output, "RESULT.json", result)
        print(json.dumps({"outcome": result["outcome"], "failure": result.get("failure"), "cleanup": result["cleanup"]}), flush=True)
