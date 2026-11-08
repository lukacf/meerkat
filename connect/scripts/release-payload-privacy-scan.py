#!/usr/bin/env python3
"""Scan release payload files for private text through every nested container, at any depth.

One engine does all matching and all container checks: release-evidence-privacy-scan.py beside
this file, unchanged and pinned by sha256 below. The engine decodes nested containers 4 levels
deep, does not unpack zstd, and reads no member or stream larger than 1 GiB. This wrapper walks
each payload file with the engine's own traversal (its Scanner, with matching switched off), so it
sees exactly the containers and locations the engine sees, and gives the engine what it would not
enter as extra artifacts of the same engine run, written to a private work directory and scanned
from level 0 (the walk repeats inside them, at any depth):
  - a container nested deeper than the engine's 4 levels: its bytes;
  - a zstd container: its decompressed bytes;
  - a gzip, bzip2 or xz stream that decompresses past the engine's bound and holds a tar (a large
    product archive): the tar, cut at member boundaries into pieces below the bound. Each piece is
    the original header and data bytes of whole members plus an end-of-archive marker; the pieces
    together are every byte of the tar up to its end, and what follows must be zero bytes.

  python3 -I release-payload-privacy-scan.py --rules RULES --exemptions EXEMPTIONS \\
      [--accepted-findings ACCEPTED] --report REPORT --detail DETAIL [--work DIR] [--zstd ZSTD] PAYLOAD...

The rules and exemptions are private and never committed (tests use synthetic ones). The run
refuses without them.

Accepted findings (optional and private, kept beside the rules by the release owner; without the
file nothing is accepted), schema release-payload-privacy-scan/accepted/1:
  {"schema": "release-payload-privacy-scan/accepted/1",
   "accepted": [{"category": C, "rule": R, "sha256": H, "reason": "one line"}]}
  - A hit of category C is accepted only when every rule behind it is listed with C and a sha256
    H of a file that holds the hit: the file or member it is in, or a container that encloses it.
  - A structural error (category "structure", rule = the engine's error kind: archive-layout,
    corrupt-archive, unsupported-archive or malformed-json) is accepted only for the exact
    container it was reported in.
  - Nothing else is ever accepted: a member over the engine's size bound (other than a tar stream
    this wrapper cuts into pieces), an input that is missing, unreadable or not a regular file,
    and a zstd container or limit this wrapper could not handle always refuse.

Exit status (bit flags, as the engine's): 0 clean; 1 a refused hit; 2 coverage incomplete (a
refused error, a container this wrapper could not handle, a limit); 3 both; 4 usage or
configuration (a missing rules or exemptions file, a pin mismatch, a malformed accepted-findings
file, an engine usage error). The report (--report, a new file) holds counts only. The engine's
detail (--detail, a new file, mode 0600) holds the matched strings: it is private, never upload it.
Standard library only, no network.
"""
from __future__ import annotations

import argparse
import bz2
import gzip
import hashlib
import io
import json
import lzma
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import types
from pathlib import Path

HERE = Path(__file__).resolve().parent
ENGINE = HERE / "release-evidence-privacy-scan.py"
# The vendored engine, byte for byte.
ENGINE_SHA256 = "9405cacc8e3f4d7e0aea5a1ec2087bf50565425d9dbb5e9d5b199a5366f19485"
SCHEMA_REPORT = "release-payload-privacy-scan/report/1"
SCHEMA_ACCEPTED = "release-payload-privacy-scan/accepted/1"
STRUCTURE = "structure"
ACCEPTABLE_ERRORS = ("archive-layout", "corrupt-archive", "unsupported-archive", "malformed-json")
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
EXIT_HITS, EXIT_COVERAGE, EXIT_USAGE = 1, 2, 4
TAR_BLOCK = 512
STREAM_OPENERS = {"gzip": gzip.GzipFile, "bzip2": bz2.BZ2File, "xz": lzma.LZMAFile}


class Usage(Exception):
    pass


def split_tar(path: Path, piece_bytes: int, out_prefix: Path) -> tuple[list[Path], bool]:
    """Cut the tar at path into pieces at member boundaries: each piece is the original bytes from the
    end of the previous piece through the end of whole members (header, extension and data blocks),
    followed by an end-of-archive marker, and holds at most piece_bytes of original bytes unless one
    member alone is larger. Returns the piece paths and whether everything after the last member is
    zero bytes. Raises tarfile.TarError, OSError or ValueError for a tar it cannot read."""
    ends = []
    with tarfile.open(path, "r:") as archive:
        for member in archive:
            data_blocks = -(-member.size // TAR_BLOCK) * TAR_BLOCK
            if member.isreg() or member.type not in tarfile.SUPPORTED_TYPES:
                ends.append(member.offset_data + data_blocks)
            else:
                ends.append(member.offset_data)
    if not ends:
        raise ValueError("a tar without members")
    cuts, start, previous = [], 0, None
    for end in ends:
        if previous is not None and end - start > piece_bytes:
            cuts.append((start, previous))
            start = previous
        previous = end
    cuts.append((start, previous))
    pieces = []
    with open(path, "rb") as source:
        for index, (begin, end) in enumerate(cuts):
            piece = out_prefix.with_name(f"{out_prefix.name}.piece{index + 1:03d}.tar")
            source.seek(begin)
            with open(piece, "xb") as target:
                remaining = end - begin
                while remaining:
                    block = source.read(min(remaining, 1 << 20))
                    if not block:
                        raise ValueError("the tar is shorter than its members")
                    target.write(block)
                    remaining -= len(block)
                target.write(b"\x00" * (2 * TAR_BLOCK))
            os.chmod(piece, 0o600)
            pieces.append(piece)
        source.seek(previous)
        zero_tail = True
        while block := source.read(1 << 20):
            if block.count(0) != len(block):
                zero_tail = False
                break
    return pieces, zero_tail


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def load_engine():
    """The pinned engine as a module, compiled from the exact bytes that were hashed."""
    raw = ENGINE.read_bytes()
    if hashlib.sha256(raw).hexdigest() != ENGINE_SHA256:
        raise Usage("the vendored engine differs from its pinned sha256")
    module = types.ModuleType("release_evidence_privacy_scan")
    module.__file__ = str(ENGINE)
    exec(compile(raw, str(ENGINE), "exec"), module.__dict__)
    return module


def read_accepted(path: Path | None) -> dict:
    """{(category, rule): {sha256, ...}}; an absent file accepts nothing."""
    accepted: dict = {}
    if path is None:
        return accepted
    try:
        value = json.loads(path.read_bytes())
    except (OSError, ValueError) as error:
        raise Usage(f"the accepted-findings file cannot be read: {type(error).__name__}") from None
    if not isinstance(value, dict) or set(value) != {"schema", "accepted"} or value["schema"] != SCHEMA_ACCEPTED \
            or not isinstance(value["accepted"], list):
        raise Usage(f"the accepted-findings file is not a {SCHEMA_ACCEPTED} document")
    for entry in value["accepted"]:
        if not isinstance(entry, dict) or set(entry) != {"category", "rule", "sha256", "reason"} \
                or not all(isinstance(entry[key], str) and entry[key] for key in entry) \
                or not HEX64.fullmatch(entry["sha256"]) or not entry["reason"].strip() or "\n" in entry["reason"] \
                or (entry["category"] == STRUCTURE and entry["rule"] not in ACCEPTABLE_ERRORS):
            raise Usage("an accepted finding needs category, rule, an exact sha256 and a one-line reason")
        accepted.setdefault((entry["category"], entry["rule"]), set()).add(entry["sha256"])
    return accepted


def zstd_decompress(data: bytes, zstd_cli: str | None, limit: int) -> bytes:
    try:
        from compression import zstd  # Python 3.14 and later
    except ImportError:
        zstd = None
    if zstd is not None:
        content = zstd.decompress(data)
    elif not zstd_cli:
        raise RuntimeError("no zstd decoder")
    else:
        done = subprocess.run([zstd_cli, "-d", "-c", "-q"], input=data, stdout=subprocess.PIPE,
                              stderr=subprocess.DEVNULL, check=False)
        if done.returncode != 0:
            raise RuntimeError("zstd decoding failed")
        content = done.stdout
    if len(content) > limit:
        raise RuntimeError("zstd content over the size bound")
    return content


def walker_class(engine):
    walk_rules = engine.Rules(
        {"schema": engine.SCHEMA_RULES, "categories": {"walk": {"rules": [
            {"id": "walk", "kind": "literal", "value": "\x00walk-only\x00"}]}}},
        {"schema": engine.SCHEMA_EXEMPTIONS, "exemptions": []})

    class Walker(engine.Scanner):
        """The engine's own traversal with matching switched off: it records each location's bytes
        (sha256), which locations are containers, and re-roots what the engine would not enter."""

        def __init__(self, work: Path, zstd_cli: str | None, max_reroots: int, max_bytes: int, max_nesting: int,
                     piece_bytes: int):
            super().__init__(walk_rules)
            self.work, self.zstd_cli = work, zstd_cli
            self.max_reroots, self.max_bytes, self.max_nesting = max_reroots, max_bytes, max_nesting
            self.piece_bytes = piece_bytes
            self.digests: dict[str, str] = {}
            self.container_locations: set[str] = set()
            self.reroots: dict[str, tuple[str, list[str]]] = {}
            self.origins: dict[str, str] = {}
            self.nesting: dict[str, int] = {}
            self.artifacts: list[str] = []
            self.failures: list[str] = []
            self.reroot_bytes = 0
            self.counter = 0
            self.roots: list[str] = []
            self.root_paths: set[str] = set()

        def scan_string(self, text, location, where):  # no matching during the walk
            return 0

        def walk(self, index: int, path: str, nesting: int) -> None:
            self.nesting[path] = nesting
            self.root_paths.add(path)
            self.roots.append(path)
            try:
                self.scan_artifact(index, path)
            finally:
                self.roots.pop()

        def scan_content(self, data, name, location, depth, required=False):
            self.digests.setdefault(location, hashlib.sha256(data).hexdigest())
            kind = engine.archive_kind(data)
            if kind is None:
                return
            if kind == "opaque-zstd":
                self.reroot(location, "zstd", name, lambda: [self.write(name, ".zstd-decompressed", zstd_decompress(
                    data, self.zstd_cli, engine.MAX_MEMBER_BYTES))])
                return
            if kind.startswith("opaque-"):
                return  # the engine reports it as unsupported; it cannot be decoded here either
            self.container_locations.add(location)
            if depth >= engine.MAX_DEPTH:
                self.reroot(location, "depth", name, lambda: [self.write(name, "", data)])
                return
            seen = len(self.detail_errors)
            super().scan_content(data, name, location, depth, required)
            if kind in STREAM_OPENERS and any(error["kind"] == "member-too-large" and error["location"] == location
                                              for error in self.detail_errors[seen:]):
                self.reroot(location, "split", name, lambda: self.split(kind, data, name))

        def write(self, name: str, suffix: str, data: bytes) -> Path:
            self.reroot_bytes += len(data)
            if self.reroot_bytes > self.max_bytes:
                raise RuntimeError("re-root byte limit")
            path = self.new_path(name, suffix)
            with open(path, "xb") as stream:
                stream.write(data)
            os.chmod(path, 0o600)
            return path

        def new_path(self, name: str, suffix: str) -> Path:
            self.counter += 1
            base = re.sub(r"[^A-Za-z0-9._-]", "_", name.rsplit("/", 1)[-1])[-80:] or "member"
            if suffix == ".zstd-decompressed":
                base = re.sub(r"\.(zst|zstd)$", "", base)
            return self.work / f"{self.counter:05d}-{base}{suffix}"

        def split(self, kind: str, data: bytes, name: str) -> list[Path]:
            """A stream past the engine's bound that holds a tar: decompress it to the work directory
            and cut the tar into pieces at member boundaries; anything else is not handled."""
            tar_path = self.new_path(name, ".decompressed")
            total = 0
            with STREAM_OPENERS[kind](fileobj=io.BytesIO(data)) as source, open(tar_path, "xb") as target:
                while block := source.read(1 << 20):
                    total += len(block)
                    if self.reroot_bytes + total > self.max_bytes:
                        raise RuntimeError("re-root byte limit")
                    target.write(block)
            with open(tar_path, "rb") as stream:
                head = stream.read(engine.TAR_BLOCK)
            if len(head) < 262 or head[257:262] != b"ustar":
                tar_path.unlink()
                raise RuntimeError("a stream past the size bound that is not a tar")
            pieces, zero_tail = split_tar(tar_path, self.piece_bytes, tar_path)
            tar_path.unlink()
            if not zero_tail:
                raise RuntimeError("non-zero bytes after the last member of a tar past the size bound")
            self.reroot_bytes += sum(piece.stat().st_size for piece in pieces)
            return pieces

        def reroot(self, location: str, reason: str, name: str, produce) -> None:
            if location in self.reroots:
                return
            nesting = self.nesting[self.roots[-1]] + 1
            if len(self.artifacts) >= self.max_reroots or nesting > self.max_nesting:
                self.failures.append("re-root limit")
                return
            try:
                paths = produce()
            except Exception as error:  # fail closed: counted as a container that was not handled
                self.failures.append(f"{reason}: {type(error).__name__}")
                return
            self.reroots[location] = (reason, [str(path) for path in paths])
            for path in paths:
                self.origins[str(path)] = location
                self.artifacts.append(str(path))
            for path in paths:
                self.walk(10**6 + self.artifacts.index(str(path)), str(path), nesting)

    return Walker


def prefixes(location: str):
    """location, then each of its '!'-separated ancestors, longest first."""
    yield location
    position = len(location)
    while (position := location.rfind("!", 0, position)) > 0:
        yield location[:position]


def classify(walker, detail: dict, accepted: dict) -> dict:
    roots = walker.root_paths

    def chain(location: str) -> set[str]:
        found: set[str] = set()
        while True:
            root = None
            for prefix in prefixes(location):
                if prefix in walker.digests:
                    found.add(walker.digests[prefix])
                if prefix in roots:
                    root = prefix
            if root is None or root not in walker.origins:
                return found
            location = walker.origins[root]

    def container_digest(location: str) -> str | None:
        for prefix in prefixes(location):
            if prefix in walker.container_locations:
                return walker.digests.get(prefix)
        return None

    zstd_unsupported = {c["location"] for c in detail["containers"]
                        if c["kind"] == "zstd" and c["outcome"] == "unsupported"}
    counts = {"hits": {"accepted": {}, "refused": {}},
              "errors": {"covered": {}, "accepted": {}, "refused": {}}}

    def add(group: dict, key: str, n: int) -> None:
        group[key] = group.get(key, 0) + n

    for hit in detail["hits"]:
        digests = chain(hit["location"])
        ok = bool(hit["rules"]) and all(accepted.get((hit["category"], rule), set()) & digests for rule in hit["rules"])
        add(counts["hits"]["accepted" if ok else "refused"], hit["category"], int(hit["count"]))
    for error in detail["errors"]:
        kind, location = error["kind"], error["location"]
        reason = walker.reroots.get(location, (None, []))[0]
        if (kind, reason) in (("depth-limit", "depth"), ("member-too-large", "split")) or (
                kind == "unsupported-archive" and reason == "zstd" and location in zstd_unsupported):
            add(counts["errors"]["covered"], kind, 1)
        elif kind in ACCEPTABLE_ERRORS and container_digest(location) in accepted.get((STRUCTURE, kind), set()):
            add(counts["errors"]["accepted"], kind, 1)
        else:
            add(counts["errors"]["refused"], kind, 1)
    return counts


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--rules", type=Path, help="the private rules file (required)")
    parser.add_argument("--exemptions", type=Path, help="the private exemptions file (required)")
    parser.add_argument("--accepted-findings", type=Path,
                        help="the private accepted-findings file (optional; without it nothing is accepted)")
    parser.add_argument("--report", type=Path, required=True, help="counts-only report (a new file)")
    parser.add_argument("--detail", type=Path, required=True, help="the engine's private detail (a new file, 0600)")
    parser.add_argument("--work", type=Path, help="parent of the private work directory")
    parser.add_argument("--keep-work", action="store_true")
    parser.add_argument("--zstd", help="a zstd executable, used when Python has no zstd decoder")
    parser.add_argument("--max-reroots", type=int, default=100000)
    parser.add_argument("--max-reroot-bytes", type=int, default=64 << 30)
    parser.add_argument("--max-nesting", type=int, default=64)
    parser.add_argument("--piece-bytes", type=int, default=256 << 20,
                        help="the target size of the pieces a tar past the engine's bound is cut into")
    parser.add_argument("payload", nargs="*", type=Path)
    args = parser.parse_args()
    try:
        if not 0 < args.piece_bytes < (1 << 30):
            raise Usage("--piece-bytes must be positive and below the engine's 1 GiB bound")
        for label, path in (("rules", args.rules), ("exemptions", args.exemptions)):
            if path is None or not path.is_file():
                raise Usage(f"a {label} file is required (--{label}); release scanning refuses without one")
        if args.accepted_findings is not None and not args.accepted_findings.is_file():
            raise Usage("the accepted-findings file does not exist")
        if not args.payload:
            raise Usage("at least one payload file is required")
        for path in args.payload:
            if path.is_symlink() or not path.is_file():
                raise Usage(f"a payload is not a regular file: {path.name}")
        if args.report.exists() or args.detail.exists():
            raise Usage("the report and detail must be new files")
        engine = load_engine()
        accepted = read_accepted(args.accepted_findings)
    except Usage as error:
        print(f"release-payload-privacy-scan: {error}", file=sys.stderr)
        return EXIT_USAGE
    work = Path(tempfile.mkdtemp(prefix="payload-scan-", dir=args.work))
    os.chmod(work, 0o700)
    try:
        reroot_dir = work / "reroot"
        reroot_dir.mkdir()
        walker = walker_class(engine)(reroot_dir, args.zstd or shutil.which("zstd"),
                                      args.max_reroots, args.max_reroot_bytes, args.max_nesting, args.piece_bytes)
        payload = [str(path.resolve()) for path in args.payload]
        for index, path in enumerate(payload):
            walker.walk(index, path, 0)
        engine_report = work / "engine-report.json"
        command = [sys.executable, "-I", str(ENGINE), "--rules", str(args.rules), "--exemptions", str(args.exemptions),
                   "--expect-scanner-sha256", ENGINE_SHA256, "--report", str(engine_report),
                   "--detail", str(args.detail), *payload, *walker.artifacts]
        engine_run = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                    stderr=subprocess.PIPE, check=False)
        if args.detail.exists():
            os.chmod(args.detail, 0o600)
        if engine_run.returncode == EXIT_USAGE or not engine_report.is_file() or not args.detail.is_file():
            sys.stderr.write(engine_run.stderr.decode("utf-8", "replace"))
            print("release-payload-privacy-scan: the engine refused its arguments or inputs", file=sys.stderr)
            return EXIT_USAGE
        value = json.loads(engine_report.read_text(encoding="utf-8"))
        detail = json.loads(args.detail.read_text(encoding="utf-8"))
        counts = classify(walker, detail, accepted)
        refused_hits = sum(counts["hits"]["refused"].values())
        refused_errors = sum(counts["errors"]["refused"].values())
        coverage_failed = bool(refused_errors or walker.failures)
        code = (EXIT_HITS if refused_hits else 0) | (EXIT_COVERAGE if coverage_failed else 0)
        reroot_reasons = [reason for reason, _ in walker.reroots.values()]
        report = {
            "schema": SCHEMA_REPORT,
            "status": "clean" if code == 0 else "refused",
            "exit_code": code,
            "engine": {"sha256": ENGINE_SHA256, "exit_code": engine_run.returncode, "result": value["result"]},
            "rules_sha256": value["rules"]["sha256"],
            "exemptions_sha256": value["exemptions"]["sha256"],
            "accepted_findings": None if args.accepted_findings is None else {
                "sha256": sha256_file(args.accepted_findings), "entries": sum(len(v) for v in accepted.values())},
            "payload": [{"name": path.name, "bytes": path.stat().st_size, "sha256": sha256_file(path)}
                        for path in args.payload],
            "reroots": {"depth": reroot_reasons.count("depth"), "zstd": reroot_reasons.count("zstd"),
                        "split": reroot_reasons.count("split"), "artifacts": len(walker.artifacts),
                        "bytes": walker.reroot_bytes},
            "wrapper_failures": len(walker.failures),
            "containers": value["containers"],
            "coverage": {key: value["coverage"][key] for key in ("files", "members", "bytes")},
            "hits": counts["hits"],
            "errors": counts["errors"],
        }
        with open(args.report, "x", encoding="utf-8") as stream:
            stream.write(json.dumps(report, indent=2, sort_keys=True) + "\n")
        print(f"release-payload-privacy-scan: {report['status']}: hits refused {refused_hits}, accepted "
              f"{sum(counts['hits']['accepted'].values())}; errors refused {refused_errors}, accepted "
              f"{sum(counts['errors']['accepted'].values())}, covered {sum(counts['errors']['covered'].values())}; "
              f"wrapper failures {len(walker.failures)}; re-rooted {len(walker.artifacts)}", file=sys.stderr)
        return code
    finally:
        if not args.keep_work:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
