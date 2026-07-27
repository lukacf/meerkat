# Witness v3: revision-identity transcript-history witness + migration story

Status: IMPLEMENTED for 0.8.9 (design approved with required changes,
Luka review, 2026-07-27). This document records the design AS APPROVED and
AS SHIPPED; the original proposal's versioning rail (riding
`digest_format`), its `created_at` pinning, its persisted midstate, and its
forced idle-fleet migration were rejected and are replaced below.
Implementation landmarks: typed carrier + format resolution + v3
computation in `meerkat-core/src/checkpoint.rs`
(`TranscriptHistoryWitness`, `session_checkpoint_history_digest_v3`,
`session_checkpoint_digest_for_mint`), ingress gate in
`meerkat-core/src/session.rs`
(`validate_carried_transcript_history_witness_format`), accepted formats in
the generated `SessionPersistenceVersionAuthority`
(`restore_transcript_history_witness_format`), stamp door
`SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3`, tests in
`meerkat-core/tests/witness_v3_migration.rs` against committed v0.8.8
fixtures.

Driver: the witness is the largest *named* per-turn digest cost on the large
fixture — 109 MB/turn of 435 (measured, site split, gate-run1.log).

## Why the witness is O(document bytes) today

`assemble_transcript_history_witness` reproduces the canonical form
`{commits, head, revisions}` and feeds ONE sequential SHA-256. Canonical
segments are cached, but every derivation still re-absorbs every retained
body's bytes plus the live head transcript stream. Every append re-hashes
the whole retained set.

## Why it cannot simply be changed

The witness digest is folded into the canonical checkpoint document
(`session_checkpoint_digest_uncached` replaces the graph value with a
witness marker), so it participates in every stamp mint and every stamp
verification. Change the computation without a version rail and every
stored stamp reads as a digest mismatch: all saved sessions fail closed at
load. 0.8.6 taught us exactly what shipping a format transition with an
unfinished read path does to a fleet.

## Two distinct version axes (review decision)

`digest_format` on the graph is NOT the witness version. That field names
the revision-STRING format (the message-digest generation, currently 2); its
reader treats every value >= 2 as current and OVERWRITES the field at decode
(graph.rs), so a v3 marker written there would be silently erased. It stays
untouched at 2.

The witness gets its own axis: `transcript_history_witness_format`, with
accepted versions registered under the generated
`SessionPersistenceVersionAuthority` rather than a handwritten constant
check. v2 = today's sequential whole-graph canonical hash. v3 = the
revision-identity computation below.

## The v3 computation (typed, domain-separated preimage)

    commits_digest            = H(canonical(full ordered commit log))
    retained_revisions_digest = H(canonical(sorted unique retained revision IDs))
    witness_v3 = H(canonical({
        domain: "meerkat/transcript-history-witness/v3",
        revision_digest_format: 2,
        head_revision,
        commits_digest,
        retained_revisions_digest,
    }))

Properties:

- Revision IDs transitively pin canonical message content: a body's
  `revision` string IS `transcript_messages_digest(messages)`, and ingress
  (seal-at-ingress / `validate_transcript_history_state`) verifies body
  bytes against it. The set digest pins body PRESENCE without rehashing
  body bytes. What v3 deliberately stops doing per derivation is
  re-verifying body bytes against revision strings — that verification
  belongs to ingress, and the integrity budget moved there, not vanished.
- `created_at` and parent pointers are NOT pinned: the existing contract
  classifies revision-body timestamps and parent pointers as storage
  bookkeeping, not transcript identity. (The earlier cache-key finding
  pinned them in the decode-memo KEY because that cache substitutes a rich
  object; a cache-key obligation is not a durable-identity obligation.)
- Complexity is honest O(number of retained revisions), not "typically
  O(1)": replacing the live head can shift an arbitrary position in the
  sorted ID list, re-hashing a suffix or the whole (small) list. That is
  the accepted win: O(total retained transcript bytes) → O(retained
  revision count). No Merkle structure unless retained-revision counts
  prove this insufficient.
- `commits_digest` is CACHED on the sealed graph (ordinary appends never
  change commits) and recomputed on the rare rewrite. No persisted SHA-256
  midstate anywhere — a midstate would be another versioned integrity
  surface; caches only.

## The typed witness carrier (full and slim rows resolve identically)

Head-canonical slim rows deliberately drop the graph and today carry a bare
digest string under `SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY`. An
algorithm can never be inferred from an absent graph, so every carried
witness normalizes to one typed shape:

    TranscriptHistoryWitness {
        witness_format: u32,           // 2 or 3
        revision_digest_format: u32,   // 2
        digest: "sha256:...",
    }

- A bare string (every existing durable row) parses as
  `{witness_format: 2, revision_digest_format: 2, digest}`.
- v3 writers persist the object form.
- An UNKNOWN `witness_format` fails closed with a typed refusal BEFORE any
  normalization, healing, or mutation of the row.
- A slim v2 projection can NEVER relabel itself v3: it lacks the retained
  bodies needed to derive the v3 value. It keeps carrying v2 until an
  authority reconstructs and validates the complete graph.

## Downgrade one-way door: checkpoint stamp schema v3

Downgrade safety cannot ride `digest_format` (its reader erases future
values). It rides the checkpoint stamp schema:

    stamp schema = max(provenance-required schema, witness-format-required schema)

A document whose stamp digest was minted over a v3 witness mints stamp
schema 3. A pre-v3 binary refuses it through its existing typed
future-schema path (`UnsupportedSchemaVersion`) — never unknown-enum
corruption, never a silent v2 interpretation. Upgrading a fleet to
v3-writing binaries is one-way PER SESSION, the same contract as every
`meerkat_schema` ledger bump.

## Migration story

1. **Verify under the format the evidence declares.** The typed carrier
   (or, for full documents, the algorithm implied by the stamp schema)
   selects the computation; v2 evidence verifies with the v2 computation
   indefinitely. Mixed v2/v3 sessions coexist in one store forever;
   per-session, no flag day.
2. **Mint at CURRENT on authoritative full-graph write.** A save that
   installs/refreshes the complete validated graph mints the v3 witness
   and its schema-3 stamp inside the same document write. Lazy only.
3. **No forced idle-fleet migration.** Idle sessions pay no per-turn
   witness cost; readers support v2 indefinitely; bulk-rewriting dormant
   sessions adds custody, projection-coherence, fencing, and recovery risk
   with no immediate value. A read-only `storage doctor` format census may
   ship; an explicit opt-in migration waits for a demonstrated operational
   need and a protocol covering full/blob/head-canonical projections under
   one maintenance fence.
4. **Crash safety.** A crash or CAS failure during a lazy conversion
   leaves the v2 authority readable and unmodified — the conversion is one
   atomic document write that either lands or does not.

## Test obligations (mutation-proven, per repo discipline)

- REAL serialized v2 full-document fixture (checked in, not minted by the
  code under test) verifies under a v3-writing binary.
- REAL v2 slim-head + out-of-line graph fixture verifies.
- v3 full → slim → full round trip preserves the same typed witness.
- A slim v2 save cannot relabel itself v3.
- Unknown witness format refuses typed BEFORE healing or mutation.
- A v3 stamp is refused by the v2-only reader rule (pinned against the
  future-schema refusal path, which in 0.8.8 is the `schema_version != 1`
  check).
- Mutation proofs: head flip, commit-log flip, retained-revision-set flip,
  and a retained BODY byte flip — the last caught by seal-at-ingress,
  proving the integrity budget moved to ingress rather than vanished.
- Mixed v2/v3 sessions in one store.
- Crash/CAS failure during lazy conversion leaves the v2 authority
  readable.
- Bytes-per-turn on the large fixture: witness bucket 109 MB →
  O(revision count). The performance assertion claims independence from
  retained BODY BYTES, never from revision count.

## Explicit non-goals

- No change to `head_revision`/strand-row identity (content digests stay).
- No change to the checkpoint stamp SHAPE or provenance vocabulary — the
  stamp schema VERSION advances for v3-witness documents.
- No change to `digest_format` (revision-string format stays 2).
- SessionDelta whole-document serialization is a separate item.
