#!/usr/bin/env python3
"""Compare the resource packager's public pins with their publishers' documents.

The packager carries a pin (file name, sha256, size) for each public runtime
input. This check reads what the publishers say: python-build-standalone's
SHA256SUMS of the pinned release, nodejs.org's SHASUMS256.txt of the pinned
version, and the npm registry's document of the pinned npm version (its
dist.integrity, a sha512). The pip zipapp has no publisher checksum; its pin
is reported as our own hash. With --pins the committed pin file is compared
with the same pins, and with --pinned-inputs the fetched files are hashed.

Only --fetch uses the network, and only for the three small documents. No
fetched byte is executed.
"""
from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import sys
import urllib.request

SPEC = importlib.util.spec_from_file_location(
    "connect_resource_package_pins", Path(__file__).with_name("package-connect-resources.py"))
PACKAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGE)
DOCUMENT_LIMIT = 4 * 1024 * 1024
ROLES = ("python", "node", "npm")
PASSING = ("equal", "own hash: no publisher checksum")


def releases(target):
    """The packager's pin rows and the releases their file names say they are."""
    rows = PACKAGE.pinned_rows(target)
    python = re.fullmatch(r"cpython-[0-9.]+\+([0-9]{8})-[A-Za-z0-9_+.-]+\.tar\.zst", rows["python"][1])
    node = re.fullmatch(r"node-(v[0-9]+\.[0-9]+\.[0-9]+)-[a-z0-9-]+\.tar\.gz", rows["node"][1])
    npm = re.fullmatch(r"npm-([0-9]+\.[0-9]+\.[0-9]+)\.tgz", rows["npm"][1])
    if python is None or node is None or npm is None:
        raise ValueError("a pinned file name does not name its release")
    return rows, {"python": python[1], "node": node[1], "npm": npm[1]}


def document_names(target):
    return {"python": "SHA256SUMS", "node": "SHASUMS256.txt", "npm": "npm-" + releases(target)[1]["npm"] + ".json"}


def document_urls(target):
    release = releases(target)[1]
    return {
        "python": "https://github.com/astral-sh/python-build-standalone/releases/download/"
                  + release["python"] + "/SHA256SUMS",
        "node": "https://nodejs.org/dist/" + release["node"] + "/SHASUMS256.txt",
        "npm": "https://registry.npmjs.org/npm/" + release["npm"],
    }


def fetch(target, documents):
    """Download the three publisher documents into a new directory."""
    names, urls = document_names(target), document_urls(target)
    documents.mkdir()
    for role in ROLES:
        request = urllib.request.Request(
            urls[role], headers={"User-Agent": "connect-resource-pin-check/1", "Accept": "*/*"})
        with urllib.request.urlopen(request, timeout=60) as response:
            raw = response.read(DOCUMENT_LIMIT + 1)
        if not raw or len(raw) > DOCUMENT_LIMIT:
            raise ValueError("publisher document is empty or too large")
        with (documents / names[role]).open("xb") as output:
            output.write(raw)


def document(path):
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= DOCUMENT_LIMIT:
            raise ValueError("invalid publisher document")
        return source.read(DOCUMENT_LIMIT + 1)


def listed_sha256(raw, name):
    """The sha256 a checksum file gives one file name, or None."""
    found = set()
    for line in raw.decode("utf-8").splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[1].lstrip("*") == name and re.fullmatch(r"[0-9a-f]{64}", parts[0]):
            found.add(parts[0])
    if len(found) > 1:
        raise ValueError("checksum file gives one file two digests")
    return found.pop() if found else None


def registry_sha512(raw, name, version):
    """The registry's dist.integrity for this exact archive as hex, or None."""
    value = json.loads(raw, object_pairs_hook=PACKAGE.unique_object)
    dist = value.get("dist") if isinstance(value, dict) else None
    if (not isinstance(dist, dict) or value.get("name") != "npm" or value.get("version") != version
            or dist.get("tarball") != "https://registry.npmjs.org/npm/-/" + name):
        raise ValueError("registry document is not the pinned npm release")
    integrity = dist.get("integrity")
    if not isinstance(integrity, str) or not integrity.startswith("sha512-"):
        return None
    try:
        decoded = base64.b64decode(integrity.removeprefix("sha512-"), validate=True)
    except binascii.Error:
        return None
    return decoded.hex() if len(decoded) == 64 else None


def fetched(path, sha256, size, sha512):
    """Hash one fetched file; a link or a missing file is not a fetched file."""
    try:
        source = os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb")
    except OSError:
        return "missing"
    with source:
        if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
            return "missing"
        first, second, total = hashlib.sha256(), hashlib.sha512(), 0
        while block := source.read(1024 * 1024):
            total += len(block)
            first.update(block)
            second.update(block)
    equal = total == size and first.hexdigest() == sha256 and sha512 in (None, second.hexdigest())
    return "equal" if equal else "differs"


def check(target, documents, pins=None, pinned_inputs=None):
    rows, release = releases(target)
    names = document_names(target)
    checks = []
    for role in ("python", "node"):
        published = listed_sha256(document(documents / names[role]), rows[role][1])
        checks.append({"subject": role, "against": "publisher checksum file", "file": rows[role][1],
                       "document": names[role], "pin": rows[role][2], "published": published,
                       "result": "missing" if published is None else
                                 "equal" if published == rows[role][2] else "differs"})
    published = registry_sha512(document(documents / names["npm"]), rows["npm"][1], release["npm"])
    checks.append({"subject": "npm", "against": "registry integrity", "file": rows["npm"][1],
                   "document": names["npm"], "pin": rows["npm"][4], "published": published,
                   "result": "missing" if published is None else
                             "equal" if published == rows["npm"][4] else "differs"})
    checks.append({"subject": "pip", "against": "publisher", "file": rows["pip"][1], "pin": rows["pip"][2],
                   "result": "own hash: no publisher checksum"})
    if pins is not None:
        row = {"subject": "pin file", "against": "packager pins", "file": str(pins)}
        try:
            row.update(result="equal", **PACKAGE.checked_pin_file(target, pins))
        except ValueError as error:
            row.update(result="differs", reason=str(error))
        checks.append(row)
    if pinned_inputs is not None:
        for role, (kind, name, sha256, size, sha512) in rows.items():
            checks.append({"subject": role, "against": "fetched bytes", "file": kind + "/" + name,
                           "result": fetched(pinned_inputs / kind / name, sha256, size, sha512)})
    return {"target": target, "ok": all(row["result"] in PASSING for row in checks), "checks": checks}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], allow_abbrev=False)
    parser.add_argument("--target", choices=PACKAGE.PINNED_TARGETS, default=PACKAGE.PINNED_TARGETS[0])
    parser.add_argument("--documents", type=Path, required=True,
                        help="directory of the three publisher documents; with --fetch a NEW directory")
    parser.add_argument("--fetch", action="store_true", help="download the three publisher documents first")
    parser.add_argument("--pins", type=Path, help="also compare this committed pin file with the packager's pins")
    parser.add_argument("--pinned-inputs", type=Path, help="also hash the fetched files under this KIND/FILE directory")
    args = parser.parse_args(argv)
    if args.fetch:
        fetch(args.target, args.documents)
    # Absolute without following a link: the pin file itself must not be one.
    pins = None if args.pins is None else Path(os.path.abspath(args.pins))
    report = check(args.target, args.documents, pins=pins, pinned_inputs=args.pinned_inputs)
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
