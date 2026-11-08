#!/usr/bin/env python3
"""Build only the synthetic, bounded CLI test resource closure.

This exercises normal binary layout and compile-time pin validation. The service
preparer and MCP service are test doubles; the local-round owner and helper
rendezvous use the shipped local_session.py unchanged. Python is supplied by the
test operator. This is not a redistributable Connect product or runtime proof.
"""

import hashlib
import json
from pathlib import Path
import platform
import runpy
import shlex
import sys


def build(prefix, python):
    prefix = Path(prefix).absolute()
    python = Path(python).resolve(strict=True)
    if prefix.exists() or not python.is_file():
        raise ValueError("fresh prefix and existing test Python required")
    target = {("Darwin", "arm64"): "macos-arm64", ("Linux", "x86_64"): "linux-x86_64"}[
        platform.system(), platform.machine()
    ]
    root = prefix / "libexec/meerkat-connect"
    root.mkdir(parents=True, mode=0o755)
    (prefix / "bin").mkdir(mode=0o755)
    server = '''import argparse, json, pathlib, runpy, sys
f = runpy.run_path(str(pathlib.Path(__file__).resolve().parents[1] / "fixture.py"))
p = argparse.ArgumentParser(allow_abbrev=False)
p.add_argument("--audit-log", required=True)
p.add_argument("--prepared", required=True)
a = p.parse_args()
d = pathlib.Path(a.prepared)
if pathlib.Path(a.audit_log) != d / "fixture-mcp.jsonl":
    p.error("fixture_audit_mismatch")
binding = f["selected"](d)
local = f["local"]
# Keep the real helper lease for this actual MCP process's entire lifetime.
with local.session_owner(d / "local-session", binding) as (state, record, reopened):
    factory = local.controller_factory(f["backend"], state, record, reopened)
    controller = factory(None, None, None)
    try:
        serve(a)
    finally:
        controller.close_admission()
        controller.join()
'''
    # Define the synthetic service before entering the real helper lifecycle.
    service = '''def serve(a):
 for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"],
                  "capabilities": {"tools": {}},
                  "serverInfo": {"name": "synthetic-preparation-only", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [
            {"name": name, "description": "Controlled fixture only",
             "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False},
             "annotations": {"readOnlyHint": read_only, "destructiveHint": not read_only}}
            for name, read_only in [("setup_help", True), ("setup_install", False)]]}
    elif method == "tools/call":
        name = request["params"]["name"]
        with open(a.audit_log, "a") as output:
            output.write(json.dumps({"tool": name}) + "\\n")
        if name == "setup_help":
            result = {"content": [{"type": "text", "text": json.dumps({"fixture": "service-preparation-only"})}], "isError": False}
        else:
            result = {"content": [{"type": "text", "text": "Forbidden fixture tool reached dispatch"}], "isError": True}
    elif method == "ping":
        result = {}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"],
                          "error": {"code": -32601, "message": "Unsupported fixture method"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
'''
    server = server.replace("# Keep the real helper lease", service + "\n# Keep the real helper lease")
    fixture = '''import os, pathlib, runpy, stat
from types import SimpleNamespace
root = pathlib.Path(__file__).resolve().parent
local = SimpleNamespace(**runpy.run_path(str(root / "local_session.py")))
verify = runpy.run_path(str(root / "resource_checks.py"))["verify_resources"]

class EmptyController:
    """Synthetic service state only; production local_session owns the round."""
    def __init__(self, _unused_a, _unused_b, workspace, **_options):
        workspace.mkdir(mode=0o700)
        (workspace / "requests").mkdir(mode=0o700)
    def close_admission(self):
        pass
    def join(self):
        pass

def no_recovery(_raw):
    raise ValueError("synthetic_fixture_has_no_service_effects")

backend = SimpleNamespace(Controller=EmptyController, read_recovery_selection=no_recovery)

def private_read(path):
    fd = os.open(local.absolute(path), os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077 or info.st_size > local.LIMIT:
            raise ValueError("fixture_private_file_required")
        with os.fdopen(fd, "rb", closefd=False) as stream:
            raw = stream.read(local.LIMIT + 1)
        if len(raw) > local.LIMIT:
            raise ValueError("fixture_file_too_large")
        return raw
    finally:
        os.close(fd)

def configuration(d):
    return {"mcpServers": {"toolkit-setup": {"command": "/usr/bin/env",
        "args": ["-i", PYTHON, "-I", "-B", str(root / "starter/start.py"),
                 "--audit-log", str(d / "fixture-mcp.jsonl"), "--prepared", str(d)]}}}

def binding(d):
    return {"resources_sha256": verify(root)[1],
            "descriptor_sha256": local.digest(local.encoded(configuration(d))),
            "helper_sha256": local.digest(local.read(root / "local_session.py"))}

def selected(d):
    expected = binding(d)
    if (private_read(d / "product-mcp.json") != local.encoded(configuration(d))
            or local.decode(private_read(d / "product-prepared.json")) != {
                "scope": "synthetic_resource_closure_only", "local_setup": expected}):
        raise ValueError("fixture_preparation_changed")
    return expected
'''.replace("PYTHON", repr(str(python)))
    prepare = '''import argparse, os, pathlib, runpy, signal, sys
f = runpy.run_path(str(pathlib.Path(__file__).resolve().parent / "fixture.py"))
local, root = f["local"], f["root"]
p = argparse.ArgumentParser(allow_abbrev=False)
p.add_argument("--destination", required=True)
p.add_argument("--realm-state-root")
p.add_argument("--realm")
p.add_argument("--local-round-owner")
a = p.parse_args()

def realm_parent(state, realm):
    state = pathlib.Path(state)
    if not state.is_absolute() or not state.is_dir():
        p.error("invalid_native_state_root")
    if (not realm or len(realm) > 256 or realm in (".", "..")
            or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.-" for c in realm)):
        p.error("invalid_native_realm")
    # Native keeps its selected locator alias; only artifact ownership is canonical.
    return state.resolve(strict=True) / ".meerkat-connect" / realm

def check_destination(d, parent):
    if (not d.is_absolute() or d.parent.resolve(strict=True) != d.parent
            or d.is_relative_to(root) or root.is_relative_to(d)
            or d.name != "prepared" or d.parent.parent != parent
            or not d.parent.name.startswith("setup-")):
        p.error("preparation_realm_mismatch")

if a.local_round_owner:
    if a.realm_state_root or a.realm:
        p.error("invalid_local_round_request")
    request_path = pathlib.Path(a.local_round_owner)
    raw = f["private_read"](request_path)
    request = local.decode(raw)
    if type(request) is not dict or set(request) != {"prepared", "realm_state_root", "realm", "descriptor_sha256", "preparation_sha256"}:
        p.error("invalid_local_round_request")
    d = pathlib.Path(request["prepared"])
    check_destination(d, realm_parent(request["realm_state_root"], request["realm"]))
    binding = f["selected"](d)
    def recheck():
        if (f["private_read"](request_path) != raw or f["selected"](d) != binding
                or local.digest(f["private_read"](d / "product-mcp.json")) != request["descriptor_sha256"]
                or local.digest(f["private_read"](d / "product-prepared.json")) != request["preparation_sha256"]):
            p.error("local_round_selection_changed")
    recheck()
    wake_read, wake_write = os.pipe()
    os.set_blocking(wake_write, False)
    def cancel(*_):
        try:
            os.write(wake_write, b"x")
        except BlockingIOError:
            pass
    handlers = {n: signal.signal(n, cancel) for n in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        local.offer_next_round(f["backend"], d / "local-session", binding,
                               sys.stdin.buffer, sys.stdout.buffer, wake_read)
        recheck()
    finally:
        for n, handler in handlers.items():
            signal.signal(n, handler)
        os.close(wake_read)
        os.close(wake_write)
else:
    if a.realm_state_root is None or a.realm is None:
        p.error("complete_native_realm_required")
    d = pathlib.Path(a.destination)
    check_destination(d, realm_parent(a.realm_state_root, a.realm))
    if d.exists() or d.is_symlink():
        p.error("fresh_disjoint_destination_required")
    d.mkdir(mode=0o700)
    descriptor = local.encoded(f["configuration"](d))
    local.write_new(d / "product-mcp.json", descriptor)
    local.write_new(d / "product-prepared.json", local.encoded({
        "scope": "synthetic_resource_closure_only", "local_setup": f["binding"](d)}))
    sys.stdout.buffer.write(descriptor)
'''
    files = {
        "prepare.py": (prepare, 0o644),
        "fixture.py": (fixture, 0o644),
        "runtime/python/bin/python3.11": ("#!/bin/sh\nexec " + shlex.quote(str(python)) + ' "$@"\n', 0o755),
        "runtime/node/bin/node": ("#!/bin/sh\nexit 91\n", 0o755),
        "runtime/node/lib/node_modules/npm/bin/npm-cli.js": ("// Inert fixture placeholder.\n", 0o644),
        "starter/start.py": (server, 0o644),
        "archives/library.tar.gz": ("Inert fixture placeholder, not an archive.\n", 0o644),
    }
    repository = Path(__file__).resolve().parents[1]
    helper = repository / "sdk/consumer-setup/product/prepare.py"
    local_session = repository / "sdk/consumer-setup/local_session.py"
    # Reuse verified production resource checks and the entire actual round
    # handshake; only the empty Controller and MCP tool responses are synthetic.
    files["resource_checks.py"] = (helper.read_text(), 0o644)
    files["local_session.py"] = (local_session.read_text(), 0o644)
    for name, (text, mode) in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        path.chmod(mode)
    for path in root.rglob("*"):
        if path.is_dir():
            path.chmod(0o755)
    helpers = runpy.run_path(str(helper))
    manifest = {
        "schema_version": 1, "kind": "meerkat_connect_resources", "target": target,
        "prepare": "prepare.py", "python": "runtime/python/bin/python3.11",
        "node": "runtime/node/bin/node", "npm_cli": "runtime/node/lib/node_modules/npm/bin/npm-cli.js",
        "starter": "starter/start.py", "archive": "archives/library.tar.gz",
        "sources": [], "entries": helpers["tree_entries"](root),
    }
    raw = (json.dumps(manifest, indent=2) + "\n").encode()
    (root / "resources.json").write_bytes(raw)
    (root / "resources.json").chmod(0o644)
    _, pin = helpers["verify_resources"](root)
    assert pin == hashlib.sha256(raw).hexdigest()
    record = {"scope": "synthetic_resource_closure_only", "prefix": str(prefix),
              "manifest_sha256": pin, "python": str(python),
              "packaging_helper_sha256": hashlib.sha256(helper.read_bytes()).hexdigest(),
              "local_session_sha256": hashlib.sha256(local_session.read_bytes()).hexdigest(),
              "entries": len(manifest["entries"])}
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("prefix", help="Fresh output directory for test-only resources")
    parser.add_argument("--python", default=sys.executable,
                        help="Existing Python executable used only by the test fixture")
    options = parser.parse_args()
    build(options.prefix, options.python)
