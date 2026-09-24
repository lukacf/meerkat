#!/usr/bin/env python3
"""Mint the pre-ledger 0.7.x realm corpus from published `rkat` binaries.

The corpus this writes is the only evidence in the tree for what a realm
created before the 0.8.10 durable-state floor physically looks like. It is
therefore minted by running the published binaries themselves; nothing here
constructs old bytes with current source.

Each release contributes one or more *captures*. A capture is one execution of
one published binary under one documented environment, and it owns the realm
directory that execution left behind:

    bootstrap-only   no provider credentials at all. The binary writes its
                     realm files during storage bootstrap and dies at LLM
                     client construction, leaving pre-ledger schemas and no
                     rows.

    attempted-turn   `ANTHROPIC_API_KEY` set to a dummy value and the model
                     named explicitly. The binary admits the operator input,
                     persists runtime and session state, then fails at the
                     provider call. The realm it leaves carries rows, which is
                     what makes it able to reach the bridge's row-preparation
                     callback.

The published `checksums.sha256` for the release is the provenance root: the
downloaded asset is verified against it, the extracted binary's `--version`
output is verified against the release, and both digests are recorded in the
manifest that the test re-checks.

The script is additive. It refuses to overwrite a capture that already exists,
and it carries an existing capture's recorded provenance forward while
re-hashing its committed bytes, so adding a capture never re-mints (and never
silently replaces) bytes that are already committed and verified.

Usage:

    python3 mint_pre_ledger_fixture.py \
        --capture attempted-turn \
        --release 0.7.5=/abs/path/to/rkat-0.7.5-aarch64-apple-darwin.tar.gz \
        --checksums 0.7.5=/abs/path/to/v0.7.5/checksums.sha256 \
        --corpus "$PWD/corpus"

With no `--release`, the script only re-verifies every capture on disk against
the manifest and rewrites the manifest from those bytes.

Assets and checksums come from the public releases:

    gh release download v0.7.5 -R lukacf/meerkat \
        -p 'rkat-0.7.5-aarch64-apple-darwin.tar.gz' -p 'checksums.sha256'
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import tarfile
import tempfile

REALM_ID = "legacy-realm"
TARGET = "aarch64-apple-darwin"
SOURCE_REPO = "https://github.com/lukacf/meerkat"
CANONICAL_REALM_FILES = ("realm_manifest.json", "sessions.sqlite3", "workgraph.sqlite3")

# Suffixes of files the capturing process leaves behind that are not state of
# the realm being captured. A sequence lock is held by the writer while it is
# running and means nothing once it has stopped, so binding one to a content
# manifest makes the corpus unverifiable the moment the capture ends. The repo
# `.gitignore` drops the same suffix from the committed corpus tree, so a
# capture that carried one into the manifest would describe a file no clean
# checkout can have. `scripts/test_mint_pre_ledger_fixture.py` holds this
# tuple and that `.gitignore` rule to the same answer.
TRANSIENT_CAPTURE_SUFFIXES = (".lock",)
MANIFEST_SCHEMA_VERSION = 2
FIXTURE_ID = "meerkat-0.7.x-pre-ledger-realms"
BOOTSTRAP_ONLY = "bootstrap-only"
ATTEMPTED_TURN = "attempted-turn"
CAPTURE_IDS = (BOOTSTRAP_ONLY, ATTEMPTED_TURN)

# Not a credential. It exists so the published binary gets past auth
# resolution and into the turn, where the provider rejects it.
DUMMY_PROVIDER_SECRET = "dummy-not-a-real-key"

PURPOSE = (
    "Realms written by published pre-0.8.10 rkat binaries, used to prove the explicit "
    "storage bridge authenticates and migrates the schema those releases actually wrote, "
    "and admits the rows they actually left."
)


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def parse_pairs(values: list[str], flag: str) -> dict[str, pathlib.Path]:
    pairs: dict[str, pathlib.Path] = {}
    for value in values:
        if "=" not in value:
            raise SystemExit(f"{flag} expects <version>=<path>, got {value!r}")
        version, raw = value.split("=", 1)
        path = pathlib.Path(raw).expanduser().resolve()
        if not path.is_file():
            raise SystemExit(f"{flag} path does not exist: {path}")
        pairs[version] = path
    return pairs


def version_key(version: str) -> list[int]:
    return [int(part) for part in version.split(".")]


def published_asset_digest(checksums: pathlib.Path, asset: str) -> str:
    for line in checksums.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[1].rsplit("/", 1)[-1] == asset:
            return parts[0].lower()
    raise SystemExit(f"{asset} is not listed in {checksums}")


def extract_binary(asset: pathlib.Path, workdir: pathlib.Path) -> pathlib.Path:
    with tarfile.open(asset, "r:gz") as archive:
        members = [m for m in archive.getmembers() if m.isfile()]
        names = [m.name for m in members]
        if names != ["rkat"]:
            raise SystemExit(f"unexpected archive contents in {asset}: {names}")
        archive.extractall(workdir, members=members)
    binary = workdir / "rkat"
    binary.chmod(0o755)
    return binary


def capture_environment(workdir: pathlib.Path, secret: str | None) -> dict[str, str]:
    """A closed environment for the published binary.

    The host environment is not inherited: an ambient key, an `RKAT_*`
    override, or the operator's own `HOME` would all change what the binary
    writes, and none of that would be visible in the manifest.
    """
    home = workdir / "home"
    scratch = workdir / "tmp"
    for directory in (home, scratch):
        directory.mkdir(parents=True, exist_ok=True)
    env = {
        "HOME": str(home),
        "TMPDIR": str(scratch),
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
        "LANG": "en_US.UTF-8",
    }
    if secret is not None:
        env["ANTHROPIC_API_KEY"] = secret
    return env


def anthropic_default_model(binary: pathlib.Path, workdir: pathlib.Path) -> str:
    """Ask the published binary which Anthropic model its own catalog defaults to.

    Hardcoding a model id here would silently mint a broken capture the day a
    release's catalog disagrees with this script.
    """
    probe = workdir / "catalog-probe"
    probe.mkdir(parents=True, exist_ok=True)
    completed = subprocess.run(
        [
            str(binary),
            "--state-root",
            str(probe / "state"),
            "--context-root",
            str(probe),
            "--user-config-root",
            str(probe / "user"),
            "--realm",
            "catalog-probe",
            "--realm-backend",
            "sqlite",
            "models",
        ],
        cwd=probe,
        env=capture_environment(workdir, None),
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=300,
        check=True,
    )
    catalog = json.loads(completed.stdout)
    for provider in catalog.get("providers", []):
        if provider.get("provider") == "anthropic":
            model = provider.get("default_model_id")
            if not model:
                raise SystemExit("published catalog has no anthropic default model id")
            return str(model)
    raise SystemExit("published catalog lists no anthropic provider")


def run_capture(
    binary: pathlib.Path,
    version: str,
    capture_id: str,
    workdir: pathlib.Path,
) -> tuple[pathlib.Path, dict[str, object]]:
    """Execute one published binary once and return the realm it left."""
    context = workdir / "ctx"
    state = workdir / "state"
    user = workdir / "user"
    for directory in (context, state, user):
        directory.mkdir(parents=True, exist_ok=True)

    argv = [
        str(binary),
        "--state-root",
        str(state),
        "--context-root",
        str(context),
        "--user-config-root",
        str(user),
        "--realm",
        REALM_ID,
        "--realm-backend",
        "sqlite",
        "run",
    ]
    if capture_id == ATTEMPTED_TURN:
        model = anthropic_default_model(binary, workdir)
        env = capture_environment(workdir, DUMMY_PROVIDER_SECRET)
        argv += ["-m", model]
        command = (
            f"published rkat {version}: --realm {REALM_ID} --realm-backend sqlite "
            f"run -m {model} 'hello', with ANTHROPIC_API_KEY set to a dummy value and "
            "no other environment inherited"
        )
    elif capture_id == BOOTSTRAP_ONLY:
        env = capture_environment(workdir, None)
        command = (
            f"published rkat {version}: --realm {REALM_ID} --realm-backend sqlite "
            "run 'hello' with no provider credentials in the environment"
        )
    else:
        raise SystemExit(f"unknown capture id {capture_id!r}")
    argv.append("hello")

    completed = subprocess.run(
        argv,
        cwd=context,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=600,
        check=False,
    )
    if completed.returncode == 0:
        raise SystemExit(
            f"rkat {version} capture {capture_id} succeeded; a completed turn means a real "
            "provider answered, and this corpus must stay synthetic"
        )
    stderr = completed.stderr.strip().splitlines()
    writer_error = stderr[0][:400] if stderr else ""
    if DUMMY_PROVIDER_SECRET in writer_error:
        raise SystemExit("the writer echoed its secret; refusing to record that line")

    realm = state / REALM_ID
    if not realm.is_dir():
        raise SystemExit(f"rkat {version} did not create a realm directory at {realm}")
    receipt = {
        "capture_command": command,
        "writer_exit_code": completed.returncode,
        "writer_error": writer_error,
    }
    return realm, receipt


def is_transient_capture_artifact(relative_path: str) -> bool:
    """True for a path the capturing process wrote that is not realm state.

    This is the single place that decides it. Both the copy into the corpus and
    the manifest payload list are derived from `realm_relative_files`, so a
    suffix named here can neither reach the committed tree nor the manifest.
    """
    return relative_path.endswith(TRANSIENT_CAPTURE_SUFFIXES)


def realm_relative_files(realm: pathlib.Path) -> list[str]:
    """The realm state a capture carries, as `/`-joined paths relative to it.

    Transient capture artifacts are dropped rather than refused: they are
    expected output of a live writer. WAL/SHM sidecars are refused instead,
    because they mean the realm was opened *after* the writer stopped, so the
    bytes are no longer the ones the writer left.
    """
    files = sorted(
        relative
        for relative in (
            entry.relative_to(realm).as_posix() for entry in realm.rglob("*") if entry.is_file()
        )
        if not is_transient_capture_artifact(relative)
    )
    missing = [name for name in CANONICAL_REALM_FILES if name not in files]
    if missing:
        raise SystemExit(f"realm at {realm} is missing {missing}")
    sidecars = [name for name in files if name.endswith("-wal") or name.endswith("-shm")]
    if sidecars:
        raise SystemExit(
            f"realm at {realm} carries {sidecars}; a WAL/SHM sidecar means the realm was "
            "opened after the writer stopped, so these are not the bytes the writer left"
        )
    return files


def copy_realm(realm: pathlib.Path, destination: pathlib.Path) -> list[str]:
    files = realm_relative_files(realm)
    for name in files:
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(realm / name, target)
    return files


def payload_entries(capture_dir: pathlib.Path) -> list[dict[str, object]]:
    files = realm_relative_files(capture_dir)
    return [
        {
            "path": name,
            "bytes": (capture_dir / name).stat().st_size,
            "sha256": sha256_file(capture_dir / name),
        }
        for name in files
    ]


def with_probe_copy(database: pathlib.Path, reader):
    """Read a committed database through a throwaway copy.

    Opening the corpus file in place would let SQLite create sidecars next to
    bytes that must stay exactly as the writer left them.
    """
    with tempfile.TemporaryDirectory() as scratch:
        probe = pathlib.Path(scratch) / database.name
        shutil.copyfile(database, probe)
        conn = sqlite3.connect(str(probe))
        try:
            return reader(conn)
        finally:
            conn.close()


def sqlite_catalog(database: pathlib.Path) -> list[dict[str, str]]:
    def read(conn: sqlite3.Connection) -> list[dict[str, str]]:
        rows = conn.execute(
            "SELECT type, name, COALESCE(sql, '') FROM sqlite_master ORDER BY type, name"
        ).fetchall()
        return [{"type": kind, "name": name, "sql": sql} for kind, name, sql in rows]

    return with_probe_copy(database, read)


def sqlite_row_counts(database: pathlib.Path) -> dict[str, int]:
    def read(conn: sqlite3.Connection) -> dict[str, int]:
        names = [
            row[0]
            for row in conn.execute(
                "SELECT name FROM sqlite_master WHERE type = 'table' "
                "AND name NOT LIKE 'sqlite_%' ORDER BY name"
            )
        ]
        counts: dict[str, int] = {}
        for name in names:
            quoted = name.replace('"', '""')
            counts[name] = int(conn.execute(f'SELECT COUNT(*) FROM "{quoted}"').fetchone()[0])
        return counts

    return with_probe_copy(database, read)


def runtime_input_state_rows(database: pathlib.Path) -> list[dict[str, object]]:
    """Describe every runtime input row the writer left, by identity.

    Row counts alone would let a corpus claim to be row-bearing while carrying
    something else entirely. These descriptors name what is actually in there,
    and the test re-derives them from the committed bytes.
    """

    def read(conn: sqlite3.Connection) -> list[dict[str, object]]:
        present = conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'runtime_input_states'"
        ).fetchone()
        if present is None:
            return []
        described: list[dict[str, object]] = []
        for runtime_id, input_id, state in conn.execute(
            "SELECT runtime_id, input_id, state_json FROM runtime_input_states "
            "ORDER BY runtime_id, input_id"
        ):
            payload = state if isinstance(state, bytes) else str(state).encode("utf-8")
            decoded = json.loads(payload.decode("utf-8"))
            described.append(
                {
                    "runtime_id": runtime_id,
                    "input_id": input_id,
                    "stored_input_state_version": decoded.get("stored_input_state_version"),
                    "current_state": decoded.get("current_state"),
                    "input_type": (decoded.get("persisted_input") or {}).get("input_type"),
                    "state_json_sha256": hashlib.sha256(payload).hexdigest(),
                }
            )
        return described

    return with_probe_copy(database, read)


def describe_capture(capture_dir: pathlib.Path) -> dict[str, object]:
    """Everything about a capture that is derived from its committed bytes."""
    sessions = capture_dir / "sessions.sqlite3"
    workgraph = capture_dir / "workgraph.sqlite3"
    session_counts = sqlite_row_counts(sessions)
    return {
        "payloads": payload_entries(capture_dir),
        "sessions_sqlite_catalog": sqlite_catalog(sessions),
        "sessions_sqlite_row_counts": session_counts,
        "workgraph_sqlite_row_counts": sqlite_row_counts(workgraph),
        "runtime_input_state_rows": runtime_input_state_rows(sessions),
        # The property that decides whether this capture can reach the
        # bridge's row-preparation callback at all. Named for that, not for
        # the vaguer "has some rows": a bootstrap-only realm has a
        # `runtime_states` row and still never reaches the callback.
        "carries_runtime_input_rows": session_counts.get("runtime_input_states", 0) > 0,
    }


def enforce_capture_expectation(version: str, capture_id: str, described: dict[str, object]) -> None:
    counts = described["sessions_sqlite_row_counts"]
    assert isinstance(counts, dict)
    inputs = counts.get("runtime_input_states", 0)
    if capture_id == ATTEMPTED_TURN and inputs < 1:
        raise SystemExit(
            f"rkat {version} capture {capture_id} left {inputs} runtime_input_states rows; "
            "this capture exists to reach the bridge's row-preparation callback, and a realm "
            "with no input rows cannot"
        )
    if capture_id == BOOTSTRAP_ONLY and inputs != 0:
        raise SystemExit(
            f"rkat {version} capture {capture_id} left {inputs} runtime_input_states rows; "
            "the two profiles would then be the same capture twice"
        )


def load_recorded(corpus: pathlib.Path) -> tuple[dict[str, dict], dict[tuple[str, str], dict]]:
    """Read whatever manifest is already committed, in either schema version.

    Returns the per-release provenance and the per-capture provenance, which is
    the part that cannot be recomputed from disk: which published asset wrote
    these bytes, and under which command.
    """
    manifest_path = corpus / "fixture-manifest.json"
    if not manifest_path.is_file():
        return {}, {}
    recorded = json.loads(manifest_path.read_text(encoding="utf-8"))
    schema = recorded.get("schema_version")
    if schema not in (1, MANIFEST_SCHEMA_VERSION):
        raise SystemExit(f"unsupported committed manifest schema_version {schema!r}")
    releases: dict[str, dict] = {}
    captures: dict[tuple[str, str], dict] = {}
    for release in recorded.get("releases", []):
        version = release["meerkat_version"]
        releases[version] = {
            key: release[key]
            for key in (
                "meerkat_version",
                "source_release",
                "release_asset",
                "release_asset_sha256",
                "binary_version_output",
                "binary_sha256",
                "current_source_build",
            )
        }
        if schema == 1:
            listed = [
                {
                    "capture_id": BOOTSTRAP_ONLY,
                    "capture_command": release["capture_command"],
                    "payloads": release["payloads"],
                }
            ]
        else:
            listed = release["captures"]
        for capture in listed:
            captures[(version, capture["capture_id"])] = {
                "capture_command": capture["capture_command"],
                "writer_exit_code": capture.get("writer_exit_code"),
                "writer_error": capture.get("writer_error"),
                "payloads": capture["payloads"],
            }
    return releases, captures


def verify_carried_payloads(
    version: str,
    capture_id: str,
    recorded: list[dict[str, object]],
    observed: list[dict[str, object]],
) -> None:
    recorded_index = {entry["path"]: entry for entry in recorded}
    observed_index = {entry["path"]: entry for entry in observed}
    if set(recorded_index) != set(observed_index):
        raise SystemExit(
            f"{version}/{capture_id} file set changed: manifest has "
            f"{sorted(recorded_index)}, disk has {sorted(observed_index)}"
        )
    for name, entry in recorded_index.items():
        if entry["sha256"] != observed_index[name]["sha256"]:
            raise SystemExit(
                f"{version}/{capture_id}/{name} no longer matches its recorded digest; "
                "committed corpus bytes must never be edited in place"
            )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", action="append", default=[], metavar="VERSION=ASSET")
    parser.add_argument("--checksums", action="append", default=[], metavar="VERSION=FILE")
    parser.add_argument(
        "--capture",
        action="append",
        default=[],
        choices=list(CAPTURE_IDS),
        help="capture profile to mint for every --release (repeatable)",
    )
    parser.add_argument("--corpus", required=True)
    args = parser.parse_args()

    assets = parse_pairs(args.release, "--release")
    checksums = parse_pairs(args.checksums, "--checksums")
    if set(assets) != set(checksums):
        raise SystemExit("every --release version needs a matching --checksums version")
    requested = list(dict.fromkeys(args.capture))
    if assets and not requested:
        raise SystemExit("--release needs at least one --capture to mint")
    if requested and not assets:
        raise SystemExit("--capture needs at least one --release to mint from")

    corpus = pathlib.Path(args.corpus).expanduser().resolve()
    realms_root = corpus / "realms"
    realms_root.mkdir(parents=True, exist_ok=True)
    recorded_releases, recorded_captures = load_recorded(corpus)

    minted: dict[tuple[str, str], dict[str, object]] = {}
    for version in sorted(assets, key=version_key):
        asset = assets[version]
        expected_digest = published_asset_digest(checksums[version], asset.name)
        actual_digest = sha256_file(asset)
        if actual_digest != expected_digest:
            raise SystemExit(
                f"{asset.name} digest {actual_digest} does not match the published "
                f"checksum {expected_digest}"
            )
        already = recorded_releases.get(version)
        if already is not None and already["release_asset_sha256"] != expected_digest:
            raise SystemExit(
                f"release {version} is already bound to asset digest "
                f"{already['release_asset_sha256']}; minting a new capture from a different "
                "asset would leave one release entry describing two different binaries"
            )
        for capture_id in requested:
            destination = realms_root / version / capture_id
            if destination.exists():
                raise SystemExit(
                    f"refusing to overwrite the committed capture at {destination}; "
                    "remove it deliberately if it really must be re-minted"
                )

        with tempfile.TemporaryDirectory() as scratch:
            workdir = pathlib.Path(scratch)
            binary = extract_binary(asset, workdir / "bin")
            reported = subprocess.run(
                [str(binary), "--version"],
                capture_output=True,
                text=True,
                timeout=300,
                check=True,
            ).stdout.strip()
            if reported != f"rkat {version}":
                raise SystemExit(
                    f"extracted binary reports {reported!r}, expected 'rkat {version}'"
                )
            binary_digest = sha256_file(binary)
            recorded_releases[version] = {
                "meerkat_version": version,
                "source_release": f"{SOURCE_REPO}/releases/tag/v{version}",
                "release_asset": asset.name,
                "release_asset_sha256": expected_digest,
                "binary_version_output": f"rkat {version}",
                "binary_sha256": binary_digest,
                "current_source_build": False,
            }

            for index, capture_id in enumerate(requested):
                capture_work = workdir / f"mint-{index}-{capture_id}"
                realm, receipt = run_capture(binary, version, capture_id, capture_work)
                destination = realms_root / version / capture_id
                destination.mkdir(parents=True)
                copy_realm(realm, destination)
                minted[(version, capture_id)] = receipt

    # Rebuild the whole manifest from what is on disk, so a capture that is
    # present but unbound cannot hide in the corpus.
    releases: list[dict[str, object]] = []
    for version_dir in sorted(
        (entry for entry in realms_root.iterdir() if entry.is_dir()),
        key=lambda entry: version_key(entry.name),
    ):
        version = version_dir.name
        provenance = recorded_releases.get(version)
        if provenance is None:
            raise SystemExit(
                f"realms/{version} has no release provenance; mint it with --release/--checksums "
                "or remove it"
            )
        captures: list[dict[str, object]] = []
        for capture_dir in sorted(entry for entry in version_dir.iterdir() if entry.is_dir()):
            capture_id = capture_dir.name
            if capture_id not in CAPTURE_IDS:
                raise SystemExit(f"realms/{version}/{capture_id} is not a known capture profile")
            described = describe_capture(capture_dir)
            enforce_capture_expectation(version, capture_id, described)
            fresh = minted.get((version, capture_id))
            carried = recorded_captures.get((version, capture_id))
            if fresh is not None:
                command = fresh["capture_command"]
                writer_exit_code = fresh["writer_exit_code"]
                writer_error = fresh["writer_error"]
            elif carried is not None:
                verify_carried_payloads(
                    version, capture_id, carried["payloads"], described["payloads"]
                )
                command = carried["capture_command"]
                writer_exit_code = carried["writer_exit_code"]
                writer_error = carried["writer_error"]
            else:
                raise SystemExit(
                    f"realms/{version}/{capture_id} is on disk but no manifest records how it "
                    "was produced; provenance cannot be reconstructed after the fact"
                )
            capture_entry: dict[str, object] = {
                "capture_id": capture_id,
                "root": f"realms/{version}/{capture_id}",
                "capture_command": command,
            }
            if writer_exit_code is not None:
                capture_entry["writer_exit_code"] = writer_exit_code
            if writer_error is not None:
                capture_entry["writer_error"] = writer_error
            capture_entry.update(described)
            captures.append(capture_entry)
        if not captures:
            raise SystemExit(f"realms/{version} has no captures")
        releases.append({**provenance, "captures": captures})

    manifest = {
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "fixture_id": FIXTURE_ID,
        "data_classification": "synthetic_non_production",
        "purpose": PURPOSE,
        "realm_id": REALM_ID,
        "target": TARGET,
        "releases": releases,
    }
    manifest_path = corpus / "fixture-manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, indent=2, sort_keys=False) + "\n", encoding="utf-8"
    )
    print(f"wrote {manifest_path}")
    for release in releases:
        for capture in release["captures"]:
            counts = capture["sessions_sqlite_row_counts"]
            populated = {name: count for name, count in counts.items() if count > 0}
            print(
                f"  {release['meerkat_version']}/{capture['capture_id']}: "
                f"carries_runtime_input_rows={capture['carries_runtime_input_rows']} "
                f"sessions_rows={populated or '{}'}"
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
