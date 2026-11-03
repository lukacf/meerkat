#!/usr/bin/env python3
"""Export a 0.9.3 SDK-hosted continuity store into a continuity export file.

Meta 0.9.4 keeps session and identity truth in Core's local stores, so the
0.9.3 SDK continuity provider (``continuity_store`` on the 0.9.3 builder) has no
0.9.4 runtime. A host whose continuity lived behind such a provider:

1. stops every 0.9.3 gateway that uses the store;
2. runs this script once, in its 0.9.3 environment, against its existing
   provider object;
3. upgrades to 0.9.4;
4. imports the file with ``meta_gateway storage-migrate-native``.

This script is standalone: it needs Python 3.10 or later and the standard
library, and imports nothing from the meerkat-meta SDK of either version. The
export calls only the 0.9.3 read methods: ``resolve_many``,
``load_session_snapshot`` and, when the provider has it,
``resolve_record_by_session``. It reads their results the way the 0.9.3 SDK
forwarded them to the gateway (``to_dict()`` on each value, or a mapping
already in that shape) and checks them with the gateway's rules. It writes
nothing to the provider, and it cannot see changes made after it finishes.

When any listed identity cannot be exported as it stands, the whole export is
refused, no file is written, and every such identity is reported with its
class and the action that clears it. A provider exception, from a read method
or from reading the value it returned, aborts the export; it is never written
down as a fact, and the export can be run again.

A ready record whose session the provider holds no snapshot for is refused
(RuntimeMissing) unless ``--missing-snapshot fresh`` is given. A 0.9.3 host
built with the Python or TypeScript SDK never saved a snapshot to its
provider: its gateway kept sessions in memory and started every identity
fresh at each restart. With ``fresh`` the export writes each such identity
as ``uninitialized``, so it starts fresh after the upgrade as well, with an
empty transcript, and reports it with its 0.9.3 record.

Usage:

    python3 continuity_export.py --provider MODULE:ATTRIBUTE \\
        (--identity ID ... | --identities-file FILE) --output FILE \\
        [--missing-snapshot refuse|fresh]
    python3 continuity_export.py --verify FILE [FILE ...]

``--provider`` names the provider object, or a function returning it (it may
be async), as ``module.path:attribute``; the current directory is importable.
List every identity the store holds a record for, including retired
identities and identities the roster no longer lists: the 0.9.3 protocol
cannot enumerate them, and only the listed identities are exported. An
identities file is UTF-8 with one identity per line; a byte order mark at
its start, whitespace around an identity and blank lines are ignored. Only
each identity's current session is exported.

Exit status: 0 written or verified; 1 refused (each row is printed); 2 not
started or failed locally (a usage error, or the output exists or cannot be
written); 3 the provider failed (its module, its factory, a read or the value
a read returned; run it again); 4 a file failed verification.

File format, version 1. The file is UTF-8, and every line ends in a newline:

- line 1, the header: ``{"format": "meerkat-meta.continuity-export",
  "version": 1, "writer": <name>, "entries": <count>, "entries_length":
  <bytes>, "entries_sha256": <hex>}``;
- then one line per listed identity, its entry, in compact JSON.

``entries`` counts the entry lines, ``entries_length`` is the number of bytes
after the header line, and ``entries_sha256`` is their SHA-256. Entries are
sorted by identity in code point order and are unique. An entry line is at
most 536,870,888 bytes, the longest string Node.js holds, so that either
script reads any file; a session whose entry would be longer is refused
(TooLarge). A ``ready`` entry is
``{"identity", "state", "record", "document"}``: ``record`` carries the 0.9.3
continuity record's five fields, with ``session_id`` in canonical form
(lowercase and hyphenated, the form 0.9.3 handed the provider), and
``document`` is ``{"length", "sha256", "base64"}`` over the exact bytes
``load_session_snapshot`` returned.
An ``uninitialized`` entry is ``{"identity", "state"}``. Documents are never
decoded here; the migration verb decodes and checks them.

Both directions stream. The export writes each entry to a temporary file as
soon as its document is read, then writes the file from there and verifies
it, so its memory follows the largest document (up to about 20 times its
size), not the store, and it needs free space for about twice the file.
Reading a file holds one entry at a time.
"""
from __future__ import annotations

import argparse
import asyncio
import base64
import binascii
import contextlib
import hashlib
import importlib
import inspect
import json
import os
import re
import secrets
import shutil
import sys
from collections.abc import Awaitable, Callable, Iterable, Mapping
from dataclasses import dataclass, replace
from typing import Any, NoReturn

CONTINUITY_EXPORT_FORMAT = "meerkat-meta.continuity-export"
CONTINUITY_EXPORT_VERSION = 1

WRITER = "python"
_HEADER_KEYS = frozenset(
    {"format", "version", "writer", "entries", "entries_length", "entries_sha256"}
)
_READY_KEYS = frozenset({"identity", "state", "record", "document"})
_UNINITIALIZED_KEYS = frozenset({"identity", "state"})
_RECORD_KEYS = frozenset(
    {"identity", "agent_runtime_id", "session_id", "generation", "checkpoint_version"}
)
_DOCUMENT_KEYS = frozenset({"length", "sha256", "base64"})
_U64_MAX = 2**64 - 1
# The longest entry line, without its newline, either script writes or reads:
# the longest string Node.js can hold (buffer.constants.MAX_STRING_LENGTH on
# 64-bit platforms), so that the Node.js script holds any entry line whole.
MAX_ENTRY_LINE_BYTES = 536_870_888
_MAX_HEADER_LINE_BYTES = 65_536
_COPY_CHUNK_BYTES = 1 << 20
# The form an export file holds: SessionId's Display, lowercase and hyphenated.
_SESSION_ID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
# Every spelling Core 0.9.3's SessionId parsed (the uuid crate): simple,
# hyphenated, braced and urn, with hex digits in either case. The 0.9.3
# gateway handed the provider's session methods the canonical form.
_HYPHENATED = r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}"
_SESSION_ID_SPELLINGS = re.compile(
    rf"[0-9A-Fa-f]{{32}}|{_HYPHENATED}|\{{{_HYPHENATED}\}}|urn:uuid:{_HYPHENATED}"
)
_SHA256_HEX = re.compile(r"[0-9a-f]{64}")
# The Unicode White_Space set (Rust char::is_whitespace), which AgentIdentity
# and AgentRuntimeId refuse along with the empty string and "/".
_WHITE_SPACE = frozenset(
    "\t\n\x0b\x0c\r \x85\xa0\u1680"
    "\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a"
    "\u2028\u2029\u202f\u205f\u3000"
)
_WHITE_SPACE_CHARACTERS = "".join(sorted(_WHITE_SPACE))
_BOM = "\ufeff"

# What the export does with a ready record whose session has no snapshot:
# refuse it (RuntimeMissing), or write the identity as uninitialized.
MISSING_SNAPSHOT_REFUSE = "refuse"
MISSING_SNAPSHOT_FRESH = "fresh"
MISSING_SNAPSHOT_POLICIES = (MISSING_SNAPSHOT_REFUSE, MISSING_SNAPSHOT_FRESH)

# Refusal classes, in the migration verb's doctor vocabulary, and the action
# that clears each.
BROKEN = "Broken"
RUNTIME_MISSING = "RuntimeMissing"
IDENTITY_DISAGREES = "IdentityDisagrees"
UNREADABLE = "Unreadable"
UNLISTED = "Unlisted"
TOO_LARGE = "TooLarge"
ACTIONS = {
    BROKEN: "settle or delete the identity with the 0.9.3 binary, then rerun the export",
    RUNTIME_MISSING: (
        "if a 0.9.3 host built with the Python or TypeScript SDK wrote the store, it never "
        "saved snapshots: rerun the export with --missing-snapshot fresh, which writes the "
        "identity as uninitialized, so it starts fresh with an empty transcript, as at every "
        "0.9.3 restart; otherwise restore the session's snapshot, or delete the identity with "
        "the 0.9.3 binary, then rerun the export"
    ),
    IDENTITY_DISAGREES: (
        "settle the identity's record with the 0.9.3 binary (or delete the identity), "
        "then rerun the export"
    ),
    UNREADABLE: "make the provider answer in the 0.9.3 wire shape, then rerun the export",
    UNLISTED: "add the identity to the identity list, then rerun the export",
    TOO_LARGE: (
        "reset or delete the identity with the 0.9.3 binary (either drops its session), "
        "then rerun the export"
    ),
}


# ---------------------------------------------------------------------------
# Result types
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class ContinuityExportRecord:
    """The 0.9.3 continuity record of an exported identity; ``session_id`` is
    in canonical form, lowercase and hyphenated."""

    identity: str
    agent_runtime_id: str
    session_id: str
    generation: int
    checkpoint_version: int


@dataclass(frozen=True)
class ContinuityExportEntry:
    """One listed identity.

    ``state`` is ``"ready"`` (``record``, ``document_length`` and
    ``document_sha256`` describe the session and its verified document) or
    ``"uninitialized"`` (the provider holds no continuity for the identity;
    all three are ``None``). The document bytes stay in the file.
    """

    identity: str
    state: str
    record: ContinuityExportRecord | None = None
    document_length: int | None = None
    document_sha256: str | None = None


@dataclass(frozen=True)
class ContinuityExport:
    """A verified continuity export file.

    ``missing_snapshots`` holds, for an export run with
    ``missing_snapshot="fresh"``, the 0.9.3 records whose session the provider
    held no snapshot for; the file lists those identities as
    ``uninitialized``. It is empty for a file read back.
    """

    path: str
    version: int
    writer: str
    entries_sha256: str
    entries: tuple[ContinuityExportEntry, ...]
    missing_snapshots: tuple[ContinuityExportRecord, ...] = ()


@dataclass(frozen=True)
class ContinuityExportRefusal:
    """A listed identity the export cannot carry: its class and the action that clears it."""

    row_class: str
    identity: str | None
    detail: str
    session_id: str | None = None

    @property
    def action(self) -> str:
        return ACTIONS[self.row_class]


# ---------------------------------------------------------------------------
# Errors
# ---------------------------------------------------------------------------


class ContinuityExportError(Exception):
    """Base class of the continuity export errors."""


class ContinuityExportRefusedError(ContinuityExportError):
    """Listed identities cannot be exported as they stand; no file was written."""

    def __init__(self, refusals: Iterable[ContinuityExportRefusal]):
        self.refusals: tuple[ContinuityExportRefusal, ...] = tuple(refusals)
        rows = "; ".join(
            f"{refusal.identity if refusal.identity is not None else '-'}: "
            f"{refusal.row_class} ({refusal.detail})"
            for refusal in self.refusals
        )
        super().__init__(f"continuity export refused, nothing written: {rows}")


class ContinuityExportProviderError(ContinuityExportError):
    """A provider read failed; nothing was written and the export can be run again.

    ``operation`` names the provider method, ``subject`` the session it was
    asked about (``None`` for ``resolve_many``), and ``__cause__`` is the
    provider's exception: raised by the method, or by the value it returned
    while the export read it.
    """

    retryable = True

    def __init__(self, operation: str, subject: str | None, cause: BaseException):
        self.operation = operation
        self.subject = subject
        about = f" for session {subject}" if subject is not None else ""
        super().__init__(
            f"continuity provider {operation}{about} failed, nothing written; "
            f"run the export again: {cause!r}"
        )


class ContinuityExportFileError(ContinuityExportError):
    """A file is not an intact version 1 continuity export."""

    def __init__(self, path: str, reason: str):
        self.path = path
        self.reason = reason
        super().__init__(f"continuity export file {path} refused: {reason}")


# ---------------------------------------------------------------------------
# Export
# ---------------------------------------------------------------------------


async def export_continuity(
    provider: Any,
    identities: Iterable[str],
    path: str | os.PathLike[str],
    *,
    missing_snapshot: str = MISSING_SNAPSHOT_REFUSE,
) -> ContinuityExport:
    """Export the provider's continuity for ``identities`` into a new file at ``path``.

    ``path`` must not exist. Each entry is written to a temporary file in the
    same directory as soon as its document is read; the export file is then
    written next to it under a temporary name, verified, and given its name.
    Only its owner can read it, because it holds full transcripts.

    ``missing_snapshot`` decides a ready record whose session the provider
    holds no snapshot for: ``"refuse"`` (the default) refuses it as
    RuntimeMissing; ``"fresh"`` writes the identity as ``uninitialized`` and
    returns its record in ``missing_snapshots``.

    Raises:
        TypeError, ValueError: a malformed identity list, an unknown
            ``missing_snapshot``, or a provider without the 0.9.3 read
            methods. The provider is not called.
        FileExistsError: ``path`` exists, or a file appeared there while the
            export ran; it is never replaced.
        ContinuityExportRefusedError: identities that cannot be exported.
        ContinuityExportProviderError: a provider read, or reading the value
            it returned, failed (retryable).
    """
    if missing_snapshot not in MISSING_SNAPSHOT_POLICIES:
        raise ValueError(
            f"missing_snapshot must be {' or '.join(map(repr, MISSING_SNAPSHOT_POLICIES))}, "
            f"not {missing_snapshot!r}"
        )
    requested = _requested_identities(identities)
    resolve_many = _provider_method(provider, "resolve_many")
    load_session_snapshot = _provider_method(provider, "load_session_snapshot")
    resolve_record_by_session = getattr(provider, "resolve_record_by_session", None)
    if not callable(resolve_record_by_session):
        resolve_record_by_session = None
    target = _path_string(path)
    if os.path.lexists(target):
        raise _target_exists(target)
    stem = os.path.join(
        os.path.dirname(os.path.abspath(target)),
        f".{os.path.basename(target)}.{os.getpid()}.{secrets.token_hex(8)}",
    )
    entries = _EntryFile(stem + ".entries.partial")
    try:
        return await _export_entries(
            requested, resolve_many, load_session_snapshot, resolve_record_by_session,
            missing_snapshot, entries, target, stem + ".partial",
        )
    finally:
        entries.discard()


async def _export_entries(
    requested: tuple[str, ...],
    resolve_many: Callable[..., Any],
    load_session_snapshot: Callable[..., Any],
    resolve_record_by_session: Callable[..., Any] | None,
    missing_snapshot: str,
    entries: _EntryFile,
    target: str,
    partial: str,
) -> ContinuityExport:
    answer = await _provider_call("resolve_many", None, resolve_many, list(requested))
    if not isinstance(answer, Mapping):
        raise ContinuityExportRefusedError(
            [ContinuityExportRefusal(UNREADABLE, None, "resolve_many did not return a mapping")]
        )
    refusals, records = _provider_value(
        "resolve_many", None, _read_resolve_answer, requested, answer
    )

    owners: dict[str, list[str]] = {}
    for identity, record in records.items():
        owners.setdefault(record.session_id, []).append(identity)
    for session_id, bound in owners.items():
        if len(bound) > 1:
            for identity in bound:
                others = ", ".join(other for other in sorted(bound) if other != identity)
                refusals.append(
                    ContinuityExportRefusal(
                        IDENTITY_DISAGREES,
                        identity,
                        f"session {session_id} is also bound to {others}",
                        session_id,
                    )
                )
                del records[identity]

    # Entries are written in file order as their documents are read, so at
    # most one document is held. After the first refusal nothing more is
    # written, but every document is still read, to report every refusal.
    missing: list[ContinuityExportRecord] = []
    for identity in sorted(requested):
        record = records.get(identity)
        if record is None:
            if not refusals:
                entries.write(_uninitialized_line(identity))
            continue
        session_id = record.session_id
        snapshot = await _provider_call(
            "load_session_snapshot", session_id, load_session_snapshot, session_id
        )
        if snapshot is None:
            if missing_snapshot == MISSING_SNAPSHOT_FRESH:
                # The entry an identity without a record gets: it starts fresh.
                missing.append(record)
                if not refusals:
                    entries.write(_uninitialized_line(identity))
                continue
            refusals.append(
                ContinuityExportRefusal(
                    RUNTIME_MISSING,
                    identity,
                    f"the provider holds no snapshot for session {session_id} "
                    f"({_record_values(record)})",
                    session_id,
                )
            )
            continue
        document = _provider_value(
            "load_session_snapshot", session_id, _snapshot_bytes, snapshot
        )
        del snapshot
        if isinstance(document, str):
            refusals.append(ContinuityExportRefusal(UNREADABLE, identity, document, session_id))
            continue
        if resolve_record_by_session is not None:
            bound = await _provider_call(
                "resolve_record_by_session",
                session_id,
                resolve_record_by_session,
                session_id,
            )
            disagreement = _provider_value(
                "resolve_record_by_session",
                session_id,
                _session_index_refusal,
                identity,
                session_id,
                bound,
            )
            if disagreement is not None:
                refusals.append(disagreement)
                continue
        line = _ready_entry_line(record, document)
        del document
        if isinstance(line, ContinuityExportRefusal):
            refusals.append(line)
        elif not refusals:
            entries.write(*line)

    if refusals:
        refusals.sort(key=lambda refusal: (refusal.identity or "", refusal.row_class))
        raise ContinuityExportRefusedError(refusals)
    export = _write_export(target, entries, partial)
    return replace(export, missing_snapshots=tuple(missing))


def read_continuity_export(path: str | os.PathLike[str]) -> ContinuityExport:
    """Read and verify a continuity export file.

    Raises :class:`ContinuityExportFileError` unless the file is an intact
    version 1 export: the header, the count, length and SHA-256 of the entry
    lines and every entry are checked, and documents must match their
    recorded length and SHA-256. The file is read one line at a time.
    """
    target = _path_string(path)
    with open(target, "rb") as handle:
        return _read_export(target, handle)


# ---------------------------------------------------------------------------
# Reading provider values (the 0.9.3 wire shapes)
# ---------------------------------------------------------------------------


def _requested_identities(identities: Iterable[str]) -> tuple[str, ...]:
    if isinstance(identities, (str, bytes)):
        raise TypeError("identities must be a list of identity strings, not one string")
    requested = tuple(identities)
    if not requested:
        raise ValueError("identities must name at least one identity")
    seen: set[str] = set()
    for identity in requested:
        if not isinstance(identity, str):
            raise TypeError(f"identity {identity!r} is not a string")
        problem = _identity_problem(identity)
        if problem is not None:
            raise ValueError(f"identity {identity!r} {problem}")
        if identity in seen:
            raise ValueError(f"identity {identity!r} is listed twice")
        seen.add(identity)
    return requested


def _provider_method(provider: Any, name: str) -> Callable[..., Any]:
    method = getattr(provider, name, None)
    if not callable(method):
        raise TypeError(f"the continuity provider has no {name} method (a 0.9.3 read method)")
    return method


async def _provider_call(
    operation: str, subject: str | None, method: Callable[..., Any], *args: Any
) -> Any:
    try:
        result = method(*args)
        if inspect.isawaitable(result):
            result = await _await(result)
    except Exception as error:
        raise ContinuityExportProviderError(operation, subject, error) from error
    return result


async def _await(value: Awaitable[Any]) -> Any:
    return await value


def _provider_value(
    operation: str, subject: str | None, read: Callable[..., Any], *args: Any
) -> Any:
    """``read(*args)`` over a value ``operation`` returned. Reading it runs
    provider code too (``to_dict()``, properties, mapping methods), so what
    that raises is a provider fault, like a failed call."""
    try:
        return read(*args)
    except Exception as error:
        raise ContinuityExportProviderError(operation, subject, error) from error


def _wire_value(value: Any) -> Any:
    """The value as the 0.9.3 SDK forwarded it to the gateway."""
    if isinstance(value, Mapping):
        return value
    to_dict = getattr(value, "to_dict", None)
    if callable(to_dict):
        return to_dict()
    return value


def _read_resolve_answer(
    requested: tuple[str, ...], answer: Mapping[Any, Any]
) -> tuple[list[ContinuityExportRefusal], dict[str, ContinuityExportRecord]]:
    """The refusals and the records in resolve_many's answer."""
    refusals: list[ContinuityExportRefusal] = []
    requested_set = set(requested)
    for key in answer:
        if not isinstance(key, str) or key not in requested_set:
            refusals.append(
                ContinuityExportRefusal(
                    UNLISTED,
                    str(key),
                    "resolve_many answered for an identity that is not in the identity list",
                )
            )

    records: dict[str, ContinuityExportRecord] = {}
    for identity in requested:
        if identity not in answer:
            refusals.append(
                ContinuityExportRefusal(
                    UNREADABLE, identity, "resolve_many returned no entry for this identity"
                )
            )
            continue
        outcome = _parse_resolve_state(identity, answer[identity])
        if isinstance(outcome, ContinuityExportRefusal):
            refusals.append(outcome)
        elif outcome is not None:
            records[identity] = outcome
    return refusals, records


def _parse_resolve_state(
    identity: str, value: Any
) -> ContinuityExportRecord | ContinuityExportRefusal | None:
    def unreadable(detail: str) -> ContinuityExportRefusal:
        return ContinuityExportRefusal(UNREADABLE, identity, detail)

    wire = _wire_value(value)
    if not isinstance(wire, Mapping):
        return unreadable("the resolve state is not an object")
    state = wire.get("state")
    record = wire.get("record")
    failure = wire.get("failure")
    if state == "uninitialized":
        if record is not None or failure is not None:
            return unreadable("the uninitialized state carries a record or a failure")
        return None
    if state == "broken":
        failure_wire = _wire_value(failure) if failure is not None else None
        if isinstance(failure_wire, Mapping):
            reported = f"kind {failure_wire.get('kind')!s}, detail {failure_wire.get('detail')!s}"
        else:
            reported = "no failure detail"
        return ContinuityExportRefusal(
            BROKEN, identity, f"the provider reports broken continuity: {reported}"
        )
    if state != "ready":
        return unreadable(f"unknown state {state!r}")
    if failure is not None:
        return unreadable("the ready state carries a failure")
    if record is None:
        return unreadable("the ready state has no record")
    parsed = _parse_record(_wire_value(record))
    if isinstance(parsed, str):
        return unreadable(parsed)
    if parsed.identity != identity:
        return ContinuityExportRefusal(
            IDENTITY_DISAGREES,
            identity,
            f"the record under this identity names identity {parsed.identity!r}",
            parsed.session_id,
        )
    return parsed


def _parse_record(wire: Any) -> ContinuityExportRecord | str:
    """Check a record by the gateway's rules; a string is the reason it fails."""
    if not isinstance(wire, Mapping):
        return "the record is not an object"
    for field in ("identity", "agent_runtime_id"):
        value = wire.get(field)
        if not isinstance(value, str):
            return f"record {field} is missing or not a string"
        problem = _identity_problem(value)
        if problem is not None:
            return f"record {field} {value!r} {problem}"
    session_id = _canonical_session_id(wire.get("session_id"))
    if session_id is None:
        return f"record session_id {wire.get('session_id')!r} is not a session id"
    for field in ("generation", "checkpoint_version"):
        if not _is_u64(wire.get(field)):
            return f"record {field} {wire.get(field)!r} is not an unsigned 64-bit integer"
    return ContinuityExportRecord(
        identity=wire["identity"],
        agent_runtime_id=wire["agent_runtime_id"],
        session_id=session_id,
        generation=wire["generation"],
        checkpoint_version=wire["checkpoint_version"],
    )


def _canonical_session_id(value: Any) -> str | None:
    """A session id in any spelling 0.9.3 accepted, in canonical form; else ``None``."""
    if not isinstance(value, str) or not _SESSION_ID_SPELLINGS.fullmatch(value):
        return None
    digits = value.removeprefix("urn:uuid:").strip("{}").replace("-", "").lower()
    return f"{digits[:8]}-{digits[8:12]}-{digits[12:16]}-{digits[16:20]}-{digits[20:]}"


def _snapshot_bytes(snapshot: Any) -> bytes | str:
    """The snapshot's exact bytes, or the reason it is unreadable."""
    wire = _wire_value(snapshot)
    if isinstance(wire, Mapping):
        data = wire.get("data")
    else:
        data = getattr(wire, "data", None)
    if isinstance(data, str):
        try:
            document = base64.b64decode(data, validate=True)
        except (binascii.Error, ValueError):
            return "the snapshot data is not base64"
    elif isinstance(data, (bytes, bytearray, memoryview)):
        document = bytes(data)
    else:
        return "the snapshot has no data"
    if not document:
        return "the snapshot is empty"
    return document


def _session_index_refusal(
    identity: str, session_id: str, bound: Any
) -> ContinuityExportRefusal | None:
    if bound is None:
        return ContinuityExportRefusal(
            IDENTITY_DISAGREES,
            identity,
            "resolve_record_by_session holds no record for this session",
            session_id,
        )
    try:
        record, _fencing_token, _checkpoint_version = bound
    except (TypeError, ValueError):
        return ContinuityExportRefusal(
            UNREADABLE,
            identity,
            "resolve_record_by_session did not return "
            "(record, fencing_token, checkpoint_version)",
            session_id,
        )
    wire = _wire_value(record)
    if not isinstance(wire, Mapping):
        return ContinuityExportRefusal(
            UNREADABLE,
            identity,
            "resolve_record_by_session returned a record that is not an object",
            session_id,
        )
    if wire.get("identity") != identity or _canonical_session_id(wire.get("session_id")) != session_id:
        return ContinuityExportRefusal(
            IDENTITY_DISAGREES,
            identity,
            "resolve_record_by_session binds this session to identity "
            f"{wire.get('identity')!r} and session {wire.get('session_id')!r}",
            session_id,
        )
    return None


# ---------------------------------------------------------------------------
# Writing the file
# ---------------------------------------------------------------------------


def _ready_entry_line(
    record: ContinuityExportRecord, document: bytes
) -> tuple[bytes, bytes, bytes] | ContinuityExportRefusal:
    """A ready entry's line in three parts (the document's base64 is the
    middle one, never copied into the others), or a TooLarge refusal when the
    line would be longer than MAX_ENTRY_LINE_BYTES."""
    entry = {
        "identity": record.identity,
        "state": "ready",
        "record": {
            "identity": record.identity,
            "agent_runtime_id": record.agent_runtime_id,
            "session_id": record.session_id,
            "generation": record.generation,
            "checkpoint_version": record.checkpoint_version,
        },
        "document": {
            "length": len(document),
            "sha256": hashlib.sha256(document).hexdigest(),
            "base64": "",
        },
    }
    skeleton = _compact_json(entry).encode("utf-8")
    size = len(skeleton) + 4 * ((len(document) + 2) // 3)
    if size > MAX_ENTRY_LINE_BYTES:
        return ContinuityExportRefusal(
            TOO_LARGE,
            record.identity,
            f"the snapshot is {len(document)} bytes, so its entry would be {size} bytes; "
            f"an entry holds at most {MAX_ENTRY_LINE_BYTES} bytes",
            record.session_id,
        )
    # The skeleton ends with the empty base64 string and the two closing braces.
    return skeleton[:-3], base64.b64encode(document), skeleton[-3:]


def _record_values(record: ContinuityExportRecord) -> str:
    """A record's 0.9.3 values, as the export prints them."""
    return (
        f"0.9.3 record: agent_runtime_id {record.agent_runtime_id}, "
        f"generation {record.generation}, checkpoint_version {record.checkpoint_version}"
    )


def _uninitialized_line(identity: str) -> bytes:
    return _compact_json({"identity": identity, "state": "uninitialized"}).encode()


def _compact_json(value: Any) -> str:
    # Matches JavaScript's JSON.stringify for these values, so the readers in
    # both scripts can require exactly this form.
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False)


class _EntryFile:
    """The entry lines, written as they are made, and their count, length and SHA-256."""

    def __init__(self, path: str) -> None:
        self.path = path
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
        self.handle = os.fdopen(os.open(path, flags, 0o600), "wb")
        self.count = 0
        self.length = 0
        self.digest = hashlib.sha256()

    def write(self, *parts: bytes) -> None:
        for part in (*parts, b"\n"):
            self.handle.write(part)
            self.digest.update(part)
            self.length += len(part)
        self.handle.flush()
        self.count += 1

    def discard(self) -> None:
        self.handle.close()
        with contextlib.suppress(OSError):
            os.unlink(self.path)


def _write_export(target: str, entries: _EntryFile, partial: str) -> ContinuityExport:
    """Write the header and the entries to ``partial``, verify it, and give it its name."""
    entries.handle.close()
    header = _compact_json(
        {
            "format": CONTINUITY_EXPORT_FORMAT,
            "version": CONTINUITY_EXPORT_VERSION,
            "writer": WRITER,
            "entries": entries.count,
            "entries_length": entries.length,
            "entries_sha256": entries.digest.hexdigest(),
        }
    ).encode("utf-8")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
    descriptor = os.open(partial, flags, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as handle, open(entries.path, "rb") as source:
            handle.write(header + b"\n")
            shutil.copyfileobj(source, handle, _COPY_CHUNK_BYTES)
            handle.flush()
            os.fsync(handle.fileno())
        entries.discard()
        export = read_continuity_export(partial)
        _publish_exclusive(partial, target)
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(partial)
        raise
    _sync_directory(os.path.dirname(os.path.abspath(target)))
    return replace(export, path=target)


def _publish_exclusive(partial: str, target: str) -> None:
    """Give the verified file its name without replacing anything there.

    A hard link fails when the target exists, so a file that appears after
    the export's first check is never overwritten. On a filesystem without
    hard links the export checks again and renames, which leaves a short
    window open.
    """
    try:
        os.link(partial, target)
    except FileExistsError:
        raise _target_appeared(target) from None
    except OSError:
        if os.path.lexists(target):
            raise _target_appeared(target) from None
        os.replace(partial, target)
        return
    with contextlib.suppress(OSError):
        os.unlink(partial)


def _sync_directory(directory: str) -> None:
    if os.name != "posix":
        return
    # Best effort: the provider still holds every byte, so a lost rename only
    # means running the export again.
    with contextlib.suppress(OSError):
        descriptor = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


class _TargetExistsError(FileExistsError):
    """The target existed before the export read anything, so the command
    line reports the export as not started."""


def _target_exists(target: str) -> FileExistsError:
    return _TargetExistsError(
        17, "continuity export target already exists; it is never overwritten", target
    )


def _target_appeared(target: str) -> FileExistsError:
    return FileExistsError(
        17,
        "continuity export target appeared while the export ran; it is never overwritten",
        target,
    )


def _path_string(path: str | os.PathLike[str]) -> str:
    target = os.fspath(path)
    if not isinstance(target, str):
        raise TypeError("path must be a str or an os.PathLike[str]")
    return target


# ---------------------------------------------------------------------------
# Reading the file
# ---------------------------------------------------------------------------


def _read_export(path: str, handle: Any) -> ContinuityExport:
    def fail(reason: str) -> NoReturn:
        raise ContinuityExportFileError(path, reason)

    header_line = handle.readline(_MAX_HEADER_LINE_BYTES + 1)
    if not header_line.endswith(b"\n"):
        if len(header_line) > _MAX_HEADER_LINE_BYTES:
            fail(f"the header line is longer than {_MAX_HEADER_LINE_BYTES} bytes")
        fail("the file must start with a header line that ends in a newline")
    header = _canonical_json(header_line[:-1], "header", fail)
    if not isinstance(header, dict):
        fail("the header is not an object")
    if header.get("format") != CONTINUITY_EXPORT_FORMAT:
        fail(f"the header format is {header.get('format')!r}, not {CONTINUITY_EXPORT_FORMAT!r}")
    version = header.get("version")
    if version != CONTINUITY_EXPORT_VERSION or not _is_u64(version):
        fail(
            f"unsupported version {version!r}; this script reads version "
            f"{CONTINUITY_EXPORT_VERSION}"
        )
    if set(header) != _HEADER_KEYS:
        fail(f"the header keys must be exactly {sorted(_HEADER_KEYS)}")
    writer = header["writer"]
    if not isinstance(writer, str) or not writer:
        fail("the header writer must be a non-empty string")
    for field in ("entries", "entries_length"):
        if not _is_u64(header[field]):
            fail(f"the header {field} {header[field]!r} is not an unsigned 64-bit integer")
    if header["entries"] == 0:
        fail("the header lists no entries")
    entries_sha256 = header["entries_sha256"]
    if not isinstance(entries_sha256, str) or not _SHA256_HEX.fullmatch(entries_sha256):
        fail("the header entries_sha256 is not a lowercase SHA-256 hex digest")

    # Every line is read, hashed and counted even after an entry fails its
    # checks: when the header no longer describes the file, the damage is
    # what to report.
    limit = MAX_ENTRY_LINE_BYTES
    digest = hashlib.sha256()
    length = 0
    count = 0
    problem: str | None = None
    overlong = False
    entries: list[ContinuityExportEntry] = []
    sessions: dict[str, str] = {}
    while chunk := handle.readline(limit + 1):
        digest.update(chunk)
        length += len(chunk)
        if overlong:
            overlong = not chunk.endswith(b"\n")
            continue
        index = count
        count += 1
        if not chunk.endswith(b"\n"):
            overlong = len(chunk) > limit
            reason = "is longer than" if overlong else "does not end in a newline"
            problem = problem or f"entry {index} {reason}" + (f" {limit} bytes" if overlong else "")
            continue
        if problem is not None:
            continue
        try:
            entry = _parse_entry(chunk[:-1], index, fail)
            if entries and not entries[-1].identity < entry.identity:
                fail(
                    f"entry {index} ({entry.identity!r}) breaks the identity order "
                    "(strictly ascending by code point)"
                )
            if entry.record is not None:
                if entry.record.session_id in sessions:
                    fail(
                        f"entry {index} session {entry.record.session_id} is also bound to "
                        f"{sessions[entry.record.session_id]!r}"
                    )
                sessions[entry.record.session_id] = entry.identity
            entries.append(entry)
        except ContinuityExportFileError as error:
            problem = error.reason

    if length != header["entries_length"]:
        fail(f"the entries are {length} bytes, the header's entries_length says {header['entries_length']}")
    if digest.hexdigest() != entries_sha256:
        fail("the entries do not match the header's entries_sha256")
    if count != header["entries"]:
        fail(f"the file holds {count} entry lines, the header says {header['entries']}")
    if problem is not None:
        fail(problem)
    return ContinuityExport(
        path=path,
        version=CONTINUITY_EXPORT_VERSION,
        writer=writer,
        entries_sha256=entries_sha256,
        entries=tuple(entries),
    )


def _parse_entry(raw: bytes, index: int, fail: Callable[[str], NoReturn]) -> ContinuityExportEntry:
    item = _canonical_json(raw, f"entry {index}", fail)
    if not isinstance(item, dict):
        fail(f"entry {index} is not an object")
    identity = item.get("identity")
    if not isinstance(identity, str):
        fail(f"entry {index} identity is missing or not a string")
    problem = _identity_problem(identity)
    if problem is not None:
        fail(f"entry {index} identity {identity!r} {problem}")
    state = item.get("state")
    if state == "uninitialized":
        if set(item) != _UNINITIALIZED_KEYS:
            fail(f"entry {index} keys must be exactly {sorted(_UNINITIALIZED_KEYS)}")
        return ContinuityExportEntry(identity, "uninitialized")
    if state != "ready":
        fail(f"entry {index} has unknown state {state!r}")
    if set(item) != _READY_KEYS:
        fail(f"entry {index} keys must be exactly {sorted(_READY_KEYS)}")
    raw_record = item["record"]
    if not isinstance(raw_record, dict) or set(raw_record) != _RECORD_KEYS:
        fail(f"entry {index} record keys must be exactly {sorted(_RECORD_KEYS)}")
    record = _parse_record(raw_record)
    if isinstance(record, str):
        fail(f"entry {index} {record}")
    if not _SESSION_ID.fullmatch(raw_record["session_id"]):
        fail(f"entry {index} session_id {raw_record['session_id']!r} is not in canonical form")
    if record.identity != identity:
        fail(f"entry {index} record names identity {record.identity!r}")
    document_length, document_sha256 = _parse_document(item["document"], index, fail)
    return ContinuityExportEntry(identity, "ready", record, document_length, document_sha256)


def _parse_document(
    raw: Any, index: int, fail: Callable[[str], NoReturn]
) -> tuple[int, str]:
    if not isinstance(raw, dict) or set(raw) != _DOCUMENT_KEYS:
        fail(f"entry {index} document keys must be exactly {sorted(_DOCUMENT_KEYS)}")
    length, sha256, encoded = raw["length"], raw["sha256"], raw["base64"]
    if not isinstance(encoded, str):
        fail(f"entry {index} document base64 is not a string")
    try:
        document = base64.b64decode(encoded, validate=True)
    except (binascii.Error, ValueError):
        fail(f"entry {index} document base64 does not decode")
    if base64.b64encode(document).decode("ascii") != encoded:
        fail(f"entry {index} document base64 is not in canonical padded form")
    if not _is_u64(length) or length == 0 or len(document) != length:
        fail(f"entry {index} document is {len(document)} bytes, its length says {length!r}")
    if not isinstance(sha256, str) or not _SHA256_HEX.fullmatch(sha256):
        fail(f"entry {index} document sha256 is not a lowercase SHA-256 hex digest")
    if hashlib.sha256(document).hexdigest() != sha256:
        fail(f"entry {index} document bytes do not match their sha256")
    return length, sha256


def _canonical_json(raw: bytes, part: str, fail: Callable[[str], NoReturn]) -> Any:
    """Parse one line and require the writers' compact form.

    Re-encoding must give back the exact text, which refuses repeated keys,
    whitespace, unneeded escapes and non-integer numbers in one check.
    """
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        fail(f"the {part} is not valid UTF-8")

    def reject_constant(name: str) -> NoReturn:
        fail(f"the {part} holds the non-JSON number {name}")

    try:
        value = json.loads(text, parse_constant=reject_constant)
    except RecursionError:
        fail(f"the {part} is nested too deeply to check")
    except ValueError as error:
        fail(f"the {part} is not JSON: {error}")
    try:
        canonical = _compact_json(value)
    except RecursionError:
        fail(f"the {part} is nested too deeply to check")
    except (TypeError, ValueError):
        fail(f"the {part} is not in canonical compact form")
    if canonical != text:
        fail(f"the {part} is not in canonical compact form")
    return value


# ---------------------------------------------------------------------------
# Shared checks
# ---------------------------------------------------------------------------


def _identity_problem(value: str) -> str | None:
    """Why ``value`` is not a valid AgentIdentity, or ``None``."""
    if not value:
        return "is empty"
    if any(character in _WHITE_SPACE for character in value):
        return "contains whitespace"
    if "/" in value:
        return "contains a slash"
    try:
        value.encode("utf-8")
    except UnicodeEncodeError:
        return "is not valid Unicode"
    return None


def _is_u64(value: Any) -> bool:
    return (
        isinstance(value, int)
        and not isinstance(value, bool)
        and 0 <= value <= _U64_MAX
    )


# ---------------------------------------------------------------------------
# Command line
# ---------------------------------------------------------------------------

EXIT_REFUSED = 1
EXIT_USAGE = 2
EXIT_PROVIDER = 3
EXIT_INVALID_FILE = 4


async def load_provider(spec: str) -> Any:
    """Resolve ``module.path:attribute`` to a provider object.

    The attribute is the provider itself, or a function (sync or async) that
    takes no arguments and returns it. A module that cannot be found or
    compiled, or a missing attribute, is a usage error; an exception the
    provider's module or function raises is a provider fault
    (:class:`ContinuityExportProviderError`), and running again may succeed.
    """
    module_name, separator, attribute = spec.partition(":")
    if not separator or not module_name or not attribute:
        raise ValueError(f"--provider must be MODULE:ATTRIBUTE, not {spec!r}")
    if os.getcwd() not in sys.path:
        sys.path.insert(0, os.getcwd())
    try:
        target: Any = importlib.import_module(module_name)
    except (ImportError, SyntaxError):
        raise
    except Exception as error:
        raise ContinuityExportProviderError("module import", None, error) from error
    for part in attribute.split("."):
        target = getattr(target, part)
    if not callable(getattr(target, "resolve_many", None)) and callable(target):
        target = await _provider_call("factory", None, target)
    return target


def read_identities_file(path: str) -> list[str]:
    """The identities an identities file lists, one per line.

    The file is UTF-8, and a byte order mark at its start is dropped. Lines
    end at "\\n"; Unicode White_Space (the set the identity rules use, which
    includes the "\\r" of a CRLF line end) is trimmed from both ends of each
    line, and lines left empty are skipped. A byte order mark anywhere else is
    refused: it is invisible, and no identity an operator meant contains it.
    The Node script reads identities files by the same rule.
    """
    with open(path, "rb") as handle:
        data = handle.read()
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError(f"{path} is not valid UTF-8: {error}") from None
    identities = []
    for number, line in enumerate(text.removeprefix(_BOM).split("\n"), start=1):
        identity = line.strip(_WHITE_SPACE_CHARACTERS)
        if not identity:
            continue
        if _BOM in identity:
            raise ValueError(f"{path} line {number} contains U+FEFF (a byte order mark); remove it")
        identities.append(identity)
    return identities


def _describe(export: ContinuityExport) -> list[str]:
    ready = sum(1 for entry in export.entries if entry.state == "ready")
    lines = [
        f"{export.path}: {CONTINUITY_EXPORT_FORMAT} version {export.version}, "
        f"writer {export.writer}, entries sha256 {export.entries_sha256}",
        f"  {ready} ready, {len(export.entries) - ready} uninitialized",
    ]
    for entry in export.entries:
        if entry.state == "ready" and entry.record is not None:
            lines.append(
                f"  ready          {entry.identity}  session {entry.record.session_id}  "
                f"{entry.document_length} bytes  sha256 {entry.document_sha256}"
            )
        else:
            lines.append(f"  uninitialized  {entry.identity}")
    return lines


def _describe_missing_snapshots(export: ContinuityExport) -> list[str]:
    if not export.missing_snapshots:
        return []
    lines = [
        f"  {len(export.missing_snapshots)} written fresh by --missing-snapshot fresh "
        "(uninitialized, with an empty transcript):"
    ]
    for record in export.missing_snapshots:
        lines.append(
            f"  fresh          {record.identity}  session {record.session_id}  "
            f"({_record_values(record)}): transcript not carried, "
            "because 0.9.3 never saved a snapshot of this session"
        )
    return lines


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Export a 0.9.3 SDK-hosted continuity store, or verify export files.",
    )
    parser.add_argument("--verify", nargs="+", metavar="FILE", help="verify export files and exit")
    parser.add_argument("--provider", metavar="MODULE:ATTRIBUTE",
                        help="the 0.9.3 continuity provider object, or a function returning it")
    parser.add_argument("--identity", action="append", default=[], metavar="ID",
                        help="an identity to export (repeatable)")
    parser.add_argument("--identities-file", metavar="FILE",
                        help="a file with one identity per line")
    parser.add_argument("--output", metavar="FILE", help="the export file to create")
    parser.add_argument("--missing-snapshot", choices=MISSING_SNAPSHOT_POLICIES,
                        help="a ready record whose session has no snapshot: refuse it (the default), "
                             "or write the identity as uninitialized (fresh)")
    args = parser.parse_args(argv)

    if args.verify:
        if (args.provider or args.identity or args.identities_file or args.output
                or args.missing_snapshot is not None):
            parser.error("--verify takes only files")
        status = 0
        for path in args.verify:
            try:
                print("\n".join(_describe(read_continuity_export(path))))
            except ContinuityExportFileError as error:
                print(f"invalid: {error}", file=sys.stderr)
                status = EXIT_INVALID_FILE
            except OSError as error:
                print(f"cannot read {path}: {error}", file=sys.stderr)
                status = EXIT_INVALID_FILE
        return status

    if not args.provider or not args.output:
        parser.error("an export needs --provider and --output (or use --verify)")
    identities = list(args.identity)
    for identity in identities:
        if _BOM in identity:
            parser.error(f"--identity {identity!r} contains U+FEFF (a byte order mark); remove it")
    try:
        if args.identities_file:
            identities += read_identities_file(args.identities_file)
    except (OSError, ValueError) as error:
        parser.error(f"cannot read --identities-file: {error}")
    if not identities:
        parser.error("list the identities with --identity or --identities-file")

    async def run() -> ContinuityExport:
        provider = await load_provider(args.provider)
        return await export_continuity(
            provider, identities, args.output,
            missing_snapshot=args.missing_snapshot or MISSING_SNAPSHOT_REFUSE,
        )

    try:
        export = asyncio.run(run())
    except ContinuityExportRefusedError as refused:
        print("continuity export refused; nothing was written:", file=sys.stderr)
        for refusal in refused.refusals:
            print(
                f"  class={refusal.row_class} identity={refusal.identity or '-'} "
                f"session={refusal.session_id or '-'}\n"
                f"    detail: {refusal.detail}\n"
                f"    action: {refusal.action}",
                file=sys.stderr,
            )
        return EXIT_REFUSED
    except ContinuityExportProviderError as error:
        print(f"{error}", file=sys.stderr)
        return EXIT_PROVIDER
    except (_TargetExistsError, TypeError, ValueError, ImportError, SyntaxError, AttributeError) as error:
        print(f"continuity export not started: {error}", file=sys.stderr)
        return EXIT_USAGE
    except OSError as error:
        # An output that appeared while the export ran lands here: the export
        # had started, and it replaced nothing.
        print(f"continuity export failed, nothing written: {error}", file=sys.stderr)
        return EXIT_USAGE
    except Exception as error:
        # Exit 1 means "refused", with rows; nothing else may end that way.
        print(f"continuity export failed, nothing written: {error!r}", file=sys.stderr)
        return EXIT_USAGE
    print("\n".join(_describe(export) + _describe_missing_snapshots(export)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
