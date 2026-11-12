#!/usr/bin/env python3
"""Add the macOS executables one Connect release run built to its release.

connect-release.yml's build_macos job builds meerkat-connect and
toolkit_gateway for each macOS target on a hosted runner and uploads, as one
workflow artifact per target and attempt
(connect-macos-binaries-<target>-<attempt>):

  meerkat-connect-<version>-<target>.tar.gz          the meerkat-connect executable
  meerkat-connect-<version>-<target>.tar.gz.sha256
  meerkat-connect-gateway-<version>-<target>.tar.gz  the toolkit_gateway executable
  meerkat-connect-gateway-<version>-<target>.tar.gz.sha256
  connect-<target>-provenance.jsonl                  the build's attestation bundle

This command reads the downloaded artifacts (--download) and adds exactly
those files to the existing release of --tag:

  - every file is checked before the release is read: the newest attempt's
    artifact holds exactly the five names for --version, each archive hashes
    to its checksum file (sha256sum format, naming that archive) and holds
    exactly one regular member, the executable, and the bundle is not empty;
  - the release must exist for exactly that tag and must not be a draft; this
    command creates no release and no tag;
  - an asset already on the release is downloaded and compared: the same
    bytes are left, other bytes fail the run before anything is uploaded;
  - the missing files are uploaded with `gh release upload` and no
    `--clobber`, so nothing is ever replaced, and the release must list every
    name afterwards.

It never writes any other asset: SHA256SUMS and RELEASE-MANIFEST.json are the
release owner's files.

Usage (GH_TOKEN and GITHUB_REPOSITORY from the workflow):
  connect_release_macos_assets.py --tag connect-vX.Y.Z --version X.Y.Z \
      --download DIR --work NEW_DIR
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tarfile
from pathlib import Path

# The macOS targets the release workflow builds (its build_macos matrix).
TARGETS = ("aarch64-apple-darwin",)
# Archive name prefix and the one executable each archive holds.
ARCHIVES = (("meerkat-connect", "meerkat-connect"), ("meerkat-connect-gateway", "toolkit_gateway"))
ARTIFACT_PREFIX = "connect-macos-binaries"
# The release's unstripped Linux executables are below 600 MB; nothing this
# command adds may reach GitHub's 2 GiB asset limit.
ASSET_LIMIT = 2 * 1024 * 1024 * 1024


class Refused(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise Refused(message)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def expected_names(version: str, target: str) -> list[str]:
    """The five files of one target, in upload order."""
    names = []
    for prefix, _ in ARCHIVES:
        archive = f"{prefix}-{version}-{target}.tar.gz"
        names += [archive, f"{archive}.sha256"]
    return [*names, f"connect-{target}-provenance.jsonl"]


def newest_attempt(download: Path, target: str) -> Path:
    attempts = {}
    marker = f"{ARTIFACT_PREFIX}-{target}-"
    for entry in download.iterdir() if download.is_dir() else ():
        suffix = entry.name[len(marker):] if entry.name.startswith(marker) else ""
        if entry.is_dir() and re.fullmatch(r"[1-9][0-9]*", suffix):
            attempts[int(suffix)] = entry
    require(attempts, f"no built macOS archives for {target}: {download} holds no {marker}<attempt> artifact")
    return attempts[max(attempts)]


def check_archive(path: Path, member: str):
    """The archive holds exactly one regular file, the executable, at its root."""
    try:
        with tarfile.open(path, "r:gz") as archive:
            members = archive.getmembers()
    except (tarfile.TarError, OSError, EOFError) as error:
        raise Refused(f"{path.name} is not a readable gzip tar archive: {error}") from error
    require(len(members) == 1, f"{path.name} must hold exactly one member, the {member} executable; "
                               f"it holds {[item.name for item in members]}")
    only = members[0]
    require(only.name == member and only.isfile() and only.size > 0 and only.mode & 0o111,
            f"{path.name} must hold the executable {member}; it holds {only.name!r}")


def checked_files(download: Path, version: str) -> list[Path]:
    """Every file to add, checked, in upload order."""
    files = []
    for target in TARGETS:
        folder = newest_attempt(download, target)
        names = expected_names(version, target)
        found = sorted(entry.name for entry in folder.iterdir())
        for name in names:
            require(name in found, f"{folder.name} has no {name}")
        extra = sorted(set(found) - set(names))
        require(not extra, f"{folder.name} holds files this release does not expect: {extra}")
        for name in names:
            path = folder / name
            require(path.is_file() and not path.is_symlink(), f"{folder.name}/{name} is not a regular file")
            size = path.stat().st_size
            require(size > 0, f"{folder.name}/{name} is empty")
            require(size < ASSET_LIMIT, f"{folder.name}/{name} is {size} bytes, over the release asset limit")
        for prefix, member in ARCHIVES:
            archive = folder / f"{prefix}-{version}-{target}.tar.gz"
            recorded = (folder / f"{archive.name}.sha256").read_text(encoding="utf-8", errors="replace")
            wanted = f"{sha256(archive)}  {archive.name}\n"
            require(recorded == wanted,
                    f"{archive.name} does not match its checksum file: expected the line {wanted.strip()!r}, "
                    f"the file holds {recorded.strip()!r}")
            check_archive(archive, member)
        files += [folder / name for name in names]
    return files


def gh(repository: str, *arguments: str, capture=False):
    return subprocess.run(["gh", *arguments, "--repo", repository], check=False, text=True,
                          stdout=subprocess.PIPE if capture else None, stderr=subprocess.PIPE)


def release_assets(repository: str, tag: str) -> set[str]:
    """The asset names of the published release of exactly `tag`."""
    viewed = gh(repository, "release", "view", tag, "--json", "tagName,isDraft,assets", capture=True)
    require(viewed.returncode == 0,
            f"no release for {tag} in {repository} ({viewed.stderr.strip() or 'gh release view failed'}); "
            "this command creates no release")
    try:
        release = json.loads(viewed.stdout)
        names = {asset["name"] for asset in release["assets"]}
        found_tag, draft = release["tagName"], release["isDraft"]
    except (ValueError, KeyError, TypeError) as error:
        raise Refused(f"unreadable release record for {tag}: {error}") from error
    require(found_tag == tag, f"the release found for {tag} is the release of {found_tag}")
    require(draft is False, f"the release of {tag} is a draft; assets are added to a published release only")
    return names


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Add one run's macOS executables to its Connect release.")
    parser.add_argument("--tag", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--download", required=True, type=Path)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", ""))
    args = parser.parse_args(argv)
    try:
        require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", args.version), f"not a release version: {args.version!r}")
        require(args.tag == f"connect-v{args.version}",
                f"tag {args.tag} does not name the Connect version {args.version} (connect-v{args.version})")
        require(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repo), "no repository (GITHUB_REPOSITORY)")
        files = checked_files(args.download, args.version)

        present = release_assets(args.repo, args.tag)
        require(not args.work.exists(), f"--work {args.work} already exists; name a new directory")
        args.work.mkdir(parents=True)
        missing, conflicts = [], []
        for path in files:
            if path.name not in present:
                missing.append(path)
                continue
            fetched = gh(args.repo, "release", "download", args.tag, "--pattern", path.name, "--dir", str(args.work))
            require(fetched.returncode == 0 and (args.work / path.name).is_file(),
                    f"cannot read back the existing asset {path.name}: {fetched.stderr.strip()}")
            if sha256(args.work / path.name) == sha256(path):
                print(f"{path.name}: already on the release with the same bytes; left")
            else:
                conflicts.append(path.name)
        require(not conflicts,
                f"the release of {args.tag} already holds other bytes under {conflicts}; "
                "an asset is never replaced, and nothing was uploaded")

        for path in missing:
            uploaded = gh(args.repo, "release", "upload", args.tag, str(path))
            require(uploaded.returncode == 0, f"upload of {path.name} failed: {uploaded.stderr.strip()}")
            print(f"{path.name}: added ({path.stat().st_size} bytes, sha256 {sha256(path)})")

        listed = release_assets(args.repo, args.tag)
        unlisted = [path.name for path in files if path.name not in listed]
        require(not unlisted, f"the release of {args.tag} does not list {unlisted} after the upload")
        print(f"{args.tag}: {len(missing)} added, {len(files) - len(missing)} already there; "
              f"{len(files)} macOS files on the release")
        return 0
    except Refused as refusal:
        print(f"::error::{refusal}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
