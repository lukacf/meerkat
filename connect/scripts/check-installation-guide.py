#!/usr/bin/env python3
"""Read-only consistency check for the advertised standalone installation guide.

The guide chooses the edition. Its checksum, selected profile and referenced kit
files must match the chosen archive. This does not execute package contents or
prove that a host can install it; the separate consumer trial covers that step.
"""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import sys
import tarfile

ROOT = Path(__file__).resolve().parents[1]
GUIDE = "agents/setup-installer/HOST-SETUP.md"
ENTRY_POINTS = ("README.md", "agents/setup-installer/README.md",
                "docs/site/acquisition/local-install.mdx")
LINUX_TOOLS = {"bin/install-library-consumer.py", "bin/toolkit-package-inspect",
               "bin/toolkit-python-plan"}
LINUX_NODE_TOOLS = LINUX_TOOLS | {"bin/toolkit-node-plan"}


def check_combined_links(root):
    """Compare combined-library front doors with README's existing selections."""
    site = "docs/site/library/local-install.mdx"
    pages = {name: (root / name).read_text() for name in (GUIDE, *ENTRY_POINTS)}
    if (root / site).exists():
        pages[site] = (root / site).read_text()
    pattern = (r"dist/toolkit-library-[A-Za-z0-9-]+/"
               r"(?:README\.md|toolkit-library-[A-Za-z0-9-]+-"
               r"(?:macos-arm64|linux-x86_64)\.tar\.gz)")
    marker = r"dist/toolkit-library-|\[current (?:macOS|Linux) archive\]"
    if not any(re.search(marker, text) for text in pages.values()):
        # Standalone-only guides have no combined release to compare.
        return None
    selected = {}
    for target, label in (("macos", "macOS"), ("linux", "Linux")):
        links = re.findall(r"\[current " + label + r" archive\]\(([^)]+)\)", pages["README.md"])
        if len(links) != 1 or re.fullmatch(pattern, links[0]) is None:
            raise ValueError("README must select exactly one current combined " + label + " library")
        path = links[0]
        if path.endswith(".tar.gz"):
            suffix = "macos-arm64" if target == "macos" else "linux-x86_64"
            if not path.endswith("-" + suffix + ".tar.gz"):
                raise ValueError("README combined library archive target differs from " + label + " label")
        if not (root / path).is_file():
            kind = "archive" if path.endswith(".tar.gz") else "guide"
            raise ValueError("missing combined library " + kind + ": " + path)
        selected[target] = path
    if len(set(selected.values())) != 2:
        raise ValueError("combined library targets must be distinct")
    allowed = set(selected.values())
    required = {GUIDE, site, "docs/site/acquisition/local-install.mdx"}
    for name in pages.keys() | required:
        links = set(re.findall(pattern, pages.get(name, "")))
        if not links <= allowed:
            raise ValueError(name + ": combined library edition differs from README")
        if name in required and links != allowed:
            raise ValueError(name + ": must reference both combined library targets")
    for target, label in (("macos", "macOS"), ("linux", "Linux")):
        links = re.findall(r"\[" + label + r" library\]\(([^)]+)\)", pages[GUIDE])
        if (len(links) != 1 or
                ((root / GUIDE).parent / links[0]).resolve() != (root / selected[target]).resolve()):
            raise ValueError(GUIDE + ": combined library target differs from README")
    return selected


def check_linux(directory, relative):
    """Check the selected Linux delivery and its exact tools, without execution."""
    archive_name = "agent-install-kit-linux-x86_64.tar.gz"
    expected = re.fullmatch(r"([0-9a-f]{64})  " + re.escape(archive_name) + r"\n?",
                            (directory / "SHA256SUMS").read_text())
    if expected is None:
        raise ValueError("missing exact Linux archive checksum")
    archive = directory / archive_name
    hasher = hashlib.sha256()
    with archive.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    digest = hasher.hexdigest()
    if digest != expected[1]:
        raise ValueError("Linux archive checksum mismatch")
    prefix = "agent-install-kit-linux-x86_64/"
    with tarfile.open(archive) as tar:
        records = {}
        total = 0
        for member in tar:
            name = member.name.removeprefix(prefix)
            total += member.size
            if (not member.isfile() or not member.name.startswith(prefix)
                    or not name or any(part in ("", ".", "..") for part in name.split("/"))
                    or name in records or not 0 <= member.size <= 64 * 1024 * 1024
                    or len(records) >= 5002 or total > 260 * 1024 * 1024):
                raise ValueError("unexpected Linux archive member")
            records[name] = member

        def read(name):
            item = records.get(name)
            if item is None:
                raise ValueError("missing Linux kit file: " + name)
            return tar.extractfile(item).read()

        metadata = json.loads(read("provenance/kit.json"))
        if (not isinstance(metadata, dict) or type(metadata.get("schema_version")) is not int
                or metadata["schema_version"] != 1 or metadata.get("platform") != "linux-x86_64"
                or metadata.get("scope") not in ("selected-python-installation-kit", "selected-python-node-installation-kit")
                or not isinstance(metadata.get("files"), dict)):
            raise ValueError("Linux kit must declare its selected Python scope and platform")
        tools = LINUX_NODE_TOOLS if metadata["scope"] == "selected-python-node-installation-kit" else LINUX_TOOLS
        manifest = metadata["files"]
        if set(manifest) != set(records) - {"SHA256SUMS", "provenance/kit.json"}:
            raise ValueError("Linux manifest file membership mismatch")
        if {name for name in records if name.startswith("bin/")} != tools:
            raise ValueError("Linux kit must contain exactly its " +
                             ("four Python/Node" if tools == LINUX_NODE_TOOLS else "three Python") + " installation tools")
        hashes = {name: hashlib.sha256(read(name)).hexdigest()
                  for name in records if name != "SHA256SUMS"}
        for name, entry in manifest.items():
            if (not isinstance(entry, dict) or set(entry) != {"sha256", "bytes", "mode"}
                    or entry["sha256"] != hashes[name]
                    or type(entry["bytes"]) is not int or entry["bytes"] != records[name].size
                    or type(entry["mode"]) is not int or entry["mode"] != records[name].mode
                    or entry["mode"] != (0o755 if name in tools else 0o644)):
                raise ValueError("Linux manifest file mismatch: " + name)
        sums = read("SHA256SUMS").decode("ascii").splitlines()
        if sorted(sums) != sorted(digest + "  " + name for name, digest in hashes.items()):
            raise ValueError("Linux internal checksum membership or digest mismatch")
    return {"edition": relative, "archive_sha256": digest,
            "scope": metadata["scope"], "platform": metadata["platform"],
            "files_checked": len(hashes), "tools_checked": sorted(tools)}


def check(root=ROOT):
    combined = check_combined_links(root)
    guide = (root / GUIDE).read_text()
    selected = re.findall(r"\[current macOS arm64 kit\]\(([^)]+/README\.md)\)", guide)
    if len(selected) != 1:
        raise ValueError("host guide must choose exactly one current kit")
    directory = ((root / GUIDE).parent / selected[0]).resolve().parent
    relative = directory.relative_to(root.resolve()).as_posix()
    if not re.fullmatch(r"dist/agent-install-kit-[A-Za-z0-9-]+", relative):
        raise ValueError("current kit must be a repository distribution")
    linux_links = re.findall(r"\[current Linux x86_64 kit\]\(([^)]+/README\.md)\)", guide)
    if len(linux_links) > 1:
        raise ValueError("host guide may choose only one current Linux kit")
    linux = None
    allowed = {directory.name}
    if linux_links:
        linux_directory = ((root / GUIDE).parent / linux_links[0]).resolve().parent
        linux_relative = linux_directory.relative_to(root.resolve()).as_posix()
        if (not re.fullmatch(r"dist/agent-install-kit-[A-Za-z0-9-]+", linux_relative)
                or linux_directory == directory):
            raise ValueError("Linux kit must be a distinct repository distribution")
        linux = check_linux(linux_directory, linux_relative)
        allowed.add(linux_directory.name)
    for name in (GUIDE, *ENTRY_POINTS):
        contents = (root / name).read_text()
        # These front-door pages may discuss past evidence records, but must
        # not direct installation to another archive edition.
        links = re.findall(r"(?:\.\./)*dist/(agent-install-kit-[A-Za-z0-9-]+)", contents)
        if links and any(link not in allowed for link in links):
            raise ValueError(name + ": advertised edition differs from host guide")
        if not links and "HOST-SETUP.md" not in contents:
            raise ValueError(name + ": no current guide or kit reference")
    archive_name = "agent-install-kit-macos-arm64.tar.gz"
    expected = re.findall(r"([0-9a-f]{64})  " + re.escape(archive_name),
                          (directory / "SHA256SUMS").read_text())
    if len(expected) != 1:
        raise ValueError("missing exact archive checksum")
    command = re.findall(r"printf [^\n]*? ([0-9a-f]{64}) \"\$KIT_ARCHIVE\"", guide)
    if command != expected:
        raise ValueError("guide archive verification does not match selected edition")
    archive = directory / archive_name
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if digest != expected[0]:
        raise ValueError("selected archive checksum mismatch")
    with tarfile.open(archive) as tar:
        members = tar.getmembers()
        if len(members) > 5000:
            raise ValueError("unexpected archive member count")
        prefix = "agent-install-kit-macos-arm64/"
        records = {}
        for member in members:
            if (not member.isfile() or not member.name.startswith(prefix)
                    or ".." in PurePosixPath(member.name).parts or member.name in records
                    or member.size > 64 * 1024 * 1024):
                raise ValueError("unexpected archive member")
            records[member.name] = member
        def read(name):
            item = records.get(prefix + name)
            if item is None:
                raise ValueError("missing kit file: " + name)
            return tar.extractfile(item).read()
        manifest = json.loads(read("provenance/kit.json"))["files"]
        profile_blocks = [json.loads(block) for block in re.findall(r"```json\n(.*?)\n```", guide, re.S)]
        profiles = [p for p in profile_blocks if p.get("scope") == "standing_local_software_installation_only"]
        if len(profiles) != 1:
            raise ValueError("guide must contain one selected installation profile")
        profile = profiles[0]
        if (set(profile) != {"schema_version", "scope", "staging_root", "installer", "validators"}
                or type(profile["schema_version"]) is not int or profile["schema_version"] != 1
                or not isinstance(profile["staging_root"], str)
                or not profile["staging_root"].startswith("/")
                or not isinstance(profile["validators"], dict)
                or set(profile["validators"]) != {"python"}):
            raise ValueError("basic host guide must enable its advertised Python profile")
        checked = []
        for item in [profile["installer"], *profile["validators"].values()]:
            name = item["path"].split(prefix, 1)[1]
            actual = hashlib.sha256(read(name)).hexdigest()
            if item["sha256"] != actual or manifest[name]["sha256"] != actual:
                raise ValueError("guide tool pin mismatch: " + name)
            checked.append(name)
        contents_digest = hashlib.sha256(read("verification-environment/CONTENTS.json")).hexdigest()
        bootstrap_pins = re.findall(r"--contents-sha256 ([0-9a-f]{64})", guide)
        if not bootstrap_pins or set(bootstrap_pins) != {contents_digest}:
            raise ValueError("guide bootstrap environment pin mismatch")
        refs = set(re.findall(r"\$KIT/([A-Za-z0-9_./-]+)", guide))
        refs.update(re.findall(r"`((?:contracts|bin|setup)/[A-Za-z0-9_./-]+)`", guide))
        for name in refs:
            if prefix + name not in records and not any(key.startswith(prefix + name + "/") for key in records):
                raise ValueError("guide references missing kit path: " + name)
    result = {"edition": relative, "archive_sha256": digest,
            "tool_pins_checked": checked, "bootstrap_sha256": contents_digest,
            "referenced_paths_checked": len(refs), "entry_points_checked": len(ENTRY_POINTS)}
    if linux is not None:
        result["linux"] = linux
    if combined is not None:
        result["combined_libraries"] = combined
    return result


if __name__ == "__main__":
    try:
        print(json.dumps(check(), sort_keys=True))
    except (ValueError, OSError, KeyError, IndexError, TypeError, tarfile.TarError) as error:
        print("installation-guide-check: " + str(error), file=sys.stderr)
        sys.exit(1)
