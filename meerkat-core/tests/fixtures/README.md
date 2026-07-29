# N-1 evidence corpus — scope and honest limits

## What this corpus IS

Three files capturing **one tiny session document, in two projections, as
serialized by real meerkat-core 0.8.8 code** (never minted by the code under
test):

| File | Contents |
|------|----------|
| `v0_8_8_full_session.json` | Full session document (inline transcript-history graph, v2 witness, stamp schema 1) |
| `v0_8_8_slim_session.json` | Head-canonical slim row of the SAME session (out-of-line witness carrier) |
| `v0_8_8_manifest.json` | The 0.8.8-recorded `checkpoint_digest` + `history_witness` values and session id |

Consumed by `meerkat-core/tests/witness_v3_migration.rs`
(`v2_evidence_keeps_verifying_under_the_v3_binary` and the downgrade-door
tests): it pins exactly one contract — **0.8.8-minted v2 digest evidence
keeps verifying, byte-for-byte, under the current (v3-writing) binary**, with
no flag day for mixed stores.

## What this corpus is NOT

This is **not** an upgrade-boundary corpus, and green tests over it must not
be read as upgrade-boundary coverage. It contains **none** of what a real N-1
realm carries on disk:

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

Concretely: both 0.8.8→0.8.9 upgrade-boundary defects shipped in a release
whose fixture-level suites were green, and were caught only by restoring a
**full production realm dump** (the HomeCore dump gate — legacy-tail
adoption and the verified-evolution erase-guard refusal; see task history
for 0.8.10). This corpus could not have caught either.

## What a full N-1 realm fixture needs

A corpus that actually covers the upgrade boundary is a **complete realm
state root** captured by the previous release's binaries:

1. session store (SQLite) with several sessions, at least one carrying a
   compaction rewrite and a chain of resume-system-prompt-refresh commits;
2. runtime store rows for live and idle sessions (snapshots, lifecycle
   records, receipts, input states — including an accepted-but-unconsumed
   input);
3. the event log with rewrite-audit envelopes matching (2);
4. blob store content referenced by the sessions;
5. realm manifest + `meerkat_schema` ledger rows exactly as the N-1 binary
   left them;
6. a manifest of expected post-upgrade invariants (session ids, digests,
   message counts) to assert after the current binary opens the realm.

## Capture sketch (cheap path)

No dedicated capture script exists yet. The cheap version, using only
shipping tools:

```bash
# with the N-1 release binary (e.g. cargo install rkat --version =0.8.8):
export REALM_ROOT=$(mktemp -d)
rkat --state-root "$REALM_ROOT" --realm fixture-realm run "seed turn one"
rkat --state-root "$REALM_ROOT" --realm fixture-realm resume last "seed turn two"
# ... drive enough turns/resumes to mint refresh rewrites and a compaction ...
rkat --state-root "$REALM_ROOT" storage doctor   # read-only: record the diagnosis
tar -C "$REALM_ROOT" -cf v0_8_x_realm.tar .      # the fixture is the whole state root
```

plus a small generator that records the expected invariants (ids, digests,
counts) into a manifest the consuming test asserts. `rkat storage doctor` is
read-only and safe to run on the capture both before and after upgrade;
`rkat session list`/`rkat blob` can enumerate expected contents for the
manifest.

Until that exists, treat this directory as digest-evidence coverage only.
