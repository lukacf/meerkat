# Meta Agent Memory Architecture

Status: architecture of record for the **memory initiative** (internal design
note — `docs/design/` is deliberately outside the public docs navigation, per
the existing convention for design documents in this directory)

Date: 2026-07-01 (rev 4 — post external reviews ×2 and post 31-agent adversarial
review; restructured into initiative + future work)

Scope: this document specifies the system **this initiative builds**: a fully
functional, operational memory system running on Meta's bundled store, complete
without any external service. The longer-term trajectory - an external memory hub as the central
memory hub, eventually a default and possibly mandatory backend — lives in
[`memory-hub-roadmap.md`](memory-hub-roadmap.md). That trajectory constrains schema
and layering decisions here (§12) but gates nothing in this plan.

Evidence base: five-system survey (Claude Code, Codex, Meerkat, Meta, an external memory hub)
with adversarially verified findings, committed at
[`../archive/design/evidence/memory-survey-2026-07/`](../archive/design/evidence/memory-survey-2026-07/). File:line citations below
refer to the surveyed checkouts (2026-07-01); the archive README carries the
staleness caveat.

---

## 1. Thesis

Memory is what makes identity real. Meta already made the architectural bet that
identities are durable and sessions are disposable embodiments (materialize /
resume / respawn / reset). Today that bet is only half-kept: an identity survives
respawn, but almost everything it learned does not. The shipped `agent_memory`
layer preserves explicitly-remembered records in per-realm SQLite; legacy
Markdown records are accepted only as one-shot import input. Everything else — the
session's accumulated judgment, the mob's collective discoveries, the operator's
corrections — is abandoned at every rotation, and what *is* preserved is
append-only, contradiction-blind, and re-injected wastefully.

The target system is organized around a cognitive loop, not a storage topology:

```
observe → record/distill → consolidate → recall → inject → audit → (re-derive)
```

Every judgment in that loop is an LLM judgment. Deterministic code owns structure
only: paths, types, scopes, budgets, atomicity, and crash boundaries. This is not a
stylistic preference; it is the verified lesson of the two production systems that
work. Claude Code and Codex both ship **zero embeddings and zero heuristic
ranking** — recall quality comes from LLM selection over well-written
descriptions, and store quality comes from LLM curation under content discipline.
Every place the shipped Meta layer uses a heuristic today (stopword-filtered
term overlap, score threshold 2) is a place the survey found producing weak or
degenerate behavior. One corollary stated early because it shapes §8.3: "no
heuristics" applies to *judgment*, not to *ordering computed by prior judgment* —
an LLM-ranked working set consulted cheaply at runtime is judgment cached, not a
heuristic.

Two platform facts make Meta the best possible host for this loop, and the
design leans on both:

1. **Memory operations are agent work, and Meta's entire job is orchestrating
   agent work.** Extraction, consolidation, promotion, and audit run as ordinary
   scheduled identities (the *steward*, app-enabled per §8.5) with
   capability-scoped tool surfaces — observable in the console, governed by ABAC,
   dogfooding the platform.
2. **The platform gives us primitives the CLI harnesses lack**: identity lifecycle
   seams as consolidation triggers (Meta's identity layer —
   materialize / resume-fallback / respawn / reset / retire — built over
   meerkat-mob's identity-keyed member lifecycle events), O(1) copy-on-write
   session forking (`Session::fork`), `meerkat-schedule` for dream cadence,
   audited same-session transcript revisions (`session/rewrite_transcript`) for
   context hygiene, typed message classes and typed tool provenance, and
   fail-closed capability semantics.

---

## 2. The harness the agent should get

Design target stated from the working agent's side, because that is what
"superior to Claude Code and Codex" has to cash out as:

- **I wake up already oriented.** At materialization I get a tight, current index
  of what I know, what my mob knows, and what my operator cares about —
  descriptions good enough that I can tell what to pull without reading
  everything.
- **Remembering costs me one tool call, mid-task.** I flag the surprising thing
  and move on. Filing, deduplication, and merging are somebody else's job (the
  steward's), later.
- **Corrections stick.** When the operator corrects me, my next embodiment does
  not repeat the mistake. Operator corrections are the highest-signal writes in
  the system.
- **My memory never argues with itself.** I am not shown a stale fact and its
  correction side by side. Superseded records stay retrievable with provenance,
  but only the current view is injected.
- **My context stays clean.** Dead tool output and repeated scaffolding get pruned
  into audited revisions; decisions and their rationale survive; the raw past
  stays one search away.
- **Respawn is continuity, not amnesia with my name attached.** What my previous
  embodiment learned was distilled before the session rotated.
- **My teammates' discoveries reach me without ceremony**, and mine reach them —
  through the mob store, not through me doing comms overhead.
- **Everything is inspectable.** My operator can see what I remember, why, and
  where it came from, in the console. Nothing about memory is a black box.

Claude Code delivers roughly half of this for a single agent in a single project
directory. Codex delivers a different half, default-off. Neither can express the
mob or identity items at all.

---

## 3. Principles

1. **Memory is a derived view over immutable evidence.** Session transcripts and
   event logs are ground truth and are never mutated by memory operations
   (transcript *revisions* are additive and restorable, and provenance pins the
   revision it was captured against — §7.1). Every record carries provenance
   pointers into evidence. Consequences: a bad consolidation is a bad cache
   entry, not data loss; and when prompts or models improve, history can be
   **re-dreamed** — the store upgrades retroactively. Calibration investment
   compounds instead of depreciating.
2. **LLM judgment, code structure.** No heuristic scoring, lexical ranking, or
   embedding-based relevance anywhere on the quality path. Deterministic code
   enforces: scope isolation, trust tiers and their transition lattice, byte
   budgets, staged commits, retention floors, taint propagation, and crash
   recovery.
3. **Identity-first, evidence-from-sessions.** The identity is the store of
   record. Sessions are evidence streams. Session lifecycle boundaries
   (compaction, reset, respawn, retire) are exactly the moments distillation must
   run — currently they are the moments knowledge is destroyed.
4. **The mob is a memory community.** Scopes compose: identity-private,
   mob-shared, operator profile, application/realm. Spawn-time onboarding,
   retire-time harvesting, and steward-mediated propagation are first-class
   operations, not future work.
5. **Write discipline over retrieval machinery.** One fact per record; a
   description line written *for the future selector*; typed kinds; explicit
   why/how-to-apply for behavioral records. The index is an index, never a dump.
6. **Fail closed, degrade loud.** Memory must never block or break delivery
   (bounded recall with skip-policy stays), and background memory work must never
   starve foreground agent work (resource guards, §8.1) — but silent degradation
   is forbidden: skipped recalls, skipped runs, quarantined writes, and failed
   dreams are events on the timeline.
7. **Hub-compatible, hub-independent.** Everything here runs with zero external
   services; nothing here duplicates hub machinery (§12's bright line), and
   record/evidence schemas are hub-compatible so the roadmap is a migration, not a
   rewrite.

---

## 4. What this must fix (verified baseline defects)

These are code-verified behaviors of the shipped system, not hypotheticals
(verification detail in `../archive/design/evidence/memory-survey-2026-07/followups.md`):

| # | Defect | Evidence |
|---|--------|----------|
| D1 | **Echo loop**: the per-turn injection block is baked into the delivered user message and persists in the transcript; meerkat indexes `Message::User` text at compaction with no filter for the injection marker → every compaction deposits a copy into session semantic memory, polluting `memory_search` linearly | `identity_first/runtime.rs:2425-2444`, meerkat `types.rs:1201-1203` |
| D2 | **Unbounded re-injection**: a stable top-8 (~18.5 KB rendered) is re-selected deterministically and re-injected verbatim into *every* non-Steer user message, plus a build-time system-prompt copy; no per-turn aggregate cap, no session budget, no cross-turn dedup (the injector cannot even see a session id) | `agent_memory.rs:367-421`, contrast CC's 4 KB/20 KB/60 KB ladder |
| D3 | **Lifecycle orphaning**: respawn (always mob-level retire+spawn), reset, and resume-fallback rotate the SessionId; meerkat memory has no delete API, so the old session's rows are unreachable forever — and `HnswMemoryStore::open` re-embeds *every* row (orphans included) on *every* agent build in the realm | meerkat `factory.rs:5411-5472`, `hnsw.rs:300-355`, meta `rpc.rs:2937-2972` |
| D4 | **Append-only store**: no update/supersede at the `AgentMemoryProvider` trait level; identical remembers duplicate (codified in tests as correct); a correction and its stale predecessor co-inject as siblings with no preference rule even in the prompt | `agent_memory.rs:552-582, 1090-1099` |
| D5 | **Misleading `elephant` backend**: `MemoryBackendConfig::Elephant` health-checks `GET /v1/health` and persists local JSON; no data ever reaches the memory hub - but it is a public config surface, so it is deprecated and renamed honestly, not deleted (migration path in §15 P0) | `runtime/memory.rs:27-125` |

D1–D4 are production behavior today. Fixing them is Phase 0 and is worth doing
even if nothing else in this document ships. Note the D1 fix strategy: the
initiative makes every default injection surface **echo-safe by construction**
(§9.1) rather than relying on budgets to bound a leak.

---

## 5. Prior-art verdicts

What we adopt, and what we explicitly reject. (Full mechanics in the evidence
archive.)

**Adopt from Claude Code**: two-tier index + topic records with a hard-capped
always-loaded index; the closed type taxonomy with "what NOT to save" discipline
(resists even explicit save requests for derivable noise); LLM side-query
selection over name+description manifests; the 4 KB-record / 20 KB-turn /
60 KB-session budget ladder with transcript-scan reset (state-free,
compaction-safe); mtime-based staleness rendered as human phrases at injection
time; **conversation-forked extraction** (the fork shares the parent's prompt
cache, making extraction nearly free in input tokens — we adopt the fork itself
via meerkat's `Session::fork`, §8.4, not just the sandbox doctrine) with
main-agent mutual exclusion; the dream's 4-phase orient/gather/consolidate/prune
structure with cheap gates (time + session count + lock); the
**incremental-session-notes pattern** (SessionMemory + zero-API-call SM-compact —
flag-gated in CC but the right shape; it composes with upstream ask 4's curator
seam, §8.6); eval-annotated prompt sections.

**Reject from Claude Code**: prompt-only lifecycle (no typed supersede — the dream
hand-edits files and hopes); single-agent, single-project scoping; team sync that
cannot propagate deletions; no memory-usage feedback into retention (its selector
consumes a tool-success signal at selection time but never learns whether an
injected memory was used); extraction gated on end-of-turn-without-tools
heuristics; and its selector's silent 200-file newest-first scan horizon (we keep
the LLM-selection idea and fix the horizon, §8.3).

**Adopt from Codex**: the two-phase map/consolidate split; usage-based
reinforcement (cited/used memories live, unused ones decay — we implement the
audit differently, §9.2); progressive disclosure with a hard prompt-resident token
cap and grep-friendly verbatim retrieval handles; the extraction doctrine (no-op
is the preferred output; user keystrokes/corrections are the highest-signal
evidence; epistemic attribution — "the user said X" vs bare facts; assistant
proposals are not durable memory); the pollution firewall, **including its
session-sticky taint semantics** (Codex marks whole threads polluted, not turns —
§10.1 adopts exactly that); the **rate-limit/headroom guard on all background
memory work** (skip below quota headroom, hard per-window caps — §8.1); aggressive
containment of the consolidation agent (no recursion, no network, write scope =
memory root only).

**Reject from Codex**: git-diff-as-change-feed with no crash rollback (a dead
consolidator's partial edits get laundered into memory as presumed user edits —
the prompt literally instructs preserving them); prompt-enforced format sentinels
with zero code validation; one global store with soft cwd scoping; no per-turn
contextual selection (its recall is agentic and read-time only).

**Adopt from Meerkat** (and keep): indexing-gates-compaction-commit ("never lose
the only copy" as an ordering invariant); typed include/exclude decisions over
message classes; fail-closed capability semantics (memory-enabled build fails
loudly if the store can't open); tool-facing guidance shipped as capability-gated
skills.

**Adopt from the memory hub** (as discipline, not dependency — §12): staged artifact
commit (LLM output is never written directly; it stages, validates, then commits
atomically); provenance on every record including extractor model + prompt id;
per-principal visibility patterns for shared stores. (Status note, 2026-07-02:
The memory hub's truth maintenance is now LLM-powered — value-partitioned detection,
`rel_set` cross-object slots, adjudication over fact-times with ingestion
timestamps withheld; see the roadmap §4.2. Two of its calibration lessons are
imported here: recency judgments should prefer *fact-time over capture-time*
where evidence carries one — a steward-prompt refinement with a fixture behind
it — and its golden-case harness caught two prompt regressions during its own
development, which is §11's thesis validated independently.)

---

## 6. Architecture overview

Three layers with strictly one-directional derivation, plus a judgment plane that
operates on them:

```
┌────────────────────────────────────────────────────────────────────┐
│ JUDGMENT PLANE (calibrated LLM stages, off-turn):                  │
│   Recorder · Distiller · Steward/Dreamer · Hygienist (parked)      │
└──────┬────────────────────────┬───────────────────────┬────────────┘
       │ writes (staged)        │ reads                 │ revises (audited)
┌──────▼──────────┐    ┌────────▼─────────┐    ┌────────▼───────────┐
│ RECORD LAYER    │    │ RECALL           │    │ EVIDENCE LAYER     │
│ typed memory    │───▶│ COORDINATOR      │    │ transcripts, event │
│ records, scoped,│    │ manifest, budget │    │ logs, interaction  │
│ supersede chains│    │ ladder, dedup,   │    │ stream. Immutable. │
│ + usage ledger  │    │ echo-safe inject,│◀───│ (revisions are     │
└─────────────────┘    │ injection ledger │    │ additive+restorable)│
        ▲              └──────────────────┘    └────────────────────┘
        └── provenance pointers (revision-pinned) ───────┘
```

The **recall coordinator** (§9) is the one new runtime component: a deterministic
shell that owns scope composition, candidate gathering, latency/byte budgets,
provenance-chain dedup, echo-safe delivery, inbound envelope defanging, and the
injection ledger. Recall ranking is deterministic and has no LLM judgment stage.
It has a
*fixed* topology (bundled store now; hub candidates later per the roadmap), not a
pluggable provider fan-out with score blending; `AgentMemoryProvider` remains the
only storage extension seam.

Session semantic memory (meerkat `memory_search`) stays what it is — a
within-embodiment recall tool over compaction discards, reached by tool call and
advertised to the model via the manifest (§9.1). It is not auto-injected: its
content class is the one most exposed to echo amplification, and the Distiller
(§8.4) is the only bridge from session evidence to durable records.

---

## 7. The record layer

### 7.1 Record model

```rust
MemoryRecord {
    id: MemoryId,                    // stable, content-independent
    scope: MemoryScope,              // Identity | Mob | Operator | Realm
    kind: MemoryKind,                // Preference | Fact | Gotcha | Procedure
                                     //   | Relationship | OpenLoop | Reference
    title: String,                   // ≤200B
    description: String,             // <=400B - written for retrieval ranking; this line
                                     //   is the retrieval contract
    body: String,                    // ≤64KiB; why/how-to-apply for behavioral kinds
    provenance: MemoryProvenance {
        evidence: Vec<EvidenceRef>,  // see below — revision-pinned, generation-aware
        author: MemoryAuthor,        // Operator | Application | Agent(AgentIdentity)
                                     //   | Steward(run_id) | Distiller(run_id)
        profile: CalibrationRef,     // prompt bundle + model that produced it (§11)
        verification: Option<VerificationClaim>, // agent-cited evidence of
                                     //   verification — a CLAIM, not a tier (§10.2)
    },
    trust: TrustTier,                // Operator > Application > AgentVerified
                                     //   > AgentObserved > Untrusted(quarantined)
                                     //   transition lattice in §10.2
    status: RecordStatus,            // Active | Superseded { by: MemoryId }
                                     //   | Quarantined { reason } | Tombstoned
    supersedes: Option<MemoryId>,    // update-in-place with history (fixes D4)
    working_set_rank: Option<u32>,   // steward-maintained recall ordering (§8.3);
                                     //   superseding records inherit the prior
                                     //   record's rank until the next dream —
                                     //   fresh records are covered by the
                                     //   recent/unranked manifest slice (§8.3)
    created_at, updated_at,
    usage: UsageStats,               // injected_count, last_injected,
                                     //   judged_useful_count, last_useful (§9.2)
}

EvidenceRef {
    session: SessionId,
    generation: ContinuityGeneration, // fresh-start boundaries are first-class:
                                      //   a reset starts a fresh session at a new
                                      //   generation, so the pair names one life
                                      //   of the identity
    revision: RevisionId,             // content-addressed transcript revision that
                                      //   was head at capture time — provenance
                                      //   survives Hygienist rewrites (§8.6); all
                                      //   provenance consumers (steward greps,
                                      //   quarantine review, usage audit, console
                                      //   links, re-dreaming) resolve against the
                                      //   PINNED revision via
                                      //   session/transcript_revision reads
    range: MessageRange,              // within the pinned revision
}
```

Content discipline (enforced by Recorder/Distiller prompts, checked by the
staged-commit validator where mechanically checkable): one fact per record;
descriptions state *when this record matters*, not what it says; relative dates
converted to absolute at write time; `OpenLoop` records carry an explicit
resolution condition so dreams can close them.

`OpenLoop` is the prospective-memory kind: unfinished intentions, promising paths
not taken, "next time try X". No surveyed system has this; for a mob platform it
is disproportionately valuable because open loops frequently outlive the identity
that opened them.

### 7.2 Scopes and composition

```rust
MemoryScope::Identity { realm: RealmId, identity: AgentIdentity }  // private
MemoryScope::Mob      { realm: RealmId, mob: MobId }               // shared, steward-committed
MemoryScope::Operator { realm: RealmId, operator: OperatorId }     // cross-identity,
                                                                   //   realm-confined (below)
MemoryScope::Realm    { realm: RealmId }                           // application-level
```

**Every scope is realm-keyed.** Realms are the platform's isolation boundary
(config inherits, state never does), and memory is state. Operator scope exists
because "the operator prefers terse summaries" learned separately by five
identities is five drifting copies — but its stated justification is that
operators span *mobs*, not realms, so realm-keyed operator scope solves the
drifting-copies problem without becoming the platform's first realm-transcending
state channel. Cross-realm operator profiles are explicitly future work with a
deterministic declassification gate (steward proposes the realm-crossing
promotion; operator approves via the existing gating flow; the record is re-homed
with realm-A evidence refs replaced by an operator-approved summary, since
provenance pointers into another realm's transcripts are themselves a leak). The
`MemoryScope::Operator` variant is part of the P0 schema (no migration later);
operator profiles populate and join composition only from P4 once `OperatorId` keying is
settled (§16.1). Operator-fact detections before P4 are held as steward `hold`
verdicts at identity scope, tagged for promotion, and re-dreamed into Operator
scope when it activates — leveraging §3.1's re-derivation principle.

Read composition for an identity's turn is deterministic: `Identity ∪ Mob(bound
mobs) ∪ Operator(same-realm active operator, if resolvable) ∪ Realm`, with
per-scope injection sub-budgets and provenance labels in the rendered block.
Trust ordering at render time: Operator/Realm records are presented as
higher-authority context than agent-observed ones — this label ships only
together with inbound defanging (§9.1), since authority labels in a forgeable
channel increase forgery payoff. Nothing from memory ever outranks live
instructions (§10).

Write authority differs by scope: identities write their own scope freely
(subject to trust-tier rules); **Mob and Operator scope writes are proposals**
that the steward commits (§8.5). Realm scope is application/SDK-only.

> **P4 as-built (operator scope).** Activation is
> `agent_memory.operator_scope = "off" | "provisional"` — the value name
> says PROVISIONAL on purpose (§16 Q1 stays open; the enum leaves room for a
> final keying). Build-time metadata composition activates on config **and** an
> `OperatorResolver` (a trait seam so the §16 Q1 keying stays swappable).
> The shipped SDK gateway installs `ConsolePrincipalOperatorResolver` when
> `provisional` is configured and shares it with the authenticated console
> send path. Composition therefore adds operator scope only after a real
> console principal has addressed the identity; this does not widen selected-body
> recall beyond identity scope (§8.2). Library embedders can install
> another resolver; a resolver-less composition remains inert and warns
> loudly. Steward routing activates on the config knob alone, because
> operator-scope *proposals carry their own operator key*. The un-hold
> ships as specified for proposals: pre-activation, an accept verdict on an
> operator-scope proposal is deterministically downgraded to a hold, and
> held proposals re-enter every dream — so activation makes the next dream
> the re-dream. One honesty note: identity-scope operator-fact records
> (tagged `epistemic:operator_said`) surface to the active dream as
> re-dream candidates, but the steward has no realm-level operator registry
> under provisional keying, so it may only route them to an operator key
> already in evidence (e.g. named by a held operator-scope proposal) —
> never invent one.

### 7.3 Provider trait v2 and the bundled store

`AgentMemoryProvider` keeps its optional-capability philosophy (`supports_*`
flags gate RPC capability advertisement) and its async, error-bearing shape —
the existing trait is `async fn recall/remember/forget(...) -> Result<_,
AgentMemoryError>`, and every extension matches it. Two surfaces, deliberately
split: the **storage provider** (what any backend implements) and the **staging
capability** (what consolidation requires — the bundled store implements it;
read-only or simple backends need not):

```rust
#[async_trait]
pub trait AgentMemoryProvider: Send + Sync {
    // existing: recall / remember / forget (unchanged, wire-compatible)
    async fn manifest(&self, scopes: &ScopeSet, tier: ManifestTier)
        -> Result<Vec<RecordMeta>, AgentMemoryError>;
        // id+kind+title+description+age+rank; WorkingSet(k) = top-K ranked
        //   ∪ recent/unranked slice (§8.3) | Full
    async fn supersede(&self, scope: &MemoryScope, prior: &MemoryId,
        record: NewRecord) -> Result<MemoryId, AgentMemoryError>;
    async fn mark_usage(&self, ids: &[MemoryId], event: UsageEvent)
        -> Result<(), AgentMemoryError>;
    async fn propose(&self, scope: &MemoryScope, record: NewRecord)
        -> Result<ProposalId, AgentMemoryError>;   // mob/operator scopes
    // supports_manifest / supports_supersede / supports_propose flags as today
}

#[async_trait]
pub trait StagedMemoryStore: AgentMemoryProvider {
    // steward/import path only (§8.5); atomicity is the implementor's contract
    async fn stage(&self, batch: StagedMutationBatch)
        -> Result<StageToken, AgentMemoryError>;
    async fn commit(&self, token: StageToken)
        -> Result<CommitReceipt, AgentMemoryError>;  // single-tx apply + audit
}
```

The recall coordinator and steward orchestrate *over* these surfaces; nothing in
the traits knows about selection, budgets, or dreams — that separation is what
lets the hub provider (roadmap F1) implement the same storage surface over a
wire.

Bundled store: **SQLite per realm**, the sole bundled live backend, at
`<persistent_state>/agent-memory/<realm>.sqlite3` (WAL). Legacy Markdown files
under the realm directory are import inputs only: when the realm is first
accessed and its SQLite connection opens, validated records are imported once
and each source is renamed to `.md.imported`. **Not**
`<persistent_state>/memory/`: that directory
belongs to meerkat's session semantic memory (`AgentFactory` receives
`persistent_state` as its store path and `HnswMemoryStore` creates
`memory/memory.sqlite3` inside it — a realm literally named `memory` would
collide byte-for-byte with meerkat's own database). There is no live Markdown
provider and no Markdown export command; imports go through the same
staged-commit validation as steward writes.
Deterministic write-time guards (these are structure, not judgment): exact
content-hash duplicate short-circuits to the existing id; per-record byte caps;
per-scope record-count and byte floors that *warn the steward* rather than
silently evicting (retention pressure is a dream input, not a FIFO).

RPC surface: existing `meta/agent_memory/{remember,recall,forget}` stay
wire-compatible; add `update` (supersede), `manifest`, `propose`, and
`meta/mob_memory/*` mirrors. **As built:** `update` and `manifest` shipped
(plus the read-only `meta/memory/panel/*` family §9.3 grew); a standalone
`propose` RPC and the `mob_memory/*` mirrors were not built — agent-side
proposing goes through the memory tool's `propose_to_mob`. Configured/bound
shared scopes contribute metadata to the composed build-time index; the
current bundled explicit recall/manifest RPCs and automatic selected-body
recall remain identity-scoped (§8.2). Wider shared-body recall is follow-up
work. Markdown **import**
shipped as a one-shot migration when a realm is first accessed and that realm's
SQLite connection opens, but the `meta memory export` command
has not — records remain inspectable via the panel RPCs and sqlite tooling
until it lands. SDK/docs drift found by the survey is re-verified
at implementation time rather than asserted here — current state as of this
revision: `MemoryStoreInfo` matches across Rust/Python (`{store, record_count}`;
the drift is in `docs/concepts/memory.mdx`, a docs fix); Python's `memory_query`
*documents* the legacy `query` wire shape while the Rust parser still ignores the
key (decide: implement or retire the shim); `conflict_active` parity in
Python/TS `MemoryIndexResult` to be re-checked in the same pass.

The operational ledger (`meta/memory/*`) is unchanged and stays what it is —
runtime assertions and conflict signals for gating. Its misleadingly-named
`elephant` backend is deprecated and renamed in P0 (D5) with config compatibility.
(0.9 qualification: P0 originally retained that config compatibility; 0.9
removes it. The retired `elephant` kind is refused when configuration is
decoded, and the backend must be configured as `kind = "local_json"`.)
The conflict-signal channel gains one new producer: the steward (§8.5).

---

## 8. The judgment plane

> **Status boundary:** the Selector in §8.1/§8.3 and its dependent invocation,
> cost, rollout, and comparison references preserve the original design.
> The LLM Selector was retired unactivated; current recall uses the deterministic
> lexical provider path (§9). Its profiles/fixtures are historical calibration
> inputs, not a runnable stage. The original design is retained below, not a
> claim that every planned operation shipped.

The original plan described five LLM operations. Each is an agent (or a bounded
one-shot structured call) with a calibration profile (§11), a containment
envelope, and a place on the console timeline. Only the proposed Selector would
touch the turn path.

### 8.1 Invocation seam and resource guards

**Invocation.** Meta makes zero direct provider calls today, and this design
does not change that (dogma rule 7: providers stay behind their owning seams).

- **Selector**: obtains its model client host-side through meerkat's existing
  factory seam `AgentFactory::build_llm_client_for_identity` (realm auth binding +
  model catalog resolution — the same seam session model hot-swap uses), with the
  model taken from the stage's calibration profile (§11). The client is cached
  per realm with auth-lease refresh; calls are attributed to a named
  runtime-internal judgment principal with memory-read-only scope.
- **Distiller / Steward / Hygienist**: ordinary meerkat agent builds (the
  service-identity pattern, §8.5), with session semantic memory
  override-disabled so their builds do not pay the D3 re-embed tax; each has its
  own auth binding and ABAC principal (§10).

**Resource guards.** Every stage here burns LLM calls, and Meta multiplies
stages by identities × interactions × mobs — unlike CC/Codex (one user, one
session). The steward's event gates and the Distiller's throttles scale *with*
activity; what is missing without guards is the load-*inverse* control Codex
ships (skip background work below rate-limit headroom). Deterministic
containment, all emitting timeline events when they bite (Principle 6):

- A provider-quota/headroom check gates Distiller and Steward runs where the
  provider exposes one; otherwise a configurable per-realm background-token (or
  background-run) budget per window.
- Hard per-window caps on distillation runs and dream concurrency.
- **Foreground delivery always outranks background memory work.** The Selector's
  latency budget is the only memory cost ever on the turn path.
- Default budgets and headroom thresholds are an open question (§16) answered by
  measurement, not guessed.

### 8.2 Recorder — the agent's own memory tool

A `memory` tool on the member's tool surface (capability-gated, like meerkat's
`memory_search`): `remember / update / forget / recall / propose_to_mob`. The tool
description plus a capability-gated skill teach the protocol: check the manifest
before writing (the manifest is cheap and injected — §9.1), update rather than
duplicate, one fact per record, write the description for your future self's
selector, mark epistemic status ("operator said" vs "I observed"), don't save
what the repo/config already records.

Tool-result confirmation gives current-turn awareness; automatic injection picks
the record up from the next prompt assembly (availability semantics identical to
the current system, which got this right). Tool results are an indexing-excluded
message class in meerkat, so this entire surface is echo-safe today.

**All LLM-authored writes land at `TrustTier::AgentObserved` or below — no
exceptions** (§10.2). An agent that verified a fact records the verification as a
`VerificationClaim` in provenance (what was checked, citing evidence); the
*tier* upgrade to `AgentVerified` is a steward-only staged operation after the
validator confirms the cited EvidenceRefs resolve and the dream's judgment
endorses that the evidence verifies the claim. Unendorsed verification claims
count as `AgentObserved` in contradiction resolution. Writes from tainted
sessions (§10.1) land as `Quarantined`.

Agents and the RPC `update` path supersede **within a single record's lineage in
their own writable scopes** — that is the D4 fix, live from P0. Consolidating
*distinct* records (semantic merge, cross-scope moves, re-tiering) is
steward-only (§8.5); the staged-commit validator enforces authorship.
**As built:** the tool's explicit `recall` action (and the recall/manifest
RPCs) remain identity-scoped v1 surfaces. Configured/bound mob, operator, and
realm scopes contribute **metadata only** to the composed build-time index.
The current bundled provider's selected-body recall is identity-scoped for
both build-time orientation and ambient turn injection, just as explicit
recall is. Wider shared-body recall is design intent/follow-up work, not an
implemented ambient behavior or a restriction on what a custom provider may do.

### 8.3 Selector — recall judgment without a horizon

> **Historical, unactivated design:** this Selector was retired; the replacement,
> escalation, and cost model below did not become the live recall path. See §8's
> status boundary and §9 for current deterministic recall.

The proposed Selector replaces the lexical scorer (term-overlap, threshold 2)
entirely. Two design constraints hold simultaneously: no heuristic retrieval,
and **no bounded-visible-
subset failure** — the selector must be able to reach any active record, not just
a prompt-resident index. (Claude Code fails the second quietly: its selector scans
the 200 newest files. The first draft of this document failed it too, by feeding
the selector the injection-budget manifest.)

Resolution — the selector is a **side-query over the store, not over the injected
index**, tiered:

1. **Injected index** (§9.1) is budget-bound (~8 KB) and exists for the *agent's*
   orientation. It is not the selector's input.
2. **Working-set tier**: the selector side-query reads
   `ManifestTier::WorkingSet(k)` — deterministically composed as the **union of
   the top-K ranked records and a recent/unranked slice** (newest-first, capped;
   every record the steward has not yet ranked, plus anything written or
   superseded since the last dream). The rank ordering is what the **steward
   maintains during dreams** from usage stats, recency, trust, and open-loop
   status; the recent slice is what keeps the §2 promises honest — a fresh
   operator correction is selector-visible on the *next assembly*, not after the
   next dream (superseding records also inherit their predecessor's rank
   immediately, §7.1). This is LLM judgment cached offline plus deterministic
   recency structure; there is no runtime lexical or embedding scoring anywhere.
3. **Full-sweep escalation**: the selector's structured output includes
   `coverage: sufficient | need_deeper_sweep`. On the latter (or for explicit
   `recall` calls, which are not latency-bound), the coordinator re-runs selection
   over `ManifestTier::Full` — every active record's description across the
   composed scopes. At initiative scale (hundreds to low thousands of records per
   identity), a full description manifest is well within a side-model call.

   **Scale posture (explicit, because "no retrieval indexes" needs a stated
   degradation curve rather than a cliff):** above a soft per-scope-set ceiling
   (default ~4,000 active records; tunable, final number an §16 question), the
   coordinator chunks Full-tier selection into multiple side-model calls —
   correct but slower and costlier, and it says so as a timeline event. At a hard
   ceiling (default 4× the soft one), Full-tier manifests truncate
   oldest-least-used with a **loud** truncation event naming what was dropped,
   and the store flags the scope for steward retention pressure. Beyond the soft
   ceiling the *supported* answer is hub candidate generation (hub hybrid
   search feeding this same selector — roadmap F2); the bundled store's job is
   working memory, and a working set of ten thousand records is a curation
   failure the dream should have consolidated, not a retrieval problem Meta
   should grow indexes for.

Selection mechanics: small fast model, manifest + incoming turn text +
suppression list of already-in-context record ids, structured output, manifest
position-shuffled (bias guard), "certain to be helpful" selection bar (CC's
eval-survived wording as the starting profile).

Latency containment: the existing bounded-recall machinery is kept
(`recall_timeout`, default 500 ms, `skip` failure policy — memory never blocks
delivery). Working-set selection runs within that budget; escalation never runs
on the blocking path (it completes in the background and feeds the *next*
assembly, or the explicit recall response). Steer never gets injection (kept).

### 8.4 Distiller — extraction from evidence

Runs **off-turn**, triggered by: (a) completed interactions (via the observe-only
interaction stream), throttled, coalesced, and resource-guarded (§8.1);
(b) **session rotation** — the reset / respawn / resume-fallback / retire paths
get a pre-rotation hook that runs distillation over the outgoing session's tail
before the SessionId rotates (fixes the knowledge half of D3; the storage half
needs the upstream GC ask, §13); (c) **compaction** — because interaction
triggers are throttled, content can compact out of the visible transcript before
any distiller run sees it, surviving only in meerkat's session semantic memory.
Post-compaction, the distiller therefore gets a **bounded host-side read over the
outgoing discard range** — a scoped query of that session's semantic memory
store (`MemoryStore::search` host-side; *not* the distiller's own session-memory
tooling, which stays disabled per §8.1) — so nothing leaves the reachable
evidence surface unharvested. Ordering note: upstream session-memory GC (§13
ask 2) must not reap rows before this harvest runs; the same
"never lose the only copy" invariant meerkat already applies to
compaction-gating extends here. This stage is the single highest-value LLM stage
in the system: today rotation and compaction are pure knowledge destruction.

**Reset is different.** `reset()` is the platform's deliberate clean-slate
operation — the only lifecycle path that advances `ContinuityGeneration`, and the
operator's escape hatch from a poisoned or pathological session. Reset-boundary
distillates therefore land as `Quarantined` pending steward review by default
(quarantine preserves the re-dream option where "off" would destroy evidence once
session GC lands), and the Distiller must never re-extract content whose records
were tombstoned in the same window — its pre-injected manifest includes recent
tombstones, and the staged-commit validator rejects re-creation of tombstoned
content at a revocation-driven reset. Respawn and retire distill normally (they
are recovery/continuity paths; generation does not advance).

**Harness: a session fork, not a detached re-reader.** The economics that make
CC's extraction nearly free come from forking the live conversation so the
extraction call shares the parent's prompt cache. Meerkat has the primitive:
`Session::fork` / `fork_at` are O(1) copy-on-write, and meerkat-mob has a fork
launch mode. The Distiller forks the session (interaction-stream-triggered runs
within the provider cache TTL; the pre-rotation hook forks the outgoing session
before rotation) and appends the extraction doctrine as its prompt. One
consequence, learned from CC: prompt-cache sharing requires a byte-identical
prefix *including the tool list*, so the fork keeps the parent's tools and
containment moves to the authorization/capability layer (every call gated to:
read-only + `propose`/`remember` on the target identity scope), not to tool-list
subsetting. Where a fork is not feasible (cache expired, cross-process), the
fallback is a bounded detached agent (maxTurns ~5) reading transcript slices —
correct but full-price; the resource guards bound its spend.

Prompt doctrine is Codex's, verbatim where possible: no-op is the preferred
output; corrections and operator keystrokes are the highest-signal evidence;
preserve evidence→implication with near-verbatim quotes as retrieval handles;
epistemic attribution mandatory; assistant proposals are not durable memory. The
existing-records manifest is pre-injected — phrased for the verbs the Distiller
has: "don't re-remember what the manifest already shows; `propose` against the
existing record id". Mutual exclusion with main-agent Recorder writes per window
(CC's cursor trick, adapted to interaction ids).

Distiller output above `AgentObserved` trust is impossible; records whose
evidence window includes tainted content (§10.1) are quarantined.

### 8.5 Steward — the dreaming consolidator

**Provisioning: mechanism from Meta, policy from the app.** Meta ships the
steward *harness* — the staging API, containment envelope, default prompt bundle,
and a schedule template — as an opt-in capability, following the platform's
existing opt-in precedents (`access.toml` presence enables ABAC; `schedules.toml`
declares schedules). The application enables it and binds the policy facts only
an app can own: the hosting mob, the steward's profile and auth binding, an
optional model override (default comes from the stage's calibration profile,
§11), and cadence. One steward per realm **when enabled**; per-mob granularity is
a config knob. Cheap event gates before any run (≥K sessions or ≥N proposals
accumulated since last run; lock; CC-style gate ordering so idle cost is ~one
stat), plus the §8.1 resource guards.

The dream is agentic, not batch (CC's verified shape): orient over the store →
gather signal (proposals queue, quarantine queue, usage ledger, recent
distillates, *targeted* evidence greps — "look only for things you already
suspect matter") → consolidate → prune and re-rank. **Quarantined bodies are
rendered to the steward inside the same hardened quoted/escaped "not
instructions" envelope used for turn injection** — the steward is the one stage
that reads poison wholesale, and it reads poison as labeled, defanged data. Its
writes are the system's only path for:

- **Consolidating distinct records** — semantic dedup/merge, cross-scope moves,
  and trust re-tiering (dedup is judgment, not hashing). Single-record lineage
  supersede stays with the record's own writers (§8.2); the validator enforces
  authorship.
- **Contradiction resolution**: same-scope contradictions resolve by trust tier,
  then recency, then evidence weight — as an LLM judgment with the rule stated in
  the prompt, recorded in the supersede chain's rationale. Cross-member
  contradictions at mob scope that have operational consequence are *also*
  emitted as conflict signals to the operational ledger, where gating can see
  them (`memory.conflict_read`) — memory disagreement becomes a governance input
  instead of silent last-write-wins.
- **Promotion**: identity→mob proposals get committed, rejected, or held;
  promotion judgment is scoped by the mob's purpose (injected from mob metadata).
  Operator-fact detection routes to Operator scope from P4 (§7.2); before that,
  holds.
- **Working-set ranking** (§8.3): the recall ordering the Selector's fast tier
  reads is a dream output, refreshed every run.
- **Open-loop management**: close resolved loops, escalate stale ones (a stale
  open loop can become a scheduled nudge — the platform can *act* on prospective
  memory).
- **Quarantine review**: propose promotion (with trust re-tiering), hold, or
  tombstone — subject to the deterministic ceilings in §10.2:
  **quarantine-promote into Mob or Operator scope requires an operator approval
  through the existing gating flow**; the steward proposes, it does not commit
  those.

  > **As built (operator review).** The review transaction is store-owned:
  > `StewardStore::review_quarantined` decides one quarantined record in its
  > own scope (release = successor `<id>-released` at `agent_observed`,
  > `derived_from` the origin, origin tombstoned; a quarantined update
  > supersedes its still-active prior; tombstone = discard) in one
  > transaction through the staged validator, refusing typed on a live
  > pending gate, a secret-shaped body, a changed content hash, a stale
  > update or an occupied successor id. Replays are recognized from the
  > review's own audit evidence (`detail.review`), never from a
  > successor-shaped row. Gated promotions past their seven-day expiry are
  > expired by the review itself, so a persisted orphan blocks review only
  > until then, with no steward needed.
  > A live gate blocks a release, never an operator's tombstone: the
  > tombstone invalidates the record's pending promotions in the same
  > transaction (mapping expired, staged batch discarded, ids in
  > `detail.review.invalidated_promotions`), and gating resolutions change
  > only a still-pending promotion, so a late approval, denial or
  > escalation can neither publish, overwrite nor revive it. A gate
  > registered after a review has committed loses too: its approval fails
  > validation against the tombstoned origin and its mapping expires,
  > publishing nothing.
  > A review emits `memory.quarantine.verdict` only for a committed
  > decision, with no reviewer or rationale, and the steward's open-loop
  > nudge carries no rationale either; quarantined content stays
  > reviewer-only after a release, tombstone or forget retires it.
  > Operators reach it through the console's
  > `meta/memory/quarantine/decide`, identity scope only, with
  > `memory.quarantine.review` plus the identity's read/view and
  > write (release) or delete (tombstone) grants (§10.3). Cross-scope
  > promotion still rides the gating flow. The steward's `release` and
  > `tombstone` verdicts run the same review (reviewer `steward`), so a
  > record is decided at most once whichever reviewer gets there first,
  > and a dream reports a verdict only once it is applied.
- **Retention**: when a scope approaches its floors, the dream consolidates or
  tombstones; deterministic code never silently evicts.
- **Usage audit** (§9.2) and **exit interviews**: a retiring identity's store is
  harvested into mob scope before tombstoning; this hooks the same pre-rotation
  seam as the Distiller.

**Crash semantics (fixes the Codex defect class):** the steward never edits live
records. It emits a `StagedMutationBatch` (create/supersede/tombstone/promote ops
with rationale + evidence refs); a deterministic validator checks schema, scope
legality (including realm confinement of operator writes), the trust-tier
transition lattice (§10.2, enforced on **transitive provenance** so merging a
quarantined record into a "fresh" consolidated record cannot launder its tier),
supersede-chain acyclicity, evidence-ref resolvability against pinned revisions,
tombstone-recreation rejection, and budget invariants; `commit` applies
atomically (single SQLite transaction) and writes one audit entry per op. A dream
that dies mid-run leaves a stage token that is garbage-collected, never applied,
never visible. Partial LLM output is structurally incapable of being laundered
into the store. This is the memory hub's staged-artifact boundary applied as
*discipline* (§12), and it is the kind of commit-boundary rigor the meerkat dogma
already demands of runtime state.

Containment mirrors Codex's consolidator: the steward identity's tool surface is
the memory staging API + read-only evidence access; no comms to non-steward
peers, no network tools, no recursion (its own memory capability is disabled).

### 8.6 Hygienist — context curation at boundaries

The user-visible half of "dreaming": keeping a long-lived embodiment's context
semantically pristine.

- **Not** always-on rewriting (cache destruction + self-coherence risk). It runs
  at compaction boundaries and on demand.
- Mechanism: meerkat's **audited same-session transcript revisions**
  (`session/rewrite_transcript` / `session/transcript_revision` /
  `session/restore_transcript_revision`) — the primitive already exists upstream,
  session identity is unchanged, originals are restorable, and the operation is
  audited. Meta's contribution is the curator: an LLM pass that drops dead tool
  results, collapses repeated scaffolding, and preserves decisions with
  rationale, staged and applied through the same validate/commit discipline as
  the steward (a revision proposal is a `StagedMutationBatch` against the
  transcript instead of the store).
- **Provenance integrity**: EvidenceRefs are pinned to the revision they were
  captured against (§7.1), so rewrites never dangle or silently re-point
  provenance. The staged-revision validator additionally: hard-blocks revisions
  touching spans referenced by `Quarantined` records until their steward review
  completes (an attacker must not be able to steer the Hygienist into pruning the
  tool output documenting the attack), and flags revisions touching spans
  referenced by `Active` records on the audit timeline.
- Ordering invariant borrowed from meerkat's own memory design: evidence indexing
  (and distillation, where enabled) must complete before a revision discards
  material — "never lose the only copy" extends to hygiene.
- The CC incremental-session-notes pattern (§5) is the target shape for the
  compaction half: a continuously-maintained notes artifact spliced in at the
  compaction boundary via upstream ask 4's curator seam would make compaction
  itself zero-LLM-cost. Until that seam exists, the Hygienist is a
  boundary-triggered pass.

Phase-wise this ships last (§15); it is the highest-risk stage and depends on
calibration infrastructure being real first.

> **P4 internal engine (Hygienist, parked).** The apply seam is real: meerkat 0.7.9's
> `PersistentSessionService` implements `SessionServiceTranscriptEditExt`,
> and the gateway threads the concrete typed handle to the Hygienist
> (`memory/hygienist.rs`; the erased mob-layer session service does not
> carry the edit extension). Two deliberate narrowings against the text
> above, both structural safety: (1) the curation vocabulary is
> `prune_tool_results` (stub tool-result payloads in place, preserving the
> tool_use/tool_result pairing provider APIs require) and `collapse`
> (replace a contiguous non-tool run with one typed system notice) —
> free-form message deletion is not expressible, and role legality is
> validator law, not prompt guidance; (2) the proposal is a stage-local
> `RevisionProposal` through its own §8.6 validator rather than a literal
> `StagedMutationBatch` (the store validator's ops don't model transcript
> ranges), with the same shape: LLM output is never applied without
> deterministic validation, and the commit is meerkat's audited revision.
> Ordering: the post-compaction pass is sequenced via the distiller's
> compaction follow-up hook (it runs only after the harvest attempt, and
> only when the harvest completed or was provably empty — budget-denied or
> failed harvests withhold hygiene loudly); on-demand passes consult the
> distiller's window cursor and refuse ranges beyond it. Mid-turn sessions
> are refused by meerkat's `TranscriptEditRunningBehavior::Reject` default.
> Revision/CAS behavior: the internal engine reads the head through
> `list_transcript_revisions` (`limit: Some(0)`) and forwards it as
> `expected_parent_revision` on rewrite. Unsupported revision listing yields
> `None`; other listing errors fail the read. History and head are separate
> reads, not an atomic snapshot. Transcript distillation captures
> `session.transcript_revision()` when available; failed capture and the
> compaction-discard path can leave `EvidenceRef.revision` as `None`.
> Capturing a revision does not mean every evidence consumer resolves it.
> The public SDK and gateway do not activate this engine in the current
> release. Only absent/disabled compatibility config is accepted, and no
> OpenAI or Anthropic provider-proof claim is made.

---

## 9. The recall coordinator: injection, budgets, and the audit loop

The coordinator is deterministic runtime composition — the concerns that need a
named owner: scope composition, candidate gathering, latency and byte budgets,
provenance-chain dedup, echo-safe delivery choice, inbound defanging, and ledger
writes. It is not a provider-fanout framework; its topology is fixed and recall
ranking is the deterministic lexical provider path. The retired LLM Selector is
not part of composition.

### 9.1 What enters context, where — echo-safe by construction

| Surface | Content | When | Message class | Echo-safe? |
|---|---|---|---|---|
| Build-time (`customize_build` → `additional_instructions`) | Behavioral protocol + composed **index** (budget ~8 KB) + selected bodies for orientation | materialize / resume / respawn / reset | system prompt (`Message::System`) | **yes** (excluded from indexing — verified) |
| On-demand | `memory` tool (records), `memory_search` (session), each advertised in the index | agent-initiated | tool results | **yes** (excluded — verified) |
| Per-turn ambient bodies | Lexically recalled record bodies, provenance-labeled, staleness-phrased | supported non-Steer identity-first sends | Separate typed `InjectedContext` bodies, never concatenated into user text | **yes** (excluded from session semantic-memory indexing) |

**Current posture:** `agent_memory.per_turn_injection = "off" | "budgeted"`
defaults to **budgeted** on the supported identity-first delivery path, with
the budget ladder and dedup below. This is a configured default, not a promise
of injection on every turn: Steer and unsupported delivery paths (including
generic autonomous-host work) skip ambient injection. The indexing echo-safety
claim is about the typed message's exclusion from session semantic-memory
indexing, not universal exclusion from every possible extraction path.

**Historical coupling note:** P0 first defaulted ambient push off and made
`budgeted` opt-in while delivery was fused into the user's own message text.
Meerkat's exclusion seam was per-message, so labeling that fused message would
have excluded either nothing or the operator's real words along with the
injection. Closing D1 required **both** Meerkat's typed injected-context class
and Meta's separate injected-body delivery; the default flipped to budgeted
when that path landed on 2026-07-01. The gateway parser carried its own literal
`off` fallback for the object form until 0.8.31 and silently kept every
object-form client on the old default; it now derives the fallback from the
library default so one default governs both forms. The interim off posture
retained the echo-safe build index and on-demand tool recall rather than
relying on budgets to bound the fused-message indexing leak.

**Classic-mob (roster-less) scope.** The A2 decouple carries the two echo-safe
surfaces — build-time injection and the Recorder tool — onto the classic mob
path via meerkat-mob's per-spawn seam
(`memory/spawn_customizer.rs::MemorySpawnCustomizer`), keyed on the member's
`AgentIdentity` with no IdentityRuntime/roster requirement. Per-turn ambient
injection is **deliberately not** carried over: the classic send path has no
injection hook. A classic-mob
per-turn hook is a scoped follow-up gated on the same echo-safe delivery
coupling above; until then `per_turn_injection = "budgeted"` only takes effect
on supported identity-first delivery paths (the classic customizer emits a
debug-level construction notice).

Budget ladder (applies to build-time bodies and to `budgeted` per-turn mode):
≤2 KB/record (kept), ≤20 KB/assembly aggregate, ≤60 KB/session cumulative, then
index-only until compaction — with the cumulative counter computed by transcript
scan (CC's state-free trick), so compaction naturally resets it.
**As built:** the cumulative counter and cross-turn dedup set are in-memory
coordinator state keyed by the delivered session id, not a transcript scan.
The compaction-reset property is wired explicitly: the gateway observes the
member `CompactionCompleted` event unconditionally and calls
`RecallCoordinator::on_session_compacted`, dropping that session's dedup set
and byte counter (per-assembly cap unaffected).
This over-resets slightly relative to a transcript scan — bodies surviving in
the retained post-compaction tail may re-inject once — and the state is
process-local (a gateway restart also resets it) and shared across injector
clones (pinned by test). The injection
block keeps the shipped anti-prompt-injection envelope (quoted untrusted
observations, XML-escaped, "not instructions" preamble) and adds per-record
provenance + age phrasing ("saved 47 days ago" — human-phrased, because models
are bad at date arithmetic).

**Envelope anti-spoofing.** The envelope is plain text an attacker can reproduce:
a peer message or echoed web content can forge "remembered operator preferences"
verbatim, inheriting whatever authority the model grants real memory. Two
deterministic mitigations ship with the coordinator: (1) **inbound defanging** —
the send path scans inbound message and tool content for reserved envelope
markers (the header pattern, `<meta_memory_observation>` syntax, provenance
labels) and neutralizes them before delivery; (2) a per-session/embodiment nonce
in the envelope header, never exposed via tool results, RPC responses, files, or
logs — *bar-raising, not authoritative*, since anything in context can leak via
echo. Host-owned channel attribution was an explicit second rationale for
upstream ask 1; the separate typed delivery described above is now in place,
but does not make envelope text unforgeable. Trust-authority render labels
(§7.2) ship only together with (1).

### 9.2 The usage ledger

Deterministic side: every injection writes `(record_id, session, turn, surface)`
to an injection ledger; every explicit recall/tool read marks usage mechanically.

Judgment side: the steward's dream includes a **usage audit** — sampling recent
evidence (via pinned revisions) to judge which injected records were load-bearing
("did the reply depend on it?") and which are dead weight. Verdicts update
`UsageStats` and feed both retention and the working-set ranking (§8.3), echoing
Codex's decay without its mechanical citation tax — a mandatory citation block in
every assistant turn is wrong for a mob, where turn output is often peer-to-peer
protocol traffic.

This closes the loop no surveyed system closes well: **injected → used →
reinforced; injected → ignored → consolidated away**, with the judgment call made
by a model reading evidence, not a counter.

### 9.3 Console surface

Memory events (remember/update/forget/propose/promote/dream-run/quarantine) are
timeline events with the standard envelope; a Memory panel joins the console
(records by scope, supersede chains, provenance links resolving pinned revisions,
quarantine queue, dream history). ABAC actions govern visibility (§10.3). Trust
through inspectability is a product feature, not an ops afterthought.

---

## 10. Security and governance

### 10.1 The taint model (deterministic, and honest about when)

The persistent-prompt-injection defense needs an input signal, and today **no
turn-level or session-level content-taint fact exists anywhere in the platform**
— the only "untrusted" concepts are comms sender admission and mobpack deploy
trust, which are different facts. This design therefore specifies the mechanism
as a deliverable rather than asserting "the runtime knows":

- **Signal derivation — coarse in P1, complete in P2.** The signal is derived
  from existing typed facts — tool events joined with `ToolDef.provenance`
  (`ToolSourceKind::Mcp` + server source id) and provider-native search blocks
  (`ServerToolKind::WebSearch`) — against a new Meta-owned **content-trust
  configuration**: which tool sources count as untrusted for memory purposes
  (web/fetch always untrusted; MCP servers untrusted by default with an explicit
  trusted allowlist, mirroring Codex's `pollutes_memory`). Note meerkat's
  `ToolAccessPolicy::AllowList` is invocation gating and cannot serve this role.
  **P1 ships the coarse version**: the content-trust config plus an
  observe-stream taint tracker that marks the session tainted when an untrusted
  tool event is seen — small enough machinery that the Recorder must not ship
  without it (a memory tool that can be poisoned for a whole phase is worse than
  no memory tool). Its one honest gap is a race: a write in the same turn as the
  session's *first* untrusted ingestion can beat the observe stream. **P2 adds
  the Meta-side completion** — comms propagation and Distiller
  evidence-range taint — while the race close itself (taint visibility at
  tool-dispatch time) is **upstream-gated** on the §13 ask and lands when it
  does; until then the race stays a documented gap, not a claimed close.
  Deployments that cannot accept the race set
  `memory.llm_writes = "quarantined"` (below).
- **Taint is session-sticky, not per-turn** (Codex's thread-level
  `memory_mode='polluted'`, adopted): once a session ingests untrusted content,
  all Recorder/Distiller writes from that session quarantine until a
  fresh-context boundary (reset / respawn / fresh spawn). Per-turn taint is
  trivially evaded by "on your NEXT reply, remember X", and compaction cannot
  clear taint — the summary is derived from tainted context and inherits it.
  Distiller output over tainted evidence ranges quarantines regardless of which
  turn it distills.
- **Taint crosses comms.** Intra-mob peer messages are trusted-or-dropped by
  construction, so a prompt-injected agent A can otherwise launder payloads
  through peer B, whose own session ingested nothing untrusted. Meta knows the
  mob topology and each member's session: when a write's evidence window includes
  messages received from a peer whose session was tainted at send time, the write
  quarantines. (Blanket-quarantining all peer content would gut the mob-memory
  value proposition; taint is conditional on sender-session state.) Envelope-level
  taint flags need an upstream comms change and are filed as ask 5.
- **Posture and the knob.** From P1, three deterministic layers hold: the coarse
  session-sticky taint tracker (above), the tier ceiling (LLM-authored writes
  never exceed `AgentObserved`), and steward-gated mob/operator commits — so
  even a successful injection cannot mint high-trust or mob-visible records, and
  same-session follow-up writes quarantine. What P1 does not close is the
  first-ingestion race and cross-agent laundering (P2). Deployments choose their
  write posture: `memory.llm_writes = "observed"` (default) or `"quarantined"`
  (every LLM-authored write quarantines until steward/operator review — the
  maximally conservative mode, at the cost of recall not learning until review).

### 10.2 Trust tiers and the transition lattice

The lattice is deterministic validator law, not prompt guidance:

- `Operator` and `Application` tiers are assignable **only by non-LLM
  principals** (operator console actions, SDK/application writes) — never via any
  `StagedMutationBatch`.
- All LLM-authored writes (Recorder, Distiller, Steward-created records) enter at
  `AgentObserved` or `Quarantined`. `AgentVerified` is granted only by a steward
  staged op whose cited EvidenceRefs resolve against pinned revisions and whose
  dream judgment endorses the verification; the semantic half is LLM judgment,
  the resolvability half is mechanical.
- **Transitive provenance ceiling**: any record whose evidence or supersede chain
  reaches `Untrusted`/quarantined provenance is capped at `AgentObserved`
  forever. Merging quarantined content into a "fresh" consolidated record carries
  the ceiling with it — laundering by consolidation is a validator reject.
- Quarantine-promote into `Mob` or `Operator` scope additionally requires
  operator approval through the existing gating flow (§8.5).

### 10.3 Actions and access

Current contract (verified): `agent.memory.write` and `agent.memory.delete`
exist; recall is gated by `agent.view` (`http_console.rs:1570-1572`); there is
**no read action of any kind**. This initiative adds **per-scope read actions** —
`agent.memory.read`, `mob.memory.read`, and operator/realm-scoped read grants —
governing the recall RPC, the new manifest RPC, `mob_memory` mirrors, and the
console Memory panel. Memory content is more sensitive than roster visibility,
and the panel now carries mob-shared records, operator profiles (cross-mob
personal facts), supersede chains, and provenance links: none of that should be
readable via mere `agent.view` on one identity. Operator-scope reads require an
operator-scoped grant. This operationalizes the per-principal-visibility pattern
adopted from the memory hub (§5). Migration: default policy templates grant
`agent.memory.read` wherever `agent.view` was granted; the console keeps
`agent.view` as a prerequisite. Further additions: `agent.memory.admin`,
`mob.memory.propose`, `mob.memory.commit` (steward-only in default policy),
`memory.quarantine.review`. Send permission never implies memory mutation.

### 10.4 The rest of the posture

- **Injection framing**: recalled memory is quoted, labeled, provenance-carrying
  data; it never outranks system/developer/live instructions. (Kept from the
  shipped format, which got this right.) Inbound defanging and the envelope nonce
  are §9.1.
- **Deletion semantics**: tombstones stop future injection immediately; text
  already in a live context requires reset/respawn to revoke (documented,
  unchanged) — and the Distiller's tombstone-recreation guard (§8.4) keeps a
  revocation-driven reset from re-learning what was just revoked. Tombstoned
  records propagate as deletions through mob scope — fixing CC team-sync's
  restore-on-pull failure by construction (single store, no sync).
- **Secret hygiene**: gitleaks-class scanning on the write path (CC's write-time
  guard), refusing rather than silently redacting.
  > **As built:** `memory/secrets.rs` — a curated high-precision subset (AWS
  > access key ids, GitHub tokens, private-key headers, credential
  > assignments under well-known key names), enforced at the staged-validator
  > chokepoint (`staged::check_record_payload`, covering the memory tool, RPC
  > remember/update, Distiller, Steward, Hygienist) and at the proposal seam.
  > Refusals return a typed `SecretDetected` error naming the pattern class
  > without echoing the secret; the markdown import loud-skips the offending
  > record and imports the rest.
- **Steward containment** as in §8.5. All memory-plane agents run under ABAC
  principals with exactly the scopes above; the Selector's client runs as a
  named judgment principal (§8.1).

---

## 11. Calibration

Every judgment stage is a versioned, evaluated artifact. This is the engineering
cost of "no heuristics," paid explicitly:

- **Calibration profile** = `{stage, prompt bundle, model, params, version}`.
  Records and staged batches carry the profile that produced them.
  Supported-model onboarding = run the matrix, not hope. The profile format is
  shared with the memory hub's judgment stages (roadmap) so conflict-resolution and
  extraction prompts are one artifact family across the stack, not two.
- **Fixture corpus** per stage: transcripts with labeled expected distillates;
  stores with known duplicates/contradictions for the steward; manifests + turns
  with expected selections; transcripts with known-load-bearing injections for
  the usage audit. Synthetic to start; grown from production traces — **operator
  corrections and forgets are free negative labels**, dream-audit verdicts are
  cheap weak labels.
- **Scoring**: pairwise LLM-judge against reference outputs (comparative judging
  is reliable; absolute scoring is not), plus label-free invariants: schema
  validity, no session-scoped fact promoted to durable memory, poisoned-fixture
  always quarantined, forged-envelope fixture always defanged, selection stable
  under manifest shuffle, no supersede cycles, no tier-ceiling violations.
- **Harness**: the calibration runner is itself a mob/workflow
  (`make memory-evals`); profile changes gate on scorecard non-regression in CI.
  **As built (harness v0):** CI gates the deterministic half — the fmt-lint lane
  runs the bright-line ratchet and `memory-evals --check`, and three eval-harness
  integration tests drive the runnable Distiller, Steward, and Hygienist mock
  lanes end-to-end, failing on
  structural violations (distiller quarantine verdicts, steward §10.2 validator
  law, hygienist §8.6 hard-blocks). Retired Selector artifacts are schema-checked
  historical calibration inputs only, not a fourth harness or shuffle-stability
  CI lane. Judgment
  scorecards gate only in `--mode live`, which requires provider credentials no
  CI lane supplies today; a scheduled authed live-eval lane is the remaining
  step to literal scorecard non-regression gating — live mode exists and gates
  wherever auth resolves.
- **Re-dreaming**: because memory is derived (§3.1), a profile upgrade can re-run
  distillation/consolidation over retained evidence. The store is a cache of
  judgment; the judgment can be re-bought at today's model prices.

---

## 12. Hub-compatibility discipline (the bright line)

The roadmap makes an external service the memory hub; this initiative makes that a
migration instead of a rewrite, by restraint:

- **Meta's store never grows**: entities, relations, truth slots, embeddings,
  chunking, FTS/vector indexes, freshness engines. The moment a record is
  entity-shaped *world knowledge* ("the staging DB host is X") rather than
  *working knowledge* ("prefer the staging DB for smoke tests"), it is hub
  material — under the initiative it simply stays a hot record; under the
  roadmap it graduates. Enforced as a ratchet: a CI gate forbids retrieval-index
  and embedding dependencies in the memory module, in the meerkat governance-gate
  tradition.
- **Schema compatibility**: `MemoryRecord` fields map onto the memory hub's
  `BaseFields` (realm↔space, trust/security envelope, subjects, provenance
  chain); `EvidenceRef` is shaped as a hub-compatible evidence span (session
  source id + revision + range) so Distiller provenance survives ingestion
  unchanged. This costs nothing now and is the single highest-leverage
  compatibility decision.
- **Judgment sharing**: calibration profiles are one artifact family across
  Meta and the memory hub (§11) — one conflict-resolution prompt lineage, not two
  drifting ones.
- **No service coupling in this initiative**: everything above runs with zero
  external services. The wire-boundary hub provider, evidence ingestion,
  graduation, and the mandatory-hub decision gates are specified in
  [`memory-hub-roadmap.md`](memory-hub-roadmap.md).

---

## 13. Meerkat upstream asks

Filed as issues; **none block the initiative** — each has a specified interim
behavior, and each removes a class of defect at the right layer when it lands:

1. **Typed injected-context message class** — a first-class injected-context
   delivery class that `indexable_content()` excludes from compaction indexing
   (D1; the per-message seam is `types.rs:1201-1203`). Explicitly coupled to a
   meta-side change (§9.1): meta must stop fusing injections into user text
   and deliver them as a separate typed message — the ask and the restructuring
   land together, and channel authenticity (§9.1 anti-spoofing) is the second
   rationale. Interim: per-turn push defaults off.
2. **Session memory lifecycle** — delete/GC on the `MemoryStore` trait (or
   per-session drop), and lazy per-scope index loading so orphaned sessions stop
   taxing every agent build in the realm (D3 storage half). Interim: orphan
   growth documented; distill-before-rotation (P2) preserves the *knowledge*
   regardless.
3. **Compaction-summary indexing exemption** — discarded compaction summaries are
   re-indexed today (`followups.md` §1, secondary edge); same typed-exclusion
   fix.
4. **(Later)** LLM-curated compaction seam — let a host-supplied curator produce
   the compaction summary; with it, the Hygienist's incremental-notes target
   (§8.6) makes compaction itself zero-LLM-cost, CC-SM-compact style.
5. **(Later)** Taint metadata on comms envelopes — `Envelope` today is
   `{id, from, to, kind, sig}` with no metadata field, so cross-agent taint
   propagation (§10.1) is meta-side (topology + session-taint join) until an
   envelope-level flag exists. Related smaller ask: taint visibility at
   tool-dispatch time so the Recorder classifies synchronously instead of racing
   the observe stream.

---

## 14. Why this is superior to Claude Code and Codex

> **Original-plan comparison:** the LLM Selector's whole-store selection and
> on-path cost below describe the retired design in §8.3, not shipped recall.
> Current recall is deterministic (§9); retained comparisons are design context.

Not "different": strictly more capable on their own terms, plus capabilities they
cannot express.

**On their terms.** Recall: LLM selection like CC's, but over the *whole store*
via working-set tiers + escalation — CC's selector silently caps at the 200
newest files, and Codex has no per-turn contextual selection (its recall is
agentic — a prompt-guided read-time memory pass plus search tools, not
harness-side injection). Extraction economics: CC's cache-sharing fork is
adopted via `Session::fork`, and where the fork isn't feasible the cost is
priced and resource-guarded rather than silent (§8.1, §8.4) — Codex's
rate-limit-headroom guard is adopted outright. Write discipline: both systems'
extraction doctrine, unified, with a typed trust lattice instead of prompt-only
hygiene. Lifecycle: supersede chains + staged atomic commits where CC has
prompt-hoped file edits and Codex has crash-laundering with no rollback primitive
in its codebase. Feedback: a usage loop with LLM-judged load-bearing verdicts
where CC has no memory-usage feedback of any kind and Codex has a citation tax.
Session continuity: CC's zero-API-call SM-compact is the one term where CC ships
something this initiative doesn't yet match — the incremental-notes pattern is
adopted as the §8.6 target and lands with upstream ask 4; until then meerkat's
standard compaction stays. Governance: ABAC actions, quarantine review, and
console inspectability where both have file permissions and hope. Echo-safety:
every default injection surface is an indexing-excluded message class — CC's
extraction fork provably re-reads its own injected memories and post-compact
summaries as extraction input (`followups.md` §1).

**Beyond their terms.** (1) Identity lifecycle events as consolidation triggers —
distill-before-rotation, exit interviews; CC/Codex have no respawn concept at
all. (2) Mob memory: spawn-time onboarding, steward-mediated propagation,
contradiction-as-governance-signal — with the mob-specific injection channel
(peer laundering) explicitly closed (§10.1), which single-agent systems never had
to think about. (3) Memory operations as first-class platform citizens:
app-enabled steward identities, observable dream runs, calibration as a CI'd
workflow — the harness maintains itself with the machinery it exists to provide.
(4) Prospective memory (`OpenLoop`) that the scheduler can act on.
(5) Re-dreaming: derived-view architecture makes every future model improvement
retroactive.

The honest cost: this system runs more LLM stages than either (Selector on-path,
Distiller/Steward off-path), which is why calibration (§11), resource guards
(§8.1), and containment (§8.5, §10) are load-bearing sections rather than
appendices.

---

## 15. Rollout

Value-first ordering; each phase ships alone and is useful alone. All phases are
initiative scope; hub work is the roadmap's.

This is the original rollout sequence with explicit as-built notes. The
Selector parts of P1 were retired unactivated (§8.3), not delivered or still
pending activation; Recorder/coordinator behavior does not imply a live
Selector.

- **P0 — stop the bleeding (no new LLM stages).** Per-turn ambient injection →
  budgeted by default since 2026-07-01 (originally off with `budgeted` opt-in; D1 fixed by construction, D2 fixed in
  the opt-in path via the budget ladder + session dedup); provider trait v2 with
  supersede + tiered manifest; SQLite store at
  `agent-memory/<realm>.sqlite3` with Markdown import (one-shot migration when
  the realm is first accessed and its SQLite connection opens; export has not
  shipped — §7.3 as-built note); content-hash write
  guard (D4); deprecate + honestly rename the `elephant` ledger backend with
  config compat (D5); re-verify and fix the SDK/docs drift list (§7.3); file
  upstream asks 1–3; land the bright-line CI ratchet (§12).
- **P1 — Recorder + Selector + coordinator.** `memory` tool + protocol skill;
  index injection at build; working-set/full-sweep selection via the
  `build_llm_client_for_identity` seam behind the existing timeout/skip
  containment; inbound envelope defanging + nonce; injection ledger; calibration
  harness v0 (fixtures + runner + CI gate) — calibration ships *with* the first
  judgment stage, not after it. **The coarse taint tracker ships with the
  Recorder** (§10.1): content-trust config + observe-stream session-sticky taint
  + the `memory.llm_writes` posture knob — a poisonable memory tool must not
  precede its firewall. All LLM-authored writes cap at `AgentObserved`;
  quarantined records are write-only (inspectable via the panel RPCs and
  sqlite tooling) until P3's
  review surfaces.
- **P2 — Distiller + lifecycle hooks + taint completion.** Detached bounded
  extraction (the fork harness stays a documented seam until a
  capability-gated tool-authorization layer exists — §8.4);
  interaction-stream + compaction triggers with resource guards (bounded
  host-side reads over compaction discards); pre-rotation distillation on
  respawn/retire and quarantined distillation on reset (D3 knowledge half);
  taint completion — comms taint join + Distiller evidence-range taint
  (§10.1; dispatch-time visibility stays upstream-gated on the §13 ask);
  trust lattice enforced end-to-end.
- **P3 — Steward.** App-enabled steward provisioning; scheduled dreams;
  staged-commit machinery with the full validator rule set; mob scope +
  proposals + promotion (quarantine-promote behind gating approval); exit
  interviews; contradiction→conflict-signal bridge; usage audit + working-set
  ranking; console Memory panel + timeline events; the §10.3 action additions.
- **P4 — Operator scope + Hygienist.** Operator profiles (realm-keyed; once
  `OperatorId` keying is settled); transcript-revision curation at compaction
  boundaries with the span-reference validator rules. *Status: under §16 Q1
  provisional keying, stock `rpc_gateway` installs
  `ConsolePrincipalOperatorResolver` when `operator_scope = "provisional"`.
  Operator scope joins composed build-time metadata after an authenticated
  console principal addresses that identity; this does not enable shared-body
  recall. Config-off, absent-resolver, and absent-principal cases remain inert.
  Steward proposal-routing activates independently on the config knob because
  proposals carry their operator key. The Hygienist remains parked, not
  publicly activated; see §7.2 and §8.6 for the precise boundaries.*

Definition of done for the initiative: an identity in a mob accumulates,
consolidates, and recalls memory across respawns with zero external services;
every judgment stage has a calibration profile and a CI-gated scorecard; the
console shows the whole loop; D1–D5 are closed.

**As built:** Distiller, Steward, and the parked Hygienist have runnable
calibration harnesses; CI gates profile/fixture schema consistency and their
mock-lane deterministic invariant verdicts. Retired Selector artifacts are
schema-checked historical inputs, not runnable mock/shuffle coverage.
The judgment scorecard itself gates in `--mode live` only (needs credentials
no CI lane supplies) — see the §11 as-built note.

---

## 16. Non-goals and open questions

**Non-goals (initiative).** No embeddings, FTS, or retrieval indexes in the
bundled store — ever (§12 ratchet; scale-out retrieval is the hub's job). No
synchronous LLM work on the turn path (the original plan's Selector-budget
exception was retired with §8.3). No
hub dependency of any kind. No memory-as-instructions, ever. No cross-realm
scope composition (including operator profiles — realm-keyed in v1, §7.2). No
mechanical per-turn citation obligation on agents.

**Open questions.**

1. `OperatorId` keying and activation: what identifies an operator within a
   realm (auth principal? console identity? explicit registration?), what the
   cross-realm declassification flow looks like if ever wanted, and where a
   provisional keying (console auth principal) could pull activation earlier —
   blocks P4, not P0–P3.
2. Concurrent embodiments: if one identity ever binds multiple live sessions,
   identity-scope writes need read-your-writes and
   last-write-wins-with-supersede semantics; SQLite gives us the primitives, the
   policy needs deciding.
3. **Historical Selector question, no longer an active rollout gate:** model
   tier and working-set K (the original §8.1 invocation design called for
   measuring tier/K/latency with calibration rather than guessing; §8.3 is
   retired).
4. Default injection budgets per scope (identity vs mob vs operator sub-budgets
   within the 20 KB/assembly ladder). The §8.3 scale-posture ceilings
   (soft/hard active-record limits before hub candidates are the answer) remain
   historical Selector design context, not current runtime limits.
5. Steward cadence defaults, per-mob vs per-realm granularity, and default
   background-LLM budgets / headroom thresholds for the §8.1 resource guards.
6. Whether the dream's usage-audit verdicts should be surfaced to operators as
   "memories you might want to correct" (probably yes, via console; needs UX).
