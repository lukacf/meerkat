# Memory Hub Roadmap - an External Memory Hub as Meta's Central Memory

Status: future-work direction (companion to
[`agent-memory-architecture.md`](agent-memory-architecture.md), which is the
architecture of record for the current initiative)

Date: 2026-07-01

---

## 1. Destination

An external memory hub becomes Meta's central memory hub — the durable system of record for
knowledge, eventually the default memory backend and possibly a mandatory one.
The memory hub is not just a graph database: it is an entity graph, a document +
**vector** store, a **timeline** (events with temporal refinement), a truth layer
(assertions → claims → truth slots, with LLM-powered conflict resolution landing
in parallel work), byte-span evidence provenance, ABAC with per-principal
subject visibility, staged commits, freshness/decay, and an outbox — behind REST
and MCP surfaces.

The initiative's system is deliberately the **runtime edge** of that hub: the
turn path (selector, injection, budgets, echo-safety), the lifecycle coupling
(distill-before-rotation, exit interviews, onboarding, hygiene), the agent-facing
tool surface, and a hot local store scoped to *working knowledge*. The division
of labor, stated once and enforced by the initiative's bright-line ratchet:

> **Meta owns how to work. The memory hub owns what is true.**

Meta's store never grows entities, relations, truth slots, embeddings, or
retrieval indexes. The moment a record is entity-shaped world knowledge rather
than working knowledge, it is hub material.

## 2. Why this is not a commitment made today

"Adopt the external memory hub" decomposes into three separately-timed decisions,
and the integration boundary keeps them separable:

1. **Compatibility discipline (made now, free, private).** The initiative's
   record schema maps onto the memory hub's `BaseFields` (realm↔space, security
   envelope, subjects, provenance chain); `EvidenceRef` is a hub-compatible
   evidence span (session source + revision + range); calibration profiles are
   one artifact family across both stacks. Zero public exposure, zero
   dependency; pure restraint and schema hygiene.
2. **Wire-boundary provider (cheap, still private).** `HubMemoryProvider`
   speaks HTTP/MCP to a *configured endpoint* — **not** a crate dependency. No
   license coupling, no crates.io exposure, no version lockstep beyond
   wire-schema stability, and both ends are ours. Meta ships publicly with
   "bundled store by default; point `memory.hub` at a memory hub endpoint if you
   have one" — and initially only our own deployments (the downstream app) have one.
3. **Mandatory hub (the real commitment, made later with evidence).** Requires
   public distribution of the memory hub — open-sourcing under MIT/Apache or shipping
   a supported binary/container — plus the operational and API-stability burden
   that implies. Decided after the provider has run in production, not guessed
   now. "Mandatory" can stay scoped for a long time: mandatory for full memory
   features, bundled-store fallback for basic operation.

The embedded-hub option (linking `crates/hub` in-process) is
**rejected for this trajectory**: it collapses decision 2 into decision 3 by
creating crate/license coupling at integration time, and hub-db is
in-memory-between-snapshots. If laptop-scale installs need a hub later, a
sidecar binary (the `hub-local-helper` pattern, or gateway-style process
management) achieves it without link-time coupling.

## 3. Phases (after the initiative's P0–P4)

**F1 — Wire-boundary provider, proven in the downstream app.**
`HubMemoryProvider` implements the trait v2 surface over REST/MCP against a
configured endpoint: records written through to a designated space, manifest
served from a local **projection cache** synced via the memory hub's outbox (the turn
path never blocks on the hub — the availability guarantee survives mandatory-hub
futures). Write-through with a durable local queue when the hub is unreachable.
The steward stages through the memory hub's `work_artifact`/`commit_bundle` boundary
where the hub is active, and hub-side conflict resolution uses the LLM-powered
truth maintenance from the parallel hub work — one calibrated conflict
resolver in the stack, not two.

**F2 — Evidence unification and graduation.**
Session transcripts and interaction streams ingest into the memory hub as sources
(immutable `doc_revision`s), making "memory is a derived view over immutable
evidence" literal: Distiller provenance refs become real hub evidence
spans; memory events land on the hub timeline; re-dreaming history becomes an
the hub's `reprocess_job`; the vector store yields semantic search over the whole
interaction history as a by-product. **Graduation** activates: dream-identified
world knowledge moves to the hub as source records, and the hot record becomes a
provenance-chained pointer. The fact-identity rule preventing double-recall:
hot records are authoritative for facts they originated; hub candidates derived
from them are suppressed by provenance chain, never by similarity guessing.
Hub-scale recall: hub hybrid search becomes the candidate generator feeding
the initiative's LLM Selector (deterministic candidate generation lives in the
hub, exactly once).

**F3 — Hub as default.**
The bundled store's role narrows to bootstrap + hot projection + offline
fallback. New deployments default to hub-backed memory; docs and examples flip.

**Decision gate — mandatory hub.** Named preconditions, all evidence-based:

- F1 provider proven in the downstream app's production across N Meta releases with a
  stable wire contract (the hub surfaces Meta pins are versioned and
  stability-marked — worth marking **now**, while both efforts are in flight).
- License sweep of the memory hub's dependency tree clean for MIT/Apache distribution
  (SurrealDB's ecosystem checked carefully: the server is BSL; SDK and embedded
  engines need explicit verdicts). Cheap to do early; do it before schemas
  freeze so the answer can't ambush the decision.
- Distribution story chosen: open source vs supported binary/container vs
  sidecar-managed.
- Realm↔space mapping settled (below), including the operator-scope question.

## 4. Design questions to settle before F1/F2 schemas freeze

1. **Realm↔space mapping.** Hub principals carry exactly one `space_id` and
   spaces are hard tenant boundaries. Likely: space-per-realm with
   `subject_allowlist` for identity-private records. The friction point is
   **operator scope** — realm-keyed in the initiative, but operators span realms
   in principle; cross-space queries don't exist. Either operator profiles live
   in a dedicated space Meta queries separately (with the initiative's
   declassification gate for realm-crossing facts), or the memory hub grows a sharing
   primitive. Decide before F2.
2. **TM detection routing — RESOLVED (hub PR #25, merged 2026-07-02).**
   The survey-era finding (detection purely structural, values never compared,
   cross-object rel conflicts structurally invisible, reasoner effectively
   unreachable) is fixed: detection now partitions claims by value (identical
   values corroborate — evidence merges, no bogus supersession windows; only
   real divergence opens conflicts), new `rel_set` slots catch cross-object
   contradictions, and real conflicts route to an LLM adjudicator that sees
   actual values, per-claim fact-times (ingestion timestamps deliberately
   withheld), and rendered evidence; verdicts carry supersession dates
   extracted from text, and predicates without semantics get LLM-proposed
   cardinality/temporal models persisted to the registry. Proven by a
   real-LLM e2e (two conflicting documents through the full chain, validity
   window closing at the verdict date, not ingestion date) plus a
   deterministic CI sibling. F1/F2's "one conflict resolver in the stack"
   premise is now real on the hub side.
3. **Wire-contract surface.** Which REST/MCP endpoints the provider pins
   (ingest, search, truth slots, outbox stream, staged commit), versioned and
   CI-checked on the hub side the way its MCP tool counts already are.
4. **Projection-cache consistency.** Outbox cursor semantics for the manifest
   projection (per-space monotonic seq exists; per-realm cursor storage and
   replay on Meta's side), and read-your-writes for an identity's own recent
   records during hub round-trips.

## 5. Non-goals of the hub path

- No merging of the stores in either direction: working memory in a knowledge
  graph buys the graph→prose reconstruction problem on the turn path
  permanently; a knowledge graph rebuilt in markdown records is the memory hub
  re-invented badly. The layers stay distinct (hippocampal working memory /
  cortical semantic knowledge); the *judgment* is shared, the storage is not.
- No Meta-side retrieval machinery at any scale — hub candidates feed the
  Selector; the bright line holds on both sides of the boundary.
- No hub crate dependency in Meta, in any phase.
