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
    pass


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
    with archive.open("xb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as zipped:
            with tarfile.open(fileobj=zipped, mode="w", format=tarfile.USTAR_FORMAT) as tar:
                for name, (mode, data) in sorted(files.items()):
                    info = tarfile.TarInfo(root + "/" + name)
                    info.size, info.mode, info.mtime = len(data), mode, 0
                    info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    tar.addfile(info, io.BytesIO(data))
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", type=Path, required=True, help="reviewed kit-inputs.json and retained local bytes")
    parser.add_argument("--output", type=Path, required=True, help="new distribution directory; parent must exist")
    args = parser.parse_args()
    try:
        result = package(args.inputs, args.output)
    except (Rejected, OSError, ValueError, TypeError, KeyError, RecursionError, tarfile.TarError):
        parser.exit(2, "installation kit packaging refused; preserve any partial output\n")
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
