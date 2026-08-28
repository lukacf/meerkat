#!/usr/bin/env python3
"""Prepare and verify the complete, flat Meerkat binary release asset set."""

import argparse
import hashlib
import json
import shutil
import sys
from pathlib import Path


ARCHIVE_PREFIXES = ("rkat", "rkat-mcp", "rkat-rest", "rkat-rpc")
TARGET_ARCHIVES = (
    ("x86_64-unknown-linux-gnu", "tar.gz"),
    ("aarch64-unknown-linux-gnu", "tar.gz"),
    ("aarch64-apple-darwin", "tar.gz"),
    ("x86_64-apple-darwin", "tar.gz"),
    ("x86_64-pc-windows-msvc", "zip"),
)
MANIFEST_NAME = "index.json"
CHECKSUMS_NAME = "checksums.sha256"


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


def expected_archive_names(version):
    return sorted(
        f"{prefix}-{version}-{target}.{extension}"
        for prefix in ARCHIVE_PREFIXES
        for target, extension in TARGET_ARCHIVES
    )


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


def verify_assets(assets_dir, release_tag):
    tag, version = normalized_version(release_tag)
    assets_dir = assets_dir.resolve()
    if not assets_dir.is_dir():
        raise ReleaseAssetError(f"asset directory does not exist: {assets_dir}")

    all_files = sorted(path for path in assets_dir.rglob("*") if path.is_file())
    nested = [
        path.relative_to(assets_dir).as_posix()
        for path in all_files
        if path.parent != assets_dir
    ]
    if nested:
        raise ReleaseAssetError(
            "release assets must be flat; nested files: " + ", ".join(nested)
        )

    expected_archives = set(expected_archive_names(version))
    actual_archives = {path.name for path in all_files if is_archive(path)}
    if actual_archives != expected_archives:
        raise ReleaseAssetError(
            describe_set_mismatch("archive set", expected_archives, actual_archives)
        )

    allowed_files = expected_archives | {MANIFEST_NAME, CHECKSUMS_NAME}
    actual_files = {path.name for path in all_files}
    if actual_files != allowed_files:
        raise ReleaseAssetError(
            describe_set_mismatch("release file set", allowed_files, actual_files)
        )

    manifest_path = assets_dir / MANIFEST_NAME
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ReleaseAssetError(f"cannot read {MANIFEST_NAME}: {error}") from error

    expected_manifest = {
        "version": version,
        "tag": tag,
        "artifacts": sorted(expected_archives),
        "checksums": CHECKSUMS_NAME,
    }
    if manifest != expected_manifest:
        raise ReleaseAssetError(
            f"{MANIFEST_NAME} does not match the flat release asset set"
        )

    checksum_path = assets_dir / CHECKSUMS_NAME
    checksums = {}
    try:
        checksum_lines = checksum_path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ReleaseAssetError(f"cannot read {CHECKSUMS_NAME}: {error}") from error

    for line_number, line in enumerate(checksum_lines, start=1):
        try:
            digest, name = line.split("  ", 1)
        except ValueError as error:
            raise ReleaseAssetError(
                f"invalid {CHECKSUMS_NAME} line {line_number}: {line!r}"
            ) from error
        if len(digest) != 64 or any(
            character not in "0123456789abcdef" for character in digest
        ):
            raise ReleaseAssetError(
                f"invalid SHA-256 on {CHECKSUMS_NAME} line {line_number}"
            )
        if "/" in name or "\\" in name:
            raise ReleaseAssetError(
                f"checksum entry must use a flat basename, found {name!r}"
            )
        if name in checksums:
            raise ReleaseAssetError(f"duplicate checksum entry for {name}")
        checksums[name] = digest

    if set(checksums) != expected_archives:
        raise ReleaseAssetError(
            describe_set_mismatch(
                "checksum entries", expected_archives, set(checksums)
            )
        )
    for name in sorted(expected_archives):
        actual_digest = sha256(assets_dir / name)
        if checksums[name] != actual_digest:
            raise ReleaseAssetError(f"checksum mismatch for {name}")

    print(f"verified {len(expected_archives)} flat release archives for {tag}")


def prepare_assets(source_dir, output_dir, release_tag):
    tag, version = normalized_version(release_tag)
    source_dir = source_dir.resolve()
    output_dir = output_dir.resolve()
    if not source_dir.is_dir():
        raise ReleaseAssetError(f"artifact directory does not exist: {source_dir}")
    if output_dir.exists():
        raise ReleaseAssetError(f"output directory already exists: {output_dir}")

    archives = sorted(
        path for path in source_dir.rglob("*") if path.is_file() and is_archive(path)
    )
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
            locations = ", ".join(
                path.relative_to(source_dir).as_posix() for path in paths
            )
            duplicate_details.append(f"{name}: {locations}")
        raise ReleaseAssetError(
            "duplicate archive basenames detected; refusing to flatten: "
            + "; ".join(duplicate_details)
        )

    expected_archives = set(expected_archive_names(version))
    actual_archives = set(archives_by_name)
    if actual_archives != expected_archives:
        raise ReleaseAssetError(
            describe_set_mismatch(
                "downloaded archive set", expected_archives, actual_archives
            )
        )

    output_dir.mkdir(parents=True)
    for name in sorted(expected_archives):
        shutil.copy2(archives_by_name[name], output_dir / name)

    checksum_lines = [
        f"{sha256(output_dir / name)}  {name}\n" for name in sorted(expected_archives)
    ]
    (output_dir / CHECKSUMS_NAME).write_text(
        "".join(checksum_lines), encoding="utf-8"
    )

    manifest = {
        "version": version,
        "tag": tag,
        "artifacts": sorted(expected_archives),
        "checksums": CHECKSUMS_NAME,
    }
    (output_dir / MANIFEST_NAME).write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    verify_assets(output_dir, tag)


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    prepare = subparsers.add_parser(
        "prepare", help="flatten and prepare downloaded artifacts"
    )
    prepare.add_argument("--source-dir", type=Path, required=True)
    prepare.add_argument("--output-dir", type=Path, required=True)
    prepare.add_argument("--release-tag", required=True)

    verify = subparsers.add_parser(
        "verify", help="verify a prepared flat release asset set"
    )
    verify.add_argument("--assets-dir", type=Path, required=True)
    verify.add_argument("--release-tag", required=True)

    return parser.parse_args()


def main():
    args = parse_args()
    try:
        if args.command == "prepare":
            prepare_assets(args.source_dir, args.output_dir, args.release_tag)
        else:
            verify_assets(args.assets_dir, args.release_tag)
    except ReleaseAssetError as error:
        print(f"release asset error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
