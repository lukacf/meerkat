# Historical digest vectors — not an upgrade-compatibility corpus

## Supported release boundary

The 0.8.11 release supports upgrades from **Meerkat 0.8.10**. There are no
supported deployments on older Meerkat releases, so these files are not a
backward-compatibility promise and must not gate preservation of pre-0.8.10
storage shapes.

## What these vectors are

Three historical files capture **one tiny session document, in two
projections, serialized by meerkat-core 0.8.8 code**:

| File | Contents |
|------|----------|
| `v0_8_8_full_session.json` | Full session document (inline transcript-history graph, v2 witness, stamp schema 1) |
| `v0_8_8_slim_session.json` | Head-canonical slim row of the SAME session (out-of-line witness carrier) |
| `v0_8_8_manifest.json` | The 0.8.8-recorded `checkpoint_digest` + `history_witness` values and session id |

They are retained only as forensic reference. No current test or ordinary
Session loader consumes them, because 0.8.11 does not support a 0.8.8 upgrade
boundary. Production release evidence starts from the exact 0.8.10 importer
and complete-realm fixtures described below.

## What these vectors do not prove

Green tests over this directory do not establish any release upgrade
boundary. The vectors contain:

- **No SQLite files at all** — no `sessions.db` / `runtime.db` /
  `runtime.sqlite3`, no `meerkat_schema` migration-ledger rows, no table
  shapes, no WAL sidecars. Nothing here exercises `meerkat-sqlite` ledger
  pinning or `meerkat-store` migrations.
- **No runtime-store rows** — session snapshots, machine lifecycle records,
  unregister progress, boundary receipts, ops-lifecycle state.
- **No input ledger** — accepted/queued/consumed input states with their
  recovery-lane bookkeeping.
- **No event log** — no `EventStore` envelopes, in particular no
  transcript-rewrite audit events to replay against the graph.
- **No blob store**, no realm manifest/layout, no `.rkat/sessions/`
  projections, no mob storage (`mob.db`), no schedule store.
- **No scale or history** — a handful of KB, no compaction commits, no
  resume-refresh rewrite chains, no provider `ProviderMeta`, none of the
  multi-MB revision graphs where the 0.8.x defects actually lived.

The historical filenames remain because they describe where the known-answer
bytes came from. They do not declare that release supported.

## OB3 0.8.10 recovery-migration stamp

`v0_8_10_ob3_recovery_migration_session.json` is an exact, immutable
52,693-byte session document minted by Meerkat 0.8.10 and explicitly
authorized by OB3 for this corpus. It contains system content only. Its
SHA-256 is
`43e49a7b216cf61f6ba8f289824c9d6e24a64a81d873f9eb4a09c5b3f6f0cd98`;
the adjacent `.provenance.json` binds the producer, classification, session
identity, and expected checkpoint facts.

This artifact pins the production-relevant
`RecoveryMigration` + `authority_base.kind = legacy` stamp shape. It carries
neither a transcript-history graph nor an out-of-line witness, so it cannot
serve as evidence for the separate v2/v3 witness-before-transcode ordering.
That axis still requires a released graph-bearing artifact or another
proof-observable acceptance seam; this fixture must not be stretched to
certify facts absent from its bytes.

The shape is accepted only by the explicit one-time 0.8.10 importer. The
importer validates the released domain shape, strips the retired stamp as
untrusted metadata, and returns ordinary domain state plus a single-use receipt
that must be bound to the exact physical source by the adopting store. Current
Session decode and current writers do not interpret or mint this checkpoint
vocabulary.

## 0.8.10 → 0.8.11 release evidence

Upgrade evidence for this release must use a **complete realm state root
captured by the 0.8.10 binaries**:

1. session store (SQLite) with several sessions, binding any compaction and
   resume-system-prompt-refresh rewrite commits the released realm contains;
2. runtime store rows for live and idle sessions (snapshots, lifecycle
   records, receipts, input states — including an accepted-but-unconsumed
   input);
3. the event log with rewrite-audit envelopes matching (2);
4. blob store content referenced by the sessions;
5. realm manifest + `meerkat_schema` ledger rows exactly as 0.8.10
   left them;
6. a manifest of expected post-upgrade invariants (session ids, digests,
   message counts) to assert after the current binary opens the realm.

## Released-realm corpus

The capture/import contract and consuming recovery test live under
`meerkat-runtime/tests/fixtures/v0_8_10_released_realm`. That harness refuses
current-source generated or production-redacted bytes, binds the released
producer by version and SHA-256, verifies the raw 0.8.10 ledger/invariants
before import, and opens only a temporary copy with the current stores.

Until its real `corpus/` directory is populated from the HomeCore released
artifact, this directory remains historical digest-vector coverage only and
the released-realm recovery test fails as an explicit release blocker.
