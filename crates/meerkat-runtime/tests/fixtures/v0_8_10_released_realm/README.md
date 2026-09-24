# Meerkat 0.8.10 released-realm upgrade fixture

This directory is the release-boundary corpus for opening a synthetic SQLite
realm written by the published `rkat` 0.8.10 binary with the 0.8.11 stores. It
is deliberately separate from the small 0.8.8 digest vectors under
`meerkat-core/tests/fixtures`.

The committed corpus has this shape:

```text
corpus/
  fixture-manifest.json
  provenance/
    capture-receipt.json
  realm/
    realm_manifest.json
    sessions.sqlite3
    sessions.sqlite3.mfence
    ... every other file left by the cleanly stopped released writer ...
```

`fixture-manifest.json` binds every corpus payload by SHA-256, the exact
released producer binary and release, the capture receipt bytes, the complete
pre-upgrade `meerkat_schema` ledger, and the relevant session/runtime rows.
`recovery_contract.rs` verifies those hashes and raw 0.8.10 facts before
opening a temporary copy with the current session and runtime stores. It then
reopens the copy to prove activation is idempotent and verifies the session
head, message sequence, runtime snapshot authority, and consumed inputs through
public store reads.

The fixture is synthetic from inception. Redacting a production database is
not accepted: rewriting JSON or SQLite rows would no longer exercise bytes
written by the released artifact.

This corpus intentionally tests the persistence owned by Meerkat. MobKit
continuity ledgers and HomeCore deployment state are separate stores with
separate release evidence; they are not fabricated inside a Meerkat realm.
Likewise, a clean successful shutdown has consumed its accepted inputs. A
pending accepted input belongs in a crash/recovery corpus, not this
clean-shutdown upgrade corpus.

## Import

Import the cleanly stopped synthetic realm using the exact published binary
that wrote it:

```bash
python3 import_released_fixture.py \
  --released-binary /absolute/path/to/rkat \
  --released-binary-sha256 7a60f631c78cf6abc5abb523b503b86e752abeb13ae05d100f85164679435815 \
  --source-release https://github.com/lukacf/meerkat/releases/tag/v0.8.10 \
  --realm-root /absolute/path/to/state/fixture-realm \
  --capture-receipt /absolute/path/to/capture-receipt.json \
  --expectations /absolute/path/to/expectations.json \
  --output-corpus "$PWD/corpus"
```

The importer refuses:

- a producer whose version output is not exactly `rkat 0.8.10`;
- a binary under this source checkout or any `target/` directory;
- a binary whose SHA-256 differs from the independently supplied digest or
  capture receipt;
- a source other than the public Meerkat `v0.8.10` release;
- a production, redacted, or current-source capture receipt;
- symlinks, WAL/SHM sidecars, non-zero maintenance fences, or non-zero event
  sequence locks;
- missing realm/session tables, non-v2 session-store data, or expectations
  that disagree with the raw released rows;
- an existing output directory.

Zero-byte `*.mfence` files and the zero-byte
`.rkat/events/.sequence/<session>.lock` are durable artifacts created by the
released writer and are therefore included and hashed. The source realm must
have no WAL/SHM sidecars after its clean shutdown. The importer opens SQLite
only in read-only immutable mode.

## Expectations shape

`expectations.json` is schema 1:

```json
{
  "schema_version": 1,
  "fixture_id": "meerkat-0.8.10-rkat-released-synthetic",
  "realm_id": "fixture-realm",
  "sqlite_database": "sessions.sqlite3",
  "pre_upgrade_ledgers": [
    {"domain": "runtime-store", "version": 1},
    {"domain": "schedule-store", "version": 1},
    {"domain": "session-store", "version": 2}
  ],
  "sessions": [
    {
      "session_id": "<uuid>",
      "strand": "root",
      "message_count": 5,
      "head_revision": "sha256:<64 lowercase hex characters>",
      "rewrite_count": 0,
      "messages": [
        {
          "sequence": 0,
          "bytes": 123,
          "sha256": "<SHA-256 of the exact message_json BLOB>"
        }
      ]
    }
  ],
  "runtime_snapshots": [
    {
      "runtime_id": "rt:session:<uuid>",
      "session_id": "<uuid>",
      "bytes": 12345,
      "sha256": "<SHA-256 of the exact session_snapshot BLOB>"
    }
  ],
  "consumed_inputs": [
    {
      "runtime_id": "rt:session:<uuid>",
      "input_id": "<uuid>",
      "last_run_id": "<uuid>",
      "last_boundary_sequence": 0,
      "bytes": 1234,
      "sha256": "<SHA-256 of the exact state_json BLOB>"
    }
  ]
}
```

The session expectations bind the exact `session_heads` identity set and every
`session_strand_messages` BLOB in sequence. Runtime snapshot expectations bind
the exact released row set, byte length, digest, and embedded session identity.
Consumed-input expectations bind the exact `runtime_input_states` row set and
the durable facts that survive activation. The importer refuses a pre-upgrade
`runtime_session_authority` table because 0.8.10 did not have one; current
store-owned authority must appear only through the 0.8.11 activation.

## Capture receipt shape

`capture-receipt.json` is schema 1:

```json
{
  "schema_version": 1,
  "data_classification": "synthetic_non_production",
  "producer": "meerkat-release",
  "meerkat_version": "0.8.10",
  "writer_binary_path": "<path to the extracted published rkat asset>",
  "writer_binary_version_output": "rkat 0.8.10",
  "writer_binary_sha256": "7a60f631c78cf6abc5abb523b503b86e752abeb13ae05d100f85164679435815",
  "source_release": "https://github.com/lukacf/meerkat/releases/tag/v0.8.10",
  "release_asset": "rkat-0.8.10-aarch64-apple-darwin.tar.gz",
  "release_asset_sha256": "97501fa6bc078b344315e91981240f4b66a8d2f64c26f4575b04c74df73b5db7",
  "current_source_build": false,
  "capture_command": "<exact synthetic capture entry point>",
  "captured_at": "<RFC 3339 timestamp>",
  "clean_shutdown": true,
  "sanitization_method": "synthetic_inputs_before_capture"
}
```

The committed `corpus/` directory was produced through this importer from the
published 0.8.10 artifact. Its absence or any digest mismatch is a
release-blocking test failure, never a skipped test or permission to recreate
0.8.10 bytes with the code under test.
