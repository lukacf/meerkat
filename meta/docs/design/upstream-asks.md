# Meerkat upstream asks — work order

## Current status (2026-07-12)

This file is both the historical evidence record and the current upstream work
queue. Historical problem statements remain below, but their status is
authoritative only through the explicit status line under each ask.

**Open actionable asks: none.** All 40 tracked asks now have an explicit
closed or shipped disposition. Ask 34 shipped in Meerkat 0.7.30 via PR #879
(`9300fcf0df351125c6196d25a1cf97b8b26f3faa`) and was adopted in Meta
0.7.37 via PR #277 (`5f22e445`). Ask 35 shipped in Meerkat 0.7.30 via PR #878
(`d084689a84c27c0a6daefcd926952110a967abfd`).

meerkat PR #874 (`005e67837888497bdb266653de033481b6c58f18`, included
in the published 0.7.29) closed asks 10, 14, 27, 28, 29, 32, and 33 plus ask
31's destructive-rollback half at their root ownership boundaries. Ask 31's
only unfinished property was split out and is tracked exclusively as ask 34.
Ask 31's rollback half and asks 33/10/29 shipped changelog-silent and were
source-verified 2026-07-12.

**Meta adoption:**
- **0.7.35**: ask 32 (retire-disposition fence — 0.7.34 drain-poll workaround
  removed), ask 14 (`MemberProgressSnapshot` on member_status wire + SDK types
  + identity-inspect projection), ask 28 (objective events projected), ask 27
  (verified no meta surface consumes the receipt — outcomes reach agents
  upstream).
- **0.7.36**: ask 29 (`SpawnMemberSpec.model_override` field-scoped seam for
  model-only reprofiles — whole-profile snapshot retained only for
  provider-pinned profiles, since upstream does not re-infer the provider
  under `model_override`), ask 31's rollback half acknowledged (resume
  rollback restores to durable idle; Meta forwards the retired-session
  revival seam and carries ask 34's ignored end-to-end acceptance test, but
  ordinary resume does not auto-revive yet; the 0.7.34 GONE-probe stays as a
  regression tripwire), ask 33 acknowledged (busy_timeout 5s→60s +
  `SqliteConnectionOptions`; `META_IDENTITY_RESTORE_CONCURRENCY` can return
  to its default of 4, knob retained). Ask 10 (call-level fork authorization)
  has no meta surface to adopt.
- **0.7.37**: all Meerkat pins moved to 0.7.30 and ask 34's end-to-end
  retired-session revival acceptance test was un-ignored. Ordinary cold
  restart now revives the same archived + retired session identity with its
  transcript intact; the Bug I manual row-copy repair is no longer needed on
  Meerkat 0.7.30+.

At the 2026-07-12 checkpoint, the non-ask follow-ups were a Meta
console-health affordance from ask 14's progress data, an optional Meta
gateway projection for M4's already-shipped terminal-status query, and refresh
of Meerkat's downstream compatibility table after M3. **Follow-up update
(2026-09-22):** the console-health affordance subsequently landed in Meta
PR #279 (`f3db01834548ede68faf56bfd2d8cba8a8fea3c3`, 2026-07-13) and is no
longer outstanding. The M4 projection and compatibility-table refresh are
separate local projection/documentation follow-ups, not open upstream runtime
asks.

Everything else in this document is **closed, shipped, superseded, or retired
as a separate upstream ask**. In particular, ask 12 was fixed before it was
filed, ask 13's operational incidents were closed by a Meta-local forwarder
fix plus later Meerkat member-scoped failure containment, and the previously
status-less Studio M1–M4 reports now carry explicit closure lines below.

> **Handed to the meerkat coding agent by Luka (2026-07-02).** Eight asks for
> the meerkat repo, derived from the Meta memory initiative
> ([`agent-memory-architecture.md`](agent-memory-architecture.md) §13) and
> refined by what its implementation actually found. **None of these block
> Meta** — every ask has a shipped Meta interim behavior, so land them in
> whatever order fits meerkat's own constraints. Each ask states the problem,
> code-verified evidence, a proposed shape (a starting point, not a contract —
> deviate where meerkat's internals argue otherwise, but preserve the stated
> *property*), and what Meta does today / will do when it lands.
>
> **Suggested sequencing by impact:**
> 1. **Ask 1 + Ask 3** (same seam, one change set) — unlocks Meta flipping
>    ambient per-turn memory injection on by default, echo-safe by
>    construction, and closes the injection-channel-authenticity gap.
> 2. **Ask 2 (+ Ask 8, natural pair)** — stops the per-build re-embed tax that
>    grows with every respawn/reset in a realm; operational debt accruing
>    daily.
> 3. **Ask 5** — closes the remaining persistent-prompt-injection gaps
>    (cross-process taint, send-time semantics, dispatch-time race).
> 4. **Ask 6** — deepest change, biggest architectural unlock (contained
>    fork-launched extraction with prompt-cache economics).
> 5. **Ask 4** (incl. the head-revision-read refinement) and **Ask 7** —
>    quality-of-life; no urgency.

Evidence citations were code-verified against meerkat 0.7.9 (the version
Meta pins) and the meta `feat/agent-memory-system` branch; line numbers
may have drifted. The memory-survey archive
([`../archive/design/evidence/memory-survey-2026-07/followups.md`](../archive/design/evidence/memory-survey-2026-07/followups.md))
holds the original deep-dive evidence.

---


## Ask 1 — Typed injected-context message class, excluded from compaction indexing

**CLOSED — shipped in Meerkat 0.7.12 (#821).** Typed injected context is a
separate transcript class and is excluded from semantic-memory indexing.

**Title:** Add a typed injected-context message class that
`indexable_content()` excludes from compaction indexing

**Problem statement.** Hosts that inject ambient context into a turn (Meta
injects agent-memory recall blocks) have no message class to deliver it in.
`ContentInput` has no role field, so the injection is fused into the user's
own message text and persists verbatim in the transcript. At compaction,
`indexable_content()` classifies `Message::User` as
`Indexable(u.text_content())` unconditionally, so every discarded injected
turn deposits a full copy of the injection block (~18.5 KB at Meta
defaults) into session-scoped semantic memory. The result is linear,
unbounded-within-a-session pollution of `memory_search` (Meta defect D1):
duplicate near-identical entries progressively dominate results for any query
overlapping the injected records' vocabulary.

**Evidence.**
- Per-message indexing seam ignores content provenance:
  `meerkat-core/src/types.rs:1201-1203` (`Message::User` →
  `Indexable(u.text_content())`, unconditional).
- `MemoryIndexExclusion` (`meerkat-core/src/types.rs:1237-1247`) is the right
  typed shape but has no variant for injected/synthetic user content — while
  `Message::ToolResults` is `Excluded(MemoryIndexExclusion::ToolResults)`
  (`types.rs:1220-1222`), which is exactly why recalled text never re-indexes
  and the loop stays linear rather than geometric.
- Indexing of every discarded message on every successful compaction:
  `meerkat-core/src/agent/state.rs:1265` (call site), `state.rs:1362-1431`
  (session-scoped `MemoryIndexScope`).
- Meta side of the fusion: `crates/meerkat-meta/src/identity_first/runtime.rs:2425-2448`
  (every non-Steer send passes through `inject_for_turn`),
  `identity_first/agent_memory.rs:854-871` (`prepend_memory_injection`
  physically concatenates: `format!("{injection}\n\nCurrent user
  message:\n{text}")`).
- No filter exists for the injection marker: `grep
  meta_memory_observation` over meerkat-core and meerkat-memory returns
  zero hits (survey, `followups.md` §1).

**Proposed shape.** A first-class injected-context delivery class — either a
new message variant or a typed transcript role that `indexable_content()`
actually consults — with a corresponding
`MemoryIndexExclusion::InjectedContext`, plus a delivery surface that lets a
host attach injected-context messages alongside (not inside) the user's
content on the submit-work path.

**Coupling note (important).** This ask alone is not sufficient, and it is
explicitly coupled to a Meta-side restructuring
(`agent-memory-architecture.md` §9.1): because Meta today *fuses* the
injection into the user's message text, a role class applied to the fused
message would either exclude nothing or exclude the operator's real words
along with the injection. Meta must stop fusing and deliver injected bodies
as a **separate typed message** — the ask and the restructuring land
together.

**Second rationale: channel authenticity.** The injection envelope is plain
text an attacker can reproduce — a peer message or echoed web content can
forge "remembered operator preferences" verbatim, inheriting whatever
authority the model grants real memory. Meta's interim mitigations (inbound
defanging of reserved envelope markers, a per-session nonce) are bar-raising,
not authoritative. A typed message class gives injected memory an
authenticated channel instead of a text pattern (§9.1 anti-spoofing).

**Meta interim behavior.** Per-turn ambient push defaults **off**
(`agent_memory.per_turn_injection = "off" | "budgeted"`); the opt-in
`budgeted` mode carries the full budget ladder and dedup. All *default*
injection surfaces are already-excluded message classes (system prompt, tool
results). The per-turn default flips to on only when both halves — this ask
and Meta's separate-typed-message delivery — have landed.

---

## Ask 2 — `MemoryStore` lifecycle: delete/GC and lazy per-scope index loading

**CLOSED — shipped in Meerkat 0.7.12 (#821).** `drop_scope`, paged scoped
enumeration, and lazy HNSW scope loading close the lifecycle and rebuild-cost
properties requested here.

**Title:** `MemoryStore` needs delete/GC (per-scope drop) and lazy per-scope
index loading

**Problem statement.** Session semantic memory is append-only and scoped to
exactly one `SessionId`, but session ids rotate under host lifecycle
operations, permanently orphaning rows. And the cost of orphans is not just
disk: the store re-embeds *every* row in the shared DB on *every* agent
build in the realm, so orphan growth taxes all future builds with CPU, RAM,
and startup latency — monotonically, with no cleanup facility of any kind
(Meta defect D3, storage half).

**Evidence.**
- The `MemoryStore` trait has exactly three methods — `index_scoped`,
  `index_scoped_batch`, `search`; no delete/prune/expire API exists:
  `meerkat-core/src/memory.rs:404-434`. Scope is strict equality on one
  `session_id` (`memory.rs:10-27`).
- `HnswMemoryStore` persists into one shared `<dir>/memory.sqlite3`
  (`meerkat-memory/src/hnsw.rs:281`); inserts are append-only
  (`hnsw.rs:419-608`); the only `DELETE FROM memory_*` statements are the
  rollback repair of a partially failed batch (`hnsw.rs:548-559`) — not a
  cleanup facility.
- `HnswMemoryStore::open` runs inside every `build_agent`
  (`meerkat/src/factory.rs:5419-5420`, wiring `factory.rs:5411-5472`), does
  an unfiltered `SELECT` over every row (`hnsw.rs:319-335`), re-embeds every
  text (`hnsw.rs:344`), and constructs an in-memory HNSW graph per session id
  found — including orphaned sessions nothing will ever search
  (`hnsw.rs:348-354`).
- Host lifecycle rotates session ids: Meta's `meta/respawn` RPC is always
  a mob-level retire+spawn that mints a fresh `SessionId` and rebinds
  continuity (`crates/meerkat-meta/src/rpc.rs:2937-2972`; meerkat-mob
  `actor.rs:14553` builds the replacement with no `resume_session`); `reset`
  mints a new id by design; resume has a fresh-spawn fallback; retire/delete
  leave all rows behind forever (survival matrix in `followups.md` §2).

**Proposed shape.** Two additions, independently useful:
1. **Delete/GC on the trait** — a per-scope drop (`drop_scope(session_id)`
   or equivalent) so a host that rotates or deletes a session can reclaim its
   rows; record-level delete is a bonus, per-scope is the need.
2. **Lazy per-scope index loading** — `open()` (or first search) loads and
   embeds only the requested scope's rows instead of rebuilding every
   session's graph in the realm on every agent build.

**Meta interim behavior.** Orphan growth is documented as a known cost.
The *knowledge* half of D3 is handled Meta-side regardless:
distill-before-rotation (initiative P2) runs the Distiller over the session
before respawn/reset rotates it, so what mattered survives in Meta's own
identity-keyed store even though the meerkat rows are stranded.

---

## Ask 3 — Compaction-summary indexing exemption

**CLOSED — shipped in Meerkat 0.7.12 (#821).** Compaction summaries are
excluded from semantic-memory indexing and the retained-turn budget.

**Title:** Discarded compaction summaries should be excluded from compaction
indexing

**Problem statement.** The compactor injects its summary as a `Message::User`
carrying `TranscriptUserRole::CompactionSummary`, and discards prior
summaries at the next compaction — but `indexable_content()` ignores
`transcript_role`, so a discarded summary is itself indexed into session
semantic memory. This is model-mediated content (an LLM distillation of the
window, potentially including injected-memory text) being mechanically
indexed as if it were first-hand user input — a secondary echo edge on top
of ask 1.

**Evidence.**
- Summary injected as user message: `meerkat-session/src/compactor.rs:194-197`
  (`Message::User(UserMessage::compaction_summary(...))`), role type at
  `meerkat-core/src/types.rs:2171-2178`.
- Prior summaries discarded at the next compaction:
  `compactor.rs:226-229`.
- `indexable_content()` never inspects `transcript_role`
  (`types.rs:1201-1203`); the only consumer of `is_compaction_summary` in the
  codebase is `session_store.rs:701` (survey, `followups.md` §1, point 5).

**Proposed shape.** The same typed-exclusion fix as ask 1: have
`indexable_content()` consult the transcript role and return
`Excluded(MemoryIndexExclusion::CompactionSummary)` for summaries. The seam
is one line today; the role already exists and is already stamped.

**Meta interim behavior.** None required — the edge is meerkat-internal.
Meta's echo-safe injection posture (ask 1 interim) shrinks what leaks into
summaries in the first place, and the residual pollution is documented as a
known defect rather than worked around.

---

## Ask 4 — LLM-curated compaction seam (host-supplied curator)

**CLOSED — shipped in Meerkat 0.7.12 (#821).** `CompactionCurator` supplies
the summary in place of the compactor's LLM call; transcript revision listing
also exposes the current head requested by the implementation refinement.

**Title:** Let a host-supplied curator produce the compaction summary

**Problem statement.** Summary generation is fixed inside the compactor; a
host cannot supply or amend the summary content. Meta's Hygienist
(`agent-memory-architecture.md` §8.6) targets the incremental-session-notes
pattern proven out by Claude Code's SessionMemory/SM-compact: a
continuously-maintained notes artifact that gets spliced in at the compaction
boundary, making compaction itself **zero-LLM-cost** — no summary call at the
boundary at all, because the notes were maintained incrementally during the
session. Without a curator seam, a host can only run its curation as a
separate boundary-triggered pass alongside meerkat's own summary call, paying
twice.

**Evidence.**
- The seam today: `DefaultCompactor::rebuild_history` constructs the summary
  internally and injects it (`meerkat-session/src/compactor.rs:194-197`);
  there is no host hook.
- The prior art being adopted: Claude Code's SM-compact splices a maintained
  session-notes artifact as the post-compact summary
  (`claude-code/src/services/compact/compact.ts:614-624`,
  `sessionMemoryCompact.ts:479-480`; survey `followups.md` §1) — flag-gated
  in CC but the right shape (`agent-memory-architecture.md` §5, adopt list).
- The Meta consumer: Hygienist incremental-notes target, §8.6 — "a
  continuously-maintained notes artifact spliced in at the compaction
  boundary via upstream ask 4's curator seam would make compaction itself
  zero-LLM-cost."

**Proposed shape.** A compactor extension point where the host registers a
curator: given the transcript window being compacted (and the prior summary),
it returns the summary content — with the default behavior unchanged when no
curator is registered. Must preserve meerkat's ordering invariant (indexing
gates the compaction commit — "never lose the only copy") and compose with
ask 3's exclusion so a curated summary is not itself re-indexed later.
Marked **later** in §13 — valuable, not urgent.

**Meta internal parked engine.** Meerkat's standard compaction stays as-is.
The retained Hygienist engine can use meerkat's existing audited same-session
transcript revisions
(`session/rewrite_transcript` / `session/transcript_revision` /
`session/restore_transcript_revision`), staged and validated like any other
memory mutation.

**P4 implementation refinement (found building the Hygienist against
meerkat 0.7.9).** The revision *apply* surface is reachable and used:
`PersistentSessionService` implements `SessionServiceTranscriptEditExt`
(`rewrite_session_transcript` / `restore_session_transcript_revision`) and
Meta's internal seam can accept the concrete typed service (the erased
`MobSessionService` does not carry the edit extension), but the public SDK and
gateway do not activate it in this release. One adjacent read gap remains:
**no `SessionService`-level API
exposes the current transcript head revision id.**
`Session::transcript_revision()` computes it, but only on an owned
`Session`; `read_transcript_revision` requires already knowing a revision
id. Two consequences, both currently absorbed rather than closed:
(a) rewrites cannot compare-and-swap — `expected_parent_revision` is sent
as `None`, and the service's internal mutation guard plus
`TranscriptEditRunningBehavior::Reject` (refuse mid-turn) are the only
concurrency protections; (b) §7.1's revision-pinned `EvidenceRef`s cannot
be captured at write time — Distiller/Recorder provenance keeps
`revision: None` ("head at capture time") until a head-revision read
exists. Nothing dangles meanwhile (revisions are additive and parent
bodies are retained), but pinned-revision provenance resolution stays
approximate. Small ask: expose `current_transcript_revision(id)` (and
ideally a revision list) on `SessionService` or the history extension.

---

## Ask 5 — Comms envelope taint metadata + dispatch-time taint visibility

**CLOSED — shipped across Meerkat 0.7.12–0.7.13 (#821, #824).** Signed sender
taint, typed tool provenance, synchronous dispatch classification, and
host-consumable peer-ingestion/outbound-taint surfaces close both halves.

**Title:** Taint metadata on comms envelopes; tool-source taint visibility at
dispatch time

**Problem statement.** Two related gaps for persistent-prompt-injection
defense:
1. **Cross-agent taint has nowhere to ride.** `Envelope` is
   `{id, from, to, kind, sig}` with no metadata field, so a sender cannot
   carry a content-taint flag to its peers. Intra-mob peer messages are
   trusted-or-dropped by construction, which means a prompt-injected agent A
   can launder payloads through peer B, whose own session ingested nothing
   untrusted — B's memory writes look clean
   (`agent-memory-architecture.md` §10.1).
2. **Taint is only visible asynchronously.** The host learns that a session
   ingested untrusted content by watching the observe stream, so a memory
   write in the same turn as the session's *first* untrusted ingestion can
   race the tracker and land unquarantined. The typed facts needed for
   synchronous classification already exist (`ToolDef.provenance` with
   `ToolSourceKind::Mcp` + server source id; provider-native
   `ServerToolKind::WebSearch`) — they are just not exposed to the host at
   tool-dispatch time.

**Evidence.**
- Envelope shape: `meerkat-comms/src/types.rs:86-97` — fields
  `id, from, to, kind, sig`; the signature is over canonical CBOR of
  `[id, from, to, kind]` with field order fixed per spec, so there is no
  slot for metadata today and adding one touches the signing contract.
- No turn-level or session-level content-taint fact exists anywhere in the
  platform; the only "untrusted" concepts are comms sender admission and
  mobpack deploy trust, which are different facts
  (`agent-memory-architecture.md` §10.1).
- The dispatch-time race and its consequence (Recorder classifying against
  the observe stream instead of synchronously): §10.1, "its one honest gap
  is a race: a write in the same turn as the session's *first* untrusted
  ingestion can beat the observe stream."

**Proposed shape.**
1. An optional, signed metadata field on `Envelope` (protocol-versioned,
   since the canonical signable bytes are spec-fixed) able to carry a
   sender-session taint flag or labels, so receivers can propagate taint
   without out-of-band joins.
2. A smaller, independent ask: expose tool provenance/taint classification at
   tool-dispatch time (hook or ordering guarantee), so a host's memory write
   path can classify synchronously instead of racing the observe stream.
   Marked **later** in §13.

**Meta interim behavior.** P1 ships the coarse deterministic posture: a
content-trust configuration (web/fetch always untrusted; MCP servers
untrusted by default with an explicit allowlist) plus an observe-stream
tracker that marks the session tainted — session-sticky, cleared only at
fresh-context boundaries — with the first-ingestion race documented. P2
ships the comms taint join over what the observe surface actually carries
(implemented in `crates/meerkat-meta/src/memory/taint.rs`,
`observe_inbound_peer_content`), and building it sharpened this ask:
meerkat 0.7.9 has **no typed inbound peer-message event** — a delivery
surfaces only as injected prompt text on the receiver's `RunStarted`
(`format_peer_message_projection`, meerkat-core `interaction.rs:126`), so
the join parses the sender's `MemberCommsName` out of that text and checks
the sender's tracked taint at delivery-observe time, not send time. Three
gaps only envelope-level flags can close: (a) send-time vs observe-time
taint state can differ across a sender rotation; (b) peer *requests* render
a raw cryptographic peer id, unmappable to an identity host-side; (c)
cross-process senders are not in the host tracker at all. Deployments that
cannot accept the P1 race set `memory.llm_writes = "quarantined"` (every
LLM-authored write quarantines until steward/operator review).

---

## Ask 6 — Capability-gated tool authorization for fork-launched members

**CLOSED for member spawn — shipped in Meerkat 0.7.12 (#821).**
`SpawnMemberSpec.tool_access_policy` is enforced end to end. The distinct
call-level transcript-fork residue was tracked narrowly as ask 10 and shipped
in Meerkat 0.7.29.

**Title:** Call-level tool authorization policy on fork/spawn launch modes, so
a fork-launched member can be capability-contained without changing its tool
list

**Problem statement.** Meta's memory Distiller wants to run as a fork of
the live session — `Session::fork`/`fork_at` are O(1) copy-on-write, and a
fork shares the parent's prompt-cache prefix, making extraction nearly free
in input tokens (the pattern Claude Code uses for exactly this workload). But
prompt-cache identity requires a byte-identical prefix *including the tool
list*, and the only fork path Meta can reach —
`MemberLaunchMode::Fork` / `fork_helper` — spawns a **live member carrying
the parent's full tool surface**. An LLM stage whose entire job is reading
possibly-poisoned transcript evidence cannot be handed live tools: that is an
uncontained extractor. Containment must therefore live at the *authorization*
layer (every call gated to an allowlist, e.g. read-only + a memory-write
seam), not at tool-list subsetting (which breaks the cache) — and no such
layer exists on the fork/spawn path today. The same gap forced Meta's
memory Steward to be a shell-driven pipeline of structured calls instead of
an agentic dream member.

**Evidence.**
- Fork launch surfaces: `meerkat-mob/src/launch.rs:23-57` (`fork_helper`),
  `meerkat-mob/src/runtime/handle.rs:5946` — a fork-launched member gets the
  parent's tool surface; there is no per-launch authorization hook.
- CoW fork primitive: `meerkat-core/src/session.rs:3644` (`fork_at`),
  `:3764` (`fork`).
- The prior-art constraint (tool list is part of the prompt-cache key, so
  containment must be call-level): Claude Code keeps the parent's exact tool
  list and gates via a `canUseTool` closure —
  `../archive/design/evidence/memory-survey-2026-07/followups.md` and
  `agent-memory-architecture.md` §8.4.
- Meta's decision record: `crates/meerkat-meta/src/memory/distiller.rs` module
  docs ("full tool surface … §8.4's fork containment") — the fork seam is
  named and parked there.

**Proposed shape.** A per-launch tool-authorization policy on member
launch (at minimum the fork mode; ideally any spawn):
`ToolCallAuthorizer`-style trait (`fn authorize(&self, tool_name, args) ->
Allow | Deny(reason)`) accepted by the launch/spawn config and enforced in
the tool dispatch path *after* ToolDef listing (list unchanged → cache
prefix unchanged; execution gated). Deny should surface to the model as an
ordinary tool error. Composes with meerkat's fail-closed capability
philosophy; dogma-wise it is an authorization seam, not a new semantic
authority.

**Meta interim behavior.** The Distiller runs as a detached bounded
one-shot over a transcript slice read from the session store (correct but
full-input-price; spend bounded by Meta's background resource guards), and
the Steward is a deterministic multi-phase pipeline of structured LLM calls
(the model holds no tools at all). When this ask lands, Meta switches the
interaction-triggered and pre-rotation distillation paths to session forks
and can consider an agentic steward gather phase; the seam is documented in
`distiller.rs` module docs.

---

## Ask 7 — Internal-runnable schedule targets in meerkat-schedule

**CLOSED — shipped in Meerkat 0.7.12, with runtime-host wiring completed in
0.7.13 (#821, #824).** Host-runnable targets flow through the normal durable
occurrence lifecycle.

**Title:** Let `meerkat-schedule` target a host-registered runnable, not only
mob members/sessions

**Problem statement.** Schedule target bindings only address sessions and mob
members. Host-internal periodic work — Meta's memory steward is the
concrete case: a scheduled consolidation "dream" that must NOT be a mob
member (see Ask 6's containment problem) — cannot be expressed as a
schedule, so it runs as a guarded `tokio` interval loop outside the schedule
subsystem: invisible to schedule listing/inspection surfaces, no shared
cadence semantics, separate operational story.

**Evidence.**
- Target vocabulary: `meerkat-schedule` `TargetBinding::{Session, Mob}` —
  verified during Meta P3 implementation; no host-runnable variant.
- The workaround and its deliberate forward-compatibility:
  `crates/meerkat-meta/src/memory/steward.rs` (`spawn_dream_loop`) uses the
  schedule subsystem's own interval-marker grammar
  (`parse_interval_marker_ms`) for its cadence config, so the config
  migrates unchanged the day a real target type exists; module docs carry
  the TODO.

**Proposed shape.** A `TargetBinding::HostRunnable { name }` (or
callback-registration equivalent): the host registers named runnables with
the schedule service at startup; occurrences invoke them through the normal
occurrence lifecycle (so listing, history, and failure surfaces work
unchanged). No new machine semantics — the schedule/occurrence machines
already own the lifecycle; this adds a target kind whose "delivery" is a
host callback.

**Meta interim behavior.** The steward's interval loop stays; cadence
config is already schedule-grammar-compatible. On landing, Meta registers
the dream as a host runnable and deletes the loop.

---

## Ask 8 — `MemoryStore` enumeration API

**CLOSED — shipped in Meerkat 0.7.12 (#821).** `enumerate_scoped` provides
paged deterministic reads with source-range and indexed-time filters.

**Title:** Add a scoped enumeration/read API to the `MemoryStore` trait
(pairs with Ask 2's delete/GC)

**Problem statement.** The `MemoryStore` trait exposes exactly
`index_scoped`, `index_scoped_batch`, and `search` — similarity search is
the *only* read surface. Hosts that need to read back what a scope contains
must approximate enumeration with a generous-limit search query. Meta hits
this in the post-compaction harvest: after `CompactionCompleted`, the
Distiller reads the discarded message range host-side from meerkat's own
session-memory store, and "read the discard range" becomes "one
big-limit scoped `search` and hope the limit covered it." Ask 2's GC/delete
work needs the same primitive (you cannot audit or selectively reap what you
cannot list), as does any re-derivation/re-indexing tooling.

**Evidence.**
- Trait surface: `meerkat-core/src/memory.rs:404-434` — three methods, no
  list/enumerate/count.
- The approximation and its honesty caveat:
  `crates/meerkat-meta/src/memory/distiller.rs` (`HnswDiscardSource`) — one
  scoped `MemoryStore::search` with a generous limit per harvest, caveat
  documented in code ("search is the only read surface … approximates
  enumeration").

**Proposed shape.** `async fn enumerate_scoped(&self, scope:
MemorySearchScope, page: PageRequest) -> Result<Page<MemoryEntry>, _>`
(paged, deterministic order, optional `source_range` overlap filter so a
caller can fetch exactly the entries covering a transcript range). Read-only
and additive — no change to indexing or search semantics; implementations
that cannot page can return one page.

**Meta interim behavior.** The generous-limit search approximation, with
the limit sized to compaction discard batches. On landing, the harvest reads
the exact range and the approximation caveat comes out of `distiller.rs`.

---

# Batch 2 — handed to the meerkat coding agents by Luka (2026-07-04)

> Seven further asks, derived from (a) the two §10.1 security gaps the shipped
> taint firewall documents as upstream-gated, (b) the fork-Distiller seam that
> ask 6's landing did not fully unlock, and (c) production evidence from
> **operator_validator** — the most general Meta deployment (one mob; hundreds of
> years-lived durable identities in the personal-agents + domain-agents +
> task-agents pattern; multi-GB sessions; BigQuery-backed custom stores; heavy
> agent-spawned worker churn). The operator deployment's own filed docs (in
> `operator_validator/docs/`, commit `c36c105`) are the authoritative specs for
> asks 11–13 — work from them directly. None of these block Meta; every ask
> states the interim behavior.
>
> *Locator note:* `operator_validator` is a pseudonymous, redacted stand-in for
> the operator deployment's private repository. Every `operator_validator/...`
> path, line number and commit cited in this document refers to that
> repository (the filed specs at commit `c36c105`, the evidence citations at
> its HEAD of 2026-07-04) and cannot be resolved from this tree.
>
> **Suggested sequencing by impact:**
> 1. **Ask 11** (incremental session persistence) — the standing scale wall;
>    two production fleet wedges already attributed to it.
> 2. **Ask 12 + Ask 13** — production incident classes at fleet scale
>    (stranded singletons; hot-loops and failure cascades).
> 3. **Ask 9** — closes the last two documented holes in the memory taint
>    firewall.
> 4. **Ask 10** — unlocks the deferred fork-Distiller (prompt-cache economics
>    over multi-GB transcripts).
> 5. **Ask 14 + Ask 15** — quality-of-life; no urgency.
>
> Evidence citations verified against meerkat 0.7.15 and meta `main`
> (post-#218/#219); the operator deployment citations against `operator_validator` HEAD 2026-07-04
> (a pseudonymous locator; see the locator note above).

## Ask 9 — Dispatch-time content-taint visibility + `ToolDef` provenance

**CLOSED — shipped across Meerkat 0.7.12–0.7.13 (#821, #824).** Tool
provenance is typed and the host receives synchronous/typed taint facts rather
than relying on name parsing and a lagging observe-only signal.

**Title:** Give hosts a synchronous taint signal at tool dispatch, and typed
tool provenance (completes Ask 5)

**Problem statement.** Meta's content-taint tracker derives taint from the
asynchronous observe stream, which leaves two documented, deliberate gaps:
(1) the **first-ingestion race** — a memory write in the same turn as the
session's *first* untrusted ingestion can reach the store before the taint
observer processes the tool event; (2) **name-based MCP classification** —
tool events carry only the tool NAME, so MCP tools can only be attributed to
a server when their names are server-qualified (`mcp__<server>__<tool>`);
anything else needs manual `untrusted_tools` config.

**Historical evidence and correction.** The filing-time Meta
`crates/meerkat-meta/src/memory/taint.rs` module docs recorded the first-ingestion
race and that its asynchronous tool events carried names without
`ToolDef.provenance`. The original report incorrectly generalized this to
missing upstream APIs in Meerkat 0.7.15: `ToolDef.provenance` and
provenance-bearing `HookToolCall`/`HookToolResult` payloads were already present
in that release (`84ecb083e982929d06217185acdcfc2f0182fffb`). This corrects
the reported upstream diagnosis, not the historical Meta observer-only
integration limitation; upstream capability availability and a host
composition's use of those capabilities are separate adoption questions.

**Proposed shape.** (a) A synchronous seam at tool dispatch — either a
pre-result host hook or a dispatch-time event with a completion barrier —
such that the host can classify "untrusted content is entering this session"
*before* the tool result lands in context. (b) `ToolDef.provenance` (typed:
builtin / MCP server id / bundle / host) surfaced on dispatch events, so
classification stops guessing from name shapes.

**Meta interim behavior.** Session-sticky async taint (conservative), plus
the `agent_memory.llm_writes = "quarantined"` posture knob for deployments
that cannot accept the race. On landing: dispatch-time classification closes
the race; name-shape probing and the `untrusted_tools` escape hatch shrink to
compat.

## Ask 10 — Fork tool authorization + fork-from-persisted (completes Ask 6)

**SHIPPED in meerkat 0.7.29** (PR #874, changelog-silent — call-level fork
authorization landed at its ownership boundary; no meta surface to adopt).
Historical status before PR #874: OPEN, NARROWED (2026-07-10) to call-level
fork authorization only. `PersistentSessionService` already resolved
transcript-edit sources from the authoritative persisted session when no live
source was available, so the fork-from-persisted half was closed (and appears
to have predated this filing). At that point `SessionForkAtRequest` and
`SessionForkReplaceRequest` carried no call-scoped tool authorization; PR #874
closed that residue. The paragraph is retained as history, not a current gap.

**Title:** Call-level `tool_access_policy` on session fork, and forking a
persisted (non-live) session

**Historical problem statement (pre-#874).** Ask 6 had landed fork-launched
members, but the fork-Distiller was blocked on two specifics:
`SessionForkAtRequest` carried no `tool_access_policy` (fork authorization was
build-time only), and
`fork_session_at` requires a LIVE parent session — while Meta's
reset/resume-fallback/compaction distillations run AFTER teardown, reading
from the session store.

**Evidence.** meerkat-core 0.7.15 `service/mod.rs:1632-1636`
(`SessionForkAtRequest { message_index, running_behavior }` — no policy
field); the fork-harness seam documented in
`crates/meerkat-meta/src/memory/distiller.rs` module docs ("TODO(§8.4 fork
harness)").

**Proposed shape.** Add `tool_access_policy: Option<ToolAccessPolicy>` to
the fork request family (fork-at / fork-replace), and a
fork-from-persisted path (parent loaded from the session store when not in
the live map) preserving the prompt-cache prefix.

**Historical Meta interim behavior.** The Distiller ran detached bounded
extraction over a bare LLM client (zero tools — so no containment gap, only foregone
prompt-cache economics). On landing, extraction moves to a fork sharing the
parent's cached prefix — material at production-scale multi-GB transcripts.

## Ask 11 — Incremental session persistence (`IncrementalSessionStore`)

**CLOSED — shipped in Meerkat 0.7.25 (#857).** `IncrementalSessionStore`
makes ordinary saves O(delta), and compaction commits a shrinking canonical
head rather than growing a monolithic session blob.

**Title:** Adopt the operator deployment's incremental session persistence spec — O(delta) saves,
compaction that shrinks the persisted head

**Problem statement.** `SessionStore::save()` persists the whole session
blob every turn: O(session) write amplification on every turn of every
member. Measured in production: a 9.2 MB session that was 95% retained
compaction revisions and 5% live messages; BigQuery's 10 MB DML cap turned
this into HTTP 413 → **two fleet-wide wedges (2026-06-12)**. Compaction
currently *grows* the persisted blob (revisions are retained), so the
mechanism meant to bound context growth unbounds storage growth.

**Evidence + spec.** `operator_validator/docs/MEERKAT_SPEC_incremental_session_persistence.md`
(authoritative; includes the trait sketch:
`append_messages` / `commit_rewrite` / `save_head` / `load_head` /
`load_messages`). The operator deployment's chunked-persistence mitigation:
`operator_validator/src/meerkat/session.rs:39-73`. (Pseudonymous locators:
both paths refer to the operator deployment's private repository at the
versions in the locator note in the introduction.)

**Proposed shape.** As the operator deployment's spec: an additive `IncrementalSessionStore`
trait with append/rewrite-commit semantics; whole-blob `SessionStore` remains
the compat surface. Compaction commits a *rewrite* (shrinking the head), not
a superset blob.

**Meta interim behavior.** None needed in meta itself; the operator deployment carries
chunked plain-text persistence as a workaround. This ask also sets the
persistence contract Meta's memory providers will follow (memory writes
must stay O(delta) — architecture §12 discipline).

## Ask 12 — Compaction-archive must not strand singletons

**CLOSED BEFORE FILING — shipped in Meerkat 0.6.30 (#747).** The validated
projection CAS now accepts a legitimate compacted shrink when the durable
projection lags, and disposal removes dead roster anchors even if an unrelated
archive error occurs. The exact persistence and full-archive regressions remain
in the current Meerkat test suite. Asks 20/21/21c were later, different
never-run/archive and cleanup-deadlock bugs; they do not reopen this ask.

**SHIPPED in meerkat 0.6.30 (PR #747) — predates this tracker entry; the
tracker incorporated an older production report without recording that its
original incident had already been fixed (maintainer annotation,
2026-07-10).** Two layers: (1) root cause — a legitimately compacted
runtime-authoritative session can replace a lagging, LONGER durable
projection using the already-validated CAS/continuity proof instead of
falling through to the rewrite-blind append-only guard; (2) defense in
depth — even if archival fails for an unrelated reason, disposal removes
the dead roster anchor so respawn/reset never leave an unreachable
singleton stranded. Regression tests still on origin/main:
`test_compacted_session_persists_when_durable_projection_lagged_across_compaction`,
`test_archive_succeeds_for_compacted_session_with_lagging_durable_projection`.
NOT the same bug as the later archive asks: 20/21 were created-but-never-run
members with no runtime snapshot (ArchiveSession/NotFound), 21c was a
runtime-loop self-deadlock during cleanup, and the 0.7.26 classic-store
MonotonicityViolation fix addressed torn-shutdown projection rollback.

**Title:** Fix retire-after-compaction stranding (`MonotonicityViolation` on
ArchiveSession shrink-save, no fallback, ghost roster entry)

**Problem statement.** When a member is retired after compaction, the
ArchiveSession shrink-save can hit `MonotonicityViolation`; there is no
bridge fallback, and the roster keeps a ghost entry. Respawn/reset cannot
recover the identity — only a full process restart. For fleets of years-lived
singletons this is a standing incident class, and compaction boundaries are
exactly where Meta's Distiller harvest hooks live.

**Evidence + spec.** `operator_validator/docs/MEERKAT_BUG_compaction_archive_singleton.md`
(authoritative repro + trace).

**Proposed shape.** Per the operator deployment's doc: make the archive save tolerate (or
sequence around) the shrink, and give the failure path a typed, recoverable
outcome instead of a ghost entry.

**Meta interim behavior.** None possible meta-side (the seam is inside
meerkat's session archive path); hosts carry watchdogs + process restarts.

## Ask 13 — Event-forwarder backoff + per-member commit-failure quarantine

**CLOSED AS AN OPERATIONAL INCIDENT (2026-07-10).** The forwarder hot-loop was
fixed in Meta (`9b55a063`): only active members are subscribed, failures use
per-member exponential backoff capped at 30 seconds, and repeated failures do
not flood WARN logs. Meerkat later made failed-batch resolution machine-total
(#859) and session-scoped composition-dispatch failures degrade only the
affected member instead of terminating the mob actor (#864). The originally
suggested generic forwarder terminal-state API was never needed to restore the
requested operational property and is retired rather than left open.

**Title:** Stop dead-stream hot-loops and fleet-wide commit cascades

**Problem statement.** (a) The agent-event forwarder retries dead streams at
~4 Hz per identity, forever — measured 208,693 warnings in 50 minutes on one
fleet; (b) a runtime-loop commit failure cascades fleet-wide instead of
quarantining the single failing member. At hundreds of members these are
outages, not nuisances.

**Evidence + spec.** `operator_validator/docs/META_EVIDENCE_forwarder_hotloop_and_commit_cascade.md`
(authoritative; includes measurements and call sites).

**Proposed shape.** Exponential backoff with a ceiling (and eventual typed
terminal state) on dead event streams; per-member isolation on commit
failure — one member degrades, the fleet continues.

**Meta interim behavior.** None meta-side; the operator deployment wraps every mob call in
app-level timeouts and runs its own wedge sentinel.

## Ask 14 — Typed member health/progress surface

**SHIPPED in meerkat 0.7.29** (`MemberProgressSnapshot` on the member
snapshot: `run_state` Idle/RunOpen, `in_flight_work`, `last_progress_at_ms`,
typed last-progress event, machine-owned `health`
Healthy/Degraded/Wedged/Unknown). MobKit 0.7.35 exposure: flows natively on
`meta/member_status` (whole-snapshot serde), typed as
`MemberProgressSnapshot` on `RichMemberSnapshot` in both SDKs, and projected
on the identity-inspect RPC. The console-health affordance remained a follow-up
at that checkpoint; it subsequently landed in Meta PR #279 (`f3db0183`,
2026-07-13). When progress is available, sidebar chips expose non-healthy
machine health and roster details show Health, Run state, In flight, and Last
progress.
Originally filed 2026-07-10. Lifecycle/member status and restart-durable terminal
input status had improved before this ask landed, but did not provide the
requested machine-owned run-open/idle, in-flight-work, last-progress/event,
and degraded/wedged projection. That paragraph records the pre-0.7.29 state;
the explicit shipped status above is authoritative.

**Title:** A per-member liveness/progress signal, so hosts stop
reverse-engineering health from event streams

**Problem statement.** Nothing in meerkat-mob answers "is this member alive
and making progress?". Production hosts hand-build it: the operator deployment runs a
WedgeSentinel (accepted-work vs event-flow gap > 300 s → pod restart) and a
silence watchdog (compaction-failed-then-silent detection → auto-respawn with
cooldown + breaker). The console "working" indicator bug is the same gap
surfacing in UI (indicator driven by event recency, not run-open state).

**Evidence.** `operator_validator/src/wedge.rs`, `operator_validator/src/watchdog.rs`;
`operator_validator/docs/META_BUG_console_working_indicator_clears_mid_run.md`.

**Proposed shape.** A typed per-member health projection (last-event-at,
run-open/idle state, in-flight work count, wedge classification) queryable
from `MobHandle` and/or emitted as a low-rate status event. This is the
historical design target; the shipped projection above closes it.

**Meta interim behavior.** Hosts keep their watchdogs; Meta's console
approximates from event recency.

## Ask 15 — Persist `interaction_id` in the transcript

**CLOSED — shipped in Meerkat 0.7.25 (#856) and adopted in MobKit 0.7.30.**
`SubscribableInjector::inject_with_interaction_id` + `WorkSpec.interaction_id`
thread host ids to runtime admission; `TranscriptMessageIdentity` persists
them onto committed messages. Identity-first console sends mint UUIDv5 ids,
history backfill stamps `interaction_id`/`run_id`, and console dedup treats
UUID-form ids as authoritative twin identity. The CLASSIC compatibility path
cannot thread an id through `external_turn_for_member`; that limitation does
not reopen the identity-first Meta ask and should only become a new ask if a
supported classic consumer requires it.


**Title:** Give live and historical copies of the same assistant reply a
shared identity

**Problem statement.** The console must deduplicate a live-streamed reply
against its own persisted twin, but the two share no id — forcing lossy
content-heuristic dedup (the meta console over-cull class). The
`TranscriptMessageIdentity` machinery (meerkat #808) is the natural home;
it currently carries identity for other purposes but the interaction id is
not persisted through the transcript.

**Evidence.** Meta console dedup investigation (2026-07-01): an
interaction-id-based meta fix was a no-op and was reverted because the
persisted transcript lacks the id; heuristic collapse remains.

**Proposed shape.** Persist `interaction_id` (and `run_id`, already modeled)
in `TranscriptMessageIdentity` for assistant messages, serde-defaulted so old
sessions deserialize unchanged.

**Meta interim behavior.** Content-heuristic dedup with its documented
over-cull edge cases; removed when the id lands.

---

**Explicitly not asked (batch 2):** multi-embodiment / multi-bind primitives.
The two production deployments (the operator deployment, the downstream app) both run strictly one live
session per identity; Meta ships a fail-closed single-embodiment guard and
will file the ask when a real use case materializes.

---

# Batch 3 — schedule firing pipeline (2026-07-06, the downstream app's 0.7.24 field report)

Field context: rpc_gateway persistent mode, on-disk SQLite schedule store
carried across upgrades since ~0.7.13-era binaries. Authoring and planning
work; NOTHING fires. Every occurrence in the store sat at `phase=pending`
with `lease_expires_at_ms=NULL` — including a fresh one-shot authored on a
just-cleaned store — with zero log output at any level. Root-caused in the
published 0.7.18 sources (all four asks are visible there); meta ships a
read-only watchdog (`spawn_schedule_claim_watchdog`) that names the stall
and the poisoned row, but only meerkat can make the driver survive one.

### Ask 15 addendum (2026-07-08, meta issue #254 item 3)

A second consumer class hit this: the console aggregator's session-history
backfill re-emits past turns as `interaction_complete` frames with
`session_id` set and `interaction_id: None` (nothing to stamp — the
transcript doesn't persist it), and identity queries force-trigger
backfill. A consumer correlating by session id alone treats old history as
fresh completions (field: false job completion). meta interim: frames
carry `source.kind == "session_history"` and the API docs now declare its
exclusion mandatory. Real fix remains this ask: persist interaction ids in
the transcript so backfill can stamp them.

## Ask 16 — `spawn_schedule_host` discards every driver tick error

**CLOSED — shipped in Meerkat 0.7.19 (#842).** Tick failures are tracked,
attributed, rate-limited, and recovery is reported.

**Problem statement.** `meerkat/src/surface/schedule_host.rs` runs the firing
loop as `let _ = driver.tick_once().await;` every 250 ms. When ticks fail
persistently (see asks 17–19 for why they do), schedules silently stop firing
forever: no ERROR, no WARN, nothing for `RUST_LOG=debug` to show — the error
value is dropped before tracing can see it. The downstream app's operator offered a
debug trace; there was literally nothing to capture.

**Proposed shape.** Log `tick_once` errors — on first occurrence and on
change at ERROR, with a rate-limited heartbeat while the same error persists;
count consecutive failures in the report struct. A driver that cannot claim
is an incident, not a no-op.

## Ask 17 — one poisoned row starves every schedule (all-or-nothing scans)

**CLOSED — shipped in Meerkat 0.7.19 (#842).** Schedule and occurrence row
faults are isolated while healthy neighbors continue claiming.

**Problem statement.** `ScheduleDriver::tick_once` begins with
`service.list()` — which fails wholesale on the FIRST schedule row whose
recovered machine state is rejected — and `claim_due_occurrences` (sqlite
store) deserializes and `classify_due_action`s EVERY occurrence row in the
store inside one transaction before leasing anything. Any single stale or
invariant-rejected row (old schema_version, `misfire deadline projection
does not match machine_state`, corrupted JSON) aborts the entire tick. One
bad row → zero claims, for every schedule, forever — combined with ask 16,
silently.

**Evidence.** The downstream app: 31/31 occurrences pending, no lease ever taken, on a
store whose rows span ~5 binary generations. A fresh, valid one-shot on the
same store never fired — starved by a neighbor row.

**Proposed shape.** Per-row tolerance in both scans: a row that fails
deserialization or due-classification is skipped and logged (schedule_id /
occurrence_id + reason), optionally quarantined to a dead-row table or a
`phase=quarantined` marker so operators can inspect and purge. The tick
claims everything healthy.

## Ask 18 — Deleted schedule tombstones fail recovery and kill `list()`

**CLOSED — shipped in Meerkat 0.7.19 (#842).** Legacy tombstones heal at the
durable parse boundary and new invalid states remain fail-closed.

**Problem statement.** Deleting a schedule persists a tombstone row whose
recovered `ScheduleLifecycleMachine` state is then REJECTED on read:
`RecoveredStateInvariantRejected { phase: Deleted, invariant:
"deleted_has_no_planning_cursor" }`. Every subsequent `list schedules` (and
therefore every driver tick and every meta boot repair) fails until the
operator hand-deletes rows in sqlite. The writer and the recovery invariant
disagree about what a legal Deleted state looks like — one of them is wrong.

**Evidence.** The downstream app's boot logs (every boot until manual cleanup of 16
tombstones): `list schedules: serialization error: generated
ScheduleLifecycleMachine rejected recovered machine_state: ...`.

**Proposed shape.** Either (a) the delete flow clears the planning cursor
before persisting the tombstone AND a one-time migration repairs existing
tombstones, or (b) recovery accepts Deleted-with-cursor and normalizes it.
Plus an upgrade-carry test: delete a schedule under version N, open under
N+1, `list()` must succeed.

## Ask 19 — claim scan reads the whole store per tick

**CLOSED — shipped in Meerkat 0.7.19 (#842).** SQL prefiltering bounds scans
to active schedules and live, due/lease-expired occurrences.

**Problem statement.** `claim_due_occurrences_impl` SELECTs and deserializes
every occurrence (joined with its schedule) with no SQL predicate on phase or
due time, four times per second. Beyond the poison surface (ask 17), this is
O(store) per tick: production-scale stores (multi-GB, years of terminal receipts and
occurrences) pay full-table deserialization at 4 Hz.

**Proposed shape.** Push the filter into SQL — `WHERE phase-in-pending-set
AND due_at_ms <= :now + horizon` (due_at_ms is already a column and already
indexed by the ORDER BY) — and let per-row tolerance (ask 17) handle the
stragglers. Terminal rows never enter the scan.

**Meta interim behavior (ships in 0.7.25).** Read-only
`spawn_schedule_claim_watchdog` in both gateways: probes `list()`, the
occurrence scan, and overdue-pending-unclaimed state every 60 s, and logs a
row-level diagnosis (poisoned row ids via direct sqlite triage) when the
pipeline is stalled. It makes the failure loud and attributable but cannot
make the driver claim past a poisoned row.

---

# Batch 4 — meerkat-studio field report (2026-07-06)

Reporter context: desktop app pinning meerkat =0.7.17 / meta =0.7.23; thread
mobs through an embedded gateway (FactoryAgentBuilder →
PersistentSessionService → MobBootstrapSpec → UnifiedRuntime), helper crews
through per-thread meta_gateway HTTP children. Asks M1–M4 below are the
reporter's, relayed verbatim-in-substance; ask 20 is meta's root-cause of
the reporter's K1, which turned out to be upstream.

## Ask 20 — retire/respawn of a never-ran member fails ArchiveSession and strands it in `retiring` (reported as meta K1) — P0

**CLOSED — converged across Meerkat 0.7.19–0.7.23 (#842, #843, #845, #847,
#849).** The original K1 disposal path and the subsequently exposed
never-run/session-authority residues now retire and respawn without a stranded
roster anchor or registered-runtime leak.

**Problem statement.** A session-bound mob member that has never run a turn
has no runtime-store session snapshot (the machine commits at run
boundaries). Retiring it runs the disposal pipeline to completion, but the
ArchiveSession step's authority lookup returns NotFound
(`load_runtime_authority_session_for_control` → no runtime snapshot →
`Ok(None)` → "mob archive authority returned NotFound for registered runtime
session …"), which `destroy_disposal_failure` escalates to a fatal error
("disposal completed but ArchiveSession failed"). The member is left in the
roster at `state=retiring`, `is_final=false` — permanently. `respawn` runs
retire first, checks `roster_still_contains_member` — still true — and
aborts, leaving a cancelled-kickoff zombie. Net: the only per-member control
primitives on a non-crashing path (see M1) fail for exactly the members an
operator most wants to manage.

**Evidence.** Deterministic meta repro
(`crates/meerkat-meta/tests/studio_k_asks.rs`, the `#[ignore]`d persistent test):
3-member crew via `ensure_member` on
FactoryAgentBuilder→PersistentSessionService; both RPCs fail with the exact
field string; members verified stranded in `retiring`. The identity-first
bridge already classifies this exact string as recoverable for SESSION-OWNED
retire (meta `is_recoverable_session_owned_retire_cleanup_error`) — the mob-
member path needs the equivalent judgment at the source.

**Proposed shape.** In meerkat-mob (the escalation site is the archive
helper at `src/runtime/provisioner.rs:894`): when disposal COMPLETED and the
archive miss is `NotFound for registered runtime session`, treat the
session-owned member as already-archived — per the helper's own doc comment
— and let the retirement commit finish; respawn then proceeds naturally.
(Alternative: register/commit an initial runtime snapshot at member session
creation so the authority never misses; either side of the seam works, the
judgment belongs upstream.) Do NOT fix this with downstream tolerance:
retrying fails identically and `list_members` is unchanged afterwards — the
member is neither respawned nor removed — so a console-layer
"tolerate and report accepted" patch misreports reality (verified
empirically by both meta and the reporter, independently). meta's
identity-first bridge already carries the exact tolerance string
(`is_recoverable_session_owned_retire_cleanup_error`) but it never applies
on the console `handle.retire()`/`respawn()` path, and lifting it there
would hit the misreporting trap above. Regression: retire + respawn of a
freshly spawned, never-prompted member on a persistent service must
succeed; a `roster.get(...)`-after-failed-retire assertion to pin the
no-strand invariant.

## M1 — force_cancel_member stack-overflows (SIGABRT) mid-turn — P0

**CLOSED — shipped in Meerkat 0.7.19 (#842).** Machine-owned
`boundary_cancel_dispatch_pending` bounds re-entrant cancellation to one
dispatch, and the exact mid-turn `force_cancel_member` stack-overflow shape is
covered by a regression test.

**Historical problem statement (pre-0.7.19).**
`MobHandle::force_cancel_member` → `MobActor::handle_force_cancel` →
provisioner `interrupt_member` → `LocalMobRuntimeBridge::interrupt_member` →
`MeerkatMachine::cancel_after_boundary` recurses inside
`execute_meerkat_machine_command` until the tokio worker aborts the process.
Reporter byte-diffed the chain: unchanged 0.7.4 → 0.7.17. No downstream
consumer can stop a running member; the model turn burns tokens to the next
boundary. Acceptance: cancel during an in-flight (not idle) turn interrupts
and returns without crashing; regression test for exactly that.

## M2 — no MCP path for library/factory embedders; per-spawn overlay lossy — P1

**CLOSED — declarative factory MCP shipped in Meerkat 0.7.19 (#842);
revival/cold-restore tool retention completed in 0.7.25 (#856).** Embedders can
declare `mcp_servers` on build options, builtin composites forward the runtime
handles, and profile/per-spawn tools are recomposed on revival. Opaque
in-memory dispatchers are intentionally reconstructed by the host after a
process restart; declarative profile MCP is the durable mechanism.

**Historical problem statement (pre-0.7.19/0.7.25).**
`AgentBuildConfig` has `external_tools`/`wait_for_mcp` but no MCP-server
config; `.rkat/mcp.toml` is CLI-only. The working embedder pattern
(`McpRouter::new_with_surface_handle(RuntimeExternalToolSurfaceHandle::ephemeral())`
→ stage_add → apply_staged → wait_until_ready → inject via
`SpawnMemberSpec.external_tools`) exists only in meerkat test code
(`service_factory.rs:1827`); `McpRouter::new()` fail-closes stage_add. With
`builtins = true`, `CompositeDispatcher` does not forward
`bind_external_tool_surface_handle`/`bind_mcp_server_lifecycle_handle` (only
`bind_ops_lifecycle`), so session-time late binding never reaches the
adapter. And `materialize_revived_member_session` composes
`external_tools_for_profile(&profile, None)` — revived members silently lose
per-spawn tools. Acceptance: a supported way to attach an MCP stdio server to
factory-built agents that composes with builtins, survives revival, and needs
no test-code incantation. (Meta shipped its half: a mob-wide
revival-surviving provider seam, `MobBootstrapSpec::with_default_external_tools_provider`.)

## M3 — semver discipline within 0.7.x — P1

**CLOSED AS POLICY — exact-pin and breaking-change policy documented in
Meerkat 0.7.19; release enforcement shipped in 0.7.25 (#855).** Meerkat
documents pre-1.0 patch-break behavior and a downstream compatibility matrix;
release preflight runs `cargo-semver-checks` and requires a `### Breaking`
changelog section for detected public-API breaks. The matrix needs routine
refresh, which is documentation maintenance rather than an open runtime ask.

**Historical problem statement (pre-0.7.25).**
0.7.16 changed `PersistentSessionService::new` (Option → required Arc);
0.7.12 added a required `content_taint` field to `CommsCommand::PeerMessage`.
Both broke 0.7-pinned downstreams. Ask: treat public-signature changes as
breaking (minor bump) or publish a meerkat↔meta compatibility matrix.
(Meta now exact-pins its meerkat family and records the pin per release.)

## M4 — durable run/interaction lifecycle query — P2

**CLOSED — durable reconciliation shipped in Meerkat 0.7.19 (#842), with
restart-first-class interaction and run terminal-status queries completed in
0.7.25 (#856).** Rust exposes live-or-durable terminal reports, and
`session/input_status` projects interaction status over JSON-RPC and the SDKs.
A Meta-gateway wrapper and a direct run-id wire projection remain optional
surface follow-ups, not missing upstream runtime authority.

**Historical problem statement (pre-0.7.19/0.7.25).**
Run framing is broadcast-only; after a host restart there is no "did
interaction X reach a terminal state, and which?" query. Reporter hand-rolled
runs.jsonl + a lookup RPC. A first-class terminal-status-by-interaction/run-id
query in meerkat-session/runtime deletes that code for every embedder.

## Ask 21 — owned-but-snapshotless sessions still strand on archive (ask-20 residue) — P1

**CLOSED — shipped across Meerkat 0.7.20–0.7.23 (#843, #845, #847, #849).**
Never-run durable sessions archive, cleanup no longer self-deadlocks, retry
states converge, and terminal registered-runtime NotFound residue completes
disposal.

**Problem statement.** 0.7.19's ask-20 fix routes disposal on
`session_known_to_archive_authority`, but that gate only reroutes sessions
the authority does NOT own (`known=false`, host-adopted). A mob-CREATED
member that has never run a turn is `known=true` — its durable session
record exists (create persisted it) — while its RUNTIME SNAPSHOT does not
(the machine commits at run boundaries). The owned-path archive then fails
exactly as before: `archive_with_mob_lifecycle_authority` → control-read
(`load_persisted_session_for_control`) NotFounds on the missing snapshot →
"disposal completed but ArchiveSession failed: … NotFound for registered
runtime session" → member stranded `state=retiring`. The authority-read
(`load_authoritative_session`, store-projection-eligible) and the
control-read disagree about the same session.

**Evidence.** Deterministic meta repro on meerkat =0.7.19
(`crates/meerkat-meta/tests/studio_k_asks.rs`, `#[ignore]`d persistent test):
3-member `ensure_member` crew on FactoryAgentBuilder→PersistentSessionService;
probe confirms `session_known_to_archive_authority = Ok(true)` for all three
idle members; retire still fails with the exact field string. The wrapper-
forwarding gap on meta's side is FIXED (meta forwards the seam through
`PreBuildMobSessionService`/`AfterCreateMobSessionService`); the remaining
failure is upstream.

**Proposed shape.** Make the owned-path archive tolerate the
never-committed-snapshot case: when the durable record exists, the runtime is
registered, and NO snapshot was ever committed, archive the durable record
and retire/release the runtime binding (the control-read should accept the
same store-projection eligibility the authority-read already accepts).
Regression: retire + respawn of a freshly created, never-prompted member on a
persistent service with a runtime store must succeed.

**Meta interim behavior.** Identity-first gateways (default-on since
0.7.25) route crew members through the identity authority, which disposes
tolerantly — the strand is only reachable for mob-plane (worker) members
that never ran, a narrow window in practice since workers receive kickoff
messages at spawn.

## Ask 22 — past-due one-shot regenerates occurrences unboundedly (planning-cursor precision loss) — P0

**CLOSED — shipped in Meerkat 0.7.20 (#843).** Trigger comparison and the
machine planning cursor use one millisecond representation, with a monotonic
planning guard preventing recurrence.

**Field report (the downstream app on 0.7.25):** one one-shot with a fire time
near/just-past now produced 223 misfired occurrences + 223 receipts in ~2
minutes (~1/sec, unbounded) on a clean store; halting required stopping the
gateway and truncating the schedule tables.

**Root cause (found by Luka; repro sits as a failing test in
meerkat-schedule/src/driver.rs):** sub-millisecond precision loss in the
planning-cursor round-trip. A one-shot's `due_at_utc` is a full-precision
`DateTime<Utc>` (ns); the machine-owned planning cursor is
`planning_cursor_utc_ms` (ms) — `RecordPlanningWindow` stores
`truncate_ms(due)`. The planner's guard `next_due_after(Once, cursor)`
yields when `due > cursor`, and `truncate_ms(due) < due` by up to 999µs, so
the trigger re-yields the same due forever. The pending-occurrence dedupe
only covers PENDING dues — the moment the occurrence goes terminal
(misfired in the incident), nothing blocks the re-plan: new occurrence,
same due, next ordinal → immediately misfires → terminal → next tick
re-plans. Aggravations: the same loop shape fires for COMPLETED one-shots
(re-plan → re-dispatch = double-fire, not just misfire spam), and interval
triggers are plausibly exposed on their last-planned slot.

Meta cross-confirmation: the loop reproduces with pure meerkat service
APIs (`refill_horizon` + `claim_due_occurrences`) — no meta code in the
cycle; the #237 claim watchdog is read-only and ticks at 60s (cannot drive
a 1/s loop). The ms/ns mismatch also explains regeneration WITH a pending
occurrence present: the stored occurrence due is ms-truncated while the
trigger re-yields the ns-precision due, so the `existing_due` dedupe set
never matches. Carry guard: `one_shot_misfire_must_not_regenerate`
(`#[ignore]`d in schedule_wiring tests, un-ignored on the 0.7.20 upgrade).

**Why the machines didn't catch it (recorded for the RCT discipline):**
(1) an RCT failure, not transition legality — one semantic fact ("planning
has covered up to T") stored at ms precision but compared at ns across the
shell/machine boundary; no RCT existed for the cursor's time-precision
round-trip. (2) The machines verify per-entity safety, not cross-entity
convergence — every lap of the loop is legal (the M1 cancel-recursion
family); "a Once trigger plans at most one occurrence, ever" and "the
planning cursor is monotone/idempotent under re-planning" were never
encoded as machine-checkable invariants.

**Fix (dogmatic, for 0.7.20):** (1) normalize ALL schedule time facts to ms
precision at the domain admission boundary (Once due_at, interval
start/end, planned dues) — one precision for one fact everywhere, matching
the sqlite `due_at_ms` columns; (2) a machine-owned convergence invariant
(`trigger_exhausted`-style fact, or RecordPlanningWindow terminal for Once)
so even a future representation bug converges instead of running away;
(3) regressions: the failing one-shot misfire repro, completed-one-shot
no-refire, interval terminal-tail stability over N ticks.

### Ask 21 addendum (21b) — 0.7.20 verification: read fixed, archive still NotFounds afterwards

Verified against meerkat =0.7.20 with the meta repro: the archive-scoped
read (`load_persisted_session_for_archive`) works — traces show the durable
document COMMITTING (`discarding stale live session … reason=StoredArchived`)
— yet `archive_with_mob_lifecycle_authority` still returns
`SessionError::NotFound` for the never-run registered session, so the
provisioner escalation ("mob archive authority returned NotFound for
registered runtime session") and the `retiring` strand persist unchanged.
The failure now sits AFTER the snapshot resolution — candidates: the
runtime-retire realization for a registered-but-snapshotless machine session
(the register-before-retire fallback may still NotFound on the second
retire), or the post-commit archived-typed-NotFound contract folding into
the error path mid-protocol.

Empirically ruled out on the meta side: the two-adapter split (a separate
`with_session_runtime_adapter` machine vs the service's cached adapter) —
the repro fails identically when the mob shares the concrete service's own
cached machine as the sole authority. Wrapper forwarding of
`session_known_to_archive_authority` verified live (probe: `Ok(true)`).

Repro unchanged: `crates/meerkat-meta/tests/studio_k_asks.rs`, `#[ignore]`d
persistent test; the trace capture recipe is a `tracing_subscriber` init
with `meerkat_mob=debug,meerkat_session=debug` in the test body.

## Ask 21c — 0.7.21's retire-completing archive arm deadlocks the whole mob (runtime-loop self-deadlock on the session mutation gate) — P0, blocks 0.7.21 adoption

**CLOSED — shipped in Meerkat 0.7.22 (#847), with the identity-first 21d
residue closed in 0.7.23 (#849).** Runtime-loop stop realization no longer
runs executor cleanup while holding the session mutation gate.

Verified against meerkat =0.7.21 with the meta K1 persistent repro
(`crates/meerkat-meta/tests/studio_k_asks.rs`,
`studio_k1_retire_respawn_succeed_on_persistent_ensure_member_crew`):
respawn of a never-run member now HANGS FOREVER at 0% CPU (deterministic,
4/4 runs) instead of 0.7.20's fast-fail NotFound. The wedged task is the
single-threaded MobActor, so the ENTIRE MOB is dead — every subsequent mob
command queues forever. Severity inverted: 0.7.20 stranded one member in
`retiring`; 0.7.21 bricks the mob. meta is HOLDING its pins at =0.7.20
and will not ship 0.7.21 to consumers until this is fixed.

**Root cause — a single-task self-deadlock in the runtime loop's
terminal-failure exit, PRE-EXISTING on 0.7.20 and newly load-bearing:**

1. During disposal, the boundary cancel makes the member's in-flight run
   fail; the runtime-loop task takes the terminal-failure arm and acquires
   the per-session mutation gate (`runtime_loop.rs:1547-1549`
   `lock_current_driver_authority` → `mod.rs:1292` →
   `lock_current_session_mutation_gate` → the entry's `mutation_gate`).
2. Recording the terminal fails — `driver.rs:1634` "generated RunFailed
   authority absent for run …" (the cancel already moved the generated
   lifecycle off the run) — and, STILL HOLDING THE GATE, the arm calls
   `stop_runtime_loop_executor_from_dsl_effect` (`runtime_loop.rs:1597-1603`;
   the guard drops only at scope exit, `:1613`).
3. That awaits inline `control_plane.rs:79
   executor.cleanup_after_runtime_stop_terminalized()` → the mob executor
   (`provisioner.rs:1841-1856`, the "mob runtime executor received stop"
   log) → `runtime_adapter.unregister_session` →
   `unregister_session_inner` (`session_management.rs:1357`) whose first
   await is `lock_current_session_mutation_gate` (`:1359`) — the SAME
   non-reentrant tokio mutex its own frame holds. The task parks forever
   (trace shows "unregister_session_inner start" but never "locked
   mutation gate"). Cycle of length 1: B waits on B.
4. First victim: the MobActor's disposal chain (`handle_respawn` →
   `dispose_member` → `SessionBackend::archive_with_authority_then_unregister`
   → `archive_with_mob_lifecycle_authority` →
   `PersistentSessionService::archive_with_machine_protocol`). The NEW #845
   arm (`session_document.rs:2744-2752`: Ready + Archived +
   `runtime_session_registered==true` → retire-only action vector,
   `write_document:false, retire_runtime:true`) proceeds where 0.7.20
   returned NotFound before any gate work, and calls
   `retire_runtime_control_plane` (`traits.rs:634`) → `gate.lock().await`
   (`traits.rs:645-647`) on the gate the parked loop task holds. Note
   `runtime_session_registered` is true precisely BECAUSE the unregister is
   the deadlocked continuation — the arm's own trigger condition is the
   deadlock's output.
5. Second victim (diagnostic red herring): meta's detached console
   event forwarder's 250ms reconcile sends
   `MobCommand::ApplyMachineInputEffects` (`handle.rs:6058`) to the wedged
   actor and parks on the reply. Not load-bearing.

**Why this is ALSO the true root of asks 20/21/21b:** the loop-task
self-deadlock exists on 0.7.20 with identical structure
(v0.7.20 `runtime_loop.rs:1545-1560`) — every never-run-member disposal
leaks a forever-parked runtime-loop task holding the gate AND leaves the
runtime session registered forever (unregister never runs). That
permanently-registered session is exactly the state 21/21b kept hitting.
0.7.21 fixed the SYMPTOM arm (archive of Archived+registered) while the
producer of that state still deadlocks upstream of it.

**Fix directions (in preference order):**
1. Drop the terminal-arm authority guard BEFORE
   `stop_runtime_loop_executor_from_dsl_effect` — the ChannelClosed arm
   already does exactly this (`runtime_loop.rs:1163` explicit
   `drop(effect_authority_guard)` with a comment); the terminal-failure arm
   builds the inverse of session_management.rs:1391-1397's own warning
   ("awaiting the loop under the gate would deadlock" — here the loop
   awaits unregister under the gate).
2. Audit the sibling stop-under-gate sites with the same latent shape:
   select! direct-effect arm (`runtime_loop.rs:768-777`), process_queue
   effect-drain (`:1140-1146`), queue-processing failure stops
   (`:1362`, `:1402`, `:1653`), Ok-arm commit/checkpoint-failure stops
   (`:1486`, `:1520` — guard from `:1439`).
3. Defense in depth: `retire_runtime_control_plane` should not block a mob
   actor indefinitely on the gate (bounded acquisition + typed busy error),
   so a future regression of this class fast-fails instead of wedging mobs.

**Acceptance (both meta repros, both must pass):**
- `studio_k1_retire_respawn_succeed_on_persistent_ensure_member_crew`
  (classic persistent chain) — currently DEADLOCKS on 0.7.21. Run it with
  a nextest per-test timeout; a hang is a fail, not a slow test.
- `doctrine_member_rpcs_route_identity_owned_members_through_identity_authority`
  (identity-first gateway construction, mob-plane worker respawn) —
  currently still fails on 0.7.21 with the ORIGINAL 21b error, fast:
  "respawn_member failed: … disposal completed but ArchiveSession failed:
  … mob archive authority returned NotFound for registered runtime session
  <id>". I.e. 0.7.21's #845 arms do not fire at all on this construction
  (first-archive of a never-run session: document not yet Archived, so the
  Ready+Archived+registered arm cannot match — ask 21's original
  owned-but-snapshotless strand). 21b is NOT converged on 0.7.21; fixing
  the loop-exit deadlock (which un-sticks unregister) likely changes this
  path too — re-derive the arms against a world where unregister actually
  completes.

Trace capture recipe unchanged (tracing_subscriber in the test body;
`meerkat_mob=debug,meerkat_runtime=debug` shows the full timeline:
RunFailed authority-absent ERROR → "unregister_session_inner start" with
no "locked mutation gate" → "retire_runtime_control_plane start" → silence).

### Ask 21 addendum (21d) — 0.7.22 verification: classic chain converged, identity-first-construction workers still archive-NotFound — P1

meerkat 0.7.22 (#847) verification with both meta acceptance repros:

- `studio_k1_retire_respawn_succeed_on_persistent_ensure_member_crew`
  (classic persistent chain): **PASSES** — retire AND respawn of never-run
  members converge in <0.3s. Ask 21c fixed as specified; the whole
  20/21/21b/21c family is closed on this construction. Un-ignored for good.
- `doctrine_member_rpcs_route_identity_owned_members_through_identity_authority`
  (identity-first gateway construction, mob-plane worker): **still fails**,
  fast, with the ORIGINAL error: "respawn_member failed: … disposal
  completed but ArchiveSession failed: … mob archive authority returned
  NotFound for registered runtime session <id>".

Trace delta vs the (now green) classic chain: on this construction
`SessionBackend::retire_runtime_before_archive` logs "calling runtime
retire" → "runtime retired" (the runtime is NOT already terminal, and the
0.7.22 bounded retire genuinely succeeds — no deadlock, no 30s stall),
then "archive_with_authority_then_unregister archiving session service"
→ the dispose ArchiveSession step fails NotFound with the session still
REGISTERED per the escalation text. I.e. the durable runtime is Retired
by the pre-archive retire, yet the archive protocol still resolves
NotFound and the machine still reports the session registered — the #845
Ready+Archived+registered retire-only arm apparently does not fire (or
fires and NotFounds) on this wiring. Construction difference to focus on:
the identity-first gateway runtime (meta `gateway_wiring::
open_identity_substrate` + identity_first bridge on MobHandle) with the
worker member on the mob plane; session service wiring differs from the
classic FactoryAgentBuilder → PersistentSessionService chain.

Repro: `crates/meerkat-meta/tests/studio_k_asks.rs`,
`doctrine_member_rpcs_route_identity_owned_members_through_identity_authority`
— currently TOLERANT of the ArchiveSession failure class (assert-routing
only); tighten the branch at the end of the test when 21d lands. Trace
recipe: tracing_subscriber init in the test body,
`meerkat_mob=debug,meerkat_runtime=info,meerkat_session=debug`.

Severity: P1 not P0 — fast-fail (no wedge, no strand escalation beyond the
known retiring-retry state), identity-first DURABLE members are unaffected
(tolerant disposal on the identity plane), and the classic chain is fixed.
Affects mob-plane worker churn under identity-first gateways (the downstream app
agent-tool workers, the operator deployment worker plane after migration).

## Ask 23 — break-glass host reassignment for stuck bindings — P3 (reframed 2026-07-08)

**CLOSED — shipped in Meerkat 0.7.25 (#854).**
`WorkGraphService::break_glass_reassign_attention` is host-only, requires a
principal and reason, and is event-stream audited. MobKit 0.7.30 exposes it on
the authenticated console surface only.


meerkat 0.7.23's `reassign_attention` requires the witness's
`can_link_derived_from`, which the machine derives ONLY for
coordinate-mode bindings (`tool_surface.rs` pins: coordinate=true,
pursue=false). That is the right model for AGENT-driven reassignment,
but it also binds the HOST: an operator console cannot move a
pursue/review/falsify/judge/observe binding to another member at all —
there is no host-authenticated reassign that bypasses the mode-derived
authority, and hosts must not forge projections. mobkit 0.7.30 surfaces
a precise error and hides the affordance for non-coordinate bindings;
the workaround (goal_request_close + create a new goal on the new
target) loses binding history/evidence continuity.

REFRAMED per doctrine (Luka, 2026-07-08): WorkGraphs are AGENT-operated;
humans debug. The mode-derived restriction is the design, not a gap — the
agent-native transfer is a COORDINATE-mode agent executing the move at a
human's conversational request (pursuers can only request, which is the
right shape: transfers are coordination acts). The one genuinely stuck
case is a binding on a wedged/retired agent with NO coordinator holding
authority over it — the graph cannot heal agent-natively.

Ask (narrowed): a BREAK-GLASS host reassign — explicitly attributed to an
authenticated principal, audit-logged, intended for debug/recovery only
(`with_trusted_principal`-style promotion on `AttentionReassignRequest`).
Not an operating path; the agent tool surface's mode-derived restriction
stays untouched.

## Ask 24 — filtered attention queries + terminal-binding GC — P2

**CLOSED — shipped in Meerkat 0.7.25 (#854, #856).** SQL-pushed filters,
NULL-tolerant migration, `prune_terminal_attention`, and
`SessionStore::load_meta` close all three clauses. MobKit 0.7.30 exposes prune
through RPC and the SDKs.


`SqliteWorkGraphStore::list_sqlite_attention` (0.7.23 store.rs:1576) runs
`SELECT attention_json FROM workgraph_attention` with no WHERE clause and
JSON-decodes EVERY row — all realms, all statuses — before Rust-side
filtering; and every `reassign_attention` keeps the superseded row forever,
so the table grows monotonically with binding churn. Any host that must
read binding state on a hot path (meta's one-binding-per-target
admission guard runs under a runtime-wide gate and, on shared stores, a
cross-process lock) pays an unbounded scan while serializing all binding
mutations behind it; at production-style eternal-fleet churn this walks into
lock-timeout territory.

Ask: (1) push `realm_id`/`namespace`/`status`/`target` filters into the
SQL (indexed columns or generated columns over the JSON); (2) a GC/prune
facility for terminal (Superseded/Stopped) bindings, mirroring the ask-2
memory-store lifecycle shape. meta interim: realm+status-filtered list
calls (upstream still decodes every row) and an actionable lock-timeout
error; the guard's session→member resolution memoizes per session id, but
each FIRST resolution still pays `load_persisted_session`'s full-session
deserialization — `MobSessionService` has no metadata-only read seam (its
own ask candidate).

## Ask 25 — attention-binding uniqueness belongs in the service/store (or arbitrate like sessions) — P1

**CLOSED — shipped in Meerkat 0.7.25 (#854).** Both halves landed:
transactional store-level uniqueness with a typed occupant conflict, and
newest-binding-wins arbitration for legacy duplicates. Meta's admission
layer is defense in depth; its sidecar-lock cleanup is downstream maintenance,
not an open upstream ask.


`MultipleActiveBindings` is a HARD per-turn error (0.7.23
meerkat/src/surface.rs:209): two active bindings matching one member's
turns brick that member until an operator intervenes. But nothing upstream
prevents the state: no store-level uniqueness constraint, and
`create_goal` / `reassign_attention` / `resume_attention` all mint
duplicates freely. mobkit 0.7.30 compensates with a host-layer admission
guard (occupancy check + per-runtime gate + cross-process SQLite sidecar +
write-time target normalization + session-metadata fallback) — five
review rounds of hardening for an invariant that can only be enforced
race-free next to the data. Residual holes remain by construction
(meerkat-CLI writes to a shared store bypass any host guard).

Ask, either (both is best):
1. Service/store-level uniqueness — active-binding-per-target enforced
   transactionally at create/reassign/resume (typed conflict naming the
   occupant), so hosts get the invariant instead of building it.
2. Degrade the overlay failure the way 0.7.23 degraded session
   arbitration (newest-session-wins): MultipleActiveBindings resolves
   deterministically (e.g. newest-binding-wins) with a loud diagnostic,
   instead of hard-failing every turn.

When either lands, meta's admission layer demotes to defense-in-depth.


## Ask 26 — structural reply affordance on comms deliveries — P2

**CLOSED — shipped in Meerkat 0.7.25 (#856).** `PeerReplyCapability` in the
dispatch context and the pre-addressed `reply_to_peer` tool provide the
structural affordance. No Meta change is required.


Peer messages arrive framed as prose ("Peer message from <endpoint>...")
with no structural reply affordance. Models reliably answer in their own
transcript instead of calling the comms tool back — charter/prompt
strengthening does not hold (meta consumer field report, 2026-07-08),
so every consumer ends up building an app-side relay.

Ask: deliver peer messages with a typed reply seam — e.g. a delivery
envelope carrying a `reply_to` capability/token the runtime turns into a
pre-addressed comms call (or a first-class `reply` tool scoped to the
delivery), so "answer the peer" is an affordance rather than a prompt
convention. meta interim: none (app-side relays).

## Ask 27 — peer sends to unreachable targets must not read as success — P2

**SHIPPED in meerkat 0.7.29** (`PeerDeliveryOutcome { Acked, HandedOff,
Queued }` typed peer-send outcomes; Python/TS SDKs validate the canonical
`comms/send` result variants and reject legacy/malformed shapes). Verified
2026-07-12: no meta surface consumes `PeerMessageReceipt`/`acked` — peer
sends happen agent-side inside the actor, so the typed outcome reaches the
complaining agents (the downstream app's false-success sends) directly upstream;
nothing to expose in meta. Originally filed 2026-07-10. Ordinary comms still collapse successful no-ACK
handoff and some unreachable paths into `acked: false`. Supervisor rotation's
durable operation receipts are intentionally scoped to that lifecycle
operation and do not make general peer delivery truthful.

Field (the downstream app, mobkit 0.7.30 / meerkat 0.7.25, 2026-07-09): an agent's
comms `send_message` to a peer whose runtime is not booted (or whose
transport is unreachable) returns `{"status": "sent", "receipt": {"acked":
false}}` — indistinguishable from a successful no-ACK-kind delivery from the
agent's seat. Agents confidently proceed ("ASKED") while nothing was
delivered; the downstream app now relays cross-runtime asks host-side as a workaround.

Root: `SendOutcome.acked` is only `true` on a verified peer ACK round-trip;
kinds that do not await an ACK (and every inproc handoff) report
`acked: false` for BOTH "delivered, no ack awaited" and — on some transport
paths — "endpoint gone". The agent-facing tool result collapses delivery
truth into one bit that cannot distinguish outcome classes.

Ask: deliver-or-error at the comms tool seam (a send whose transport
connect/write fails must surface a typed error to the agent), or a typed
receipt state the agent can act on — e.g. `delivery: "acked" | "handed_off"
| "queued" | "unreachable"` — so "the peer never got this" is representable.
meta interim: none (host-side relay).

## Ask 28 — kickoff-scoped objective→outcome correlation (reply-to-kickoff) — P2

**SHIPPED in meerkat 0.7.29** (`ObjectiveOwnerBound` durable owner-authority
binding + `ObjectiveConcluded { member, objective_id, outcome }` explicit
conclusion mob events; batched runtime inputs merge transcript causality with
conflict-stable objective consensus). MobKit 0.7.35 projects both events
through the structural mob-event surface. Originally filed 2026-07-10. Interaction ids and peer-reply capabilities are
available building blocks, but no durable objective id propagates across the
delegated work, and the lead has no explicit conclude-objective affordance.
Kickoff quiescence prevents premature teardown; it does not identify the final
answer.

Field (the downstream app): the kickoff send's interaction completes on the lead's
FIRST turn — usually the delegation, often textless. The real outcome lands
turns later under different interaction ids, so hosts reconstruct
"objective → final answer" with session-scope matching, a crew-quiescence
gate over runtime.sqlite, and an empty-answer hold. This same gap is the
likely trigger for their "peer no longer found/trusted after kickoff
wind-down" reports: treating kickoff-interaction completion as objective
completion starts teardown while workers still run, and a worker's report-
back then races the lead's retirement.

Ask, building on ask 15's interaction threading + ask 26's
`PeerReplyCapability` shape: a kickoff-scoped correlation seam — the kickoff
delivery mints a durable objective id that (a) stamps every transcript
message and delegated turn transitively caused by it, and (b) gives the
LEAD a pre-addressed "conclude the objective" affordance (mirroring
`reply_to_peer`) whose payload is the objective's final answer. Hosts then
await "objective concluded" instead of inferring it from interaction
completion + quiescence heuristics. meta would surface the conclusion as
a typed console/RPC event.

## Ask 29 — profile overrides freeze the whole tool surface across definition drift — P2

**SHIPPED in meerkat 0.7.29** (PR #874, changelog-silent; source-verified:
`SpawnMemberSpec.model_override: Option<String>` — a field-scoped model pin
reapplied over the CURRENT role profile on every materialization, including
cold restore, revival, and respawn, so tools/skills/peer posture keep
following the definition). MobKit 0.7.36 adopts it in `build_spawn_spec`
(whole-profile snapshot retained only for provider-pinned profiles — upstream
does not re-infer the provider under `model_override`); reset-reprofiled
members frozen under the old whole-profile override (the downstream app's security-domain agent)
heal on their next reset. The planned meta heal-on-divergence mitigation is
superseded. Originally filed 2026-07-10 (Bug G′).

Field (the downstream app's Bug G′, 2026-07-10): the only reset-reprofiled member
(domain:security, gen 8) has NO `meerkat_schedule_*` tools despite
`[profiles.security.tools] schedule = true` — the agent improvised via its
unrestricted shell, hand-writing schedule rows into `schedule.sqlite` that
the driver fired into the wrong member.

Mechanism (confirmed): a model-only reprofile forces meta to snapshot the
WHOLE profile into `SpawnMemberSpec.override_profile` (there is no
narrower override — handle.rs:1894). meerkat 0.7.25 M2 durably persists
`effective_profile_override` and the revival path takes it VERBATIM over
the current definition (actor.rs:5141, deliberately: "revival inputs must
equal spawn-time inputs"). Net: the member's tool flags freeze at
reset-time; definition edits (adding `schedule = true`) reach every
ordinary member but never the reprofiled one. The 0.7.26 cold-revival
epoch fix made revival work cold, which made the freeze bite every boot.

Ask (either resolves it):
- A FIELD-SCOPED override on `SpawnMemberSpec` (e.g. `model_override:
  Option<String>`) so a reprofile pins only what it changed and the
  definition stays authoritative for everything else (tools, skills,
  peer_description) across drift; or
- revival re-resolves the CURRENT definition profile and applies the
  recorded override as a patch (requires knowing which fields were
  overridden — same shape as the first option).

Historical Meta interim (superseded): heal-on-divergence at identity-first
restore (compare the replayed effective profile against the freshly-composed
spec; retire + resume-respawn when they diverge). The field-scoped upstream
fix and MobKit 0.7.36 adoption made this mitigation unnecessary.

## Ask 30 — live seed-window: bounded/summarized session projection for realtime opens — P2

**SHIPPED in meerkat 0.7.28** (`live/open.seed_max_chars` — windowed
projection preserving enabled root context, an affordable compaction
summary, the identity/tombstone/rewrite-generation/canonical-image
sidecars, with explicit degraded-continuity reporting). Meta adoption
(0.7.33): `live_open_config_for_session` delegates to
`realtime_projection_messages_with_window`; the 0.7.32 oldest-first clamp
stopgap is DELETED — the `seed_max_chars` wire params are unchanged.

Field (the downstream app's device client, 2026-07-10): the realtime provider adapter enforces a
65,536-token instruction cap, and `live/open` seeds the WHOLE projected
session (system prompt + full transcript projection). Long-lived members
with large durable histories cannot open a live channel at all once the
projection exceeds the cap.

Ask: a seed-window knob on the realtime open path — project a bounded
suffix (and/or a compaction-style summary head) of the session instead of
the full transcript, in the session-projection layer where
`realtime_projection_messages` lives, so the bound composes with the
user-content identity lane and exact-retry guards. meta stopgap (shipped
0.7.32): the gateway clamps the projected seed at open time — correct but
lossy; the principled summarize-then-seed belongs in core.

## Ask 31 — resume-provision rollback destroys the durable session it was resuming — P0

**CLOSED FOR TRACKING after meerkat 0.7.29** (PR #874,
changelog-silent). The rollback half shipped; the separately discovered
revival-flow acceptance was moved to ask 34 rather than counted twice.
- **Rollback half SHIPPED** (source-verified): `ProvisionSessionOrigin::{Fresh,
  ResumedDurable, RevivedRetired}` — rollback of a resumed durable session
  calls `restore_resumed_member` and returns it to durable idle, never
  archive. New Bug I destruction cannot occur.
- **Revival follow-up → ask 34, SHIPPED**: the 0.7.29 machinery existed
  (`load_revivable_retired_session` / `promote_revivable_retired_session` /
  `create_session_with_machine_archived_resume_authority`) but initially failed
  end-to-end with "generated MeerkatMachine did not grant active executor
  registration" — bindings/registration run against the still-Retired machine
  before the revival reset, and no upstream test drove the flow. Meerkat
  0.7.30 fixed the authority order and added upstream coverage; MobKit 0.7.37
  adopted it and un-ignored
  `identity_first_resume_revives_terminally_retired_runtime`. Existing victims
  no longer need the manual row-copy repair on those versions.
Meta keeps the destruction-detection probe as a regression tripwire.
Originally filed 2026-07-11 (Bug I). The single most destructive defect of the
downstream app's saga: on 0.7.33 boots, 1-2 slow-restoring eternal identities per boot
were terminally retired by their own failed resume, and repair boots re-rolled
the race instead of converging (14/16 when the operator stopped repairing).

Mechanism (code-verified in meerkat-mob 0.7.28):

1. A resume spawn fails at a LATE stage — in the field, supervisor-trust
   lifecycle persists failing with `SQLite error: database is locked` under
   concurrent restores (ask 33), or a spawn canceled by a queued retire
   (ask 32).
2. `MobActor::finalize_spawn_from_pending` (actor.rs:10838 and the sibling
   arms at 10789/10880/10938) compensates via `provision.rollback()`.
3. `PendingProvision::rollback` (provision_guard.rs:56) calls
   `provisioner.retire_member(&member_ref)` — "archiving the session" per its
   own doc comment.
4. `SessionBackend::retire_runtime_before_archive` (provisioner.rs:685)
   durably persists `runtime_state=retired` with the binding nulled.
5. Retire is terminal by design: every subsequent resume fails with
   `missing durable session snapshot` (actor.rs:9544). The identity's entire
   transcript is unreachable forever.

Forensic confirmation (the downstream app's server, store
`data/meta_state/5f68197796d9.damaged-bug-i/runtime.sqlite`): victims'
`runtime_states` rows read `{"record_version":2,"runtime_state":"retired",
"binding":{"agent_runtime_id":null,...}}` while their multi-hundred-MB
snapshots sit intact alongside. Gateway stderr shows the full chain, including
`archive compensation failed` arms where even the destruction itself hit the
locked store.

The rollback semantics are correct for a FRESH spawn (the provisioned session
is garbage without its member). They are catastrophic for a RESUME: the
provisioned session is the only copy of an eternal identity's history, and the
meta bridge's rejection contract ("durable session preserved, identity
degraded pending reconcile retry") is silently violated — the bridge REFUSES
fresh-spawn fallback precisely because it assumes the session survived.

Ask (both halves):
- **Non-destructive resume rollback**: `PendingProvision` (or the provision
  request) must distinguish provision-of-fresh-session from
  provision-of-resumed-durable-session; rollback of a resume returns the
  session to its pre-resume durable state (idle, binding intact or cleanly
  cleared) and NEVER archives/retires it.
- **Revive affordance**: an authority-level operation to return a
  retired-with-intact-snapshot session to `idle` (the manual row-copy repair,
  made legitimate). Without it, every past and future victim needs hand
  surgery on `runtime.sqlite`.

meta interim (0.7.34): post-rejection verification probes the session store
and logs continuity destruction explicitly instead of the false "durable
session preserved"; restore concurrency knob + collision-retry drain reduce
the two known triggers. meta CANNOT prevent the destruction — the rollback
runs inside the spawn path.

## Ask 32 — RetireAbsent leaves a queued retire that cancels the next spawn of the same id — P0

**SHIPPED in meerkat 0.7.29** (`ClassifyRetirePendingSpawnDisposition`:
public mob retire asks MobMachine for an incarnation-scoped pending-spawn
disposition; only the exact machine-authorized pending session can be
canceled, and an absent committed identity preserves a pending later
incarnation). MobKit 0.7.35 removed the 0.7.34 drain-poll + retry-once
workaround. Originally filed 2026-07-11 (Bug I secondary racer). Field trace (2ms apart,
two independent incidents captured):

```
WARN  resume_session hit a roster collision; retiring the stale member and retrying resume
       error=mob member already exists: mk--rt_cfamily-group_cmain_c0
WARN  meerkat_mob::runtime::actor: retire requested for unknown meerkat id; MobMachine accepted RetireAbsent
ERROR resume rejected ... error=internal error: spawn canceled for 'mk--rt_cfamily-group_cmain_c0': retire command received
```

The colliding roster entry is a Broken residue of an earlier rejected resume
(same boot); it exists in the MobMachine roster but has no live actor. The
bridge retires it; `retire()` returns Ok when the command is ACCEPTED
(MobMachine records RetireAbsent), but the command is still queued when the
resume retry arrives, so the retry spawn is canceled — and via ask 31 the
cancel's rollback then archives the durable session (the family-group victim's
next attempt logged `session not found`, then `missing durable session
snapshot` forever).

Ask: a retire resolved as RetireAbsent must be inert for later spawns of the
same identity (it retired nothing; there is nothing to cancel), or the machine
exposes an awaitable retire-drain so a caller can sequence
retire-then-respawn without racing the command queue.

meta interim (0.7.34): after a collision retire the bridge drain-polls the
roster (bounded 2s) before retrying, and treats `spawn canceled ... retire
command received` as retryable-once-after-drain. This narrows the window; it
cannot close it (the queue is unobservable from the handle surface).

## Ask 33 — SQLite writer contention under concurrent session restores — P1

**SHIPPED in meerkat 0.7.29** (PR #874, changelog-silent; source-verified:
`SQLITE_BUSY_TIMEOUT_MS` 5s→60s default sized for long WAL writer holds from
large snapshot commits, plus per-store `SqliteConnectionOptions.busy_timeout`).
`META_IDENTITY_RESTORE_CONCURRENCY` can return to its default (knob
retained). The snapshot-size defect initially observed here is now diagnosed
and tracked separately as ask 35; it does not reopen ask 33. Originally filed
2026-07-11 (Bug I trigger). At filing, meerkat-store set
`busy_timeout=5s` + WAL (sqlite_store.rs:24,113-114). A downstream app boot restores
16 identities with sessions up to 366MB; meta restores up to 4 concurrently
(#265). Multi-hundred-MB snapshot writes hold the WAL writer lock past 5s, and
every lifecycle persist that loses the wait surfaces raw `Store write failed:
SQLite error: database is locked` — in the field this hit supervisor-trust
revoke/install begin-lifecycle persists and recovery persists mid resume-spawn,
which then fed the ask 31 destructive rollback. Slow-restoring members are
victimized precisely in proportion to their restore duration.

Ask:
- lifecycle persists (small, critical, idempotent begin/commit rows) should
  retry bounded on SQLITE_BUSY instead of failing the whole spawn; and/or
- busy_timeout configurable per store open, sized for stores whose single
  writes can take tens of seconds.

Related defect, same store, observed 2026-07-10 and now tracked as ask 35:
domain:security's 366MB snapshot failed `runtime transcript rewrite snapshot
persistence failed: Store write failed: string or blob too big`. The apparent
per-value ceiling was the symptom; retained full transcript bodies in the
revision chain are the diagnosed root cause.

meta interim (0.7.34): `META_IDENTITY_RESTORE_CONCURRENCY` env knob
(clamped 1-16, default 4); `=1` serializes restores and removes meta's own
contribution to writer contention.

## Ask 34 — retired-session revival flow refuses executor registration — P1

**SHIPPED in Meerkat 0.7.30** (PR #879,
`9300fcf0df351125c6196d25a1cf97b8b26f3faa`) **and adopted in MobKit 0.7.37**
(PR #277, `5f22e445`). The fix promotes the durable session document,
synchronizes a genuinely archived live projection, resets the `Retired`
runtime, and only then attaches the executor. Failure after promotion restores
the exact `Archived` + `Retired` pair without leaking a live agent, executor,
runtime registration, or provisioner sidecar.

Reproduction (meta `identity_first_cold_restart_continuity.rs::
identity_first_resume_revives_terminally_retired_runtime`, landed `#[ignore]`d
as the acceptance criterion): boot 1 creates a durable member and delivers a
turn; a MOB-PLANE retire archives the session and durably writes
`runtime_state=retired` while the continuity record still binds the identity
(the exact Bug I terminal state); boot 2's ordinary resume then fails:

```
resume spawn: internal error: Input validation failed: generated
MeerkatMachine did not grant active executor registration
```

Progression evidence: before meta forwarded the revival seam through its
session-service wrappers, the same boot failed earlier with `missing durable
session snapshot` — so `load_revivable_retired_session` now finds the session
and the flow reaches session materialization, where executor registration is
staged against the still-Retired machine and the claim check refuses
(`stage_generated_executor_registration_claim`,
meerkat-runtime `meerkat_machine/mod.rs:1407`; `EnsureSessionWithExecutor`
leaves `registration_phase` non-Active for a Retired lifecycle).

Ordering hypothesis: `SessionBackend::provision_member` prepares local session
bindings (~line 2100) and creates the session (~2146) BEFORE the revival
promote + `reset_runtime` pair (~2302); the archived-resume authority create
exists but the machine is only reset to a registrable lifecycle after
creation. Either the revival branch must reset/authorize registration before
bindings/creation, or the registration claim must admit the archived-resume
authority.

Resolution: upstream now covers both explicit `Archived` + `Retired` revival
and the `Active` + `Retired` crash-recovery compatibility shape. Meta's
`identity_first_resume_revives_terminally_retired_runtime` acceptance test is
un-ignored and shipped in 0.7.37 against Meerkat 0.7.30. Bug I victims no
longer require the manual `runtime_states` row-copy repair on these versions.

## Ask 35 — quadratic mechanical transcript-head retention inflates long-lived snapshots — P0

**SHIPPED in Meerkat 0.7.30** (PR #878,
`d084689a84c27c0a6daefcd926952110a967abfd`). The released fix keeps genuine
audited rewrite endpoints plus the live head, compacts legacy mechanical
append-head chains on read/save, and routes typed synthetic-notice refreshes
outside the audited undo path. No Meta API adoption is required.

Field-reported reproduction (the downstream app's forensic measurement, 2026-07-12): the
895-message `domain:security` member has a roughly 2MB live transcript but 764
retained revisions. Its decoded `session_transcript_history_state_v1` is
roughly 1,005MB and dominates a 366MB+ persisted snapshot. Each revision has
shape `{created_at, messages, revision}`, where `messages` is another complete
copy of the transcript at that moment. Other long-lived members scale with
turn count (home-automation 213MB, network 154MB, parent-1 128MB), not with
live context size.

Mechanism (code-verified against Meerkat 0.7.29):

1. `meerkat-core::Session::commit_transcript_rewrite` initializes history by
   retaining the full parent and rewritten endpoint bodies for a genuine
   audited commit.
2. Once any history exists, ordinary message appends advance the history head
   by retaining another complete transcript body even though the audited
   `commits` list does not change. The field-matched regression needs only one
   genuine rewrite followed by 762 ordinary appends: together they yielded
   764 bodies.
3. Every mechanical head body grows with the live transcript. Retaining one
   full body per append therefore makes the history state
   O(appends × transcript), which is O(N²) in cumulative message mutations for
   a steadily growing conversation.
4. Transient/synthetic-notice replacement is another mechanical mutation and
   must not create a genuine audited commit, but rewrite-per-turn cleanup is
   not required to explain the field reproduction.
5. Context assembly reads only the live transcript. The model never consumes
   these mechanical head bodies. Genuine endpoint bodies support revision
   reads/restores plus strict lineage, integrity, and save validation.

Field-reported impact: multi-hundred-megabyte boundary writes hold the SQLite
writer lock, feeding `database is locked` failures and dead delivery surfaces;
serialized restores exceeded 900 seconds and hit boot initialization timeouts;
persisted stores grew toward SQLite's single-value ceiling. The earlier ask 33
timeout increase reduces contention fallout but cannot fix this source of the
writes.

Resolution at the owning seams (the shipped fix takes this shape):

- In `meerkat-core`, distinguish actual audited rewrite endpoints from
  mechanical append heads. Retain only bodies required by genuine rewrite
  commits plus the current live head; compact old append-head bodies on
  read/save.
- In `meerkat-core`, make typed transient/synthetic-notice replacement a
  mechanical message mutation so core agent-loop callers cannot accidentally
  mint audited undo points. No Meerkat-runtime API change is required.
- Compact legacy unbounded revision state on read/save without weakening
  revision-digest, parent-chain, recurrence, cycle, or persisted-MCP integrity
  checks.
- Preserve real operator/compaction rewrite endpoints as restorable audited
  history; the fix removes mechanical per-append retention rather than
  deleting the audited-restore feature.
- A future last-K/digest-only policy or blob externalization for genuine
  audited endpoints is a separate retention-policy decision: it would change
  which historical revisions remain directly restorable and is neither
  required nor implemented by this fix.

Acceptance:

- A field-matched 895-message/764-body regression scenario retains only the
  current head and genuine rewrite endpoint bodies (the 0.7.30 fix yields
  three bodies for one genuine rewrite), not one body per ordinary append.
- Routine typed synthetic-notice injection/removal creates no audited
  transcript revision.
- A genuine rewrite remains listable and restorable at both retained
  endpoints, with strict history-integrity checks intact.
- Legacy parentless `{created_at, messages, revision}` chains compact safely on
  the next persistence cycle.
- The first legacy load still pays one parse of the oversized stored state
  before read-time compaction. Subsequent serialization/persistence and cold
  resumes scale with the live transcript plus genuine audited rewrite
  endpoints, not with every append/message mutation times transcript size.
