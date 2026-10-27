#!/usr/bin/env python3
"""Prepare and verify the flat Meta gateway release asset set.

Schema 1 (releases up to v0.9.1): one flat set of all 15 archives, with
index.json and checksums.sha256 listing every one of them.

Schema 2: the targets form two tiers. The core tier (Linux x86_64 and
aarch64, macOS arm64) publishes first with index.json and checksums.sha256
listing only its archives; the trailing tier (macOS x86_64, Windows) is
added later, add-only, with its own index-trailing.json and
checksums-trailing.sha256. Each tier's files describe that tier alone, so
adding the trailing tier never rewrites a published file.
"""

import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path


ARCHIVE_PREFIXES = ("meta-gateway", "meta-rpc-gateway", "meta-repair")
TARGET_ARCHIVES = (
    ("x86_64-unknown-linux-gnu", "tar.gz"),
    ("aarch64-unknown-linux-gnu", "tar.gz"),
    ("aarch64-apple-darwin", "tar.gz"),
    ("x86_64-apple-darwin", "tar.gz"),
    ("x86_64-pc-windows-msvc", "zip"),
)
MANIFEST_NAME = "index.json"
CHECKSUMS_NAME = "checksums.sha256"
TIER_TARGETS = {
    "core": ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin"),
    "trailing": ("x86_64-apple-darwin", "x86_64-pc-windows-msvc"),
}
TIERS = tuple(TIER_TARGETS)
TIER_METADATA = {
    "core": (MANIFEST_NAME, CHECKSUMS_NAME),
    "trailing": ("index-trailing.json", "checksums-trailing.sha256"),
}
assert sorted(t for targets in TIER_TARGETS.values() for t in targets) == sorted(t for t, _ in TARGET_ARCHIVES)


class ReleaseAssetError(RuntimeError):
    pass


def normalized_version(release_tag):
    tag = release_tag.strip()
    if not tag:
        raise ReleaseAssetError("release tag must not be empty")
    version = tag[1:] if tag.startswith("v") else tag
    version = version.replace("/", "-")
    if not version:
        raise ReleaseAssetError("release tag does not contain a version")
    return tag, version


def tier_names(tiers):
    """None means schema 1 (one untiered set); otherwise known tiers, core first."""
    if tiers is None:
        return None
    tiers = tuple(tiers)
    if not tiers or tiers[0] != "core" or len(set(tiers)) != len(tiers) or not set(tiers) <= set(TIERS):
        raise ReleaseAssetError(f"invalid release tiers: {tiers!r}")
    return tiers


def tier_of(target):
    for tier, targets in TIER_TARGETS.items():
        if target in targets:
            return tier
    raise ReleaseAssetError(f"unknown target: {target}")


def expected_archive_names(version, tier=None):
    """Every archive name, or one tier's when `tier` names it."""
    if tier is not None and tier not in TIERS:
        raise ReleaseAssetError(f"unknown release tier: {tier!r}")
    return sorted(
        f"{prefix}-{version}-{target}.{extension}"
        for prefix in ARCHIVE_PREFIXES
        for target, extension in TARGET_ARCHIVES
        if tier is None or target in TIER_TARGETS[tier]
    )


def metadata_names(tier=None):
    """The (index, checksums) file names of schema 1 (None) or of one tier."""
    if tier is None:
        return MANIFEST_NAME, CHECKSUMS_NAME
    if tier not in TIERS:
        raise ReleaseAssetError(f"unknown release tier: {tier!r}")
    return TIER_METADATA[tier]


def index_document(version, tag, tier, archives):
    index_name, checksums_name = metadata_names(tier)
    document = {"version": version, "tag": tag, "artifacts": sorted(archives), "checksums": checksums_name}
    if tier is not None:
        document["tier"] = tier
    return document


def is_archive(path):
    return path.name.endswith(".tar.gz") or path.name.endswith(".zip")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def describe_set_mismatch(label, expected, actual):
    missing = sorted(expected - actual)
    unexpected = sorted(actual - expected)
    details = []
    if missing:
        details.append("missing: " + ", ".join(missing))
    if unexpected:
        details.append("unexpected: " + ", ".join(unexpected))
    return f"{label} mismatch ({'; '.join(details)})"


def read_checksums(path, checksums_name):
    checksums = {}
    try:
        checksum_lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ReleaseAssetError(f"cannot read {checksums_name}: {error}") from error

    for line_number, line in enumerate(checksum_lines, start=1):
        try:
            digest, name = line.split("  ", 1)
        except ValueError as error:
            raise ReleaseAssetError(
                f"invalid {checksums_name} line {line_number}: {line!r}"
            ) from error
        if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest):
            raise ReleaseAssetError(
                f"invalid SHA-256 on {checksums_name} line {line_number}"
            )
        if "/" in name or "\\" in name:
            raise ReleaseAssetError(
                f"checksum entry must use a flat basename, found {name!r}"
            )
        if name in checksums:
            raise ReleaseAssetError(f"duplicate checksum entry for {name}")
        checksums[name] = digest
    return checksums


def verify_tier_files(assets_dir, tag, version, tier):
    """One tier's (or schema 1's) index and checksums against its archives."""
    index_name, checksums_name = metadata_names(tier)
    expected_archives = set(expected_archive_names(version, tier))
    manifest_path = assets_dir / index_name
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ReleaseAssetError(f"cannot read {index_name}: {error}") from error

    if manifest != index_document(version, tag, tier, expected_archives):
        raise ReleaseAssetError(
            f"{index_name} does not match the flat release asset set"
        )

    checksums = read_checksums(assets_dir / checksums_name, checksums_name)
    if set(checksums) != expected_archives:
        raise ReleaseAssetError(
            describe_set_mismatch("checksum entries", expected_archives, set(checksums))
        )
    for name in sorted(expected_archives):
        actual_digest = sha256(assets_dir / name)
        if checksums[name] != actual_digest:
            raise ReleaseAssetError(f"checksum mismatch for {name}")
    return expected_archives


def verify_assets(assets_dir, release_tag, tiers=None):
    """Verify a flat asset set: schema 1 when `tiers` is None, otherwise
    exactly the listed tiers (core first), each with its own metadata."""
    tiers = tier_names(tiers)
    return verify_groups(assets_dir, release_tag, (None,) if tiers is None else tiers)


def verify_tier_assets(assets_dir, release_tag, tier):
    """Verify a directory holding exactly one tier's archives and metadata."""
    if tier not in TIERS:
        raise ReleaseAssetError(f"unknown release tier: {tier!r}")
    return verify_groups(assets_dir, release_tag, (tier,))


def verify_groups(assets_dir, release_tag, groups):
    tag, version = normalized_version(release_tag)
    assets_dir = assets_dir.resolve()
    if not assets_dir.is_dir():
        raise ReleaseAssetError(f"asset directory does not exist: {assets_dir}")

    all_files = sorted(path for path in assets_dir.rglob("*") if path.is_file())
    nested = [path.relative_to(assets_dir).as_posix() for path in all_files if path.parent != assets_dir]
    if nested:
        raise ReleaseAssetError("release assets must be flat; nested files: " + ", ".join(nested))

    expected_archives = set()
    allowed_files = set()
    for tier in groups:
        expected_archives |= set(expected_archive_names(version, tier))
        allowed_files |= set(metadata_names(tier))
    allowed_files |= expected_archives

    actual_archives = {path.name for path in all_files if is_archive(path)}
    if actual_archives != expected_archives:
        raise ReleaseAssetError(
            describe_set_mismatch("archive set", expected_archives, actual_archives)
        )

    actual_files = {path.name for path in all_files}
    if actual_files != allowed_files:
        raise ReleaseAssetError(
            describe_set_mismatch("release file set", allowed_files, actual_files)
        )

    for tier in groups:
        verify_tier_files(assets_dir, tag, version, tier)

    label = "" if groups == (None,) else f" ({', '.join(groups)})"
    print(f"verified {len(expected_archives)} flat release archives for {tag}{label}")
    return expected_archives


def prepare_assets(source_dir, output_dir, release_tag, tier=None):
    """Flatten downloaded archives: all of them with schema 1 metadata, or
    exactly one tier's with that tier's metadata."""
    tag, version = normalized_version(release_tag)
    source_dir = source_dir.resolve()
    output_dir = output_dir.resolve()
    if not source_dir.is_dir():
        raise ReleaseAssetError(f"artifact directory does not exist: {source_dir}")
    if output_dir.exists():
        raise ReleaseAssetError(f"output directory already exists: {output_dir}")

    archives = sorted(path for path in source_dir.rglob("*") if path.is_file() and is_archive(path))
    archives_by_name = {}
    duplicates = {}
    for path in archives:
        if path.name in archives_by_name:
            duplicates.setdefault(path.name, [archives_by_name[path.name]]).append(path)
        else:
            archives_by_name[path.name] = path
    if duplicates:
        duplicate_details = []
        for name, paths in sorted(duplicates.items()):
            locations = ", ".join(path.relative_to(source_dir).as_posix() for path in paths)
            duplicate_details.append(f"{name}: {locations}")
        raise ReleaseAssetError(
            "duplicate archive basenames detected; refusing to flatten: "
            + "; ".join(duplicate_details)
        )

    expected_archives = set(expected_archive_names(version, tier))
    actual_archives = set(archives_by_name)
    if actual_archives != expected_archives:
        raise ReleaseAssetError(
            describe_set_mismatch("downloaded archive set", expected_archives, actual_archives)
        )

    index_name, checksums_name = metadata_names(tier)
    output_dir.mkdir(parents=True)
    for name in sorted(expected_archives):
        shutil.copy2(archives_by_name[name], output_dir / name)

    checksum_lines = [
        f"{sha256(output_dir / name)}  {name}\n" for name in sorted(expected_archives)
    ]
    (output_dir / checksums_name).write_text("".join(checksum_lines), encoding="utf-8")

    manifest = index_document(version, tag, tier, expected_archives)
    (output_dir / index_name).write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    if tier is None:
        verify_assets(output_dir, tag)
    else:
        verify_tier_assets(output_dir, tag, tier)


def tiers_present(assets_dir):
    """The default for `verify` without --tier: schema 1 when index.json has
    no tier, otherwise every tier whose index is present, core first."""
    try:
        index = json.loads((assets_dir / MANIFEST_NAME).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ReleaseAssetError(f"cannot read {MANIFEST_NAME}: {error}") from error
    if not isinstance(index, dict) or "tier" not in index:
        return None
    return tuple(tier for tier in TIERS if (assets_dir / metadata_names(tier)[0]).is_file())


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    prepare = subparsers.add_parser("prepare", help="flatten and prepare downloaded artifacts")
    prepare.add_argument("--source-dir", type=Path, required=True)
    prepare.add_argument("--output-dir", type=Path, required=True)
    prepare.add_argument("--release-tag", required=True)
    prepare.add_argument("--tier", choices=TIERS, help="prepare one tier (schema 2); default schema 1")

    verify = subparsers.add_parser("verify", help="verify a prepared flat release asset set")
    verify.add_argument("--assets-dir", type=Path, required=True)
    verify.add_argument("--release-tag", required=True)
    verify.add_argument("--tier", choices=TIERS, action="append",
                        help="verify these tiers, core first (schema 2); default: schema 1, or every "
                             "tier present when index.json names a tier")

    return parser.parse_args()


def main():
    args = parse_args()
    try:
        if args.command == "prepare":
            prepare_assets(args.source_dir, args.output_dir, args.release_tag, args.tier)
        else:
            verify_assets(args.assets_dir, args.release_tag, args.tier or tiers_present(args.assets_dir))
    except ReleaseAssetError as error:
        print(f"release asset error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
