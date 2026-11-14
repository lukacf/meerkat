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
environment never skips the check. A row of the check's exclusion table is
honoured when it names a registered capability and refused when it does not.

Three release jobs give the check a sparse checkout of the release tree
instead of a full one. The last cases read those jobs' `sparse-checkout`
lists from .github/workflows/release.yml and require that each list holds
every file the check reads: the check runs on a tree made of exactly the
files a list selects, and every source file the check reads in this checkout
is matched against the list. A list that lacks a path the check reads (the
member assertion's skill body lives outside `src/`) fails here, not in a
release run. No network beyond loopback, no Rust build.
"""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHECK = ROOT / "scripts" / "check-release-binary-registrations"
RELEASE_WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
# The step that checks out the tree a job passes to the check as
# `--source-root release-tree`, and that flag.
RELEASE_TREE_STEP = "- name: Checkout the release tree's registrations"
RELEASE_TREE_FLAG = "--source-root release-tree"
# The exclusion table of the check, as the cases that fill it find it.
NOT_REPORTED_LINE = "NOT_REPORTED: dict[str, dict[str, str]] = {surface: {} for surface in SURFACES}\n"

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

# The skill every mob member build preloads, registered with a body file.
COMMS_LIB_RS = """\
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "mob-communication",
        name: "Mob Communication",
        requires_capabilities: &["comms"],
        body: include_str!("../skills/mob-communication/SKILL.md"),
        extensions: &[],
    }
}
"""

# Not the real skill's heading: the check must read it from the body file.
MEMBER_HEADING = "# Working With Peers (fixture)"
MOB_SKILL_MD = f"""---
name: Mob Communication
description: fixture
requires_capabilities: [comms]
---

{MEMBER_HEADING}

Body.
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
MEMBER_SKILL_BODY = __MEMBER_SKILL_BODY__
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
    def member_refusal():
        # The member spawn needs what the check provides: the `global` realm
        # and a self-hosted model with auth method `none` on a loopback port.
        config = Path(os.environ["HOME"]) / ".rkat" / "config.toml"
        text = config.read_text(encoding="utf-8") if config.is_file() else ""
        if sys.argv[-2:] != ["--realm", "global"]:
            return "the member spawn was not started in the global realm"
        if 'auth_method = "none"' not in text or 'base_url = "http://127.0.0.1:' not in text:
            return "the member's model is not a keyless self-hosted server on loopback"
        return None

    def answer(request):
        method = request.get("method")
        if method == "capabilities/get":
            return capabilities()
        if method == "skills/list":
            return {"skills": skills()}
        if method == "mob/create":
            return {"mob_id": request["params"]["definition"]["id"], "claim_token": "stub"}
        if method == "mob/spawn":
            profile = request["params"]["profile"]
            return {"mob_id": request["params"]["mob_id"], "agent_identity": request["params"]["agent_identity"], "profile": profile}
        if method == "mob/member_history":
            refusal = member_refusal()
            prompt = refusal or ("# Inter-agent Communication\n\ntools\n\n" + MEMBER_SKILL_BODY)
            return {"page": {"messages": [{"role": "system", "content": prompt}]}}
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


def write_source(root: Path, extra_tools: str = "", member_skill: bool = True) -> Path:
    crates = {
        "meerkat": {"lib.rs": FACADE_LIB_RS, "help.rs": HELP_RS},
        "meerkat-tools": {"lib.rs": TOOLS_LIB_RS + extra_tools},
        "meerkat-capabilities": {"lib.rs": CAPABILITIES_LIB_RS},
    }
    if member_skill:
        crates["meerkat-comms"] = {"lib.rs": COMMS_LIB_RS}
        skill = root / "crates" / "meerkat-comms" / "skills" / "mob-communication"
        skill.mkdir(parents=True, exist_ok=True)
        (skill / "SKILL.md").write_text(MOB_SKILL_MD, encoding="utf-8")
    for crate, files in crates.items():
        src = root / "crates" / crate / "src"
        src.mkdir(parents=True, exist_ok=True)
        (root / "crates" / crate / "Cargo.toml").write_text('[package]\nname = "x"\n', encoding="utf-8")
        for name, text in files.items():
            (src / name).write_text(text, encoding="utf-8")
    return root


def write_stub(
    directory: Path, name: str, capabilities: list[str], builtin: list[str], member: bool | None = None
) -> Path:
    """A stub release binary. `member`: whether a mob member it spawns carries
    the mob-communication body (default: when it has any built-in skill)."""
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / name
    carries = bool(builtin) if member is None else member
    path.write_text(
        STUB.replace("__CAPABILITIES__", json.dumps(capabilities))
        .replace("__BUILTIN__", json.dumps(builtin))
        .replace("__MEMBER_SKILL_BODY__", json.dumps(MOB_SKILL_MD if carries else "")),
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


def load_checker():
    """The check as a module, for the cases that call it directly."""
    loader = importlib.machinery.SourceFileLoader("check_release_binary_registrations", str(CHECK))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


def run(
    binary: Path, source: Path, *flags: str, version: str | None = None, check: Path = CHECK
) -> subprocess.CompletedProcess:
    env = {key: value for key, value in os.environ.items() if key != "VERSION"}
    if version is not None:
        env["VERSION"] = version
    return subprocess.run(
        [sys.executable, str(check), *flags, "--source-root", str(source), str(binary)],
        env=env,
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )


def check_with_not_reported(directory: Path, rows: dict[str, str]) -> Path:
    """A copy of the check whose exclusion table holds `rows` for every
    binary. The table's line must occur exactly once, so a rewording of it
    fails here instead of leaving the table untested."""
    text = CHECK.read_text(encoding="utf-8")
    if text.count(NOT_REPORTED_LINE) != 1:
        raise SystemExit(
            f"{CHECK} holds the line {NOT_REPORTED_LINE.strip()!r} {text.count(NOT_REPORTED_LINE)} times, "
            "expected once: update NOT_REPORTED_LINE in this test"
        )
    directory.mkdir(parents=True, exist_ok=True)
    copy = directory / CHECK.name
    copy.write_text(
        text.replace(
            NOT_REPORTED_LINE,
            f"NOT_REPORTED: dict[str, dict[str, str]] = {{surface: {rows!r} for surface in SURFACES}}\n",
        ),
        encoding="utf-8",
    )
    return copy


def release_tree_checkouts() -> tuple[dict[str, list[str]], set[str], set[str]]:
    """(job id -> the sparse-checkout patterns of its release-tree checkout
    step, the ids of the jobs that have that step, the ids of the jobs that
    pass `release-tree` to the check), read from the release workflow's text.
    A step without a list is a full checkout: nothing can be missing from it."""
    lines = RELEASE_WORKFLOW.read_text(encoding="utf-8").split("\n")
    lists: dict[str, list[str]] = {}
    steps: set[str] = set()
    users: set[str] = set()
    job = ""
    index = 0
    while index < len(lines):
        line = lines[index]
        head = re.fullmatch(r"  ([A-Za-z0-9_-]+):", line)
        if head:
            job = head.group(1)
        if RELEASE_TREE_FLAG in line:
            users.add(job)
        if line.strip() == RELEASE_TREE_STEP:
            steps.add(job)
            step_indent = len(line) - len(line.lstrip())
            cursor = index + 1
            while cursor < len(lines):
                inner = lines[cursor]
                indent = len(inner) - len(inner.lstrip())
                if inner.strip() and indent <= step_indent:
                    break
                if inner.strip() == "sparse-checkout: |":
                    patterns = lists.setdefault(job, [])
                    cursor += 1
                    while cursor < len(lines) and (
                        not lines[cursor].strip() or len(lines[cursor]) - len(lines[cursor].lstrip()) > indent
                    ):
                        if lines[cursor].strip():
                            patterns.append(lines[cursor].strip())
                        cursor += 1
                    continue
                if inner.strip().startswith("sparse-checkout:"):
                    # A sparse list in a form this reader does not read: an
                    # empty list for the job, which the cases refuse.
                    lists.setdefault(job, [])
                cursor += 1
        index += 1
    return lists, steps, users


def sparse_selects(patterns: list[str], path: str) -> bool:
    """Whether a non-cone sparse checkout with `patterns` holds the file at
    `path` (posix, relative to the root). Models the pattern forms the
    release workflow uses: a root-relative pattern with `*` inside a path
    segment, selecting a file or, with or without a trailing `/`, a directory
    and everything beneath it. Any other form raises: extend the model before
    using it."""
    segments = path.split("/")
    for pattern in patterns:
        body = pattern.rstrip("/")
        if (
            not body
            or body.startswith(("/", "!", "#"))
            or "/" not in body
            or any(token in body for token in ("**", "?", "[", "\\"))
        ):
            raise ValueError(f"sparse pattern form not modelled by this test: {pattern!r}")
        matcher = re.compile("/".join(re.escape(part).replace(r"\*", "[^/]*") for part in body.split("/")))
        # A directory match selects what is beneath it; a pattern with a
        # trailing slash matches directories only.
        longest = len(segments) - 1 if pattern.endswith("/") else len(segments)
        if any(matcher.fullmatch("/".join(segments[:count])) for count in range(1, longest + 1)):
            return True
    return False


def sparse_tree(source: Path, target: Path, patterns: list[str]) -> Path:
    """The files of `source` a sparse checkout with `patterns` holds, copied to `target`."""
    for path in sorted(source.rglob("*")):
        if path.is_file() and sparse_selects(patterns, path.relative_to(source).as_posix()):
            copy = target / path.relative_to(source)
            copy.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, copy)
    return target


def files_the_check_reads(checker, root: Path) -> list[str]:
    """Every file under `root` the check opens when it is given `root` as the
    source root, on a binary whose mob member it spawns: each crate's
    manifest and sources, and the body of the member's skill."""
    read: set[Path] = set(root.glob("crates/*/Cargo.toml"))
    for texts in checker.crate_sources(root):
        read.update(texts)
    _, _, bodies = checker.registrations(root)
    body = bodies.get(checker.MEMBER_SKILL)
    if body is None:
        raise checker.NoAnswer(f"{root} registers no `{checker.MEMBER_SKILL}` skill with a body file")
    read.add(body)
    return sorted(path.resolve().relative_to(root.resolve()).as_posix() for path in read)


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
        # (d): the mob member rkat-rpc spawns.
        expect(
            "rkat-rpc: the spawned mob member carries mob-communication",
            run(work / "good" / "rkat-rpc", source),
            0,
            "the mob member it spawned carries the `mob-communication` skill",
        )
        expect(
            "rkat-rpc: no constructor ran, so the member has no mob-communication",
            run(work / "unregistered" / "rkat-rpc", source),
            1,
            "(d) the mob member it spawned was built without the built-in `mob-communication` skill",
            f"heading `{MEMBER_HEADING}` (crates/meerkat-comms/src/lib.rs)",
        )
        expect(
            "rkat-rpc: lists are whole, the member alone lacks mob-communication",
            run(write_stub(work / "member-without", "rkat-rpc", CAPABILITIES, healthy, member=False), source),
            1,
            "RUN: (d) the mob member it spawned was built without the built-in `mob-communication` skill",
        )
        no_member_skill = write_source(work / "source-no-member-skill", member_skill=False)
        expect(
            "rkat-rpc: a source that registers no mob-communication body cannot be checked",
            run(work / "good" / "rkat-rpc", no_member_skill),
            2,
            "the source registers no `mob-communication` skill with a body file",
        )
        expect(
            "rkat: no mob member is spawned on the other binaries",
            run(good, no_member_skill),
            0,
            "the project skill that requires `sessions` is listed",
        )
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
        checker = load_checker()
        expect(
            "the target this host runs is run",
            run(unregistered, source, "--if-runnable-target", checker.host_target()),
            1,
            "(a) it reports 0 capabilities",
        )
        expect(
            "the Bazel output name needs --surface",
            run(write_stub(work / "bazel-name", "rkat-rpc", CAPABILITIES, healthy).rename(work / "bazel-name" / "rkat_rpc_bin"), source),
            2,
            "pass --surface",
        )
        expect(
            "two init-array tables are refused when the file cannot answer",
            run(elf_with_init_array_tables(work / "two-tables" / "rkat-rpc", 2), source),
            1,
            "READ: the file has 2 init-array tables",
            "RUN: no answer",
        )
        # A file with two tables that does answer correctly (a process start
        # that runs both) is still refused: the stub answers, the read is
        # replaced.
        checker.elf_init_array_tables = lambda path: 2
        try:
            checker.check("rkat", good, source, "")
        except checker.Refused as refusal:
            answered = subprocess.CompletedProcess([], 1, "", str(refusal))
        except checker.NoAnswer as failure:
            # A check that cannot ask: a failed case here, not the end of the test.
            answered = subprocess.CompletedProcess([], 2, "", f"no answer: {failure}")
        else:
            answered = subprocess.CompletedProcess([], 0, "accepted", "")
        expect(
            "two init-array tables are refused when the file answers correctly",
            answered,
            1,
            "READ: the file has 2 init-array tables",
            "RUN: the answers match the source registrations.",
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

        # The exclusion table: a row must name a capability the source
        # registers, and it then takes exactly that capability out of what the
        # binary must report.
        expect(
            "a row of the exclusion table that names no registered capability is refused",
            run(good, source, check=check_with_not_reported(work / "table-stale", {"no_such_capability": "contract"})),
            1,
            "NOT_REPORTED in scripts/check-release-binary-registrations names ['no_such_capability'] for rkat",
            "remove the rows",
        )
        excluding = check_with_not_reported(work / "table-row", {"work_graph": "contract"})
        expect(
            "a row of the exclusion table takes its capability out of what the binary must report",
            run(write_stub(work / "capability-excluded", "rkat", CAPABILITIES[:2], healthy), source, check=excluding),
            0,
            "RUN: 2 capabilities, equal to the source registrations",
        )
        expect(
            "a binary that reports a capability its row excludes is refused",
            run(good, source, check=excluding),
            1,
            "not registered in the source for it: work_graph",
        )

        # The release jobs that give the check a checkout of the release tree
        # of their own: a sparse list must hold every file the check reads.
        lists, steps, users = release_tree_checkouts()
        named = subprocess.CompletedProcess(
            [],
            0 if users and users == steps else 1,
            f"jobs that pass `release-tree` to the check: {sorted(users)}; "
            f"jobs with the step `{RELEASE_TREE_STEP[8:]}`: {sorted(steps)}",
            "",
        )
        expect(
            "every release job that passes a release tree to the check checks it out by the step this test reads",
            named,
            0,
        )
        reading = load_checker()
        try:
            read_here: list[str] | None = files_the_check_reads(reading, ROOT)
            unreadable = ""
        except (OSError, ValueError, reading.NoAnswer) as error:
            read_here, unreadable = None, f"{type(error).__name__}: {error}"
        for job, patterns in sorted(lists.items()):
            try:
                selected = sparse_tree(source, work / "sparse" / job, patterns)
                on_sparse_tree = run(work / "good" / "rkat-rpc", selected)
            except ValueError as error:
                on_sparse_tree = subprocess.CompletedProcess([], 1, "", str(error))
            expect(
                f"{job}: the check runs on a tree made of exactly the files its sparse list selects",
                on_sparse_tree,
                0,
                "the mob member it spawned carries the `mob-communication` skill",
            )
            if read_here is None:
                covered = subprocess.CompletedProcess([], 1, "", unreadable)
            else:
                try:
                    left_out = [path for path in read_here if not sparse_selects(patterns, path)]
                    covered = subprocess.CompletedProcess(
                        [],
                        1 if left_out else 0,
                        f"sparse list {patterns} leaves out {len(left_out)} of the {len(read_here)} files "
                        f"the check reads: {', '.join(left_out[:8])}",
                        "",
                    )
                except ValueError as error:
                    covered = subprocess.CompletedProcess([], 1, "", str(error))
            expect(
                f"{job}: its sparse list selects every file the check reads in this checkout",
                covered,
                0,
            )

    if FAILURES:
        print("\n" + "\n".join(FAILURES), file=sys.stderr)
        return 1
    print("check-release-binary-registrations contract: all cases hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
