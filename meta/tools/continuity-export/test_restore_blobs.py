#!/usr/bin/env python3
"""Contracts for tools/continuity-export/restore_blobs.py.

Run: python3 tools/continuity-export/test_restore_blobs.py
"""
from __future__ import annotations

import base64
import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "restore_blobs.py"
# The continuity mirror's golden rows and the tree this script writes for
# them; the Rust suite reads that tree back through the gateway's blob store.
FIXTURE = HERE.parent.parent / "crates/meerkat-meta/tests/fixtures/continuity_mirror_blobs_v1"

_spec = importlib.util.spec_from_file_location("restore_blobs", SCRIPT)
assert _spec is not None and _spec.loader is not None
restore_blobs = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(restore_blobs)


def image_row(media_type: str, payload: bytes) -> dict[str, str]:
    """A row under Meerkat's image address: the hash covers the base64 text."""
    data = base64.b64encode(payload).decode("ascii")
    digest = hashlib.sha256(media_type.encode() + b"\x00" + data.encode()).hexdigest()
    return {"blob_id": f"sha256:{digest}", "media_type": media_type, "data": data}


def raw_row(media_type: str, payload: bytes) -> dict[str, str]:
    """A row under Meta's raw address: the hash covers the decoded bytes."""
    digest = hashlib.sha256(media_type.encode() + b"\x00" + payload).hexdigest()
    return {
        "blob_id": f"sha256:{digest}",
        "media_type": media_type,
        "data": base64.b64encode(payload).decode("ascii"),
    }


class RestoreBlobsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.state = self.root / "state"
        self.state.mkdir()
        self.blobs_file = self.root / "blobs.jsonl"

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write_rows(self, rows: list[dict[str, str]], extra: str = "") -> None:
        text = "".join(json.dumps(row) + "\n" for row in rows) + extra
        self.blobs_file.write_text(text, encoding="utf-8")

    def run_script(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--blobs",
                str(self.blobs_file),
                "--state-dir",
                str(self.state),
            ],
            capture_output=True,
            text=True,
            check=False,
        )

    def stored(self, row: dict[str, str]) -> tuple[bytes, dict[str, object]]:
        key = row["blob_id"].removeprefix("sha256:")
        blobs = self.state / "blobs"
        payload = (blobs / "objects" / f"{key}.bin").read_bytes()
        meta = json.loads((blobs / "meta" / f"{key}.json").read_text(encoding="utf-8"))
        return payload, meta

    def test_writes_each_blob_in_the_gateway_layout_and_reruns_as_a_no_op(self) -> None:
        png = image_row("image/png", b"\x89PNG\r\n\x1a\n camera snapshot")
        jpeg = raw_row("image/jpeg", b"\xff\xd8\xff legacy upload")
        self.write_rows([png, jpeg, png])
        first = self.run_script()
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(
            json.loads(first.stdout), {"present": 0, "rows": 2, "torn_tail": False, "written": 2}
        )
        for row in (png, jpeg):
            payload, meta = self.stored(row)
            self.assertEqual(payload, base64.b64decode(row["data"]))
            self.assertEqual(meta["media_type"], row["media_type"])
            self.assertEqual(meta["size"], len(payload))
        second = self.run_script()
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(
            json.loads(second.stdout), {"present": 2, "rows": 2, "torn_tail": False, "written": 0}
        )

    def test_refuses_an_id_that_does_not_match_its_bytes(self) -> None:
        row = image_row("image/png", b"one image")
        row["data"] = base64.b64encode(b"another image").decode("ascii")
        self.write_rows([row])
        result = self.run_script()
        self.assertEqual(result.returncode, 2)
        self.assertIn("blobs.jsonl:1", result.stderr)
        self.assertIn("does not match", result.stderr)
        self.assertFalse((self.state / "blobs" / "meta").exists())

    def test_refuses_a_malformed_line_naming_it(self) -> None:
        good = image_row("image/png", b"fine")
        self.write_rows([good], extra='{"blob_id": "sha256:00"}\n')
        result = self.run_script()
        self.assertEqual(result.returncode, 2)
        self.assertIn("blobs.jsonl:2", result.stderr)

    def test_skips_a_last_row_the_mirror_never_finished(self) -> None:
        whole = image_row("image/png", b"whole")
        torn = json.dumps(image_row("image/png", b"torn"))[:-9]
        self.write_rows([whole], extra=torn)
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout), {"present": 0, "rows": 1, "torn_tail": True, "written": 1}
        )
        self.assertEqual(self.stored(whole)[0], b"whole")

    def test_refuses_a_broken_line_that_ends_with_a_newline(self) -> None:
        torn = json.dumps(image_row("image/png", b"torn"))[:-9]
        self.write_rows([], extra=torn + "\n")
        result = self.run_script()
        self.assertEqual(result.returncode, 2)
        self.assertIn("blobs.jsonl:1", result.stderr)

    def test_refuses_a_stored_blob_with_different_bytes(self) -> None:
        row = image_row("image/png", b"the real bytes")
        self.write_rows([row])
        self.assertEqual(self.run_script().returncode, 0)
        key = row["blob_id"].removeprefix("sha256:")
        (self.state / "blobs" / "objects" / f"{key}.bin").write_bytes(b"tampered")
        result = self.run_script()
        self.assertEqual(result.returncode, 2)
        self.assertIn("different bytes", result.stderr)

    def test_refuses_invalid_base64(self) -> None:
        row = image_row("image/png", b"x")
        row["data"] = "not base64!"
        self.write_rows([row])
        result = self.run_script()
        self.assertEqual(result.returncode, 2)
        self.assertIn("base64", result.stderr)

    def test_the_golden_mirror_rows_restore_to_the_golden_tree(self) -> None:
        self.blobs_file = FIXTURE / "blobs.jsonl"
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        golden = FIXTURE / "restored" / "blobs"
        written = self.state / "blobs"

        def files(root: Path) -> list[str]:
            return sorted(path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file())

        self.assertEqual(files(written), files(golden))
        self.assertEqual(len(files(golden)), 4)
        for name in files(golden):
            if name.startswith("objects/"):
                self.assertEqual((written / name).read_bytes(), (golden / name).read_bytes(), name)
                continue
            ours = json.loads((written / name).read_text(encoding="utf-8"))
            kept = json.loads((golden / name).read_text(encoding="utf-8"))
            # The write time is the only field that differs between runs.
            self.assertIsInstance(ours.pop("created_at_ms"), int)
            kept.pop("created_at_ms")
            self.assertEqual(ours, kept, name)

    def test_a_missing_state_directory_is_refused(self) -> None:
        self.write_rows([image_row("image/png", b"x")])
        self.state.rmdir()
        result = self.run_script()
        self.assertEqual(result.returncode, 2)
        self.assertIn("does not exist", result.stderr)


if __name__ == "__main__":
    unittest.main()
