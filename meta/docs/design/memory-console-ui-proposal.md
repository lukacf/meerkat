# Memory Console UI Proposal — The Ledger of What It Knows

**Status:** proposal · **Target:** meta console (`console/src`) + read-only `meta/memory/panel/*` RPC surface (`crates/meerkat-meta/src/http_console.rs`)
**Basis:** winning concept from a three-way design competition ("The Ledger of What It Knows", state-first Memory Explorer), with eight judge-required grafts from the two runners-up. Judge scores in Appendix A.
**Feasibility note:** data-source claims below refer to the investigated `memory-system` worktree (post-`caf18995`); corrections from earlier investigation snapshots are called out inline — notably that snapshot's event vocabulary has **17** `memory.*` subtypes (`memory.quarantine.release_blocked` is new, `events.rs:169`) and `DreamVerdicts` carries **14** counters (`quarantine_release_blocked`, `steward.rs:804`). The budget limits are **20 KiB per assembly** (`MAX_INJECTED_ASSEMBLY_BYTES = 20 * 1024`) and **60 KiB for the session-keyed per-turn counter** (`MAX_INJECTED_SESSION_BYTES = 60 * 1024`, `coordinator.rs:46-47`), not a durable session-lifetime total. That counter resets on compaction, coordinator/process reset, or bounded-map clearing. Injection-ledger rows have **`session_key = NULL` for build-surface assembly** (`sqlite_store.rs:121-128`); turn rows carry no reset epoch, so repeated session-key groups alone cannot prove a dedup violation.

**Status boundary:** “Today” and “NEW” below preserve that proposal snapshot,
not a current shipped-feature inventory (see [the current memory guide](../concepts/memory.mdx#console-memory-panel)).
Mockups illustrate intended populated views; missing evidence must still produce
`UNVERIFIABLE`, not the mockup's sample green verdicts.

---

## 1. Purpose

The memory system (docs/design/agent-memory-architecture.md, rev 3) is a five-stage cognitive loop — observe → distill → store → select/inject → dream — with a trust lattice, a taint firewall, gated promotions, and hard budgets. It is also, deliberately, mostly invisible: process events ride a lossy in-memory ring (1024/identity, 4096 total, `console_events.rs:14-15`), and several enforcement mechanisms live only in process memory by design.

This UI serves two modes, at two levels:

- **Understanding** — *what does this agent know? what does this mob know?* An operator should be able to open the console and read an agent's memory the way they read its transcript: what is stored, how much it is trusted, how it got there, and what the agent actually sees on each build.
- **Verification** — *is the architecture doing what it claims?* Echo-safety, the taint wall, the trust lattice, recall utility, dream honesty, and store floors are falsifiable claims. The UI renders them as standing verdicts with one-click paths to the rows that would falsify them.

Both modes must work for a **single agent** (identity scope) and for a **mob** (mob/realm scope, promotion pipeline, per-member composition), and both must respect the console's constitution: **adapters own all formatting** (no raw envelopes reach React — `describeMemoryTimelineEvent` or sibling pure helpers), **nav and affordances come from the server-projected experience** (`experience.memory` in `console_ingress.rs:288-313`) **and per-row ABAC** (`agent.memory.read`, `mob.memory.read`, `operator.memory.read`, `memory.quarantine.review` — `access/model.rs:19-37`), and **view-level config decides presentation only, never authorization**.

## 2. The organizing concept

**The store is the primary object.** Every screen answers "what does this agent/mob know right now, how much does it trust each piece, and how did that piece come to be here." Process — distill, inject, dream, supersede, quarantine — is never a separate feed the operator correlates by hand; it arrives as **biography attached to state**: each record carries its birth (evidence), its lineage (supersede lane), its life (injections, usage), and the dream runs that touched it.

This inverts the observability problem. Instead of chasing 17 ephemeral `memory.*` event types through a 4096-frame ring, the operator anchors on durable SQLite state (records, chains, injection ledger, audit rows, proposals, pending queues) and uses live SSE events only as freshness signals and pivot points.

Two grafts from the runner-up concepts complete the design:

- **From the Claims Board (verification-first):** a **verdict-tile strip** — `HOLDING / DEGRADED / VIOLATED / UNVERIFIABLE` — mounted as the Holdings header. Each tile is a door into the Ledger view that holds its evidence. `UNVERIFIABLE` names the exact missing surface; a tile the principal cannot evidence renders **"no grant"**, never green. This converts the Ledger's manual verification story into standing proofs without changing its navigation.
- **From the Flight Recorder (process-first):** the **"state here" pivot** — every live `memory.*` signal (SignalsRail, timeline rows) carries one-click pivots into state (record Biography, Pipeline stage, Holdings scope row, Health snapshot) — plus **live-follow discipline** for any live strip (pause-on-scroll, "N behind / jump to live", dedupe across `snapshot_complete` replays, and a hairline **"ring history starts here"** seam marker so "no events" is never mistaken for "nothing happened").

The Flight Recorder's full lane/scrub **Loop Trace** view is deliberately **reserved for phase 3** (§7): it only becomes honest once a durable trace RPC and persisted DreamRun detail exist. The Ledger's event↔Biography links are shaped so the trace can later reuse them verbatim.

### Fit with the existing console

The proposal extends the shipped P3b `MemoryPanel` (`console/src/panels/MemoryPanel.tsx`) rather than introducing a new navigation paradigm. Today's tab union is `records | quarantine | dreams` (`MemoryPanel.tsx:38`, quarantine tab gated on `canReviewQuarantine`, `:421-423`). The proposal grows it to:

```
[Holdings] [Records] [Knowledge] [Pipeline] [Dreams]        (+ [Trace], phase 3)
```

The quarantine tab **folds into Pipeline** (its sections remain gated per-row on `memory.quarantine.review`, exactly as `handle_memory_panel_quarantine` enforces today, `http_console.rs:2104`). The Memory nav entry continues to appear **only** from `experience.memory.can_read` (`ConsoleApp.tsx` nav gating; "gated server-side per principal, never by view config"). `console_config.rs` visibility lists may hide the control; they can never reveal it.

Every new RPC pays the CI-enforced process tax: a `CONSOLE_RPC_METHODS` constant (`packages/console-core/src/contract.ts`), a `docs/rct/console-rest-sse-contract-v0.5.0.json` entry, an `http_console.rs` dispatch arm visible to the `contract.test.ts` regex parser (bijection both ways), an entry gate in `console_rpc_access_requirements` plus per-row filtering per `memory_panel_record_visible` (`http_console.rs:1821`) / `memory_panel_promotion_visible` (`:1857`), and the embedded-bundle rebuild (`console/build.cjs` → `console-dist` → `check-embedded-freshness.cjs`).

## 3. Views

### 3.1 Holdings — store overview + verdict strip (front door)

**Purpose.** One glance answers "what does this mob know, where is it concentrated, is anything unhealthy, and are the architecture's claims holding right now." Scope rows (identity > mob > operator > realm, reusing `groupRecordsByScope` ranking, `MemoryPanel.tsx:96`) show status counts, byte pressure against the floors (4,000 records / 32MB per scope, `sqlite_store.rs:46-48`), trust mix, and in-transit badges. The grafted verdict-tile strip sits above it.

```
┌─ Memory · realm: example  ───────────────────────────────────────────────────┐
│ [Holdings] [Records] [Knowledge] [Pipeline] [Dreams]            ⟳ Refresh    │
├──────────────────────────────────────────────────────────────────────────────┤
│ ECHO-SAFETY   TAINT WALL    LATTICE       RECALL        DREAMS    STORE FLOOR│
│ HOLDING       HOLDING       HOLDING       DEGRADED      HOLDING   HOLDING    │
│ 0 dup-inject  4 quarantined 0 violations  61 dead       last run  12% floor  │
│ 0 build-      0 laundered   214/214       weight (28%)  3h ago    214/4000   │
│  overlap ⚠1   1 released     checked                    0 skips   3.1/32MB   │
├──────────────────────────────────────────────────────────────────────────────┤
│ SCOPE             ACTIVE  QUAR  SUPER  TOMB   BYTES   FLOOR                  │
│ ▸ identity:ada      41     1     12     3    412KB  ▓▓░░░░ 12%               │
│ ▸ identity:bob      17     0      2     0     88KB  ▓░░░░░  4%               │
│ ▸ mob:research      23     2      5     1    301KB  ▓▓░░░░  9%               │
│ ▸ operator           6     0      0     0     14KB  ░░░░░░  1%  (or: no grant)│
│ ▸ realm              9     0      1     0     52KB  ░░░░░░  2%               │
├──────────────────────────────────────────────────────────────────────────────┤
│ LAST DREAM 2h ago · 14 ops · ⚠1 quarantined      PENDING GATE 2              │
│ QUAR QUEUE 3      PROPOSALS 4 pending / 1 held (taint)                       │
│ HEALTH (live snapshot — resets on restart)  taint · budgets · cursors  [▸]   │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Data sources.**
- **NEW RPC `meta/memory/panel/overview`** — thin handler over `SqliteAgentMemoryStore::scope_overview()` (`sqlite_store.rs:1379`, fully built, doc-comment already names the panel) + `scope_floors()` (`:1373`). Near-zero cost.
- **Today:** `meta/memory/panel/dreams` (last-dream strip); `meta/memory/panel/quarantine` (queue + pending-gate counts, only when `experience.memory.can_review_quarantine`).
- **NEW RPC `meta/memory/panel/proposals`** over `pending_proposals()` (`sqlite_store.rs:1439`) — proposals counters incl. propose-time taint.
- **Verdict tiles:** each computed by the corresponding view's logic (§5); tiles missing a surface render `UNVERIFIABLE — needs <RPC>`; tiles the principal cannot evidence render **no grant**.
- **Live:** `memory.*` SSE frames trigger `refreshMemoryData` while a memory panel is docked — one client-only condition in `handleLiveFrame` (`ConsoleApp.tsx:2338`; `memory.*` frames already reach `activityRef` since `ACTIVITY_SKIP_EVENTS` omits them, `:484`). **As-built correction (55d1c442):** the
client-side path was indeed fine, but `_system`-attributed `memory.*` frames
were dropped SERVER-side by the roster-visibility gate
(`frame_is_visible_cached`) and required an exact `_system` exemption plus a
no-namespacing guard in `frame_from_console_event` — the feasibility claim
here was verified against the client only.

**Interactions.** Scope row → Records pre-filtered to that scope (the records RPC already accepts `scope/identity/scope_key/status` params that `refreshMemoryData` never passes, `http_console.rs:1912`). Verdict tile → its evidence view (§5). QUAR/GATE/PROPOSALS badges → Pipeline filtered to that stage. Trust-mix segment → Records filtered by trust (client-side; trust is on every row). Floor bar past 80% → warning tone (gpolicy chip vocabulary). Health footer expands (§3.6). Scope rows the principal cannot read render access-denied tone, mirroring the per-section `-32030` tolerance `refreshMemoryData` already implements (`ConsoleApp.tsx:2032`).

### 3.2 Records — master list + Record Biography detail pane

**Purpose.** The heart of the state-first design. Left: body-free record rows (list rows are body-free by design; `body_bytes` stands in, `http_console.rs:1879`) with a scope/status/kind/trust filter bar and keyset load-more. Right: the **Biography** — the record's whole life as attached history: **BORN** (author + evidence with live transcript click-through), **LINEAGE** (GitKraken-style single vertical supersede lane, dimmed ancestors, trust chip per node so taint flows visibly along the chain), **LIFE** (usage counters + per-record injection ledger), **DREAMS** (which runs touched it and what they did). Supersede-not-delete becomes navigable history — no trash icons.

A grafted **Recall Utility mode** (from the Claims Board) mounts as a sort/flag toggle on this list rather than a separate tab: rank by utility ratio (`judged_useful / injected`), show bytes-spent, flag `DEAD` rows, and offer "pin flagged ids & diff against the next dream's ops" (§5.4).

```
┌─ Records ─ sort:[recency|utility] ──┬─ Biography · rec_7f3a ────────────┐
│ scope[mob:research▾] status[all▾]   │ "API uses keyset pagination"      │
│ kind[all▾] trust[all▾]              │ gotcha · agent_observed · ACTIVE  │
│─────────────────────────────────────│ ⚑ ever-quarantined                │
│ ● rec_7f3a gotcha observed  2d      │───────────────────────────────────│
│ ● rec_91bc fact   verified  5d      │ BODY 1.2KB           [copy JSON]  │
│ ○ rec_004d fact   observed  9d ᴰ    │───────────────────────────────────│
│ ⚠ rec_aa21 proc  untrusted 1h Q     │ LINEAGE                           │
│ ● rec_9c3  ref    observed 30d DEAD │  ● rec_7f3a observed  now         │
│ [load more · cursor ms:id]          │  ○ rec_51e0 observed  6d  ᴰrun_88 │
│                                     │  ○ rec_0b77 untrusted 21d ⚑       │
│ utility mode adds:                  │ BORN distiller run d_42           │
│  inj recall useful ratio bytes flag │  evidence sess_9 gen3 [12–19]     │
│                                     │  → open timeline excerpt          │
│                                     │ LIFE inj×14 recall×2 useful×5     │
│                                     │  last injected 2h ago (build)     │
│                                     │ DREAMS run_88 set_rank ·          │
│                                     │  run_84 supersede← rec_51e0       │
└─────────────────────────────────────┴───────────────────────────────────┘
```

**Data sources.**
- **Today:** `meta/memory/panel/records` — the filter bar maps 1:1 onto its unused `scope/identity/scope_key/status/limit/cursor` params; load-more uses `next_cursor` (**single-realm only**, `http_console.rs` keyset constraint — the realm selector makes paging honest). Utility mode is a pure client-side sort: `UsageStats {injected_count, explicit_recall_count, judged_useful_count, last_*}` and `working_set_rank` already serialize on every row (`records.rs`, `types.ts:363-504`).
- **Today:** `meta/memory/panel/record` — record with body + supersede chain (≤32, `MEMORY_PANEL_CHAIN_MAX`, `http_console.rs:1753`) + per-record injections (≤50, via `injection_log_for_record`, `sqlite_store.rs:2120`). LINEAGE and LIFE render from this response with zero backend change.
- **Today:** `meta/console/query_timeline` (contracted, `contract.ts:23`) — a best-effort projected timeline excerpt linked from `MemoryEvidenceRef {session_id, generation, revision, range}` (`types.ts:396-401`). This client-only click-through can be approximate or unavailable: the RPC accepts no transcript revision, generation, or source-message-range selector. A captured revision reference does not make this endpoint revision-aware. Exact historical evidence resolution, including after transcript rewrites, requires a revision-aware transcript read seam outside this client-only path.
- **NEW fields on `panel/record`:** `ever_quarantined` + `rank_set_at_ms` (durable columns, `sqlite_store.rs:72/:82`, currently dropped in row→record deserialization) — `ever_quarantined` is the ⚑ chip explaining permanently-ceilinged trust (§10.2 marker; note it is **materialized transitively** at write time via the one-level ancestor check, `sqlite_store.rs:2773-2788`).
- **NEW:** `history[]` array on `panel/record` — per-record audit rows (audit table keys by `memory_id` with `op_kind/detail/applied_at_ms`, `sqlite_store.rs:102`; `dream_history` proves the read path). Makes the DREAMS section exact instead of a lossy client-side join through `MemoryDreamRun.memory_ids` (≤12 samples).
- **Approximation until `panel/injections` lands:** bytes-spent = `injected_count × body_bytes`, labeled as approximate.
- `packages/console-components/src/copy-button.tsx` — boundary-clean copy-as-JSON.

**Interactions.** Row → Biography in the persistent right pane via existing `loadMemoryRecordDetail` (`ConsoleApp.tsx:2092`); list context never lost. Lineage node → that Biography in place, breadcrumb back-stack. "Open timeline excerpt" → a best-effort conversation window, labeled approximate; if unavailable, retain the evidence reference as text rather than claiming an exact transcript range. DREAMS entry → Dreams view scrolled to that run. ⚑ chip → tooltip names the quarantine reason; click-through to Pipeline if still quarantined. `DEAD` flag → filters to dead weight; **watch-next-dream** pins the flagged ids and diffs them against the next dream's ops. Filter changes re-issue the records RPC; `-32030` tolerated per section exactly as today.

### 3.3 Knowledge Lens — what one agent (or the mob) actually sees

**Purpose.** State-first answer to the injection question: not "what events fired" but "what is in this agent's head right now." Composition strip shows the scope union (Identity ∪ Mob ∪ Operator ∪ Realm, `coordinator.rs` scope composition) as clickable segments; **AS-INJECTED** shows the exact composed memory block from the last build (the Letta "exactly what the model sees" borrow — the single highest-trust surface); **INJECTION HISTORY** shows the per-identity ledger with a **possible-repeat diagnostic** and **consecutive-build overlap annotation** — useful investigation paths for the historical ~18.5KB/turn duplication defect, not conclusive violations without reset-boundary evidence. Switching the selector to a mob swaps identity sections for mob+realm scope plus a per-member summary table.

```
┌─ Knowledge · [identity: ada ▾]  (or [mob: research ▾]) ─────────────────┐
│ COMPOSITION identity(41) ∪ mob:research(23) ∪ operator(6) ∪ realm(9)    │
│ TURN BUDGET ▓▓░░░░░ 9.1KiB / 60KiB · per assembly ≤20KiB             │
│─────────────────────────────────────────────────────────────────────────│
│ AS-INJECTED (last build · 2h ago)     │ INJECTION HISTORY (ada)          │
│ ┌ index ──────────────────────────┐   │ 14:02 build 12 records           │
│ │ [identity] prefers dark mode    │   │  ⚠ same 12 ids as previous build │
│ │ [mob] api uses keyset paging    │   │ 13:10 turn   3 records           │
│ │ [realm] no friday deploys       │   │ 12:44 turn   3 records ⚠ repeat  │
│ └─────────────────────────────────┘   │   rec_7f3a again in sess_9       │
│ each line → opens Record Biography    │ 11:58 build 12 records           │
│ (phase 3 until panel/context lands;   │ [surface: all▾][session: s-9▾]   │
│  until then: honest placeholder)      │ ── ring history starts here ──   │
└───────────────────────────────────────┴──────────────────────────────────┘
```

**Data sources.**
- **Today:** `meta/memory/panel/records` — composition counts via one call per scope section (`scope=identity&identity=ada`; `scope=mob&scope_key=research`; `scope=operator`; `scope=realm`), tolerating `-32030` per section since `experience.memory.can_read` is deliberately coarse.
- **NEW RPC `meta/memory/panel/injections`** — realm-wide injection ledger over `injection_log()` (`sqlite_store.rs:994`, already consumed by the steward's usage audit) with proposed `identity/session_key/surface` filters. Rows carry `record_id/identity/session_key/surface(build|turn)/at_ms`. **Feasibility correction:** (a) group *turn* rows by `(record_id, identity, session_key)` to inspect **possible repeats**, not to declare every count > 1 a violation. Compaction deliberately resets dedup without changing the session key; coordinator/process reset and bounded-map clearing also lose that state. A violation requires both injections to belong to the same uninterrupted dedup interval. Existing rows have no reset epoch, so that verdict is **UNVERIFIABLE — missing reset-boundary evidence**. (b) **Cross-build overlap** remains a separate diagnostic: build rows have `session_key=NULL` (`sqlite_store.rs:121-128`); compare record-id sets of consecutive builds per identity and annotate `⚠ same N ids as previous build`, not a turn-dedup violation. Nonce and stage_token never appear (commit capabilities: `coordinator.rs:399-405`, `http_console.rs:2137`).
- **NEW RPC `meta/memory/panel/context`** (phase 3) — last-rendered composed injection block per identity from the `RecallCoordinator` (`assemble_build_injection` already produces block + provenance labels); nonce stripped; per-turn section honestly labeled **OFF** until the echo-safe delivery default flips (meerkat 0.7.18 follow-up).
- **Budget gauge:** the session-keyed per-turn `injected_bytes` counter vs `MAX_INJECTED_SESSION_BYTES` (**60 KiB**), with the separate **20 KiB per-assembly** limit (`MAX_INJECTED_ASSEMBLY_BYTES`) shown alongside it. The proposed health-snapshot RPC (§3.6) must label this counter "current accounting interval": it is monotone only within one uninterrupted interval, resets on compaction, and can be lost on coordinator/process reset or bounded-map clearing. Build assembly has its own 20 KiB cap and does not add to this per-turn counter; the gauge is not a durable lifetime total.

**Interactions.** Selector swaps the lens; mob mode adds a member table (name · identity-scope count · last injection). Composition segment → Records filtered to that scope. AS-INJECTED line → Record Biography. **⚠ possible-repeat row → expands both ledger rows side by side**, with the missing reset-boundary evidence named; only a proven same-interval duplicate earns a violation badge. The echo-safety tile must not infer HOLDING merely from a repeat count in a partial ledger window. Budget gauge > 80% → warning tone. The history strip follows **live-follow discipline** (pause-on-scroll, "N behind / jump to live", `snapshot_complete` dedupe) and renders the hairline **ring-seam marker** where durable ledger rows end and live-only knowledge begins.

### 3.4 Pipeline — knowledge in transit (proposals → gate → committed, plus quarantine)

**Purpose.** State that hasn't finished becoming knowledge, as a left-to-right conveyor with the **reason rendered at the decision point** (the compliance-UX finding: a bare approve/reject queue divorced from rationale destroys operator trust). Proposals show propose-time taint capture; pending gated promotions show rationale and gate latency; quarantined records show taint source and evidence window inline. **Deliberately read-only** — verdicts ride the existing gating flow (`meta/gating/decide`), so every actionable row deep-links into `GatingInboxPanel` rather than growing a parallel decision surface (`mob.memory.propose/commit` remain reserved, unmapped actions).

```
┌─ Pipeline ──────────────────────────────────────────────────────────┐
│ PROPOSED(4) ─▶ HELD(1) ─▶ PENDING GATE(2) ─▶ COMMITTED · QUAR(3)    │
│──────────────────────────────────────────────────────────────────── │
│ ⚠ prop_31 "auth token rotates hourly"   author distiller d_51       │
│    taint@propose: untrusted-mcp (sess_12) → HELD                    │
│ ⧖ pend_09 rec_aa21 → mob:research  "seen 3× across members"         │
│    waiting 41m · → decide in Gating inbox                           │
│ ✓ pend_04 rec_91bc → mob:research  approved · latency 12m           │
│──────────────────────────────────────────────────────────────────── │
│ QUARANTINE (reviewer-only)                                          │
│ 🛑 rec_aa21 · reason: tainted evidence (mcp source)                 │
│    evidence sess_12 gen2 [8–9] → open transcript                    │
│ 🚫 release blocked: content matches secret pattern (§10.4)          │
│    verdict rides the steward dream + gating flow — not here         │
└──────────────────────────────────────────────────────────────────── ┘
```

**Data sources.**
- **NEW RPC `meta/memory/panel/proposals`** — `pending_proposals()` (`sqlite_store.rs:1439`): `proposal_id, scope, record JSON, author, status pending|held|accepted|rejected, created_at_ms`, **propose-time taint** (captured durably at propose time precisely because tracker state is in-memory, `sqlite_store.rs:99`). Row visibility mirrors `memory_panel_promotion_visible` (unknown scope kinds denied).
- **Today:** `meta/memory/panel/quarantine` — quarantined records (reviewer-gated per row) + `pending_promotions` (stage_token never surfaced).
- **NEW fields on `panel/quarantine`:** `resolved_at_ms` + resolved `pending_promotions` rows (column exists, `sqlite_store.rs:157`) — the ✓ lane's gate-decision latency and history.
- **Today:** `meta/gating/pending` + `meta/gating/audit` (`ACTION_GATING_VIEW`) — join `pending_id` → gating entry for the deep link and decider identity.
- **Live (refresh triggers only, formatted via `describeMemoryTimelineEvent`):** `memory.promotion.pending_gate`, `memory.record.promoted`, `memory.quarantine.verdict`, `memory.write.quarantined`, and — **new since the investigations** — `memory.quarantine.release_blocked` (`events.rs:169`): a dream's release/promotion verdict blocked pre-staging because the record content matches a §10.4 secret pattern class. Rendered as the 🚫 row. **Client follow-up:** this subtype currently falls to the humanized-unknown fallback — add explicit copy to **both** `describeMemoryTimelineEvent` copies (`packages/console-core/src/adapters.ts:439` and `console/src/lib/adapters.ts:435`), `adapters.test.ts`, and SignalsRail classification (warning severity), which today drops it (`SignalsRail.tsx:331-353`).

**Interactions.** Stage header → filter. "→ decide in Gating inbox" → `dock.openTarget` on the gating panel at that pending entry; the decision itself uses `gating.decide`, untouched. Quarantined record → Biography with reason + ⚑ chip; "open transcript" uses `query_timeline` for a best-effort projected excerpt, explicitly approximate or unavailable rather than guaranteed to contain the tainted source turn (§3.2). ✓ row → decision, decider (gating audit), latency. Quarantine section renders only under `memory.quarantine.review`; whole tab tolerates `-32030`.

### 3.5 Dreams — the ledger of state mutations

**Purpose.** Dreams framed state-first: every mutation the steward committed against the store, plus its verdict sheet. Collapsible run cards (AgentPrism progressive disclosure — collapse repetitive rank ops, red-flag quarantined ops); every touched `memory_id` links into a Biography, closing the Marquez-style node→run→node loop.

```
┌─ Dreams · realm: example  ──────────────────────────────────────────┐
│ ▾ run_88  today 04:00 · 14 ops · ⚠1 quarantined      ● dreaming now │
│    ├ orient       ok                                                │
│    ├ gather       31 candidates                                     │
│    ├ consolidate  2 supersede · 1 contradiction emitted             │
│    └ prune        9 set_rank (collapsed) · 1 tombstone              │
│    verdicts props 3✓1✗0⏸2⛭ · quar 1↩0†2⏸0⛭ 🚫1 blocked ·           │
│             usage 4 load-bearing / 14 dead-weight · harvests 1      │
│    touched rec_7f3a rec_91bc rec_0b77 +9 more                       │
│ ▸ run_84  yest 22:00 · skipped: min_signals (2 < 3)     (live-only) │
│ ▸ run_83  yest 16:00 · 6 ops                                        │
└─────────────────────────────────────────────────────────────────────┘
```

**Data sources.**
- **Today:** `meta/memory/panel/dreams` (`http_console.rs:2176`) — `run_id`, timestamps, ops, `op_kinds` histogram, `quarantined_ops`, `memory_ids` (≤12 sample), `rationales` (≤6 sample), reconstructed from durable audit rows (5,000-row scan cap). Enough for the collapsed card + badges today.
- **NEW persistence:** DreamRun detail — `phases[(name, outcome)]`, **14** `DreamVerdicts` counters (incl. the new `quarantine_release_blocked`, `steward.rs:793-810`), `skips` — written as one audit/`dream_runs` row at commit time and surfaced on `panel/dreams`. Today the detail exists **only** in the ephemeral `memory.dream.completed` event (`DreamRun::detail()`, `steward.rs:825-846`) and dies with the ring. The single biggest steward-observability gap; the phase tree, verdict line, and durable SKIPPED rows all depend on it.
- **Live:** `memory.dream.started/completed/skipped` — "dreaming now" pulse and opportunistic verdict cache for runs caught live (cache labeled cached-vs-durable).

**Interactions.** Run header → expand/collapse phase tree; collapsed by default; **virtualized list** per docs/archive/design/console-timeline-scalability-plan-2026-05.md (must not break at 50 runs). Touched `memory_id` → Biography via `loadMemoryRecordDetail` verbatim (the investigations flag this as the zero-backend cross-nav win - `memory_ids[]` is already returned and rendered as text only). Supersede op → LINEAGE lane of the survivor. Skip reason → tooltip (min_signals, `runs_per_day` budget) linking to the Health snapshot. SignalsRail memory signals gain `onSelect` → dock opens the Memory panel at the relevant view (**"state here" graft** - rail frames already carry record ids for `record.promoted`/`quarantine.verdict`).

### 3.6 Health strip — the in-memory truth (taint · budgets · cursors · harvest queue)

**Purpose.** A slim, always-honest strip (Holdings footer, expandable) exposing state that is in-memory **by design** and therefore invisible after the fact: session taint per identity, background-budget windows, distiller cursors, plus the durable exit-interview queue. Answers the three questions currently answerable only by catching transition events live in the lossy ring: *why was this run skipped, why did this write quarantine, why did nothing distill.*

```
┌─ Health (live snapshot · not durable) ─────────────────────────────┐
│ TAINT   ada: clean · bob: ⚠ tainted (mcp, 14:02, session-sticky)   │
│ BUDGET  distiller 7/12·hr · steward 1/4·day · hygienist 0/2·day    │
│ CURSORS ada sess_9: distilled→gen3 · 2 interactions pending        │
│ HARVEST queue: bob (retired 3h, cause: retire) awaiting dream      │
└────────────────────────────────────────────────────────────────────┘
```

**Data sources.**
- **NEW RPC `meta/memory/panel/health`** — snapshot serialization of: `SessionTaintTracker` maps (`current_session`/`tainted`/`reset_boundaries`, `taint.rs:203-227`), `BackgroundBudget` used/cap per realm-stage (`guards.rs:61`, deny reasons `WindowExhausted{used,cap} | ConcurrencyCeiling{cap}`, `:92-93`), distiller `WindowState` cursors (`distiller.rs`), steward signal counter, and the coordinator's session-keyed per-turn `injected_bytes` vs **60 KiB** cap (separate from **20 KiB per assembly**). Explicitly labeled **"live snapshot — resets on restart"**; injection accounting also resets on compaction or bounded-map clearing, excludes build assemblies, and is not globally monotone or a durable session-lifetime total. Durable re-derivation is impossible by design (that is exactly why propose-time taint is captured durably). Read-only; must never surface the envelope nonce or stage tokens.
- **NEW RPC `meta/memory/panel/harvests`** (or a field on `/health`) — `pending_harvests()` (`sqlite_store.rs:1653`): retired identities awaiting exit-interview, cause, age. Currently invisible until `memory.harvest.completed` fires — which is exactly when it stops mattering.
- **Live:** `memory.taint.transition` / `memory.budget.denied` / `memory.distill.timed_out` flip the strip between snapshots.

**Interactions.** Tainted identity → explanation card: source, when, the session-sticky rule (retained across rotation for the evidence gate); link to that identity's held proposals in Pipeline. Budget meter at cap → last denial reason if still in the ring, else "window resets at T". Harvest row → the retired member's identity-scope records in Records (what the exit interview will consolidate).

### 3.7 Reserved: Trace tab (phase 3)

The Flight Recorder's Loop Trace (per-agent lanes, mob grouping toggle, dream spans, scrub-with-seam) is reserved as a future **[Trace]** tab. It becomes honest only once (a) the durable trace RPC composing audit + injections + proposals + pending_* into one time-ordered stream exists and (b) DreamRun detail is persisted. Until then, the Ledger's event→Biography pivots and Biography→Dreams links are kept bidirectional and id-addressed so the trace can later reuse them verbatim.

## 4. The follow-a-fact walkthrough

The operator tells agent **ada**: *"we use keyset pagination, cursors are ms:id."*

1. **TURN** — nothing stored yet. **Health strip** (§3.6) shows ada's distiller cursor at `sess_9: 2 interactions pending` (`min_interactions` default 3) — the *absence* is explained, not mysterious.
2. **DISTILLER** — on the third interaction the distiller fires. In the proposal snapshot this hop is visible live **only on failure** (`memory.distill.timed_out`, `memory.budget.denied`); the **proposed phase-2 event `memory.distill.completed`** (§6, graft) would close the blind spot. When extraction produces successful ordinary writes, they appear directly as identity-scope **Records** with Distiller provenance on refresh; write-gated results appear as quarantined records in the reviewer-visible quarantine surface. No-op extraction creates neither a record nor a proposal. A pending mob/operator promotion proposal is a distinct action, not an automatic extraction hop, so this step has no `taint@propose` queue row.
3. **RECORD** — **Records → Biography** (§3.2): BORN distiller run d_42, evidence `sess_9 gen3 [12–19]`, initial trust `agent_observed`. Clicking the evidence via `query_timeline` opens a best-effort projected excerpt, labeled approximate or unavailable. The exact revision-pinned birth window remains the goal, but needs the revision-aware transcript read seam described in §3.2.
4. **SELECTION** — on ada's next build, the **Knowledge Lens** (§3.3) composition includes it; once `panel/context` lands, the AS-INJECTED block shows `[identity] api uses keyset pagination` verbatim.
5. **INJECTION** — **Knowledge Lens INJECTION HISTORY** shows the ledger row (build, +1.2KB); the record's **Biography LIFE** section ticks `inj×1, surface=build`. Consecutive-build overlap is an annotation, not a turn-dedup violation. Repeated turn rows warrant investigation; without evidence of one uninterrupted dedup interval, the violation verdict stays `UNVERIFIABLE`.
6. **DREAM** — at cadence, **Dreams** (§3.5) pulses "dreaming now", then run_88's card lands: consolidate shows one supersede — the steward merged the fact with an older, vaguer `rec_51e0`. The **Biography DREAMS** section lists `run_88 supersede`.
7. **SUPERSEDE** — the **Biography LINEAGE** lane now shows `rec_7f3a ●` current above `rec_51e0 ○` dimmed, annotated with the run id. Supersede-not-delete rendered as navigable history.

**The same loop at mob scale:** three research-mob members independently learn the fact → **Holdings** shows identity counts rising → the next dream's gather phase spots the repetition and **stages a promotion** to `mob:research` → **Pipeline** shows `pend_09 … "seen 3× across members"` in PENDING GATE and the SignalsRail signal's `onSelect` opens that exact row → the operator decides in **GatingInbox** (normal `gating.decide`) → `memory.record.promoted {gated:true}` fires → Pipeline's ✓ lane records the 12m gate latency → Holdings' mob count increments → every member's **Knowledge Lens** now carries the fact under `[mob]`. One fact, promoted from private observation to mob knowledge, chain of custody inspectable at each hop.

## 5. Verification widgets

The verdict strip (§3.1) mounts six standing proofs. Vocabulary: `HOLDING / DEGRADED / VIOLATED / UNVERIFIABLE(names the missing surface)` — plus **no grant** when the principal cannot evidence a tile (never green by default). Every green verdict is one click from the rows that would have made it red.

| Tile | Architecture claim proved | How |
|---|---|---|
| **ECHO-SAFETY** | §9.1: turn dedup holds within an uninterrupted interval; assembly and tracked turn accounting stay under their separate budgets; indexing excludes injected context | (a) Group *turn* rows by `(record_id, identity, session_key)` for **possible-repeat diagnostics**. A duplicate violates dedup only within the same uninterrupted interval; compaction, coordinator/process reset, and bounded-map clearing are valid reset boundaries. Existing rows lack an epoch, so the dedup proof stays **UNVERIFIABLE — missing reset-boundary evidence**, even after `panel/injections` lands. (b) Consecutive-build record-id overlap is a separate annotation, not a turn-dedup violation (`session_key=NULL` on builds). (c) Budget: **20 KiB per assembly**, **60 KiB for session-keyed per-turn `injected_bytes` between resets**; the counter excludes builds and is monotone only within one uninterrupted accounting interval, not over a durable session lifetime. (d) Re-entry investigation may compare Biography with timeline excerpts, but an approximate projected window cannot prove exact source-range overlap or indexing exclusion; exact evidence needs the revision-aware read seam (§3.2). Missing surfaces or interval evidence keep the corresponding proof `UNVERIFIABLE`. |
| **TAINT WALL** | §10.1/10.2: tainted ancestry cannot exceed its ceiling; taint is session-sticky; the marker is durable | Three durable seams: (1) proposals carry **propose-time taint** (immune to tracker restarts); (2) quarantined records carry their reason; (3) `ever_quarantined` permanently marks anything ever caught (needs the field surfaced). Live pairing `taint.transition → write.quarantined` shows the wall catching in real time; session rotation must keep taint until `reset_boundary`. **Laundering check:** inspect the record and walk transitive `supersedes` and `derived_from` ancestry. An untrusted, quarantined, or ever-quarantined record anywhere on that walk imposes an **`agent_observed` ceiling**, not `min(parent trust)`: reviewed quarantine release can legitimately create an `agent_observed` derivative of an untrusted origin. Missing ancestry/marker evidence makes the verdict **UNVERIFIABLE**, not conclusively violated or holding. `memory.quarantine.release_blocked` rows evidence the §10.4 release-path secret gate. |
| **LATTICE** | §10.2: initial-write ceilings and reviewed trust transitions are respected; chains are sound | Grafted client page-walk over `panel/records` (single-realm keyset cursor): (a) initial LLM-created records enter at `agent_observed` or below, but an authorized steward-reviewed retier to `agent_verified` with valid verification is allowed, subject to the transitive taint ceiling. Original `provenance.author` remains unchanged by retiering, so **author plus current tier alone cannot prove a violation**; missing review/verification history means **UNVERIFIABLE**. (b) An untrusted/quarantined/ever-quarantined record itself or in its transitive `supersedes`/`derived_from` ancestry caps trust at `agent_observed`; incomplete ancestry or hidden markers likewise prevent a verdict. (c) Inspect supersede-chain acyclicity and links only to the extent coverage permits; missing or inaccessible rows must not be mistaken for a complete proof. Header shows **"checked N/M"** so partial walks never masquerade as complete. Proven violations deep-link to Biography LINEAGE. |
| **RECALL** | §8: recall earns its bytes; dreams demote dead weight | Utility table over `UsageStats` (durable on every row today): ratio, bytes-spent, `DEAD` flag (high injections, zero usefulness). **Loop-closure interaction:** pin flagged ids, then diff against the next dream's ops (join `memory_ids`/history to the pinned set) — the operator literally watches the demotion claim verify itself. Durable after restart once DreamRun detail persists; until then the dream half is live-only and labeled. |
| **DREAMS** | §7: consolidation is honest, cadence gaps explainable | Last-run recency + `quarantined_ops` from durable audit today; verdict sheet (14 counters) and skip reasons require persisted DreamRun detail — the tile renders `UNVERIFIABLE after restart — needs persisted DreamRun` until it lands, rather than faking completeness from the ring. Gate integrity cross-check: every `gated:true` promotion must have a matching gating-audit decision (orphans render red). |
| **STORE FLOOR** | §7.3: floors warn, deterministic code never evicts | `scope_overview()` counts + `body_bytes` vs 4,000-record/32MB floors per scope; warning tone past 80%. `UNVERIFIABLE — needs panel/overview` until phase 2. |

**Existing UI follow-up (outside this documentation correction):**
`console/src/panels/MemoryPanel.tsx` currently contains an LLM-origin predicate
that treats agent/distiller/steward authorship above `agent_observed` as a
violation. That can flag valid reviewed `agent_verified` records. Correcting
these design assertions does **not** fix that implementation; a separately
scoped code change is required. No UI/runtime change is implied here.

**ABAC verification is itself a widget behavior:** principal-swapping must make quarantined rows vanish entirely (per-row `memory_panel_record_visible`, not just the tab), operator-scope rows require explicit `operator.memory.read` (unscoped `agent.memory.read` must not leak them — the Holdings operator row renders access-denied tone), and hidden rows never consume page budget. Tiles inherit this: a tile whose evidence the principal cannot read says **no grant**.

## 6. New surfaces needed, ranked by cost

Every RPC below pays the contract-bijection + ABAC + embedded-bundle process tax (§2). Ranked cheapest first:

1. **RPC `meta/memory/panel/overview`** — wraps `scope_overview()` + `scope_floors()` (`sqlite_store.rs:1379/:1373`; built, doc-comment names the panel). Near-zero. → Holdings, STORE FLOOR tile.
2. **Field adds on `panel/record`:** `ever_quarantined` + `rank_set_at_ms` (`sqlite_store.rs:72/:82`, dropped in deserialization today). One-field additions. → ⚑ chip, LATTICE invariant (b).
3. **Fields on `panel/quarantine`:** `resolved_at_ms` + resolved `pending_promotions` rows (column exists, `:157`). → gate latency + history.
4. **RPC `meta/memory/panel/proposals`** — wraps `pending_proposals()` (`:1439`) incl. propose-time taint; visibility mirrors `memory_panel_promotion_visible`. → Pipeline, TAINT tile.
5. **RPC `meta/memory/panel/harvests`** — wraps `pending_harvests()` (`:1653`). → Health strip exit-interview queue.
6. **RPC `meta/memory/panel/injections`** — `injection_log()` (`:994`) + `identity/session_key/surface` filters + two-layer per-row ABAC; never carries nonce/stage_token. → Knowledge Lens history, ECHO-SAFETY tile. (Slightly costlier than 4–5 for the filter/ABAC work.)
7. **`history[]` array on `panel/record`** — per-record audit rows (`:102`; `dream_history` proves the read path). → exact Biography DREAMS section, tier-transition trail, Recall loop-closure joins.
8. **RPC `meta/memory/panel/health`** — serialize in-memory `SessionTaintTracker` / `BackgroundBudget` / distiller cursors / coordinator byte accounting; labeled restart-volatile. Medium (touches four subsystems; read-only snapshots of each). → Health strip, budget gauge.
9. **Event `memory.distill.completed` `{identity, session_key, cause, written, quarantined}`** — `DistillOutcome::Completed` reaches logs/tests only (`distiller.rs:997-1022`); the loop's most frequent hop is invisible live without it. **Judge-required phase-2 ask.** Requires `events.rs` + sink wiring + **both** `describeMemoryTimelineEvent` copies + `adapters.test.ts` + SignalsRail classification.
10. **RPC `meta/memory/panel/context`** — last-rendered injection block per identity from `RecallCoordinator` (nonce stripped; per-turn sections honestly absent until the injection default flips). Medium-high (new coordinator retention of the last block). → AS-INJECTED.
11. **Persist DreamRun detail** — phases, **14** verdict counters, skips (`steward.rs:793-846`) as one audit/`dream_runs` row at commit time, surfaced on `panel/dreams`. The single biggest observability gap; unblocks durable verdict sheets, durable SKIPPED rows, and the Recall loop-closure check.
12. **Durable trace RPC** (`meta/memory/panel/trace`) — time-ordered composition of audit + injections + proposals + pending_* (single-realm keyset, per-row ABAC). Highest cost; gates the reserved Trace tab only.
13. *(Optional)* **Durable calibration scorecard artifact** — `scripts/memory-evals` prints to stdout only; a JSON artifact would let Biography render calibration status beside the `CalibrationRef` each record already carries.

## 7. Phased implementation plan

**Phase 1 — existing data only (client-only except one aggregator visibility fix — the `_system` exemption in `console_aggregator/mod.rs`, see §3.1's as-built correction; standalone value).**
- Records filter bar + realm-scoped load-more (server-ready `scope/identity/scope_key/status/limit/cursor` params, unused today).
- Biography LINEAGE + LIFE from the existing `panel/record` chain + injections response; CopyButton in the detail pane.
- Best-effort evidence click-through via existing `meta/console/query_timeline`, labeled approximate or unavailable; retain `evidenceLabel` text when no excerpt can be found. Exact revision-pinned resolution needs a separate transcript-read seam, not this client-only phase.
- Dreams `memory_ids[]` → Biography links (data already returned, rendered as text today).
- Live refresh: `frame.event.startsWith("memory.")` → `refreshMemoryData` while a memory panel is docked (one condition in `handleLiveFrame`, `ConsoleApp.tsx:2338`); the frames only ARRIVE because of the §3.1 as-built server fix.
- SignalsRail memory-signal `onSelect` → dock `openTarget('memory')` + `loadMemoryRecordDetail` ("state here" pivots).
- Recall Utility sort/flag mode over `UsageStats` (bytes-spent approximated as `injected_count × body_bytes`, labeled).
- Partial Lattice Audit via the client page-walk, with the "checked N/M" header. Chain checks require adequate coverage; initial-write ceilings cannot be inferred from author plus current tier after reviewed retiers. Missing review, verification, or transitive-ancestry evidence keeps those verdicts `UNVERIFIABLE` (§5).
- Verdict strip boots with LATTICE (partial), RECALL, and DREAMS (recency) live; ECHO-SAFETY, TAINT (durable-shadow half), and STORE FLOOR boot as `UNVERIFIABLE` naming their missing surface.
- Live-follow discipline + ring-seam marker on any live strip.
- **Formatting fix:** explicit `memory.quarantine.release_blocked` copy in both `describeMemoryTimelineEvent` copies + `adapters.test.ts` + SignalsRail (today it falls to the humanized fallback and is dropped from the rail).
- Tab restructure (Holdings/Records/Knowledge/Pipeline/Dreams) with quarantine folded into Pipeline; all sections keep per-section `-32030` tolerance.

**Phase 2 — cheap reads (surfaces 1–9).** Each RPC upgrades one view independently: `overview` → Holdings + STORE FLOOR; record fields + `history[]` → ⚑ chip, exact DREAMS section, evidence for LATTICE checks subject to §5's review/ancestry requirements; `proposals` + quarantine fields → full Pipeline + gate latency; `injections` → Knowledge Lens history, possible-repeat diagnostics, and consecutive-build overlap. ECHO-SAFETY does **not** automatically flip to HOLDING: reset-boundary evidence is still needed to prove turn dedup, and exact evidence reads are needed for source-range checks. `harvests` + `health` → Health strip; **`memory.distill.completed`** → the proposed live distill-hop signal.

**Phase 3 — heavier surfaces (10–12).** `context` → AS-INJECTED; persisted DreamRun detail → durable verdict sheets + Recall loop-closure; durable trace RPC → the reserved **Trace** tab (lanes, mob grouping, dream spans), reusing the phase-1/2 event↔Biography links verbatim.

## 8. Open questions

1. **Grant shape for the health snapshot.** Taint maps and distiller cursors reveal cross-identity operational state. Does `panel/health` gate on unscoped `agent.memory.read` (like `panel/dreams`) or does it deserve a distinct projected affordance (e.g. `experience.memory.can_read_runtime`)?
2. **`panel/context` retention.** The coordinator does not retain rendered blocks today; retaining the last block per (identity, surface) is new memory pressure. Retain bytes, or retain the manifest (record ids + byte counts) and re-render on demand?
3. **DreamRun persistence home.** One JSON audit row (cheap, rides existing GC) vs. a `dream_runs` table (queryable, schema migration)? Does any part belong upstream via the (now-shipped-0.7.18) ask channel, or is it purely meta-side?
4. **Page-walk ceilings.** The Lattice/Recall exhaustive walks page 200 rows/request against a fetch-on-demand data plane. At what store size do we cap the walk (with the honest partial header) vs. move the invariant server-side?
5. **`memory.distill.completed` payload.** Should it carry directly written record ids so Records/Biography (or reviewer-visible quarantine) can pivot from the pulse to extraction results? Proposal ids belong to actual mob/operator proposal actions, not ordinary direct extraction.
6. **Quarantine tab migration.** Folding quarantine into Pipeline changes `data-testid` surfaces (`memory-tab:*`) covered by `MemoryPanel.test.ts` — keep a redirect alias for one release?
7. **Multi-realm verdicts.** Tiles are per-realm (keyset cursors are single-realm). One tile row per realm, or a realm selector with per-realm badges?
8. **Calibration surfacing.** If the scorecard artifact lands (surface 13), does calibration status belong on Biography (per-record `CalibrationRef`) or as a seventh tile?

---

## Appendix A — Judge scores

| Concept | C1 both-modes/levels | C2 loop legibility | C3 console/data fit | C4 phase-1 value | Total |
|---|---|---|---|---|---|
| Memory Flight Recorder (trace-first) | 8 | 9 | 5 | 5 | 27 |
| **The Ledger of What It Knows (state-first) — winner** | 7 | 6 | **9** | **9** | **31** |
| The Claims Board (verification-first) | 8 | 7 | 7 | 7 | 29 |

The Ledger won on console/data fit (extends the exact MemoryPanel idiom; front door needs only the already-built `scope_overview()` behind one thin handler) and phase-1 value (a client-only phase 1 is standalone-valuable). Judge-required grafts incorporated: verdict-tile strip, standing DUP proof (limited to diagnostics without reset-boundary evidence, as corrected in §5), exhaustive Lattice Audit (subject to review/ancestry evidence), Recall loop-closure (Claims Board); "state here" pivots, live-follow discipline, consecutive-build overlap, `memory.distill.completed` promoted to a required phase-2 ask, and the reserved phase-3 Trace tab (Flight Recorder).
