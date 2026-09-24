#!/usr/bin/env python3
"""Hold the pre-ledger corpus generator to the exclusions the repo enforces.

`crates/meerkat-runtime/tests/fixtures/v0_7_x_pre_ledger_realm/mint_pre_ledger_fixture.py`
walks the realm a published binary left and turns that walk into two things:
the bytes copied into the committed corpus, and the payload list bound in
`fixture-manifest.json`. A transient sequence lock is an artifact of the
capturing process, and `.gitignore` drops it from the corpus tree, so a walk
that emitted one would bind a path no clean checkout can have. That is exactly
how the corpus arrived unverifiable in 0.8.23, and it was repaired in the
manifest rather than in the generator, so the next `--capture` would have
reintroduced it.

These tests pin the generator side of that: a fresh capture walk emits no lock
paths, the copied tree and the manifest payload list agree, and the generator's
notion of "transient" gives the same answer as `.gitignore` for every path
shape the corpus actually contains.

The `.gitignore` comparison asks git rather than parsing the file, and passes
`--no-index` deliberately: without it git never reports a *tracked* path as
ignored, so the "this path is not ignored" half of the table would pass for
free for `.seq`/`.jsonl`/`checkpoint`, which are tracked.
"""

from __future__ import annotations

import importlib.util
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
FIXTURE_DIR = ROOT / "crates" / "meerkat-runtime" / "tests" / "fixtures" / "v0_7_x_pre_ledger_realm"
GENERATOR = FIXTURE_DIR / "mint_pre_ledger_fixture.py"
CORPUS = FIXTURE_DIR / "corpus"

SPEC = importlib.util.spec_from_file_location("mint_pre_ledger_fixture", GENERATOR)
assert SPEC and SPEC.loader
mint = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(mint)

SESSION_ID = "01a00703-2118-7712-8d1b-5fd002e2a7d1"

# One capture root under the real corpus. `git check-ignore` answers from the
# ignore rules, so these paths do not have to exist; anchoring them here is
# what makes the repo-wide rule (`corpus/**/*.lock`) apply at all.
CAPTURE_ROOT = CORPUS / "realms" / "0.7.5" / "attempted-turn"

# Capture-relative path -> is it a transient artifact of the capturing process?
# Every shape the committed corpus carries, plus the lock shapes that must not
# survive a capture.
CORPUS_PATH_EXPECTATIONS = {
    f".rkat/events/.sequence/{SESSION_ID}.lock": True,
    "sequence.lock": True,
    f".rkat/events/.sequence/{SESSION_ID}.seq": False,
    f".rkat/events/{SESSION_ID}.jsonl": False,
    f".rkat/sessions/{SESSION_ID}/checkpoint": False,
    f".rkat/sessions/{SESSION_ID}/events.jsonl": False,
    "realm_manifest.json": False,
    "sessions.sqlite3": False,
    "workgraph.sqlite3": False,
}


def git_ignores(repo_relative: str) -> bool:
    """Ask git whether the ignore rules drop this path, index state aside."""
    completed = subprocess.run(
        ["git", "-C", str(ROOT), "check-ignore", "--no-index", "--quiet", "--", repo_relative],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode not in (0, 1):
        raise AssertionError(
            f"git check-ignore could not answer for {repo_relative}: "
            f"exit {completed.returncode}: {completed.stderr.strip()}"
        )
    return completed.returncode == 0


def write_capture_tree(realm: pathlib.Path, *, extra: dict[str, bytes] | None = None) -> None:
    """A realm shaped like the one an attempted-turn capture leaves behind."""
    payloads: dict[str, bytes] = {
        "realm_manifest.json": b'{"realm_id":"legacy-realm"}\n',
        "sessions.sqlite3": b"SQLite format 3\x00sessions",
        "workgraph.sqlite3": b"SQLite format 3\x00workgraph",
        f".rkat/events/{SESSION_ID}.jsonl": b'{"event":"turn_started"}\n',
        f".rkat/events/.sequence/{SESSION_ID}.seq": b"3\n",
        f".rkat/events/.sequence/{SESSION_ID}.lock": b"",
        f".rkat/sessions/{SESSION_ID}/checkpoint": b'{"seq":3}\n',
        f".rkat/sessions/{SESSION_ID}/events.jsonl": b'{"event":"turn_started"}\n',
    }
    payloads.update(extra or {})
    for name, body in payloads.items():
        target = realm / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(body)


class MintPreLedgerFixtureExclusionTests(unittest.TestCase):
    def test_a_fresh_capture_walk_emits_no_lock_paths(self) -> None:
        """The acceptance property: locks never enter the payload list."""
        with tempfile.TemporaryDirectory() as scratch:
            realm = pathlib.Path(scratch) / "legacy-realm"
            write_capture_tree(realm)
            # The lock is really there, so a walk that emitted it would.
            self.assertTrue((realm / f".rkat/events/.sequence/{SESSION_ID}.lock").is_file())

            files = mint.realm_relative_files(realm)

            self.assertEqual(
                [name for name in files if name.endswith(".lock")],
                [],
                "a fresh capture walk emitted a transient lock path",
            )
            # Targeted, not a blanket drop of the sequence directory.
            self.assertIn(f".rkat/events/.sequence/{SESSION_ID}.seq", files)
            self.assertIn(f".rkat/sessions/{SESSION_ID}/checkpoint", files)
            self.assertIn("sessions.sqlite3", files)

    def test_a_fresh_capture_copies_and_binds_the_same_lock_free_file_set(self) -> None:
        """The copied tree and the manifest payloads cannot disagree."""
        with tempfile.TemporaryDirectory() as scratch:
            realm = pathlib.Path(scratch) / "legacy-realm"
            destination = pathlib.Path(scratch) / "corpus" / "realms" / "0.7.5" / "attempted-turn"
            write_capture_tree(realm)

            copied = mint.copy_realm(realm, destination)
            payloads = mint.payload_entries(destination)

            on_disk = sorted(
                entry.relative_to(destination).as_posix()
                for entry in destination.rglob("*")
                if entry.is_file()
            )
            bound = sorted(str(entry["path"]) for entry in payloads)
            self.assertEqual(on_disk, sorted(copied))
            self.assertEqual(bound, on_disk)
            self.assertEqual([name for name in on_disk if name.endswith(".lock")], [])
            self.assertIn(f".rkat/events/.sequence/{SESSION_ID}.seq", bound)

    def test_the_generator_and_gitignore_agree_on_every_corpus_path_shape(self) -> None:
        """One answer for "transient", asked of both owners of the question."""
        self.assertTrue(CORPUS.is_dir(), f"corpus root is missing at {CORPUS}")
        corpus_relative_capture = CAPTURE_ROOT.relative_to(ROOT).as_posix()

        disagreements = []
        for capture_relative, expected_transient in CORPUS_PATH_EXPECTATIONS.items():
            repo_relative = f"{corpus_relative_capture}/{capture_relative}"
            ignored = git_ignores(repo_relative)
            excluded = mint.is_transient_capture_artifact(capture_relative)
            if not (ignored == excluded == expected_transient):
                disagreements.append(
                    f"{capture_relative}: gitignore drops={ignored}, "
                    f"generator excludes={excluded}, expected={expected_transient}"
                )
        self.assertEqual(disagreements, [])

        # Both polarities are exercised, so neither half can be vacuous.
        self.assertIn(True, CORPUS_PATH_EXPECTATIONS.values())
        self.assertIn(False, CORPUS_PATH_EXPECTATIONS.values())

    def test_a_wal_sidecar_is_still_refused_rather_than_dropped(self) -> None:
        """Dropping locks must not become dropping evidence of a late open."""
        with tempfile.TemporaryDirectory() as scratch:
            realm = pathlib.Path(scratch) / "legacy-realm"
            write_capture_tree(realm, extra={"sessions.sqlite3-wal": b"wal"})
            with self.assertRaises(SystemExit) as raised:
                mint.realm_relative_files(realm)
            self.assertIn("sessions.sqlite3-wal", str(raised.exception))

    def test_a_capture_missing_canonical_realm_files_is_still_refused(self) -> None:
        """The exclusion must not be able to empty a capture into silence."""
        with tempfile.TemporaryDirectory() as scratch:
            realm = pathlib.Path(scratch) / "legacy-realm"
            (realm / ".rkat" / "events" / ".sequence").mkdir(parents=True)
            (realm / ".rkat" / "events" / ".sequence" / f"{SESSION_ID}.lock").write_bytes(b"")
            with self.assertRaises(SystemExit) as raised:
                mint.realm_relative_files(realm)
            self.assertIn("sessions.sqlite3", str(raised.exception))


if __name__ == "__main__":
    unittest.main()
