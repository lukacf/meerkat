#!/usr/bin/env python3
"""Anonymous public GitHub research through the existing owned setup worker.

No model, account, package selection, arbitrary origin or code execution is
involved. Results are untrusted research evidence, never installation authority.
"""
from __future__ import annotations

import base64
from collections import deque
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import ssl
import sys
import tempfile
import time
import unicodedata
import urllib.parse

ORIGIN = "https://api.github.com"
TIMEOUT = 30
RESPONSE_LIMIT = 256 * 1024
TEXT_LIMIT = 64 * 1024
PAGE_SIZE = 20
PAGE_LIMIT = 5
RELEASE_ASSET_LIMIT = 200
OWNER = re.compile(r"[A-Za-z0-9][A-Za-z0-9-]{0,38}\Z")
REPOSITORY = re.compile(r"[A-Za-z0-9._-]{1,100}\Z")
BLOB = re.compile(r"[0-9a-f]{40}(?:[0-9a-f]{24})?\Z")
ERRORS = {"github_rate_limited", "github_not_found", "github_request_rejected",
          "github_redirect_refused", "github_response_invalid", "github_read_too_large",
          "github_text_required", "github_network_unavailable"}


def wire(value):
    return json.dumps(value, ensure_ascii=True, separators=(",", ":")).encode("ascii")


def unique(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError()
        value[key] = item
    return value


def decode(raw):
    return json.loads(raw, object_pairs_hook=unique)


def text(value, limit):
    if (type(value) is not str or not value or value != value.strip()
            or len(value.encode("utf-8")) > limit
            or any(unicodedata.category(char).startswith("C") for char in value)):
        raise ValueError()
    return value


def repository(owner, name):
    if (type(owner) is not str or not OWNER.fullmatch(owner)
            or type(name) is not str or not REPOSITORY.fullmatch(name) or name in (".", "..")):
        raise ValueError()
    return owner + "/" + name


def file_path(value):
    text(value, 1024)
    if (value.startswith("/") or "\\" in value
            or any(part in ("", ".", "..") for part in value.split("/"))):
        raise ValueError()
    return value


def arguments(operation, request):
    if type(request) is not dict:
        raise ValueError()
    if operation == "search":
        if set(request) - {"query", "page"} or "query" not in request:
            raise ValueError()
        query = text(request["query"], 256)
        page = request.get("page", 1)
        if type(page) is not int or not 1 <= page <= PAGE_LIMIT:
            raise ValueError()
        return {"query": query, "page": page}
    if operation != "read" or set(request) - {"owner", "repository", "path", "ref", "release_tag"}:
        raise ValueError()
    repository(request.get("owner"), request.get("repository"))
    value = {"owner": request["owner"], "repository": request["repository"]}
    if "release_tag" in request:
        if "path" in request or "ref" in request:
            raise ValueError()
        value["release_tag"] = file_path(text(request["release_tag"], 256))
        return value
    if "path" in request:
        value["path"] = file_path(request["path"])
    if "ref" in request:
        value["ref"] = text(request["ref"], 256)
    return value


def endpoint(operation, request):
    request = arguments(operation, request)
    if operation == "search":
        return "/search/repositories?" + urllib.parse.urlencode(
            {"q": request["query"], "per_page": PAGE_SIZE, "page": request["page"]})
    root = "/repos/" + urllib.parse.quote(repository(request["owner"], request["repository"]), safe="/")
    if "release_tag" in request:
        return root + "/releases/tags/" + urllib.parse.quote(request["release_tag"], safe="")
    path = root + ("/contents/" + urllib.parse.quote(request["path"], safe="/")
                   if "path" in request else "/readme")
    return path + ("?" + urllib.parse.urlencode({"ref": request["ref"]}) if "ref" in request else "")


class Failure(Exception):
    def __init__(self, code, retry_after=None):
        self.code, self.retry_after = code, retry_after


def fetch(path):
    # http.client does not consult proxy, netrc, cookie or credential stores and
    # does not follow redirects. No response URL is ever used as a new origin.
    connection = http.client.HTTPSConnection("api.github.com", timeout=10,
        context=ssl.create_default_context())
    try:
        connection.request("GET", path, headers={"Accept": "application/vnd.github+json",
            "User-Agent": "meerkat-connect-public-research/0.9.0",
            "X-GitHub-Api-Version": "2022-11-28", "Accept-Encoding": "identity"})
        response = connection.getresponse()
        if 300 <= response.status < 400:
            raise Failure("github_redirect_refused")
        if response.status == 429 or response.status == 403 and (
                response.getheader("X-RateLimit-Remaining") == "0"
                or response.getheader("Retry-After") is not None):
            retry = response.getheader("Retry-After", "60")
            retry = int(retry) if re.fullmatch(r"[0-9]{1,6}", retry) else 60
            raise Failure("github_rate_limited", min(3600, max(1, retry)))
        if response.status == 404:
            raise Failure("github_not_found")
        if response.status != 200:
            raise Failure("github_request_rejected")
        if (response.getheader("Content-Encoding", "identity") != "identity"
                or response.getheader("Content-Type", "").split(";", 1)[0] != "application/json"):
            raise Failure("github_response_invalid")
        raw = response.read(RESPONSE_LIMIT + 1)
        if len(raw) > RESPONSE_LIMIT:
            raise Failure("github_read_too_large")
        return decode(raw)
    finally:
        connection.close()


def release_url(value, host, full, suffix):
    """Validate an observed URL without following it or changing its spelling."""
    value = text(value, 4096)
    url = urllib.parse.urlsplit(value)
    parts = urllib.parse.unquote(url.path, errors="strict").split("/")
    prefix = ["", "repos"] if host == "api.github.com" else [""]
    owner, name = full.split("/")
    if (url.scheme != "https" or url.netloc != host or url.query or url.fragment
            or parts[:len(prefix)] != prefix
            or [part.casefold() for part in parts[len(prefix):len(prefix) + 2]] != [owner.casefold(), name.casefold()]
            or "/".join(parts[len(prefix) + 2:]) != suffix):
        raise ValueError()
    return value


def release_projection(request, value, source):
    """Project only metadata returned for the chosen tag, with no asset choice."""
    full = repository(request["owner"], request["repository"])
    identifier, tag, assets = value.get("id"), value.get("tag_name"), value.get("assets")
    if (type(identifier) is not int or not 0 < identifier < 2 ** 63
            or tag != request["release_tag"] or value.get("draft") is not False
            or type(value.get("prerelease")) is not bool or type(assets) is not list):
        raise ValueError()
    if len(assets) > RELEASE_ASSET_LIMIT:
        raise Failure("github_read_too_large")
    api_url = release_url(value.get("url"), "api.github.com", full, "releases/" + str(identifier))
    html_url = release_url(value.get("html_url"), "github.com", full, "releases/tag/" + tag)
    rows, identifiers, names = [], set(), set()
    for asset in assets:
        if type(asset) is not dict:
            raise ValueError()
        asset_id, size = asset.get("id"), asset.get("size")
        name = text(asset.get("name"), 256)
        if (type(asset_id) is not int or not 0 < asset_id < 2 ** 63 or asset_id in identifiers
                or type(size) is not int or not 0 <= size < 2 ** 63
                or name in names or name in (".", "..") or "/" in name or "\\" in name):
            raise ValueError()
        state = text(asset.get("state"), 64)
        digest = asset.get("digest")
        sha256 = None
        if digest is not None:
            digest = text(digest, 256)
            if digest.startswith("sha256:"):
                if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
                    raise ValueError()
                sha256 = digest[7:]
        rows.append({"id": asset_id, "name": name, "state": state, "bytes": size,
            "api_url": release_url(asset.get("url"), "api.github.com", full, "releases/assets/" + str(asset_id)),
            "download_url": release_url(asset.get("browser_download_url"), "github.com", full,
                "releases/download/" + tag + "/" + name),
            "reported_digest": digest, "sha256": sha256})
        identifiers.add(asset_id)
        names.add(name)
    result = {"scope": "untrusted_public_github_release_metadata", "source_url": source,
        "repository": full, "release_id": identifier, "release_tag": tag,
        "release_api_url": api_url, "release_url": html_url, "prerelease": value["prerelease"],
        "assets": rows, "asset_count": len(rows), "asset_limit": RELEASE_ASSET_LIMIT,
        "asset_scope": "assets_returned_by_release_endpoint", "selection_required": True,
        "digest_provenance": "reported_by_github_not_locally_verified", "assets_downloaded": False}
    if len(wire(result)) > RESPONSE_LIMIT:
        raise Failure("github_read_too_large")
    return result


def projection(operation, request, value):
    if type(value) is not dict:
        raise ValueError()
    source = ORIGIN + endpoint(operation, request)
    if operation == "search":
        total, incomplete, items = value.get("total_count"), value.get("incomplete_results"), value.get("items")
        if (type(total) is not int or total < 0 or type(incomplete) is not bool
                or type(items) is not list or len(items) > PAGE_SIZE):
            raise ValueError()
        rows = []
        for item in items:
            if type(item) is not dict or item.get("private") is not False:
                raise ValueError()
            full = repository(item["owner"]["login"], item["name"])
            if item.get("full_name") != full:
                raise ValueError()
            description = item.get("description") or ""
            if type(description) is not str:
                raise ValueError()
            description = "".join(" " if unicodedata.category(c).startswith("C") else c for c in description)
            description = description.encode("utf-8")[:2048].decode("utf-8", errors="ignore")
            rows.append({"repository": full, "description": description,
                         "url": "https://github.com/" + urllib.parse.quote(full, safe="/")})
        return {"scope": "untrusted_public_github_repository_search", **request,
                "source_url": source, "total_count": total, "incomplete_results": incomplete,
                "page_size": PAGE_SIZE, "max_pages": PAGE_LIMIT, "repositories": rows,
                "selection_required": True}
    if "release_tag" in request:
        return release_projection(request, value, source)
    if value.get("type") != "file" or value.get("encoding") != "base64":
        raise Failure("github_text_required")
    size, sha = value.get("size"), value.get("sha")
    if type(size) is not int or size < 0 or size > TEXT_LIMIT:
        raise Failure("github_read_too_large")
    if type(sha) is not str or not BLOB.fullmatch(sha):
        raise ValueError()
    path = file_path(value.get("path"))
    if "path" in request and path != request["path"]:
        raise ValueError()
    content = value.get("content")
    if type(content) is not str:
        raise ValueError()
    raw = base64.b64decode(content.replace("\n", ""), validate=True)
    if len(raw) != size:
        raise ValueError()
    blob = b"blob " + str(len(raw)).encode("ascii") + b"\0" + raw
    if (hashlib.sha1(blob).hexdigest() if len(sha) == 40 else hashlib.sha256(blob).hexdigest()) != sha:
        raise ValueError()
    try:
        content = raw.decode("utf-8")
    except UnicodeError:
        raise Failure("github_text_required") from None
    if any(unicodedata.category(c).startswith("C") and c not in "\n\r\t" for c in content):
        raise Failure("github_text_required")
    return {"scope": "untrusted_public_github_repository_text", "source_url": source,
            "repository": repository(request["owner"], request["repository"]),
            "ref": request.get("ref"), "path": path, "blob_sha": sha,
            "bytes": len(raw), "content_sha256": hashlib.sha256(raw).hexdigest(), "text": content}


def execute(operation, request):
    request = arguments(operation, request)
    return projection(operation, request, fetch(endpoint(operation, request)))


class PublicGitHub:
    def __init__(self, io, python):
        self.io, self.python = io, python
        self.source = Path(__file__).read_bytes()
        self.sha256 = hashlib.sha256(self.source).hexdigest()
        self.calls = deque()
        self.retry_at = 0.0

    def bind(self, workspace):
        self.workspace = workspace
        directory = workspace / "public-github"
        directory.mkdir(mode=0o700)
        self.worker = directory / "worker.py"
        self.io.write_new(self.worker, self.source)

    def run(self, operation, request, runner):
        try:
            request = arguments(operation, request)
        except (ValueError, UnicodeError, TypeError):
            raise self.io.Error("invalid_github_research_arguments") from None
        now = time.monotonic()
        while self.calls and now - self.calls[0] >= 60:
            self.calls.popleft()
        if now < self.retry_at or len(self.calls) >= 10:
            raise self.io.Error("github_rate_limited", 409)
        if self.io.read_file(self.worker.parent, self.worker.name, 64 * 1024) != self.source:
            raise self.io.Error("github_research_source_changed", 409)
        self.calls.append(now)
        with tempfile.TemporaryDirectory(prefix="github-research-", dir=self.workspace / "tmp") as scratch:
            path = Path(scratch) / "request.json"
            raw = wire({"operation": operation, "request": request})
            self.io.write_new(path, raw)
            code, output = runner.run_compose([self.python, "-I", "-S", "-B", str(self.worker),
                "--request", hashlib.sha256(raw).hexdigest()], input_file=path,
                input_sha256=hashlib.sha256(raw).hexdigest(), output_limit=RESPONSE_LIMIT, timeout=TIMEOUT)
        if self.io.read_file(self.worker.parent, self.worker.name, 64 * 1024) != self.source:
            raise self.io.Error("github_research_source_changed", 409)
        try:
            value = decode(output)
            if code or type(value) is not dict:
                raise ValueError()
            if "error" in value:
                if value["error"] not in ERRORS:
                    raise ValueError()
                retry = value.get("retry_after_seconds", 60)
                if type(retry) is not int or not 1 <= retry <= 3600:
                    raise ValueError()
                if value["error"] == "github_rate_limited":
                    self.retry_at = time.monotonic() + retry
                raise self.io.Error(value["error"], 409)
            if value.get("scope") != ("untrusted_public_github_repository_search" if operation == "search"
                                       else "untrusted_public_github_release_metadata" if "release_tag" in request
                                       else "untrusted_public_github_repository_text"):
                raise ValueError()
            return value
        except (ValueError, TypeError, KeyError, RecursionError, UnicodeError):
            raise self.io.Error("github_research_unconfirmed", 409) from None


def main():
    os.environ.clear()
    try:
        if len(sys.argv) != 3 or sys.argv[1] != "--request":
            raise ValueError()
        raw = sys.stdin.buffer.read(4097)
        if len(raw) > 4096 or hashlib.sha256(raw).hexdigest() != sys.argv[2]:
            raise ValueError()
        value = decode(raw)
        if type(value) is not dict or set(value) != {"operation", "request"}:
            raise ValueError()
        result = execute(value["operation"], value["request"])
    except Failure as error:
        result = {"error": error.code}
        if error.retry_after is not None:
            result["retry_after_seconds"] = error.retry_after
    except (OSError, http.client.HTTPException):
        result = {"error": "github_network_unavailable"}
    except Exception:
        result = {"error": "github_response_invalid"}
    sys.stdout.buffer.write(wire(result))


if __name__ == "__main__":
    main()
