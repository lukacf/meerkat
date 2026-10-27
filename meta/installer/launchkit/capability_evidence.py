"""Read publisher documentation without installing or executing MCP packages."""
from __future__ import annotations

import asyncio
from datetime import datetime, timezone
import hashlib
import re
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

MAX_DOCUMENT = 1_500_000


class _PublisherRedirects(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if urlsplit(newurl).hostname != "raw.githubusercontent.com" or not newurl.startswith("https://"):
            raise ValueError("Publisher documentation redirected outside its source host.")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def _document(url):
    request = Request(url, headers={"User-Agent": "Meta-Forge/0.2", "Accept": "text/plain"})
    with build_opener(_PublisherRedirects).open(request, timeout=15) as response:
        body = response.read(MAX_DOCUMENT + 1)
    if len(body) > MAX_DOCUMENT:
        raise ValueError("Publisher document is too large. Choose a focused tool reference.")
    return body.decode("utf-8")


def extract_tools(document):
    """Only explicit tool-list entries count, never arbitrary inline identifiers."""
    result, headings = {}, []
    tool_table = False
    fenced = False
    name_pattern = r"([a-z][a-zA-Z0-9_./:-]{0,127})"
    for line in document.splitlines():
        if line.lstrip().startswith(("```", "~~~")):
            fenced = not fenced
            continue
        if fenced:
            continue
        heading = re.match(r"^(#{1,6})\s+(.+)$", line)
        if heading:
            level, title = len(heading[1]), heading[2].strip().lower()
            headings = [(depth, value) for depth, value in headings if depth < level]
            headings.append((level, title))
            tool_table = False
        titles = [value for _, value in headings]
        excluded = any(re.search(r"toolsets?|configuration|environment|installation|authentication|oauth|parameters|arguments", title) for title in titles)
        in_tools = not excluded and any(re.search(r"\btools?\b", title) for title in titles)
        if line.startswith("|"):
            cells = [value.strip().lower() for value in line.strip("|").split("|")]
            if cells and re.fullmatch(r"(?:mcp )?tool(?: name)?", cells[0]):
                tool_table = not excluded
                continue
            if cells and re.fullmatch(r"(?:variable|key|toolset|parameter|argument|scope)(?: name)?", cells[0]):
                tool_table = False
        elif line.strip():
            tool_table = False
        match = None
        if tool_table:
            match = re.match(r"^\|\s*`" + name_pattern + r"`\s*\|\s*([^|]+)\|", line)
        if in_tools and not match:
            match = re.match(r"^- \*\*" + name_pattern + r"\*\*\s*[-:]\s*(.+)$", line)
            if not match:
                match = re.match(r"^#{3,6}\s+`" + name_pattern + r"`\s*$", line)
        if match:
            name = match[1]
            description = match[2].strip() if match.lastindex > 1 else "Publisher documented tool; inspect its schema during installation."
            result[name] = {"name": name, "description": description[:600], "effect": "unclassified"}
    return list(result.values())[:500]


async def inspect_capabilities(connection, *, path="README.md", ref="main"):
    repository = connection.get("source", {}).get("repository", "")
    parsed = urlsplit(repository)
    parts = parsed.path.strip("/").removesuffix(".git").split("/")
    if parsed.scheme != "https" or parsed.hostname != "github.com" or len(parts) != 2 or any(not re.fullmatch(r"[A-Za-z0-9_.-]+", part) for part in parts):
        return {"status": "unresolved", "tools": [], "detail": "No supported publisher source repository is available. Supply a verified tool allowlist or resolve capabilities with a local authenticated handshake before assigning this connection."}
    if not re.fullmatch(r"[A-Za-z0-9_./-]{1,180}", path) or ".." in path.split("/") or not path.endswith((".md", ".mdx", ".json")):
        return {"status": "unresolved", "tools": [], "detail": "Use a relative publisher documentation path ending in .md, .mdx or .json, with no parent traversal. Start with README.md."}
    if not re.fullmatch(r"[A-Za-z0-9_.-]{1,100}", ref):
        return {"status": "unresolved", "tools": [], "detail": "Use a branch, tag or commit reference without path separators. Start with main or the publisher's default branch."}
    url = f"https://raw.githubusercontent.com/{'/'.join(parts)}/{ref}/{path}"
    try:
        document = await asyncio.to_thread(_document, url)
    except Exception as error:
        return {"status": "unresolved", "tools": [], "source_url": url,
                "detail": f"Publisher capability documentation could not be read ({type(error).__name__}). Try its documented path or branch; do not invent tool names."}
    tools = extract_tools(document)
    return {"status": "documented" if tools else "unresolved", "source_url": url,
            "sha256": hashlib.sha256(document.encode()).hexdigest(), "retrieved_at": datetime.now(timezone.utc).isoformat(),
            "tools": tools, "detail": "Publisher documentation is evidence of tool names, not an authenticated runtime handshake or an effect policy. Read descriptions, choose exact capabilities, and verify the installed server at setup.",
            "excerpt": document[:12000] if not tools else ""}
