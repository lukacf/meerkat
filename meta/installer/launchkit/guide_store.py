"""Durable setup interviews; native MobKit still owns agent execution state."""
from __future__ import annotations

from contextlib import contextmanager
import json
import os
from pathlib import Path
import sqlite3
from uuid import UUID, uuid4

from .projects import DraftConflict

MAX_STATE_BYTES = 1_000_000


class GuideStore:
    def __init__(self, root: Path):
        self.root = Path(root)

    @staticmethod
    def identity(value: str):
        try:
            if not isinstance(value, str) or str(UUID(value)) != value:
                raise ValueError
        except (ValueError, AttributeError):
            raise ValueError("Invalid guided setup ID.") from None
        return value

    @contextmanager
    def _database(self):
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(self.root, 0o700)
        path = self.root / "guided-setups.sqlite3"
        # Set the mode before SQLite writes any private interview content.
        descriptor = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
        os.fchmod(descriptor, 0o600)
        os.close(descriptor)
        db = sqlite3.connect(path, timeout=10)
        db.row_factory = sqlite3.Row
        try:
            db.execute("CREATE TABLE IF NOT EXISTS guided_setups (id TEXT PRIMARY KEY, version INTEGER NOT NULL, body TEXT NOT NULL)")
            db.commit()
            with db:
                yield db
        finally:
            db.close()

    @staticmethod
    def _encode(state: dict):
        encoded = json.dumps(state, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False)
        if len(encoded.encode("utf-8")) > MAX_STATE_BYTES:
            raise ValueError("This setup conversation is too large to save. Start a new setup with a shorter summary.")
        return encoded

    def create(self, state: dict, *, request_id: str | None = None):
        if request_id is not None:
            try:
                self.identity(request_id)
            except ValueError:
                raise ValueError("Invalid setup creation request ID.") from None
        saved = {**state, "id": str(uuid4()), "version": 1}
        encoded = self._encode(saved)
        with self._database() as db:
            if request_id is not None:
                # Claim and create together. Concurrent retries must never leave
                # a second interview behind or reset an existing conversation.
                db.execute("BEGIN IMMEDIATE")
                db.execute("CREATE TABLE IF NOT EXISTS guide_creation_requests (request_id TEXT PRIMARY KEY, guide_id TEXT NOT NULL UNIQUE)")
                receipt = db.execute("SELECT guide_id FROM guide_creation_requests WHERE request_id=?", (request_id,)).fetchone()
                if receipt:
                    row = db.execute("SELECT body FROM guided_setups WHERE id=?", (receipt["guide_id"],)).fetchone()
                    if row is None:
                        raise ValueError("The saved setup creation could not be recovered. No new setup was created.")
                    return json.loads(row["body"])
            db.execute("INSERT INTO guided_setups (id,version,body) VALUES (?,?,?)", (saved["id"], 1, encoded))
            if request_id is not None:
                db.execute("INSERT INTO guide_creation_requests (request_id,guide_id) VALUES (?,?)", (request_id, saved["id"]))
        return json.loads(encoded)

    def created_request(self, request_id: str):
        try:
            request_id = self.identity(request_id)
        except ValueError:
            raise ValueError("Invalid setup creation request ID.") from None
        if not (self.root / "guided-setups.sqlite3").exists():
            return None
        with self._database() as db:
            # A readback must not claim a request or create a correlation table
            # for an older store that has never received a correlated start.
            table = db.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='guide_creation_requests'").fetchone()
            if table is None:
                return None
            receipt = db.execute("SELECT guide_id FROM guide_creation_requests WHERE request_id=?", (request_id,)).fetchone()
            if receipt is None:
                return None
            row = db.execute("SELECT body FROM guided_setups WHERE id=?", (receipt["guide_id"],)).fetchone()
            if row is None:
                raise ValueError("The saved setup creation could not be recovered. No new setup was created.")
        return json.loads(row["body"])

    def get(self, identity: str):
        identity = self.identity(identity)
        with self._database() as db:
            row = db.execute("SELECT body FROM guided_setups WHERE id=?", (identity,)).fetchone()
        return json.loads(row["body"]) if row else None

    def list(self):
        with self._database() as db:
            rows = db.execute("SELECT body FROM guided_setups ORDER BY rowid DESC LIMIT 100").fetchall()
        result = []
        for row in rows:
            state = json.loads(row["body"])
            plan = state.get("proposal") or state.get("blueprint") or {}
            messages = state.get("messages", [])
            result.append({"id": state["id"], "name": plan.get("project", {}).get("name") or "New agent system",
                           "stage": state["stage"], "status": state["status"],
                           "updated_at": messages[-1].get("created_at", "") if messages else ""})
        return sorted(result, key=lambda item: item["updated_at"], reverse=True)

    def operation(self, identity):
        identity = self.identity(identity)
        with self._database() as db:
            db.execute("CREATE TABLE IF NOT EXISTS guide_operations (guide_id TEXT PRIMARY KEY, body TEXT NOT NULL)")
            row = db.execute("SELECT body FROM guide_operations WHERE guide_id=?", (identity,)).fetchone()
        return json.loads(row["body"]) if row else None

    def save_operation(self, identity, operation):
        identity = self.identity(identity)
        with self._database() as db:
            db.execute("CREATE TABLE IF NOT EXISTS guide_operations (guide_id TEXT PRIMARY KEY, body TEXT NOT NULL)")
            db.execute("INSERT INTO guide_operations (guide_id,body) VALUES (?,?) ON CONFLICT(guide_id) DO UPDATE SET body=excluded.body",
                       (identity, self._encode(operation)))

    def save_interaction(self, identity, binding):
        """Index a native admission receipt without copying runtime events."""
        identity = self.identity(identity)
        with self._database() as db:
            db.execute("CREATE TABLE IF NOT EXISTS guide_interactions (guide_id TEXT NOT NULL, interaction_id TEXT PRIMARY KEY, body TEXT NOT NULL)")
            db.execute("INSERT INTO guide_interactions (guide_id,interaction_id,body) VALUES (?,?,?) "
                       "ON CONFLICT(interaction_id) DO NOTHING",
                       (identity, binding["interaction_id"], self._encode(binding)))

    def interactions(self, identity):
        identity = self.identity(identity)
        with self._database() as db:
            db.execute("CREATE TABLE IF NOT EXISTS guide_interactions (guide_id TEXT NOT NULL, interaction_id TEXT PRIMARY KEY, body TEXT NOT NULL)")
            rows = db.execute("SELECT body FROM guide_interactions WHERE guide_id=? ORDER BY rowid", (identity,)).fetchall()
        return [json.loads(row["body"]) for row in rows]

    def save(self, state: dict, expected_version: int):
        identity = self.identity(state.get("id"))
        if type(expected_version) is not int or expected_version < 1:
            raise ValueError("Use the current guided setup version when saving.")
        if state.get("version") != expected_version:
            raise DraftConflict("This setup changed. Reload before continuing.")
        saved = {**state, "version": expected_version + 1}
        encoded = self._encode(saved)
        with self._database() as db:
            result = db.execute("UPDATE guided_setups SET version=?,body=? WHERE id=? AND version=?", (saved["version"], encoded, identity, expected_version))
            if result.rowcount != 1:
                raise DraftConflict("This setup changed in another tab. Reload before continuing.")
        return json.loads(encoded)
