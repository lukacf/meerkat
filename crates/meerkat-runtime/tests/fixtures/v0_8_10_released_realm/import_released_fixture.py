#!/usr/bin/env python3
"""Import a synthetic realm written by the published Meerkat 0.8.10 rkat."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import subprocess
import tempfile
from typing import Any, NoReturn


MEERKAT_FLOOR_VERSION = "0.8.10"
RELEASED_VERSION_OUTPUT = "rkat 0.8.10"
PUBLIC_RELEASE = "https://github.com/lukacf/meerkat/releases/tag/v0.8.10"
FIXTURE_SCHEMA = 1
CAPTURE_SCHEMA = 1
TRANSIENT_NAMES = {".DS_Store", "leases", ".leases"}


def refuse(message: str) -> NoReturn:
    raise SystemExit(f"fixture import refused: {message}")


def read_json(path: Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        refuse(f"{label} is not readable JSON: {error}")
    if not isinstance(value, dict):
        refuse(f"{label} must be a JSON object")
    return value


def canonical_json(value: Any) -> bytes:
    return (
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        + "\n"
    ).encode("utf-8")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require_sha256(value: Any, label: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value):
        refuse(f"{label} must be 64 lowercase hexadecimal characters")
    return value


def safe_relative(raw: Any, label: str) -> Path:
    if not isinstance(raw, str) or not raw:
        refuse(f"{label} must be a non-empty relative path")
    path = Path(raw)
    if path.is_absolute() or any(part in ("", ".", "..") for part in path.parts):
        refuse(f"{label} must be a normalized relative path")
    return path


def sqlite_read_only(path: Path) -> sqlite3.Connection:
    # WAL/SHM sidecars are rejected before this is called. Immutable mode
    # observes the clean-shutdown image without creating SQLite artifacts.
    uri = f"file:{path.resolve().as_posix()}?mode=ro&immutable=1"
    return sqlite3.connect(uri, uri=True)


def table_exists(connection: sqlite3.Connection, table: str) -> bool:
    row = connection.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1",
        (table,),
    ).fetchone()
    return row is not None


def ledger_rows(connection: sqlite3.Connection) -> list[tuple[str, int]]:
    if not table_exists(connection, "meerkat_schema"):
        refuse("SQLite database has no meerkat_schema migration ledger")
    rows = connection.execute(
        "SELECT domain, version FROM meerkat_schema ORDER BY domain"
    ).fetchall()
    return [(str(domain), int(version)) for domain, version in rows]


def require_released_runtime_snapshot_schema(
    connection: sqlite3.Connection,
) -> None:
    columns = connection.execute(
        "PRAGMA table_info(runtime_session_snapshots)"
    ).fetchall()
    exact = [
        (0, "runtime_id", "TEXT", 0, None, 1),
        (1, "session_snapshot", "BLOB", 1, None, 0),
    ]
    if columns != exact:
        refuse(
            "runtime_session_snapshots does not have the exact released "
            f"0.8.10 schema: expected {exact}, got {columns}"
        )
    if table_exists(connection, "runtime_session_authority"):
        refuse(
            "released 0.8.10 fixture already contains the 0.8.11 "
            "runtime_session_authority table"
        )


def require_clean_realm(root: Path) -> list[Path]:
    if not root.is_dir() or root.is_symlink():
        refuse("realm root must be a real directory, not a symlink")
    files: list[Path] = []
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if path.is_symlink():
            refuse(f"realm contains symlink {relative}")
        if any(part in TRANSIENT_NAMES for part in relative.parts):
            refuse(f"realm contains transient artifact {relative}")
        if path.name.endswith(("-wal", "-shm")):
            refuse(f"realm contains uncheckpointed SQLite sidecar {relative}")
        if path.is_dir():
            continue
        if not path.is_file():
            refuse(f"realm contains a non-regular filesystem entry {relative}")
        if path.name.endswith(".mfence") and path.stat().st_size != 0:
            refuse(f"realm contains non-empty maintenance fence {relative}")
        if path.name.endswith(".lock"):
            allowed_parent = Path(".rkat/events/.sequence")
            if relative.parent != allowed_parent or path.stat().st_size != 0:
                refuse(f"realm contains unexpected or non-empty lock {relative}")
        files.append(path)
    if not files:
        refuse("realm root is empty")
    return files


def require_capture_receipt(receipt: dict[str, Any]) -> None:
    required = {
        "schema_version": CAPTURE_SCHEMA,
        "data_classification": "synthetic_non_production",
        "producer": "meerkat-release",
        "meerkat_version": MEERKAT_FLOOR_VERSION,
        "writer_binary_version_output": RELEASED_VERSION_OUTPUT,
        "source_release": PUBLIC_RELEASE,
        "current_source_build": False,
        "clean_shutdown": True,
        "sanitization_method": "synthetic_inputs_before_capture",
    }
    for key, expected in required.items():
        if receipt.get(key) != expected:
            refuse(f"capture receipt {key!r} must equal {expected!r}")
    for key in (
        "writer_binary_path",
        "release_asset",
        "capture_command",
        "captured_at",
    ):
        if not isinstance(receipt.get(key), str) or not receipt[key].strip():
            refuse(f"capture receipt {key!r} must be a non-empty string")
    require_sha256(receipt.get("writer_binary_sha256"), "capture receipt writer SHA-256")
    require_sha256(receipt.get("release_asset_sha256"), "capture receipt asset SHA-256")


def require_released_binary(
    binary: Path,
    expected_sha256: str,
    source_release: str,
    capture_receipt: dict[str, Any],
) -> tuple[str, str]:
    if binary.is_symlink():
        refuse("released binary path must not be a symlink")
    binary = binary.resolve()
    repository_root = Path(__file__).resolve().parents[4]
    if not binary.is_file():
        refuse("released binary must be a regular non-symlink file")
    if repository_root == binary or repository_root in binary.parents:
        refuse("producer binary is inside the source checkout under test")
    if "target" in binary.parts:
        refuse("producer binary is under a target/ directory")
    if source_release != PUBLIC_RELEASE:
        refuse(f"source release must be the published Meerkat {PUBLIC_RELEASE}")
    if capture_receipt["source_release"] != source_release:
        refuse("source release differs from the capture receipt")
    actual_sha256 = sha256_file(binary)
    if actual_sha256 != expected_sha256:
        refuse(
            f"released binary SHA-256 mismatch: expected {expected_sha256}, "
            f"got {actual_sha256}"
        )
    try:
        version_output = subprocess.run(
            [os.fspath(binary), "--version"],
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError) as error:
        refuse(f"released binary version probe failed: {error}")
    if version_output != RELEASED_VERSION_OUTPUT:
        refuse(
            "released writer version output is not the exact Meerkat 0.8.10 "
            f"rkat: {version_output!r}"
        )
    return version_output, actual_sha256


def require_non_empty_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        refuse(f"{label} must be a non-empty string")
    return value


def validate_raw_expectations(root: Path, expected: dict[str, Any]) -> None:
    if expected.get("schema_version") != FIXTURE_SCHEMA:
        refuse(f"expectations schema_version must be {FIXTURE_SCHEMA}")
    for key in ("fixture_id", "realm_id"):
        require_non_empty_string(expected.get(key), f"expectations {key!r}")

    manifest = read_json(root / "realm_manifest.json", "realm manifest")
    if manifest.get("realm_id") != expected["realm_id"]:
        refuse("realm manifest identity differs from expectations")
    if manifest.get("backend") != "sqlite":
        refuse("released upgrade fixture must be a SQLite realm")

    sqlite_relative = safe_relative(
        expected.get("sqlite_database"), "sqlite_database"
    )
    database = root / sqlite_relative
    if not database.is_file() or database.is_symlink():
        refuse(f"required SQLite database {sqlite_relative} is absent")

    raw_ledgers = expected.get("pre_upgrade_ledgers")
    if not isinstance(raw_ledgers, list) or not raw_ledgers:
        refuse("pre_upgrade_ledgers must be a non-empty array")
    declared_ledgers: list[tuple[str, int]] = []
    for index, row in enumerate(raw_ledgers):
        if not isinstance(row, dict):
            refuse(f"pre_upgrade_ledgers[{index}] must be an object")
        domain = require_non_empty_string(
            row.get("domain"), f"pre_upgrade_ledgers[{index}].domain"
        )
        version = row.get("version")
        if not isinstance(version, int) or isinstance(version, bool) or version < 1:
            refuse(f"pre_upgrade_ledgers[{index}].version is invalid")
        declared_ledgers.append((domain, version))
    if len(set(declared_ledgers)) != len(declared_ledgers):
        refuse("pre_upgrade_ledgers contains duplicate rows")

    sessions = expected.get("sessions")
    if not isinstance(sessions, list) or not sessions:
        refuse("expectations must name at least one HeadCanonical session")
    expected_session_ids: set[str] = set()
    with sqlite_read_only(database) as connection:
        actual_ledgers = ledger_rows(connection)
        if actual_ledgers != sorted(declared_ledgers):
            refuse(
                "SQLite ledger differs from expectations: "
                f"expected {sorted(declared_ledgers)}, got {actual_ledgers}"
            )
        for required in (
            ("session-store", 2),
            ("runtime-store", 1),
            ("schedule-store", 1),
        ):
            if required not in declared_ledgers:
                refuse(f"fixture does not bind released ledger {required!r}")
        for table in (
            "session_heads",
            "session_strand_messages",
            "session_rewrites",
            "runtime_input_states",
            "runtime_session_snapshots",
        ):
            if not table_exists(connection, table):
                refuse(f"released database has no {table} table")
        require_released_runtime_snapshot_schema(connection)

        actual_heads = {
            str(session_id): {
                "strand": str(strand),
                "message_count": int(message_count),
                "head_revision": str(head_revision),
                "rewrite_count": int(rewrite_count),
            }
            for (
                session_id,
                strand,
                message_count,
                head_revision,
                rewrite_count,
            ) in connection.execute(
                "SELECT session_id, strand, message_count, head_revision, "
                "rewrite_count FROM session_heads ORDER BY session_id"
            )
        }
        for index, row in enumerate(sessions):
            if not isinstance(row, dict):
                refuse(f"sessions[{index}] must be an object")
            session_id = require_non_empty_string(
                row.get("session_id"), f"sessions[{index}].session_id"
            )
            if session_id in expected_session_ids:
                refuse(f"duplicate expected session id {session_id}")
            expected_session_ids.add(session_id)
            actual = actual_heads.get(session_id)
            if actual is None:
                refuse(f"expected session {session_id} has no session_heads row")
            for key in ("strand", "message_count", "head_revision", "rewrite_count"):
                if row.get(key) != actual[key]:
                    refuse(
                        f"session {session_id} {key} differs: "
                        f"expected {row.get(key)!r}, got {actual[key]!r}"
                    )
            messages = row.get("messages")
            if not isinstance(messages, list) or len(messages) != actual["message_count"]:
                refuse(
                    f"session {session_id} messages must bind its exact "
                    f"{actual['message_count']}-row head"
                )
            raw_messages = connection.execute(
                "SELECT seq, message_json, typeof(message_json) "
                "FROM session_strand_messages "
                "WHERE session_id = ? AND strand = ? ORDER BY seq",
                (session_id, actual["strand"]),
            ).fetchall()
            if len(raw_messages) != len(messages):
                refuse(f"session {session_id} message row count differs")
            for message_index, (declared, raw) in enumerate(zip(messages, raw_messages)):
                if not isinstance(declared, dict):
                    refuse(
                        f"sessions[{index}].messages[{message_index}] must be an object"
                    )
                sequence, message_json, storage_class = raw
                if storage_class != "blob" or not isinstance(message_json, bytes):
                    refuse(
                        f"session {session_id} message {sequence} is not an exact BLOB"
                    )
                digest = require_sha256(
                    declared.get("sha256"),
                    f"sessions[{index}].messages[{message_index}].sha256",
                )
                if (
                    declared.get("sequence") != sequence
                    or declared.get("bytes") != len(message_json)
                    or digest != sha256_bytes(message_json)
                ):
                    refuse(
                        f"session {session_id} message {sequence} bytes differ "
                        "from expectations"
                    )
        if set(actual_heads) != expected_session_ids:
            refuse("expectations do not bind the exact session_heads identity set")

        runtime_snapshots = expected.get("runtime_snapshots")
        if not isinstance(runtime_snapshots, list) or not runtime_snapshots:
            refuse("expectations must bind every released runtime session snapshot")
        expected_snapshots: dict[str, dict[str, Any]] = {}
        for index, row in enumerate(runtime_snapshots):
            if not isinstance(row, dict):
                refuse(f"runtime_snapshots[{index}] must be an object")
            runtime_id = require_non_empty_string(
                row.get("runtime_id"), f"runtime_snapshots[{index}].runtime_id"
            )
            session_id = require_non_empty_string(
                row.get("session_id"), f"runtime_snapshots[{index}].session_id"
            )
            if session_id not in expected_session_ids:
                refuse(
                    f"runtime snapshot {runtime_id} names undeclared session {session_id}"
                )
            if runtime_id in expected_snapshots:
                refuse(f"duplicate expected runtime snapshot {runtime_id}")
            byte_count = row.get("bytes")
            if not isinstance(byte_count, int) or isinstance(byte_count, bool):
                refuse(f"runtime_snapshots[{index}].bytes is invalid")
            expected_snapshots[runtime_id] = {
                "session_id": session_id,
                "bytes": byte_count,
                "sha256": require_sha256(
                    row.get("sha256"), f"runtime_snapshots[{index}].sha256"
                ),
            }
        actual_snapshots = {
            str(runtime_id): snapshot
            for runtime_id, snapshot, storage_class in connection.execute(
                "SELECT runtime_id, session_snapshot, typeof(session_snapshot) "
                "FROM runtime_session_snapshots ORDER BY runtime_id"
            )
            if storage_class == "blob" and isinstance(snapshot, bytes)
        }
        if set(actual_snapshots) != set(expected_snapshots):
            refuse("runtime snapshot expectations do not bind the exact row set")
        for runtime_id, snapshot in actual_snapshots.items():
            declared = expected_snapshots[runtime_id]
            if (
                len(snapshot) != declared["bytes"]
                or sha256_bytes(snapshot) != declared["sha256"]
            ):
                refuse(f"runtime snapshot {runtime_id} bytes differ")
            try:
                document = json.loads(snapshot)
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                refuse(f"runtime snapshot {runtime_id} is not session JSON: {error}")
            if (
                not isinstance(document, dict)
                or document.get("version") != 2
                or document.get("id") != declared["session_id"]
            ):
                refuse(
                    f"runtime snapshot {runtime_id} does not bind the declared "
                    "released v2 session identity"
                )

        consumed_inputs = expected.get("consumed_inputs")
        if not isinstance(consumed_inputs, list) or not consumed_inputs:
            refuse("expectations must bind every released consumed input")
        expected_inputs: dict[tuple[str, str], dict[str, Any]] = {}
        for index, row in enumerate(consumed_inputs):
            if not isinstance(row, dict):
                refuse(f"consumed_inputs[{index}] must be an object")
            runtime_id = require_non_empty_string(
                row.get("runtime_id"), f"consumed_inputs[{index}].runtime_id"
            )
            input_id = require_non_empty_string(
                row.get("input_id"), f"consumed_inputs[{index}].input_id"
            )
            last_run_id = require_non_empty_string(
                row.get("last_run_id"), f"consumed_inputs[{index}].last_run_id"
            )
            boundary = row.get("last_boundary_sequence")
            byte_count = row.get("bytes")
            if not isinstance(boundary, int) or isinstance(boundary, bool) or boundary < 0:
                refuse(
                    f"consumed_inputs[{index}].last_boundary_sequence is invalid"
                )
            if (
                not isinstance(byte_count, int)
                or isinstance(byte_count, bool)
                or byte_count < 1
            ):
                refuse(f"consumed_inputs[{index}].bytes is invalid")
            key = (runtime_id, input_id)
            if key in expected_inputs:
                refuse(f"duplicate expected consumed input {runtime_id}/{input_id}")
            expected_inputs[key] = {
                "last_run_id": last_run_id,
                "last_boundary_sequence": boundary,
                "bytes": byte_count,
                "sha256": require_sha256(
                    row.get("sha256"), f"consumed_inputs[{index}].sha256"
                ),
            }
        actual_inputs: dict[tuple[str, str], bytes] = {}
        for runtime_id, input_id, state_json, storage_class in connection.execute(
            "SELECT runtime_id, input_id, state_json, typeof(state_json) "
            "FROM runtime_input_states ORDER BY runtime_id, input_id"
        ):
            if storage_class != "blob" or not isinstance(state_json, bytes):
                refuse(f"runtime input {runtime_id}/{input_id} is not an exact BLOB")
            actual_inputs[(str(runtime_id), str(input_id))] = state_json
        if set(actual_inputs) != set(expected_inputs):
            refuse("consumed-input expectations do not bind the exact row set")
        for key, state_bytes in actual_inputs.items():
            declared = expected_inputs[key]
            if (
                len(state_bytes) != declared["bytes"]
                or sha256_bytes(state_bytes) != declared["sha256"]
            ):
                refuse(f"runtime input {key[0]}/{key[1]} bytes differ")
            try:
                state = json.loads(state_bytes)
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                refuse(f"runtime input {key[0]}/{key[1]} is not JSON: {error}")
            if (
                state.get("current_state") != "consumed"
                or state.get("last_run_id") != declared["last_run_id"]
                or state.get("last_boundary_sequence")
                != declared["last_boundary_sequence"]
                or state.get("terminal_outcome") != {"outcome_type": "consumed"}
                or state.get("persisted_input") is None
            ):
                refuse(
                    f"runtime input {key[0]}/{key[1]} is not the declared "
                    "durably consumed input"
                )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--released-binary", required=True, type=Path)
    parser.add_argument("--released-binary-sha256", required=True)
    parser.add_argument("--source-release", required=True)
    parser.add_argument("--realm-root", required=True, type=Path)
    parser.add_argument("--capture-receipt", required=True, type=Path)
    parser.add_argument("--expectations", required=True, type=Path)
    parser.add_argument("--output-corpus", required=True, type=Path)
    arguments = parser.parse_args()

    expected_binary_sha256 = require_sha256(
        arguments.released_binary_sha256, "released binary SHA-256"
    )
    if not arguments.capture_receipt.is_file() or arguments.capture_receipt.is_symlink():
        refuse("capture receipt must be a regular non-symlink file")
    receipt = read_json(arguments.capture_receipt, "capture receipt")
    require_capture_receipt(receipt)
    if expected_binary_sha256 != receipt["writer_binary_sha256"]:
        refuse(
            "independently supplied binary SHA-256 differs from the capture "
            "receipt writer SHA-256"
        )
    version_output, binary_sha256 = require_released_binary(
        arguments.released_binary,
        expected_binary_sha256,
        arguments.source_release,
        receipt,
    )
    expectations = read_json(arguments.expectations, "expectations")
    if arguments.realm_root.is_symlink():
        refuse("realm root path must not be a symlink")
    realm_root = arguments.realm_root.resolve()
    source_files = require_clean_realm(realm_root)
    validate_raw_expectations(realm_root, expectations)

    output = arguments.output_corpus.resolve()
    if output.exists() or output.is_symlink():
        refuse("output corpus already exists; replacement requires explicit review/removal")
    output.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(
        prefix=f".{output.name}.incoming.", dir=output.parent
    ) as temporary:
        staging = Path(temporary)
        staged_realm = staging / "realm"
        shutil.copytree(realm_root, staged_realm, symlinks=False)
        files = []
        for source in source_files:
            relative = source.relative_to(realm_root)
            staged = staged_realm / relative
            files.append(
                {
                    "path": (Path("realm") / relative).as_posix(),
                    "bytes": staged.stat().st_size,
                    "sha256": sha256_file(staged),
                }
            )
        staged_receipt = staging / "provenance" / "capture-receipt.json"
        staged_receipt.parent.mkdir()
        shutil.copyfile(arguments.capture_receipt, staged_receipt)
        files.append(
            {
                "path": "provenance/capture-receipt.json",
                "bytes": staged_receipt.stat().st_size,
                "sha256": sha256_file(staged_receipt),
            }
        )
        files.sort(key=lambda row: row["path"])

        sqlite_relative = safe_relative(
            expectations["sqlite_database"], "sqlite_database"
        )
        manifest = {
            "schema_version": FIXTURE_SCHEMA,
            "fixture_id": expectations["fixture_id"],
            "data_classification": "synthetic_non_production",
            "producer": {
                "artifact_origin": "published_release",
                "product": "rkat",
                "meerkat_version": MEERKAT_FLOOR_VERSION,
                "binary_name": arguments.released_binary.name,
                "binary_version_output": version_output,
                "binary_sha256": binary_sha256,
                "source_release": arguments.source_release,
                "capture_receipt_path": "provenance/capture-receipt.json",
                "capture_receipt_sha256": sha256_file(staged_receipt),
            },
            "realm": {
                "root": "realm",
                "realm_id": expectations["realm_id"],
                "manifest": "realm/realm_manifest.json",
                "sqlite_database": (Path("realm") / sqlite_relative).as_posix(),
                "pre_upgrade_ledgers": expectations["pre_upgrade_ledgers"],
            },
            "files": files,
            "expected": {
                "sessions": expectations["sessions"],
                "runtime_snapshots": expectations["runtime_snapshots"],
                "consumed_inputs": expectations["consumed_inputs"],
            },
        }
        (staging / "fixture-manifest.json").write_bytes(canonical_json(manifest))
        os.replace(staging, output)

    print(f"imported published Meerkat 0.8.10 fixture: {output}")


if __name__ == "__main__":
    main()
