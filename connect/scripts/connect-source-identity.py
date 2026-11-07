#!/usr/bin/env python3
"""Record or check the exact monorepo source of a direct Connect production.

Connect is the connect/ product of lukacf/meerkat and builds in the
repository's one Cargo workspace. A direct (retained-host) production builds
each target from one clean checkout of one exact commit. `record` writes the
source manifest that the build record names; `check` derives the manifest
again from the checkout it runs in and refuses any byte of difference.

The full commit and its full tree are the identity. Under them the manifest
binds every tracked connect/ file (path, Git mode, size, SHA-256), the root
workspace files Cargo reads for every product build, and the connect/ export:
the commit that imported Connect's exported source into connect/ (its message
records the export mapping), which must be in the selected commit's history,
and Connect's export provenance records. The manifest is a pure function of
the two commits, so every target's build record names the same bytes.

  python3 -I scripts/connect-source-identity.py record --commit SHA --export-commit SHA --output ABS
  python3 -I scripts/connect-source-identity.py check --manifest ABS --manifest-sha256 HEX
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess

ROOT = Path(__file__).resolve().parents[1]
WORKSPACE = ROOT.parent
REPOSITORY = "lukacf/meerkat"
KIND = "meerkat_connect_monorepo_source"
PRODUCT_ROOT = "connect"
# Root files every product build reads: the workspace manifest and lock, the
# toolchain selection and the repository's Cargo configuration ([env]).
WORKSPACE_INPUTS = ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml")
# connect/ records that tie this tree to Connect's exported source history.
EXPORT_RECORDS = ("SOURCE-PROVENANCE.md", "PUBLIC-SOURCE-COMMITS.json")
KEYS = {"schema_version", "kind", "repository", "revision", "tree", "product_root",
        "workspace", "export", "files"}
SHA = re.compile(r"[0-9a-f]{40}\Z")
HEX = re.compile(r"[0-9a-f]{64}\Z")
GIT_MODES = {"100644", "100755"}
LIMIT = 32 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args], text=True).strip()


def checkout_identity():
    """The clean checkout's full commit and tree; connect/ must be its product directory."""
    require(Path(git("rev-parse", "--show-toplevel")).resolve() == WORKSPACE,
            "Connect is not the connect/ product of this checkout")
    require(git("status", "--porcelain", "--untracked-files=normal") == "", "source checkout is dirty")
    return git("rev-parse", "--verify", "HEAD^{commit}"), git("rev-parse", "--verify", "HEAD^{tree}")


def regular(path, name):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode), "source census requires regular tracked files: " + name)
    return {"sha256": digest(path), "size": info.st_size}


def census():
    rows = []
    listing = subprocess.check_output(["git", "-C", str(ROOT), "ls-files", "-s", "-z"]).decode()
    for entry in filter(None, listing.split("\0")):
        header, name = entry.split("\t", 1)
        mode = header.split(" ", 1)[0]
        require(mode in GIT_MODES and PurePosixPath(name).as_posix() == name and ".." not in PurePosixPath(name).parts,
                "source census requires plain regular tracked files: " + name)
        rows.append({"path": name, "git_mode": mode, **regular(ROOT / name, name)})
    require(rows, "empty connect/ source census")
    return sorted(rows, key=lambda row: row["path"])


def checked_export_commit(commit):
    """The connect/ export commit: a full commit in the selected commit's history."""
    require(isinstance(commit, str) and SHA.fullmatch(commit) is not None, "explicit 40-hex export commit required")
    def succeeds(*args):
        return subprocess.run(["git", "-C", str(ROOT), *args], capture_output=True, text=True).returncode == 0
    require(succeeds("cat-file", "-e", commit + "^{commit}"), "export commit is not in this checkout")
    require(succeeds("merge-base", "--is-ancestor", commit, "HEAD"),
            "export commit is not in the selected commit's history")
    return commit


def manifest(revision, tree, export_commit):
    """The canonical source manifest of this checkout at revision and tree."""
    return {"schema_version": 1, "kind": KIND, "repository": REPOSITORY,
            "revision": revision, "tree": tree, "product_root": PRODUCT_ROOT,
            "workspace": {name: regular(WORKSPACE / name, name) for name in WORKSPACE_INPUTS},
            "export": {"commit": checked_export_commit(export_commit),
                       "records": {name: regular(ROOT / name, name) for name in EXPORT_RECORDS}},
            "files": census()}


def serialized(value):
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True, allow_nan=False) + "\n").encode("ascii")


def unique(pairs):
    value = {}
    for key, item in pairs:
        require(key not in value, "duplicate source manifest key")
        value[key] = item
    return value


def checked_source(path, expected_sha256):
    """Refuse unless path holds exactly the manifest this clean checkout derives."""
    require(isinstance(expected_sha256, str) and HEX.fullmatch(expected_sha256) is not None,
            "exact source manifest SHA-256 required")
    require(path.is_absolute() and not path.is_symlink() and path.is_file()
            and path.stat().st_size <= LIMIT, "bounded regular absolute source manifest required")
    raw = path.read_bytes()
    require(len(raw) <= LIMIT and hashlib.sha256(raw).hexdigest() == expected_sha256, "source manifest pin mismatch")
    value = json.loads(raw, object_pairs_hook=unique)
    require(isinstance(value, dict) and set(value) == KEYS and value["schema_version"] == 1
            and value["kind"] == KIND and value["repository"] == REPOSITORY
            and value["product_root"] == PRODUCT_ROOT
            and isinstance(value["revision"], str) and SHA.fullmatch(value["revision"]) is not None
            and isinstance(value["tree"], str) and SHA.fullmatch(value["tree"]) is not None,
            "not a lukacf/meerkat Connect source manifest")
    revision, tree = checkout_identity()
    require(value["revision"] == revision and value["tree"] == tree,
            "source manifest names another commit than this checkout")
    export = value["export"]
    require(isinstance(export, dict) and set(export) == {"commit", "records"}, "invalid connect/ export identity")
    require(raw == serialized(manifest(revision, tree, export["commit"])),
            "source manifest differs from this checkout's source")
    return value


def record(commit, export_commit, output):
    require(isinstance(commit, str) and SHA.fullmatch(commit) is not None, "explicit 40-hex commit required")
    revision, tree = checkout_identity()
    require(revision == commit, "checkout HEAD is not the selected commit")
    require(output.is_absolute() and not output.exists() and output.parent.is_dir()
            and output.parent.resolve(strict=True) == output.parent, "fresh absolute output in a canonical directory required")
    data = serialized(manifest(revision, tree, export_commit))
    with output.open("xb") as stream:
        stream.write(data)
    output.chmod(0o644)
    print(json.dumps({"path": str(output), "sha256": hashlib.sha256(data).hexdigest(), "size": len(data),
                      "revision": revision, "tree": tree}, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("record", "check"))
    parser.add_argument("--commit")
    parser.add_argument("--export-commit")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--manifest-sha256")
    args = parser.parse_args()
    if args.command == "record":
        require(args.commit and args.export_commit and args.output, "--commit, --export-commit and --output required")
        record(args.commit, args.export_commit, args.output)
    else:
        require(args.manifest and args.manifest_sha256, "--manifest and --manifest-sha256 required")
        value = checked_source(args.manifest, args.manifest_sha256)
        print(json.dumps({"revision": value["revision"], "tree": value["tree"], "status": "verified"}, sort_keys=True))


if __name__ == "__main__":
    main()
