"""Durable frontend delivery owner extracted from the paired host.

The outbox records provider attempts and receipts. Hosts retain recipient policy,
provider adapters and authorization; no row grants permission to send.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable, Mapping, Sequence
from contextlib import closing
import copy
from contextvars import Context
from dataclasses import asdict, dataclass, field
from enum import Enum
import hashlib
import inspect
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
import time
from types import MappingProxyType
from typing import Any
import unicodedata
from urllib.parse import quote
import uuid

from .sender import (
    SenderAuthority, SenderDispatchSelection, _checked_thread_id, _thread_arguments,
    _output_arguments,
)

EGRESS_RECOVERY_GRACE_SECONDS = 30.0
EGRESS_DISPATCH_TIMEOUT_SECONDS = 60.0
EGRESS_FRESHNESS_SECONDS = 900.0
EGRESS_RECOVERY_INTERVAL_SECONDS = 30.0
EGRESS_BACKLOG_ALARM_PASSES = 3


def _canonical_json(value: object) -> bytes:
    return json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")


def _value(value: object, name: str, default: Any = None) -> Any:
    if isinstance(value, Mapping):
        return value.get(name, default)
    return getattr(value, name, default)


async def _await(value: Any) -> Any:
    return await value if inspect.isawaitable(value) else value


def _fsync_file(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _fsync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _decode_entities(raw: object) -> tuple[dict[str, Any], ...] | None:
    """Reconstruct a stored entity array, or None when the row carries none.

    Returns None - not an empty tuple - for a row written before rich text
    existed, so the reconstructed part has no ``entities`` key at all and is
    indistinguishable from what it dispatched before the upgrade.
    """

    if not isinstance(raw, str) or not raw:
        return None
    try:
        decoded = json.loads(raw)
    except ValueError:
        # A corrupt row must still deliver its prose. Formatting is the only
        # thing worth losing here.
        return None
    if not isinstance(decoded, list) or not decoded:
        return None
    return tuple(row for row in decoded if isinstance(row, dict))


@dataclass(frozen=True)
class EgressDeliveryPolicy:
    """Host-selected duplicate tradeoff, retained with each new output."""

    mode: str = "at_most_once"
    unknown_resends: int = 0

    def __post_init__(self) -> None:
        if (
            type(self.mode) is not str
            or type(self.unknown_resends) is not int
            or not (
                self.mode == "at_most_once" and self.unknown_resends == 0
                or self.mode == "at_least_once" and 1 <= self.unknown_resends <= 3
            )
        ):
            raise ValueError("invalid egress delivery policy")


@dataclass(frozen=True)
class EgressUncertainty:
    """A retained UNKNOWN observation, without raw provider diagnostics."""

    part_index: int
    attempt: int
    reason: str
    provider_message_id: str | None = None


@dataclass(frozen=True)
class EgressStatus:
    output_id: str
    part_states: tuple[str, ...]
    cancel_requested: bool = False
    uncertain_attempts: tuple[EgressUncertainty, ...] = ()
    delivery_policy: EgressDeliveryPolicy | None = None

    @property
    def delivery_state(self) -> str:
        """Provider acceptance is neither transport delivery nor recipient reading."""
        uncertain_parts = {row.part_index for row in self.uncertain_attempts}
        if "unknown" in self.part_states or any(
            index in uncertain_parts and state != "sent"
            for index, state in enumerate(self.part_states)
        ):
            return "unknown"
        if "pending" in self.part_states:
            return "queued"
        if self.part_states and all(state == "sent" for state in self.part_states):
            return "provider_accepted"
        if "cancelled" in self.part_states:
            return "cancelled"
        return "failed"


EGRESS_STATE_NAMESPACE = "channels/egress"
EGRESS_STATE_OWNER = "frontend/egress"
EGRESS_STATE_FORMAT = "egress-sqlite-4"
EGRESS_MIGRATION_OPERATION = "adopt-egress-sqlite-4"

_UNKNOWN_REASONS = frozenset({
    "provider_unknown", "invalid_outcome", "invalid_receipt", "timeout",
    "sender_exception", "dispatch_cancelled", "result_lost",
})


def _delivery_policy(raw: str | None) -> EgressDeliveryPolicy:
    # NULL is an old row. A new constructor setting cannot grant it retries.
    if raw is None:
        return EgressDeliveryPolicy()
    try:
        value = json.loads(raw)
        if not isinstance(value, dict) or set(value) != {"mode", "unknown_resends"}:
            raise ValueError
        policy = EgressDeliveryPolicy(**value)
        if _canonical_json(asdict(policy)).decode() != raw:
            raise ValueError
        return policy
    except (TypeError, ValueError):
        raise ValueError("invalid stored egress delivery policy") from None


def _uncertainties(raw: str | None) -> list[dict[str, Any]]:
    if raw is None:
        return []
    try:
        rows = json.loads(raw)
        if not isinstance(rows, list) or len(rows) > 4:
            raise ValueError
        attempts = set()
        for row in rows:
            if (
                not isinstance(row, dict)
                or set(row) != {"attempt", "reason", "provider_message_id"}
                or type(row["attempt"]) is not int or not 1 <= row["attempt"] <= 4
                or row["attempt"] in attempts
                or row["reason"] not in _UNKNOWN_REASONS
                or (row["provider_message_id"] is not None
                    and not isinstance(row["provider_message_id"], str))
            ):
                raise ValueError
            attempts.add(row["attempt"])
        if _canonical_json(rows).decode() != raw:
            raise ValueError
        return rows
    except (TypeError, ValueError):
        raise ValueError("invalid stored egress uncertainty") from None


def _digest(value: object) -> str:
    return hashlib.sha256(_canonical_json(value)).hexdigest()


def _checked_digest(value: object) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise ValueError("invalid egress digest")
    return value


class EgressAuthorizationDecision(Enum):
    """A current host decision for one exact delivery attempt, not a grant."""

    ALLOWED = "allowed"
    DENIED = "denied"
    DEFERRED = "deferred"


@dataclass(frozen=True)
class EgressAuthorizationRefusal:
    """Terminal host refusal for this exact candidate, without provider entry."""

    reason: str

    def __post_init__(self) -> None:
        if type(self.reason) is not str or re.fullmatch(r"[a-z][a-z0-9_]{0,63}", self.reason) is None:
            raise ValueError("invalid egress refusal reason")


@dataclass(frozen=True)
class EgressCandidate:
    """An exact host-selected destination, never a permission or inferred reply."""

    sender_id: str
    recipient: str
    thread_id: str | None = None

    def __post_init__(self) -> None:
        for value in (self.sender_id, self.recipient):
            if not isinstance(value, str) or not value.strip():
                raise ValueError("candidate identity must be nonblank text")
        _checked_thread_id(self.thread_id)


def _freeze_delivery_value(value: Any) -> Any:
    if isinstance(value, Mapping):
        return MappingProxyType({key: _freeze_delivery_value(item) for key, item in value.items()})
    if isinstance(value, (tuple, list)):
        return tuple(_freeze_delivery_value(item) for item in value)
    return value


@dataclass(frozen=True)
class EgressDeliveryContext:
    """Retained correlation and exact send candidate for the host to authorize.

    Identifiers are data, not authenticated authority. The host resolves its
    current work/notification association, audience, expiry and resource policy.
    ``part`` is recursively immutable and describes the provider candidate;
    JSON arrays are exposed as tuples.
    """

    output_id: str
    issuer_id: str
    logical_key: str
    request_digest: str
    sender_id: str
    recipient: str
    part_index: int
    part: Mapping[str, Any]
    thread_id: str | None = None
    host_values: Mapping[str, Any] = field(default_factory=dict)

    def __post_init__(self) -> None:
        _checked_thread_id(self.thread_id)
        for value in (
            self.output_id,
            self.issuer_id,
            self.logical_key,
            self.sender_id,
            self.recipient,
        ):
            if not isinstance(value, str) or not value.strip():
                raise ValueError("missing delivery context identity")
        _checked_digest(self.request_digest)
        if type(self.part_index) is not int or self.part_index < 0:
            raise ValueError("invalid delivery part index")
        if not isinstance(self.part, Mapping):
            raise ValueError("invalid delivery part")
        # Round-trip before freezing also rejects non-JSON/non-finite values
        # and detaches every nested value from caller-owned objects.
        part = json.loads(_canonical_json(dict(self.part)))
        if (
            isinstance(part.get("text"), str) and bool(part["text"])
        ) == (
            isinstance(part.get("media_ref"), str) and bool(part["media_ref"])
        ):
            raise ValueError("delivery part requires exactly one text or media reference")
        object.__setattr__(self, "part", _freeze_delivery_value(part))
        object.__setattr__(self, "host_values", _freeze_delivery_value(
            json.loads(_canonical_json(dict(self.host_values)))))


@dataclass(frozen=True)
class EgressMigrationBinding:
    """Host request correlation only; not artifact verification or permission."""

    target_manifest_digest: str
    owner_artifact_digest: str
    migration_artifact_digest: str
    previous_manifest_digest: str | None = None

    def __post_init__(self) -> None:
        for value in (
            self.target_manifest_digest,
            self.owner_artifact_digest,
            self.migration_artifact_digest,
        ):
            _checked_digest(value)
        if self.previous_manifest_digest is not None:
            _checked_digest(self.previous_manifest_digest)


@dataclass(frozen=True)
class EgressTransition:
    """Historical committed operation, reconciled with current schema on read."""

    operation: str
    source_state: str
    source_observation_digest: str
    schema_digest: str
    binding: EgressMigrationBinding


@dataclass(frozen=True)
class EgressFormatObservation:
    """A structural observation, not a file-identity or business-row snapshot."""

    state: str
    schema_digest: str | None = None
    format: str | None = None
    transition: EgressTransition | None = None
    namespace: str = EGRESS_STATE_NAMESPACE
    owner: str = EGRESS_STATE_OWNER

    @property
    def observation_digest(self) -> str:
        return _digest(asdict(self))


_OUTPUT_COLUMNS = {
    "output_id": ("text primary key",),
    "issuer_id": ("text not null",),
    "logical_key": ("text not null",),
    "request_digest": ("text not null",),
    "sender_id": ("text not null",),
    "recipient": ("text not null",),
    "incident_delivery": ("integer not null",),
    "created_at_ms": ("integer not null",),
    "root": ("text not null", "text not null default ''"),
    "part_count": ("integer not null", "integer not null default 0"),
    "cancel_requested": ("integer not null default 0",),
    "thread_id": ("text",),
    "delivery_policy": ("text",),
}
_PART_COLUMNS = {
    "output_id": ("text not null",),
    "part_index": ("integer not null",),
    "state": ("text not null",),
    "text": ("text",),
    "caption": ("text",),
    "mime_type": ("text",),
    "media_ref": ("text",),
    "attempts": ("integer not null default 0",),
    "provider_msg_id": ("text",),
    "error": ("text",),
    "unknown_attempts": ("integer not null default 0",),
    "last_outcome": ("text",),
    "entities": ("text",),
    "caption_entities": ("text",),
    "attempt_open": ("integer not null default 0",),
    "unknown_resends_used": ("integer not null default 0",),
    "unknown_history": ("text",),
}
_OPTIONAL_COLUMNS = {
    "egress_outputs": {"root", "part_count", "cancel_requested", "thread_id", "delivery_policy"},
    "egress_parts": {
        "unknown_attempts",
        "last_outcome",
        "entities",
        "caption_entities",
        "attempt_open",
        "unknown_resends_used",
        "unknown_history",
    },
}
_TABLE_CONSTRAINTS = {
    "egress_outputs": {"unique ( issuer_id , logical_key )"},
    "egress_parts": {
        "primary key ( output_id , part_index )",
        "foreign key ( output_id ) references egress_outputs ( output_id )",
    },
}
_FORMAT_SQL = """CREATE TABLE egress_format (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    namespace TEXT NOT NULL, owner TEXT NOT NULL, format TEXT NOT NULL,
    operation TEXT NOT NULL, source_state TEXT NOT NULL,
    source_observation_digest TEXT NOT NULL, schema_digest TEXT NOT NULL,
    request_binding TEXT NOT NULL
)"""
_SOURCE_STATES = {
    "absent_database",
    "absent_namespace",
    "unmarked_legacy",
    "unmarked_current",
}


@dataclass(frozen=True)
class EgressHostExtension:
    """Trusted host codecs and exact schema declarations, never DDL authority.

    Output values are TEXT or NULL; declared part fields use canonical JSON.
    Codecs receive detached immutable values. They must be pure, bounded and
    synchronous. Classification, schema creation and business policy stay with
    the host. The declaration must be identical for inspection, adoption and use.
    """

    output_columns: Mapping[str, str]
    part_columns: tuple[str, ...]
    tables: Mapping[str, str]
    indexes: Mapping[str, str]
    normalize_output: Callable[[Mapping[str, Any]], Mapping[str, Any]]
    normalize_part: Callable[[Mapping[str, Any]], Mapping[str, Any]]
    derive_output: Callable[[str, Mapping[str, Any], tuple[Mapping[str, Any], ...]], Mapping[str, Any]]
    digest_fields: Callable[[Mapping[str, Any]], Mapping[str, Any]]
    pass_output_id: bool = False

    def __post_init__(self) -> None:
        for name in ("normalize_output", "normalize_part", "derive_output", "digest_fields"):
            if not callable(getattr(self, name)) or inspect.iscoroutinefunction(getattr(self, name)):
                raise ValueError("host extension requires synchronous codecs")
        if type(self.pass_output_id) is not bool:
            raise ValueError("invalid output identity opt-in")
        for name, limit in (("output_columns", 16), ("tables", 8), ("indexes", 16)):
            source = getattr(self, name)
            if not isinstance(source, Mapping) or len(source) > limit:
                raise ValueError("invalid host schema declaration")
            copied = dict(source)
            for key, value in copied.items():
                if (type(key) is not str or re.fullmatch(r"[a-z][a-z0-9_]{0,63}", key) is None
                        or type(value) is not str):
                    raise ValueError("invalid host schema declaration")
                if name == "output_columns":
                    if key in _OUTPUT_COLUMNS or re.fullmatch(
                        r"text(?: not null)?(?: default '[A-Za-z0-9_-]{0,64}')?", value
                    ) is None:
                        raise ValueError("unsupported host output column")
                else:
                    tokens = _ddl_tokens(value)
                    if tokens[2:5] == ["if", "not", "exists"]:
                        del tokens[2:5]
                    kind = "table" if name == "tables" else "index"
                    if (tokens[:3] != ["create", kind, key] or ";" in tokens
                            or not key.startswith("egress_" if kind == "table" else "idx_egress_")
                            or key in {"egress_outputs", "egress_parts", "egress_format",
                                       "idx_egress_outputs_created", "idx_egress_parts_state"}):
                        raise ValueError("unsupported host schema declaration")
                    copied[key] = " ".join(tokens)
            object.__setattr__(self, name, MappingProxyType(copied))
        if (not isinstance(self.part_columns, (tuple, list)) or len(self.part_columns) > 16
                or any(type(key) is not str or re.fullmatch(r"[a-z][a-z0-9_]{0,63}", key) is None
                       or key in _PART_COLUMNS for key in self.part_columns)
                or len(set(self.part_columns)) != len(self.part_columns)):
            raise ValueError("invalid host part columns")
        object.__setattr__(self, "part_columns", tuple(self.part_columns))
        for sql in self.indexes.values():
            tokens = sql.split()
            if len(tokens) < 8 or tokens[3] != "on" or tokens[4] not in {
                "egress_outputs", "egress_parts", *self.tables,
            }:
                raise ValueError("invalid host index target")

    def _values(self, raw: Mapping[str, Any], *, stored: bool = False) -> dict[str, Any]:
        value = _host_json(raw, set(self.output_columns))
        result = _host_json(self.normalize_output(_freeze_delivery_value(value)), set(self.output_columns))
        if set(result) != set(self.output_columns) or any(
            item is not None and type(item) is not str for item in result.values()
        ) or any(result[key] is None and "not null" in definition
                 for key, definition in self.output_columns.items()):
            raise ValueError("invalid host output values")
        if stored and result != value:
            raise ValueError("noncanonical stored host values")
        return result

    def _parts(self, raw: Mapping[str, Any], *, stored: bool = False) -> dict[str, Any]:
        value = _host_json(raw, set(self.part_columns))
        result = _host_json(self.normalize_part(_freeze_delivery_value(value)), set(self.part_columns))
        if stored and result != value:
            raise ValueError("noncanonical stored host part")
        return result

    def _digest(self, values: Mapping[str, Any]) -> dict[str, Any]:
        result = _host_json(self.digest_fields(_freeze_delivery_value(values)))
        if set(result) & {"candidates", "parts", "incident_delivery"}:
            raise ValueError("host digest fields collide with core content")
        if set(result) - set(self.output_columns):
            raise ValueError("undeclared host digest field")
        return result


def _host_json(raw: object, allowed: set[str] | None = None) -> dict[str, Any]:
    if not isinstance(raw, Mapping) or any(type(key) is not str for key in raw):
        raise ValueError("host codec requires a mapping")
    if allowed is not None and set(raw) - allowed:
        raise ValueError("undeclared host value")
    # Convert immutable callback arrays/mappings without accepting arbitrary
    # encoders, awaitables or objects with a custom serialization protocol.
    def thaw(value: Any) -> Any:
        if isinstance(value, Mapping):
            return {key: thaw(item) for key, item in value.items()}
        if isinstance(value, (tuple, list)):
            return [thaw(item) for item in value]
        return value
    try:
        encoded = _canonical_json(thaw(raw))
    except (TypeError, ValueError, UnicodeError):
        raise ValueError("invalid host JSON value") from None
    if len(encoded) > 65536:
        raise ValueError("host values exceed bounds")
    return json.loads(encoded)


def _checked_extension(value: EgressHostExtension | None) -> EgressHostExtension | None:
    if value is not None and type(value) is not EgressHostExtension:
        raise ValueError("invalid egress host extension")
    return value


def _extra_select(names: Sequence[str]) -> str:
    # Descriptor identifiers are validated, but may be SQL keywords.
    return "" if not names else "," + ",".join('"' + name + '"' for name in names)


def _ddl_tokens(sql: object) -> list[str]:
    """Recognize only the small DDL vocabulary emitted by this owner.

    This is a bounded lexical normalizer, not a general SQL parser. Quoted
    identifiers, case and whitespace vary in SQLite's retained ALTER text.
    No comments, escaped identifiers, arbitrary literals or operators enter.
    """
    if not isinstance(sql, str) or len(sql) > 16384:
        raise ValueError("unsupported egress schema")
    pattern = re.compile(
        r"""\s+|[A-Za-z_][A-Za-z_0-9]*|"[A-Za-z_][A-Za-z_0-9]*"|`[A-Za-z_][A-Za-z_0-9]*`|\[[A-Za-z_][A-Za-z_0-9]*\]|'[A-Za-z0-9_-]{0,64}'|[01]|[(),=;]"""
    )
    tokens = []
    offset = 0
    for match in pattern.finditer(sql):
        if match.start() != offset:
            raise ValueError("unsupported egress schema")
        offset = match.end()
        token = match.group()
        if token.isspace():
            continue
        if token[0] in ('"', "`", "["):
            token = token[1:-1]
        tokens.append(token if token.startswith("'") else token.lower())
    if offset != len(sql) or len(tokens) > 2048:
        raise ValueError("unsupported egress schema")
    if tokens and tokens[-1] == ";":
        tokens.pop()
    return tokens


def _column_profile(
    connection: sqlite3.Connection, table: str, sql: str,
    host_extension: EgressHostExtension | None = None,
) -> list[object]:
    allowed = dict(_OUTPUT_COLUMNS if table == "egress_outputs" else _PART_COLUMNS)
    if host_extension is not None:
        allowed.update({key: (value,) for key, value in host_extension.output_columns.items()}
                       if table == "egress_outputs" else
                       {key: ("text",) for key in host_extension.part_columns})
    tokens = _ddl_tokens(sql)
    prefix = ["create", "table", table, "("]
    if tokens[2:5] == ["if", "not", "exists"]:
        del tokens[2:5]
    if tokens[:4] != prefix or tokens[-1:] != [")"]:
        raise ValueError("unsupported egress table")
    clauses = []
    clause = []
    depth = 0
    for token in tokens[4:-1]:
        if token == "," and depth == 0:
            clauses.append(clause)
            clause = []
        else:
            clause.append(token)
            depth += (token == "(") - (token == ")")
            if depth < 0 or depth > 1:
                raise ValueError("unsupported egress constraint")
    clauses.append(clause)
    if depth:
        raise ValueError("unsupported egress constraint")
    definitions = {}
    constraints = set()
    for clause in clauses:
        if clause and clause[0] in allowed:
            name = clause[0]
            definition = " ".join(clause[1:])
            if name in definitions or definition not in allowed[name]:
                raise ValueError("unsupported egress column")
            definitions[name] = definition
        else:
            value = " ".join(clause)
            if value in constraints or value not in _TABLE_CONSTRAINTS[table]:
                raise ValueError("unsupported egress constraint")
            constraints.add(value)
    if (
        constraints != _TABLE_CONSTRAINTS[table]
        or not (set(allowed) - _OPTIONAL_COLUMNS[table]) <= definitions.keys()
    ):
        raise ValueError("incomplete egress schema")
    columns = connection.execute(f"PRAGMA table_xinfo({table})").fetchall()
    if len(columns) != len(definitions):
        raise ValueError("egress schema disagreement")
    for _, name, kind, not_null, default, key, hidden in columns:
        definition = definitions.get(name, "")
        expected_key = (
            (1 if name == "output_id" else 0)
            if table == "egress_outputs"
            else ({"output_id": 1, "part_index": 2}.get(name, 0))
        )
        expected_default = definition.split(" default ", 1)[1] if " default " in definition else None
        if (
            kind.lower() != definition.split(" ")[0]
            or not_null != int("not null" in definition)
            or default != expected_default
            or key != expected_key
            or hidden != 0
        ):
            raise ValueError("egress schema disagreement")
    foreign = connection.execute(f"PRAGMA foreign_key_list({table})").fetchall()
    expected_foreign = (
        []
        if table == "egress_outputs"
        else [
            (
                0,
                0,
                "egress_outputs",
                "output_id",
                "output_id",
                "NO ACTION",
                "NO ACTION",
                "NONE",
            )
        ]
    )
    if foreign != expected_foreign:
        raise ValueError("unsupported egress foreign key")
    expected_indexes = (
        {("pk", ("output_id",)), ("u", ("issuer_id", "logical_key"))}
        if table == "egress_outputs"
        else {("pk", ("output_id", "part_index"))}
    )
    observed_indexes = set()
    for _, name, unique, origin, partial in connection.execute(
        f"PRAGMA index_list({table})"
    ):
        # Names are data here, so quote them before PRAGMA interpolation.
        quoted = '"' + name.replace('"', '""') + '"'
        info = connection.execute(f"PRAGMA index_xinfo({quoted})").fetchall()
        keys = [row for row in info if row[5]]
        names = tuple(row[2] for row in keys)
        if partial or any(
            row[1] < 0 or row[3] != 0 or row[4] != "BINARY" for row in keys
        ):
            raise ValueError("unsupported egress index")
        if origin in ("pk", "u"):
            if (
                unique != 1
                or (origin, names) not in expected_indexes
                or (origin, names) in observed_indexes
            ):
                raise ValueError("unsupported egress index")
            observed_indexes.add((origin, names))
        else:
            expected_name, expected_column = (
                ("idx_egress_outputs_created", "created_at_ms")
                if table == "egress_outputs"
                else ("idx_egress_parts_state", "state")
            )
            if host_extension is not None and name in host_extension.indexes:
                index_sql = connection.execute(
                    "SELECT substr(sql,1,16385) FROM sqlite_master WHERE name=?", (name,)
                ).fetchone()[0]
                if _ddl_tokens(index_sql) != _ddl_tokens(host_extension.indexes[name]):
                    raise ValueError("unsupported host index")
                continue
            if name != expected_name or names != (expected_column,) or unique:
                raise ValueError("unsupported egress index")
            index_sql = connection.execute(
                "SELECT substr(sql,1,16385) FROM sqlite_master WHERE name=?", (name,)
            ).fetchone()[0]
            if _ddl_tokens(index_sql) != [
                "create",
                "index",
                name,
                "on",
                table,
                "(",
                expected_column,
                ")",
            ]:
                raise ValueError("unsupported egress index")
    if observed_indexes != expected_indexes:
        raise ValueError("incomplete egress indexes")
    return [[name, definition] for name, definition in sorted(definitions.items())]


def _observe_format(
    connection: sqlite3.Connection, host_extension: EgressHostExtension | None = None,
) -> EgressFormatObservation:
    extension = _checked_extension(host_extension)
    host_tables = dict(extension.tables) if extension else {}
    host_indexes = dict(extension.indexes) if extension else {}
    owned_tables = ("egress_outputs", "egress_parts", "egress_format", *host_tables)
    objects = connection.execute(
        "SELECT type,substr(name,1,193),substr(tbl_name,1,193),substr(sql,1,16385) "
        "FROM sqlite_master WHERE lower(name) GLOB 'egress_*' OR lower(name) GLOB 'idx_egress_*' "
        "OR lower(tbl_name) IN (" + ",".join("?" for _ in owned_tables) + ") ORDER BY name LIMIT 64",
        owned_tables,
    ).fetchall()
    if len(objects) == 64:
        raise ValueError("too many egress schema objects")
    if not objects:
        if extension is not None:
            raise ValueError("host must prepare the declared schema before adoption")
        return EgressFormatObservation("absent_namespace")
    tables = {name: sql for kind, name, _, sql in objects if kind == "table"}
    required_tables = {"egress_outputs", "egress_parts", *host_tables}
    if set(tables) not in (required_tables, required_tables | {"egress_format"}):
        raise ValueError("incomplete egress namespace")
    for kind, name, table, sql in objects:
        if kind == "index":
            expected_table = {
                "idx_egress_outputs_created": "egress_outputs",
                "idx_egress_parts_state": "egress_parts",
                **{key: value.split()[4] for key, value in host_indexes.items()},
            }.get(name)
            if expected_table == table:
                if name in host_indexes and _ddl_tokens(sql) != _ddl_tokens(host_indexes[name]):
                    raise ValueError("unsupported host index")
                continue
            if name.startswith("sqlite_autoindex_") and table in required_tables:
                continue
            raise ValueError("unsupported egress index location")
        if kind != "table" or table not in tables:
            raise ValueError("unsupported egress schema object")
    profiles = {
        name: _column_profile(connection, name, tables[name], extension)
        for name in ("egress_outputs", "egress_parts")
    }
    indexes = sorted(name for kind, name, _, _ in objects
                     if kind == "index" and not name.startswith("sqlite_autoindex_"))
    if not set(host_indexes) <= set(indexes):
        raise ValueError("incomplete host indexes")
    schema = {"tables": profiles, "indexes": indexes}
    if extension is not None:
        # Include exact declared DDL and observed structural facts. This neither
        # creates host tables nor grants this owner permission to write them.
        host_profiles = {}
        for name, declaration in host_tables.items():
            tokens = _ddl_tokens(tables[name])
            if tokens[2:5] == ["if", "not", "exists"]:
                del tokens[2:5]
            if tokens != _ddl_tokens(declaration):
                raise ValueError("unsupported host table")
            host_profiles[name] = {
                "ddl": tokens,
                "columns": connection.execute(f"PRAGMA table_xinfo({name})").fetchall(),
                "foreign_keys": connection.execute(f"PRAGMA foreign_key_list({name})").fetchall(),
                "indexes": [],
            }
            for _, index, unique, origin, partial in connection.execute(f"PRAGMA index_list({name})"):
                quoted = '"' + index.replace('"', '""') + '"'
                host_profiles[name]["indexes"].append([
                    index, unique, origin, partial,
                    connection.execute(f"PRAGMA index_xinfo({quoted})").fetchall(),
                ])
        schema["host_tables"] = host_profiles
        schema["host_indexes"] = {name: _ddl_tokens(sql) for name, sql in host_indexes.items()}
    schema_digest = _digest(schema)
    # Host columns are required by the declaration, but are not a new core
    # format version. Preserve the exact old digest/profile when no extension.
    core_profiles = {name: dict(rows) for name, rows in profiles.items()}
    if extension is not None:
        for name in extension.output_columns:
            core_profiles["egress_outputs"].pop(name, None)
        for name in extension.part_columns:
            core_profiles["egress_parts"].pop(name, None)
    core_indexes = sorted(set(indexes) - set(host_indexes))
    complete = all(set(core_profiles[table]) == set(allowed)
                   for table, allowed in (("egress_outputs", _OUTPUT_COLUMNS), ("egress_parts", _PART_COLUMNS)))
    complete = complete and core_indexes == ["idx_egress_outputs_created", "idx_egress_parts_state"]
    prior_parts = set(_PART_COLUMNS) - {"attempt_open", "unknown_resends_used", "unknown_history"}
    prior_outputs = set(_OUTPUT_COLUMNS) - {"delivery_policy"}
    complete_v1 = (
        set(core_profiles["egress_outputs"]) == prior_outputs - {"cancel_requested", "thread_id"}
        and set(core_profiles["egress_parts"]) == prior_parts
        and core_indexes == ["idx_egress_outputs_created", "idx_egress_parts_state"]
    )
    complete_v2 = (
        set(core_profiles["egress_outputs"]) == prior_outputs - {"thread_id"}
        and set(core_profiles["egress_parts"]) == prior_parts
        and core_indexes == ["idx_egress_outputs_created", "idx_egress_parts_state"]
    )
    complete_v3 = (
        set(core_profiles["egress_outputs"]) == prior_outputs
        and set(core_profiles["egress_parts"]) == prior_parts
        and core_indexes == ["idx_egress_outputs_created", "idx_egress_parts_state"]
    )
    if "egress_format" not in tables:
        return EgressFormatObservation(
            "unmarked_current" if complete else "unmarked_legacy", schema_digest
        )
    if _ddl_tokens(tables["egress_format"]) != _ddl_tokens(_FORMAT_SQL):
        raise ValueError("unsupported managed egress schema")
    rows = connection.execute(
        "SELECT singleton,substr(namespace,1,193),substr(owner,1,193),substr(format,1,257),substr(operation,1,193),substr(source_state,1,193),substr(source_observation_digest,1,65),substr(schema_digest,1,65),substr(request_binding,1,1025) FROM egress_format LIMIT 2"
    ).fetchall()
    if len(rows) != 1 or len(rows[0]) != 9:
        raise ValueError("invalid egress format record")
    (
        singleton,
        namespace,
        owner,
        version,
        operation,
        source_state,
        prior_digest,
        stored_schema,
        raw_binding,
    ) = rows[0]
    if (
        type(singleton) is not int
        or singleton != 1
        or namespace != EGRESS_STATE_NAMESPACE
        or owner != EGRESS_STATE_OWNER
        or stored_schema != schema_digest
    ):
        raise ValueError("incompatible egress format record")
    if version == "egress-sqlite-1":
        valid = complete_v1 and operation == "adopt-egress-sqlite-1" and source_state in _SOURCE_STATES
    elif version == "egress-sqlite-2":
        valid = (
            complete_v2 and operation == "adopt-egress-sqlite-2"
            and source_state in _SOURCE_STATES | {"managed_v1"}
        )
    elif version == "egress-sqlite-3":
        valid = (
            complete_v3 and operation == "adopt-egress-sqlite-3"
            and source_state in _SOURCE_STATES | {"managed_v1", "managed_v2"}
        )
    else:
        valid = (
            version == EGRESS_STATE_FORMAT
            and complete
            and operation == EGRESS_MIGRATION_OPERATION
            and source_state in _SOURCE_STATES | {"managed_v1", "managed_v2", "managed_v3"}
        )
    if not valid:
        raise ValueError("incompatible egress format record")
    _checked_digest(prior_digest)
    if not isinstance(raw_binding, str) or len(raw_binding) > 1024:
        raise ValueError("invalid egress migration binding")
    try:
        value = json.loads(raw_binding)
        if not isinstance(value, dict) or set(value) != {
            "target_manifest_digest",
            "owner_artifact_digest",
            "migration_artifact_digest",
            "previous_manifest_digest",
        }:
            raise ValueError
        binding = EgressMigrationBinding(**value)
        if _canonical_json(asdict(binding)).decode() != raw_binding:
            raise ValueError
    except (ValueError, TypeError) as error:
        raise ValueError("invalid egress migration binding") from error
    transition = EgressTransition(
        operation, source_state, prior_digest, stored_schema, binding
    )
    return EgressFormatObservation("managed", schema_digest, version, transition)


def _state_path(path: Path) -> Path:
    path = Path(path).absolute()
    try:
        info = path.lstat()
    except FileNotFoundError:
        return path
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
        raise ValueError("egress database must be a regular unaliased file")
    return path


def _format_connection(path: Path, mode: str) -> sqlite3.Connection:
    connection = sqlite3.connect(
        "file:" + quote(str(path), safe="/") + "?mode=" + mode, uri=True, timeout=5
    )
    try:
        connection.execute("PRAGMA foreign_keys = ON")
        if mode == "ro":
            connection.execute("PRAGMA query_only = ON")
        else:
            connection.execute("PRAGMA synchronous = FULL")
        return connection
    except BaseException:
        connection.close()
        raise


def _inspect_format(path: Path, *, host_extension: EgressHostExtension | None = None) -> EgressFormatObservation:
    """Observe schema and committed transition without initialization or DDL.

    This does not prove writer quiescence, file identity or row immutability.
    """
    _checked_extension(host_extension)
    path = _state_path(path)
    if not path.exists():
        if host_extension is not None:
            raise ValueError("host must prepare the declared schema before adoption")
        return EgressFormatObservation("absent_database")
    connection = _format_connection(path, "ro")
    try:
        connection.execute("BEGIN")
        return _observe_format(connection, host_extension)
    finally:
        connection.close()


def _require_thread_format(connection: sqlite3.Connection, host_extension: EgressHostExtension | None = None) -> None:
    observed = _observe_format(connection, host_extension)
    if observed.state != "managed" or observed.format != EGRESS_STATE_FORMAT:
        raise ValueError("thread targets require managed egress-sqlite-4")


def _migrate_format(
    path: Path, expected: EgressFormatObservation, binding: EgressMigrationBinding,
    *, host_extension: EgressHostExtension | None = None,
) -> EgressFormatObservation:
    """Explicitly adopt the supported format after host-established quiescence.

    Bindings correlate the host request only. On a lost return, inspect rather
    than replaying this transition. An upgrade links the prior observation;
    the host retains that observation if it needs its complete historical data.
    """
    if (
        type(expected) is not EgressFormatObservation
        or not (
            expected.state in _SOURCE_STATES
            or (expected.state == "managed" and expected.format in {"egress-sqlite-1", "egress-sqlite-2", "egress-sqlite-3"})
        )
        or type(binding) is not EgressMigrationBinding
    ):
        raise ValueError("invalid egress migration request")
    # Recheck frozen dataclass values; they are correlation, never authority.
    EgressMigrationBinding(**asdict(binding))
    _checked_extension(host_extension)
    path = _state_path(path)
    if host_extension is not None and expected.state in {"absent_database", "absent_namespace"}:
        raise ValueError("host must prepare the declared schema before adoption")
    created = expected.state == "absent_database"
    if created:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        os.close(descriptor)
    connection = _format_connection(path, "rw")
    try:
        connection.execute("BEGIN IMMEDIATE")
        observed = _observe_format(connection, host_extension)
        if created and observed.state == "absent_namespace":
            observed = EgressFormatObservation("absent_database")
        if observed != expected:
            raise ValueError("stale egress format observation")
        upgrading = observed.state == "managed"
        if upgrading:
            connection.execute("DROP TABLE egress_format")
        _migrate_legacy_schema(connection)
        # Backfills have no permission to alter accepted/uncertain part evidence.
        if (
            connection.execute("PRAGMA foreign_key_check(egress_parts)").fetchone()
            is not None
        ):
            raise ValueError("invalid egress row references")
        if (
            connection.execute(
                "SELECT 1 FROM egress_outputs WHERE root != 'hc-notify-v1:' || issuer_id || '/' || logical_key OR part_count != (SELECT COUNT(*) FROM egress_parts WHERE egress_parts.output_id=egress_outputs.output_id) LIMIT 1"
            ).fetchone()
            is not None
        ):
            raise ValueError("invalid egress output identity")
        if host_extension is not None:
            for table in host_extension.tables:
                if connection.execute(f"PRAGMA foreign_key_check({table})").fetchone() is not None:
                    raise ValueError("invalid host row references")
        upgraded = _observe_format(connection, host_extension)
        if upgraded.state != "unmarked_current":
            raise ValueError("incomplete egress upgrade")
        connection.execute(_FORMAT_SQL)
        connection.execute(
            "INSERT INTO egress_format VALUES (?,?,?,?,?,?,?,?,?)",
            (
                1,
                EGRESS_STATE_NAMESPACE,
                EGRESS_STATE_OWNER,
                EGRESS_STATE_FORMAT,
                EGRESS_MIGRATION_OPERATION,
                "managed_v" + observed.format.rsplit("-", 1)[1] if upgrading else expected.state,
                expected.observation_digest,
                upgraded.schema_digest,
                _canonical_json(asdict(binding)).decode(),
            ),
        )
        result = _observe_format(connection, host_extension)
        connection.commit()
    finally:
        connection.close()
    if created:
        _fsync_file(path)
        _fsync_directory(path.parent)
    return result


def _migrate_legacy_schema(connection: sqlite3.Connection) -> None:
    connection.execute(
        """
                CREATE TABLE IF NOT EXISTS egress_outputs (
                    output_id TEXT PRIMARY KEY,
                    root TEXT NOT NULL,
                    issuer_id TEXT NOT NULL,
                    logical_key TEXT NOT NULL,
                    request_digest TEXT NOT NULL,
                    sender_id TEXT NOT NULL,
                    recipient TEXT NOT NULL,
                    part_count INTEGER NOT NULL,
                    incident_delivery INTEGER NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    UNIQUE (issuer_id, logical_key)
                )
                """
    )
    connection.execute(
        """
                CREATE TABLE IF NOT EXISTS egress_parts (
                    output_id TEXT NOT NULL,
                    part_index INTEGER NOT NULL,
                    state TEXT NOT NULL,
                    text TEXT,
                    caption TEXT,
                    mime_type TEXT,
                    media_ref TEXT,
                    entities TEXT,
                    caption_entities TEXT,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    unknown_attempts INTEGER NOT NULL DEFAULT 0,
                    last_outcome TEXT,
                    provider_msg_id TEXT,
                    error TEXT,
                    PRIMARY KEY (output_id, part_index),
                    FOREIGN KEY (output_id) REFERENCES egress_outputs(output_id)
                )
                """
    )
    # The stale sweep and drainer scan by part state and output age.
    # These indexes and the legacy column/data upgrades commit together.
    connection.execute(
        "CREATE INDEX IF NOT EXISTS idx_egress_parts_state ON egress_parts(state)"
    )
    connection.execute(
        "CREATE INDEX IF NOT EXISTS idx_egress_outputs_created "
        "ON egress_outputs(created_at_ms)"
    )
    output_columns = {
        str(row[1])
        for row in connection.execute("PRAGMA table_info(egress_outputs)").fetchall()
    }
    if "root" not in output_columns:
        connection.execute(
            "ALTER TABLE egress_outputs ADD COLUMN root TEXT NOT NULL DEFAULT ''"
        )
    if "part_count" not in output_columns:
        connection.execute(
            "ALTER TABLE egress_outputs ADD COLUMN part_count INTEGER NOT NULL DEFAULT 0"
        )
    if "cancel_requested" not in output_columns:
        connection.execute(
            "ALTER TABLE egress_outputs ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0"
        )
    if "thread_id" not in output_columns:
        connection.execute("ALTER TABLE egress_outputs ADD COLUMN thread_id TEXT")
    if "delivery_policy" not in output_columns:
        connection.execute("ALTER TABLE egress_outputs ADD COLUMN delivery_policy TEXT")
    connection.execute(
        "UPDATE egress_outputs SET root = "
        "'hc-notify-v1:' || issuer_id || '/' || logical_key WHERE root = ''"
    )
    connection.execute(
        "UPDATE egress_outputs SET part_count = "
        "(SELECT COUNT(*) FROM egress_parts "
        "WHERE egress_parts.output_id = egress_outputs.output_id) "
        "WHERE part_count = 0"
    )
    columns = {
        str(row[1])
        for row in connection.execute("PRAGMA table_info(egress_parts)").fetchall()
    }
    if "unknown_attempts" not in columns:
        connection.execute(
            "ALTER TABLE egress_parts ADD COLUMN unknown_attempts "
            "INTEGER NOT NULL DEFAULT 0"
        )
    if "last_outcome" not in columns:
        # No backfill invents safe retries. Legacy attempted rows without
        # an exact recorded refusal remain uncertain, even when the old
        # unknown_attempts migration supplied a zero default.
        connection.execute("ALTER TABLE egress_parts ADD COLUMN last_outcome TEXT")
    if "entities" not in columns:
        # Rich text (2026-08-20). NULL on every pre-existing row, which
        # reconstructs as a part with no entities key at all - byte
        # identical to what those rows dispatched before the upgrade.
        connection.execute("ALTER TABLE egress_parts ADD COLUMN entities TEXT")
    if "caption_entities" not in columns:
        # Inline images (2026-08-20): a photo's caption is rich text too,
        # so it needs its own array - the caption is a different string
        # from the part text and an offset into one is meaningless in the
        # other.
        connection.execute("ALTER TABLE egress_parts ADD COLUMN caption_entities TEXT")
    for name in ("attempt_open", "unknown_resends_used"):
        if name not in columns:
            connection.execute(f"ALTER TABLE egress_parts ADD COLUMN {name} INTEGER NOT NULL DEFAULT 0")
    if "unknown_history" not in columns:
        connection.execute("ALTER TABLE egress_parts ADD COLUMN unknown_history TEXT")


class ProactiveEgress:
    """Persist every output and part before invoking a sender.

    Managed mode requires explicit prior format adoption and never runs DDL or
    repairs. The default preserves unmarked legacy initialization; once a format
    record exists, even a default caller must accept its managed schema gate.
    Hosts still own quiescence and must fence incompatible old executables.
    """

    def __init__(
        self,
        *,
        path: Path,
        senders: Mapping[str, Callable[..., Awaitable[Any] | Any]],
        sender_authority: SenderAuthority | None = None,
        incident_sink: Callable[[Mapping[str, Any]], None],
        health_sink: Callable[[Mapping[str, Any]], None],
        sleep: Callable[[float], Awaitable[Any] | Any],
        dispatch_timeout_seconds: float | None = None,
        freshness_seconds: float | None = None,
        recovery_interval_seconds: float | None = None,
        schema_mode: str = "legacy",
        host_extension: EgressHostExtension | None = None,
        delivery_policy: EgressDeliveryPolicy | None = None,
        authorization_mode: str = "legacy",
        authorize_delivery: Callable[
            [EgressDeliveryContext],
            Awaitable[EgressAuthorizationDecision | EgressAuthorizationRefusal]
            | EgressAuthorizationDecision | EgressAuthorizationRefusal,
        ]
        | None = None,
        normalize_parts: Callable[
            [Sequence[Mapping[str, Any]]], Sequence[Mapping[str, Any]]
        ]
        | None = None,
    ) -> None:
        if delivery_policy is None:
            delivery_policy = EgressDeliveryPolicy()
        if type(delivery_policy) is not EgressDeliveryPolicy:
            raise ValueError("invalid egress delivery policy")
        self._delivery_policy = EgressDeliveryPolicy(**asdict(delivery_policy))
        if authorization_mode not in {"legacy", "enforced"}:
            raise ValueError("invalid egress authorization mode")
        if authorization_mode == "enforced" and not callable(authorize_delivery):
            raise ValueError("enforced egress requires host delivery authorization")
        if authorization_mode == "legacy" and authorize_delivery is not None:
            raise ValueError("delivery authorization requires enforced mode")
        self._host_extension = _checked_extension(host_extension)
        if host_extension is not None and schema_mode != "managed":
            raise ValueError("host extension requires managed mode and prior host schema adoption")
        self.path = Path(path)
        if schema_mode == "legacy":
            self.path.parent.mkdir(parents=True, exist_ok=True)
        created = not self.path.exists()
        self.senders = dict(senders)
        self.sender_authority = sender_authority
        self.incident_sink = incident_sink
        self.health_sink = health_sink
        self.sleep = sleep
        self._part_normalizer = normalize_parts
        # This is host configuration, not persisted policy. Every reopening
        # must select enforced mode and the host must fence legacy writers.
        self._authorization_mode = authorization_mode
        self._authorize_delivery = authorize_delivery
        self._dispatch_locks: dict[str, asyncio.Lock] = {}
        # Bounds: kwarg wins, else the module constant is read AT CALL TIME so
        # tests can monkeypatch the constant without re-plumbing the engine.
        self._dispatch_timeout_seconds = dispatch_timeout_seconds
        self._freshness_seconds = freshness_seconds
        self._recovery_interval_seconds = recovery_interval_seconds
        # Detached dispatch tasks are strong-referenced here until done so a
        # caller abandoning its wait (or being cancelled at the gateway's 125s
        # host deadline) can never garbage-collect a dispatch mid-flight.
        self._inflight: set[asyncio.Task[Any]] = set()
        self._drainer: asyncio.Task[Any] | None = None
        self._backlog_streak = 0
        self._backlog_alarmed = False
        if schema_mode not in ("legacy", "managed"):
            raise ValueError("invalid egress schema mode")
        self._schema_mode = schema_mode
        if schema_mode == "managed":
            observed = _inspect_format(self.path, host_extension=self._host_extension)
            if observed.state != "managed" or observed.format != EGRESS_STATE_FORMAT:
                raise ValueError("managed egress format required")
        else:
            with closing(self._connect()) as connection, connection:
                connection.execute("BEGIN IMMEDIATE")
                # A present managed record disables all legacy DDL/backfills.
                managed = connection.execute(
                    "SELECT 1 FROM sqlite_master WHERE lower(name)='egress_format'"
                ).fetchone()
                if managed:
                    observed = _observe_format(connection)
                    if observed.state != "managed" or observed.format != EGRESS_STATE_FORMAT:
                        raise ValueError("managed egress format required")
                    self._schema_mode = "managed"
                else:
                    _migrate_legacy_schema(connection)
                connection.commit()
            if created:
                _fsync_file(self.path)
                _fsync_directory(self.path.parent)

    inspect_format = staticmethod(_inspect_format)
    migrate_format = staticmethod(_migrate_format)

    def _has_sender(self, sender_id: str) -> bool:
        if self.sender_authority is not None:
            return self.sender_authority.has_sender(sender_id)
        return sender_id in self.senders

    async def _dispatch_sender(
        self,
        *,
        sender_id: str,
        recipient: str,
        part_index: int,
        part: Mapping[str, Any],
        selection: SenderDispatchSelection | None = None,
        thread_id: str | None = None,
        output_id: str | None = None,
    ) -> Any:
        if selection is not None:
            return await _await(
                selection.sender(
                    sender_id=sender_id, recipient=recipient, part_index=part_index, part=part,
                    **_thread_arguments(selection.sender, thread_id),
                    **_output_arguments(selection.sender, output_id),
                )
            )
        if self.sender_authority is not None:
            return await self.sender_authority.dispatch(
                sender_id=sender_id,
                recipient=recipient,
                part_index=part_index,
                part=part,
                **({"thread_id": thread_id} if thread_id is not None else {}),
                **({"output_id": output_id} if output_id is not None else {}),
            )
        sender = self.senders.get(sender_id)
        if sender is None:
            return {"outcome": "known_not_sent", "error": "no_active_sender"}
        return await _await(
            sender(
                sender_id=sender_id,
                recipient=recipient,
                part_index=part_index,
                part=part,
                **_thread_arguments(sender, thread_id),
                **_output_arguments(sender, output_id),
            )
        )

    async def _authorize_attempt(self, *, expected_attempts: int, **context_fields: Any) -> bool:
        if self._authorization_mode == "legacy":
            return True
        decision = EgressAuthorizationDecision.DEFERRED
        refusal = None
        code = "invalid_delivery_context"
        try:
            context = EgressDeliveryContext(**context_fields)
        except (TypeError, ValueError):
            pass
        else:
            code = "authorization_unavailable"
            try:
                # Cooperative like provider dispatch; host callbacks must not
                # block the event loop or suppress cancellation.
                loop = asyncio.get_running_loop()
                deadline = loop.time() + self._dispatch_timeout()
                timeout = asyncio.timeout_at(deadline)
                async with timeout:
                    result = await _await(self._authorize_delivery(context))
                if timeout.expired() or loop.time() >= deadline:
                    pass
                elif type(result) is EgressAuthorizationRefusal:
                    refusal = EgressAuthorizationRefusal(result.reason)
                elif type(result) is EgressAuthorizationDecision:
                    decision = result
                    code = "authorization_" + decision.value
                else:
                    code = "invalid_authorization_decision"
            except asyncio.CancelledError:
                raise
            except Exception:
                # Never persist or emit callback exception text or policy data.
                pass
        task = asyncio.current_task()
        if task is not None and task.cancelling():
            raise asyncio.CancelledError
        if decision is EgressAuthorizationDecision.ALLOWED:
            return True
        if refusal is not None:
            with closing(self._connect()) as connection, connection:
                connection.execute("BEGIN IMMEDIATE")
                changed = connection.execute(
                    "UPDATE egress_parts SET state='refused', error=? "
                    "WHERE output_id=? AND part_index=? AND state='pending' "
                    "AND attempts=? AND attempt_open=0 AND "
                    "(SELECT cancel_requested FROM egress_outputs WHERE output_id=egress_parts.output_id)=0",
                    (refusal.reason, context_fields["output_id"], context_fields["part_index"], expected_attempts),
                ).rowcount
                connection.commit()
            if changed:
                self.health_sink({"kind": "egress_authorization_refused",
                                  "output_id": context_fields["output_id"],
                                  "part_index": context_fields["part_index"], "code": refusal.reason})
                self._report_unknown(context_fields["output_id"], context_fields["part_index"])
            return False
        # Denial is not cancellation of the durable output. Current policy can
        # change, so both refusal forms remain pending until authorized or the
        # ordinary freshness bound expires. No provider attempt has occurred.
        self.health_sink(
            {
                "kind": "egress_authorization_" + decision.value,
                "output_id": context_fields["output_id"],
                "part_index": context_fields["part_index"],
                "code": code,
            }
        )
        return False

    def _connect(self) -> sqlite3.Connection:
        if self._schema_mode == "managed":
            return _format_connection(_state_path(self.path), "rw")
        connection = sqlite3.connect(self.path)
        try:
            connection.execute("PRAGMA foreign_keys = ON")
            connection.execute("PRAGMA synchronous = FULL")
        except BaseException:
            # Setup failed before the transaction owner acquired the connection.
            try:
                connection.close()
            except BaseException:
                pass  # Preserve the original setup failure.
            raise
        return connection

    # Bounds resolve kwarg-or-module-constant AT CALL TIME (monkeypatch-able).
    def _dispatch_timeout(self) -> float:
        value = self._dispatch_timeout_seconds
        return float(value) if value is not None else EGRESS_DISPATCH_TIMEOUT_SECONDS

    def _freshness(self) -> float:
        value = self._freshness_seconds
        return float(value) if value is not None else EGRESS_FRESHNESS_SECONDS

    def _recovery_interval(self) -> float:
        value = self._recovery_interval_seconds
        return float(value) if value is not None else EGRESS_RECOVERY_INTERVAL_SECONDS

    def _sender_attached(self, sender_id: str) -> bool:
        """Mirror _dispatch_sender's routing split, as a read-only peek."""

        if self.sender_authority is not None:
            return self.sender_authority.is_attached(sender_id)
        return self.senders.get(sender_id) is not None

    async def send(
        self,
        *,
        issuer_id: str,
        logical_key: str,
        candidates: Sequence[tuple[str, str] | EgressCandidate],
        parts: Sequence[Mapping[str, Any]],
        incident_delivery: bool,
        delivery_policy: EgressDeliveryPolicy | None = None,
        host_values: Mapping[str, Any] | None = None,
    ) -> str:
        """Commit then dispatch INLINE - the pre-2026-08-11 contract, preserved.

        Background callers (IncidentDrain, direct engine users, existing tests)
        keep this shape: the dispatch ladder is now internally bounded (per-
        attempt timeout + finite backoff), so inline awaiting is safe for them.
        The agent-turn path must NOT use this - DurableNotifier._deliver_parts
        uses commit_output + spawn_dispatch so a turn is never coupled to the
        ladder's worst case.
        """

        output_id = self.commit_output(
            issuer_id=issuer_id,
            logical_key=logical_key,
            candidates=candidates,
            parts=parts,
            incident_delivery=incident_delivery,
            delivery_policy=delivery_policy,
            host_values=host_values,
        )
        await self._dispatch_output(output_id)
        return output_id

    def commit_output(
        self,
        *,
        issuer_id: str,
        logical_key: str,
        candidates: Sequence[tuple[str, str] | EgressCandidate],
        parts: Sequence[Mapping[str, Any]],
        incident_delivery: bool,
        delivery_policy: EgressDeliveryPolicy | None = None,
        host_values: Mapping[str, Any] | None = None,
    ) -> str:
        """Retain first-commit policy; a later selection never changes a saved row."""
        requested_policy = self._delivery_policy if delivery_policy is None else delivery_policy
        if type(requested_policy) is not EgressDeliveryPolicy:
            raise ValueError("invalid egress delivery policy")
        requested_policy = EgressDeliveryPolicy(**asdict(requested_policy))
        if not isinstance(issuer_id, str) or not issuer_id:
            raise ValueError("issuer_id is required")
        if not isinstance(logical_key, str) or not logical_key:
            raise ValueError("logical_key is required")
        extension = self._host_extension
        if extension is None and host_values is not None:
            raise ValueError("host values require a declared extension")
        requested_values = extension._values(host_values if host_values is not None else {}) if extension else {}
        normalized_parts = self._normalize_parts(parts)
        candidate_rows = []
        for candidate in candidates:
            if isinstance(candidate, EgressCandidate):
                # Recheck even a frozen object; only this copied tuple is retained.
                selected = EgressCandidate(
                    candidate.sender_id, candidate.recipient, candidate.thread_id
                )
                candidate_rows.append((selected.sender_id, selected.recipient, selected.thread_id))
            else:
                sender, recipient = candidate
                candidate_rows.append((str(sender), str(recipient), None))
        # No-thread calls preserve the exact historical digest representation.
        digest_candidates = tuple(row if row[2] is not None else row[:2] for row in candidate_rows)
        root = f"hc-notify-v1:{issuer_id}/{logical_key}"
        core_digest = {"candidates": digest_candidates, "parts": normalized_parts,
                       "incident_delivery": incident_delivery}
        selected = next(
            (row for row in candidate_rows if self._has_sender(row[0])), None
        )
        if selected is None:
            selected = candidate_rows[0] if candidate_rows else ("", "", None)
        output_id: str
        existing_output_id: str | None = None
        with closing(self._connect()) as connection, connection:
            # Serialize the lookup and insert. Multiple in-process MCP calls or
            # a short-lived overlapping legacy subprocess must converge on the
            # same durable root instead of surfacing UNIQUE races.
            connection.execute("BEGIN IMMEDIATE")
            if extension is not None:
                observed = _observe_format(connection, extension)
                if observed.state != "managed" or observed.format != EGRESS_STATE_FORMAT:
                    raise ValueError("managed host egress format required")
            if any(row[2] is not None for row in candidate_rows):
                # A column alone cannot fence an older unmarked reader. Targeted
                # output requires the explicit managed format transition.
                _require_thread_format(connection, extension)
            existing = connection.execute(
                "SELECT output_id, request_digest, root, part_count"
                + _extra_select(tuple(extension.output_columns) if extension else ()) + " FROM egress_outputs "
                "WHERE issuer_id = ? AND logical_key = ?",
                (issuer_id, logical_key),
            ).fetchone()
            if existing is not None:
                retained = extension._values(dict(zip(extension.output_columns, existing[4:])), stored=True) if extension else {}
                request_digest = _digest({**core_digest, **(extension._digest(retained) if extension else {})})
                if (
                    existing[1] != request_digest
                    or existing[2] != root
                    or existing[3] != len(normalized_parts)
                ):
                    raise ValueError(
                        "logical-key reuse changed recipient or normalized payload"
                    )
                existing_output_id = str(existing[0])
                connection.commit()
            else:
                # A new opt-in needs the managed format. An unchanged replay
                # retains its original policy, regardless of the new request.
                if requested_policy.unknown_resends:
                    observed = _observe_format(connection, extension)
                    if observed.state != "managed" or observed.format != EGRESS_STATE_FORMAT:
                        raise ValueError("UNKNOWN resends require managed egress-sqlite-4")
                output_id = str(uuid.uuid4())
                values = requested_values
                if extension is not None:
                    values = extension._values(extension.derive_output(
                        output_id, _freeze_delivery_value(requested_values),
                        tuple(_freeze_delivery_value(row) for row in normalized_parts)))
                request_digest = _digest({**core_digest, **(extension._digest(values) if extension else {})})
                connection.execute(
                    "INSERT INTO egress_outputs "
                    "(output_id, root, issuer_id, logical_key, request_digest, sender_id, "
                    "recipient, thread_id, part_count, incident_delivery, created_at_ms, delivery_policy"
                    + _extra_select(tuple(values)) + ") VALUES (" + ",".join("?" for _ in range(12 + len(values))) + ")",
                    (
                        output_id,
                        root,
                        issuer_id,
                        logical_key,
                        request_digest,
                        selected[0],
                        selected[1],
                        selected[2],
                        len(normalized_parts),
                        int(incident_delivery),
                        int(time.time_ns() // 1_000_000),
                        _canonical_json(asdict(requested_policy)).decode(),
                        *values.values(),
                    ),
                )
                for index, part in enumerate(normalized_parts):
                    entities = part.get("entities")
                    caption_entities = part.get("caption_entities")
                    extra_parts = {key: (_canonical_json(part[key]).decode() if key in part else None)
                                   for key in extension.part_columns} if extension else {}
                    connection.execute(
                        "INSERT INTO egress_parts "
                        "(output_id, part_index, state, text, caption, mime_type, media_ref, "
                        "entities, caption_entities, attempts" + _extra_select(tuple(extra_parts)) + ") "
                        "VALUES (?, ?, 'pending', ?, ?, ?, ?, ?, ?, 0" + ", ?" * len(extra_parts) + ")",
                        (
                            output_id,
                            index,
                            part.get("text"),
                            part.get("caption"),
                            part.get("mime_type"),
                            part.get("media_ref"),
                            (
                                _canonical_json(entities).decode("utf-8")
                                if entities
                                else None
                            ),
                            (
                                _canonical_json(caption_entities).decode("utf-8")
                                if caption_entities
                                else None
                            ),
                            *extra_parts.values(),
                        ),
                    )
                connection.commit()
        if existing_output_id is not None:
            # A previous process may have committed the root and died before
            # provider dispatch. A same-key retry is itself a recovery signal.
            return existing_output_id
        _fsync_file(self.path)
        return output_id

    def spawn_dispatch(self, output_id: str) -> "asyncio.Task[None]":
        """Start dispatch as a DETACHED, strong-referenced task and return it.

        The caller may wait on the returned task with a budget - and may be
        cancelled while doing so (the gateway cancels callbacks at its 125s
        host completion deadline; that cancellation is what stranded the three
        2026-08-11 production rows). Because the dispatch runs in its own task,
        caller cancellation never reaches the sender await or the outcome
        classifier: the part always terminalizes or leaves a durable
        breadcrumb, regardless of what happens to the turn that asked.
        """

        task = asyncio.get_running_loop().create_task(self._dispatch_output(output_id), context=Context())
        self._inflight.add(task)
        task.add_done_callback(self._dispatch_task_done)
        return task

    def _dispatch_task_done(self, task: "asyncio.Task[Any]") -> None:
        self._inflight.discard(task)
        if task.cancelled():
            return
        error = task.exception()
        if error is not None:
            # A caller that abandoned its budgeted wait would otherwise learn
            # about an engine fault only from asyncio's unretrieved-exception
            # noise at teardown. Never silent (2026-08-11 discipline); callers
            # still waiting also re-raise this same exception themselves.
            detail = str(error).strip() or repr(error).strip()
            self.health_sink(
                {
                    "kind": "egress_dispatch_task_failed",
                    "error": (detail or "<no detail>")[:512],
                    "error_type": type(error).__name__,
                }
            )

    def find_output(self, *, issuer_id: str, logical_key: str) -> str | None:
        with closing(self._connect()) as connection, connection:
            row = connection.execute(
                "SELECT output_id FROM egress_outputs WHERE issuer_id = ? AND logical_key = ?",
                (issuer_id, logical_key),
            ).fetchone()
        return str(row[0]) if row is not None else None

    def attempt_count(self, *, output_id: str) -> int:
        with closing(self._connect()) as connection, connection:
            row = connection.execute(
                "SELECT COALESCE(MAX(attempts), 0) FROM egress_parts WHERE output_id = ?",
                (output_id,),
            ).fetchone()
        return int(row[0]) if row is not None else 0

    def status(self, *, output_id: str) -> EgressStatus:
        with closing(self._connect()) as connection, connection:
            connection.execute("BEGIN")
            return self._status(connection, output_id)

    @staticmethod
    def _status(connection: sqlite3.Connection, output_id: str) -> EgressStatus:
        rows = connection.execute(
            "SELECT p.state, o.cancel_requested, p.part_index, p.unknown_history, o.delivery_policy FROM egress_parts p "
            "JOIN egress_outputs o ON o.output_id = p.output_id "
            "WHERE p.output_id = ? ORDER BY p.part_index", (output_id,),
        ).fetchall()
        if not rows:
            raise ValueError("unknown output_id")
        policy = _delivery_policy(rows[0][4]) if rows[0][4] is not None else None
        return EgressStatus(
            output_id, tuple(str(row[0]) for row in rows), bool(rows[0][1]),
            tuple(EgressUncertainty(part_index=int(row[2]), **item)
                  for row in rows for item in _uncertainties(row[3])), policy,
        )

    def cancel_output(self, *, output_id: str, issuer_id: str) -> EgressStatus:
        """Durably stop future reservations for one host-authorized output.

        The host authenticates the actor and selects this exact output/issuer.
        A reservation already committed may still enter or complete; its actual
        receipt remains authoritative. This never cancels its running task.
        """
        if not isinstance(output_id, str) or not output_id or not isinstance(issuer_id, str) or not issuer_id:
            raise ValueError("output_id and issuer_id must be nonempty text")
        with closing(self._connect()) as connection, connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT issuer_id FROM egress_outputs WHERE output_id = ?", (output_id,),
            ).fetchone()
            if row is None or row[0] != issuer_id:
                raise ValueError("unknown output or issuer mismatch")
            connection.execute(
                "UPDATE egress_outputs SET cancel_requested = 1 WHERE output_id = ?", (output_id,),
            )
            connection.execute(
                "UPDATE egress_parts SET state = 'cancelled' WHERE output_id = ? "
                "AND state = 'pending' AND unknown_attempts = 0 "
                "AND (unknown_history IS NULL OR unknown_history = '[]') "
                "AND (attempts = 0 OR last_outcome = 'known_not_sent')", (output_id,),
            )
            # A committed reservation may already have entered the provider.
            # Preserve that uncertainty; the same attempt may still refine it.
            connection.execute(
                "UPDATE egress_parts SET state = 'unknown', last_outcome = "
                "CASE WHEN unknown_history IS NOT NULL THEN last_outcome ELSE 'unknown' END "
                "WHERE output_id = ? AND state = 'pending' "
                "AND (unknown_history IS NOT NULL OR unknown_attempts > 0 OR (attempts > 0 "
                "AND (last_outcome IS NULL OR last_outcome != 'known_not_sent')))",
                (output_id,),
            )
            result = self._status(connection, output_id)
            connection.commit()
        for index in range(len(result.part_states)):
            self._report_unknown(output_id, index)
        return result

    async def recover_pending(self, *, previous_dispatches_quiesced: bool = False) -> None:
        """Expire stale rows, then run ONE bounded recovery pass inline.

        The exact-bool assertion permits recording at most 1,000 abandoned
        reservations after the host has stopped all prior dispatches/writers.
        It is not a fence and never follows from age or this instance's locks.
        Existing row policy and cancellation still control retry eligibility.

        Kept for direct engine callers and tests. The notifier's send path no
        longer awaits this (2026-08-11): it sweeps + arms the background
        drainer instead, so an agent turn never dispatches anyone else's
        backlog. Every dispatch inside a pass is bounded by the per-attempt
        timeout, so this call is now finite even against a wedged sender.
        """

        if type(previous_dispatches_quiesced) is not bool:
            raise ValueError("quiescence assertion must be a boolean")
        if previous_dispatches_quiesced:
            if any(lock.locked() for lock in self._dispatch_locks.values()):
                raise ValueError("local dispatch is still active")
            # The host has stopped every previous writer/dispatch. Capture once
            # before any provider await; a later reservation is never swept into
            # this abandoned set. This assertion does not itself fence a writer.
            with closing(self._connect()) as connection, connection:
                abandoned = connection.execute(
                    "SELECT p.output_id,p.part_index,p.attempts FROM egress_parts p "
                    "JOIN egress_outputs o USING(output_id) WHERE p.attempt_open=1 "
                    "AND p.state IN ('pending','unknown') "
                    "AND o.delivery_policy IS NOT NULL ORDER BY p.output_id,p.part_index LIMIT 1000"
                ).fetchall()
            for output_id, part_index, attempt in abandoned:
                changed = self._update_part(output_id, part_index, state="unknown",
                    attempts=attempt, unknown_attempts=1, provider_msg_id=None,
                    error="previous provider result unavailable after host quiescence",
                    last_outcome="unknown", unknown_reason="result_lost", require_open=True)
                if changed:
                    self._report_unknown(output_id, part_index)
        self.expire_stale()
        # Direct recovery resumes recorded results without the background age
        # grace. It still cannot retry an open reservation without the explicit
        # host quiescence assertion above. Age and local locks prove no such
        # cross-process exclusion.
        await self.run_recovery_pass(respect_grace=False)

    def expire_stale(self) -> int:
        """Terminally expire pending parts older than the freshness window.

        Pure synchronous sqlite - cannot hang, safe at boot before senders
        attach. Outputs whose dispatch lock is currently held are skipped: an
        owner task is responsible for those, and the age gate inside the
        dispatch loop applies the same cutoff under the lock, so nothing
        escapes - it just gets expired by its owner instead of the sweep.
        """

        cutoff_ms = time.time_ns() // 1_000_000 - int(self._freshness() * 1000)
        with closing(self._connect()) as connection, connection:
            rows = connection.execute(
                "SELECT p.output_id, p.part_index, p.attempts, p.unknown_attempts, "
                "o.created_at_ms "
                "FROM egress_parts p JOIN egress_outputs o ON o.output_id = p.output_id "
                "WHERE p.state = 'pending' "
                "ORDER BY o.created_at_ms",
                (),
            ).fetchall()
        expired = 0
        now_ms = time.time_ns() // 1_000_000
        for output_id, part_index, attempts, unknown_attempts, created_at_ms in rows:
            lock = self._dispatch_locks.get(str(output_id))
            if lock is not None and lock.locked():
                continue
            if self._retire_unresolved(str(output_id), int(part_index),
                    finalize_completed=int(created_at_ms) < cutoff_ms):
                self.health_sink(
                    {
                        "kind": "egress_delivery_unknown",
                        "output_id": str(output_id),
                        "part_index": int(part_index),
                    }
                )
                self._report_unknown(str(output_id), int(part_index))
                continue
            if int(created_at_ms) >= cutoff_ms:
                continue
            age_seconds = max(0.0, (now_ms - int(created_at_ms)) / 1000.0)
            changed = self._update_part(
                str(output_id),
                int(part_index),
                state="expired",
                attempts=int(attempts),
                unknown_attempts=int(unknown_attempts),
                provider_msg_id=None,
                last_outcome="known_not_sent" if int(attempts) else None,
                error=(
                    f"expired: output is {age_seconds:.0f}s old, beyond the "
                    f"{self._freshness():.0f}s redelivery window; never handed "
                    "to the provider after expiry"
                ),
            )
            if not changed:
                continue
            self.health_sink(
                {
                    "kind": "egress_delivery_expired",
                    "output_id": str(output_id),
                    "part_index": int(part_index),
                    "age_seconds": round(age_seconds, 1),
                }
            )
            expired += 1
            self._report_unknown(str(output_id), int(part_index))
        return expired

    async def run_recovery_pass(self, *, respect_grace: bool = True) -> dict[str, int]:
        """One bounded pass: expire stale, dispatch what is actually deliverable.

        Public and summary-returning so every drainer behavior is testable
        without the background task: the drainer loop is a thin caller of this.

        Skips outputs whose dispatch lock is held (an owner task is
        responsible; never queue behind a possibly-leaked task) and skips
        outputs whose sender is DETACHED - dispatching into a detached sender
        proves nothing yet burns the part's ambiguity budget, which is exactly
        how boot recovery terminally parked the 2026-07 production rows before
        the Telegram extension had attached (the July-zombie factory, closed
        here).
        """

        expired = self.expire_stale()
        dispatched = 0
        skipped_locked = 0
        skipped_detached = 0
        skipped_fresh = 0
        with closing(self._connect()) as connection, connection:
            rows = connection.execute(
                "SELECT DISTINCT p.output_id, o.sender_id, o.created_at_ms "
                "FROM egress_parts p JOIN egress_outputs o ON o.output_id = p.output_id "
                "WHERE p.state = 'pending' AND o.cancel_requested = 0 "
                "ORDER BY o.created_at_ms",
                (),
            ).fetchall()
        now_ms = time.time_ns() // 1_000_000
        for output_id, sender_id, created in rows:
            lock = self._dispatch_locks.get(str(output_id))
            if lock is not None and lock.locked():
                skipped_locked += 1
                continue
            if (
                respect_grace
                and (now_ms - int(created)) / 1000.0 < EGRESS_RECOVERY_GRACE_SECONDS
            ):
                # Fresh output: its committing send's own detached task owns
                # the first dispatch. Counted, never silently filtered.
                skipped_fresh += 1
                continue
            if not self._sender_attached(str(sender_id)):
                skipped_detached += 1
                continue
            await self._dispatch_output(str(output_id))
            dispatched += 1
        with closing(self._connect()) as connection, connection:
            pending_remaining = int(
                connection.execute(
                    "SELECT COUNT(DISTINCT p.output_id) FROM egress_parts p "
                    "JOIN egress_outputs o ON o.output_id = p.output_id "
                    "WHERE p.state = 'pending' AND o.cancel_requested = 0"
                ).fetchone()[0]
            )
        # Backlog alarm: PERSISTENCE, not presence - one pending pass is normal
        # (a send may be mid-ladder); a backlog that survives consecutive
        # passes means nothing is draining it. Fires exactly AT the threshold
        # so an episode alarms once, not once per pass (the wallpaper lesson of
        # the 2026-08-11 outage), and resets on a clean pass.
        # STUCK rows only: a row that is fresh (owner in flight), lock-held
        # (owner mid-ladder), or just dispatched this pass is normal traffic,
        # and counting it made the alarm fire under sustained ordinary load
        # (confirmed by probe in adversarial review). Stuck = pending that this
        # pass could not move for non-ownership reasons (detached sender,
        # dispatch that returned the row to pending).
        stuck = max(0, pending_remaining - skipped_fresh - skipped_locked - dispatched)
        if stuck > 0:
            self._backlog_streak += 1
            if (
                self._backlog_streak == EGRESS_BACKLOG_ALARM_PASSES
                and not self._backlog_alarmed
            ):
                self._backlog_alarmed = True
                self.health_sink(
                    {
                        "kind": "egress_backlog_persisting",
                        "stuck_outputs": stuck,
                        "pending_outputs": pending_remaining,
                        "consecutive_passes": self._backlog_streak,
                        "remedy": (
                            "check sender attachment and the oldest pending rows in "
                            "egress_parts; a wedged sender self-heals via per-attempt "
                            "timeouts, a detached one needs its extension"
                        ),
                    }
                )
        else:
            self._backlog_streak = 0
            self._backlog_alarmed = False
        return {
            "expired": expired,
            "dispatched": dispatched,
            "skipped_locked": skipped_locked,
            "skipped_detached": skipped_detached,
            "skipped_fresh": skipped_fresh,
            "pending_remaining": pending_remaining,
        }

    def ensure_recovery_drainer(self) -> None:
        """Start the background drainer if it is not already alive.

        Armed from boot recovery, from every send, and from sender attachment.
        Self-terminating: the drainer exits when a pass leaves zero pending
        parts, so an idle household carries no background task.
        """

        task = self._drainer
        if task is not None and not task.done():
            return
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            # Synchronous construction contexts (engine built before the loop
            # runs). The next send or attach re-arms from async context.
            return
        self._drainer = loop.create_task(self._drain_until_empty(), context=Context())

    async def _drain_until_empty(self) -> None:
        while True:
            try:
                summary = await self.run_recovery_pass()
            except Exception as error:  # noqa: BLE001 - a glass drainer helps nobody
                # Any store/dispatch fault must not kill recovery SILENTLY (a
                # probe during adversarial review killed the drainer with one
                # injected sqlite error and nothing anywhere said so). Emit and
                # exit; the next send/attach re-arms a fresh drainer.
                detail = str(error).strip() or repr(error).strip()
                self.health_sink(
                    {
                        "kind": "egress_drainer_failed",
                        "error": (detail or "<no detail>")[:512],
                        "error_type": type(error).__name__,
                    }
                )
                return
            if summary["pending_remaining"] == 0:
                return
            # No early exit on an "all detached" summary: that summary is
            # STALE by one sleep, and a sender attaching during the sleep
            # found the drainer alive (so attach did not re-arm) yet the exit
            # then stranded the very backlog the attach made deliverable -
            # reproduced by probe during adversarial review (2026-08-12).
            # Termination is still guaranteed without it: every pending row
            # either gets dispatched, ages past freshness and expires, or the
            # pass count stays bounded by freshness/interval (~30 cheap
            # passes worst case), after which pending_remaining hits zero.
            await _await(self.sleep(self._recovery_interval()))
            # A true scheduler yield regardless of what sleep was injected -
            # several fixtures inject synchronous no-ops, which starved the
            # loop (found as a whole-suite hang on 2026-08-12).
            await asyncio.sleep(0)

    def backlog_summary(self) -> dict[str, Any]:
        """Queryable egress state for health surfaces - counts, oldest age."""

        with closing(self._connect()) as connection, connection:
            counts = {
                str(state): int(count)
                for state, count in connection.execute(
                    "SELECT state, COUNT(*) FROM egress_parts GROUP BY state"
                )
            }
            oldest = connection.execute(
                "SELECT MIN(o.created_at_ms) "
                "FROM egress_parts p JOIN egress_outputs o ON o.output_id = p.output_id "
                "WHERE p.state = 'pending' AND o.cancel_requested = 0"
            ).fetchone()[0]
            pending_parts = connection.execute(
                "SELECT COUNT(*) FROM egress_parts p JOIN egress_outputs o "
                "ON o.output_id = p.output_id WHERE p.state = 'pending' AND o.cancel_requested = 0"
            ).fetchone()[0]
        oldest_age = (
            max(0.0, (time.time_ns() // 1_000_000 - int(oldest)) / 1000.0)
            if oldest is not None
            else None
        )
        return {
            "parts_by_state": counts,
            "pending_parts": int(pending_parts),
            "oldest_pending_age_seconds": (
                round(oldest_age, 1) if oldest_age is not None else None
            ),
            "drainer_alive": self._drainer is not None and not self._drainer.done(),
            "backlog_streak": self._backlog_streak,
        }

    def incident_cycles(self, incident_id: str) -> list[tuple[int, str, str]]:
        prefix = f"{incident_id}:"
        cycles: list[tuple[int, str, str]] = []
        with closing(self._connect()) as connection, connection:
            outputs = connection.execute(
                "SELECT output_id, logical_key FROM egress_outputs "
                "WHERE issuer_id = 'incident_drain'"
            ).fetchall()
            for output_id, logical_key in outputs:
                if not str(logical_key).startswith(prefix):
                    continue
                try:
                    cycle = int(str(logical_key)[len(prefix) :])
                except ValueError:
                    continue
                parts = connection.execute(
                    "SELECT state,last_outcome,attempts,unknown_attempts FROM egress_parts "
                    "WHERE output_id = ? ORDER BY part_index",
                    (output_id,),
                ).fetchall()
                states = [row[0] for row in parts]
                if states and all(state == "sent" for state in states):
                    state = "sent"
                elif "unknown" in states or any(
                    row[0] != "sent"
                    and (row[3] > 0 or (row[2] > 0 and row[1] != "known_not_sent"))
                    for row in parts
                ):
                    state = "unknown"
                elif "pending" in states:
                    state = "pending"
                elif "cancelled" in states:
                    state = "cancelled"
                elif states and all(
                    row[0] == "failed" and row[1] == "known_not_sent" for row in parts
                ):
                    state = "failed"
                elif "expired" in states:
                    state = "expired"
                else:
                    state = "unknown"
                cycles.append((cycle, str(output_id), state))
        return sorted(cycles)

    async def _dispatch_output(self, output_id: str) -> None:
        lock = self._dispatch_locks.setdefault(output_id, asyncio.Lock())
        async with lock:
            await self._dispatch_output_locked(output_id)

    async def _dispatch_output_locked(self, output_id: str) -> None:
        extension = self._host_extension
        output_columns = tuple(extension.output_columns) if extension else ()
        part_columns = extension.part_columns if extension else ()
        with closing(self._connect()) as connection, connection:
            # Correlation and all candidate parts come from one verified snapshot.
            connection.execute("BEGIN")
            if extension is not None:
                observed = _observe_format(connection, extension)
                if observed.state != "managed" or observed.format != EGRESS_STATE_FORMAT:
                    raise ValueError("managed host egress format required")
            output = connection.execute(
                "SELECT sender_id, recipient, incident_delivery, created_at_ms, "
                "issuer_id, logical_key, request_digest, thread_id" + _extra_select(output_columns) +
                " FROM egress_outputs WHERE output_id = ?", (output_id,),
            ).fetchone()
            rows = connection.execute(
                "SELECT part_index, state, text, caption, mime_type, media_ref, attempts, "
                "unknown_attempts, entities, caption_entities" + _extra_select(part_columns) +
                " FROM egress_parts WHERE output_id = ? ORDER BY part_index", (output_id,),
            ).fetchall()
            if output is not None and output[7] is not None:
                _checked_thread_id(output[7])
                _require_thread_format(connection, extension)
        if output is None:
            raise ValueError("unknown output_id")
        (sender_id, recipient, incident_delivery, created_at_ms, issuer_id,
         logical_key, request_digest, thread_id) = output[:8]
        host_values = extension._values(dict(zip(output_columns, output[8:])), stored=True) if extension else {}
        for stored_part in rows:
            (part_index, state, text, caption, mime_type, media_ref, attempts,
             unknown_attempts, entities_json, caption_entities_json) = stored_part[:10]
            if state != "pending":
                continue
            if self._retire_unresolved(output_id, part_index):
                self.health_sink({"kind": "egress_delivery_unknown", "output_id": output_id,
                                  "part_index": part_index})
                self._report_unknown(output_id, part_index)
                continue
            part = {key: value for key, value in {
                "text": text, "caption": caption, "mime_type": mime_type,
                "media_ref": media_ref, "entities": _decode_entities(entities_json),
                "caption_entities": _decode_entities(caption_entities_json),
            }.items() if value is not None}
            if extension is not None:
                extras = {}
                for key, raw in zip(part_columns, stored_part[10:]):
                    if raw is not None:
                        if type(raw) is not str or len(raw.encode("utf-8")) > 65536:
                            raise ValueError("invalid stored host part")
                        try:
                            value = json.loads(raw)
                        except ValueError:
                            raise ValueError("invalid stored host part") from None
                        if _canonical_json(value).decode() != raw:
                            raise ValueError("noncanonical stored host part")
                        extras[key] = value
                part.update(extension._parts(extras, stored=True))
            consumed_attempts = int(attempts)
            while True:
                # Revalidate age and actual sender attachment before every
                # attempt, including after a known-not-sent backoff.
                age_seconds = max(
                    0.0, (time.time_ns() // 1_000_000 - int(created_at_ms)) / 1000.0
                )
                if age_seconds > self._freshness():
                    if self._retire_unresolved(output_id, part_index, finalize_completed=True):
                        self._report_unknown(output_id, part_index)
                        break
                    changed = self._update_part(
                        output_id,
                        part_index,
                        state="expired",
                        attempts=consumed_attempts,
                        unknown_attempts=0,
                        provider_msg_id=None,
                        error=f"expired: output is {age_seconds:.0f}s old; no further provider entry",
                        last_outcome="known_not_sent" if consumed_attempts else None,
                    )
                    if changed:
                        self.health_sink(
                            {
                                "kind": "egress_delivery_expired",
                                "output_id": output_id,
                                "part_index": part_index,
                                "age_seconds": round(age_seconds, 1),
                            }
                        )
                        self._report_unknown(output_id, part_index)
                    break
                if consumed_attempts >= 4:
                    changed = self._update_part(
                        output_id,
                        part_index,
                        state="failed",
                        attempts=consumed_attempts,
                        unknown_attempts=0,
                        provider_msg_id=None,
                        error="known-not-sent retry budget exhausted",
                        last_outcome="known_not_sent",
                    )
                    if changed:
                        self._report_unknown(output_id, part_index)
                    break
                if not self._sender_attached(str(sender_id)):
                    self.health_sink(
                        {
                            "kind": "egress_sender_detached",
                            "output_id": output_id,
                            "part_index": part_index,
                        }
                    )
                    break
                # Keep the retained candidate independent of sender mutation,
                # including when a refusal proposes a plain presentation.
                attempt_part = copy.deepcopy(part)
                selection = None
                if self.sender_authority is not None and (
                    self._authorization_mode == "enforced" or thread_id is not None or extension is not None
                ):
                    try:
                        async with asyncio.timeout(self._dispatch_timeout()):
                            selection = await self.sender_authority.select_dispatch(
                                sender_id=str(sender_id)
                            )
                    except asyncio.CancelledError:
                        raise
                    except Exception:
                        self.health_sink(
                            {
                                "kind": "egress_sender_selection_deferred",
                                "output_id": output_id,
                                "part_index": part_index,
                            }
                        )
                        break
                if not await self._authorize_attempt(
                    expected_attempts=consumed_attempts,
                    output_id=output_id,
                    issuer_id=issuer_id,
                    logical_key=logical_key,
                    request_digest=request_digest,
                    sender_id=sender_id,
                    recipient=recipient,
                    part_index=part_index,
                    part=attempt_part,
                    thread_id=thread_id,
                    host_values=host_values,
                ):
                    break
                target_sender = selection.sender if selection is not None else self.senders.get(sender_id)
                try:
                    _thread_arguments(target_sender, thread_id)
                    if extension is not None and extension.pass_output_id:
                        _output_arguments(target_sender, output_id)
                    if attempt_part.get("choices") and getattr(target_sender, "supports_choices", False) is not True:
                        raise ValueError("sender does not support choices")
                except (TypeError, ValueError):
                    self.health_sink({
                        "kind": "egress_sender_capability_unsupported" if extension else "egress_thread_target_unsupported",
                        "output_id": output_id, "part_index": part_index,
                    })
                    break
                if selection is not None and not self.sender_authority.is_current_dispatch(
                    selection
                ):
                    self.health_sink(
                        {
                            "kind": "egress_sender_selection_deferred",
                            "output_id": output_id,
                            "part_index": part_index,
                        }
                    )
                    break
                # Authorization may await host policy. Apply the existing age
                # and attachment bounds again if they changed during that wait.
                if (self._authorization_mode == "enforced" or thread_id is not None or extension is not None) and (
                    (time.time_ns() // 1_000_000 - int(created_at_ms)) / 1000.0 > self._freshness()
                    or not self._sender_attached(str(sender_id))
                ):
                    continue
                if not self._reserve_attempt(output_id, part_index, consumed_attempts):
                    # Another handle advanced the canonical row. A stale local
                    # snapshot cannot reserve another provider invocation.
                    break
                consumed_attempts += 1
                retry_without_formatting = False
                unknown_reason = "provider_unknown"
                try:
                    async with asyncio.timeout(self._dispatch_timeout()):
                        result = await self._dispatch_sender(
                            sender_id=str(sender_id),
                            recipient=str(recipient),
                            part_index=int(part_index),
                            part=attempt_part,
                            **({"thread_id": thread_id} if thread_id is not None else {}),
                            **({"selection": selection} if selection is not None else {}),
                            **({"output_id": output_id} if extension is not None and extension.pass_output_id else {}),
                        )
                    outcome = _value(result, "outcome")
                    provider_msg_id = _value(result, "provider_message_id")
                    error = _value(result, "error")
                    retry_without_formatting = (
                        outcome == "known_not_sent"
                        and _value(result, "retry_without_formatting") is True
                    )
                    # Only exact recognized strings constitute provider evidence.
                    if not isinstance(outcome, str) or outcome not in {
                        "sent",
                        "acked",
                        "known_not_sent",
                        "unknown",
                    }:
                        outcome = "unknown"
                        unknown_reason = "invalid_outcome"
                        error = "sender returned no recognized delivery outcome"
                    if provider_msg_id is not None and not isinstance(
                        provider_msg_id, str
                    ):
                        outcome = "unknown"
                        unknown_reason = "invalid_receipt"
                        provider_msg_id = None
                        error = "sender returned an invalid provider receipt"
                except asyncio.CancelledError:
                    self._update_part(
                        output_id,
                        part_index,
                        state="unknown",
                        attempts=consumed_attempts,
                        unknown_attempts=1,
                        provider_msg_id=None,
                        last_outcome="unknown",
                        error="dispatch cancelled in flight; outcome unknowable",
                        unknown_reason="dispatch_cancelled",
                    )
                    self._report_unknown(output_id, part_index)
                    raise
                except TimeoutError:
                    unknown_reason = "timeout"
                    outcome, provider_msg_id, error = (
                        "unknown",
                        None,
                        "sender dispatch timed out; outcome unknowable",
                    )
                except Exception as exc:
                    unknown_reason = "sender_exception"
                    outcome, provider_msg_id = "unknown", None
                    error = f"sender raised {type(exc).__name__}; outcome unknowable"
                if outcome in {"sent", "acked"}:
                    self._update_part(
                        output_id,
                        part_index,
                        state="sent",
                        attempts=consumed_attempts,
                        unknown_attempts=0,
                        provider_msg_id=provider_msg_id,
                        error=None,
                        last_outcome="provider_accepted",
                    )
                    break
                if outcome == "unknown":
                    changed = self._update_part(
                        output_id,
                        part_index,
                        state="unknown",
                        attempts=consumed_attempts,
                        unknown_attempts=1,
                        provider_msg_id=provider_msg_id,
                        error=str(error)
                        if error is not None
                        else "delivery outcome unknown",
                        last_outcome="unknown",
                        unknown_reason=unknown_reason,
                    )
                    self.health_sink(
                        {
                            "kind": "egress_delivery_unknown",
                            "output_id": output_id,
                            "part_index": part_index,
                        }
                    )
                    status = self.status(output_id=output_id)
                    if changed and not status.cancel_requested and status.part_states[part_index] == "pending":
                        await _await(self.sleep((5, 30, 120)[consumed_attempts - 1]))
                        continue
                    if changed:
                        self._report_unknown(output_id, part_index)
                    break
                exhausted = consumed_attempts >= 4
                changed = self._update_part(
                    output_id,
                    part_index,
                    state="failed" if exhausted else "pending",
                    attempts=consumed_attempts,
                    unknown_attempts=0,
                    provider_msg_id=provider_msg_id,
                    error=str(error) if error is not None else "known_not_sent",
                    last_outcome="known_not_sent",
                    clear_formatting=retry_without_formatting,
                )
                if not changed:
                    # Another owner advanced this attempt. A local refusal
                    # cannot authorize candidate mutation or another entry.
                    break
                if self.status(output_id=output_id).cancel_requested:
                    self._report_unknown(output_id, part_index)
                    break
                if retry_without_formatting:
                    # Only presentation fields can change. The committed
                    # request digest still identifies the original request.
                    part = {key: value for key, value in part.items()
                            if key not in {"entities", "caption_entities"}}
                if exhausted:
                    if self.status(output_id=output_id).part_states[part_index] == "unknown":
                        self._report_unknown(output_id, part_index)
                    elif not incident_delivery:
                        self.incident_sink(
                            {
                                "severity": "WARNING",
                                "kind": "egress_delivery_failed",
                                "payload": {
                                    "output_id": output_id,
                                    "part_index": part_index,
                                },
                            }
                        )
                    break
                await _await(self.sleep((5, 30, 120)[consumed_attempts - 1]))

    def _update_part(
        self,
        output_id: str,
        part_index: int,
        *,
        state: str,
        attempts: int,
        unknown_attempts: int,
        provider_msg_id: object,
        error: str | None,
        last_outcome: str | None = None,
        clear_formatting: bool = False,
        unknown_reason: str | None = None,
        require_open: bool = False,
    ) -> bool:
        if clear_formatting and (
            last_outcome != "known_not_sent" or state not in {"pending", "failed"}
        ):
            raise ValueError("formatting changes require a known-not-sent result")
        # Attempt number alone is insufficient: a different handle may have
        # recorded acceptance since a sweeper's snapshot. Terminal known facts
        # are never overwritten. A real result can refine this same reserved
        # attempt's uncertainty, but cannot supersede a newer attempt.
        if state in {"expired", "failed"} and last_outcome != "known_not_sent":
            eligible = "state = 'pending' AND attempts = 0 AND unknown_attempts = 0"
        elif state == "expired":
            eligible = "state = 'pending' AND unknown_attempts = 0 AND last_outcome = 'known_not_sent'"
        elif state == "failed":
            eligible = "state IN ('pending','unknown') AND last_outcome IN ('unknown','known_not_sent')"
        else:
            eligible = "state IN ('pending','unknown') AND last_outcome = 'unknown'"
        with closing(self._connect()) as connection, connection:
            connection.execute("BEGIN IMMEDIATE")
            retained = connection.execute(
                "SELECT p.unknown_history,p.unknown_resends_used,o.delivery_policy,o.cancel_requested "
                "FROM egress_parts p JOIN egress_outputs o USING(output_id) "
                "WHERE p.output_id=? AND p.part_index=? AND p.attempts=?",
                (output_id, part_index, attempts),
            ).fetchone()
            if retained is None:
                return False
            history = _uncertainties(retained[0])
            if history and state in {"failed", "expired"}:
                state = "unknown"
            if unknown_reason is not None:
                if unknown_reason not in _UNKNOWN_REASONS or last_outcome != "unknown":
                    raise ValueError("invalid uncertainty observation")
                if not any(row["attempt"] == attempts for row in history):
                    history.append({"attempt": attempts, "reason": unknown_reason,
                                    "provider_message_id": provider_msg_id})
                policy = _delivery_policy(retained[2])
                if (state == "unknown" and not retained[3] and attempts < 4
                        and int(retained[1]) < policy.unknown_resends):
                    state = "pending"
            presentation = ", entities = NULL, caption_entities = NULL" if clear_formatting else ""
            # Only a proved refusal can become safely cancelled. Sent/unknown
            # results refine the admitted attempt normally, even after Stop.
            next_state = (
                "CASE WHEN (SELECT cancel_requested FROM egress_outputs WHERE output_id = egress_parts.output_id) != 0 "
                + ("THEN 'unknown' ELSE ? END" if history else "THEN 'cancelled' ELSE ? END")
                if last_outcome == "known_not_sent" and state in {"pending", "failed"}
                else "?"
            )
            cursor = connection.execute(
                "UPDATE egress_parts SET state = " + next_state + ", attempts = ?, unknown_attempts = ?, "
                "provider_msg_id = ?, error = ?, last_outcome = ?, attempt_open = 0, unknown_history = ?" + presentation + " "
                "WHERE output_id = ? AND part_index = ? AND attempts = ? AND "
                + eligible + (" AND attempt_open = 1" if require_open else ""),
                (
                    state,
                    attempts,
                    unknown_attempts,
                    provider_msg_id,
                    error,
                    last_outcome,
                    _canonical_json(history).decode() if history else retained[0],
                    output_id,
                    part_index,
                    attempts,
                ),
            )
            connection.commit()
        return cursor.rowcount == 1

    def _reserve_attempt(self, output_id: str, part_index: int, attempts: int) -> bool:
        """One atomic durable reservation precedes provider entry, across handles."""
        with closing(self._connect()) as connection, connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT p.last_outcome,p.unknown_history,p.unknown_resends_used,o.delivery_policy "
                "FROM egress_parts p JOIN egress_outputs o USING(output_id) "
                "WHERE p.output_id=? AND p.part_index=? AND p.attempts=? AND p.attempt_open=0",
                (output_id, part_index, attempts),
            ).fetchone()
            if row is None or attempts >= 4:
                return False
            unknown_retry = row[0] == "unknown"
            if unknown_retry and not (
                any(item["attempt"] == attempts for item in _uncertainties(row[1]))
                and int(row[2]) < _delivery_policy(row[3]).unknown_resends
            ):
                return False
            cursor = connection.execute(
                "UPDATE egress_parts SET attempts = attempts + 1, unknown_attempts = 1, "
                "last_outcome = 'unknown', provider_msg_id = NULL, error = NULL, attempt_open = 1, "
                "unknown_resends_used = unknown_resends_used + ? "
                "WHERE output_id = ? AND part_index = ? AND state = 'pending' "
                "AND attempts = ? AND attempt_open = 0 "
                "AND (unknown_attempts = 0 OR ? = 1) "
                "AND (attempts = 0 OR last_outcome IN ('known_not_sent','unknown')) "
                "AND EXISTS (SELECT 1 FROM egress_outputs o WHERE o.output_id = egress_parts.output_id AND o.cancel_requested = 0)",
                (int(unknown_retry), output_id, part_index, attempts, int(unknown_retry)),
            )
            connection.commit()
        return cursor.rowcount == 1

    def _retire_unresolved(self, output_id: str, part_index: int, *, finalize_completed: bool = False) -> bool:
        """Retain old uncertainty and receipts; never create retry permission."""
        with closing(self._connect()) as connection, connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT p.attempts,p.attempt_open,p.unknown_history,p.unknown_resends_used,o.delivery_policy,o.cancel_requested "
                "FROM egress_parts p JOIN egress_outputs o USING(output_id) "
                "WHERE p.output_id=? AND p.part_index=? AND p.state='pending' AND p.last_outcome='unknown'",
                (output_id, part_index),
            ).fetchone()
            if row is not None and not finalize_completed and (
                not row[1] and not row[5] and int(row[0]) < 4
                and any(item["attempt"] == row[0] for item in _uncertainties(row[2]))
                and int(row[3]) < _delivery_policy(row[4]).unknown_resends
            ):
                return False
            cursor = connection.execute(
                "UPDATE egress_parts SET state = 'unknown', last_outcome = 'unknown', "
                "error = COALESCE(error, 'previous provider attempt has no known-not-sent receipt') "
                "WHERE output_id = ? AND part_index = ? AND state = 'pending' "
                "AND (unknown_attempts > 0 OR (attempts > 0 "
                "AND (last_outcome IS NULL OR last_outcome != 'known_not_sent')))",
                (output_id, part_index),
            )
            connection.commit()
        return cursor.rowcount == 1

    @staticmethod
    def _terminal_unknown_fact(output_id: str, part_index: int, row: tuple[Any, ...] | None) -> dict[str, Any] | None:
        if (row is not None and row[0] in {"unknown", "refused", "cancelled", "failed", "expired"}
                and not row[5] and not row[6]
                and (row[0] == "unknown" or _uncertainties(row[7]))):
            return {"kind": "egress_terminal_unknown", "output_id": output_id,
                    "part_index": part_index, "state": row[0], "attempts": row[1],
                    "unknown_resends_used": row[2], "delivery_policy": asdict(_delivery_policy(row[3]))}
        return None

    def _unknown_rows(self, output_id: str) -> list[tuple[Any, ...]]:
        with closing(self._connect()) as connection, connection:
            return connection.execute(
                "SELECT p.state,p.attempts,p.unknown_resends_used,o.delivery_policy,o.cancel_requested,"
                "o.incident_delivery,p.attempt_open,p.unknown_history,p.part_index "
                "FROM egress_parts p JOIN egress_outputs o USING(output_id) "
                "WHERE p.output_id=? ORDER BY p.part_index", (output_id,),
            ).fetchall()

    def report_terminal_uncertainty(self, output_id: str) -> int:
        """Re-emit current settled facts without changing delivery state.

        Only the existing health callback runs. A returned count is not durable
        acknowledgment or exactly-once handling; the host owns reconciliation,
        routing and deduplication. Callback failures propagate with rows intact.
        """
        if type(output_id) is not str or not output_id:
            raise ValueError("output_id is required")
        rows = self._unknown_rows(output_id)
        if not rows:
            raise ValueError("unknown output_id")
        count = 0
        for row in rows:
            fact = self._terminal_unknown_fact(output_id, row[8], row)
            if fact is not None:
                self.health_sink(fact)
                count += 1
        return count

    def _report_unknown(self, output_id: str, part_index: int) -> None:
        """Notify existing host sinks; durable evidence remains on failure."""
        row = next((row for row in self._unknown_rows(output_id) if row[8] == part_index), None)
        fact = self._terminal_unknown_fact(output_id, part_index, row)
        try:
            if fact is not None:
                self.health_sink(fact)
        finally:
            # The new health path cannot suppress the prior eligible incident
            # callback. Neither callback's return is a durable acknowledgment.
            if (row is not None and row[0] == "unknown" and not row[4] and not row[5] and not row[6]
                    and _delivery_policy(row[3]).unknown_resends):
                self.incident_sink({"severity": "WARNING", "kind": "egress_delivery_unknown",
                    "payload": {"output_id": output_id, "part_index": part_index,
                                "attempts": row[1], "unknown_resends_used": row[2]}})

    def _normalize_parts(
        self, parts: Sequence[Mapping[str, Any]]
    ) -> tuple[dict[str, Any], ...]:
        if self._part_normalizer is not None:
            parts = self._part_normalizer(parts)
            allowed = {
                "text",
                "caption",
                "mime_type",
                "media_ref",
                "entities",
                "caption_entities",
            }
        else:
            allowed = {"text", "caption", "mime_type", "media_ref"}
        extension = self._host_extension
        core_allowed = allowed
        if extension is not None:
            allowed = allowed | set(extension.part_columns)
        if not parts:
            raise ValueError("an output requires at least one part")
        normalized: list[dict[str, Any]] = []
        for part in parts:
            if not isinstance(part, Mapping) or set(part) - allowed:
                raise ValueError(
                    "egress part has unknown fields or requires host normalization"
                )
            text = part.get("text")
            media_ref = part.get("media_ref")
            if (isinstance(text, str) and bool(text)) == (
                isinstance(media_ref, str) and bool(media_ref)
            ):
                raise ValueError(
                    "egress part must contain exactly one of text or media_ref"
                )
            row = {key: value for key, value in part.items() if key in core_allowed and value is not None}
            if extension is not None:
                row.update(extension._parts({key: part[key] for key in extension.part_columns if key in part}))
            if text is not None and self._part_normalizer is None:
                row["text"] = unicodedata.normalize("NFC", str(text))
            # The host normalizer owns text and formatting together. Rewriting
            # its text here would invalidate character-based entity offsets.
            # Freeze nested caller-owned values before deriving the digest or
            # invoking providers; callbacks cannot mutate a committed payload.
            normalized.append(json.loads(_canonical_json(row)))
        return tuple(normalized)
