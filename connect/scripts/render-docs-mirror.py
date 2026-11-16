#!/usr/bin/env python3
"""Render Connect's documentation pages into the published copies under docs/connect.

A published copy is its source page byte for byte, with one change: root-relative link
targets outside fenced code blocks gain the /connect prefix (Markdown links `](/x)` and
`href="/x"` attributes). Fenced blocks are copied as they are.

usage:
  render-docs-mirror.py            regenerate the copies, the manifest and the navigation
  render-docs-mirror.py --check    change nothing; exit 1 when anything would change

Regeneration writes only what differs: the copies of changed pages, the manifest
docs/connect/_source.json (page count, link and fenced block counts, a source and a
rendered SHA-256 per page, the Git tree id of the source directory, and Connect's
version as its crates state it), and the Connect navigation and redirects in
docs/docs.json, taken from the source site's docs.json with the connect/ prefix. A copy
whose source page is gone is removed. The manifest's descriptive fields are kept as they
are. It records no source commit: a tool that reads working files cannot know one, and
the tree id is what binds the copies to their source.

--site, --mirror, --nav and --crates name other directories and files; the tests use
them. Standard library only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / "connect" / "docs" / "site"
MIRROR = ROOT / "docs" / "connect"
NAV = ROOT / "docs" / "docs.json"
CRATES = ROOT / "connect" / "crates"
MANIFEST = "_source.json"
PREFIX = "/connect"
COMPUTED = ("source_docs_tree", "connect_source_version", "pages", "fenced_blocks_preserved", "namespaced_links", "files")
# Earlier manifests named a source commit and a tree of the whole product; neither can be
# read from working files, so they are not carried over.
DROPPED = ("connect_source_commit", "connect_source_tree")

MD_LINK = re.compile(r"(\]\()(/)(?!/)")
HREF = re.compile(r"""(href\s*=\s*["'])(/)(?!/)""")
PACKAGE_VERSION = re.compile(r'(?m)^version\s*=\s*"([^"]+)"\s*$')


def product_version(crates: Path) -> str:
    """Connect's version: the package.version every crate under the directory states."""
    versions = {}
    for manifest in sorted(crates.glob("*/Cargo.toml")):
        text = manifest.read_text(encoding="utf-8")
        start = text.find("[package]")
        if start < 0:
            continue
        end = text.find("\n[", start + 1)
        found = PACKAGE_VERSION.search(text[start : end if end >= 0 else len(text)])
        if found is None:
            raise ValueError(f"{manifest.parent.name} states no literal package.version")
        versions[manifest.parent.name] = found.group(1)
    if not versions:
        raise ValueError(f"no crate under {crates.name} states a version")
    if len(set(versions.values())) != 1:
        raise ValueError("the crates state different versions: " + ", ".join(f"{name} {value}" for name, value in versions.items()))
    return next(iter(versions.values()))


def render(text: str) -> tuple[str, int, int]:
    """The published text of a page, the links it prefixed and the fenced blocks it kept."""
    out = []
    in_fence = False
    links = fences = 0
    for line in text.split("\n"):
        if line.lstrip().startswith("```"):
            if not in_fence:
                fences += 1
            in_fence = not in_fence
            out.append(line)
            continue
        if in_fence:
            out.append(line)
            continue
        line, first = MD_LINK.subn(r"\1" + PREFIX + "/", line)
        line, second = HREF.subn(r"\1" + PREFIX + "/", line)
        links += first + second
        out.append(line)
    return "\n".join(out), links, fences


def pages(directory: Path) -> list[str]:
    """The .mdx pages under a directory, as sorted relative POSIX paths."""
    if not directory.is_dir():
        return []
    return sorted(path.relative_to(directory).as_posix() for path in directory.rglob("*.mdx") if path.is_file())


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def git_object(kind: str, body: bytes) -> bytes:
    return hashlib.sha1(kind.encode() + b" " + str(len(body)).encode() + b"\0" + body).digest()


def git_tree(directory: Path) -> bytes:
    """The Git tree id of a directory's files as they are on disk (binary SHA-1)."""
    entries = []
    for child in directory.iterdir():
        if child.is_symlink():
            entries.append((child.name, b"120000", git_object("blob", os.readlink(child).encode())))
        elif child.is_dir():
            if any(child.iterdir()):
                entries.append((child.name + "/", b"40000", git_tree(child)))
        elif child.is_file():
            mode = b"100755" if child.stat().st_mode & stat.S_IXUSR else b"100644"
            entries.append((child.name, mode, git_object("blob", child.read_bytes())))
    body = b"".join(
        mode + b" " + name.rstrip("/").encode() + b"\0" + digest
        for name, mode, digest in sorted(entries, key=lambda entry: entry[0].encode())
    )
    return git_object("tree", body)


def dump(value: object, ascii_only: bool) -> str:
    return json.dumps(value, indent=2, ensure_ascii=ascii_only) + "\n"


def prefixed(value: object) -> object:
    """A navigation value of the source site with every page under connect/."""
    if isinstance(value, str):
        return "connect/" + value
    if isinstance(value, list):
        return [prefixed(item) for item in value]
    if isinstance(value, dict):
        return {key: (prefixed(item) if key in ("pages", "groups", "tabs") else item) for key, item in value.items()}
    return value


def navigation(site: Path, nav: Path) -> tuple[str | None, str | None]:
    """The text docs.json should have, and a refusal when it cannot be decided."""
    raw = nav.read_text(encoding="utf-8")
    document = json.loads(raw)
    if dump(document, False) != raw:
        return None, f"{nav.name} is not in the form this tool writes; it is left as it is"
    source = json.loads((site / "docs.json").read_text(encoding="utf-8"))
    products = [product for product in document["navigation"]["products"] if product.get("product") == "Connect"]
    if len(products) != 1:
        return None, f"{nav.name} does not hold exactly one Connect product"
    products[0]["tabs"] = prefixed(source["navigation"]["tabs"])
    redirects = [entry for entry in document.get("redirects", []) if not entry["source"].startswith(PREFIX + "/")]
    for entry in source.get("redirects", []):
        redirects.append({"source": PREFIX + entry["source"], "destination": PREFIX + entry["destination"]})
    if redirects or "redirects" in document:
        document["redirects"] = redirects
    return dump(document, False), None


def plan(site: Path, mirror: Path, nav: Path | None, crates: Path) -> tuple[dict[str, bytes], list[str], str, list[str]]:
    """What the mirror should hold: page bytes, stale copies, the manifest text, notes."""
    wanted: dict[str, bytes] = {}
    files = []
    links = fences = 0
    for page in pages(site):
        raw = (site / page).read_bytes()
        text, page_links, page_fences = render(raw.decode("utf-8"))
        data = text.encode("utf-8")
        wanted[page] = data
        links += page_links
        fences += page_fences
        files.append({"path": page, "source_sha256": sha256(raw), "rendered_sha256": sha256(data)})
    stale = [page for page in pages(mirror) if page not in wanted]
    manifest_path = mirror / MANIFEST
    kept = json.loads(manifest_path.read_text(encoding="utf-8")) if manifest_path.is_file() else {}
    tree, version = git_tree(site).hex(), product_version(crates)
    manifest = {}
    for key, value in kept.items():
        if key == "source_docs_tree":
            manifest[key] = tree
        elif key == "connect_source_version":
            manifest[key] = version
        elif key not in COMPUTED and key not in DROPPED:
            manifest[key] = value
    manifest.setdefault("source_docs_tree", tree)
    manifest.setdefault("connect_source_version", version)
    manifest.update({"pages": len(wanted), "fenced_blocks_preserved": fences, "namespaced_links": links, "files": files})
    notes = []
    if nav is not None and not nav.is_file():
        notes.append(f"no {nav.name}: the navigation is not handled")
    return wanted, stale, dump(manifest, True), notes


def main() -> int:
    parser = argparse.ArgumentParser(description="Render Connect's documentation pages into docs/connect.")
    parser.add_argument("--check", action="store_true", help="change nothing; exit 1 when anything would change")
    parser.add_argument("--site", type=Path, default=SITE, help="the source pages (default: connect/docs/site)")
    parser.add_argument("--mirror", type=Path, default=MIRROR, help="the published copies (default: docs/connect)")
    parser.add_argument("--nav", type=Path, default=NAV, help="the site's docs.json (default: docs/docs.json)")
    parser.add_argument("--crates", type=Path, default=CRATES, help="Connect's crates (default: connect/crates)")
    args = parser.parse_args()
    site, mirror, nav = args.site, args.mirror, args.nav
    if not site.is_dir():
        print(f"render-docs-mirror: no source directory {site}", file=sys.stderr)
        return 2
    try:
        wanted, stale, manifest, notes = plan(site, mirror, nav, args.crates)
    except ValueError as refusal:
        print(f"render-docs-mirror: Connect's version cannot be read: {refusal}", file=sys.stderr)
        return 2
    changed = []
    for page, data in wanted.items():
        target = mirror / page
        if not target.is_file():
            changed.append(("missing", page))
        elif target.read_bytes() != data:
            changed.append(("differs", page))
    changed.extend(("stale", page) for page in stale)
    manifest_path = mirror / MANIFEST
    if not manifest_path.is_file() or manifest_path.read_text(encoding="utf-8") != manifest:
        changed.append(("differs", MANIFEST))
    nav_text = None
    if nav.is_file():
        nav_text, refusal = navigation(site, nav)
        if refusal:
            notes.append(refusal)
        elif nav.read_text(encoding="utf-8") != nav_text:
            changed.append(("differs", nav.name))
        else:
            nav_text = None
    for note in notes:
        print(f"note: {note}")
    same = len(wanted) - sum(1 for kind, name in changed if name in wanted)
    if args.check:
        for kind, name in changed:
            print(f"{kind} {name}")
        print(f"same {same}, differ {len(wanted) - same}; other differences {sum(1 for _kind, name in changed if name not in wanted)}")
        return 1 if changed else 0
    for kind, name in changed:
        if name in wanted:
            target = mirror / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(wanted[name])
        elif kind == "stale":
            (mirror / name).unlink()
            parent = (mirror / name).parent
            while parent != mirror and not any(parent.iterdir()):
                parent.rmdir()
                parent = parent.parent
        elif name == MANIFEST:
            mirror.mkdir(parents=True, exist_ok=True)
            manifest_path.write_text(manifest, encoding="utf-8")
        elif nav_text is not None:
            nav.write_text(nav_text, encoding="utf-8")
        print(f"{'removed' if kind == 'stale' else 'written'} {name}")
    print(f"rendered {len(wanted)} pages; {len(changed)} files changed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
