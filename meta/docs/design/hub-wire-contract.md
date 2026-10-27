# Hub Wire Contract — the hub surfaces Meta F1 pins

> **Status: DRAFT for Luka's sign-off (2026-07-05).** The two
> "decide-before-schemas-freeze" items from
> [`memory-hub-roadmap.md`](memory-hub-roadmap.md) §4, drafted so the memory hub
> agents can be tasked with stability-marking their side while both efforts
> are in flight. Sections are labeled **DECISION (proposed)** where sign-off
> turns them binding, and **ASK (memory hub)** where the work lands in the
> hub repo.
>
> Licensing context (Luka, 2026-07-04): the memory hub itself is expected to adopt
> a **BSL license à la SurrealDB**, which reframes the roadmap's original
> MIT/Apache dep-sweep — the F-gate licensing precondition becomes "the memory hub's
> own BSL posture and its embedded-engine terms are settled", not "the dep
> tree is permissive".

## 1. The five pinned surfaces

The F1 `HubMemoryProvider` implements Meta's provider trait
(`recall` / `remember` / `forget` + manifest reads) and the steward's staged
commits over the wire — never a crate dependency (§12 bright line). It pins
exactly five hub surfaces. Anything else the hub offers is out of
contract and must not be load-bearing.

| # | Surface | Meta consumer | Contract shape |
|---|---------|-----------------|----------------|
| W1 | **Ingest** (record + source write) | `remember` write-through; F2 evidence ingestion (transcripts as immutable `doc_revision`s) | Idempotent PUT keyed by Meta record id; durable local queue on unreachability (availability guarantee: the turn path never blocks on the hub) |
| W2 | **Search** (hybrid retrieval) | Hub-scale recall: candidate generation feeding the LLM Selector | Query → ranked candidate list with provenance refs; deterministic paging; Meta treats scores as opaque ordering |
| W3 | **Truth slots** (claims/conflicts) | Graduation reads; fact-identity suppression (hot records authoritative for facts they originated — hub candidates suppressed by provenance chain, never similarity guessing) | Read-only slot query by subject/predicate; verdicts carry supersession dates (fact-time, not ingestion-time) |
| W4 | **Outbox stream** (change feed) | The manifest **projection cache** (per-space monotonic seq; Meta stores a per-realm cursor and replays on reconnect) | Cursor-resumable ordered stream; replay from any retained cursor is byte-stable |
| W5 | **Staged commit** (`work_artifact` / `commit_bundle`) | Steward dream output staged through the hub's boundary where the hub is active; hub-side conflict resolution via the memory hub's LLM truth maintenance (one calibrated resolver in the stack) | Two-phase: stage artifact → commit bundle with CAS token; rejected commits surface typed conflicts |

**ASK (memory hub):** version each surface (`X-Hub-Contract: v1` or
equivalent), stability-mark them the way the MCP tool counts already are, and
add the CI check that fails the memory hub's build when a pinned surface changes
shape without a version bump. Meta's provider will send the pinned version
and fail loud on mismatch (same posture as `META_CONTRACT_VERSION`).

## 2. Realm ↔ space mapping

**DECISION (proposed): space-per-realm, with `subject_allowlist` for
identity-private records.** Hub principals carry exactly one `space_id`
and spaces are hard tenant boundaries — this maps 1:1 onto Meta's realm
isolation (per-realm SQLite files today). Scope kinds project as:

| Meta scope | Hub projection |
|---|---|
| `Realm` | space-wide records |
| `Mob { mob }` | space records tagged `mob:<id>` (tag, not boundary — mobs share the realm trust domain) |
| `Identity { identity }` | space records with `subject_allowlist = [identity]` |
| `Operator { operator }` | space records with `subject_allowlist = [operator-principal]` — see below |

**Operator scope (resolves the roadmap's friction point).** The shipped
keying (meta #227) is the console auth principal, and operator-scope
records are **realm-keyed in v1** — the coordinator only ever composes scopes
within its own realm. Therefore F1/F2 need **no cross-space query and no
dedicated operator space**: operator records live in each realm's space,
allowlisted to the operator principal. The two futures stay open and neither
blocks a schema freeze:

- If cross-realm operator profiles are ever wanted, the declassification gate
  (architecture §7.2) runs on the Meta side and *copies* facts between
  spaces — no hub sharing primitive required.
- If the memory hub later grows a sharing primitive, the projection can migrate
  without changing the record shape (the allowlist entry is the contract).

**DECISION (proposed): the space id is derived, not configured:**
`meta/<deployment-id>/<realm>` — deterministic so a rebuilt gateway
reattaches to its space without a mapping table.

## 3. Consistency + cursor semantics (W4 detail)

- The projection cache serves manifest reads; the turn path reads ONLY the
  cache (availability guarantee survives mandatory-hub futures).
- Per-realm cursor stored in Meta's realm store (a `hub_cursor` row rides
  the existing SQLite file); replay on reconnect is idempotent because W1
  ingest is idempotent by record id.
- **Read-your-writes** for an identity's own recent records during hub
  round-trips: satisfied locally — the bundled store remains the hot layer in
  F1/F2, so an identity's own writes are visible before the hub echoes them.
  This is why F3 ("hub as default") still keeps the bundled store as
  bootstrap + hot projection + offline fallback.

## 4. What sign-off unblocks

1. Handing §1's ASK to the hub agents (surface enumeration is theirs to
   confirm against their actual REST/MCP routes; the *shape properties* above
   are the contract).
2. Freezing the F2 evidence schema against the space mapping in §2.
3. The F-gate licensing precondition rewrite (BSL posture) in
   `memory-hub-roadmap.md` §3.
