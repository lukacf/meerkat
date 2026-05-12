#!/usr/bin/env python3
"""Validate the public Mintlify docs surface."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Iterable


ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
DOCS_JSON = DOCS / "docs.json"

FRONTMATTER_REQUIRED = {"title", "description", "icon"}
FORBIDDEN_PUBLIC_SUFFIXES = {".html"}
MARKDOWN_LINK_RE = re.compile(r"(?<!!)\[[^\]]+\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
HREF_RE = re.compile(r"""href\s*=\s*(?:"([^"]+)"|'([^']+)')""")


def fail(message: str) -> None:
    print(f"docs-check: {message}", file=sys.stderr)


def flatten_pages(value: object) -> Iterable[str]:
    if isinstance(value, str):
        yield value
        return
    if isinstance(value, list):
        for item in value:
            yield from flatten_pages(item)
        return
    if isinstance(value, dict):
        for key in ("pages", "groups", "tabs"):
            if key in value:
                yield from flatten_pages(value[key])


def nav_pages(config: dict) -> list[str]:
    pages = list(flatten_pages(config.get("navigation", {})))
    return [page for page in pages if not page.startswith(("http://", "https://"))]


def resolve_page(page: str) -> Path | None:
    raw = DOCS / page
    if raw.suffix in {".md", ".mdx"} and raw.exists():
        return raw
    for suffix in (".mdx", ".md"):
        candidate = raw.with_suffix(suffix)
        if candidate.exists():
            return candidate
    return None


def public_path(path: Path) -> str:
    return str(path.relative_to(ROOT))


def parse_frontmatter(path: Path) -> dict[str, str]:
    text = path.read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        return {}
    end = text.find("\n---", 4)
    if end == -1:
        return {}
    raw = text[4:end]
    frontmatter: dict[str, str] = {}
    for line in raw.splitlines():
        if ":" not in line or line.lstrip().startswith("#"):
            continue
        key, value = line.split(":", 1)
        frontmatter[key.strip()] = value.strip().strip('"').strip("'")
    return frontmatter


def strip_code_fences(text: str) -> str:
    out: list[str] = []
    in_fence = False
    for line in text.splitlines():
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
            out.append("")
            continue
        out.append("" if in_fence else line)
    return "\n".join(out)


def has_unclosed_fence(text: str) -> bool:
    in_fence = False
    for line in text.splitlines():
        if line.lstrip().startswith("```"):
            in_fence = not in_fence
    return in_fence


def slugify(heading: str) -> str:
    heading = re.sub(r"`([^`]*)`", r"\1", heading)
    heading = re.sub(r"<[^>]+>", "", heading)
    heading = heading.strip().lower()
    heading = re.sub(r"[^\w\s-]", "", heading)
    heading = re.sub(r"\s+", "-", heading)
    heading = re.sub(r"-+", "-", heading)
    return heading.strip("-")


def heading_slugs(path: Path) -> set[str]:
    text = strip_code_fences(path.read_text(encoding="utf-8"))
    slugs: set[str] = set()
    counts: dict[str, int] = {}
    for line in text.splitlines():
        match = re.match(r"^(#{2,6})\s+(.+?)\s*$", line)
        if not match:
            continue
        base = slugify(match.group(2))
        if not base:
            continue
        count = counts.get(base, 0)
        counts[base] = count + 1
        slugs.add(base if count == 0 else f"{base}-{count}")
    return slugs


def resolve_link(source: Path, target: str) -> tuple[Path | None, str | None, bool]:
    target = target.strip()
    if not target or target.startswith(("#", "http://", "https://", "mailto:", "tel:")):
        return source, target[1:] if target.startswith("#") else None, True
    if target.startswith(("javascript:", "data:")) or "{" in target or "}" in target:
        return None, None, True

    path_part, _, anchor = target.partition("#")
    if not path_part:
        return source, anchor or None, True

    raw = (DOCS / path_part.lstrip("/")) if path_part.startswith("/") else (source.parent / path_part)
    raw = raw.resolve()

    if raw.suffix:
        if raw.exists():
            return raw, anchor or None, True
        return raw, anchor or None, False

    for suffix in (".mdx", ".md"):
        candidate = raw.with_suffix(suffix)
        if candidate.exists():
            return candidate, anchor or None, True
    index = raw / "index.mdx"
    if index.exists():
        return index, anchor or None, True
    return raw, anchor or None, False


def extract_links(path: Path) -> Iterable[str]:
    text = strip_code_fences(path.read_text(encoding="utf-8"))
    for match in MARKDOWN_LINK_RE.finditer(text):
        yield match.group(1)
    for match in HREF_RE.finditer(text):
        yield match.group(1) or match.group(2)


def main() -> int:
    errors: list[str] = []
    config = json.loads(DOCS_JSON.read_text(encoding="utf-8"))

    page_ids = nav_pages(config)
    seen: set[str] = set()
    pages: dict[str, Path] = {}
    for page in page_ids:
        if page in seen:
            errors.append(f"duplicate navigation page '{page}'")
            continue
        seen.add(page)
        path = resolve_page(page)
        if path is None:
            errors.append(f"navigation page '{page}' has no .mdx or .md file")
            continue
        pages[page] = path

    public_files = set(pages.values())
    for path in sorted(DOCS.rglob("*")):
        if path.suffix in FORBIDDEN_PUBLIC_SUFFIXES:
            errors.append(f"forbidden generated/internal file under public docs: {public_path(path)}")
            continue
        if path.suffix not in {".md", ".mdx"}:
            continue
        if path not in public_files:
            errors.append(f"public docs file is not in docs.json navigation: {public_path(path)}")

    slug_cache = {path: heading_slugs(path) for path in public_files}

    for page, path in sorted(pages.items()):
        frontmatter = parse_frontmatter(path)
        missing = sorted(FRONTMATTER_REQUIRED - set(frontmatter))
        if missing:
            errors.append(f"{public_path(path)} missing frontmatter: {', '.join(missing)}")
        if has_unclosed_fence(path.read_text(encoding="utf-8")):
            errors.append(f"{public_path(path)} has an unclosed code fence")
        for target in extract_links(path):
            resolved, anchor, ok = resolve_link(path, target)
            if not ok:
                errors.append(f"{public_path(path)} links to missing target '{target}'")
                continue
            if anchor and resolved and resolved.suffix in {".md", ".mdx"}:
                anchor = anchor.lower()
                if anchor not in slug_cache.get(resolved, set()):
                    errors.append(
                        f"{public_path(path)} links to missing anchor '{target}'"
                    )

    if errors:
        for error in errors:
            fail(error)
        return 1

    print(f"docs-check: ok ({len(public_files)} public pages)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
