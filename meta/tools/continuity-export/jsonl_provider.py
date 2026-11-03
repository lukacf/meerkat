#!/usr/bin/env python3
"""A continuity provider over two JSON Lines files, for continuity_export.py.

``continuity_export.py`` reads a 0.9.3 continuity provider object. A host
whose 0.9.3 continuity records and session snapshots sit in its own tables,
rather than behind a provider object it can import, writes them to two files
and names this module as the provider instead of writing one:

    CONTINUITY_EXPORT_RECORDS=records.jsonl \\
    CONTINUITY_EXPORT_SESSIONS=sessions.jsonl \\
    python3 continuity_export.py --provider jsonl_provider:from_env \\
        --identities-file identities.txt --output continuity-export.jsonl

Keep this file beside ``continuity_export.py``, whose directory is importable.
``jsonl-provider.mjs`` is the same provider for ``continuity-export.mjs``: it
reads the same files by the same rules.

Both files are UTF-8 JSON Lines, one JSON object per line. A byte order mark
at the start of a file and lines that hold only whitespace are skipped.

- The records file holds each identity's current 0.9.3 continuity record, one
  line per identity, with exactly the record's five keys: ``identity``,
  ``agent_runtime_id``, ``session_id``, ``generation`` and
  ``checkpoint_version``.
- The sessions file holds session snapshots, one line per session, with
  exactly the keys ``session_id`` and ``data``: the snapshot's exact bytes in
  standard base64, with padding and without line breaks.

``generation`` and ``checkpoint_version`` may be JSON integers or strings of
decimal digits, the form some SQL tools write 64-bit integers in. A session id
may be in any spelling 0.9.3 accepted. Values are otherwise handed to the
export as the file holds them, and the export checks them by the 0.9.3
gateway's rules.

``resolve_many`` answers for every identity the records file holds, listed or
not, so an identity list that misses one is refused (``Unlisted``) instead of
exporting less; a listed identity the file holds no record for is
``uninitialized``. ``load_session_snapshot`` answers from the sessions file.
The export asks only for each identity's current session, the one its record
names, so a session no record names is never exported.
``resolve_record_by_session`` answers from the records file, with no fencing
token (``None``): the files carry none, and the export does not read it.

A file this provider cannot use (a line that is not a JSON object, a missing,
unexpected or repeated key, a repeated identity or session, a session id or a
``data`` value it cannot read) stops the export before it reads a record, and
the error names the file and the line. Opening the provider reads the records
file and scans the sessions file once. It keeps the records and where each
session's line starts, not the snapshots, and reads one snapshot when the
export asks for it.

Standard library only; Python 3.10 or later.
"""
from __future__ import annotations

import json
import os
import re
from typing import Any, Iterator, NoReturn

RECORDS_ENV = "CONTINUITY_EXPORT_RECORDS"
SESSIONS_ENV = "CONTINUITY_EXPORT_SESSIONS"
RECORD_KEYS = ("identity", "agent_runtime_id", "session_id", "generation", "checkpoint_version")
SESSION_KEYS = ("session_id", "data")

_INTEGER_FIELDS = ("generation", "checkpoint_version")
_DECIMAL_DIGITS = re.compile(r"0|[1-9][0-9]*")
# Every spelling Core 0.9.3's SessionId parsed: simple, hyphenated, braced and
# urn, with hex digits in either case (continuity_export.py has the same rule).
_HYPHENATED = r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}"
_SESSION_ID_SPELLINGS = re.compile(
    rf"[0-9A-Fa-f]{{32}}|{_HYPHENATED}|\{{{_HYPHENATED}\}}|urn:uuid:{_HYPHENATED}"
)
# Padded standard base64 (the length is checked apart).
_BASE64 = re.compile(r"[A-Za-z0-9+/]*={0,2}")
_BOM = b"\xef\xbb\xbf"
_JSON_WHITESPACE = b" \t\r\n"


class JsonlProviderInputError(ValueError):
    """A records or sessions file the provider cannot use."""


class JsonlContinuityProvider:
    """The 0.9.3 read methods over a records file and a sessions file."""

    def __init__(self, records_path: str | os.PathLike[str], sessions_path: str | os.PathLike[str]) -> None:
        self._records_path = os.fspath(records_path)
        self._sessions_path = os.fspath(sessions_path)
        self._records: dict[str, dict[str, Any]] = {}
        self._identities_by_session: dict[str, list[str]] = {}
        # Canonical session id -> (offset, length, line number) of its line.
        self._sessions: dict[str, tuple[int, int, int]] = {}
        self._read_records()
        self._index_sessions()

    # -- the 0.9.3 read methods ----------------------------------------------

    def resolve_many(self, identities: list[str]) -> dict[str, dict[str, Any]]:
        answer: dict[str, dict[str, Any]] = {
            identity: {"state": "ready", "record": dict(record)}
            for identity, record in self._records.items()
        }
        for identity in identities:
            answer.setdefault(identity, {"state": "uninitialized"})
        return answer

    def load_session_snapshot(self, session_id: str) -> dict[str, str] | None:
        canonical = canonical_session_id(session_id)
        place = self._sessions.get(canonical) if canonical is not None else None
        if place is None:
            return None
        offset, length, number = place
        with open(self._sessions_path, "rb") as handle:
            handle.seek(offset)
            raw = handle.read(length)
        try:
            session = _session_line(self._sessions_path, number, raw)
        except JsonlProviderInputError:
            session = None
        if session is None or canonical_session_id(session["session_id"]) != canonical:
            _fail(self._sessions_path, number, "the file changed after the provider opened it")
        return {"data": session["data"]}

    def resolve_record_by_session(self, session_id: str) -> tuple[dict[str, Any], None, Any] | None:
        canonical = canonical_session_id(session_id)
        bound = self._identities_by_session.get(canonical, []) if canonical is not None else []
        if len(bound) != 1:
            return None
        record = self._records[bound[0]]
        return dict(record), None, record["checkpoint_version"]

    # -- reading the files ----------------------------------------------------

    def _read_records(self) -> None:
        lines: dict[str, int] = {}
        for number, _offset, raw in _lines(self._records_path):
            record = _object_line(self._records_path, number, raw, RECORD_KEYS)
            if record is None:
                continue
            identity = record["identity"]
            if not isinstance(identity, str):
                _fail(self._records_path, number, "identity is not a string")
            if identity in lines:
                _fail(self._records_path, number,
                      f"identity {_quote(identity)} is repeated (first on line {lines[identity]})")
            lines[identity] = number
            for field in _INTEGER_FIELDS:
                value = record[field]
                if isinstance(value, str) and _DECIMAL_DIGITS.fullmatch(value):
                    record[field] = int(value)
            self._records[identity] = {key: record[key] for key in RECORD_KEYS}
            canonical = canonical_session_id(record["session_id"])
            if canonical is not None:
                self._identities_by_session.setdefault(canonical, []).append(identity)

    def _index_sessions(self) -> None:
        for number, offset, raw in _lines(self._sessions_path):
            session = _session_line(self._sessions_path, number, raw)
            if session is None:
                continue
            canonical = canonical_session_id(session["session_id"])
            if canonical in self._sessions:
                _fail(self._sessions_path, number,
                      f"session {canonical} is repeated (first on line {self._sessions[canonical][2]})")
            self._sessions[canonical] = (offset, len(raw), number)


def from_env() -> JsonlContinuityProvider:
    """The provider over the files ``CONTINUITY_EXPORT_RECORDS`` and
    ``CONTINUITY_EXPORT_SESSIONS`` name."""
    paths = []
    for name in (RECORDS_ENV, SESSIONS_ENV):
        value = os.environ.get(name, "")
        if not value:
            raise JsonlProviderInputError(f"{name} is not set: set it to the path of the "
                                          f"{'records' if name == RECORDS_ENV else 'sessions'} file")
        paths.append(value)
    return JsonlContinuityProvider(*paths)


def canonical_session_id(value: Any) -> str | None:
    """A session id in any spelling 0.9.3 accepted, in canonical form; else ``None``."""
    if not isinstance(value, str) or not _SESSION_ID_SPELLINGS.fullmatch(value):
        return None
    digits = value.removeprefix("urn:uuid:").strip("{}").replace("-", "").lower()
    return f"{digits[:8]}-{digits[8:12]}-{digits[12:16]}-{digits[16:20]}-{digits[20:]}"


def _lines(path: str) -> Iterator[tuple[int, int, bytes]]:
    """Each line of ``path`` as (line number, offset, bytes), its line end kept.

    A byte order mark at the start of the file is not part of the first line.
    """
    with open(path, "rb") as handle:
        offset = 0
        for number, raw in enumerate(handle, start=1):
            start = offset
            offset += len(raw)
            if number == 1 and raw.startswith(_BOM):
                raw = raw[len(_BOM):]
                start += len(_BOM)
            yield number, start, raw


def _session_line(path: str, number: int, raw: bytes) -> dict[str, Any] | None:
    session = _object_line(path, number, raw, SESSION_KEYS)
    if session is None:
        return None
    if canonical_session_id(session["session_id"]) is None:
        _fail(path, number, f"session_id {_quote(session['session_id'])} is not a session id")
    data = session["data"]
    if not isinstance(data, str) or len(data) % 4 != 0 or not _BASE64.fullmatch(data):
        _fail(path, number, "data is not standard base64 with padding and without line breaks")
    return session


def _object_line(path: str, number: int, raw: bytes, keys: tuple[str, ...]) -> dict[str, Any] | None:
    """The line's JSON object, checked to hold exactly ``keys``; ``None`` for a
    line of whitespace."""
    if not raw.strip(_JSON_WHITESPACE):
        return None
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        _fail(path, number, "the line is not valid UTF-8")

    def constant(name: str) -> NoReturn:
        raise ValueError(f"{name} is not a JSON number")

    try:
        value = json.loads(text, object_pairs_hook=_Object.of, parse_constant=constant)
    except RecursionError:
        _fail(path, number, "the line is nested too deeply to read")
    except ValueError as error:
        _fail(path, number, f"the line is not JSON: {error}")
    if not isinstance(value, dict):
        _fail(path, number, "the line is not a JSON object")
    if value.repeated is not None:
        _fail(path, number, f"key {_quote(value.repeated)} is repeated")
    missing = [key for key in keys if key not in value]
    unexpected = sorted(key for key in value if key not in keys)
    if missing or unexpected:
        problems = []
        if missing:
            problems.append(f"missing {', '.join(missing)}")
        if unexpected:
            problems.append(f"unexpected {', '.join(map(_quote, unexpected))}")
        _fail(path, number, f"the object must have exactly the keys {', '.join(keys)} "
                            f"({'; '.join(problems)})")
    return value


class _Object(dict):
    """A parsed JSON object that remembers its first repeated key (``json``
    keeps the last value of a repeated key, as JSON.parse does)."""

    repeated: str | None = None

    @classmethod
    def of(cls, pairs: list[tuple[str, Any]]) -> _Object:
        result = cls()
        for key, value in pairs:
            if key in result and result.repeated is None:
                result.repeated = key
            result[key] = value
        return result


def _quote(value: Any) -> str:
    """``value`` in an error message, as JSON (the Node.js provider's form)."""
    return json.dumps(value, ensure_ascii=False)


def _fail(path: str, number: int, problem: str) -> NoReturn:
    raise JsonlProviderInputError(f"{path} line {number}: {problem}")
