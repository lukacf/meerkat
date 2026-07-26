# Witness v3: incremental transcript-history witness + migration story

Status: DESIGN FOR REVIEW (Luka gate — no code until approved).
Driver: the witness is the largest *named* per-turn digest cost on the large
fixture — 109 MB/turn of 435 (measured, site split, gate-run1.log).

## Why the witness is O(document) today

`assemble_transcript_history_witness` canonicalizes the graph value in the
order `commits < head < revisions` and feeds ONE sequential SHA-256. Any head
change (i.e. every append) re-hashes everything ordered after it — the whole
retained-body set. The digest it produces
(`SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST`) is folded into the canonical
checkpoint document digest, so it participates in every stamp mint and every
stamp verification.

## Why it cannot simply be changed

The witness participates in stamp digests. Change the computation and every
stored stamp reads as a digest mismatch: all saved sessions fail closed at
load. 0.8.6 taught us exactly what shipping a format transition with an
unfinished read path does to a fleet.

## The v3 computation

Split the single sequential hash into three independently-maintainable parts:

    commits_digest   = H(canonical(commits))       — retained SHA-256 midstate;
                       commits are APPEND-ONLY (audit log), so this is O(delta)
                       per rewrite and rewrites are rare.
    revisions_digest = H over the sorted per-body lines
                       "revision || serde(created_at)" — bodies are already
                       content-addressed (revision == digest of messages), so
                       this hashes ~66-byte strings per retained body:
                       O(bodies), not O(bytes). created_at is included because
                       body digests do not pin it (task #2 finding).
    witness_v3       = H("v3" || commits_digest || head || revisions_digest)

Per ordinary append: head changes, retained set changes by at most the head
body → recompute = one line change in revisions_digest input + the outer hash.
O(bodies) worst case, O(1) typical. The 109 MB/turn becomes KB/turn.

Integrity properties preserved:
- Body content stays pinned transitively (revision strings ARE content
  digests; the save guards verify body digests at ingress — the sealed
  ValidatedTranscriptHistory from task #3).
- created_at and digest_format are pinned explicitly (they were the two
  fields nothing pinned — task #2).
- Commits stay pinned byte-exactly (canonical JSON, append-only).
What v3 does NOT re-verify per turn: body bytes against revision strings.
That verification belongs to seal-at-ingress (landed), not to witness
assembly. This is the corruption-detection-vs-identity distinction already
agreed: digests remain load-bearing identity; whole-graph revalidation per
turn is what dies.

## Migration story (the part that needs review)

The graph already carries `digest_format: u32` and heal machinery keyed on it
(`heal_legacy_revision_strings` etc. run when `digest_format <
TRANSCRIPT_DIGEST_FORMAT_CURRENT`). v3 rides the same rail:

1. **Verify under the format the evidence declares.** Stamp verification
   recomputes the witness using the GRAPH's `digest_format` — v2 graphs
   verify with the v2 sequential computation forever. No stored session ever
   reads as invalid. (Fail-closed unchanged: an unknown format > CURRENT
   refuses — the one-way door stays one-way, same as `refuse_future_schema`.)
2. **Mint at CURRENT on write.** Any save that installs/refreshes the graph
   upgrades `digest_format` to v3 and mints the v3 witness inside the same
   document write that already mints the stamp — atomic per document, no
   fleet coordination.
3. **Forced convergence pass for idle fleets.** Heal-on-write never converges
   an idle fleet (case-6 lesson: "idle fleets never save"). `rkat storage
   migrate` gains a forced pass: load → upgrade format → save under CAS.
   `storage doctor` gains a format census (v2 vs v3 counts per store) so
   operators can see convergence.
4. **Downgrade behavior.** A 0.8.x binary older than v3 reading a v3 graph:
   `digest_format` ahead of its CURRENT → typed refusal at decode (verify
   this is today's behavior for a future format; if it heals-forward blindly,
   THAT is a pre-existing bug to fix in the same PR). Documented in the
   release notes: upgrading a fleet to v3-writing binaries is one-way per
   session, same contract as every meerkat_schema ledger bump.
5. **Mixed-version fleets.** Per-session, not per-store: a store may hold v2
   and v3 graphs simultaneously; every reader handles both indefinitely. No
   flag day.

## Test obligations (mutation-proven, per repo discipline)

- v2 fixture verifies under a v3-writing binary (compat pin: a REAL serialized
  v2 session checked into fixtures, not one minted by the same code under
  test).
- v3 mint → v2-only reader refuses (fail-closed pin).
- Byte-flip in a retained body under v3: seal-at-ingress refuses (proves the
  integrity budget moved to ingress, not vanished).
- created_at-flip under v3: witness changes (pins the new field).
- Bytes-per-turn on the large fixture before/after: expect witness bucket
  109 MB → <1 MB; gate assertion updated only with the measured number.
- Forced-pass idempotence: running storage-migrate twice converges once.

## Explicit non-goals

- No change to `head_revision`/strand-row identity (content digests stay).
- No change to the checkpoint stamp shape or provenance vocabulary.
- SessionDelta whole-document serialization is a separate item.
