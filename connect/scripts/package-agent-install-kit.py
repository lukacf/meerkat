#!/usr/bin/env python3
"""Package reviewed local inputs without executing any delivered program."""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import stat
import tarfile
import tomllib

ROOT = "agent-install-kit-macos-arm64"
LINUX_ROOT = "agent-install-kit-linux-x86_64"
MAX_FILE = 64 * 1024 * 1024
MAX_TOTAL = 256 * 1024 * 1024
MAX_FILES = 5000
MAX_MANIFEST = 2 * 1024 * 1024
RESERVED = {"SHA256SUMS", "provenance/kit.json"}
REQUIRED = {
    "README.md", "agent/instructions.md", "host-profile.example.json",
    "bin/install-library-consumer.py", "bin/toolkit-package-inspect",
    "bin/toolkit-python-plan", "bin/toolkit-node-plan",
    "contracts/INSPECT.md", "contracts/PYTHON-INSTALL.md", "contracts/NODE-INSTALL.md",
    "licenses/README.md", "licenses/toolkit/LICENSE-MIT", "licenses/toolkit/LICENSE-APACHE",
    "licenses/rust/COPYRIGHT-library.html", "provenance/dependencies.json",
}


MCPB_REQUIRED = {
    "bin/toolkit-mcpb-install", "contracts/MCPB-INSTALL.md",
    "provenance/mcpb/source/Cargo.toml", "provenance/mcpb/source/Cargo.lock",
    "provenance/mcpb/accepted/acceptance.json",
    "provenance/mcpb/binary-observations.json", "provenance/mcpb/compiled-closure.json",
}
PYTHON_REQUIRED = (REQUIRED - {"bin/toolkit-node-plan", "contracts/NODE-INSTALL.md"}) | {
    "setup/backend.py", "setup/mcp_adapter.py", "setup/python_acquisition.py",
    "contracts/SETUP-MCP.md", "contracts/SETUP-SELECTED.md",
    "contracts/PYTHON-ACQUISITION.md", "contracts/ACQUISITION-RECOVERY.md",
}
LINUX_NODE_REQUIRED = PYTHON_REQUIRED | REQUIRED | {
    "setup/node_acquisition.py", "contracts/NODE-ACQUISITION.md",
}
SCOPES = {
    "selected-python-node-installation-kit": REQUIRED,
    "selected-python-node-mcpb-installation-kit": REQUIRED | MCPB_REQUIRED,
    "selected-python-installation-kit": PYTHON_REQUIRED,
}


class Rejected(ValueError):
    def __init__(self, reason, names=()):
        super().__init__(reason)
        self.names = sorted(names)


# The macOS kit built in the run. Every input is the checkout's, a pinned public file or
# something this run built: no donor archive, no retained history tree, no MCP check tool.
RUN_SCOPE = "run-built-selected-tools-kit"
RUN_TARGET = "aarch64-apple-darwin"
# A thin arm64 Mach-O executable: magic, CPU type, CPU subtype, file type.
MACHO_ARM64_EXECUTABLE = bytes.fromhex("cffaedfe0c0000010000000002000000")
# Tool name to the Cargo features its build record must name.
RUN_TOOLS = {
    "toolkit-local-recipes": ("local-recipes",),
    # MCPB archive inspection exists only in a local-artifacts build of the inspector.
    "toolkit-package-inspect": ("local-artifacts",),
    "toolkit-python-plan": (), "toolkit-node-plan": (), "toolkit-compose-plan": (),
    "toolkit-mcpb-install": ("local-artifacts",),
}
RUN_CHECKOUT = {
    "bin/install-library-consumer.py": "scripts/install-library-consumer.py",
    "contracts/INSPECT.md": "crates/toolkit-package/INSPECT.md",
    "contracts/PYTHON-INSTALL.md": "crates/toolkit-package/PYTHON-INSTALL.md",
    "contracts/NODE-INSTALL.md": "crates/toolkit-package/NODE-INSTALL.md",
    "contracts/MCPB-INSTALL.md": "crates/toolkit-package/MCPB-INSTALL.md",
    "contracts/LOCAL-RECIPES.md": "crates/toolkit-package/LOCAL-RECIPES.md",
    "licenses/toolkit/LICENSE-MIT": "LICENSE-MIT",
    "licenses/toolkit/LICENSE-APACHE": "LICENSE-APACHE",
}
RUN_WORKSPACE_CRATES = {"toolkit-package", "toolkit-store"}
PIN_KINDS = {"pypi-wheel", "npm-tarball", "crate", "release-download"}
CRATE_USE = "tool build provenance"
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
NOTICE_LEAF = re.compile(r"(?i)(licen[cs]e|copying|copyright|notice|unlicense|patents|authors)[A-Za-z0-9_.+-]*\Z")
DEPENDENCY_LINE = re.compile(r"([A-Za-z0-9][A-Za-z0-9_-]{0,63}) v([0-9][A-Za-z0-9.+-]{0,63})((?: \([^()]+\))*)\Z")
MAX_NOTICE = 4 * 1024 * 1024
MAX_PINS = 16 * 1024 * 1024


def relative(value):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_.+/-]{1,220}", value):
        raise Rejected("invalid relative path")
    p = PurePosixPath(value)
    if p.is_absolute() or any(x in ("", ".", "..") or len(x) > 100 for x in value.split("/")):
        raise Rejected("invalid relative path")
    return p


def read_file(base, name, maximum):
    parts = relative(name).parts
    path = base
    for part in parts:
        path = path / part
        mode = path.lstat().st_mode
        if stat.S_ISLNK(mode):
            raise Rejected("input symlink")
    if not stat.S_ISREG(mode) or path.stat().st_size > maximum:
        raise Rejected("invalid input file")
    with path.open("rb") as stream:
        data = stream.read(maximum + 1)
    if len(data) > maximum:
        raise Rejected("input too large")
    return data


def encoded(value):
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n").encode()


def load_inputs(directory):
    if directory.is_symlink() or not directory.is_dir():
        raise Rejected("invalid input directory")
    raw = read_file(directory, "kit-inputs.json", MAX_MANIFEST)
    spec = json.loads(raw)
    if not isinstance(spec, dict) or set(spec) - {"platform"} != {"schema_version", "scope", "files"}:
        raise Rejected("invalid input manifest")
    if type(spec["schema_version"]) is not int or spec["schema_version"] != 1:
        raise Rejected("invalid input manifest version")
    if not isinstance(spec["scope"], str) or spec["scope"] not in SCOPES:
        raise Rejected("invalid input scope")
    platform = spec.get("platform", "macos-arm64")
    if (platform not in ("macos-arm64", "linux-x86_64")
            or platform == "macos-arm64" and spec["scope"] == "selected-python-installation-kit"
            or platform == "linux-x86_64" and spec["scope"] == "selected-python-node-mcpb-installation-kit"):
        raise Rejected("input scope and platform differ")
    required = (LINUX_NODE_REQUIRED if platform == "linux-x86_64"
                and spec["scope"] == "selected-python-node-installation-kit" else SCOPES[spec["scope"]])
    entries = spec["files"]
    if not isinstance(entries, list) or not 1 <= len(entries) <= MAX_FILES:
        raise Rejected("invalid input count")
    files = {}
    total = 0
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != {"source", "path", "sha256", "mode"}:
            raise Rejected("invalid file entry")
        name = str(relative(entry["path"]))
        if name.casefold() in {path.casefold() for path in RESERVED | set(files)}:
            raise Rejected("duplicate or reserved output path")
        if type(entry["mode"]) is not int or entry["mode"] not in (0o644, 0o755):
            raise Rejected("invalid file mode")
        if entry["mode"] != (0o755 if name.startswith("bin/") else 0o644):
            raise Rejected("unexpected executable mode")
        if not isinstance(entry["sha256"], str) or not re.fullmatch("[0-9a-f]{64}", entry["sha256"]):
            raise Rejected("invalid file digest")
        data = read_file(directory, entry["source"], MAX_FILE)
        if hashlib.sha256(data).hexdigest() != entry["sha256"]:
            raise Rejected("changed reviewed input")
        total += len(data)
        if total > MAX_TOTAL:
            raise Rejected("input total too large")
        files[name] = (entry["mode"], data)
    if not required.issubset(files):
        raise Rejected("incomplete installation kit")
    if platform == "linux-x86_64" and any(name.startswith("bin/") and name not in required for name in files):
        raise Rejected("Linux selected kit includes an unsupported executable tool")
    if platform == "macos-arm64" and spec["scope"] == "selected-python-node-installation-kit" and (
            {"bin/toolkit-mcpb-install", "contracts/MCPB-INSTALL.md"} & set(files)):
        raise Rejected("MCPB tools require the explicit MCPB kit scope")
    final_paths = set(files) | RESERVED
    for name in final_paths:
        for parent in PurePosixPath(name).parents:
            if str(parent).casefold() in {path.casefold() for path in final_paths}:
                raise Rejected("file and directory collision")
    metadata = {
        "schema_version": 1,
        "scope": spec["scope"],
        "input_manifest_sha256": hashlib.sha256(raw).hexdigest(),
        "platform": platform,
        "runtime_delivery": (
            "host-supplied executable runtimes; offline verification-environment inputs bundled"
            if "verification-environment/CONTENTS.json" in files else "host-supplied; none bundled"
        ),
        "files": {name: {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data), "mode": mode}
                  for name, (mode, data) in sorted(files.items())},
    }
    files["provenance/kit.json"] = (0o644, encoded(metadata))
    sums = "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n" for name, (_, data) in sorted(files.items()))
    files["SHA256SUMS"] = (0o644, sums.encode("ascii"))
    return files


def write_archive(files, root, archive):
    """Deterministic USTAR members in name order; exclusive creation."""
    with archive.open("xb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as zipped:
            with tarfile.open(fileobj=zipped, mode="w", format=tarfile.USTAR_FORMAT) as tar:
                for name, (mode, data) in sorted(files.items()):
                    info = tarfile.TarInfo(root + "/" + name)
                    info.size, info.mode, info.mtime = len(data), mode, 0
                    info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    tar.addfile(info, io.BytesIO(data))


def package(directory, output):
    """Freeze all checked bytes before exclusive output creation. Never overwrite."""
    files = load_inputs(directory)
    platform = json.loads(files["provenance/kit.json"][1])["platform"]
    root = LINUX_ROOT if platform == "linux-x86_64" else ROOT
    if not output.parent.is_dir():
        raise Rejected("output parent missing")
    output.mkdir(mode=0o755)
    archive = output / (root + ".tar.gz")
    # Failure after creation leaves the partial output visible; callers use a new
    # output directory after investigating. There is no repair or silent retry.
    write_archive(files, root, archive)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (output / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n", encoding="ascii")
    if platform == "linux-x86_64":
        includes_node = "bin/toolkit-node-plan" in files
        (output / "README.md").write_text(
            "# Agent installation kit\n\n"
            f"Verify the sibling `SHA256SUMS`, then extract [{archive.name}]({archive.name}). "
            f"Open `{root}/README.md` for the complete guide, exact agent instructions, "
            + ("selected Python/Node contracts and host requirements. This Linux x86_64 kit supports "
               "agent-selected Python and Node acquisition, installation and retained-file readback. "
               "Native 64-bit CPython 3.11, Node 24 and acquisition tools are host-supplied. "
               if includes_node else
               "selected Python contracts and host requirements. This Linux x86_64 kit supports "
               "agent-selected Python acquisition, installation and retained-file readback. "
               "Native 64-bit CPython 3.11 is host-supplied. ") +
            ("Offline MCP verification dependencies are bundled; follow the extracted bootstrap guide.\n"
             if "verification-environment/CONTENTS.json" in files else
             "MCP verification dependencies and server packages are not bundled.\n"), encoding="ascii")
        return {"archive": archive.name, "sha256": digest, "files": len(files), "bytes": archive.stat().st_size}
    (output / "README.md").write_text(
        "# Agent installation kit\n\n"
        f"Verify the sibling `SHA256SUMS`, then extract [{archive.name}]({archive.name}). "
        f"Open `{ROOT}/README.md` for the complete guide, exact agent instructions, " +
        ("selected Python/Node/MCPB contracts and host requirements. This local macOS arm64 "
         if "bin/toolkit-mcpb-install" in files else
         "selected Python/Node contracts and host requirements. This local macOS arm64 ") +
        ("kit supplies offline MCP verification dependencies; Python and Node executables remain host-supplied. "
         "See the extracted guide for the included, unconfigured services package.\n"
         if "verification-environment/CONTENTS.json" in files else
         "kit supplies no host runtime or default server package.\n"), encoding="ascii")
    return {"archive": archive.name, "sha256": digest, "files": len(files), "bytes": archive.stat().st_size}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Rejected("duplicate JSON key")
        result[key] = value
    return result


def regular_file(path, maximum):
    """One regular file by absolute path; a final link, a directory or an oversized file is refused."""
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_size > maximum:
        raise Rejected("not a bounded regular file")
    with path.open("rb") as stream:
        data = stream.read(maximum + 1)
    if len(data) > maximum:
        raise Rejected("not a bounded regular file")
    return data, stat.S_IMODE(info.st_mode)


def load_pins(path):
    """The committed pin file (connect-library-pins/1): rows by (kind, file). Nothing is fetched here."""
    try:
        raw, _ = regular_file(path, MAX_PINS)
        document = json.loads(raw, object_pairs_hook=unique_object)
        if (not isinstance(document, dict) or document.get("schema") != "connect-library-pins/1"
                or document.get("target") != "macos-arm64" or not isinstance(document.get("pins"), list)):
            raise ValueError
        rows = {}
        for row in document["pins"]:
            if (not isinstance(row, dict) or row.get("kind") not in PIN_KINDS
                    or any(not isinstance(row.get(key), str) or not row[key] for key in ("name", "version", "file", "use"))
                    or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.+@-]{0,199}", row["file"])
                    or any(row.get(key) is not None and (not isinstance(row[key], str)
                           or not re.fullmatch("[0-9a-f]{%d}" % size, row[key]))
                           for key, size in (("sha256", 64), ("sha512", 128)))
                    or row.get("sha256") is None and row.get("sha512") is None
                    or (row["kind"], row["file"]) in rows):
                raise ValueError
            rows[(row["kind"], row["file"])] = row
    except (OSError, ValueError, TypeError, RecursionError):
        raise Rejected("invalid_pin_file") from None
    return raw, rows


def pinned_file(directory, row):
    """Read <directory>/<kind>/<file> and check it against its pin row again."""
    try:
        for folder in (directory, directory / row["kind"]):
            if not stat.S_ISDIR(folder.lstat().st_mode):
                raise Rejected("not a directory")
        data, _ = regular_file(directory / row["kind"] / row["file"], MAX_FILE)
    except (OSError, Rejected):
        raise Rejected("pinned_input_unavailable", [row["file"]]) from None
    if (row.get("sha256") is not None and sha256(data) != row["sha256"]
            or row.get("sha512") is not None and hashlib.sha512(data).hexdigest() != row["sha512"]):
        raise Rejected("pinned_input_digest_mismatch", [row["file"]])
    return data


def tool_dependencies(raw):
    """Parse `cargo tree --prefix none --format "{p}"`: registry crates and the workspace crates."""
    registry, workspace = set(), set()
    try:
        lines = raw.decode("utf-8").splitlines()
    except UnicodeError:
        raise Rejected("invalid_tool_dependency_listing") from None
    for line in lines:
        if not line:
            continue
        match = DEPENDENCY_LINE.match(line)
        if match is None:
            raise Rejected("invalid_tool_dependency_listing")
        name, version, tail = match.groups()
        notes = re.findall(r" \(([^()]+)\)", tail)
        paths = [note for note in notes if note.startswith("/")]
        if any(note not in ("proc-macro", "*") and not note.startswith("/") for note in notes) or len(paths) > 1:
            raise Rejected("invalid_tool_dependency_listing")
        if paths:
            if name not in RUN_WORKSPACE_CRATES:
                raise Rejected("invalid_tool_dependency_listing")
            workspace.add((name, version))
        else:
            registry.add((name, version))
    if not registry or "toolkit-package" not in {name for name, _ in workspace}:
        raise Rejected("invalid_tool_dependency_listing")
    return sorted(registry), sorted(workspace)


def crate_notices(name, version, raw):
    """The licence and notice files at the root of one crate archive. No member is extracted to disk."""
    prefix, notices, seen = f"{name}-{version}/", {}, 0
    try:
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as archive:
            for member in archive:
                seen += 1
                if seen > 20000:
                    raise Rejected("oversized crate archive")
                leaf = member.name.removeprefix(prefix)
                if (not member.name.startswith(prefix) or "/" in leaf or not member.isfile()
                        or NOTICE_LEAF.match(leaf) is None):
                    continue
                if member.size > MAX_NOTICE or leaf.casefold() in {other.casefold() for other in notices}:
                    raise Rejected("unsupported crate notice")
                relative(leaf)
                notices[leaf] = archive.extractfile(member).read(MAX_NOTICE + 1)
    except (tarfile.TarError, OSError, EOFError, Rejected):
        raise Rejected("invalid_tool_crate_archive", [f"{name}-{version}.crate"]) from None
    return notices


def run_tool(tools, tool):
    """One tool built in this run and the record that binds its signed bytes, target and features."""
    try:
        data, mode = regular_file(tools / tool, MAX_FILE)
        record_raw, _ = regular_file(tools / (tool + ".build.json"), MAX_MANIFEST)
    except (OSError, Rejected):
        raise Rejected("tool_unavailable", [tool]) from None
    if mode != 0o755:
        raise Rejected("tool_unavailable", [tool])
    if data[:16] != MACHO_ARM64_EXECUTABLE:
        raise Rejected("tool_not_macos_arm64_executable", [tool])
    try:
        record = json.loads(record_raw, object_pairs_hook=unique_object)
        command, features = record["command"], record["features"]
        if (record["binary"] != "tools/" + tool or record["sha256"] != sha256(data)
                or type(record["bytes"]) is not int or record["bytes"] != len(data)
                or record["mode"] != "0755" or record["target"] != RUN_TARGET
                or not isinstance(command, list) or not all(isinstance(value, str) for value in command)
                or command.count("--bin") != 1 or command.index("--bin") + 1 >= len(command)
                or command[command.index("--bin") + 1] != tool
                or "--locked" not in command or "--release" not in command
                or not isinstance(features, list) or not all(isinstance(value, str) for value in features)
                or not set(RUN_TOOLS[tool]) <= set(features)):
            raise ValueError
    except (KeyError, TypeError, ValueError, RecursionError):
        raise Rejected("tool_build_record_mismatch", [tool]) from None
    return data, record_raw


def run_built_files(*, checkout, lock, pins, pinned_inputs, tools, tool_dependencies_path, rust_license):
    """Collect and check every member of the kit built in the run. Reads only; nothing is executed."""
    paths = (checkout, lock, pins, pinned_inputs, tools, tool_dependencies_path, rust_license)
    if any(not isinstance(path, Path) or not path.is_absolute() for path in paths):
        raise Rejected("invalid_run_selection")
    files = {}
    for name, source in RUN_CHECKOUT.items():
        try:
            files[name] = (0o755 if name.startswith("bin/") else 0o644, read_file(checkout, source, MAX_FILE))
        except (OSError, Rejected):
            raise Rejected("checkout_input_unavailable", [source]) from None
    for tool in RUN_TOOLS:
        data, record = run_tool(tools, tool)
        files["bin/" + tool] = (0o755, data)
        files["provenance/tools/" + tool + ".build.json"] = (0o644, record)
    try:
        licence, _ = regular_file(rust_license, 16 * 1024 * 1024)
        if not licence:
            raise Rejected("empty")
    except (OSError, Rejected):
        raise Rejected("rust_license_unavailable") from None
    files["licenses/rust/COPYRIGHT-library.html"] = (0o644, licence)
    pins_raw, rows = load_pins(pins)
    try:
        listing, _ = regular_file(tool_dependencies_path, MAX_MANIFEST)
        lock_raw, _ = regular_file(lock, 16 * 1024 * 1024)
        locked = {(row["name"], row["version"]): (row.get("source"), row.get("checksum"))
                  for row in tomllib.loads(lock_raw.decode("utf-8"))["package"]}
    except (OSError, Rejected, KeyError, TypeError, ValueError):
        raise Rejected("invalid_tool_dependency_listing") from None
    registry, workspace = tool_dependencies(listing)
    if any(locked.get(crate, ("registry", None))[0] is not None for crate in workspace):
        raise Rejected("invalid_tool_dependency_listing")
    # The closure is checked whole, so one refusal names every crate of its kind.
    archive_name = lambda crate: f"{crate[0]}-{crate[1]}.crate"
    unlocked = [archive_name(crate) for crate in registry
                if locked.get(crate, (None, None))[0] != CRATES_IO or not isinstance(locked[crate][1], str)]
    if unlocked:
        raise Rejected("tool_crate_not_locked", unlocked)
    selected = {crate: rows.get(("crate", archive_name(crate))) for crate in registry}
    unpinned = [archive_name(crate) for crate, row in selected.items() if row is None or row["use"] != CRATE_USE]
    if unpinned:
        raise Rejected("tool_crate_not_pinned", unpinned)
    different = [archive_name(crate) for crate, row in selected.items() if row["sha256"] != locked[crate][1]]
    if different:
        raise Rejected("tool_crate_pin_differs_from_lock", different)
    crates, without = [], []
    for crate in registry:
        name, version = crate
        raw = pinned_file(pinned_inputs, selected[crate])
        notices = crate_notices(name, version, raw)
        if not notices:
            without.append(archive_name(crate))
            continue
        files["provenance/crates/" + archive_name(crate)] = (0o644, raw)
        for leaf, data in notices.items():
            files[f"licenses/crates/{name}-{version}/{leaf}"] = (0o644, data)
        crates.append({"name": name, "version": version, "checksum": locked[crate][1],
                       "archive": "provenance/crates/" + archive_name(crate),
                       "notices": sorted(f"licenses/crates/{name}-{version}/{leaf}" for leaf in notices)})
    if without:
        raise Rejected("tool_crate_without_notice", without)
    inputs = {"pins_sha256": sha256(pins_raw), "lock_sha256": sha256(lock_raw),
              "tool_dependencies_sha256": sha256(listing)}
    files["provenance/dependencies.json"] = (0o644, encoded({
        "schema_version": 1, "scope": "tool_dependency_closure_of_this_run", "target": RUN_TARGET,
        "listing_sha256": inputs["tool_dependencies_sha256"], "lock_sha256": inputs["lock_sha256"],
        "workspace_crates": [{"name": name, "version": version} for name, version in workspace],
        "registry_crates": crates}))
    folded = {}
    for name in files:
        relative(name)
        if folded.setdefault(name.casefold(), name) != name:
            raise Rejected("case_colliding_members")
    if (len(files) + len(RESERVED) > MAX_FILES or any(len(data) > MAX_FILE for _, data in files.values())
            or sum(len(data) for _, data in files.values()) > MAX_TOTAL):
        raise Rejected("kit_too_large")
    files["provenance/kit.json"] = (0o644, encoded({
        "schema_version": 1, "scope": RUN_SCOPE, "platform": "macos-arm64",
        "runtime_delivery": "host-supplied; none bundled", "inputs": inputs,
        "files": {name: {"sha256": sha256(data), "bytes": len(data), "mode": mode}
                  for name, (mode, data) in sorted(files.items())}}))
    sums = "".join(f"{sha256(data)}  {name}\n" for name, (_, data) in sorted(files.items()))
    files["SHA256SUMS"] = (0o644, sums.encode("ascii"))
    return files, inputs


def package_run_built(*, checkout, lock, pins, pinned_inputs, tools, tool_dependencies, rust_license, output):
    """Build the macOS kit from this run's inputs. Every check precedes exclusive output creation."""
    files, inputs = run_built_files(checkout=checkout, lock=lock, pins=pins, pinned_inputs=pinned_inputs,
                                    tools=tools, tool_dependencies_path=tool_dependencies, rust_license=rust_license)
    if not isinstance(output, Path) or not output.is_absolute() or not output.parent.is_dir():
        raise Rejected("output parent missing")
    output.mkdir(mode=0o755)
    archive = output / (ROOT + ".tar.gz")
    write_archive(files, ROOT, archive)
    digest = sha256(archive.read_bytes())
    (output / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n", encoding="ascii")
    (output / "README.md").write_text(
        "# Agent installation kit, built in this run\n\n"
        f"Verify the sibling `SHA256SUMS`. [{archive.name}]({archive.name}) is an input of the macOS arm64 "
        "library packager: the six tools this run built with their build records, the library installer, "
        "the tool contracts, and the licence texts and crate archives of the tools' dependency closure. "
        "It supplies no host runtime and no server package, and nothing in it was executed here.\n",
        encoding="ascii")
    result = {"archive": archive.name, "sha256": digest, "files": len(files), "bytes": archive.stat().st_size,
              "platform": "macos-arm64", "scope": RUN_SCOPE, "inputs": inputs}
    (output / "build.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="ascii")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--inputs", type=Path, help="reviewed kit-inputs.json and retained local bytes")
    parser.add_argument("--output", type=Path, required=True, help="new distribution directory; parent must exist")
    run = parser.add_argument_group("the macOS kit built in the run (selected by --pins; never with --inputs)")
    run.add_argument("--target", choices=("macos-arm64",))
    run.add_argument("--pins", type=Path, help="the committed pin file (connect-library-pins/1)")
    run.add_argument("--pinned-inputs", type=Path, help="directory of fetched pinned files: <kind>/<file>")
    run.add_argument("--tools", type=Path, help="the six tools of this run, each with <tool>.build.json")
    run.add_argument("--tool-dependencies", type=Path, help="stdout of the tools' cargo tree listing")
    run.add_argument("--rust-license", type=Path, help="COPYRIGHT-library.html of the building toolchain")
    run.add_argument("--checkout", type=Path, default=Path(__file__).resolve().parents[1],
                     help="the connect directory of the release commit; default: this script's")
    run.add_argument("--cargo-lock", type=Path, default=Path(__file__).resolve().parents[2] / "Cargo.lock",
                     help="the root lock of the release commit; default: this checkout's")
    args = parser.parse_args()
    selected = (args.target, args.pins, args.pinned_inputs, args.tools, args.tool_dependencies, args.rust_license)
    if all(value is None for value in selected):
        if args.inputs is None:
            parser.error("the following arguments are required: --inputs")
        try:
            result = package(args.inputs, args.output)
        except (Rejected, OSError, ValueError, TypeError, KeyError, RecursionError, tarfile.TarError):
            parser.exit(2, "installation kit packaging refused; preserve any partial output\n")
        print(json.dumps(result, sort_keys=True))
        return
    if args.inputs is not None or any(value is None for value in selected):
        parser.exit(2, "installation kit packaging refused: incomplete_run_selection\n")
    try:
        result = package_run_built(checkout=args.checkout, lock=args.cargo_lock, pins=args.pins,
                                   pinned_inputs=args.pinned_inputs, tools=args.tools,
                                   tool_dependencies=args.tool_dependencies, rust_license=args.rust_license,
                                   output=args.output)
    except Rejected as error:
        names = ": " + ", ".join(error.names) if error.names else ""
        parser.exit(2, f"installation kit packaging refused: {error}{names}\n")
    except (OSError, ValueError, TypeError, KeyError, RecursionError, tarfile.TarError):
        parser.exit(2, "installation kit packaging refused: input_or_output_unavailable\n")
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
