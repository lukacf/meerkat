// Selector contract between the memory console UI (MemoryPanel.tsx) and the
// memory e2e (memory-e2e.cjs). The panel's data-testids MUST match this
// inventory: the e2e flows locate every element through it and nothing
// else. Names extend the scheme MemoryPanel.tsx ships (memory-* prefix,
// `:`-separated dynamic segments).
//
// This file is the single source of truth: a flow that needs a new element
// adds its name here and the panel ships it.

module.exports = {
  // ── Navigation (Sidebar.tsx renders `nav:${kind}`) ──
  // Rendered only when /console/experience reports memory.can_read.
  NAV_MEMORY: "nav:memory",

  // ── Panel chrome ──
  PANEL: "memory-panel",
  UNAVAILABLE: "memory-unavailable", // store not configured on the runtime
  ERROR: "memory-error", // the status row's error, between the tabs and the body
  REFRESH: "memory-refresh", // header button, next to the team picker

  // ── Tabs ──
  // `memory-tab:${id}` where id is holdings | records | knowledge |
  // pipeline | dreams. All five render for every principal; access shows up
  // per section inside a tab ("no grant" notes), not by hiding tabs.
  // `memory-tab:quarantine` still exists as a ONE-RELEASE redirect alias
  // (open Q6): an invisible button (opacity 0, 1x1) outside the tab list
  // that switches to Pipeline. It keeps a bounding box so driver clicks land.
  tab: (id) => `memory-tab:${id}`,

  // ── Records rows ──
  // `memory-group:${scopeGroupKey}` e.g. memory-group:identity:default:router
  // (grouped view renders only when no filter is active and the sort is
  // Recent). `memory-record:${memoryId}` one row per record, body-free.
  group: (scopeKey) => `memory-group:${scopeKey}`,
  record: (memoryId) => `memory-record:${memoryId}`,

  // ── Filter bar + paging (Records tab) ──
  FILTER: "memory-filter", // toolbar container
  FILTER_INPUT: "memory-filter-input", // <input> agent or team name, or a scope key (Enter or blur applies)
  FILTER_SCOPE: "memory-filter:scope", // <select> ""|identity|mob|operator|realm
  FILTER_STATUS: "memory-filter:status", // <select> ""|active|quarantined|superseded|tombstoned
  FILTER_REALM: "memory-filter:realm", // <select>, renders only when realms.length > 1
  FILTER_CLEAR: "memory-filter-clear", // "Clear filters", renders only while a filter is active
  SORT: "memory-sort", // <select> recency|utility, labelled Recent and Least useful
  UTILITY_NOTE: "memory-utility-note", // explains the least-useful order and the estimate
  LOAD_MORE: "memory-load-more", // renders only while a keyset cursor exists (per realm)
  // Multi-realm without a realm picked: merged single page, no paging.
  MULTI_REALM_NOTE: "memory-multi-realm-note",
  RECORDS_EMPTY: "memory-records-empty", // says "no grant" for denied queries, never "empty"
  RECORDS_DENIED_NOTE: "memory-records-denied-note", // denied load-more continuation

  // ── Record detail / Biography ──
  DETAIL: "memory-detail", // the Biography container
  DETAIL_BACK: "memory-detail-back", // "All records"
  DETAIL_BODY: "memory-detail-body",
  // Biography sections:
  DETAIL_BORN: "memory-detail-born", // "Where it came from": author and evidence
  DETAIL_LINEAGE: "memory-detail-lineage", // "History": the supersede lane, newest first
  DETAIL_LIFE: "memory-detail-life", // "Usage": counters and injections
  DETAIL_DREAMS: "memory-detail-dreams", // lossy memory_ids join
  // `memory-chain:${memoryId}`: history rows (buttons; clicking a
  // non-current version loads that Biography in place; data-current /
  // data-dimmed).
  chainEntry: (memoryId) => `memory-chain:${memoryId}`,
  // Evidence click-through: `memory-evidence:${index}` buttons per ref ("Conversation
  // with <agent>, messages a-b"; the session ID and generation are in the
  // Biography's technical details). The result renders
  // memory-evidence-excerpt (transcript window),
  // memory-evidence-degraded ("not found in the recent timeline window":
  // the reference stays, the copy never claims the session is gone), or
  // memory-evidence-empty (session found, range has no messages).
  evidenceRef: (index) => `memory-evidence:${index}`,
  EVIDENCE_EXCERPT: "memory-evidence-excerpt",
  EVIDENCE_DEGRADED: "memory-evidence-degraded",
  EVIDENCE_EMPTY: "memory-evidence-empty",

  // ── Holdings tab ──
  HOLDINGS: "memory-holdings",
  HOLDINGS_DENIED: "memory-holdings-denied", // records -32030: "no grant" note
  // `memory-holdings-scope:${scopeGroupKey}`: scope rows; click pivots to
  // Records pre-filtered to that scope. Rows render store TOTALS from
  // panel/overview when the surface answers (same key scheme); the
  // loaded-records fallback remains for principals the surface denies.
  holdingsScope: (scopeKey) => `memory-holdings-scope:${scopeKey}`,
  // `memory-holdings-floor:${scopeGroupKey}`: the "Near limit" chip on an
  // overview scope row at the 4,000-record/32MB floor.
  holdingsFloor: (scopeKey) => `memory-holdings-floor:${scopeKey}`,
  // Retired agents awaiting their exit-interview dream (Holdings), from
  // panel/harvests. Row id: memory-harvest:${identity}.
  HARVESTS: "memory-harvests",
  harvest: (identity) => `memory-harvest:${identity}`,
  // Access-denied scope rows: the one-row probes ({scope, limit:1}) render
  // these when the principal lacks the scope grant ("No grant" chip), so a
  // denied scope is never indistinguishable from an empty one. Ids:
  // memory-holdings-scope-denied:operator / :mob.
  holdingsScopeDenied: (kind) => `memory-holdings-scope-denied:${kind}`,

  // ── Verdict strip (Holdings "Checks") ──
  VERDICT_STRIP: "memory-verdict-strip",
  // `memory-verdict:${id}`: id is the STABLE tile id:
  //   echo-safety | taint-wall | lattice | recall | dreams | store-floor.
  // Lattice, recall, dreams and store-floor render first; echo-safety and
  // taint-wall, which this console can never run, render last on one quiet
  // line each. The verdict state rides the `data-status` attribute:
  //   holding | degraded | violated | unverifiable | no-grant
  // and the tile shows it as an uppercase status word.
  // Tiles are doors: a tile is a button that opens the tab holding its
  // evidence. A VIOLATED lattice tile instead lists its evidence: per-record
  // buttons (record titles, capped at 5) that open the offending Biography:
  //   `memory-verdict-evidence:${tileId}:${memoryId}`.
  // While a re-check runs, a settled tile keeps its verdict (never flickers
  // back to UNVERIFIABLE) and shows a "re-checking" marker over its corner,
  // `memory-verdict-recheck:${tileId}`, outside the text flow so the tile
  // row never grows. A refresh that brings the same records does not
  // re-check.
  verdictTile: (id) => `memory-verdict:${id}`,
  verdictEvidence: (tileId, memoryId) => `memory-verdict-evidence:${tileId}:${memoryId}`,
  verdictRecheck: (tileId) => `memory-verdict-recheck:${tileId}`,

  // ── Pipeline tab (quarantine folded in) ──
  PIPELINE: "memory-pipeline",
  PIPELINE_STAGES: "memory-pipeline-stages", // stepper: Proposed N, Waiting for approval N, Quarantined N ("no grant" without the review grant)
  // The inbound proposals lane (panel/proposals): pending proposals with
  // propose-time taint chips. Row id: memory-proposal:${proposalId}; taint
  // chip: memory-proposal-taint:${proposalId}.
  PIPELINE_PROPOSALS: "memory-pipeline-proposals",
  proposal: (proposalId) => `memory-proposal:${proposalId}`,
  proposalTaint: (proposalId) => `memory-proposal-taint:${proposalId}`,
  // The usage-audit review queue (panel/audit_verdicts), "Worth a look":
  // records a dream flagged for a closer look, read-only. Row id:
  // memory-review:${runId}:${recordId}; the record deep-link button:
  // memory-review-record:${runId}:${recordId}.
  REVIEW_QUEUE: "memory-review-queue",
  review: (runId, recordId) => `memory-review:${runId}:${recordId}`,
  reviewRecord: (runId, recordId) => `memory-review-record:${runId}:${recordId}`,
  PIPELINE_NO_GRANT: "memory-pipeline-no-grant", // shown without memory.quarantine.review
  QUARANTINE_NOTE: "memory-quarantine-note", // how the queue is decided
  quarantineRecord: (memoryId) => `memory-quarantine-record:${memoryId}`, // a row that opens the Biography
  pendingPromotion: (pendingId) => `memory-pending:${pendingId}`,
  // `memory-pipeline-decide:${pendingId}`: "Decide in Approvals", a deep
  // link into the gating panel. Renders ONLY when the nav offers gating
  // (visibleControls); on aggregator runtimes without a mob control surface
  // the button is absent.
  pipelineDecide: (pendingId) => `memory-pipeline-decide:${pendingId}`,

  // ── Quarantine decision (Biography of a quarantined identity record) ──
  // Renders only when meta/memory/quarantine/decide is advertised to the
  // caller on a mutable console. Release and Discard (the tombstone
  // verdict; QUARANTINE_TOMBSTONE keeps its name) send the detail's
  // content_hash; a refusal or denial renders QUARANTINE_ERROR in place, a
  // decision opens the resulting Biography with QUARANTINE_OUTCOME.
  QUARANTINE_DECIDE: "memory-quarantine-decide",
  QUARANTINE_RATIONALE: "memory-quarantine-rationale",
  QUARANTINE_RELEASE: "memory-quarantine-release",
  QUARANTINE_TOMBSTONE: "memory-quarantine-tombstone",
  QUARANTINE_ERROR: "memory-quarantine-error",
  QUARANTINE_OUTCOME: "memory-quarantine-outcome",

  // ── Live memory-event strip (bottom of the Pipeline tab) ──
  LIVE_STRIP: "memory-live-strip",
  liveRow: (frameId) => `memory-live-row:${frameId}`,
  // "Open record" pivot per live row (renders when the payload names a record).
  livePivot: (frameId) => `memory-live-pivot:${frameId}`,
  LIVE_JUMP: "memory-live-jump", // "N behind · Jump to live" (renders while paused)
  LIVE_SEAM: "memory-live-seam", // "Live history starts here" marker

  // ── Knowledge tab ──
  KNOWLEDGE: "memory-knowledge",
  KNOWLEDGE_IDENTITY: "memory-knowledge-identity", // agent <select>: values are raw identities, labels display names
  // `memory-knowledge-segment:${label}`: composition segments, keyed by the
  // raw segment label (identity:router, mob (all mobs), operator, realm)
  // while the row reads in words; click pivots to Records filtered to that
  // scope.
  knowledgeSegment: (label) => `memory-knowledge-segment:${label}`,
  KNOWLEDGE_AS_INJECTED: "memory-knowledge-as-injected", // footnote: the injected text is not shown here
  // The durable injection history (panel/injections), newest first. Row id:
  // memory-injection:${index}; the record deep-link:
  // memory-injection-record:${index}; the consecutive-duplicate tripwire:
  // memory-injection-dup:${index} ("Repeated" chip).
  KNOWLEDGE_HISTORY: "memory-knowledge-history",
  injection: (index) => `memory-injection:${index}`,
  injectionRecord: (index) => `memory-injection-record:${index}`,
  injectionDup: (index) => `memory-injection-dup:${index}`,
  KNOWLEDGE_BUDGET: "memory-knowledge-budget", // same footnote: session budgets are not shown here

  // ── Dreams tab ──
  dream: (runId) => `memory-dream:${runId}`, // "Earlier runs, from the audit log" rows
  // `memory-dream-record:${runId}:${memoryId}`: touched-record links to the Biography.
  dreamRecord: (runId, memoryId) => `memory-dream-record:${runId}:${memoryId}`,
  // Durable dream runs (panel/dream_runs). Row: memory-dream-run:${runId};
  // expand/collapse toggle: memory-dream-run-toggle:${runId}; expanded
  // detail (phases in order, skips, technical details):
  // memory-dream-run-detail:${runId}.
  DREAM_RUNS: "memory-dream-runs",
  dreamRun: (runId) => `memory-dream-run:${runId}`,
  dreamRunToggle: (runId) => `memory-dream-run-toggle:${runId}`,
  dreamRunDetail: (runId) => `memory-dream-run-detail:${runId}`,

  // ── Signals rail (SignalsRail.tsx) ──
  SIGNALS_RAIL: "signals-rail",
  signal: (signalId) => `signal:${signalId}`, // memory.* frames map to signals
  signalsFilter: (presetId) => `signals-filter:${presetId}`,
  // "Open in Memory" pivot inside single-item memory signal rows: opens the
  // Memory panel and loads the record Biography when the frame payload
  // names a record_id. (Clicking the row body does the same via onSelect;
  // multi-item groups expand first, and their per-event buttons fire
  // onSelect per item.) GATED: the pivot renders only when the projected
  // experience grants memory.can_read; in denied modes the button must be
  // absent even while memory.* signals are visible in the rail.
  SIGNAL_MEMORY_PIVOT: "signal-memory-pivot",
};
