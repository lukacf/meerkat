#!/usr/bin/env python3
"""Validate Toolkit navigation, frontmatter, local links, anchors and assets.

Adapted from Meerkat scripts/validate-mintlify-docs.py. This static check
does not replace Mintlify compilation or source/API review.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Iterable
from urllib.parse import quote, unquote


ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs" / "site"
DOCS_JSON = DOCS / "docs.json"
ENTRYPOINTS = (ROOT / "crates" / "meerkat-connect" / "README.md",)
# Research, coordination and verification notes belong outside docs/site.

FRONTMATTER_REQUIRED = {"title", "description"}
FORBIDDEN_PUBLIC_SUFFIXES = {".html"}
MARKDOWN_LINK_RE = re.compile(r"!?\[[^\]]+\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
LINK_ATTR_RE = re.compile(r"""(?:href|src)\s*=\s*(?:"([^"]+)"|'([^']+)')""")


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
        for key in ("pages", "groups", "tabs", "products"):
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


# Anchors are judged by the rule of the site that publishes these pages: a
# heading's id is the slug Mintlify derives from its rendered text,
# percent-decoded and with this punctuation removed, and a link's fragment
# selects the id equal to its percent-decoded form. Meerkat's
# scripts/validate-mintlify-docs.py holds the same functions and its test file
# compares them: change both together.
# The class is the renderer's own (cleanHeadingId in @mintlify/common). Its
# square brackets are from source, no published sample: no published heading
# holds one.
HEADING_ID_STRIPPED_RE = re.compile(r"""[?,;:!'"()\[\]{}]""")
TYPOGRAPHIC_QUOTES = {'"': (chr(0x201C), chr(0x201D)), "'": (chr(0x2018), chr(0x2019))}
LONG_DASH = chr(0x2014)


def heading_characters(heading: str) -> list[tuple[str, bool]]:
    """Return the characters a heading renders, each with whether it is code.

    A code span is literal. In prose a backslash escape renders its character
    (`\\{id\\}` renders `{id}`). An inline tag renders nothing, in prose and,
    as this check has always read it, in a code span.
    """
    characters: list[tuple[str, bool]] = []
    for index, part in enumerate(re.split(r"`([^`]*)`", heading)):
        if index % 2:
            characters.extend((char, True) for char in re.sub(r"<[^>]+>", "", part))
            continue
        for match in re.finditer(r"\\([!-/:-@\[-`{-~])|<[^>]+>|(.)", part):
            char = match.group(1) or match.group(2)
            if char:
                characters.append((char, False))
    return characters


def typographic_text(characters: list[tuple[str, bool]]) -> str:
    """Return the heading text with the typography the site's renderer applies.

    The renderer runs remark-smartypants over a heading before it derives the
    id: straight quotes in prose become curly ones (`The host's own inputs`
    gets the id `the-host\u2019s-own-inputs`) and two hyphens become a long dash
    (`run --resume` gets `run-\u2014resume`). The quote rule is that plugin's:
    code is never changed and counts as a word, and the heading starts after a
    space.
    """
    text = [char for char, _ in characters]
    count = len(text)

    def kind(position: int) -> str | None:
        if position < 0:
            return "space"
        if position >= count:
            return None
        if characters[position][1] or text[position].isalnum():
            return "word"
        return "space" if text[position].isspace() else "mark"

    for position in range(count):
        char = text[position]
        if characters[position][1] or char not in TYPOGRAPHIC_QUOTES:
            continue
        opening, closing = TYPOGRAPHIC_QUOTES[char]
        previous, following = kind(position - 1), kind(position + 1)
        # The next token is a run of one repeated mark, or a run of word characters.
        end = position + 1
        if following == "mark":
            while end < count and kind(end) == "mark" and text[end] == text[position + 1]:
                end += 1
        elif following == "word":
            while kind(end) == "word":
                end += 1
        token = "".join(text[position + 1 : end])
        after = kind(end)
        if following == "mark" and after != "word":
            text[position] = closing
        elif following == "mark" and token in TYPOGRAPHIC_QUOTES:
            text[position] = opening
            text[position + 1] = TYPOGRAPHIC_QUOTES[token][0]
        elif following == "word" and re.fullmatch(r"\d\ds", token):
            text[position] = closing
        elif previous != "word" and following == "word":
            text[position] = opening
        elif previous == "word":
            text[position] = closing
        elif following in {None, "space"}:
            text[position] = closing
        else:
            text[position] = opening

    # Exactly two hyphens in prose are one long dash; other runs stay.
    rendered: list[str] = []
    position = 0
    while position < count:
        end = position
        while end < count and text[end] == "-" and not characters[end][1]:
            end += 1
        if end == position:
            rendered.append(text[position])
            position += 1
            continue
        rendered.append(LONG_DASH if end - position == 2 else "-" * (end - position))
        position = end
    return "".join(rendered)


def encoded_slug(heading: str) -> str:
    """Return the slug Mintlify derives from a heading, percent escapes kept.

    Mintlify lower-cases the rendered heading, turns white space into hyphens,
    applies encodeURIComponent and then runs @sindresorhus/slugify while
    preserving percent escapes and underscores: `capabilities/get` becomes
    `capabilities%2Fget`, not `capabilitiesget`.
    """
    text = typographic_text(heading_characters(heading))
    text = re.sub(r"\s+", "-", text.strip().lower())
    encoded = quote(text, safe="-_.!~*'()")
    encoded = encoded.replace("'", "")
    encoded = re.sub(r"[^A-Za-z0-9%_]+", "-", encoded)
    encoded = re.sub(r"-+", "-", encoded)
    return encoded.strip("-")


def clean_heading_id(slug: str) -> str:
    """Return the id the published page gives a heading with this slug.

    The site decodes the percent escapes and removes `? , ; : ! ' " ( ) [ ] { }`
    (cleanHeadingId in @mintlify/common), so `One writer, fenced` has the id
    `one-writer-fenced` and `capabilities/get` the id `capabilities/get`.
    Hyphens left side by side stay.
    """
    return HEADING_ID_STRIPPED_RE.sub("", unquote(slug))


def slugify(heading: str) -> str:
    """Return the id of a heading on the published site."""
    return clean_heading_id(encoded_slug(heading))


def normalize_anchor(anchor: str) -> str:
    """Return the id a link's anchor selects, in the case `slugify` writes.

    A browser scrolls to the element whose id equals the percent-decoded
    fragment, so `#capabilities%2Fget` and `#capabilities/get` both reach the
    id `capabilities/get`, and `#one-writer%2C-fenced` reaches nothing: the id
    holds no comma.
    """
    return unquote(anchor).lower()


def heading_slugs(path: Path) -> set[str]:
    text = strip_code_fences(path.read_text(encoding="utf-8"))
    slugs: set[str] = set()
    counts: dict[str, int] = {}
    for line in text.splitlines():
        match = re.match(r"^(#{2,6})\s+(.+?)\s*$", line)
        if not match:
            continue
        # Mintlify numbers repeated slugs before it cleans them into ids.
        base = encoded_slug(match.group(2))
        if not base:
            continue
        count = counts.get(base, 0)
        counts[base] = count + 1
        slugs.add(clean_heading_id(base if count == 0 else f"{base}-{count + 1}"))
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
    if not raw.is_relative_to(DOCS.resolve()):
        return raw, anchor or None, False

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
    for match in LINK_ATTR_RE.finditer(text):
        yield match.group(1) or match.group(2)


def check_entrypoint(path: Path) -> list[str]:
    """Check repository-relative links separately from docs-site routes."""
    if not path.is_file():
        return [f"missing documentation entrypoint: {public_path(path)}"]
    errors: list[str] = []
    if has_unclosed_fence(path.read_text(encoding="utf-8")):
        errors.append(f"{public_path(path)} has an unclosed code fence")
    for target in extract_links(path):
        if target.startswith(("http://", "https://", "mailto:", "tel:")):
            continue
        relative, _, anchor = target.partition("#")
        resolved = (path.parent / relative).resolve() if relative else path.resolve()
        if not resolved.is_relative_to(ROOT.resolve()):
            errors.append(f"{public_path(path)} links outside the source tree: '{target}'")
        elif not resolved.exists():
            errors.append(f"{public_path(path)} links to missing target '{target}'")
        elif anchor and resolved.suffix in {".md", ".mdx"}:
            if normalize_anchor(anchor) not in heading_slugs(resolved):
                errors.append(f"{public_path(path)} links to missing anchor '{target}'")
    return errors


def main() -> int:
    errors: list[str] = []
    config = json.loads(DOCS_JSON.read_text(encoding="utf-8"))

    page_ids = nav_pages(config)
    if not page_ids:
        errors.append("docs.json navigation contains no public pages")
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
        if set(path.relative_to(DOCS).parts) & {".mintlify", ".next", "node_modules"}:
            continue
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
        content = path.read_text(encoding="utf-8")
        if "\u2013" in content or "\u2014" in content:
            errors.append(f"{public_path(path)} contains a non-ASCII dash")
        if has_unclosed_fence(content):
            errors.append(f"{public_path(path)} has an unclosed code fence")
        for target in extract_links(path):
            resolved, anchor, ok = resolve_link(path, target)
            if not ok:
                errors.append(f"{public_path(path)} links to missing target '{target}'")
                continue
            if anchor and resolved and resolved.suffix in {".md", ".mdx"}:
                anchor = normalize_anchor(anchor)
                if anchor not in slug_cache.get(resolved, set()):
                    errors.append(
                        f"{public_path(path)} links to missing anchor '{target}'"
                    )

    for asset in config.get("logo", {}).values():
        if isinstance(asset, str) and asset.startswith("/"):
            if not (DOCS / asset.lstrip("/")).is_file():
                errors.append(f"docs.json references missing logo {asset!r}")

    for entrypoint in ENTRYPOINTS:
        errors.extend(check_entrypoint(entrypoint))

    if errors:
        for error in errors:
            fail(error)
        return 1

    print(f"docs-check: ok ({len(public_files)} public pages, {len(ENTRYPOINTS)} repository entrypoint)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
