# Advisory: mobkit 0.8.6 — head-canonical sessions become unresumable after reboot

**Severity: availability. No data is destroyed.**

**Affected:** meerkat-mobkit 0.8.6 (and the paired meerkat 0.8.8 head-canonical
continuity path) — but only for sessions that have already converted to the
head-canonical representation. See **Am I affected?** below; most sessions in a
typical deployment are not.

**Not affected:** mobkit 0.8.5 and earlier.

---

## DO NOT ROLL BACK OR DOWNGRADE. READ THIS FIRST.

If you are seeing identities report **Broken** after a restart:

- **Do NOT run `mobkit_gateway storage-downgrade`.**
- **Do NOT restore a pre-upgrade backup.**
- **Do NOT delete or recreate the continuity database.**

Your data is intact and your sessions are recoverable by a fixed reader.
The durable state is correct — valid head rows, complete strand rows,
coherent fences. What is broken is the **read** path: it declines to serve the
durable head and hands back an older document instead.

Downgrading or restoring a backup is the one action that turns this into
real data loss: it discards every turn recorded since the upgrade. The bug
itself discards nothing.

**The safe response is to wait for the fix and restart.** If you must run in
the meantime, roll the *process* back to 0.8.5 binaries only if you have NOT
yet taken turns on 0.8.6 state; otherwise stay put.

---

## Am I affected?

Only sessions with a row in `continuity_session_heads` are at risk. Check with
a read-only query against a **stopped** gateway:

```sh
# mode=ro refuses to create a missing file and, unlike immutable=1, reads
# through the write-ahead log. Keep the -wal and -shm files next to the
# database or recent rows will be invisible. Do NOT use ?immutable=1 here:
# it silently ignores the WAL and can report "unaffected" for a database
# whose head rows still live only in the -wal file.
DB=<state>/continuity.db
test -f "$DB" && sqlite3 "file:$DB?mode=ro" \
  "SELECT session_id, message_count FROM continuity_session_heads;"
```

- **No rows / no such table** — you are not affected.
- **Rows returned** — those sessions are head-canonical and at risk. Every
  other identity in the same file is blob-canonical and reads correctly.
- **Read-only open errors** — that means "cannot determine", NOT
  "unaffected". Restore the `-wal`/`-shm` companions next to the database
  and rerun; never fall back to `?immutable=1` for this check.

In the reference corpus this was diagnosed against, 18 identities had
continuity records, exactly 2 had head rows, and exactly those 2 failed.

## Symptoms

- Identities report `Broken` after a restart that follows turns.
- Degrades across restarts (e.g. 2 ACTIVE -> 1 ACTIVE/1 BROKEN -> 2 BROKEN),
  because each turn advances the durable head while the frozen archive stays
  pinned at its last whole-document write.
- Either:
  - a save is **rejected** because the loaded document is behind the persisted
    head (e.g. reader at 101 messages, head at 103); or
  - resume reports the session snapshot **missing** despite a valid head row
    and a full set of strand rows.

These are **two distinct defects** with different causes. Both are read-path
faults; neither discards durable state.

**Form 1 — save rejected.** The reader is handed a stale *runtime snapshot*
(not the frozen archive) in place of the durable head. In the reference corpus
the runtime snapshot holds 101 messages while the durable head holds 103, so
the save is refused. The cause is a classification error: the committed
head-canonical head is treated as uncommitted intra-turn checkpoint residue
and therefore withheld. The write-side guard then refuses to regress the
transcript — correct behaviour, and it is what protected the data.

**Form 2 — snapshot reported missing.** The session carries an `archived`
lifecycle terminal in its durable document while its runtime record is not
`Retired`. The ordinary loader hides archived sessions, and the revival loader
only accepts `Retired`, so both return "nothing" and the caller reports the
snapshot as missing. It is not missing — it is archived, intact, and
byte-complete. This state is produced by retiring an identity: the archive
projection stamps the terminal, and no `Retired` runtime record survives
alongside it.

## Scope

Turns taken on 0.8.6 are durably recorded in the head and strand rows. Nothing
written is lost. A corrected reader resumes these sessions with their full
history. The stale document is the *archive*, not the truth.

## Status

Fixed in meerkat 0.8.9; the paired mobkit release picks it up. Both forms are
closed at the read path, and neither fix touches durable rows except to
promote them:

- **Form 1.** Cold reads now drive a typed, machine-owned read-source
  decision. A committed head-canonical head is served as authority, and a
  durable tail whose boundary commit lost the shutdown race is recovered
  through a machine-authorized commit instead of being withheld in favor of
  the stale runtime snapshot. Ambiguous evidence is held intact and resume
  fails typed (`SESSION_DURABLE_TAIL_HELD_FOR_RECOVERY` /
  `SESSION_DURABLE_EVIDENCE_QUARANTINED`) rather than serving a wrong copy.
- **Form 2.** An archived document whose runtime record never reached
  `Retired` is reported as archived (typed) instead of missing, and a
  repeated archive retires the residual runtime.

The operational guidance above stands until you are on the fixed versions:
do not downgrade, restore, or delete — the durable rows are the good copy,
and the fixed reader resumes them with full history.

## Credit

Found in pre-deployment validation against a real fleet corpus, before any
production deployment, by reproducing on a fresh clone with a single model
across four boot/turn/reboot cycles.

## Corrections to earlier drafts

Two mechanism claims in earlier drafts were wrong. The operational guidance
never changed and is, if anything, firmer: the durable rows are the good copy,
so do not downgrade or restore over them.

1. An early draft said the reader *declines* the frozen archive. A later draft
   said it *serves* the frozen archive. Both were wrong. The stale document
   served in Form 1 is the **runtime snapshot**, not the frozen continuity
   archive. The "99 messages" figure quoted in the second draft was a
   measurement error — it came from reading the database with SQLite's
   `immutable=1`, which silently ignores the write-ahead log and therefore
   reported pre-WAL state.

2. Both symptoms were initially attributed to one root cause. They are two
   independent defects, as set out above.

If you inspect a continuity database yourself, keep the `-wal` and `-shm` files
with it and open read-only **without** `immutable=1`, or you will read stale
values.
