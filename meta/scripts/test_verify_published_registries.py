#!/usr/bin/env python3
"""Tests for exact public-registry release verification."""

from __future__ import annotations

import importlib.util
import hashlib
from pathlib import Path
import unittest


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


if __name__ == "__main__":
    unittest.main()
