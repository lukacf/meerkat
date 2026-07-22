#!/usr/bin/env python3
"""Publish MobKit's canonical docs into the docs.rkat.ai source tree.

MobKit authors its public documentation in the meerkat-mobkit repository. The
Mintlify project for docs.rkat.ai deploys from this repository, so this script
creates the namespaced snapshot consumed by the MobKit product navigation.

Usage:
    python3 scripts/sync-mobkit-docs.py /path/to/meerkat-mobkit
    python3 scripts/sync-mobkit-docs.py /path/to/meerkat-mobkit --check
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "docs" / "mobkit"
ASSET_PATHS = ("images", "logo.png")
ROOT_LINK_RE = re.compile(r'(?P<prefix>\]\(|(?:href|src)=["\'])/(?P<path>[^/])')
REQUIRED_PAGE_ICONS = {
    "introduction": "boxes-stacked",
    "quickstart": "rocket",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "source",
        type=Path,
        help="path to a meerkat-mobkit checkout",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify that the committed snapshot matches the source",
    )
    parser.add_argument(
        "--require-clean",
        action="store_true",
        help="reject a source checkout with uncommitted documentation changes",
    )
    parser.add_argument(
        "--source-ref",
        help="immutable source tag or ref to record in the generated manifest",
    )
    return parser.parse_args()


def flatten_pages(value: object) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [page for item in value for page in flatten_pages(item)]
    if isinstance(value, dict):
        return [
            page
            for key in ("pages", "groups", "tabs", "products")
            if key in value
            for page in flatten_pages(value[key])
        ]
    return []


def git_output(source: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(source), *args],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def rewrite_root_links(text: str) -> str:
    return ROOT_LINK_RE.sub(
        lambda match: f"{match.group('prefix')}/mobkit/{match.group('path')}",
        text,
    )


def ensure_page_icon(text: str, page_id: str) -> str:
    icon = REQUIRED_PAGE_ICONS.get(page_id)
    if icon is None or re.search(r"(?m)^icon\s*:", text):
        return text
    if not text.startswith("---\n"):
        raise SystemExit(f"MobKit page lacks frontmatter: {page_id}")
    end = text.find("\n---", 4)
    if end == -1:
        raise SystemExit(f"MobKit page has unclosed frontmatter: {page_id}")
    return f'{text[:end]}\nicon: "{icon}"{text[end:]}'


def tree_digest(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def workspace_version(cargo_toml: Path) -> str:
    in_workspace_package = False
    for raw_line in cargo_toml.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if line.startswith("[") and line.endswith("]"):
            in_workspace_package = line == "[workspace.package]"
            continue
        if in_workspace_package:
            match = re.fullmatch(r'version\s*=\s*"([^"]+)"', line)
            if match:
                return match.group(1)
    raise SystemExit(f"workspace package version not found: {cargo_toml}")


def build_snapshot(
    source: Path,
    destination: Path,
    *,
    source_ref: str | None,
    require_clean: bool,
) -> None:
    source = source.resolve()
    source_docs = source / "docs"
    config_path = source_docs / "docs.json"
    if not config_path.is_file():
        raise SystemExit(f"MobKit docs config not found: {config_path}")

    source_dirty = bool(git_output(source, "status", "--short", "--", "docs"))
    if require_clean and source_dirty:
        raise SystemExit("MobKit documentation source has uncommitted changes")

    source_version = workspace_version(source / "Cargo.toml")
    if source_ref is None:
        exact_tag = subprocess.run(
            ["git", "-C", str(source), "describe", "--tags", "--exact-match", "HEAD"],
            capture_output=True,
            text=True,
        )
        source_ref = exact_tag.stdout.strip() if exact_tag.returncode == 0 else None

    config = json.loads(config_path.read_text(encoding="utf-8"))
    page_ids = flatten_pages(config.get("navigation", {}))
    if not page_ids:
        raise SystemExit("MobKit docs navigation contains no pages")
    if len(page_ids) != len(set(page_ids)):
        raise SystemExit("MobKit docs navigation contains duplicate pages")

    destination.mkdir(parents=True, exist_ok=True)
    for page_id in page_ids:
        source_page = source_docs / f"{page_id}.mdx"
        if not source_page.is_file():
            raise SystemExit(f"MobKit navigation page not found: {source_page}")
        destination_page = destination / f"{page_id}.mdx"
        destination_page.parent.mkdir(parents=True, exist_ok=True)
        rendered = source_page.read_text(encoding="utf-8")
        rendered = ensure_page_icon(rendered, page_id)
        rendered = rewrite_root_links(rendered)
        destination_page.write_text(rendered, encoding="utf-8")

    for relative in ASSET_PATHS:
        source_asset = source_docs / relative
        destination_asset = destination / relative
        if source_asset.is_dir():
            shutil.copytree(source_asset, destination_asset)
        elif source_asset.is_file():
            destination_asset.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source_asset, destination_asset)

    source_commit = git_output(source, "rev-parse", "HEAD")
    manifest = {
        "generated": True,
        "source_repository": "https://github.com/lukacf/meerkat-mobkit",
        "source_ref": source_ref,
        "source_commit": source_commit,
        "source_version": source_version,
        "source_docs_dirty": source_dirty,
        "public_pages": len(page_ids),
        "content_sha256": tree_digest(destination),
    }
    (destination / "_source.json").write_text(
        json.dumps(manifest, indent=2) + "\n",
        encoding="utf-8",
    )


def main() -> int:
    args = parse_args()
    source = args.source.resolve()

    if args.check:
        if not DESTINATION.is_dir():
            raise SystemExit(f"MobKit docs snapshot not found: {DESTINATION}")
        with tempfile.TemporaryDirectory(prefix="mobkit-docs-") as temp:
            expected = Path(temp) / "mobkit"
            build_snapshot(
                source,
                expected,
                source_ref=args.source_ref,
                require_clean=args.require_clean,
            )
            comparison = subprocess.run(
                ["diff", "-ru", str(DESTINATION), str(expected)],
                text=True,
            )
            if comparison.returncode:
                raise SystemExit(
                    "MobKit docs snapshot is stale; run "
                    f"python3 scripts/sync-mobkit-docs.py {source}"
                )
        print("mobkit-docs-sync: ok")
        return 0

    if DESTINATION.exists():
        shutil.rmtree(DESTINATION)
    build_snapshot(
        source,
        DESTINATION,
        source_ref=args.source_ref,
        require_clean=args.require_clean,
    )
    manifest = json.loads((DESTINATION / "_source.json").read_text())
    print(
        "mobkit-docs-sync: wrote "
        f"{manifest['public_pages']} pages from {manifest['source_commit']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
