"""Versioned deployment drafts, separate from native runtime/session authority."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
from contextlib import contextmanager
from datetime import datetime, timezone


def fingerprint(value: dict) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


class DraftConflict(ValueError):
    pass


class ProjectStore:
    def __init__(self, root: Path):
        self.root = root

    @contextmanager
    def _database(self):
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        path = self.root / "blueprints.sqlite3"
        db = sqlite3.connect(path, timeout=10)
        db.row_factory = sqlite3.Row
        db.execute("CREATE TABLE IF NOT EXISTS drafts (slug TEXT PRIMARY KEY, name TEXT NOT NULL, revision INTEGER NOT NULL, body TEXT NOT NULL, updated_at TEXT NOT NULL)")
        db.execute("CREATE TABLE IF NOT EXISTS revisions (slug TEXT NOT NULL, revision INTEGER NOT NULL, body TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY (slug,revision))")
        db.commit()
        os.chmod(path, 0o600)
        try:
            with db:
                yield db
        finally:
            db.close()

    @staticmethod
    def slug(value: str):
        if not re.fullmatch(r"[a-z][a-z0-9-]{1,39}", value):
            raise ValueError("Invalid project slug")
        return value

    def list(self):
        with self._database() as db:
            rows = db.execute("SELECT slug,name,revision,updated_at FROM drafts ORDER BY updated_at DESC").fetchall()
        return [dict(row) for row in rows]

    def get(self, slug: str, revision: int | None = None):
        self.slug(slug)
        with self._database() as db:
            if revision is None:
                row = db.execute("SELECT * FROM drafts WHERE slug=?", (slug,)).fetchone()
            else:
                row = db.execute("SELECT * FROM revisions WHERE slug=? AND revision=?", (slug, revision)).fetchone()
        if row is None:
            return None
        body = json.loads(row["body"])
        return {"blueprint": body, "revision": row["revision"], "updated_at": row["updated_at"], "hash": fingerprint(body)}

    def save(self, blueprint: dict, expected_revision: int | None = None):
        slug = self.slug(blueprint["project"]["slug"])
        encoded = json.dumps(blueprint, sort_keys=True, separators=(",", ":"))
        with self._database() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute("SELECT revision,body FROM drafts WHERE slug=?", (slug,)).fetchone()
            actual = row["revision"] if row else 0
            if expected_revision is not None and actual != expected_revision:
                raise DraftConflict(f"The project changed from revision {expected_revision} to {actual}. Reload before saving.")
            if row and row["body"] == encoded:
                return self.get(slug)
            revision = actual + 1
            timestamp = datetime.now(timezone.utc).isoformat()
            db.execute("INSERT INTO revisions VALUES (?,?,?,?)", (slug, revision, encoded, timestamp))
            db.execute("INSERT INTO drafts VALUES (?,?,?,?,?) ON CONFLICT(slug) DO UPDATE SET name=excluded.name,revision=excluded.revision,body=excluded.body,updated_at=excluded.updated_at", (slug, blueprint["project"]["name"], revision, encoded, timestamp))
        return self.get(slug)

    def history(self, slug: str):
        self.slug(slug)
        with self._database() as db:
            rows = db.execute("SELECT revision,updated_at FROM revisions WHERE slug=? ORDER BY revision DESC LIMIT 100", (slug,)).fetchall()
        return [dict(row) for row in rows]
