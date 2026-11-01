"""Frozen guide requests and validated results; native console owns admission."""
from __future__ import annotations

import json
import re

from .projects import DraftConflict

MAX_COMMAND_BYTES = 8 * 1024 * 1024
COMMAND_ORIGIN = "launch-slack"


def _encode(value):
    body = json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"), allow_nan=False)
    if len(body.encode()) > MAX_COMMAND_BYTES:
        raise ValueError("The retained guide command exceeds its storage limit.")
    return body


def _table(db):
    db.execute("CREATE TABLE IF NOT EXISTS guide_commands "
               "(command_id TEXT PRIMARY KEY, guide_id TEXT NOT NULL, closed INTEGER NOT NULL, body TEXT NOT NULL)")


def pending_command(store):
    with store._database() as db:
        _table(db)
        row = db.execute("SELECT command_id FROM guide_commands WHERE closed=0 LIMIT 1").fetchone()
    return row["command_id"] if row else None


def command_response(store, command_id, guide_id):
    """Read retained product completion without starting or recovering native work."""
    probe = GuideCommand(store, command_id, guide_id, {})
    with store._database() as db:
        _table(db)
        row = db.execute("SELECT body FROM guide_commands WHERE command_id=? AND guide_id=?",
                         (probe.command_id, probe.guide_id)).fetchone()
    if row is None:
        raise LookupError("The original guide command was not found. This does not prove native non-admission.")
    value = json.loads(row["body"])
    status = "completed" if value["applied"] is not None else "failed" if value["failure"] else "pending"
    return {"command_id": probe.command_id, "guide_id": probe.guide_id, "status": status,
            "response": value["applied"], "failure": value["failure"],
            "native_receipt": value.get("acceptance")}


class GuideCommand:
    def __init__(self, store, command_id, guide_id, request):
        if not isinstance(command_id, str) or not re.fullmatch(r"[a-f0-9]{64}", command_id):
            raise ValueError("Use the original 64-character command ID.")
        self.store, self.command_id = store, command_id
        self.guide_id = store.identity(guide_id)
        self.request = json.loads(_encode(request))

    def _read(self, db):
        row = db.execute("SELECT body FROM guide_commands WHERE command_id=?", (self.command_id,)).fetchone()
        if row is None:
            return None
        value = json.loads(row["body"])
        if value["guide_id"] != self.guide_id or value["request"] != self.request:
            raise DraftConflict("This command ID belongs to another original setup or request.")
        return value

    def load(self):
        with self.store._database() as db:
            _table(db)
            return self._read(db)

    def prepare(self, prepared):
        with self.store._database() as db:
            _table(db)
            db.execute("BEGIN IMMEDIATE")
            value = self._read(db)
            if value is not None:
                return value["prepared"]
            if db.execute("SELECT 1 FROM guide_commands WHERE closed=0 LIMIT 1").fetchone():
                raise DraftConflict("Recover the previous guide command before starting another.")
            value = {"guide_id": self.guide_id, "request": self.request, "prepared": prepared,
                     "native": {}, "publications": {}, "result": None, "failure": None, "applied": None}
            db.execute("INSERT INTO guide_commands VALUES(?,?,0,?)",
                       (self.command_id, self.guide_id, _encode(value)))
        return json.loads(_encode(prepared))

    def _update(self, update, *, close=False):
        with self.store._database() as db:
            _table(db)
            db.execute("BEGIN IMMEDIATE")
            value = self._read(db)
            if value is None:
                raise DraftConflict("The original guide request was not retained.")
            update(value)
            db.execute("UPDATE guide_commands SET body=?,closed=CASE WHEN ? THEN 1 ELSE closed END WHERE command_id=?",
                       (_encode(value), close, self.command_id))
        return value

    def native_request(self, request, runtime_root):
        def retain(value):
            native = {"request": request, "runtime_root": str(runtime_root)}
            if value["native"] and value["native"] != native:
                raise DraftConflict("The original command's native request or runtime store changed.")
            value["native"] = native
        return self._update(retain)["native"]

    def begin_dispatch(self):
        previous = []
        def retain(value):
            previous.append(value.get("dispatch_attempted", False))
            value["dispatch_attempted"] = True
        self._update(retain)
        return previous[0]

    def acceptance(self, accepted):
        binding = {key: accepted.get(key) for key in
                   ("identity", "interaction_id", "input_frame_id", "cursor", "session_id", "conversation_id")}
        def retain(value):
            if value.get("acceptance") and value["acceptance"] != binding:
                raise DraftConflict("The original native command receipt changed. No result can be applied.")
            value["acceptance"] = binding
        self._update(retain)

    def publication(self, token, snapshot):
        def retain(value):
            if token in value["publications"] and value["publications"][token] != snapshot:
                raise DraftConflict("The original guide publication changed.")
            value["publications"][token] = snapshot
            value["last_publication"] = token
        self._update(retain)

    def result(self, result):
        def retain(value):
            if value["result"] is not None and value["result"] != result:
                raise DraftConflict("The original guide result changed.")
            value["result"] = result
        self._update(retain)

    def fail(self, reason):
        self._update(lambda value: value.update(failure=reason), close=True)

    def apply_in(self, db, saved):
        """Save guide completion in the SAME transaction as its guide revision."""
        value = self._read(db)
        if (value is None or value["result"] is None or value["failure"] is not None
                or saved["id"] != self.guide_id
                or saved["version"] != value["request"]["version"] + 1):
            raise DraftConflict("The original command cannot complete this guide revision.")
        value["applied"] = saved
        db.execute("UPDATE guide_commands SET body=?,closed=1 WHERE command_id=?",
                   (_encode(value), self.command_id))
