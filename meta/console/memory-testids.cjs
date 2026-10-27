// Selector contract between the memory console UI (UI-P1.B) and the
// memory e2e (memory-e2e.cjs). The panel's data-testids MUST match this
// inventory — the e2e flows locate every element through it and nothing
// else. Names extend the scheme MemoryPanel.tsx already ships (memory-*
// prefix, `:`-separated dynamic segments).
//
// UPDATED by the UI-P1.B panel agent to the names MemoryPanel.tsx actually
// ships (this file is the single source of truth — flows that used the
// earlier placeholder names need the same rename).

module.exports = {
  // ── Navigation (Sidebar.tsx renders `nav:${kind}`) ──
  // Rendered only when /console/experience reports memory.can_read.
  NAV_MEMORY: "nav:memory",

  // ── Panel chrome ──
  PANEL: "memory-panel",
  UNAVAILABLE: "memory-unavailable", // store not configured on the runtime
  ERROR: "memory-error",
  REFRESH: "memory-refresh",

  // ── Tabs ──
  // `memory-tab:${id}` where id ∈ holdings | records | knowledge |
  // pipeline | dreams. All five render for every principal; access shows up
  // per-section inside a tab ("no grant" notes), not by hiding tabs.
  // `memory-tab:quarantine` still exists as a ONE-RELEASE redirect alias
  // (open Q6): an invisible button (opacity 0, 1×1) that switches to
  // Pipeline. It keeps a bounding box so driver clicks land.
  tab: (id) => `memory-tab:${id}`,

  // ── Records rows ──
  // `memory-group:${scopeGroupKey}` e.g. memory-group:identity:default:router
  // (grouped view renders only when no filter is active and sort=recency).
  // `memory-record:${memoryId}` one row per record, body-free list row.
  group: (scopeKey) => `memory-group:${scopeKey}`,
  record: (memoryId) => `memory-record:${memoryId}`,

  // ── Filter bar + paging (Records tab) ──
  FILTER: "memory-filter", // container
  FILTER_INPUT: "memory-filter-input", // <input> identity / scope key (Enter or blur applies)
  FILTER_SCOPE: "memory-filter:scope", // <select> all|identity|mob|operator|realm
  FILTER_STATUS: "memory-filter:status", // <select> all|active|quarantined|superseded|tombstoned
  FILTER_REALM: "memory-filter:realm", // <select>, renders only when realms.length > 1
  FILTER_CLEAR: "memory-filter-clear", // renders only while a filter is active
  SORT: "memory-sort", // <select> recency|utility (utility = Recall Utility mode)
  UTILITY_NOTE: "memory-utility-note", // approximation disclaimer, shows in utility mode
  LOAD_MORE: "memory-load-more", // renders only while a keyset cursor exists (per-realm)
  // Multi-realm without a realm picked: merged single page, no paging.
  MULTI_REALM_NOTE: "memory-multi-realm-note",
  RECORDS_EMPTY: "memory-records-empty", // says "no grant" for denied queries, never "empty"
  RECORDS_DENIED_NOTE: "memory-records-denied-note", // denied load-more continuation

  // ── Record detail / Biography ──
  DETAIL: "memory-detail", // the Biography container (kept from P3b)
  DETAIL_BACK: "memory-detail-back",
  DETAIL_BODY: "memory-detail-body",
  // Biography sections:
  DETAIL_BORN: "memory-detail-born", // author + evidence refs
  DETAIL_LINEAGE: "memory-detail-lineage", // supersede lane, newest-first
  DETAIL_LIFE: "memory-detail-life", // usage counters + injection ledger
  DETAIL_DREAMS: "memory-detail-dreams", // lossy memory_ids join
  // `memory-chain:${memoryId}` — lineage lane rows (now buttons; clicking a
  // non-current node loads that Biography in place; data-current / data-dimmed).
  chainEntry: (memoryId) => `memory-chain:${memoryId}`,
  // Evidence click-through: `memory-evidence:${index}` buttons per ref;
  // result renders memory-evidence-excerpt (transcript window),
  // memory-evidence-degraded ("Session not found in the recent timeline
  // window" — label-only fallback; the copy no longer claims the session is
  // gone), or memory-evidence-empty (session found, range has no messages).
  evidenceRef: (index) => `memory-evidence:${index}`,
  EVIDENCE_EXCERPT: "memory-evidence-excerpt",
  EVIDENCE_DEGRADED: "memory-evidence-degraded",
  EVIDENCE_EMPTY: "memory-evidence-empty",

  // ── Holdings tab ──
  HOLDINGS: "memory-holdings",
  HOLDINGS_DENIED: "memory-holdings-denied", // records -32030 → "no grant" note
  // `memory-holdings-scope:${scopeGroupKey}` — scope rows; click pivots to
  // Records pre-filtered to that scope. Phase 2: rows render store TOTALS
  // from panel/overview when the surface answers (same key scheme); the
  // loaded-records fallback remains for principals the surface denies.
  holdingsScope: (scopeKey) => `memory-holdings-scope:${scopeKey}`,
  // `memory-holdings-floor:${scopeGroupKey}` — FLOOR PRESSURE marker chip on
  // an overview scope row at the 4,000-record/32MB floor.
  holdingsFloor: (scopeKey) => `memory-holdings-floor:${scopeKey}`,
  // Harvest queue (Holdings): retired identities awaiting the exit-interview
  // dream, from panel/harvests. Row id: memory-harvest:${identity}.
  HARVESTS: "memory-harvests",
  harvest: (identity) => `memory-harvest:${identity}`,
  // Access-denied scope rows: the one-row probes ({scope, limit:1}) render
  // these when the principal lacks the scope grant (spec §3.1 "no grant"
  // operator row) — a denied scope must not be indistinguishable from an
  // empty one. Ids: memory-holdings-scope-denied:operator / :mob.
  holdingsScopeDenied: (kind) => `memory-holdings-scope-denied:${kind}`,

  // ── Verdict strip (Holdings header) ──
  VERDICT_STRIP: "memory-verdict-strip",
  // `memory-verdict:${id}` — id is the STABLE tile id:
  //   echo-safety | taint-wall | lattice | recall | dreams | store-floor.
  // The verdict state rides the `data-status` attribute:
  //   holding | degraded | violated | unverifiable | no-grant.
  // Tiles are doors: clicking opens the tab holding the evidence. Tiles are
  // div[role=button]; a VIOLATED lattice tile additionally nests per-record
  // evidence buttons (capped at 5) that open the offending Biography:
  //   `memory-verdict-evidence:${tileId}:${memoryId}`.
  // While a re-check runs, a settled tile keeps its verdict (never flickers
  // back to UNVERIFIABLE) and shows a "re-checking…" marker over its corner,
  // `memory-verdict-recheck:${tileId}`, outside the line flow so the tile row
  // never grows. A refresh that brings the same records does not re-check.
  verdictTile: (id) => `memory-verdict:${id}`,
  verdictEvidence: (tileId, memoryId) => `memory-verdict-evidence:${tileId}:${memoryId}`,
  verdictRecheck: (tileId) => `memory-verdict-recheck:${tileId}`,

  // ── Pipeline tab (quarantine folded in) ──
  PIPELINE: "memory-pipeline",
  PIPELINE_STAGES: "memory-pipeline-stages", // PROPOSED ─▶ PENDING GATE ─▶ COMMITTED · QUAR strip
  // Phase 2: the inbound proposals lane (panel/proposals) — pending
  // proposals with propose-time taint badges. Row id:
  // memory-proposal:${proposalId}; taint chip:
  // memory-proposal-taint:${proposalId}.
  PIPELINE_PROPOSALS: "memory-pipeline-proposals",
  proposal: (proposalId) => `memory-proposal:${proposalId}`,
  proposalTaint: (proposalId) => `memory-proposal-taint:${proposalId}`,
  // Phase 2: the usage-audit review queue (panel/audit_verdicts) —
  // "memories you might want to correct", read-only. Row id:
  // memory-review:${runId}:${recordId}; the record deep-link button:
  // memory-review-record:${runId}:${recordId}.
  REVIEW_QUEUE: "memory-review-queue",
  review: (runId, recordId) => `memory-review:${runId}:${recordId}`,
  reviewRecord: (runId, recordId) => `memory-review-record:${runId}:${recordId}`,
  PIPELINE_NO_GRANT: "memory-pipeline-no-grant", // shown without memory.quarantine.review
  QUARANTINE_NOTE: "memory-quarantine-note", // how the queue is decided (kept from P3b)
  quarantineRecord: (memoryId) => `memory-quarantine-record:${memoryId}`, // now a button → Biography
  pendingPromotion: (pendingId) => `memory-pending:${pendingId}`,
  // `memory-pipeline-decide:${pendingId}` — deep link into the Gating inbox.
  // Renders ONLY when the nav offers gating (visibleControls) — on aggregator
  // runtimes without a mob control surface the button is absent.
  pipelineDecide: (pendingId) => `memory-pipeline-decide:${pendingId}`,

  // ── Quarantine decision (Biography of a quarantined identity record) ──
  // Renders only when meta/memory/quarantine/decide is advertised to the
  // caller on a mutable console. Release/Tombstone send the detail's
  // content_hash; a refusal or denial renders QUARANTINE_ERROR in place, a
  // decision opens the resulting Biography with QUARANTINE_OUTCOME.
  QUARANTINE_DECIDE: "memory-quarantine-decide",
  QUARANTINE_RATIONALE: "memory-quarantine-rationale",
  QUARANTINE_RELEASE: "memory-quarantine-release",
  QUARANTINE_TOMBSTONE: "memory-quarantine-tombstone",
  QUARANTINE_ERROR: "memory-quarantine-error",
  QUARANTINE_OUTCOME: "memory-quarantine-outcome",

  // ── Live memory-event strip (bottom of Pipeline tab) ──
  LIVE_STRIP: "memory-live-strip",
  liveRow: (frameId) => `memory-live-row:${frameId}`,
  // "state here" pivot per live row (renders when the payload names a record).
  livePivot: (frameId) => `memory-live-pivot:${frameId}`,
  LIVE_JUMP: "memory-live-jump", // "N behind · jump to live" (renders while paused)
  LIVE_SEAM: "memory-live-seam", // "ring history starts here" marker

  // ── Knowledge tab ──
  KNOWLEDGE: "memory-knowledge",
  KNOWLEDGE_IDENTITY: "memory-knowledge-identity", // identity <select>
  // `memory-knowledge-segment:${label}` — composition segments; click pivots
  // to Records filtered to that scope.
  knowledgeSegment: (label) => `memory-knowledge-segment:${label}`,
  KNOWLEDGE_AS_INJECTED: "memory-knowledge-as-injected", // honest surface-10 placeholder
  // Phase 2: the durable injection history (panel/injections), newest first.
  // Row id: memory-injection:${index}; the record deep-link:
  // memory-injection-record:${index}; the consecutive-duplicate tripwire:
  // memory-injection-dup:${index} (DUP chip).
  KNOWLEDGE_HISTORY: "memory-knowledge-history",
  injection: (index) => `memory-injection:${index}`,
  injectionRecord: (index) => `memory-injection-record:${index}`,
  injectionDup: (index) => `memory-injection-dup:${index}`,
  KNOWLEDGE_BUDGET: "memory-knowledge-budget", // honest panel/health placeholder

  // ── Dreams tab ──
  dream: (runId) => `memory-dream:${runId}`,
  // `memory-dream-record:${runId}:${memoryId}` — touched-record links → Biography.
  dreamRecord: (runId, memoryId) => `memory-dream-record:${runId}:${memoryId}`,
  // Phase 2: durable verdict sheets (panel/dream_runs). Card:
  // memory-dream-run:${runId}; expand/collapse toggle:
  // memory-dream-run-toggle:${runId}; expanded detail (phases in order,
  // non-zero verdict counters, skips): memory-dream-run-detail:${runId}.
  DREAM_RUNS: "memory-dream-runs",
  dreamRun: (runId) => `memory-dream-run:${runId}`,
  dreamRunToggle: (runId) => `memory-dream-run-toggle:${runId}`,
  dreamRunDetail: (runId) => `memory-dream-run-detail:${runId}`,

  // ── Signals rail (SignalsRail.tsx) ──
  SIGNALS_RAIL: "signals-rail",
  signal: (signalId) => `signal:${signalId}`, // memory.* frames map to signals
  signalsFilter: (presetId) => `signals-filter:${presetId}`,
  // "State here" pivot inside single-item memory signal rows: opens the
  // Memory panel and loads the record Biography when the frame payload
  // names a record_id. (Clicking the row body does the same via onSelect;
  // multi-item groups expand first, and their per-event buttons fire
  // onSelect per item.) GATED: the pivot renders only when the projected
  // experience grants memory.can_read — in denied modes the button must be
  // absent even while memory.* signals are visible in the rail.
  SIGNAL_MEMORY_PIVOT: "signal-memory-pivot",
};
