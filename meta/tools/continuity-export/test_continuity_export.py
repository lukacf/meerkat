#!/usr/bin/env python3
"""Contracts for tools/continuity-export/continuity_export.py.

The provider fixtures behave like a host's 0.9.3 provider object: their result
values carry the 0.9.3 ``to_dict`` wire shapes, copied here because the export
runs in the host's 0.9.3 environment and imports nothing from the SDK.

JsonlProviderTests covers jsonl_provider.py, the provider over two JSON Lines
files; continuity-export.test.mjs runs the same cases against
jsonl-provider.mjs.

PublishedPinTests checks the pins that CHANGELOG.md and docs/ publish for the
scripts and the providers against their bytes.

Run: python3 tools/continuity-export/test_continuity_export.py
Rewrite this script's golden file: META_WRITE_CONTINUITY_EXPORT_FIXTURE=1 python3 ...
"""
from __future__ import annotations

import asyncio
import base64
import errno
import gzip
import hashlib
import importlib.util
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
import textwrap
import tracemalloc
import unittest
from dataclasses import dataclass, replace
from unittest import mock
from pathlib import Path
from typing import Any, Callable

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SCRIPT = HERE / "continuity_export.py"
FIXTURES = ROOT / "crates/meerkat-meta/tests/fixtures/continuity_export_v1"
WRITE_FIXTURE_ENV = "META_WRITE_CONTINUITY_EXPORT_FIXTURE"

SPEC = importlib.util.spec_from_file_location("continuity_export", SCRIPT)
ce = importlib.util.module_from_spec(SPEC)
sys.modules["continuity_export"] = ce
SPEC.loader.exec_module(ce)

JSONL_PROVIDER = HERE / "jsonl_provider.py"
JSONL_SPEC = importlib.util.spec_from_file_location("jsonl_provider", JSONL_PROVIDER)
jp = importlib.util.module_from_spec(JSONL_SPEC)
sys.modules["jsonl_provider"] = jp
JSONL_SPEC.loader.exec_module(jp)
H_REMOTE = ROOT / "crates/meerkat-meta/tests/fixtures/released_0_9_3_h_remote"

SESSION_ALPHA = "019ecb20-4899-7292-a2b4-c81bc7a44258"
SESSION_SMILE = "019f0000-0000-7000-8000-000000000001"
SESSION_OTHER = "019f0000-0000-7000-8000-000000000002"
SESSION_RENAMED = "019f0000-0000-7000-8000-000000000003"
IDENTITY_HALFWIDTH = "agent:\uff61"
IDENTITY_SMILE = "agent:\U0001f600"
DOC_ALPHA = b'{"synthetic":"continuity export golden document alpha"}\n'
DOC_SMILE = bytes(range(256))


# ---------------------------------------------------------------------------
# 0.9.3 result shapes (the to_dict wire forms the 0.9.3 SDK sent the gateway)
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Record093:
    identity: str
    agent_runtime_id: str
    session_id: str
    generation: int
    checkpoint_version: int

    def to_dict(self) -> dict[str, Any]:
        return {
            "identity": self.identity,
            "agent_runtime_id": self.agent_runtime_id,
            "session_id": self.session_id,
            "generation": self.generation,
            "checkpoint_version": self.checkpoint_version,
        }


@dataclass(frozen=True)
class Failure093:
    identity: str
    kind: str
    record: Record093 | None
    detail: str

    def to_dict(self) -> dict[str, Any]:
        return {
            "identity": self.identity,
            "kind": self.kind,
            "detail": self.detail,
            "record": self.record.to_dict() if self.record else None,
        }


@dataclass
class ResolveState093:
    state: str
    record: Record093 | None = None
    failure: Failure093 | None = None

    def to_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"state": self.state}
        if self.record is not None:
            result["record"] = self.record.to_dict()
        if self.failure is not None:
            result["failure"] = self.failure.to_dict()
        return result


@dataclass(frozen=True)
class Snapshot093:
    data: bytes

    def to_dict(self) -> dict[str, Any]:
        return {"data": base64.b64encode(self.data).decode()}


class Store093:
    """An in-memory 0.9.3 SDK continuity provider. The write methods fail the test."""

    def __init__(
        self,
        records: dict[str, Record093] | None = None,
        snapshots: dict[str, bytes] | None = None,
    ) -> None:
        self.records = dict(records or {})
        self.snapshots = dict(snapshots or {})
        self.broken: dict[str, Failure093] = {}
        self.calls: list[tuple[str, Any]] = []

    async def resolve_many(self, identities):
        self.calls.append(("resolve_many", tuple(identities)))
        answer = {}
        for identity in identities:
            if identity in self.broken:
                answer[identity] = ResolveState093("broken", failure=self.broken[identity])
            elif identity in self.records:
                answer[identity] = ResolveState093("ready", record=self.records[identity])
            else:
                answer[identity] = ResolveState093("uninitialized")
        return answer

    async def load_session_snapshot(self, session_id):
        self.calls.append(("load_session_snapshot", session_id))
        data = self.snapshots.get(session_id)
        return None if data is None else Snapshot093(data)

    async def resolve_record_by_session(self, session_id):
        self.calls.append(("resolve_record_by_session", session_id))
        for record in self.records.values():
            if record.session_id == session_id:
                return record, 7, record.checkpoint_version
        return None

    async def save_session_snapshot(self, *args):
        raise AssertionError("the export must not write to the provider")

    async def upsert_continuity_record(self, *args):
        raise AssertionError("the export must not write to the provider")

    async def delete_continuity_record(self, *args):
        raise AssertionError("the export must not write to the provider")


def golden_store() -> Store093:
    """The store every golden export file is written from (the Node test uses the same)."""
    return Store093(
        records={
            "agent:alpha": Record093("agent:alpha", "rt-alpha", SESSION_ALPHA, 2, 7),
            IDENTITY_SMILE: Record093(IDENTITY_SMILE, "rt-smile", SESSION_SMILE, 0, 1),
        },
        snapshots={SESSION_ALPHA: DOC_ALPHA, SESSION_SMILE: DOC_SMILE},
    )


# Unordered on purpose: the file orders entries by identity code points.
GOLDEN_IDENTITIES = [IDENTITY_SMILE, "agent:beta", IDENTITY_HALFWIDTH, "agent:alpha"]


def ready_store(**snapshot_or_state) -> Any:
    """A provider with one ready identity agent:a; overrides replace either answer."""

    class Store:
        async def resolve_many(self, identities):
            return {
                "agent:a": snapshot_or_state.get(
                    "state",
                    ResolveState093("ready", record=Record093("agent:a", "rt", SESSION_ALPHA, 0, 1)),
                )
            }

        async def load_session_snapshot(self, session_id):
            return snapshot_or_state.get("snapshot", Snapshot093(b"doc"))

    return Store()


def split_file(data: bytes) -> tuple[dict[str, Any], bytes]:
    """The header, and the entry bytes it describes (every entry line with its newline)."""
    header, entries = data.split(b"\n", 1)
    return json.loads(header), entries


def entry_lines(data: bytes) -> list[bytes]:
    _, entries = split_file(data)
    lines = entries.split(b"\n")
    assert lines.pop() == b""
    return lines


def frame_bytes(data: bytes, /, **header_overrides: Any) -> bytes:
    """A file whose header describes exactly the entry bytes ``data``, unless overridden."""
    header: dict[str, Any] = {
        "format": ce.CONTINUITY_EXPORT_FORMAT,
        "version": ce.CONTINUITY_EXPORT_VERSION,
        "writer": "python",
        "entries": data.count(b"\n") + (0 if data.endswith(b"\n") or not data else 1),
        "entries_length": len(data),
        "entries_sha256": hashlib.sha256(data).hexdigest(),
    }
    header.update(header_overrides)
    return compact(header).encode("utf-8") + b"\n" + data


def frame(lines: list[bytes | str], /, **header_overrides: Any) -> bytes:
    """A file with these entry lines, each ending in a newline."""
    encoded = [line.encode("utf-8") if isinstance(line, str) else line for line in lines]
    return frame_bytes(b"".join(line + b"\n" for line in encoded), **header_overrides)


def compact(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def ready_entry(identity: str, session_id: str, document: bytes) -> dict[str, Any]:
    return {
        "identity": identity,
        "state": "ready",
        "record": {
            "identity": identity,
            "agent_runtime_id": "rt-1",
            "session_id": session_id,
            "generation": 0,
            "checkpoint_version": 1,
        },
        "document": {
            "length": len(document),
            "sha256": hashlib.sha256(document).hexdigest(),
            "base64": base64.b64encode(document).decode("ascii"),
        },
    }


def baseline_entries() -> list[dict[str, Any]]:
    return [
        ready_entry("agent:a", SESSION_ALPHA, b"doc-a"),
        {"identity": "agent:b", "state": "uninitialized"},
    ]


def no_partial_files(directory: Path) -> bool:
    return not [name for name in os.listdir(directory) if name.endswith(".partial")]


def document_of(entry: Any) -> tuple[int, str] | None:
    """What an entry says about its document: its length and SHA-256."""
    if entry.document_length is None:
        return None
    return entry.document_length, entry.document_sha256


def described(document: bytes) -> tuple[int, str]:
    return len(document), hashlib.sha256(document).hexdigest()


class TempDirTestCase(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def assert_golden_entries(self, export) -> None:
        self.assertEqual(
            [entry.identity for entry in export.entries],
            ["agent:alpha", "agent:beta", IDENTITY_HALFWIDTH, IDENTITY_SMILE],
        )
        alpha, beta, halfwidth, smile = export.entries
        self.assertEqual(alpha.state, "ready")
        self.assertEqual(
            alpha.record,
            ce.ContinuityExportRecord("agent:alpha", "rt-alpha", SESSION_ALPHA, 2, 7),
        )
        self.assertEqual(document_of(alpha), described(DOC_ALPHA))
        for entry in (beta, halfwidth):
            self.assertEqual(
                (entry.state, entry.record, entry.document_length, entry.document_sha256),
                ("uninitialized", None, None, None),
            )
        self.assertEqual(smile.state, "ready")
        self.assertEqual(smile.record.session_id, SESSION_SMILE)
        self.assertEqual(document_of(smile), described(DOC_SMILE))

    async def refused(self, provider, identities, target=None) -> ce.ContinuityExportRefusedError:
        target = target or self.tmp / "export.jsonl"
        with self.assertRaises(ce.ContinuityExportRefusedError) as caught:
            await ce.export_continuity(provider, identities, target)
        self.assertFalse(target.exists())
        self.assertTrue(no_partial_files(target.parent))
        return caught.exception


# ---------------------------------------------------------------------------
# Export and read back
# ---------------------------------------------------------------------------


class ExportTests(TempDirTestCase):
    async def test_round_trips_ready_and_uninitialized_identities(self):
        target = self.tmp / "export.jsonl"
        export = await ce.export_continuity(golden_store(), GOLDEN_IDENTITIES, target)
        self.assertEqual(export.path, str(target))
        self.assertEqual(export.version, ce.CONTINUITY_EXPORT_VERSION)
        self.assertEqual(export.writer, "python")
        self.assert_golden_entries(export)
        self.assertEqual(ce.read_continuity_export(target), export)
        header, entries = split_file(target.read_bytes())
        self.assertEqual(
            header,
            {
                "format": ce.CONTINUITY_EXPORT_FORMAT,
                "version": ce.CONTINUITY_EXPORT_VERSION,
                "writer": "python",
                "entries": 4,
                "entries_length": len(entries),
                "entries_sha256": hashlib.sha256(entries).hexdigest(),
            },
        )
        self.assertEqual(export.entries_sha256, header["entries_sha256"])
        # One identity per line, in the file's identity order.
        self.assertEqual(
            [json.loads(line)["identity"] for line in entry_lines(target.read_bytes())],
            [entry.identity for entry in export.entries],
        )

    async def test_exports_and_reads_back_an_8_mib_document(self):
        document = bytes(index % 251 for index in range(8 << 20))
        store = Store093(
            records={"agent:a": Record093("agent:a", "rt", SESSION_ALPHA, 0, 1)},
            snapshots={SESSION_ALPHA: document},
        )
        target = self.tmp / "export.jsonl"
        export = await ce.export_continuity(store, ["agent:a"], target)
        self.assertEqual(document_of(export.entries[0]), described(document))
        self.assertEqual(ce.read_continuity_export(target), export)

    async def test_writes_each_entry_before_it_loads_the_next_document(self):
        # The export streams: an entry is on disk before the next document
        # is asked for, and no document is held after its entry is written.
        store = golden_store()
        load = store.load_session_snapshot
        written_before_each_load = []

        async def load_and_look(session_id):
            pending = [name for name in os.listdir(self.tmp) if name.endswith(".entries.partial")]
            written_before_each_load.append(
                [json.loads(line)["identity"] for name in pending
                 for line in (self.tmp / name).read_bytes().splitlines()]
            )
            return await load(session_id)

        store.load_session_snapshot = load_and_look
        await ce.export_continuity(store, GOLDEN_IDENTITIES, self.tmp / "export.jsonl")
        self.assertEqual(
            written_before_each_load,
            [[], ["agent:alpha", "agent:beta", IDENTITY_HALFWIDTH]],
        )
        self.assertTrue(no_partial_files(self.tmp))

    async def test_memory_does_not_grow_with_the_store(self):
        # Exporting and reading hold one entry at a time: 48 more documents
        # are 2 MiB more base64, and neither peak may grow by a quarter of it.
        peaks = {}
        for count in (16, 64):
            documents = {
                f"019f0000-0000-7000-8000-{index:012d}": bytes([index]) * (32 << 10) for index in range(count)
            }
            store = Store093(
                records={
                    f"agent:{index:02d}": Record093(f"agent:{index:02d}", "rt", session_id, 0, 1)
                    for index, session_id in enumerate(documents)
                },
                snapshots=documents,
            )
            target = self.tmp / f"export-{count}.jsonl"
            tracemalloc.start()
            try:
                await ce.export_continuity(store, sorted(store.records), target)
                _, exporting = tracemalloc.get_traced_memory()
                tracemalloc.reset_peak()
                export = ce.read_continuity_export(target)
                _, reading = tracemalloc.get_traced_memory()
            finally:
                tracemalloc.stop()
            self.assertEqual(len(export.entries), count)
            peaks[count] = exporting, reading
        for small, large in zip(peaks[16], peaks[64]):
            self.assertLess(large - small, 512 << 10, peaks)

    async def test_only_calls_the_093_read_methods(self):
        store = golden_store()
        await ce.export_continuity(store, GOLDEN_IDENTITIES, self.tmp / "export.jsonl")
        self.assertEqual(
            {operation for operation, _ in store.calls},
            {"resolve_many", "load_session_snapshot", "resolve_record_by_session"},
        )
        self.assertEqual(
            [args for operation, args in store.calls if operation == "resolve_many"],
            [tuple(GOLDEN_IDENTITIES)],
        )

    async def test_an_unchanged_store_exports_byte_identical_files(self):
        first = self.tmp / "one.jsonl"
        second = self.tmp / "two.jsonl"
        await ce.export_continuity(golden_store(), GOLDEN_IDENTITIES, first)
        await ce.export_continuity(golden_store(), list(reversed(GOLDEN_IDENTITIES)), second)
        self.assertEqual(first.read_bytes(), second.read_bytes())

    async def test_accepts_providers_that_answer_in_wire_shape(self):
        class WireStore:
            async def resolve_many(self, identities):
                return {
                    "agent:alpha": {
                        "state": "ready",
                        "record": Record093("agent:alpha", "rt-alpha", SESSION_ALPHA, 2, 7).to_dict(),
                    },
                    "agent:beta": {"state": "uninitialized"},
                }

            def load_session_snapshot(self, session_id):
                # A plain object with bytes data and no to_dict, answered
                # synchronously: still the bytes the provider holds.
                assert session_id == SESSION_ALPHA

                class Snapshot:
                    data = DOC_ALPHA

                return Snapshot()

        export = await ce.export_continuity(WireStore(), ["agent:alpha", "agent:beta"], self.tmp / "x")
        alpha, beta = export.entries
        self.assertEqual(document_of(alpha), described(DOC_ALPHA))
        self.assertEqual(alpha.record.checkpoint_version, 7)
        self.assertEqual(beta.state, "uninitialized")

    async def test_the_file_is_private_and_no_partial_file_remains(self):
        target = self.tmp / "export.jsonl"
        await ce.export_continuity(golden_store(), GOLDEN_IDENTITIES, target)
        self.assertTrue(no_partial_files(self.tmp))
        if os.name == "posix":
            self.assertEqual(stat.S_IMODE(target.stat().st_mode) & 0o077, 0)

    async def test_never_overwrites_an_existing_file(self):
        target = self.tmp / "export.jsonl"
        target.write_bytes(b"keep me")
        with self.assertRaises(FileExistsError) as caught:
            await ce.export_continuity(golden_store(), GOLDEN_IDENTITIES, target)
        self.assertIn("already exists", str(caught.exception))
        self.assertEqual(target.read_bytes(), b"keep me")
        self.assertTrue(no_partial_files(self.tmp))

    async def test_a_file_that_appears_during_the_export_is_never_replaced(self):
        target = self.tmp / "export.jsonl"
        store = golden_store()
        load = store.load_session_snapshot

        async def load_while_another_writer_creates_the_target(session_id):
            if not target.exists():
                target.write_bytes(b"keep me")
            return await load(session_id)

        store.load_session_snapshot = load_while_another_writer_creates_the_target
        with self.assertRaises(FileExistsError) as caught:
            await ce.export_continuity(store, GOLDEN_IDENTITIES, target)
        self.assertIn("appeared while the export ran", str(caught.exception))
        self.assertEqual(target.read_bytes(), b"keep me")
        self.assertTrue(no_partial_files(self.tmp))

    def test_publishing_fails_rather_than_replace_a_file(self):
        partial, target = self.tmp / "export.partial", self.tmp / "export.jsonl"
        partial.write_bytes(b"new")
        target.write_bytes(b"keep me")
        with self.assertRaisesRegex(FileExistsError, "appeared while the export ran"):
            ce._publish_exclusive(str(partial), str(target))
        self.assertEqual(target.read_bytes(), b"keep me")
        self.assertEqual(partial.read_bytes(), b"new")
        target.unlink()
        ce._publish_exclusive(str(partial), str(target))
        self.assertEqual(target.read_bytes(), b"new")
        self.assertFalse(partial.exists())

    def test_publishing_without_hard_links_still_never_replaces_a_file(self):
        # FAT and exFAT have no hard links; the export checks and renames there.
        partial, target = self.tmp / "export.partial", self.tmp / "export.jsonl"
        partial.write_bytes(b"new")
        target.write_bytes(b"keep me")

        def no_links(source, destination):
            raise PermissionError(errno.EPERM, "Operation not permitted")

        with mock.patch.object(ce.os, "link", no_links):
            with self.assertRaisesRegex(FileExistsError, "appeared while the export ran"):
                ce._publish_exclusive(str(partial), str(target))
            self.assertEqual(target.read_bytes(), b"keep me")
            target.unlink()
            ce._publish_exclusive(str(partial), str(target))
        self.assertEqual(target.read_bytes(), b"new")
        self.assertFalse(partial.exists())

    async def test_accepts_every_session_id_spelling_the_093_gateway_parsed(self):
        # Core 0.9.3's SessionId parsed every spelling the uuid crate accepts
        # and gave the provider the canonical form; the file holds that form.
        spellings = [
            SESSION_ALPHA.upper(),
            SESSION_ALPHA.replace("-", ""),
            SESSION_ALPHA.replace("-", "").upper(),
            "{" + SESSION_ALPHA.upper() + "}",
            "urn:uuid:" + SESSION_ALPHA.upper(),
        ]
        for number, spelling in enumerate(spellings):
            with self.subTest(spelling=spelling):
                store = Store093(
                    records={"agent:a": Record093("agent:a", "rt", spelling, 0, 1)},
                    snapshots={SESSION_ALPHA: b"doc"},
                )
                store.resolve_record_by_session = None
                export = await ce.export_continuity(store, ["agent:a"], self.tmp / f"export-{number}.jsonl")
                (entry,) = export.entries
                self.assertEqual(entry.record.session_id, SESSION_ALPHA)
                self.assertEqual(document_of(entry), described(b"doc"))
                self.assertIn(("load_session_snapshot", SESSION_ALPHA), store.calls)

    async def test_the_session_index_may_spell_the_session_id_differently(self):
        store = Store093(
            records={"agent:a": Record093("agent:a", "rt", SESSION_ALPHA, 0, 1)},
            snapshots={SESSION_ALPHA: b"doc"},
        )
        asked = []

        async def resolve_record_by_session(session_id):
            asked.append(session_id)
            return Record093("agent:a", "rt", "{" + SESSION_ALPHA.upper() + "}", 0, 1), 7, 1

        store.resolve_record_by_session = resolve_record_by_session
        export = await ce.export_continuity(store, ["agent:a"], self.tmp / "export.jsonl")
        self.assertEqual(export.entries[0].record.session_id, SESSION_ALPHA)
        self.assertEqual(asked, [SESSION_ALPHA])


# ---------------------------------------------------------------------------
# Refusals: the whole export, nothing written, every row listed
# ---------------------------------------------------------------------------


class RefusalTests(TempDirTestCase):
    async def test_refuses_every_unexportable_row_and_writes_nothing(self):
        store = Store093(
            records={
                "agent:ok": Record093("agent:ok", "rt-ok", SESSION_ALPHA, 0, 1),
                "agent:no-snapshot": Record093("agent:no-snapshot", "rt-n", SESSION_SMILE, 0, 1),
                "agent:shared-1": Record093("agent:shared-1", "rt-s1", SESSION_OTHER, 0, 1),
                "agent:shared-2": Record093("agent:shared-2", "rt-s2", SESSION_OTHER, 0, 1),
                "agent:renamed": Record093("agent:elsewhere", "rt-r", SESSION_RENAMED, 0, 1),
            },
            snapshots={SESSION_ALPHA: b"ok", SESSION_OTHER: b"shared", SESSION_RENAMED: b"renamed"},
        )
        store.broken["agent:broken"] = Failure093("agent:broken", "snapshot_corrupted", None, "bad checksum")
        refused = await self.refused(
            store,
            ["agent:ok", "agent:no-snapshot", "agent:shared-1", "agent:shared-2",
             "agent:renamed", "agent:broken"],
        )
        self.assertEqual(
            {(refusal.identity, refusal.row_class) for refusal in refused.refusals},
            {
                ("agent:no-snapshot", ce.RUNTIME_MISSING),
                ("agent:shared-1", ce.IDENTITY_DISAGREES),
                ("agent:shared-2", ce.IDENTITY_DISAGREES),
                ("agent:renamed", ce.IDENTITY_DISAGREES),
                ("agent:broken", ce.BROKEN),
            },
        )
        rows = {refusal.identity: refusal for refusal in refused.refusals}
        self.assertIn("snapshot_corrupted", rows["agent:broken"].detail)
        self.assertIn("bad checksum", rows["agent:broken"].detail)
        self.assertEqual(rows["agent:no-snapshot"].session_id, SESSION_SMILE)
        self.assertEqual(
            rows["agent:no-snapshot"].detail,
            f"the provider holds no snapshot for session {SESSION_SMILE} "
            "(0.9.3 record: agent_runtime_id rt-n, generation 0, checkpoint_version 1)",
        )
        self.assertIn("agent:shared-2", rows["agent:shared-1"].detail)
        for refusal in refused.refusals:
            self.assertEqual(refusal.action, ce.ACTIONS[refusal.row_class])
        self.assertIsInstance(refused, ce.ContinuityExportError)

    async def test_refuses_an_answer_that_does_not_match_the_list(self):
        class Store:
            async def resolve_many(self, identities):
                return {
                    "agent:a": {"state": "uninitialized"},
                    "agent:unlisted": {"state": "uninitialized"},
                }

            async def load_session_snapshot(self, session_id):
                raise AssertionError("no ready record to load")

        refused = await self.refused(Store(), ["agent:a", "agent:b"])
        self.assertEqual(
            {(refusal.identity, refusal.row_class) for refusal in refused.refusals},
            {("agent:unlisted", ce.UNLISTED), ("agent:b", ce.UNREADABLE)},
        )

    async def test_refuses_values_outside_the_093_wire_shape(self):
        record = Record093("agent:a", "rt", SESSION_ALPHA, 0, 1).to_dict()
        cases = [
            ({"state": "warming"}, "unknown state"),
            ({"state": "ready"}, "no record"),
            ({"state": "uninitialized", "record": record}, "carries"),
            ({"state": "ready", "record": {**record, "generation": -1}}, "generation"),
            ({"state": "ready", "record": {**record, "generation": True}}, "generation"),
            ({"state": "ready", "record": {**record, "checkpoint_version": 1.0}}, "checkpoint_version"),
            # Spellings the uuid crate refused in 0.9.3 too.
            ({"state": "ready", "record": {**record, "session_id": "urn:UUID:" + SESSION_ALPHA}}, "session_id"),
            ({"state": "ready", "record": {**record, "session_id": "{" + SESSION_ALPHA}}, "session_id"),
            ({"state": "ready", "record": {**record, "session_id": SESSION_ALPHA[:-1]}}, "session_id"),
            ({"state": "ready", "record": {**record, "session_id": SESSION_ALPHA.replace("-", "", 1)}},
             "session_id"),
            ({"state": "ready", "record": {**record, "session_id": SESSION_ALPHA.replace("a", "g", 1)}},
             "session_id"),
            ({"state": "ready", "record": {**record, "agent_runtime_id": "rt 1"}}, "agent_runtime_id"),
            ({"state": "ready", "record": {"identity": "agent:a", "session_id": SESSION_ALPHA}},
             "agent_runtime_id"),
            ("ready", "not an object"),
        ]
        for wire_state, needle in cases:
            with self.subTest(wire_state=wire_state):
                refused = await self.refused(ready_store(state=wire_state), ["agent:a"])
                (refusal,) = refused.refusals
                self.assertEqual((refusal.identity, refusal.row_class), ("agent:a", ce.UNREADABLE))
                self.assertIn(needle, refusal.detail)

    async def test_refuses_unreadable_snapshots(self):
        cases = [
            ({"data": "not base64!"}, "base64"),
            ({"data": ""}, "empty"),
            (Snapshot093(b""), "empty"),
            ({"nothing": "here"}, "no data"),
            (42, "no data"),
        ]
        for snapshot, needle in cases:
            with self.subTest(snapshot=snapshot):
                refused = await self.refused(ready_store(snapshot=snapshot), ["agent:a"])
                (refusal,) = refused.refusals
                self.assertEqual((refusal.row_class, refusal.session_id), (ce.UNREADABLE, SESSION_ALPHA))
                self.assertIn(needle, refusal.detail)

    async def test_cross_checks_the_session_index_when_the_provider_has_one(self):
        cases = [
            (None, ce.IDENTITY_DISAGREES),
            ((Record093("agent:other", "rt", SESSION_ALPHA, 0, 1), 1, 1), ce.IDENTITY_DISAGREES),
            ((Record093("agent:a", "rt", SESSION_OTHER, 0, 1), 1, 1), ce.IDENTITY_DISAGREES),
            ("not a triple", ce.UNREADABLE),
        ]
        for bound, row_class in cases:
            with self.subTest(bound=bound):
                store = Store093(
                    records={"agent:a": Record093("agent:a", "rt", SESSION_ALPHA, 0, 1)},
                    snapshots={SESSION_ALPHA: b"doc"},
                )

                async def resolve_record_by_session(session_id, bound=bound):
                    return bound

                store.resolve_record_by_session = resolve_record_by_session
                refused = await self.refused(store, ["agent:a"])
                (refusal,) = refused.refusals
                self.assertEqual((refusal.identity, refusal.row_class), ("agent:a", row_class))

    async def test_without_a_session_index_the_cross_check_is_skipped(self):
        export = await ce.export_continuity(ready_store(), ["agent:a"], self.tmp / "export.jsonl")
        (entry,) = export.entries
        self.assertEqual(document_of(entry), described(b"doc"))

    async def test_two_spellings_of_one_session_are_one_shared_session(self):
        store = Store093(
            records={
                "agent:a": Record093("agent:a", "rt-a", SESSION_ALPHA, 0, 1),
                "agent:b": Record093("agent:b", "rt-b", SESSION_ALPHA.upper(), 0, 1),
            },
            snapshots={SESSION_ALPHA: b"doc"},
        )
        store.resolve_record_by_session = None
        refused = await self.refused(store, ["agent:a", "agent:b"])
        self.assertEqual(
            {(refusal.identity, refusal.row_class, refusal.session_id) for refusal in refused.refusals},
            {("agent:a", ce.IDENTITY_DISAGREES, SESSION_ALPHA), ("agent:b", ce.IDENTITY_DISAGREES, SESSION_ALPHA)},
        )


class TooLargeTests(TempDirTestCase):
    """One entry line holds at most MAX_ENTRY_LINE_BYTES, so that the Node.js
    script can hold any entry as one string. Both scripts refuse a document
    whose entry would be longer, and say so up front."""

    def store(self) -> Store093:
        return Store093(
            records={
                "agent:a": Record093("agent:a", "rt", SESSION_ALPHA, 0, 1),
                "agent:b": Record093("agent:b", "rt", SESSION_OTHER, 0, 1),
            },
            snapshots={SESSION_ALPHA: bytes(range(256)) * 3, SESSION_OTHER: b"doc"},
        )

    async def test_refuses_a_document_whose_entry_would_be_over_the_limit(self):
        fits = self.tmp / "fits.jsonl"
        await ce.export_continuity(self.store(), ["agent:a", "agent:b"], fits)
        longest = len(entry_lines(fits.read_bytes())[0])
        with mock.patch.object(ce, "MAX_ENTRY_LINE_BYTES", longest):
            await ce.export_continuity(self.store(), ["agent:a", "agent:b"], self.tmp / "exactly.jsonl")
        with mock.patch.object(ce, "MAX_ENTRY_LINE_BYTES", longest - 1):
            refused = await self.refused(self.store(), ["agent:a", "agent:b"])
        (refusal,) = refused.refusals
        self.assertEqual(
            (refusal.identity, refusal.row_class, refusal.session_id),
            ("agent:a", ce.TOO_LARGE, SESSION_ALPHA),
        )
        self.assertIn(f"{longest} bytes", refusal.detail)
        self.assertIn(f"{longest - 1}", refusal.detail)
        self.assertEqual(refusal.action, ce.ACTIONS[ce.TOO_LARGE])

    async def test_the_limit_is_the_longest_string_nodejs_can_hold(self):
        # buffer.constants.MAX_STRING_LENGTH on 64-bit Node.js 18 to 22.
        self.assertEqual(ce.MAX_ENTRY_LINE_BYTES, 536_870_888)


# ---------------------------------------------------------------------------
# Provider faults abort, are retryable, and are never written down as facts
# ---------------------------------------------------------------------------


class ProviderFaultTests(TempDirTestCase):
    async def test_a_provider_fault_aborts_and_writes_nothing(self):
        cases = [
            ("resolve_many", None),
            ("load_session_snapshot", SESSION_ALPHA),
            ("resolve_record_by_session", SESSION_ALPHA),
        ]
        for failing, subject in cases:
            with self.subTest(failing=failing):
                store = golden_store()
                cause = OSError("store unreachable")

                async def fail(*args, cause=cause):
                    raise cause

                setattr(store, failing, fail)
                target = self.tmp / f"{failing}.jsonl"
                with self.assertRaises(ce.ContinuityExportProviderError) as caught:
                    await ce.export_continuity(store, ["agent:alpha"], target)
                fault = caught.exception
                self.assertEqual((fault.operation, fault.subject, fault.retryable), (failing, subject, True))
                self.assertIs(fault.__cause__, cause)
                self.assertFalse(target.exists())
                self.assertTrue(no_partial_files(self.tmp))

    async def test_a_returned_value_that_fails_to_read_is_a_provider_fault(self):
        # Reading what a read method returned runs provider code too: a
        # to_dict(), a property or a mapping method.
        cause = OSError("store unreachable")

        class FailingDict:
            def to_dict(self):
                raise cause

        class FailingData:
            @property
            def data(self):
                raise cause

        def change_result(store, name, change):
            read = getattr(store, name)

            async def changed(*args):
                return change(await read(*args))

            setattr(store, name, changed)

        cases = [
            ("resolve_many", None, lambda answer: {**answer, "agent:alpha": FailingDict()}),
            ("load_session_snapshot", SESSION_ALPHA, lambda snapshot: FailingData()),
            ("resolve_record_by_session", SESSION_ALPHA, lambda bound: (FailingDict(), *bound[1:])),
        ]
        for failing, subject, change in cases:
            with self.subTest(failing=failing):
                store = golden_store()
                change_result(store, failing, change)
                target = self.tmp / f"{failing}.jsonl"
                with self.assertRaises(ce.ContinuityExportProviderError) as caught:
                    await ce.export_continuity(store, ["agent:alpha"], target)
                fault = caught.exception
                self.assertEqual((fault.operation, fault.subject, fault.retryable), (failing, subject, True))
                self.assertIs(fault.__cause__, cause)
                self.assertFalse(target.exists())
                self.assertTrue(no_partial_files(self.tmp))


# ---------------------------------------------------------------------------
# Caller errors
# ---------------------------------------------------------------------------


class CallerErrorTests(TempDirTestCase):
    async def test_refuses_a_malformed_identity_list_before_calling_the_provider(self):
        cases = [
            ([], ValueError),
            (["agent:a", "agent:a"], ValueError),
            ("agent:a", TypeError),
            ([1], TypeError),
            ([""], ValueError),
            (["agent a"], ValueError),
            (["agent\u3000a"], ValueError),
            (["team/agent"], ValueError),
        ]
        for identities, error in cases:
            with self.subTest(identities=identities):
                store = golden_store()
                with self.assertRaises(error):
                    await ce.export_continuity(store, identities, self.tmp / "export.jsonl")
                self.assertEqual(store.calls, [])

    async def test_requires_the_093_read_methods(self):
        class NoSnapshots:
            async def resolve_many(self, identities):
                return {}

        with self.assertRaisesRegex(TypeError, "load_session_snapshot"):
            await ce.export_continuity(NoSnapshots(), ["agent:a"], self.tmp / "export.jsonl")


# ---------------------------------------------------------------------------
# The reader refuses anything but an intact version 1 file
# ---------------------------------------------------------------------------


def _mutate_entries(change: Callable[[list[dict[str, Any]]], None]) -> bytes:
    entries = baseline_entries()
    change(entries)
    return frame([compact(entry) for entry in entries])


def _record(field: str, value: Any) -> Callable[[list[dict[str, Any]]], None]:
    return lambda entries: entries[0]["record"].__setitem__(field, value)


def _document(field: str, value: Any) -> Callable[[list[dict[str, Any]]], None]:
    return lambda entries: entries[0]["document"].__setitem__(field, value)


def _entry(index: int, field: str, value: Any) -> Callable[[list[dict[str, Any]]], None]:
    return lambda entries: entries[index].__setitem__(field, value)


def _drop(index: int, field: str) -> Callable[[list[dict[str, Any]]], None]:
    return lambda entries: entries[index].pop(field)


def _without_header_key(key: str) -> bytes:
    header_line, entries = frame(_GOOD_LINES).split(b"\n", 1)
    header = json.loads(header_line)
    del header[key]
    return compact(header).encode("utf-8") + b"\n" + entries


_GOOD_LINES = [compact(entry).encode("utf-8") for entry in baseline_entries()]
_GOOD_ENTRIES = b"".join(line + b"\n" for line in _GOOD_LINES)
_PRETTY_LINE = json.dumps(baseline_entries()[0], indent=1).replace("\n", "")

READER_REFUSALS: list[tuple[str, bytes, str]] = [
    ("empty file", b"", "header"),
    ("header without a newline", frame(_GOOD_LINES).split(b"\n", 1)[0], "header"),
    ("header lists no entries", frame([]), "no entries"),
    ("no trailing newline", frame_bytes(_GOOD_ENTRIES[:-1]), "newline"),
    ("truncated", frame(_GOOD_LINES)[:-1], "entries_length"),
    ("extra line", frame(_GOOD_LINES) + b"{}\n", "entries_length"),
    ("count mismatch", frame(_GOOD_LINES, entries=3), "entry lines"),
    ("other format", frame(_GOOD_LINES, format="something-else"), "format"),
    ("newer version", frame(_GOOD_LINES, version=2), "unsupported version 2"),
    ("extra header key", frame(_GOOD_LINES, note="x"), "header keys"),
    ("missing header key", _without_header_key("writer"), "header keys"),
    ("empty writer", frame(_GOOD_LINES, writer=""), "writer"),
    ("negative length", frame(_GOOD_LINES, entries_length=-1), "entries_length"),
    ("uppercase digest",
     frame(_GOOD_LINES, entries_sha256=hashlib.sha256(_GOOD_ENTRIES).hexdigest().upper()), "entries_sha256"),
    ("digest mismatch", frame(_GOOD_LINES).replace(b'"agent:a"', b'"agent:c"', 1), "entries_sha256"),
    ("header not compact",
     frame(_GOOD_LINES).replace(b'"version":1,', b'"version": 1,', 1), "canonical"),
    ("entry not compact", frame([_PRETTY_LINE, _GOOD_LINES[1]]), "canonical"),
    ("CRLF line ends", frame([line + b"\r" for line in _GOOD_LINES]), "canonical"),
    ("duplicate key",
     frame([_GOOD_LINES[0].replace(b'{"identity":"agent:a",', b'{"identity":"agent:a","identity":"agent:a",', 1),
            _GOOD_LINES[1]]),
     "canonical"),
    ("invalid utf-8", frame([_GOOD_LINES[0], _GOOD_LINES[1].replace(b"agent:b", b"agent:\xff", 1)]), "UTF-8"),
    ("blank line", frame([_GOOD_LINES[0], b"", _GOOD_LINES[1]]), "JSON"),
    ("entry not an object", frame([b"[]", _GOOD_LINES[1]]), "not an object"),
    ("unsorted", _mutate_entries(lambda entries: entries.reverse()), "order"),
    ("repeated identity", _mutate_entries(lambda entries: entries.append(dict(entries[1]))), "order"),
    ("unknown state", _mutate_entries(_entry(1, "state", "warming")), "state"),
    ("uninitialized extra key", _mutate_entries(_entry(1, "record", None)), "keys"),
    ("ready without document", _mutate_entries(_drop(0, "document")), "keys"),
    ("record names another identity", _mutate_entries(_record("identity", "agent:z")), "record"),
    ("shared session",
     _mutate_entries(lambda entries: entries.__setitem__(1, ready_entry("agent:b", SESSION_ALPHA, b"doc-b"))),
     "session"),
    ("uppercase session id", _mutate_entries(_record("session_id", SESSION_ALPHA.upper())), "session_id"),
    ("negative generation", _mutate_entries(_record("generation", -1)), "generation"),
    ("boolean generation", _mutate_entries(_record("generation", True)), "generation"),
    ("u64 overflow", _mutate_entries(_record("checkpoint_version", 2**64)), "checkpoint_version"),
    ("identity with whitespace", _mutate_entries(_entry(1, "identity", "agent:b c")), "identity"),
    ("identity with slash", _mutate_entries(_entry(1, "identity", "agent:b/c")), "identity"),
    ("document sha mismatch", _mutate_entries(_document("sha256", "0" * 64)), "document"),
    ("document length mismatch", _mutate_entries(_document("length", 4)), "document"),
    ("non-canonical base64", _mutate_entries(_document("base64", "ZG9jLWF=")), "base64"),
    ("base64 alphabet", _mutate_entries(_document("base64", "ZG9j*WE=")), "base64"),
]


class ReaderTests(TempDirTestCase):
    def read(self, data: bytes) -> ce.ContinuityExport:
        target = self.tmp / "export.jsonl"
        target.write_bytes(data)
        return ce.read_continuity_export(target)

    def refused_file(self, data: bytes) -> str:
        with self.assertRaises(ce.ContinuityExportFileError) as caught:
            self.read(data)
        self.assertEqual(caught.exception.path, str(self.tmp / "export.jsonl"))
        return caught.exception.reason

    def test_refuses_anything_but_an_intact_v1_file(self):
        for name, data, needle in READER_REFUSALS:
            with self.subTest(name=name):
                self.assertIn(needle, self.refused_file(data))

    def test_accepts_the_baseline_entries(self):
        export = self.read(frame(_GOOD_LINES))
        self.assertEqual([entry.identity for entry in export.entries], ["agent:a", "agent:b"])
        self.assertEqual(document_of(export.entries[0]), described(b"doc-a"))
        self.assertEqual(export.entries_sha256, hashlib.sha256(_GOOD_ENTRIES).hexdigest())

    def test_accepts_every_unsigned_64_bit_integer(self):
        export = self.read(_mutate_entries(_record("generation", 2**64 - 1)))
        self.assertEqual(export.entries[0].record.generation, 2**64 - 1)

    def test_a_damaged_file_reports_its_digest_before_its_content(self):
        # A changed line fails its own checks too, but the header no longer
        # describes the file, and that is the fault to report.
        damaged = frame(_GOOD_LINES).replace(_GOOD_LINES[0], _PRETTY_LINE.encode("utf-8"), 1)
        reason = self.refused_file(damaged)
        self.assertIn("entries_length", reason)
        self.assertNotIn("canonical", reason)
        same_length = frame(_GOOD_LINES).replace(b'"state":"ready"', b'"state":"reaDy"', 1)
        reason = self.refused_file(same_length)
        self.assertIn("entries_sha256", reason)
        self.assertNotIn("state", reason)

    def test_refuses_an_entry_line_longer_than_the_limit(self):
        longest = max(len(line) for line in _GOOD_LINES)
        with mock.patch.object(ce, "MAX_ENTRY_LINE_BYTES", longest):
            self.read(frame(_GOOD_LINES))
        with mock.patch.object(ce, "MAX_ENTRY_LINE_BYTES", longest - 1):
            reason = self.refused_file(frame(_GOOD_LINES))
        self.assertIn(f"entry 0 is longer than {longest - 1} bytes", reason)

    def test_the_limit_is_on_each_line_not_the_file(self):
        lines = [compact(ready_entry(f"agent:{index}", f"019f0000-0000-7000-8000-{index:012d}", b"x" * 600))
                 for index in range(8)]
        limit = max(len(line) for line in lines)
        data = frame(lines)
        self.assertGreater(len(data), 4 * limit)
        with mock.patch.object(ce, "MAX_ENTRY_LINE_BYTES", limit):
            export = self.read(data)
        self.assertEqual(len(export.entries), 8)

    def test_refuses_a_line_nested_too_deeply_to_check(self):
        deep = b"[" * 200_000 + b"]" * 200_000
        self.assertIn("entry 0 is nested too deeply to check", self.refused_file(frame([deep])))

    def test_refuses_a_header_line_longer_than_its_limit(self):
        self.assertIn("header line", self.refused_file(frame(_GOOD_LINES, writer="w" * 70_000)))


# ---------------------------------------------------------------------------
# Golden files shared with the Node script and the migration verb
# ---------------------------------------------------------------------------


class GoldenTests(TempDirTestCase):
    async def test_the_file_written_by_this_script_is_byte_stable(self):
        target = self.tmp / "export.jsonl"
        export = await ce.export_continuity(golden_store(), GOLDEN_IDENTITIES, target)
        golden = FIXTURES / "written_by_python.jsonl"
        if os.environ.get(WRITE_FIXTURE_ENV) == "1":
            golden.parent.mkdir(parents=True, exist_ok=True)
            golden.write_bytes(target.read_bytes())
        self.assertEqual(target.read_bytes(), golden.read_bytes())
        self.assert_golden_entries(export)

    def test_reads_the_file_written_by_the_node_script(self):
        export = ce.read_continuity_export(FIXTURES / "written_by_node.jsonl")
        self.assertEqual(export.writer, "node")
        self.assert_golden_entries(export)

    def test_both_writers_write_the_same_entries_for_the_same_store(self):
        python_header, python_entries = split_file((FIXTURES / "written_by_python.jsonl").read_bytes())
        node_header, node_entries = split_file((FIXTURES / "written_by_node.jsonl").read_bytes())
        self.assertEqual(python_entries, node_entries)
        self.assertEqual({**python_header, "writer": "node"}, node_header)


# ---------------------------------------------------------------------------
# A ready record without a snapshot: refused, or written fresh on request
# ---------------------------------------------------------------------------

MISSING_RECORD = ce.ContinuityExportRecord(
    "agent:no-snapshot", "rt:agent:no-snapshot:1", SESSION_SMILE, 1, 0
)


def missing_snapshot_store() -> Store093:
    """What a 0.9.3 host built with an SDK left: records, and snapshots only where saved."""
    return Store093(
        records={
            "agent:ok": Record093("agent:ok", "rt-ok", SESSION_ALPHA, 0, 1),
            "agent:no-snapshot": Record093(*(getattr(MISSING_RECORD, field) for field in (
                "identity", "agent_runtime_id", "session_id", "generation", "checkpoint_version"))),
        },
        snapshots={SESSION_ALPHA: b"ok"},
    )


class MissingSnapshotTests(TempDirTestCase):
    async def test_refuses_by_default_naming_the_record_and_the_flag(self):
        for policy in ({}, {"missing_snapshot": "refuse"}):
            with self.subTest(policy=policy):
                target = self.tmp / "export.jsonl"
                with self.assertRaises(ce.ContinuityExportRefusedError) as caught:
                    await ce.export_continuity(
                        missing_snapshot_store(), ["agent:ok", "agent:no-snapshot"], target, **policy
                    )
                self.assertFalse(target.exists())
                self.assertTrue(no_partial_files(self.tmp))
                [row] = caught.exception.refusals
                self.assertEqual(
                    (row.row_class, row.identity, row.session_id),
                    (ce.RUNTIME_MISSING, "agent:no-snapshot", SESSION_SMILE),
                )
                self.assertEqual(
                    row.detail,
                    f"the provider holds no snapshot for session {SESSION_SMILE} (0.9.3 record: "
                    "agent_runtime_id rt:agent:no-snapshot:1, generation 1, checkpoint_version 0)",
                )
                self.assertIn("--missing-snapshot fresh", row.action)

    async def test_fresh_writes_the_identity_uninitialized_and_returns_its_record(self):
        store = missing_snapshot_store()
        target = self.tmp / "export.jsonl"
        export = await ce.export_continuity(
            store, ["agent:ok", "agent:no-snapshot"], target, missing_snapshot="fresh"
        )
        self.assertEqual(
            [(entry.identity, entry.state) for entry in export.entries],
            [("agent:no-snapshot", "uninitialized"), ("agent:ok", "ready")],
        )
        self.assertEqual(export.missing_snapshots, (MISSING_RECORD,))
        self.assertEqual(entry_lines(target.read_bytes())[0],
                         b'{"identity":"agent:no-snapshot","state":"uninitialized"}')
        # The file cannot say why an identity is uninitialized; reading it back
        # gives the same export without the list.
        self.assertEqual(ce.read_continuity_export(target), replace(export, missing_snapshots=()))
        # The abandoned session is never checked against the session index.
        self.assertNotIn(("resolve_record_by_session", SESSION_SMILE), store.calls)

    async def test_fresh_writes_the_golden_file_for_a_record_without_a_snapshot(self):
        # The golden store lists agent:beta without a record; a record with no
        # snapshot must give the same bytes.
        store = golden_store()
        store.records["agent:beta"] = Record093("agent:beta", "rt-beta", SESSION_OTHER, 2, 0)
        target = self.tmp / "export.jsonl"
        export = await ce.export_continuity(store, GOLDEN_IDENTITIES, target, missing_snapshot="fresh")
        self.assertEqual(target.read_bytes(), (FIXTURES / "written_by_python.jsonl").read_bytes())
        self.assertEqual(
            export.missing_snapshots,
            (ce.ContinuityExportRecord("agent:beta", "rt-beta", SESSION_OTHER, 2, 0),),
        )
        self.assert_golden_entries(export)

    async def test_fresh_leaves_every_other_refusal_in_place(self):
        store = missing_snapshot_store()
        store.broken["agent:broken"] = Failure093("agent:broken", "snapshot_corrupted", None, "bad checksum")
        target = self.tmp / "export.jsonl"
        with self.assertRaises(ce.ContinuityExportRefusedError) as caught:
            await ce.export_continuity(
                store, ["agent:ok", "agent:no-snapshot", "agent:broken"], target, missing_snapshot="fresh"
            )
        self.assertEqual(
            [(row.identity, row.row_class) for row in caught.exception.refusals],
            [("agent:broken", ce.BROKEN)],
        )
        self.assertFalse(target.exists())
        self.assertTrue(no_partial_files(self.tmp))

    async def test_an_unknown_policy_is_refused_before_the_provider_is_called(self):
        class Untouchable:
            async def resolve_many(self, identities):
                raise AssertionError("the provider must not be called")

            async def load_session_snapshot(self, session_id):
                raise AssertionError("the provider must not be called")

        for policy in ("skip", "Fresh", "", None):
            with self.subTest(policy=policy):
                target = self.tmp / "export.jsonl"
                with self.assertRaises(ValueError):
                    await ce.export_continuity(Untouchable(), ["agent:a"], target, missing_snapshot=policy)
                self.assertFalse(target.exists())


# ---------------------------------------------------------------------------
# Command line
# ---------------------------------------------------------------------------

PROVIDER_MODULE = textwrap.dedent(
    '''
    import base64


    class _Record:
        def __init__(self, identity, session_id):
            self.identity = identity
            self.session_id = session_id

        def to_dict(self):
            return {"identity": self.identity, "agent_runtime_id": "rt-" + self.identity,
                    "session_id": self.session_id, "generation": 0, "checkpoint_version": 1}


    class Store:
        def __init__(self, broken=False, failing=False):
            self.broken = broken
            self.failing = failing

        async def resolve_many(self, identities):
            if self.failing:
                raise OSError("store unreachable")
            answer = {}
            for identity in identities:
                if identity == "agent:a" and self.broken:
                    answer[identity] = {"state": "broken",
                                        "failure": {"identity": identity, "kind": "store_unavailable",
                                                    "record": None, "detail": "offline"}}
                elif identity == "agent:a":
                    answer[identity] = {"state": "ready",
                                        "record": _Record(identity, "%s").to_dict()}
                else:
                    answer[identity] = {"state": "uninitialized"}
            return answer

        async def load_session_snapshot(self, session_id):
            return {"data": base64.b64encode(b"document").decode()}


    async def make_store():
        return Store()


    def failing_factory():
        raise RuntimeError("database connection refused")


    class _UnreadableSnapshot:
        @property
        def data(self):
            raise RuntimeError("snapshot storage went away")


    class UnreadableSnapshotStore(Store):
        async def load_session_snapshot(self, session_id):
            return _UnreadableSnapshot()


    unreadable_snapshot_store = UnreadableSnapshotStore()


    class RacingStore(Store):
        """Another writer creates the output while the export reads."""

        async def load_session_snapshot(self, session_id):
            with open("out.jsonl", "x", encoding="utf-8") as other:
                other.write("keep me")
            return await super().load_session_snapshot(session_id)


    racing_store = RacingStore()


    broken_store = Store(broken=True)
    failing_store = Store(failing=True)


    class NoSnapshotStore:
        """What a 0.9.3 host built with an SDK left: a record, and no snapshot."""

        async def resolve_many(self, identities):
            answer = {}
            for identity in identities:
                if identity == "agent:a":
                    answer[identity] = {"state": "ready", "record": {
                        "identity": identity, "agent_runtime_id": "rt:agent:a:1",
                        "session_id": "%s", "generation": 1, "checkpoint_version": 0}}
                else:
                    answer[identity] = {"state": "uninitialized"}
            return answer

        async def load_session_snapshot(self, session_id):
            return None


    no_snapshot_store = NoSnapshotStore()
    '''
    % (SESSION_ALPHA, SESSION_ALPHA)
)

# What both scripts print for NoSnapshotStore (the Node suite pins the same
# text): the refusal, and the summary of an export with --missing-snapshot fresh.
NO_SNAPSHOT_REFUSAL = (
    "continuity export refused; nothing was written:\n"
    f"  class=RuntimeMissing identity=agent:a session={SESSION_ALPHA}\n"
    f"    detail: the provider holds no snapshot for session {SESSION_ALPHA} "
    "(0.9.3 record: agent_runtime_id rt:agent:a:1, generation 1, checkpoint_version 0)\n"
    "    action: if a 0.9.3 host built with the Python or TypeScript SDK wrote the store, it never "
    "saved snapshots: rerun the export with --missing-snapshot fresh, which writes the identity as "
    "uninitialized, so it starts fresh with an empty transcript, as at every 0.9.3 restart; "
    "otherwise restore the session's snapshot, or delete the identity with the 0.9.3 binary, "
    "then rerun the export\n"
)
NO_SNAPSHOT_FRESH_SUMMARY = (
    "out.jsonl: meerkat-meta.continuity-export version 1, writer {writer}, entries sha256 "
    "ec47968e29095ea3744ceb047d4eba4adae99445d7d7182f615e54223ffe2d52\n"
    "  0 ready, 2 uninitialized\n"
    "  uninitialized  agent:a\n"
    "  uninitialized  agent:b\n"
    "  1 written fresh by --missing-snapshot fresh (uninitialized, with an empty transcript):\n"
    f"  fresh          agent:a  session {SESSION_ALPHA}  "
    "(0.9.3 record: agent_runtime_id rt:agent:a:1, generation 1, checkpoint_version 0): "
    "transcript not carried, because 0.9.3 never saved a snapshot of this session\n"
)

IDENTITIES_FIXTURE = FIXTURES / "identities_bom_crlf.txt"
IDENTITIES_FIXTURE_LIST = ["agent:alpha", "agent:beta", "agent:gamma", "agent:" + chr(0x1F600), "agent:delta"]
BOM = chr(0xFEFF)


class IdentitiesFileTests(unittest.TestCase):
    """Both scripts read an identities file by one rule; the Node suite pins the same cases."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def write(self, data: bytes) -> str:
        path = self.tmp / "identities.txt"
        path.write_bytes(data)
        return str(path)

    def test_reads_the_shared_fixture(self):
        self.assertEqual(ce.read_identities_file(str(IDENTITIES_FIXTURE)), IDENTITIES_FIXTURE_LIST)

    def test_trims_exactly_unicode_white_space(self):
        # NEL is White_Space; the information separators are not, though
        # Python's str.strip() and JavaScript's trim() disagree on both.
        path = self.write((chr(0x85) + "agent:a" + chr(0x85) + "\n" + "agent:b" + chr(0x1C) + "\n").encode())
        self.assertEqual(ce.read_identities_file(path), ["agent:a", "agent:b" + chr(0x1C)])

    def test_refuses_a_byte_order_mark_after_the_start(self):
        path = self.write(b"agent:a\n\xef\xbb\xbfagent:b\n")
        with self.assertRaisesRegex(ValueError, r"line 2 .*U\+FEFF"):
            ce.read_identities_file(path)

    def test_refuses_a_file_that_is_not_utf8(self):
        path = self.write(b"agent:\xff\n")
        with self.assertRaisesRegex(ValueError, "not valid UTF-8"):
            ce.read_identities_file(path)


class CommandLineTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        (self.tmp / "host_provider.py").write_text(PROVIDER_MODULE, encoding="utf-8")

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def run_script(self, *args: str) -> subprocess.CompletedProcess:
        env = {key: value for key, value in os.environ.items() if key != "PYTHONPATH"}
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            cwd=self.tmp, env=env, capture_output=True, text=True, timeout=60,
        )

    def test_exports_through_a_provider_factory_and_verifies_the_file(self):
        (self.tmp / "ids.txt").write_text("agent:a\n\nagent:b\n", encoding="utf-8")
        result = self.run_script("--provider", "host_provider:make_store",
                                 "--identities-file", "ids.txt", "--identity", "agent:c",
                                 "--output", "out.jsonl")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("1 ready, 2 uninitialized", result.stdout)
        export = ce.read_continuity_export(self.tmp / "out.jsonl")
        self.assertEqual([entry.state for entry in export.entries], ["ready", "uninitialized", "uninitialized"])
        verified = self.run_script("--verify", "out.jsonl", str(FIXTURES / "written_by_node.jsonl"))
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertIn("writer node", verified.stdout)

    def test_a_refusal_lists_each_row_with_its_class_and_action(self):
        result = self.run_script("--provider", "host_provider:broken_store",
                                 "--identity", "agent:a", "--output", "out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_REFUSED)
        self.assertIn("class=Broken identity=agent:a", result.stderr)
        self.assertIn(ce.ACTIONS[ce.BROKEN], result.stderr)
        self.assertFalse((self.tmp / "out.jsonl").exists())

    def test_a_record_without_a_snapshot_is_refused_naming_the_flag(self):
        for extra in ([], ["--missing-snapshot", "refuse"]):
            with self.subTest(extra=extra):
                result = self.run_script("--provider", "host_provider:no_snapshot_store",
                                         "--identity", "agent:a", "--identity", "agent:b",
                                         "--output", "out.jsonl", *extra)
                self.assertEqual(result.returncode, ce.EXIT_REFUSED)
                self.assertEqual(result.stdout, "")
                self.assertEqual(result.stderr, NO_SNAPSHOT_REFUSAL)
                self.assertFalse((self.tmp / "out.jsonl").exists())

    def test_missing_snapshot_fresh_writes_it_uninitialized_and_lists_its_record(self):
        result = self.run_script("--provider", "host_provider:no_snapshot_store",
                                 "--identity", "agent:a", "--identity", "agent:b",
                                 "--output", "out.jsonl", "--missing-snapshot", "fresh")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, NO_SNAPSHOT_FRESH_SUMMARY.format(writer="python"))
        self.assertEqual(result.stderr, "")
        export = ce.read_continuity_export(self.tmp / "out.jsonl")
        self.assertEqual([(entry.identity, entry.state) for entry in export.entries],
                         [("agent:a", "uninitialized"), ("agent:b", "uninitialized")])

    def test_missing_snapshot_takes_refuse_or_fresh_and_only_with_an_export(self):
        unknown = self.run_script("--provider", "host_provider:no_snapshot_store",
                                  "--identity", "agent:a", "--output", "out.jsonl",
                                  "--missing-snapshot", "skip")
        self.assertEqual(unknown.returncode, ce.EXIT_USAGE)
        self.assertIn("--missing-snapshot", unknown.stderr)
        with_verify = self.run_script("--verify", str(FIXTURES / "written_by_python.jsonl"),
                                      "--missing-snapshot", "fresh")
        self.assertEqual(with_verify.returncode, ce.EXIT_USAGE)
        self.assertIn("--verify takes only files", with_verify.stderr)
        self.assertFalse((self.tmp / "out.jsonl").exists())

    def test_a_provider_fault_exits_retryable(self):
        result = self.run_script("--provider", "host_provider:failing_store",
                                 "--identity", "agent:a", "--output", "out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_PROVIDER)
        self.assertIn("run the export again", result.stderr)
        self.assertFalse((self.tmp / "out.jsonl").exists())

    def test_an_existing_output_is_a_usage_error(self):
        (self.tmp / "out.jsonl").write_text("keep", encoding="utf-8")
        result = self.run_script("--provider", "host_provider:make_store",
                                 "--identity", "agent:a", "--output", "out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_USAGE)
        self.assertIn("continuity export not started:", result.stderr)
        self.assertIn("already exists", result.stderr)
        self.assertEqual((self.tmp / "out.jsonl").read_text(encoding="utf-8"), "keep")

    def test_a_byte_order_mark_does_not_hide_the_first_identity(self):
        (self.tmp / "ids.txt").write_bytes(b"\xef\xbb\xbfagent:a\r\nagent:b\r\n")
        result = self.run_script("--provider", "host_provider:make_store",
                                 "--identities-file", "ids.txt", "--output", "out.jsonl")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("1 ready, 1 uninitialized", result.stdout)

    def test_a_byte_order_mark_in_an_identity_argument_is_a_usage_error(self):
        result = self.run_script("--provider", "host_provider:make_store",
                                 "--identity", BOM + "agent:a", "--output", "out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_USAGE, result.stderr)
        self.assertIn("U+FEFF", result.stderr)
        self.assertFalse((self.tmp / "out.jsonl").exists())

    def test_a_provider_factory_that_raises_exits_retryable(self):
        result = self.run_script("--provider", "host_provider:failing_factory",
                                 "--identity", "agent:a", "--output", "out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_PROVIDER, result.stderr)
        self.assertIn("database connection refused", result.stderr)
        self.assertIn("run the export again", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        self.assertFalse((self.tmp / "out.jsonl").exists())

    def test_a_provider_module_that_raises_on_import_exits_retryable(self):
        (self.tmp / "exploding_provider.py").write_text(
            'raise RuntimeError("database connection refused")\n', encoding="utf-8"
        )
        result = self.run_script("--provider", "exploding_provider:store",
                                 "--identity", "agent:a", "--output", "out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_PROVIDER, result.stderr)
        self.assertIn("database connection refused", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_a_missing_provider_module_is_a_usage_error(self):
        result = self.run_script("--provider", "no_such_provider:store",
                                 "--identity", "agent:a", "--output", "out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_USAGE, result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_a_missing_output_directory_is_a_usage_error(self):
        result = self.run_script("--provider", "host_provider:make_store",
                                 "--identity", "agent:a", "--output", "missing/out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_USAGE, result.stderr)
        self.assertIn("missing", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        self.assertTrue(no_partial_files(self.tmp))

    def test_verify_refuses_a_file_nested_too_deeply_without_a_traceback(self):
        (self.tmp / "deep.jsonl").write_bytes(frame([b"[" * 200_000 + b"]" * 200_000]))
        result = self.run_script("--verify", "deep.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_INVALID_FILE, result.stderr)
        self.assertIn("nested too deeply", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_a_missing_identities_file_is_a_usage_error(self):
        result = self.run_script("--provider", "host_provider:make_store",
                                 "--identities-file", "missing.txt", "--output", "out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_USAGE, result.stderr)
        self.assertRegex(result.stderr, r"cannot read --identities-file: .*missing\.txt")
        self.assertNotIn("Traceback", result.stderr)
        self.assertFalse((self.tmp / "out.jsonl").exists())

    def test_a_snapshot_that_cannot_be_read_exits_retryable(self):
        result = self.run_script("--provider", "host_provider:unreadable_snapshot_store",
                                 "--identity", "agent:a", "--output", "out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_PROVIDER, result.stderr)
        self.assertIn(
            f"continuity provider load_session_snapshot for session {SESSION_ALPHA} failed, "
            "nothing written; run the export again",
            result.stderr,
        )
        self.assertIn("snapshot storage went away", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        self.assertFalse((self.tmp / "out.jsonl").exists())
        self.assertTrue(no_partial_files(self.tmp))

    def test_an_output_that_appears_during_the_export_is_a_failure_after_the_start(self):
        result = self.run_script("--provider", "host_provider:racing_store",
                                 "--identity", "agent:a", "--output", "out.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_USAGE, result.stderr)
        self.assertIn("continuity export failed, nothing written:", result.stderr)
        self.assertIn("appeared while the export ran", result.stderr)
        self.assertNotIn("not started", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        self.assertEqual((self.tmp / "out.jsonl").read_text(encoding="utf-8"), "keep me")
        self.assertTrue(no_partial_files(self.tmp))

    def test_verify_accepts_a_file_with_an_8_mib_document(self):
        entry = ready_entry("agent:a", SESSION_ALPHA, bytes([7]) * (8 << 20))
        (self.tmp / "large.jsonl").write_bytes(frame([compact(entry)]))
        result = self.run_script("--verify", "large.jsonl")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("1 ready, 0 uninitialized", result.stdout)

    def test_verify_refuses_a_damaged_file(self):
        data = bytearray((FIXTURES / "written_by_python.jsonl").read_bytes())
        data[-10] ^= 0x01
        (self.tmp / "damaged.jsonl").write_bytes(bytes(data))
        result = self.run_script("--verify", "damaged.jsonl")
        self.assertEqual(result.returncode, ce.EXIT_INVALID_FILE)
        self.assertIn("refused", result.stderr)


# ---------------------------------------------------------------------------
# Published pins
# ---------------------------------------------------------------------------
#
# The scripts ship in the release tag, not as release assets. CHANGELOG.md and
# the export guide under docs/ pin each one in the forms an operator uses: its
# URL at that tag, and the line `sha256sum --check` reads.
#
# The newest pinned CHANGELOG section ([Unreleased] included) pins the scripts
# as they are, so a change to either script adds pins under the tag of the
# release that will ship it. Older sections keep the pins of their own
# release. The docs are published from main as soon as they change, so they
# follow the newest RELEASED pins, whose tag exists; before the first release
# that ships the scripts they follow [Unreleased]. The release commit that
# turns [Unreleased] into a version moves the docs pins with it.

# The export scripts and the JSON Lines providers that run beside them.
PINNED_SCRIPTS = ("continuity_export.py", "continuity-export.mjs", "jsonl_provider.py", "jsonl-provider.mjs")
_PINNED_SCRIPT = "|".join(re.escape(script) for script in PINNED_SCRIPTS)
PIN_URL_FORM = "https://raw.githubusercontent.com/lukacf/meerkat-meta/<tag>/tools/continuity-export/<script>"
PIN_URL = re.compile(
    r"https://raw\.githubusercontent\.com/lukacf/meerkat-meta/"
    r"(?P<tag>v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?)"
    rf"/tools/continuity-export/(?P<script>{_PINNED_SCRIPT})"
)
PIN_LINE = re.compile(rf"[ \t]*(?P<digest>[0-9a-f]{{64}})  (?P<script>{_PINNED_SCRIPT})[ \t]*")
URL_TOKEN = re.compile(r"[a-z]+://[^\s`'\"<>()\[\]]+")
# A run of exactly 64 hex digits (a 40-digit commit id is not a digest).
HEX_RUN = re.compile(r"(?<![0-9A-Fa-f])[0-9A-Fa-f]{64}(?![0-9A-Fa-f])")


@dataclass
class Pins:
    """One document's pins: the tags its URLs name, the scripts it has a URL
    for, and the digests it pins per script."""

    tags: set[str]
    urls: set[str]
    digests: dict[str, set[str]]

    def __bool__(self) -> bool:
        return bool(self.tags or self.urls or self.digests)


def read_pins(label: str, text: str, errors: list[str], first_line: int = 1) -> Pins:
    """Collect one document's pins. A URL that names a script must be a pin
    URL, and a digest on a line that names a script must be a checksum line;
    anything else is reported in `errors`."""
    pins = Pins(set(), set(), {})
    for number, line in enumerate(text.splitlines(), start=first_line):
        where = f"{label}:{number}"
        for token in URL_TOKEN.findall(line):
            if not any(script in token for script in PINNED_SCRIPTS):
                continue
            match = PIN_URL.fullmatch(token.rstrip(".,;:"))
            if match is None:
                errors.append(f"{where}: {token} is not a pin URL ({PIN_URL_FORM})")
            else:
                pins.tags.add(match["tag"])
                pins.urls.add(match["script"])
        if HEX_RUN.search(line) and any(script in line for script in PINNED_SCRIPTS):
            match = PIN_LINE.fullmatch(line)
            if match is None:
                errors.append(f"{where}: a digest beside a script name must be the line "
                              "sha256sum prints, '<sha256>  <script>'")
            else:
                pins.digests.setdefault(match["script"], set()).add(match["digest"])
    return pins


def check_document(label: str, pins: Pins, errors: list[str]) -> None:
    """A document that pins any script pins them all, at one tag, with one
    digest each."""
    if not pins:
        return
    if len(pins.tags) != 1:
        errors.append(f"{label}: the pin URLs name {len(pins.tags)} tags {sorted(pins.tags)}, not one")
    for script in PINNED_SCRIPTS:
        if script not in pins.urls:
            errors.append(f"{label}: no pin URL for {script}")
        found = pins.digests.get(script, set())
        if len(found) != 1:
            errors.append(f"{label}: {len(found)} SHA-256 pins for {script}, not one")


def changelog_sections(text: str) -> list[tuple[str, int, str]]:
    """Each `## [...]` section of a changelog, newest first, as (heading,
    first body line number, body)."""
    sections: list[tuple[str, int, list[str]]] = []
    for number, line in enumerate(text.splitlines(), start=1):
        if line.startswith("## ["):
            sections.append((line[3:].strip(), number + 1, []))
        elif sections:
            sections[-1][2].append(line)
    return [(heading, first, "\n".join(body)) for heading, first, body in sections]


def pin_errors(root: Path, digests: dict[str, str]) -> list[str]:
    """Every problem with the pins under `root`, given each script's SHA-256."""
    errors: list[str] = []
    changelog: list[tuple[str, Pins]] = []
    released: list[tuple[str, Pins]] = []
    text = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    for heading, first_line, body in changelog_sections(text):
        label = f"CHANGELOG.md {heading}"
        pins = read_pins("CHANGELOG.md", body, errors, first_line)
        check_document(label, pins, errors)
        if pins:
            changelog.append((label, pins))
            if not heading.startswith("[Unreleased]"):
                released.append((label, pins))
    docs: list[tuple[str, Pins]] = []
    for path in sorted((root / "docs").rglob("*")):
        if path.suffix not in (".md", ".mdx") or not path.is_file():
            continue
        label = path.relative_to(root).as_posix()
        pins = read_pins(label, path.read_text(encoding="utf-8"), errors)
        check_document(label, pins, errors)
        if pins:
            docs.append((label, pins))

    # A tag names one set of bytes wherever it is pinned.
    named: dict[tuple[str, str], set[str]] = {}
    for _, pins in changelog + docs:
        for tag in pins.tags:
            for script, found in pins.digests.items():
                named.setdefault((tag, script), set()).update(found)
    for (tag, script), found in sorted(named.items()):
        if len(found) > 1:
            errors.append(f"{tag} is pinned with {len(found)} SHA-256 digests for {script}")

    if not changelog:
        errors.append("CHANGELOG.md pins neither script")
    if not docs:
        errors.append("no page under docs/ pins the scripts")
    if not changelog:
        return errors
    newest_label, newest = changelog[0]
    for script in PINNED_SCRIPTS:
        stale = newest.digests.get(script, set()) - {digests[script]}
        if stale:
            errors.append(
                f"{newest_label}: pins {script} as {', '.join(sorted(stale))}, but its SHA-256 is "
                f"{digests[script]}; update the pins, and their tag when another release "
                "ships the new bytes"
            )
    followed_label, followed = (released or changelog)[0]
    for label, pins in docs:
        if pins.tags != followed.tags:
            errors.append(
                f"{label}: pins {', '.join(sorted(pins.tags))}, but the docs follow {followed_label} "
                f"({', '.join(sorted(followed.tags))})"
            )
        for script in PINNED_SCRIPTS:
            expected = followed.digests.get(script, set())
            unexpected = pins.digests.get(script, set()) - expected
            if unexpected:
                errors.append(
                    f"{label}: pins {script} as {', '.join(sorted(unexpected))}, but {followed_label} "
                    f"pins {', '.join(sorted(expected))}"
                )
    return errors


GUIDE = ROOT / "docs/guides/continuity-export.mdx"
GUIDE_CLASS_ROW = re.compile(r"^\| `(?P<row_class>\w+)` \| .* \| (?P<action>.*) \|$", re.MULTILINE)
GUIDE_EXIT_ROW = re.compile(r"^\| (?P<code>[0-9]+) \| .* \|$", re.MULTILINE)


def guide_text(value: str) -> str:
    """Prose compared case-insensitively, without a final period."""
    return " ".join(value.split()).rstrip(".").lower()


def pin_block(tag: str, py: str = "a" * 64, mjs: str = "b" * 64,
              provider_py: str = "e" * 64, provider_mjs: str = "f" * 64) -> str:
    base = f"https://raw.githubusercontent.com/lukacf/meerkat-meta/{tag}/tools/continuity-export"
    digests = (py, mjs, provider_py, provider_mjs)
    return ("".join(f"{base}/{script}\n" for script in PINNED_SCRIPTS)
            + "".join(f"{digest}  {script}\n" for digest, script in zip(digests, PINNED_SCRIPTS)))


class PublishedPinTests(unittest.TestCase):
    DIGESTS = {"continuity_export.py": "a" * 64, "continuity-export.mjs": "b" * 64,
               "jsonl_provider.py": "e" * 64, "jsonl-provider.mjs": "f" * 64}

    def test_the_changelog_and_the_docs_pin_the_scripts_as_they_are(self):
        digests = {script: hashlib.sha256((HERE / script).read_bytes()).hexdigest()
                   for script in PINNED_SCRIPTS}
        self.assertEqual(pin_errors(ROOT, digests), [])

    def errors_for(self, changelog_body: str, docs: dict[str, str]) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "CHANGELOG.md").write_text("# Changelog\n\n" + changelog_body, encoding="utf-8")
            for name, text in docs.items():
                path = root / "docs" / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text, encoding="utf-8")
            return pin_errors(root, self.DIGESTS)

    def assert_reported(self, errors: list[str], *fragments: str) -> None:
        for fragment in fragments:
            self.assertTrue(any(fragment in error for error in errors), f"{fragment!r} not in {errors}")

    def test_current_pins_pass_and_released_sections_keep_their_own_bytes(self):
        old = pin_block("v0.9.4", "c" * 64, "d" * 64)
        unreleased = "## [Unreleased]\n\n" + pin_block("v0.9.5")
        released = "## [0.9.4] - 2026-10-20\n\n" + old
        # Between releases the docs keep the released pins: v0.9.5 is not a tag yet.
        guide = {"guides/export.mdx": "Steps:\n\n```bash\n" + old + "```\n",
                 # A link to the directory is not a pin.
                 "guides/other.mdx": "See https://github.com/lukacf/meerkat-meta/tree/v0.9.5/tools/continuity-export.\n"}
        self.assertEqual(self.errors_for(unreleased + released, guide), [])
        self.assert_reported(
            self.errors_for(unreleased + released, {"guides/export.mdx": pin_block("v0.9.5")}),
            "docs/guides/export.mdx: pins v0.9.5, but the docs follow CHANGELOG.md [0.9.4] - 2026-10-20 (v0.9.4)",
        )
        # The release commit moves them with the section it releases.
        released_next = "## [0.9.5] - 2026-11-02\n\n" + pin_block("v0.9.5")
        self.assertEqual(self.errors_for(released_next + released, {"guides/export.mdx": pin_block("v0.9.5")}), [])
        self.assert_reported(
            self.errors_for(released_next + released, {"guides/export.mdx": old}),
            "docs/guides/export.mdx: pins v0.9.4, but the docs follow CHANGELOG.md [0.9.5] - 2026-11-02 (v0.9.5)",
        )
        # With nothing unreleased, the newest released section is current.
        self.assert_reported(self.errors_for(released, {"guides/export.mdx": old}),
                             "CHANGELOG.md [0.9.4] - 2026-10-20: pins continuity_export.py as " + "c" * 64)
        # A commit id beside a script name is not a digest.
        commit = "Fixed in 0123456789abcdef0123456789abcdef01234567 for continuity_export.py.\n"
        self.assertEqual(self.errors_for("## [Unreleased]\n\n" + pin_block("v0.9.4") + commit,
                                         {"guides/export.mdx": pin_block("v0.9.4") + commit}), [])

    def test_drifted_pins_are_reported(self):
        stale = "c" * 64
        errors = self.errors_for("## [Unreleased]\n\n" + pin_block("v0.9.4", py=stale),
                                 {"guides/export.mdx": pin_block("v0.9.4", py=stale)})
        self.assert_reported(errors, f"CHANGELOG.md [Unreleased]: pins continuity_export.py as {stale}",
                             "its SHA-256 is " + "a" * 64)
        # The docs pin exactly what the section they follow pins.
        self.assert_reported(
            self.errors_for("## [Unreleased]\n\n" + pin_block("v0.9.4"),
                            {"guides/export.mdx": pin_block("v0.9.4", py=stale)}),
            f"docs/guides/export.mdx: pins continuity_export.py as {stale}, "
            "but CHANGELOG.md [Unreleased] pins " + "a" * 64,
        )
        # A tag names one set of bytes: a new digest needs a new tag.
        reused = ("## [Unreleased]\n\n" + pin_block("v0.9.4") + "\n## [0.9.4] - 2026-10-20\n\n"
                  + pin_block("v0.9.4", py=stale))
        self.assert_reported(self.errors_for(reused, {"guides/export.mdx": pin_block("v0.9.4")}),
                             "v0.9.4 is pinned with 2 SHA-256 digests for continuity_export.py")
        # The docs name the tag of the section they follow.
        self.assert_reported(self.errors_for("## [Unreleased]\n\n" + pin_block("v0.9.4"),
                                             {"guides/export.mdx": pin_block("v0.9.5")}),
                             "docs/guides/export.mdx: pins v0.9.5, but the docs follow CHANGELOG.md [Unreleased] (v0.9.4)")

    def test_incomplete_or_malformed_pins_are_reported(self):
        changelog = "## [Unreleased]\n\n" + pin_block("v0.9.4")
        url = "https://raw.githubusercontent.com/lukacf/meerkat-meta/v0.9.4/tools/continuity-export/"
        cases = {
            "one script only": (f"{url}continuity_export.py\n{'a' * 64}  continuity_export.py\n",
                                ["no pin URL for continuity-export.mjs",
                                 "0 SHA-256 pins for continuity-export.mjs"]),
            "the scripts without the providers": (
                f"{url}continuity_export.py\n{url}continuity-export.mjs\n"
                f"{'a' * 64}  continuity_export.py\n{'b' * 64}  continuity-export.mjs\n",
                ["no pin URL for jsonl_provider.py", "0 SHA-256 pins for jsonl-provider.mjs"]),
            "two tags": (pin_block("v0.9.4").replace("v0.9.4/tools/continuity-export/continuity-export.mjs",
                                                     "v0.9.5/tools/continuity-export/continuity-export.mjs"),
                         ["the pin URLs name 2 tags"]),
            "two digests": (pin_block("v0.9.4") + f"{'c' * 64}  continuity_export.py\n",
                            ["2 SHA-256 pins for continuity_export.py"]),
            "a browser URL": (pin_block("v0.9.4") + "https://github.com/lukacf/meerkat-meta/blob/v0.9.4/"
                              "tools/continuity-export/continuity_export.py\n", ["is not a pin URL"]),
            "a branch URL": (pin_block("v0.9.4") + url.replace("v0.9.4", "main") + "continuity_export.py\n",
                             ["is not a pin URL"]),
            "a digest in prose": (pin_block("v0.9.4") + f"sha256 {'a' * 64} for `continuity_export.py`\n",
                                  ["must be the line sha256sum prints"]),
            "an uppercase digest": (pin_block("v0.9.4") + f"{'A' * 64}  continuity_export.py\n",
                                    ["must be the line sha256sum prints"]),
        }
        for case, (guide, fragments) in cases.items():
            with self.subTest(case=case):
                self.assert_reported(self.errors_for(changelog, {"guides/export.mdx": guide}), *fragments)
        self.assert_reported(self.errors_for("## [Unreleased]\n\nNothing pinned.\n", {}),
                             "CHANGELOG.md pins neither script", "no page under docs/ pins the scripts")

    def test_the_guide_documents_every_exit_code_and_refusal_action(self):
        # The export guide copies the scripts' exit codes and refusal
        # actions into its tables; the Node suite checks its own script.
        guide = GUIDE.read_text(encoding="utf-8")
        actions = {match["row_class"]: match["action"] for match in GUIDE_CLASS_ROW.finditer(guide)}
        self.assertEqual(sorted(actions), sorted(ce.ACTIONS))
        for row_class, action in ce.ACTIONS.items():
            with self.subTest(row_class=row_class):
                self.assertEqual(guide_text(actions[row_class]), guide_text(action))
        self.assertEqual(
            sorted(int(match["code"]) for match in GUIDE_EXIT_ROW.finditer(guide)),
            [0, ce.EXIT_REFUSED, ce.EXIT_USAGE, ce.EXIT_PROVIDER, ce.EXIT_INVALID_FILE],
        )


# ---------------------------------------------------------------------------
# The JSON Lines provider (jsonl_provider.py)
# ---------------------------------------------------------------------------

JSONL_RECORDS_HEADER = "identity, agent_runtime_id, session_id, generation, checkpoint_version"


def jsonl(rows: list[Any]) -> str:
    return "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows)


def record_row(identity: str, runtime: str, session_id: str, generation: Any, checkpoint: Any) -> dict[str, Any]:
    return {
        "identity": identity,
        "agent_runtime_id": runtime,
        "session_id": session_id,
        "generation": generation,
        "checkpoint_version": checkpoint,
    }


def session_row(session_id: str, document: bytes) -> dict[str, str]:
    return {"session_id": session_id, "data": base64.b64encode(document).decode()}


def golden_record_rows() -> list[dict[str, Any]]:
    """golden_store()'s records as a host's tables give them: a session id
    spelled in upper case, and integers some SQL tools write as strings."""
    return [
        record_row("agent:alpha", "rt-alpha", SESSION_ALPHA.upper(), "2", 7),
        record_row(IDENTITY_SMILE, "rt-smile", SESSION_SMILE, 0, "1"),
    ]


def golden_session_rows() -> list[dict[str, str]]:
    """golden_store()'s snapshots, in two more spellings, and one session no
    record names."""
    return [
        session_row("{" + SESSION_ALPHA + "}", DOC_ALPHA),
        session_row(SESSION_SMILE.replace("-", ""), DOC_SMILE),
        session_row(SESSION_OTHER, b"not the current session of any identity"),
    ]


def capture_rows(capture: str) -> tuple[list[str], list[dict[str, Any]], list[dict[str, str]], bytes]:
    """A released 0.9.3 remote-continuity capture's export, taken apart into
    the identities, records and sessions a host's tables would give."""
    exported = gzip.decompress((H_REMOTE / f"{capture}-export.jsonl.gz").read_bytes())
    identities: list[str] = []
    records: list[dict[str, Any]] = []
    sessions: list[dict[str, str]] = []
    for line in entry_lines(exported):
        entry = json.loads(line)
        identities.append(entry["identity"])
        if entry["state"] == "ready":
            records.append(entry["record"])
            sessions.append({"session_id": entry["record"]["session_id"], "data": entry["document"]["base64"]})
    return identities, records, sessions, exported


# Input errors both providers report the same way: (records lines, sessions
# lines, the file the error names, the message after "<path> line ").
# continuity-export.test.mjs holds the same table.
RECORD_OK = '{"identity":"agent:a","agent_runtime_id":"rt","session_id":"' + SESSION_ALPHA + '","generation":0,"checkpoint_version":1}'
SESSION_OK = '{"session_id":"' + SESSION_ALPHA + '","data":"aGk="}'
JSONL_INPUT_ERRORS = [
    ("not JSON", ["{"], [SESSION_OK], "records", "1: the line is not JSON: "),
    ("not an object", ["[1]"], [SESSION_OK], "records", "1: the line is not a JSON object"),
    ("a non-JSON number", ['{"identity":"agent:a","agent_runtime_id":"rt","session_id":"' + SESSION_ALPHA
                           + '","generation":NaN,"checkpoint_version":1}'], [SESSION_OK], "records",
     "1: the line is not JSON: "),
    ("a repeated key", ['{"identity":"agent:a","identity":"agent:b","agent_runtime_id":"rt","session_id":"'
                        + SESSION_ALPHA + '","generation":0,"checkpoint_version":1}'], [SESSION_OK], "records",
     '1: key "identity" is repeated'),
    ("a missing key", ['{"identity":"agent:a","agent_runtime_id":"rt","session_id":"' + SESSION_ALPHA
                       + '","generation":0}'], [SESSION_OK], "records",
     f"1: the object must have exactly the keys {JSONL_RECORDS_HEADER} (missing checkpoint_version)"),
    ("unexpected keys", [RECORD_OK[:-1] + ',"zeta":1,"deleted":false}'], [SESSION_OK], "records",
     f'1: the object must have exactly the keys {JSONL_RECORDS_HEADER} (unexpected "deleted", "zeta")'),
    ("an identity that is not a string", [RECORD_OK.replace('"agent:a"', "5")], [SESSION_OK], "records",
     "1: identity is not a string"),
    ("a repeated identity", [RECORD_OK, "", RECORD_OK], [SESSION_OK], "records",
     '3: identity "agent:a" is repeated (first on line 1)'),
    ("a session id it cannot read", [RECORD_OK], ['{"session_id":"nope","data":"aGk="}'], "sessions",
     '1: session_id "nope" is not a session id'),
    ("a repeated session", [RECORD_OK], [SESSION_OK, SESSION_OK.replace(SESSION_ALPHA, SESSION_ALPHA.upper())],
     "sessions", f"2: session {SESSION_ALPHA} is repeated (first on line 1)"),
    ("base64 with a line break", [RECORD_OK], [SESSION_OK.replace("aGk=", "aGk=\\naGk=")], "sessions",
     "1: data is not standard base64 with padding and without line breaks"),
    ("base64 without padding", [RECORD_OK], [SESSION_OK.replace("aGk=", "aGk")], "sessions",
     "1: data is not standard base64 with padding and without line breaks"),
    ("the URL-safe alphabet", [RECORD_OK], [SESSION_OK.replace("aGk=", "-_8=")], "sessions",
     "1: data is not standard base64 with padding and without line breaks"),
    ("data that is not a string", [RECORD_OK], [SESSION_OK.replace('"aGk="', "7")], "sessions",
     "1: data is not standard base64 with padding and without line breaks"),
]


class JsonlProviderTests(TempDirTestCase):
    def write(self, name: str, text: str | bytes) -> Path:
        path = self.tmp / name
        path.write_bytes(text if isinstance(text, bytes) else text.encode("utf-8"))
        return path

    def provider(self, records: str | bytes, sessions: str | bytes) -> Any:
        return jp.JsonlContinuityProvider(self.write("records.jsonl", records), self.write("sessions.jsonl", sessions))

    async def test_writes_the_golden_file_from_a_hosts_rows(self):
        # CRLF line ends, a byte order mark, a blank line and a whitespace line.
        provider = self.provider(
            jsonl(golden_record_rows()).replace("\n", "\r\n"),
            "\ufeff" + jsonl(golden_session_rows()).replace("\n", "\r\n") + "\n \t\r\n",
        )
        target = self.tmp / "export.jsonl"
        export = await ce.export_continuity(provider, GOLDEN_IDENTITIES, target)
        self.assertEqual(target.read_bytes(), (FIXTURES / "written_by_python.jsonl").read_bytes())
        self.assert_golden_entries(export)

    async def test_rebuilds_the_released_remote_captures_byte_for_byte(self):
        # storage_migrate_native_census.rs imports these exact files (for
        # example a_remote_export_imports_into_a_new_directory), so a host's
        # rows exported through this provider are what the migration verb
        # admits.
        for capture in ("memory", "durable"):
            with self.subTest(capture=capture):
                identities, records, sessions, exported = capture_rows(capture)
                self.assertEqual(len(records), 4)
                provider = self.provider(jsonl(records), jsonl(sessions))
                target = self.tmp / f"{capture}.jsonl"
                await ce.export_continuity(provider, identities, target)
                self.assertEqual(target.read_bytes(), exported)

    def test_the_command_line_takes_the_provider_from_the_environment(self):
        records = self.write("records.jsonl", jsonl(golden_record_rows()))
        sessions = self.write("sessions.jsonl", jsonl(golden_session_rows()))
        target = self.tmp / "export.jsonl"
        identities = [arg for identity in GOLDEN_IDENTITIES for arg in ("--identity", identity)]
        env = {**os.environ, jp.RECORDS_ENV: str(records), jp.SESSIONS_ENV: str(sessions)}
        # The provider is importable from the script's directory, wherever the
        # export runs.
        run = subprocess.run(
            [sys.executable, str(SCRIPT), "--provider", "jsonl_provider:from_env", *identities,
             "--output", str(target)],
            cwd=self.tmp, env=env, capture_output=True, text=True, check=False,
        )
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(target.read_bytes(), (FIXTURES / "written_by_python.jsonl").read_bytes())

        del env[jp.SESSIONS_ENV]
        run = subprocess.run(
            [sys.executable, str(SCRIPT), "--provider", "jsonl_provider:from_env", *identities,
             "--output", str(self.tmp / "other.jsonl")],
            cwd=self.tmp, env=env, capture_output=True, text=True, check=False,
        )
        self.assertEqual(run.returncode, ce.EXIT_PROVIDER)
        self.assertIn(f"{jp.SESSIONS_ENV} is not set", run.stderr)
        self.assertFalse((self.tmp / "other.jsonl").exists())

    async def test_an_identity_the_list_misses_is_refused_as_unlisted(self):
        provider = self.provider(jsonl(golden_record_rows()), jsonl(golden_session_rows()))
        refused = await self.refused(provider, ["agent:alpha"])
        self.assertEqual([(row.row_class, row.identity) for row in refused.refusals],
                         [(ce.UNLISTED, IDENTITY_SMILE)])

    async def test_a_listed_identity_without_a_record_is_uninitialized(self):
        provider = self.provider(jsonl(golden_record_rows()), jsonl(golden_session_rows()))
        target = self.tmp / "export.jsonl"
        export = await ce.export_continuity(provider, [*GOLDEN_IDENTITIES, "agent:gamma"], target)
        self.assertEqual({entry.identity: entry.state for entry in export.entries}["agent:gamma"], "uninitialized")

    async def test_a_missing_snapshot_follows_the_exports_policy(self):
        records = jsonl([record_row("agent:a", "rt", SESSION_RENAMED, 0, 1)])
        provider = self.provider(records, jsonl(golden_session_rows()))
        refused = await self.refused(provider, ["agent:a"])
        self.assertEqual([(row.row_class, row.identity) for row in refused.refusals],
                         [(ce.RUNTIME_MISSING, "agent:a")])
        target = self.tmp / "fresh.jsonl"
        export = await ce.export_continuity(provider, ["agent:a"], target, missing_snapshot="fresh")
        self.assertEqual([(entry.identity, entry.state) for entry in export.entries], [("agent:a", "uninitialized")])

    async def test_other_values_reach_the_export_as_the_file_holds_them(self):
        # The export checks values by the 0.9.3 gateway's rules: a string that
        # is not plain decimal digits, a float and a negative number are
        # refused as Unreadable; the largest u64 is a valid record.
        for generation in ("007", 1.5, -1, "1e3", True):
            with self.subTest(generation=generation):
                provider = self.provider(jsonl([record_row("agent:a", "rt", SESSION_ALPHA, generation, 1)]),
                                         jsonl([session_row(SESSION_ALPHA, b"doc")]))
                refused = await self.refused(provider, ["agent:a"])
                self.assertEqual([row.row_class for row in refused.refusals], [ce.UNREADABLE])
        largest = 2**64 - 1
        provider = self.provider(jsonl([record_row("agent:a", "rt", SESSION_ALPHA, str(largest), largest)]),
                                 jsonl([session_row(SESSION_ALPHA, b"doc")]))
        export = await ce.export_continuity(provider, ["agent:a"], self.tmp / "export.jsonl")
        self.assertEqual((export.entries[0].record.generation, export.entries[0].record.checkpoint_version),
                         (largest, largest))

    def test_resolve_record_by_session_answers_from_the_records_file(self):
        provider = self.provider(
            jsonl([*golden_record_rows(), record_row("agent:b", "rt-b", SESSION_OTHER, 0, 0),
                   record_row("agent:c", "rt-c", SESSION_OTHER.upper(), 0, 0)]),
            jsonl(golden_session_rows()),
        )
        record, fencing_token, checkpoint_version = provider.resolve_record_by_session(SESSION_ALPHA)
        self.assertEqual(record, record_row("agent:alpha", "rt-alpha", SESSION_ALPHA.upper(), 2, 7))
        self.assertEqual((fencing_token, checkpoint_version), (None, 7))
        # A session no record names, or two records name, has no single record.
        self.assertIsNone(provider.resolve_record_by_session(SESSION_RENAMED))
        self.assertIsNone(provider.resolve_record_by_session(SESSION_OTHER))

    def test_reads_a_snapshot_only_when_the_export_asks(self):
        sessions = self.write("sessions.jsonl", jsonl(golden_session_rows()))
        provider = jp.JsonlContinuityProvider(self.write("records.jsonl", jsonl(golden_record_rows())), sessions)
        self.assertEqual(provider.load_session_snapshot(SESSION_SMILE),
                         {"data": base64.b64encode(DOC_SMILE).decode()})
        self.assertIsNone(provider.load_session_snapshot(SESSION_RENAMED))
        # The provider keeps where each line starts, so a file rewritten after
        # it opened is caught rather than read as another session.
        sessions.write_text(jsonl([session_row(SESSION_RENAMED, DOC_ALPHA), *golden_session_rows()]), encoding="utf-8")
        with self.assertRaises(jp.JsonlProviderInputError) as caught:
            provider.load_session_snapshot(SESSION_ALPHA)
        self.assertEqual(str(caught.exception), f"{sessions} line 1: the file changed after the provider opened it")

    def test_opening_holds_no_snapshot(self):
        # The provider keeps where each session's line starts, not the
        # snapshots: opening it over 32 MiB of documents stays far below that,
        # and a snapshot is read when the export asks for it.
        documents = [bytes([index]) * (1 << 20) for index in range(32)]
        sessions = [session_row(f"019f0000-0000-7000-8000-{index:012x}", document)
                    for index, document in enumerate(documents)]
        records_path = self.write("records.jsonl", jsonl(golden_record_rows()))
        sessions_path = self.write("sessions.jsonl", jsonl(sessions))
        tracemalloc.start()
        try:
            provider = jp.JsonlContinuityProvider(records_path, sessions_path)
            current, peak = tracemalloc.get_traced_memory()
        finally:
            tracemalloc.stop()
        self.assertLess(peak, 16 << 20)
        self.assertLess(current, 1 << 20)
        snapshot = provider.load_session_snapshot(sessions[3]["session_id"])
        self.assertEqual(base64.b64decode(snapshot["data"]), documents[3])

    def test_input_errors_name_the_file_and_the_line(self):
        for name, records, sessions, which, message in JSONL_INPUT_ERRORS:
            with self.subTest(name):
                records_path = self.write("records.jsonl", "".join(line + "\n" for line in records))
                sessions_path = self.write("sessions.jsonl", "".join(line + "\n" for line in sessions))
                with self.assertRaises(jp.JsonlProviderInputError) as caught:
                    jp.JsonlContinuityProvider(records_path, sessions_path)
                path = records_path if which == "records" else sessions_path
                self.assertTrue(str(caught.exception).startswith(f"{path} line {message}"), str(caught.exception))
        records_path = self.write("records.jsonl", b"\xff\n")
        with self.assertRaises(jp.JsonlProviderInputError) as caught:
            jp.JsonlContinuityProvider(records_path, self.write("sessions.jsonl", SESSION_OK + "\n"))
        self.assertEqual(str(caught.exception), f"{records_path} line 1: the line is not valid UTF-8")

    def test_from_env_names_a_missing_variable(self):
        for missing in (jp.RECORDS_ENV, jp.SESSIONS_ENV):
            with self.subTest(missing=missing):
                env = {jp.RECORDS_ENV: "records.jsonl", jp.SESSIONS_ENV: "sessions.jsonl"}
                del env[missing]
                with mock.patch.dict(os.environ, env, clear=True):
                    with self.assertRaises(jp.JsonlProviderInputError) as caught:
                        jp.from_env()
                self.assertTrue(str(caught.exception).startswith(f"{missing} is not set"))


if __name__ == "__main__":
    unittest.main()
