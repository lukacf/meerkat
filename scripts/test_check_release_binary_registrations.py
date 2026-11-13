#!/usr/bin/env python3
"""Contract test for scripts/check-release-binary-registrations.

Stub binaries stand in for the four release binaries and answer on the same
surfaces (rkat's two commands, JSON-RPC over stdio, MCP over stdio, HTTP), over
a small source tree that registers three capabilities and four built-in
skills, two of them with no required capability. Like the real binaries, a
stub lists the project skill of its project only when it reports the
capability that skill requires. The check must pass a binary whose
registrations ran and refuse each defect: no registration at all (the
measured release defect: empty capability list, no built-in skill, the
project skill dropped), a capability missing, a capability the source does
not register, an ungated built-in skill missing, a built-in the source does
not register, a capability or an ungated skill the source gained that the
binary lacks, and an ELF file with two init-array tables (read, never run, so
it is refused for a target the host cannot run too). A release version in the
environment never skips the check. No network beyond loopback, no Rust build.
"""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHECK = ROOT / "scripts" / "check-release-binary-registrations"

CAPABILITIES = ["sessions", "skills", "work_graph"]
UNGATED = ["alpha-reference", "beta-reference"]
GATED_LISTED = ["gamma-patterns"]
PROBE = "registration-check-probe"

FACADE_LIB_RS = """\
#[used]
static LINK: fn() = other::link_embedded_registrations;

#[cfg(feature = "skills")]
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "alpha-reference",
        name: "Alpha",
        requires_capabilities: &[],
        body: "",
    }
}

#[cfg(feature = "skills")]
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: help::BETA_SKILL_NAME,
        name: "Beta",
        requires_capabilities: &[],
        body: help::BETA_SKILL_BODY,
    }
}
"""

HELP_RS = 'pub const BETA_SKILL_NAME: &str = "beta-reference";\n'

TOOLS_LIB_RS = """\
inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::Skills,
        description: "Skills",
    }
}

inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::WorkGraph,
        description: "Work graph",
    }
}

inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "gamma-patterns",
        name: "Gamma",
        requires_capabilities: &["skills"],
        body: "",
    }
}

inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "delta-workflow",
        name: "Delta",
        requires_capabilities: &["skills", "work_graph"],
        body: "",
    }
}
"""

CAPABILITIES_LIB_RS = """\
inventory::collect!(CapabilityRegistration);

inventory::submit! {
    CapabilityRegistration {
        id: CapabilityId::Sessions,
        description: "Sessions",
    }
}
"""

NEW_CAPABILITY_RS = """\
inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::MemoryStore,
        description: "Memory",
    }
}
"""

NEW_UNGATED_SKILL_RS = """\
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "epsilon-new",
        name: "Epsilon",
        requires_capabilities: &[],
        body: "",
    }
}
"""

STUB = r'''#!/usr/bin/env python3
import json
import os
import re
import sys
from pathlib import Path

CAPABILITIES = __CAPABILITIES__
BUILTIN = __BUILTIN__
NAME = {"rkat_rpc_bin": "rkat-rpc"}.get(Path(sys.argv[0]).name, Path(sys.argv[0]).name)


def context_root():
    return Path(sys.argv[sys.argv.index("--context-root") + 1])


def capabilities():
    return {
        "contract_version": "0.0.0",
        "capabilities": [{"id": c, "description": c, "status": "Available"} for c in CAPABILITIES],
    }


def entry(name, scope):
    return {
        "key": {"source_uuid": "00000000-0000-4000-8000-000000000001", "skill_name": name},
        "name": name,
        "description": name,
        "scope": scope,
        "source": {},
        "is_active": True,
    }


def skills():
    entries = [entry(name, "builtin") for name in BUILTIN]
    # A project skill is listed only when every capability it requires is
    # reported: the filter the real binaries apply.
    for document in sorted((context_root() / ".rkat" / "skills").glob("*/SKILL.md")):
        text = document.read_text(encoding="utf-8")
        needs = re.search(r"^requires_capabilities:\s*\[([^\]]*)\]", text, re.M)
        needs = [item.strip() for item in needs.group(1).split(",") if item.strip()] if needs else []
        if all(item in CAPABILITIES for item in needs):
            entries.append(entry(document.parent.name, "project"))
    return entries


def serve_stdio(answer):
    for line in sys.stdin:
        try:
            request = json.loads(line)
        except ValueError:
            continue
        if "id" not in request:
            continue
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": answer(request)}) + "\n")
        sys.stdout.flush()


if NAME == "rkat":
    if "capabilities" in sys.argv:
        print(json.dumps(capabilities()))
    elif sys.argv[-3:] == ["skill", "list", "--json"]:
        print(json.dumps(skills()))
    else:
        sys.exit(64)
elif NAME == "rkat-rpc":
    def answer(request):
        method = request.get("method")
        if method == "capabilities/get":
            return capabilities()
        if method == "skills/list":
            return {"skills": skills()}
        return {"methods": []}
    serve_stdio(answer)
elif NAME == "rkat-mcp":
    def answer(request):
        if request.get("method") != "tools/call":
            return {"protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": {"name": "stub", "version": "0"}}
        tool = request["params"]["name"]
        payload = capabilities() if tool == "meerkat_capabilities" else {"skills": skills()}
        return {"content": [{"type": "text", "text": json.dumps(payload)}], "isError": False}
    serve_stdio(answer)
elif NAME == "rkat-rest":
    from http.server import BaseHTTPRequestHandler, HTTPServer

    config = Path(os.environ["HOME"]) / ".rkat" / "config.toml"
    port = int(re.search(r"^port\s*=\s*(\d+)", config.read_text(encoding="utf-8"), re.M).group(1))

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            body = capabilities() if self.path == "/capabilities" else {"skills": skills()}
            data = json.dumps(body).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args):
            pass

    HTTPServer(("127.0.0.1", port), Handler).serve_forever()
else:
    sys.exit(64)
'''


def write_source(root: Path, extra_tools: str = "") -> Path:
    for crate, files in {
        "meerkat": {"lib.rs": FACADE_LIB_RS, "help.rs": HELP_RS},
        "meerkat-tools": {"lib.rs": TOOLS_LIB_RS + extra_tools},
        "meerkat-capabilities": {"lib.rs": CAPABILITIES_LIB_RS},
    }.items():
        src = root / "crates" / crate / "src"
        src.mkdir(parents=True, exist_ok=True)
        (root / "crates" / crate / "Cargo.toml").write_text('[package]\nname = "x"\n', encoding="utf-8")
        for name, text in files.items():
            (src / name).write_text(text, encoding="utf-8")
    return root


def write_stub(directory: Path, name: str, capabilities: list[str], builtin: list[str]) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / name
    path.write_text(
        STUB.replace("__CAPABILITIES__", json.dumps(capabilities)).replace("__BUILTIN__", json.dumps(builtin)),
        encoding="utf-8",
    )
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


def elf_with_init_array_tables(path: Path, tables: int, machine: int = 62) -> Path:
    """A minimal 64-bit little-endian ELF: header, one PT_DYNAMIC program
    header, and a dynamic section with `tables` DT_INIT_ARRAY entries."""
    dynamic = b""
    for index in range(tables):
        dynamic += (25).to_bytes(8, "little") + (0x1000 + index * 0x100).to_bytes(8, "little")
        dynamic += (27).to_bytes(8, "little") + (8).to_bytes(8, "little")
    dynamic += (0).to_bytes(16, "little")
    phoff, dynoff = 64, 64 + 56
    header = b"\x7fELF" + bytes([2, 1, 1, 0]) + bytes(8)
    header += (3).to_bytes(2, "little") + machine.to_bytes(2, "little") + (1).to_bytes(4, "little")
    header += (0).to_bytes(8, "little") + phoff.to_bytes(8, "little") + (0).to_bytes(8, "little")
    header += (0).to_bytes(4, "little") + (64).to_bytes(2, "little") + (56).to_bytes(2, "little")
    header += (1).to_bytes(2, "little") + (0).to_bytes(2, "little") * 3
    program = (2).to_bytes(4, "little") + (6).to_bytes(4, "little") + dynoff.to_bytes(8, "little")
    program += dynoff.to_bytes(8, "little") * 2 + len(dynamic).to_bytes(8, "little") * 2 + (8).to_bytes(8, "little")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(header + program + dynamic)
    return path


def run(binary: Path, source: Path, *flags: str, version: str | None = None) -> subprocess.CompletedProcess:
    env = {key: value for key, value in os.environ.items() if key != "VERSION"}
    if version is not None:
        env["VERSION"] = version
    return subprocess.run(
        [sys.executable, str(CHECK), *flags, "--source-root", str(source), str(binary)],
        env=env,
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )


FAILURES: list[str] = []


def expect(label: str, done: subprocess.CompletedProcess, code: int, *needles: str) -> None:
    output = done.stdout + done.stderr
    problems = []
    if done.returncode != code:
        problems.append(f"exit {done.returncode}, expected {code}")
    problems.extend(f"output lacks {needle!r}" for needle in needles if needle not in output)
    if problems:
        FAILURES.append(f"{label}: {'; '.join(problems)}\n    output: {output.strip()[:1200]}")
        print(f"FAIL {label}")
    else:
        print(f"ok   {label}")


def main() -> int:
    healthy = UNGATED + GATED_LISTED
    with tempfile.TemporaryDirectory(prefix="registrations-contract.") as tmp:
        work = Path(tmp)
        source = write_source(work / "source")

        for surface in ("rkat", "rkat-rpc", "rkat-mcp", "rkat-rest"):
            expect(
                f"{surface}: registrations ran",
                run(write_stub(work / "good", surface, CAPABILITIES, healthy), source),
                0,
                "RUN: 3 capabilities, equal to the source registrations",
                "3 built-in skills listed, among them the 2 that require no capability "
                "(alpha-reference, beta-reference)",
                "the project skill that requires `sessions` is listed",
            )
            expect(
                f"{surface}: no constructor ran",
                run(write_stub(work / "unregistered", surface, [], []), source),
                1,
                "(a) it reports 0 capabilities, the source registers 3 for it",
                "sessions (crates/meerkat-capabilities/src/lib.rs)",
                "(b) it lists 0 built-in skills; missing, though they require no capability: "
                "alpha-reference (crates/meerkat/src/lib.rs), beta-reference (crates/meerkat/src/lib.rs)",
                f"(c) it does not list the project skill `{PROBE}`",
            )

        good = work / "good" / "rkat"
        unregistered = work / "unregistered" / "rkat"
        expect(
            "a gated built-in skill the surface leaves off is not demanded",
            run(write_stub(work / "ungated-only", "rkat", CAPABILITIES, UNGATED), source),
            0,
            "2 built-in skills listed",
        )
        expect(
            "one registered capability missing",
            run(write_stub(work / "capability-missing", "rkat", CAPABILITIES[:2], healthy), source),
            1,
            "(a) it reports 2 capabilities, the source registers 3 for it; missing: "
            "work_graph (crates/meerkat-tools/src/lib.rs)",
        )
        expect(
            "the sessions capability missing drops the project skill",
            run(write_stub(work / "sessions-missing", "rkat", CAPABILITIES[1:], healthy), source),
            1,
            "missing: sessions",
            f"(c) it does not list the project skill `{PROBE}`",
        )
        expect(
            "a capability the source does not register",
            run(write_stub(work / "capability-extra", "rkat", CAPABILITIES + ["stray"], healthy), source),
            1,
            "not registered in the source for it: stray",
        )
        expect(
            "one ungated built-in skill missing",
            run(write_stub(work / "skill-missing", "rkat", CAPABILITIES, ["alpha-reference", "gamma-patterns"]), source),
            1,
            "(b) it lists 2 built-in skills; missing, though they require no capability: "
            "beta-reference (crates/meerkat/src/lib.rs)",
        )
        expect(
            "a built-in skill the source does not register",
            run(write_stub(work / "skill-extra", "rkat", CAPABILITIES, healthy + ["stray-skill"]), source),
            1,
            "not registered in the source: stray-skill",
        )
        expect(
            "the source gained a capability the binary lacks",
            run(good, write_source(work / "source-capability", NEW_CAPABILITY_RS)),
            1,
            "the source registers 4 for it; missing: memory_store (crates/meerkat-tools/src/lib.rs)",
        )
        expect(
            "the source gained an ungated skill the binary lacks",
            run(good, write_source(work / "source-skill", NEW_UNGATED_SKILL_RS)),
            1,
            "missing, though they require no capability: epsilon-new (crates/meerkat-tools/src/lib.rs)",
        )
        for version in ("0.9.14", "v0.8.52", "0.9.15"):
            expect(
                f"VERSION={version} does not skip the check",
                run(unregistered, source, version=version),
                1,
                "(a) it reports 0 capabilities",
            )
        expect(
            "a target this host does not run is reported as not run",
            run(unregistered, source, "--if-runnable-target", "no-such-target"),
            0,
            "READ: not an ELF file, nothing to read. NOT RUN: built for no-such-target",
        )
        expect(
            "the Bazel output name needs --surface",
            run(write_stub(work / "bazel-name", "rkat-rpc", CAPABILITIES, healthy).rename(work / "bazel-name" / "rkat_rpc_bin"), source),
            2,
            "pass --surface",
        )
        expect(
            "two init-array tables are refused without running the file",
            run(elf_with_init_array_tables(work / "two-tables" / "rkat-rpc", 2), source),
            1,
            "READ: the file has 2 init-array tables",
        )
        expect(
            "two init-array tables are refused for a target the host does not run",
            run(
                elf_with_init_array_tables(work / "two-tables-arm" / "rkat", 2, machine=183),
                source,
                "--if-runnable-target",
                "no-such-target",
            ),
            1,
            "READ: the file has 2 init-array tables",
        )
        expect(
            "one init-array table passes the read",
            run(
                elf_with_init_array_tables(work / "one-table" / "rkat", 1),
                source,
                "--if-runnable-target",
                "no-such-target",
            ),
            0,
            "READ: one init-array table. NOT RUN",
        )
        expect("an unknown binary name is a usage error", run(write_stub(work / "odd", "rkat-odd", [], []), source), 2)
        expect("a missing binary is a usage error", run(work / "absent" / "rkat", source), 2, "no such binary")
        expect("a tree that is no checkout is refused", run(good, work), 2, "pass --source-root")

        surfaced = subprocess.run(
            [
                sys.executable,
                str(CHECK),
                "--source-root",
                str(source),
                "--surface",
                "rkat-rpc",
                str(work / "bazel-name" / "rkat_rpc_bin"),
            ],
            capture_output=True,
            text=True,
            timeout=300,
            check=False,
        )
        expect("--surface names the binary of a Bazel output", surfaced, 0, "rkat-rpc: READ")

    if FAILURES:
        print("\n" + "\n".join(FAILURES), file=sys.stderr)
        return 1
    print("check-release-binary-registrations contract: all cases hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
