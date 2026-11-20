#!/usr/bin/env python3
"""Verify that one Meta release is publicly readable from every registry."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import time
from http.client import HTTPException
from typing import Any, Callable, NamedTuple
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urljoin
from urllib.request import Request, urlopen


CRATE_NAME = "meerkat-meta"
EXTENSION_CRATE_NAME = "meta-extension-state"
PYPI_NAME = "meerkat-meta"
NPM_NAME = "@rkat/meta-sdk"


class WrongAnswer(NamedTuple):
    """A registry that serves the version, and serves something else than
    expected: other bytes, another package or version, a yanked release."""

    registry: str
    served: str
    expected: str


class RegistryVerificationError(RuntimeError):
    """One or more public registries do not expose the requested release.

    `registries` names the registries whose check failed; it is empty when
    the failure is not a registry's answer. `wrong_answers` lists the checks
    that failed because the registry serves the version, and serves it wrong.
    """

    def __init__(
        self,
        message: str,
        registries: tuple[str, ...] = (),
        wrong_answers: tuple[WrongAnswer, ...] = (),
    ) -> None:
        super().__init__(message)
        self.registries = registries
        self.wrong_answers = wrong_answers


class WrongAnswerError(RegistryVerificationError):
    """A version that is served, with something else than expected.

    `served` and `expected` say what, for the refusal: an answer that only
    lacks a field (no version, no yank state) is not one of these.
    """

    def __init__(self, message: str, *, served: str, expected: str) -> None:
        super().__init__(message)
        self.served = served
        self.expected = expected


def _named(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


class PublishedArtifact(NamedTuple):
    """One registry artifact plus the digest that registry attests."""

    label: str
    url: str
    digest_algorithm: str
    expected_digest: str


def _read_json(url: str, *, timeout: float = 30.0) -> dict[str, Any]:
    request = Request(
        url,
        headers={
            "Accept": "application/json",
            "User-Agent": "meerkat-meta-release-verifier",
        },
    )
    with urlopen(request, timeout=timeout) as response:
        payload = json.load(response)
    if not isinstance(payload, dict):
        raise RegistryVerificationError(f"{url} returned a non-object JSON payload")
    return payload


def _download_artifact(url: str, *, timeout: float = 60.0) -> bytes:
    request = Request(
        url,
        headers={"User-Agent": "meerkat-meta-release-verifier"},
    )
    with urlopen(request, timeout=timeout) as response:
        payload = response.read()
    if not payload:
        raise RegistryVerificationError(f"{url} returned an empty artifact")
    return payload


def _required_text(value: Any, *, field: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise RegistryVerificationError(f"registry response is missing {field}")
    return value.strip()


def _verify_crates_payload(
    payload: dict[str, Any], version: str, crate_name: str = CRATE_NAME,
    verified_sha256: str | None = None,
) -> list[PublishedArtifact]:
    published = payload.get("version")
    if not isinstance(published, dict):
        raise RegistryVerificationError("crates.io response is missing version metadata")
    served_crate, served_version = published.get("crate"), published.get("num")
    if served_crate != crate_name or served_version != version:
        message = "crates.io returned unexpected package/version metadata"
        if _named(served_crate) and _named(served_version):
            raise WrongAnswerError(
                message,
                served=f"crate {json.dumps(served_crate)} version {json.dumps(served_version)}",
                expected=f"crate {json.dumps(crate_name)} version {json.dumps(version)}",
            )
        raise RegistryVerificationError(message)
    if published.get("yanked") is not False:
        message = "crates.io release is yanked or lacks yank state"
        if published.get("yanked") is None:
            raise RegistryVerificationError(message)
        raise WrongAnswerError(
            message,
            served=f"{crate_name} {version} with yank state {json.dumps(published.get('yanked'))}",
            expected="false",
        )
    download_path = _required_text(published.get("dl_path"), field="crate download path")
    checksum = _required_text(published.get("checksum"), field="crate checksum")
    # The release verified one exact .crate per crate; crates.io must serve
    # those bytes, whichever attempt uploaded them.
    if verified_sha256 is not None and checksum.lower() != verified_sha256.lower():
        raise WrongAnswerError(
            f"crates.io serves {crate_name} {version} with checksum {checksum}, "
            f"not the verified {verified_sha256}",
            served=f"{crate_name} {version} with sha256 {checksum.lower()}",
            expected=verified_sha256.lower(),
        )
    return [
        PublishedArtifact(
            label="crate archive",
            url=urljoin("https://crates.io", download_path),
            digest_algorithm="sha256",
            expected_digest=checksum,
        )
    ]


def _verify_pypi_payload(
    payload: dict[str, Any], version: str
) -> list[PublishedArtifact]:
    info = payload.get("info")
    if not isinstance(info, dict) or info.get("version") != version:
        message = "PyPI returned an unexpected package version"
        if isinstance(info, dict) and _named(info.get("version")):
            raise WrongAnswerError(
                message,
                served=f"version {json.dumps(info.get('version'))}",
                expected=f"version {json.dumps(version)}",
            )
        raise RegistryVerificationError(message)

    files = payload.get("urls")
    if not isinstance(files, list):
        raise RegistryVerificationError("PyPI response is missing release artifacts")
    expected_types = {"bdist_wheel", "sdist"}
    relevant = [
        item
        for item in files
        if isinstance(item, dict) and item.get("packagetype") in expected_types
    ]
    for item in relevant:
        if item.get("yanked") is False:
            continue
        message = "PyPI wheel or source distribution is yanked or lacks yank state"
        if item.get("yanked") is None or not _named(item.get("filename")):
            raise RegistryVerificationError(message)
        raise WrongAnswerError(
            message,
            served=f"{item['filename']} with yank state {json.dumps(item.get('yanked'))}",
            expected="false",
        )
    package_types = {item.get("packagetype") for item in relevant}
    missing = expected_types - package_types
    if missing:
        raise RegistryVerificationError(
            "PyPI release is missing expected artifact types: "
            + ", ".join(sorted(missing))
        )
    artifacts: list[PublishedArtifact] = []
    for item in relevant:
        digests = item.get("digests")
        if not isinstance(digests, dict):
            raise RegistryVerificationError("PyPI artifact is missing digests")
        artifacts.append(
            PublishedArtifact(
                label=_required_text(item.get("filename"), field="PyPI filename"),
                url=_required_text(item.get("url"), field="PyPI artifact URL"),
                digest_algorithm="sha256",
                expected_digest=_required_text(
                    digests.get("sha256"), field="PyPI SHA-256 digest"
                ),
            )
        )
    return artifacts


def _verify_npm_payload(
    payload: dict[str, Any], version: str
) -> list[PublishedArtifact]:
    served_name, served_version = payload.get("name"), payload.get("version")
    if served_name != NPM_NAME or served_version != version:
        message = "npm returned unexpected package/version metadata"
        if _named(served_name) and _named(served_version):
            raise WrongAnswerError(
                message,
                served=f"package {json.dumps(served_name)} version {json.dumps(served_version)}",
                expected=f"package {json.dumps(NPM_NAME)} version {json.dumps(version)}",
            )
        raise RegistryVerificationError(message)
    dist = payload.get("dist")
    if not isinstance(dist, dict):
        raise RegistryVerificationError("npm response is missing distribution metadata")
    return [
        PublishedArtifact(
            label="npm tarball",
            url=_required_text(dist.get("tarball"), field="npm tarball URL"),
            digest_algorithm="sha1",
            expected_digest=_required_text(dist.get("shasum"), field="npm SHA-1 digest"),
        )
    ]


def _registry_checks(
    version: str,
    crate_sha256: dict[str, str] | None = None,
) -> tuple[
    tuple[
        str,
        str,
        Callable[[dict[str, Any], str], list[PublishedArtifact]],
    ],
    ...,
]:
    encoded_crate = quote(CRATE_NAME, safe="")
    encoded_extension = quote(EXTENSION_CRATE_NAME, safe="")
    encoded_pypi = quote(PYPI_NAME, safe="")
    encoded_npm = quote(NPM_NAME, safe="")
    encoded_version = quote(version, safe="")
    crate_sha256 = crate_sha256 or {}
    return (
        (
            "crates.io",
            f"https://crates.io/api/v1/crates/{encoded_crate}/{encoded_version}",
            lambda payload, version: _verify_crates_payload(
                payload, version, CRATE_NAME, crate_sha256.get(CRATE_NAME)),
        ),
        (
            "crates.io extension-state",
            f"https://crates.io/api/v1/crates/{encoded_extension}/{encoded_version}",
            lambda payload, version: _verify_crates_payload(
                payload, version, EXTENSION_CRATE_NAME, crate_sha256.get(EXTENSION_CRATE_NAME)),
        ),
        (
            "PyPI",
            f"https://pypi.org/pypi/{encoded_pypi}/{encoded_version}/json",
            _verify_pypi_payload,
        ),
        (
            "npm",
            f"https://registry.npmjs.org/{encoded_npm}/{encoded_version}",
            _verify_npm_payload,
        ),
    )


CRATE_REGISTRIES = {CRATE_NAME: "crates.io", EXTENSION_CRATE_NAME: "crates.io extension-state"}
# The registries the SDK packages publish to; `--only-registry` selects among
# them (crates are selected with `--only-crates`).
SDK_REGISTRIES = ("PyPI", "npm")
# A registry accepts an upload before it serves the version: npm has taken
# from one to more than six minutes. The wait is this long on the clock.
DEFAULT_DEADLINE_SECONDS = 1800.0
DEFAULT_DELAY_SECONDS = 10.0


def _selected_checks(
    version: str,
    crate_sha256: dict[str, str] | None,
    only_crates: bool,
    only_registries: frozenset[str] | None,
):
    """The registry checks this run makes; a run that makes none is refused."""
    if only_crates and only_registries:
        raise ValueError("only_crates and only_registries select different registries")
    named_crates = {CRATE_REGISTRIES[name] for name in (crate_sha256 or {})}
    selected = [
        check
        for check in _registry_checks(version, crate_sha256)
        if not (only_crates and check[0] not in named_crates)
        and not (only_registries and check[0] not in only_registries)
    ]
    if not selected:
        raise RegistryVerificationError("no registry was checked")
    return selected


def verify_once(
    version: str,
    *,
    reader: Callable[[str], dict[str, Any]] = _read_json,
    downloader: Callable[[str], bytes] = _download_artifact,
    crate_sha256: dict[str, str] | None = None,
    only_crates: bool = False,
    only_registries: frozenset[str] | None = None,
) -> list[str]:
    """Check the release on each registry; return the registries checked."""
    failures: list[str] = []
    failed: list[str] = []
    wrong: list[WrongAnswer] = []
    checked: list[str] = []
    for registry, url, validator in _selected_checks(
        version, crate_sha256, only_crates, only_registries
    ):
        checked.append(registry)
        try:
            artifacts = validator(reader(url), version)
            for artifact in artifacts:
                payload = downloader(artifact.url)
                actual_digest = hashlib.new(
                    artifact.digest_algorithm, payload
                ).hexdigest()
                if actual_digest.lower() != artifact.expected_digest.lower():
                    raise WrongAnswerError(
                        f"{artifact.label} checksum mismatch: "
                        f"expected {artifact.expected_digest}, got {actual_digest}",
                        served=(f"{artifact.label} with {artifact.digest_algorithm} "
                                f"{actual_digest.lower()}"),
                        expected=artifact.expected_digest.lower(),
                    )
        except (
            RegistryVerificationError,
            HTTPError,
            URLError,
            TimeoutError,
            OSError,
            # An answer that ends in the middle (IncompleteRead) is no OSError.
            HTTPException,
            ValueError,
        ) as error:
            failures.append(f"{registry}: {error!r}" if isinstance(error, HTTPException)
                            else f"{registry}: {error}")
            failed.append(registry)
            if isinstance(error, WrongAnswerError):
                wrong.append(WrongAnswer(registry, error.served, error.expected))
    if failures:
        raise RegistryVerificationError("; ".join(failures), tuple(failed), tuple(wrong))
    return checked


def success_line(
    version: str,
    checked: list[str],
    *,
    only_crates: bool = False,
    only_registries: frozenset[str] | None = None,
) -> str:
    """The success line, naming exactly the registries verify_once checked.

    `--only-crates` checks crates.io alone, so its line names neither PyPI nor
    npm as verified. It used to claim all three, even while npm answered 404
    for the version. `--only-registry` names what it left unchecked too.
    """
    crates = [name for name, registry in CRATE_REGISTRIES.items() if registry in checked]
    parts: list[str] = []
    if crates:
        parts.append(f"{' and '.join(crates)} {version} on crates.io")
    if "PyPI" in checked:
        parts.append(f"{PYPI_NAME} {version} wheel+sdist on PyPI")
    if "npm" in checked:
        parts.append(f"{NPM_NAME} {version} on npm")
    joined = ", ".join(parts[:-1]) + ", and " + parts[-1] if len(parts) > 2 else " and ".join(parts)
    line = f"Verified exact registry metadata and checksummed artifacts for {joined}"
    if only_crates:
        line += " (--only-crates: PyPI and npm were not checked)"
    elif only_registries:
        unchecked = ([] if crates else ["crates.io"]) + [
            registry for registry in SDK_REGISTRIES if registry not in checked
        ]
        if unchecked:
            verb = "was" if len(unchecked) == 1 else "were"
            line += f" (--only-registry: {' and '.join(unchecked)} {verb} not checked)"
    return line


def verify_with_retries(
    version: str,
    *,
    deadline_seconds: float = DEFAULT_DEADLINE_SECONDS,
    delay_seconds: float = DEFAULT_DELAY_SECONDS,
    attempts: int | None = None,
    reader: Callable[[str], dict[str, Any]] = _read_json,
    downloader: Callable[[str], bytes] = _download_artifact,
    sleeper: Callable[[float], None] = time.sleep,
    clock: Callable[[], float] = time.monotonic,
    crate_sha256: dict[str, str] | None = None,
    only_crates: bool = False,
    only_registries: frozenset[str] | None = None,
) -> None:
    """Read the registries until all serve the release or the deadline passes.

    Only a version that is not served yet waits. The wait is
    `deadline_seconds` on `clock`, measured from the first read: after a
    failed read the next one starts `delay_seconds` later, unless that start
    would fall after the deadline. `attempts`, when given, also caps the
    number of reads.

    A registry that serves the version, and serves it wrong, is refused,
    not waited for: other bytes than the expected digest, another package
    or version, a yanked release. When two reads in a row give the same
    wrong answer, the run fails at the second, naming the registry, what it
    serves and what was expected. A wrong answer that changes between reads
    (a mirror catching up) keeps waiting, and so does every other answer:
    an absent version, a transport error, a missing artifact kind, an
    answer without the expected fields.
    """
    if not math.isfinite(deadline_seconds):
        raise ValueError("deadline_seconds must be a finite number")
    if not math.isfinite(delay_seconds):
        raise ValueError("delay_seconds must be a finite number")
    if deadline_seconds < 0:
        raise ValueError("deadline_seconds must not be negative")
    if delay_seconds < 0:
        raise ValueError("delay_seconds must not be negative")
    if attempts is not None and attempts < 1:
        raise ValueError("attempts must be at least 1")
    if delay_seconds == 0 and attempts is None:
        raise ValueError("delay_seconds of 0 needs an attempts cap")
    # A selection that checks nothing is not a registry's answer: no wait.
    _selected_checks(version, crate_sha256, only_crates, only_registries)

    started = clock()
    attempt = 0
    wrong_at_last_read: tuple[WrongAnswer, ...] = ()
    while True:
        attempt += 1
        try:
            checked = verify_once(version, reader=reader, downloader=downloader,
                                  crate_sha256=crate_sha256, only_crates=only_crates,
                                  only_registries=only_registries)
        except RegistryVerificationError as error:
            repeated = [wrong for wrong in error.wrong_answers if wrong in wrong_at_last_read]
            if repeated:
                refusals = "; ".join(
                    f"{wrong.registry} serves {wrong.served}, not the expected {wrong.expected}"
                    for wrong in repeated
                )
                raise RegistryVerificationError(
                    f"release {version} refused: {refusals} "
                    f"(the same answer at two reads in a row); last answer: {error}",
                    tuple(wrong.registry for wrong in repeated),
                    tuple(repeated),
                ) from error
            wrong_at_last_read = error.wrong_answers
            elapsed = clock() - started
            out_of_attempts = attempts is not None and attempt >= attempts
            if out_of_attempts or elapsed + delay_seconds > deadline_seconds:
                failing = ", ".join(error.registries) or "no registry answer"
                raise RegistryVerificationError(
                    f"release {version} was not fully public after {attempt} attempts "
                    f"in {elapsed:.0f} s (deadline {deadline_seconds:.0f} s); "
                    f"still failing: {failing}; last answer: {error}",
                    error.registries,
                ) from error
            # Flushed, so a hosted log shows each attempt at its own time.
            print(
                f"Registry verification attempt {attempt} incomplete "
                f"({elapsed:.0f} s of {deadline_seconds:.0f} s): {error}",
                flush=True,
            )
            sleeper(delay_seconds)
        else:
            print(success_line(version, checked, only_crates=only_crates,
                               only_registries=only_registries), flush=True)
            return


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--deadline-seconds", type=float, default=DEFAULT_DEADLINE_SECONDS,
                        help="how long to wait, from the first read, for every checked "
                             "registry to serve the release (default: %(default)s)")
    parser.add_argument("--delay-seconds", type=float, default=DEFAULT_DELAY_SECONDS,
                        help="pause between reads (default: %(default)s)")
    parser.add_argument("--attempts", type=int, default=None,
                        help="also stop after this many reads (default: no cap but the deadline)")
    parser.add_argument("--crate-sha256", action="append", default=[], metavar="CRATE=SHA256",
                        help="require crates.io to serve exactly this verified .crate checksum")
    parser.add_argument("--only-crates", action="store_true",
                        help="check only the crates named by --crate-sha256")
    parser.add_argument("--only-registry", action="append", default=[], choices=SDK_REGISTRIES,
                        help="check only this SDK package registry (repeatable)")
    args = parser.parse_args()
    crate_sha256 = {}
    for item in args.crate_sha256:
        name, _, digest = item.partition("=")
        if name not in CRATE_REGISTRIES or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
            parser.error(f"--crate-sha256 needs a known crate and a 64-hex checksum: {item!r}")
        crate_sha256[name] = digest
    if args.only_crates and not crate_sha256:
        parser.error("--only-crates needs at least one --crate-sha256")
    if args.only_crates and args.only_registry:
        parser.error("--only-registry and --only-crates select different registries; pass one")
    if args.only_registry and crate_sha256:
        parser.error("--only-registry reads no crate, so --crate-sha256 would bind nothing")

    version = args.version.strip().removeprefix("v")
    if not version:
        parser.error("--version must not be empty")
    try:
        verify_with_retries(
            version,
            deadline_seconds=args.deadline_seconds,
            delay_seconds=args.delay_seconds,
            attempts=args.attempts,
            crate_sha256=crate_sha256,
            only_crates=args.only_crates,
            only_registries=frozenset(args.only_registry) or None,
        )
    except (RegistryVerificationError, ValueError) as error:
        parser.exit(1, f"registry verification failed: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
