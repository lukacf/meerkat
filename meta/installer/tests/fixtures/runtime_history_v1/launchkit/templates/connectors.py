"""Small, scoped connector tools. Secrets are loaded locally at launch."""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import re
import sqlite3
import urllib.error
import urllib.parse
import urllib.request


def load_environment(path, *, override=False):
    """Read dotenv values as data, optionally preferring the saved local settings."""
    if not path.exists():
        return
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        key, separator, value = stripped.partition("=")
        if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
            raise ValueError("Invalid .env entry; expected KEY=value")
        value = value.strip()
        if value.startswith('"'):
            value = json.loads(value)
        elif len(value) >= 2 and value[0] == value[-1] == "'":
            value = value[1:-1]
        if override:
            os.environ[key] = value
        else:
            os.environ.setdefault(key, value)


def schema(properties=None, required=None):
    return {"type": "object", "properties": properties or {},
            "required": required or [], "additionalProperties": False}


TEXT = {"type": "string"}


class ConnectorTools:
    def __init__(self, root, spec):
        self.root = Path(root)
        self.spec = spec
        self.data_dir = self.root / "data"
        self.data_dir.mkdir(mode=0o700, exist_ok=True)
        self.database = self.data_dir / "notes.sqlite3"
        with sqlite3.connect(self.database) as db:
            db.execute("CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, content TEXT NOT NULL, created_at TEXT DEFAULT CURRENT_TIMESTAMP)")
        os.chmod(self.database, 0o600)

    async def close(self):
        pass

    def for_agent(self, agent):
        tools = []
        selected = set(agent.get("connectors", []))
        for connector in self.spec["connectors"]:
            kind = connector["id"]
            if kind not in selected:
                continue
            if kind == "files":
                tools += [
                    self.tool("files_list", self.files_list, "List files in the folder you connected (read-only)."),
                    self.tool("files_read", self.files_read, "Read a UTF-8 file inside the connected folder (maximum 200 KB).", schema({"path": TEXT}, ["path"])),
                ]
            elif kind == "github":
                tools.append(self.tool("github_issues", self.github_issues, "Read issues from the configured GitHub repository.", schema({"state": {"type": "string", "enum": ["open", "closed", "all"]}})))
        tools += [
            self.tool("notes_save", self.notes_save, "Save a useful note in the project's SQLite database.", schema({"content": TEXT}, ["content"])),
            self.tool("notes_search", self.notes_search, "Search notes saved in the project's SQLite database.", schema({"query": TEXT})),
        ]
        return tools

    @staticmethod
    def tool(name, handler, description, input_schema=None):
        return {"name": name, "handler": handler, "description": description, "schema": input_schema or schema()}

    @staticmethod
    def required(key):
        value = os.environ.get(key, "").strip()
        if not value:
            raise ValueError(f"Configure {key} in .env, then restart the app")
        return value

    def files_root(self):
        return Path(os.environ.get("FILES_ROOT") or str(self.root / "workspace")).expanduser().resolve()

    def files_list(self, args):
        folder = self.files_root()
        if not folder.is_dir():
            raise ValueError("Connected folder does not exist. Configure FILES_ROOT in .env")
        entries = []
        for entry in folder.iterdir():
            if entry.name.startswith("."):
                continue
            entries.append({"name": entry.name, "type": "folder" if entry.is_dir() else "file"})
            if len(entries) >= 200:
                break
        return {"entries": sorted(entries, key=lambda item: item["name"]), "limit": 200}

    def files_read(self, args):
        folder = self.files_root()
        relative = str(args.get("path", ""))
        if not relative or Path(relative).is_absolute() or any(part.startswith(".") for part in Path(relative).parts):
            raise ValueError("Choose a visible relative path from the connected folder")
        path = (folder / relative).resolve()
        if not path.is_relative_to(folder) or not path.is_file():
            raise ValueError("File is outside the connected folder or does not exist")
        if any(part.startswith(".") for part in path.relative_to(folder).parts):
            raise ValueError("Hidden files are excluded from the connected folder")
        with path.open("rb") as handle:
            content = handle.read(200_001)
        if len(content) > 200_000:
            raise ValueError("File is larger than the 200 KB read limit")
        try:
            return {"path": relative, "content": content.decode("utf-8"), "source": "connected folder"}
        except UnicodeDecodeError:
            raise ValueError("This tool reads UTF-8 text files. Export documents as text first.") from None

    def notes_save(self, args):
        content = str(args.get("content", "")).strip()
        if not content or len(content) > 10_000:
            raise ValueError("A note must contain 1-10000 characters")
        with sqlite3.connect(self.database) as db:
            cursor = db.execute("INSERT INTO notes(content) VALUES (?)", (content,))
            return {"saved": True, "id": cursor.lastrowid}

    def notes_search(self, args):
        query = str(args.get("query", ""))[:500]
        with sqlite3.connect(self.database) as db:
            db.row_factory = sqlite3.Row
            return {"notes": [dict(row) for row in db.execute(
                "SELECT id, content, created_at FROM notes WHERE instr(lower(content), lower(?)) > 0 ORDER BY id DESC LIMIT 20", (query,))]}

    @staticmethod
    def request(url, headers, payload=None):
        body = json.dumps(payload).encode() if payload is not None else None
        request = urllib.request.Request(url, data=body, headers={"Accept": "application/json", "User-Agent": "MobKit-Launch/1", **headers})
        try:
            with urllib.request.urlopen(request, timeout=20) as response:
                raw = response.read(1_000_001)
            if len(raw) > 1_000_000:
                raise ValueError("Connector response exceeded the 1 MB limit")
            return json.loads(raw)
        except urllib.error.HTTPError as error:
            raise RuntimeError(f"Connector returned HTTP {error.code}. Check its credential and access scope.") from None
        except urllib.error.URLError:
            raise RuntimeError("Connector could not be reached. Check network access and try again.") from None

    async def github_issues(self, args):
        repository = self.required("GITHUB_REPOSITORY")
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
            raise ValueError("GITHUB_REPOSITORY must be owner/repository")
        state = args.get("state", "open")
        if state not in {"open", "closed", "all"}:
            raise ValueError("state must be open, closed or all")
        result = await asyncio.to_thread(self.request,
            f"https://api.github.com/repos/{repository}/issues?per_page=30&state={state}",
            {"Authorization": f"Bearer {self.required('GITHUB_TOKEN')}", "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2026-03-10"})
        return {"issues": [{key: issue.get(key) for key in ("number", "title", "body", "html_url", "state", "pull_request")} for issue in result]}
