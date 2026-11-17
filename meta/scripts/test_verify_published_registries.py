#!/usr/bin/env python3
"""Tests for exact public-registry release verification."""

from __future__ import annotations

import contextlib
import importlib.util
import hashlib
import io
from pathlib import Path
import sys
import unittest
from unittest import mock
from urllib.error import HTTPError, URLError


SCRIPT = Path(__file__).with_name("verify-published-registries.py")
SPEC = importlib.util.spec_from_file_location("verify_published_registries", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
registries = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(registries)


ARTIFACTS = {
    "https://crates.example/meerkat-meta-0.8.0.crate": b"crate archive",
    "https://crates.example/meta-extension-state-0.8.0.crate": b"extension crate archive",
    "https://files.example/meerkat_meta-0.8.0-py3-none-any.whl": b"wheel",
    "https://files.example/meerkat_meta-0.8.0.tar.gz": b"sdist",
    "https://npm.example/rkat-meta-sdk-0.8.0.tgz": b"npm tarball",
}


def artifact_digest(url: str, algorithm: str) -> str:
    return hashlib.new(algorithm, ARTIFACTS[url]).hexdigest()


def exact_payload(url: str) -> dict:
    if "crates.io" in url:
        crate = "meta-extension-state" if "meta-extension-state" in url else "meerkat-meta"
        artifact = f"https://crates.example/{crate}-0.8.0.crate"
        return {
            "version": {
                "crate": crate,
                "num": "0.8.0",
                "yanked": False,
                "dl_path": artifact,
                "checksum": artifact_digest(artifact, "sha256"),
            }
        }
    if "pypi.org" in url:
        wheel = "https://files.example/meerkat_meta-0.8.0-py3-none-any.whl"
        sdist = "https://files.example/meerkat_meta-0.8.0.tar.gz"
        return {
            "info": {"version": "0.8.0"},
            "urls": [
                {
                    "filename": "meerkat_meta-0.8.0-py3-none-any.whl",
                    "packagetype": "bdist_wheel",
                    "url": wheel,
                    "yanked": False,
                    "digests": {"sha256": artifact_digest(wheel, "sha256")},
                },
                {
                    "filename": "meerkat_meta-0.8.0.tar.gz",
                    "packagetype": "sdist",
                    "url": sdist,
                    "yanked": False,
                    "digests": {"sha256": artifact_digest(sdist, "sha256")},
                },
            ],
        }
    if "npmjs.org" in url:
        artifact = "https://npm.example/rkat-meta-sdk-0.8.0.tgz"
        return {
            "name": "@rkat/meta-sdk",
            "version": "0.8.0",
            "dist": {
                "tarball": artifact,
                "shasum": artifact_digest(artifact, "sha1"),
            },
        }
    raise AssertionError(f"unexpected URL: {url}")


def exact_artifact(url: str) -> bytes:
    return ARTIFACTS[url]


class PublishedRegistryTests(unittest.TestCase):
    def test_exact_release_is_verified_across_all_registries(self):
        registries.verify_once(
            "0.8.0", reader=exact_payload, downloader=exact_artifact
        )

    def test_pypi_requires_both_wheel_and_source_distribution(self):
        def missing_sdist(url: str) -> dict:
            payload = exact_payload(url)
            if "pypi.org" in url:
                payload["urls"] = payload["urls"][:1]
            return payload

        with self.assertRaisesRegex(
            registries.RegistryVerificationError, "sdist"
        ):
            registries.verify_once(
                "0.8.0", reader=missing_sdist, downloader=exact_artifact
            )

    def test_yanked_crate_is_rejected(self):
        def yanked_crate(url: str) -> dict:
            payload = exact_payload(url)
            if "crates.io" in url:
                payload["version"]["yanked"] = True
            return payload

        with self.assertRaisesRegex(
            registries.RegistryVerificationError, "crates.io release is yanked"
        ):
            registries.verify_once(
                "0.8.0", reader=yanked_crate, downloader=exact_artifact
            )

    def test_yanked_python_artifact_is_rejected(self):
        def yanked_wheel(url: str) -> dict:
            payload = exact_payload(url)
            if "pypi.org" in url:
                payload["urls"][0]["yanked"] = True
            return payload

        with self.assertRaisesRegex(
            registries.RegistryVerificationError, "PyPI wheel.*is yanked"
        ):
            registries.verify_once(
                "0.8.0", reader=yanked_wheel, downloader=exact_artifact
            )

    def test_downloaded_artifact_must_match_registry_checksum(self):
        def corrupt_artifact(url: str) -> bytes:
            if "crates.example" in url:
                return b"corrupt"
            return exact_artifact(url)

        with self.assertRaisesRegex(
            registries.RegistryVerificationError, "checksum mismatch"
        ):
            registries.verify_once(
                "0.8.0", reader=exact_payload, downloader=corrupt_artifact
            )

    def test_retry_allows_registry_propagation_but_still_requires_exact_artifacts(self):
        calls = 0
        sleeps: list[float] = []

        def propagating(url: str) -> dict:
            nonlocal calls
            attempt = calls // 4
            calls += 1
            payload = exact_payload(url)
            if attempt == 0 and "pypi.org" in url:
                payload["urls"] = payload["urls"][:1]
            return payload

        registries.verify_with_retries(
            "0.8.0",
            attempts=2,
            delay_seconds=0.25,
            reader=propagating,
            downloader=exact_artifact,
            sleeper=sleeps.append,
        )

        self.assertEqual(calls, 8)
        self.assertEqual(sleeps, [0.25])

    def test_registry_readback_never_reads_github_release_assets(self):
        """Registry retry and its readback require the core tier only: the
        readback reads crates.io, PyPI and npm and nothing on the GitHub
        release, so a trailing tier still being added cannot hold it up."""
        checks = registries._registry_checks("0.9.2")
        hosts = {url.split("/")[2] for _, url, _ in checks}
        self.assertEqual(hosts, {"crates.io", "pypi.org", "registry.npmjs.org"})
        source = SCRIPT.read_text()
        for marker in ("github.com", "api.github", "releases/", "gh release"):
            self.assertNotIn(marker, source)

    def test_crates_io_must_serve_the_verified_crate_bytes(self):
        verified = {
            "meerkat-meta": artifact_digest("https://crates.example/meerkat-meta-0.8.0.crate", "sha256"),
            "meta-extension-state": artifact_digest(
                "https://crates.example/meta-extension-state-0.8.0.crate", "sha256"),
        }
        registries.verify_once("0.8.0", reader=exact_payload, downloader=ARTIFACTS.__getitem__,
                               crate_sha256=verified)
        for crate in verified:
            with self.subTest(crate=crate):
                with self.assertRaisesRegex(registries.RegistryVerificationError, "not the verified"):
                    registries.verify_once("0.8.0", reader=exact_payload, downloader=ARTIFACTS.__getitem__,
                                           crate_sha256={**verified, crate: "0" * 64})

    def test_only_crates_checks_just_the_named_crates(self):
        seen = []

        def reader(url):
            seen.append(url)
            return exact_payload(url)

        digest = artifact_digest("https://crates.example/meerkat-meta-0.8.0.crate", "sha256")
        registries.verify_once("0.8.0", reader=reader, downloader=ARTIFACTS.__getitem__,
                               crate_sha256={"meerkat-meta": digest}, only_crates=True)
        self.assertEqual(seen, ["https://crates.io/api/v1/crates/meerkat-meta/0.8.0"])

    def test_only_crates_success_line_names_only_the_registries_checked(self):
        """--only-crates checks crates.io alone, but its success line named
        PyPI and npm too (rc 0 while npm answered 404 for the version)."""
        digest = artifact_digest("https://crates.example/meerkat-meta-0.8.0.crate", "sha256")

        def crates_only(url: str) -> dict:
            if "crates.io" not in url:
                raise AssertionError(f"--only-crates read {url}")
            return exact_payload(url)

        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            registries.verify_with_retries(
                "0.8.0", attempts=1, reader=crates_only, downloader=exact_artifact,
                crate_sha256={"meerkat-meta": digest}, only_crates=True,
            )
        line = output.getvalue().strip()
        self.assertEqual(
            line,
            "Verified exact registry metadata and checksummed artifacts for meerkat-meta 0.8.0 "
            "on crates.io (--only-crates: PyPI and npm were not checked)",
        )
        for unchecked in ("meta-extension-state", "wheel+sdist", "@rkat/meta-sdk"):
            self.assertNotIn(unchecked, line)

    def test_full_success_line_names_every_registry(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            registries.verify_with_retries(
                "0.8.0", attempts=1, reader=exact_payload, downloader=exact_artifact,
            )
        self.assertEqual(
            output.getvalue().strip(),
            "Verified exact registry metadata and checksummed artifacts for meerkat-meta and "
            "meta-extension-state 0.8.0 on crates.io, meerkat-meta 0.8.0 wheel+sdist on PyPI, "
            "and @rkat/meta-sdk 0.8.0 on npm",
        )

    def test_success_line_follows_the_registries_checked(self):
        line = registries.success_line("0.8.0", ["crates.io extension-state"], only_crates=True)
        self.assertIn("for meta-extension-state 0.8.0 on crates.io", line)
        self.assertNotIn("meerkat-meta 0.8.0", line)
        self.assertEqual(
            registries.success_line("0.8.0", ["crates.io", "npm"]),
            "Verified exact registry metadata and checksummed artifacts for meerkat-meta 0.8.0 "
            "on crates.io and @rkat/meta-sdk 0.8.0 on npm",
        )

    def test_a_run_that_checks_no_registry_fails(self):
        with self.assertRaisesRegex(registries.RegistryVerificationError, "no registry was checked"):
            registries.verify_once("0.8.0", reader=exact_payload, downloader=exact_artifact,
                                   crate_sha256={}, only_crates=True)

    def test_retry_exhaustion_fails_the_release(self):
        def unavailable(url: str) -> dict:
            raise OSError(f"not public: {url}")

        with self.assertRaisesRegex(
            registries.RegistryVerificationError, "not fully public after 2 attempts"
        ):
            registries.verify_with_retries(
                "0.8.0",
                attempts=2,
                delay_seconds=0,
                reader=unavailable,
                downloader=exact_artifact,
                sleeper=lambda _: None,
            )


class FakeTime:
    """A clock that moves only when the verifier sleeps or a read takes time."""

    def __init__(self) -> None:
        self.now = 0.0
        self.sleeps: list[float] = []

    def clock(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.sleeps.append(seconds)
        self.now += seconds


def npm_serving_from(fake: FakeTime, served_at: float, *, npm_read_seconds: float = 0.0):
    """Every registry serves at once, except npm: 404 until `served_at`."""

    def reader(url: str) -> dict:
        if "npmjs.org" in url:
            fake.now += npm_read_seconds
            if fake.now < served_at:
                raise HTTPError(url, 404, "Not Found", None, None)
        return exact_payload(url)

    return reader


class FlushCounting(io.StringIO):
    flushes = 0

    def flush(self) -> None:
        self.flushes += 1
        super().flush()


class RegistryWaitTests(unittest.TestCase):
    """The wait is a deadline on a clock, long enough for a registry that
    accepts an upload some minutes before it serves the version."""

    def wait(self, fake: FakeTime, reader, **kwargs) -> list[str]:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            try:
                registries.verify_with_retries(
                    "0.8.0", reader=reader, downloader=exact_artifact,
                    sleeper=fake.sleep, **kwargs,
                )
            finally:
                self.lines = output.getvalue().splitlines()
        return self.lines

    def test_npm_serving_eight_minutes_after_the_first_read_is_within_the_default_wait(self):
        fake = FakeTime()
        lines = self.wait(fake, npm_serving_from(fake, 480.0))
        self.assertEqual(fake.now, 480.0)
        self.assertEqual(len(fake.sleeps), 48)
        self.assertIn("@rkat/meta-sdk 0.8.0 on npm", lines[-1])
        self.assertTrue(lines[-1].startswith("Verified exact registry metadata"))

    def test_a_registry_that_never_serves_fails_at_the_deadline_and_is_named(self):
        fake = FakeTime()
        with self.assertRaises(registries.RegistryVerificationError) as raised:
            self.wait(fake, npm_serving_from(fake, float("inf")), clock=fake.clock)
        message = str(raised.exception)
        self.assertEqual(
            message,
            "release 0.8.0 was not fully public after 181 attempts in 1800 s "
            "(deadline 1800 s); still failing: npm; last answer: "
            "npm: HTTP Error 404: Not Found",
        )
        # No read starts after the deadline, and none is skipped before it.
        self.assertEqual(fake.now, 1800.0)
        self.assertEqual(fake.sleeps, [10.0] * 180)
        self.assertEqual(len(self.lines), 180)
        self.assertEqual(
            self.lines[0],
            "Registry verification attempt 1 incomplete (0 s of 1800 s): "
            "npm: HTTP Error 404: Not Found",
        )
        self.assertEqual(
            self.lines[-1],
            "Registry verification attempt 180 incomplete (1790 s of 1800 s): "
            "npm: HTTP Error 404: Not Found",
        )

    def test_every_registry_still_failing_at_the_deadline_is_named(self):
        fake = FakeTime()

        def reader(url: str) -> dict:
            if "npmjs.org" in url or "pypi.org" in url:
                raise HTTPError(url, 404, "Not Found", None, None)
            return exact_payload(url)

        with self.assertRaisesRegex(
            registries.RegistryVerificationError, r"still failing: PyPI, npm; last answer: "
        ):
            self.wait(fake, reader, clock=fake.clock, deadline_seconds=60.0)
        self.assertEqual(fake.now, 60.0)

    def test_serving_at_the_deadline_passes_and_just_after_it_fails(self):
        fake = FakeTime()
        lines = self.wait(fake, npm_serving_from(fake, 1800.0), clock=fake.clock)
        self.assertTrue(lines[-1].startswith("Verified exact registry metadata"))
        self.assertEqual(fake.now, 1800.0)

        late = FakeTime()
        with self.assertRaisesRegex(registries.RegistryVerificationError, "still failing: npm"):
            self.wait(late, npm_serving_from(late, 1800.5), clock=late.clock)
        self.assertEqual(late.now, 1800.0)

    def test_slow_reads_count_against_the_deadline(self):
        """The bound is time on the clock, not a number of reads."""
        fake = FakeTime()
        with self.assertRaisesRegex(
            registries.RegistryVerificationError,
            r"after 6 attempts in 1850 s \(deadline 1800 s\); still failing: npm",
        ):
            self.wait(fake, npm_serving_from(fake, float("inf"), npm_read_seconds=300.0),
                      clock=fake.clock)
        self.assertEqual(len(fake.sleeps), 5)

    def test_a_wait_that_would_check_no_registry_fails_without_waiting(self):
        def never(_seconds: float) -> None:
            raise AssertionError("waited for a selection that checks nothing")

        with self.assertRaisesRegex(registries.RegistryVerificationError, "no registry was checked"):
            registries.verify_with_retries(
                "0.8.0", reader=exact_payload, downloader=exact_artifact, sleeper=never,
                crate_sha256={}, only_crates=True,
            )

    def test_each_attempt_line_is_flushed_as_it_is_written(self):
        fake = FakeTime()
        output = FlushCounting()
        with contextlib.redirect_stdout(output):
            registries.verify_with_retries(
                "0.8.0", reader=npm_serving_from(fake, 20.0), downloader=exact_artifact,
                sleeper=fake.sleep,
            )
        self.assertEqual(len(output.getvalue().splitlines()), 3)
        self.assertGreaterEqual(output.flushes, 3)


NPM_TARBALL = "https://npm.example/rkat-meta-sdk-0.8.0.tgz"


def npm_bytes(*served: bytes):
    """A downloader whose npm tarball reads return `served` in turn, then the
    exact bytes; every other artifact is exact."""
    remaining = list(served)

    def downloader(url: str) -> bytes:
        if url == NPM_TARBALL and remaining:
            return remaining.pop(0)
        return ARTIFACTS[url]

    return downloader


class WrongDigestTests(unittest.TestCase):
    """Only a version that is not served yet waits for the clock. A registry
    that serves the version with other bytes is refused at the second read
    in a row that gives the same wrong digest."""

    def wait(self, fake: FakeTime, **kwargs) -> list[str]:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            try:
                registries.verify_with_retries(
                    "0.8.0", sleeper=fake.sleep, clock=fake.clock,
                    **{"reader": exact_payload, "downloader": exact_artifact, **kwargs},
                )
            finally:
                self.lines = output.getvalue().splitlines()
        return self.lines

    def test_the_same_wrong_digest_at_two_reads_in_a_row_is_refused_at_the_second(self):
        fake = FakeTime()
        other = b"other bytes"
        with self.assertRaises(registries.RegistryVerificationError) as raised:
            self.wait(fake, downloader=npm_bytes(*[other] * 500))
        self.assertEqual(
            str(raised.exception),
            "release 0.8.0 refused: npm serves npm tarball with sha1 "
            f"{hashlib.sha1(other).hexdigest()}, not the expected "
            f"{artifact_digest(NPM_TARBALL, 'sha1')} (the same answer at two reads in a row)",
        )
        self.assertEqual(raised.exception.registries, ("npm",))
        # At the second read, not at the deadline.
        self.assertEqual(fake.sleeps, [10.0])
        self.assertEqual(fake.now, 10.0)
        self.assertEqual(len(self.lines), 1)
        self.assertIn("attempt 1 incomplete (0 s of 1800 s): npm: npm tarball checksum mismatch",
                      self.lines[0])

    def test_a_crate_served_with_other_bytes_than_the_verified_ones_is_refused_at_the_second_read(self):
        fake = FakeTime()
        served = artifact_digest("https://crates.example/meerkat-meta-0.8.0.crate", "sha256")
        with self.assertRaises(registries.RegistryVerificationError) as raised:
            self.wait(fake, crate_sha256={"meerkat-meta": "0" * 64})
        self.assertEqual(
            str(raised.exception),
            f"release 0.8.0 refused: crates.io serves meerkat-meta 0.8.0 with sha256 {served}, "
            f"not the expected {'0' * 64} (the same answer at two reads in a row)",
        )
        self.assertEqual(raised.exception.registries, ("crates.io",))
        self.assertEqual(fake.sleeps, [10.0])

    def test_a_wrong_digest_once_and_then_the_right_bytes_pass(self):
        fake = FakeTime()
        lines = self.wait(fake, downloader=npm_bytes(b"other bytes"))
        self.assertEqual(fake.sleeps, [10.0])
        self.assertEqual(len(lines), 2)
        self.assertTrue(lines[-1].startswith("Verified exact registry metadata"))

    def test_a_wrong_digest_that_changes_between_reads_keeps_waiting(self):
        fake = FakeTime()
        lines = self.wait(fake, downloader=npm_bytes(b"first", b"second", b"third"))
        self.assertEqual(fake.sleeps, [10.0] * 3)
        self.assertTrue(lines[-1].startswith("Verified exact registry metadata"))

        never = FakeTime()
        changing = npm_bytes(*[str(read).encode() for read in range(500)])
        with self.assertRaisesRegex(
            registries.RegistryVerificationError,
            r"not fully public after 181 attempts in 1800 s \(deadline 1800 s\); still failing: npm; "
            r"last answer: npm: npm tarball checksum mismatch",
        ):
            self.wait(never, downloader=changing)
        self.assertEqual(never.now, 1800.0)

    def test_the_same_wrong_digest_with_a_read_that_did_not_serve_in_between_keeps_waiting(self):
        fake = FakeTime()
        reads = 0

        def reader(url: str) -> dict:
            nonlocal reads
            if "npmjs.org" in url:
                reads += 1
                if reads == 2:
                    raise HTTPError(url, 404, "Not Found", None, None)
            return exact_payload(url)

        # Read 1: other bytes. Read 2: 404 (no download). Read 3: the same
        # other bytes, not in a row with read 1. Read 4: exact.
        lines = self.wait(fake, reader=reader, downloader=npm_bytes(b"other", b"other"))
        self.assertEqual(fake.sleeps, [10.0] * 3)
        self.assertTrue(lines[-1].startswith("Verified exact registry metadata"))

    def test_a_version_not_served_and_then_served_passes(self):
        fake = FakeTime()
        lines = self.wait(fake, reader=npm_serving_from(fake, 10.0))
        self.assertEqual(fake.sleeps, [10.0])
        self.assertEqual(
            lines[0],
            "Registry verification attempt 1 incomplete (0 s of 1800 s): "
            "npm: HTTP Error 404: Not Found",
        )
        self.assertTrue(lines[-1].startswith("Verified exact registry metadata"))

    def test_the_same_transport_error_at_two_reads_in_a_row_is_not_a_refusal(self):
        for error in (TimeoutError("timed out"), URLError("connection reset"), OSError("unreachable")):
            with self.subTest(error=type(error).__name__):
                fake = FakeTime()
                reads = 0

                def reader(url: str) -> dict:
                    nonlocal reads
                    if "npmjs.org" in url:
                        reads += 1
                        if reads <= 3:
                            raise error
                    return exact_payload(url)

                lines = self.wait(fake, reader=reader)
                self.assertEqual(fake.sleeps, [10.0] * 3)
                self.assertTrue(lines[-1].startswith("Verified exact registry metadata"))

    def test_other_incomplete_answers_repeated_keep_waiting(self):
        """A release still missing an artifact kind, twice the same, waits."""
        fake = FakeTime()
        reads = 0

        def reader(url: str) -> dict:
            nonlocal reads
            payload = exact_payload(url)
            if "pypi.org" in url:
                reads += 1
                if reads <= 3:
                    payload["urls"] = payload["urls"][:1]
            return payload

        lines = self.wait(fake, reader=reader)
        self.assertEqual(fake.sleeps, [10.0] * 3)
        self.assertTrue(lines[-1].startswith("Verified exact registry metadata"))


class RegistrySelectionTests(unittest.TestCase):
    """--only-registry: the job that published the SDK packages waits for
    exactly the registries it published to."""

    def test_only_the_named_registries_are_read(self):
        seen = []

        def reader(url):
            seen.append(url)
            return exact_payload(url)

        checked = registries.verify_once("0.8.0", reader=reader, downloader=exact_artifact,
                                         only_registries=frozenset({"PyPI", "npm"}))
        self.assertEqual(checked, ["PyPI", "npm"])
        self.assertEqual(seen, [
            "https://pypi.org/pypi/meerkat-meta/0.8.0/json",
            "https://registry.npmjs.org/%40rkat%2Fmeta-sdk/0.8.0",
        ])

    def test_the_success_line_names_only_the_registries_checked(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            registries.verify_with_retries(
                "0.8.0", reader=exact_payload, downloader=exact_artifact,
                only_registries=frozenset({"PyPI", "npm"}),
            )
        self.assertEqual(
            output.getvalue().strip(),
            "Verified exact registry metadata and checksummed artifacts for meerkat-meta 0.8.0 "
            "wheel+sdist on PyPI and @rkat/meta-sdk 0.8.0 on npm "
            "(--only-registry: crates.io was not checked)",
        )
        self.assertEqual(
            registries.success_line("0.8.0", ["npm"], only_registries=frozenset({"npm"})),
            "Verified exact registry metadata and checksummed artifacts for @rkat/meta-sdk 0.8.0 "
            "on npm (--only-registry: crates.io and PyPI were not checked)",
        )

    def test_a_selected_registry_that_does_not_serve_is_waited_for_and_named(self):
        fake = FakeTime()
        output = io.StringIO()
        with contextlib.redirect_stdout(output), self.assertRaisesRegex(
            registries.RegistryVerificationError, "still failing: npm; last answer: npm: HTTP Error 404"
        ):
            registries.verify_with_retries(
                "0.8.0", reader=npm_serving_from(fake, float("inf")), downloader=exact_artifact,
                sleeper=fake.sleep, clock=fake.clock, only_registries=frozenset({"PyPI", "npm"}),
            )
        self.assertEqual(fake.now, 1800.0)


class RegistryCommandLineTests(unittest.TestCase):
    def run_cli(self, *argv: str):
        seen: dict = {}

        def record(version, **kwargs):
            seen.update(kwargs, version=version)

        errors = io.StringIO()
        with mock.patch.object(registries, "verify_with_retries", record), \
                mock.patch.object(sys, "argv", ["verify-published-registries.py", *argv]), \
                contextlib.redirect_stderr(errors):
            try:
                status = registries.main()
            except SystemExit as stop:
                status = stop.code
        return status, seen, errors.getvalue()

    def test_the_default_wait_is_thirty_minutes_on_the_clock(self):
        status, seen, _ = self.run_cli("--version", "v0.8.0")
        self.assertEqual(status, 0)
        self.assertEqual(seen["version"], "0.8.0")
        self.assertEqual(seen["deadline_seconds"], 1800.0)
        self.assertEqual(seen["delay_seconds"], 10.0)
        self.assertIsNone(seen["attempts"])
        self.assertIsNone(seen["only_registries"])

    def test_the_wait_and_the_selection_are_passed_through(self):
        status, seen, _ = self.run_cli("--version", "0.8.0", "--deadline-seconds", "90",
                                       "--only-registry", "PyPI", "--only-registry", "npm")
        self.assertEqual(status, 0)
        self.assertEqual(seen["deadline_seconds"], 90.0)
        self.assertEqual(seen["only_registries"], frozenset({"PyPI", "npm"}))

    def test_a_selection_of_registries_and_of_crates_together_is_refused(self):
        status, seen, errors = self.run_cli(
            "--version", "0.8.0", "--only-registry", "npm", "--only-crates",
            "--crate-sha256", "meerkat-meta=" + "0" * 64,
        )
        self.assertEqual(status, 2)
        self.assertEqual(seen, {})
        self.assertIn("--only-registry and --only-crates", errors)

    def test_an_unknown_registry_is_refused(self):
        status, seen, _ = self.run_cli("--version", "0.8.0", "--only-registry", "crates.io")
        self.assertEqual(status, 2)
        self.assertEqual(seen, {})


if __name__ == "__main__":
    unittest.main()
