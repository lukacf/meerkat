import React from "react";
import { CopyButton } from "@console-components";
import type { ConversationTimelineEntry } from "@console-core";
import { describeMemoryTimelineEvent } from "../lib/adapters";
import { errorMessage, jsonRpcErrorCode } from "../lib/errors";
import type {
  ConsoleFrame,
  MemoryAuditVerdictEntry,
  MemoryAuthor,
  MemoryDreamRun,
  MemoryDreamRunDetail,
  MemoryDreamRunSheet,
  MemoryEvidenceRef,
  MemoryFullRecord,
  MemoryHarvestEntry,
  MemoryInjectionEntry,
  MemoryLedgerEntry,
  MemoryPanelOverviewResult,
  MemoryPanelRecord,
  MemoryPanelRecordsResult,
  MemoryPendingPromotion,
  MemoryProposalEntry,
  MemoryQuarantineDecideResult,
  MemoryQuarantineVerdict,
  MemoryRecordScope,
  MemoryRecordStatus,
  MemoryTrust,
} from "../types";
import { PanelStatusRow } from "./PanelStatusRow";

export interface MemoryRecordDetail {
  realm: string;
  record: MemoryFullRecord;
  chain: MemoryPanelRecord[];
  injections: MemoryInjectionEntry[];
}

/// Filter-bar state for the Records tab. Maps 1:1 onto the (previously
/// unused) `scope/identity/scope_key/status` params of panel/records.
export interface MemoryRecordsFilter {
  scope?: "identity" | "mob" | "operator" | "realm";
  /// Identity name when scope is identity; scope key for mob/operator.
  key?: string;
  status?: "active" | "quarantined" | "superseded" | "tombstoned";
  /// Realm name. Keyset cursors are single-realm on the server, so picking a
  /// realm is what makes load-more honest on multi-realm gateways.
  realm?: string;
}

export interface MemoryPanelProps {
  records: MemoryPanelRecord[];
  realms: string[];
  quarantineRecords: MemoryPanelRecord[];
  pendingPromotions: MemoryPendingPromotion[];
  dreams: MemoryDreamRun[];
  detail: MemoryRecordDetail | null;
  detailLoading?: boolean;
  canReviewQuarantine?: boolean;
  unavailable?: boolean;
  error?: string | null;
  /// Cursor from the initial records page (single-realm only, keyset).
  nextCursor?: string | null;
  /// Per-section -32030 outcomes: the section data is empty because the
  /// principal lacks the grant, not because the store is empty. Tiles whose
  /// evidence lives behind a denied section render "no grant", never green.
  recordsDenied?: boolean;
  dreamsDenied?: boolean;
  /// Scope-probe outcomes (§3.1/§5 ABAC): the unfiltered listing row-filters
  /// denied scopes silently, so a one-row probe per restricted scope kind is
  /// the only honest way to know a scope exists but is not readable. When
  /// set, Holdings renders the access-denied-tone scope row.
  operatorScopeDenied?: boolean;
  mobScopeDenied?: boolean;
  /// Phase-2 read surfaces. Each section carries its own -32030 outcome so a
  /// denied surface renders "no grant", never an indistinguishable empty one.
  overview?: MemoryPanelOverviewResult | null;
  overviewDenied?: boolean;
  proposals?: MemoryProposalEntry[];
  proposalsDenied?: boolean;
  injections?: MemoryLedgerEntry[];
  injectionsDenied?: boolean;
  harvests?: MemoryHarvestEntry[];
  harvestsDenied?: boolean;
  dreamRuns?: MemoryDreamRunSheet[];
  dreamRunsDenied?: boolean;
  auditVerdicts?: MemoryAuditVerdictEntry[];
  auditVerdictsDenied?: boolean;
  /// Live `memory.*` frames from the in-memory ring (lossy: 1024/identity,
  /// 4096 total). Used only as freshness signals and pivot points.
  liveFrames?: ConsoleFrame[];
  onRefresh: (record?: { realm: string; memoryId: string }) => void;
  onSelectRecord: (realm: string | undefined, memoryId: string) => void;
  onClearDetail: () => void;
  /// Issue a filtered/paged panel/records query. Resolves null on access
  /// denial (-32030), mirroring the per-section tolerance of the base load.
  onQueryRecords?: (params: Record<string, unknown>) => Promise<MemoryPanelRecordsResult | null>;
  /// Resolve an evidence ref against meta/console/query_timeline. Resolves
  /// null when the session is no longer in the timeline (the degrade rule:
  /// fall back to the evidenceLabel text).
  onLoadEvidence?: (
    identity: string | undefined,
    evidence: MemoryEvidenceRef,
  ) => Promise<ConversationTimelineEntry[] | null>;
  /// Deep-link into the gating inbox — memory promotion verdicts ride the
  /// normal gating flow, never a parallel decision surface here.
  onOpenGating?: () => void;
  /// The operator half of quarantine review (meta/memory/quarantine/decide)
  /// for quarantined records that no gated promotion is waiting on. Passed
  /// only when the caller is advertised the method on a mutable console;
  /// absent, quarantined records stay read-only here. Rejects with the
  /// transport's typed JSON-RPC error.
  onDecideQuarantine?: (
    realm: string,
    record: MemoryFullRecord,
    verdict: MemoryQuarantineVerdict,
    rationale: string | undefined,
  ) => Promise<MemoryQuarantineDecideResult>;
}

export type MemoryTab = "holdings" | "records" | "knowledge" | "pipeline" | "dreams";

export const MEMORY_TABS: MemoryTab[] = [
  "holdings",
  "records",
  "knowledge",
  "pipeline",
  "dreams",
];

const TAB_LABEL: Record<MemoryTab, string> = {
  holdings: "Holdings",
  records: "Records",
  knowledge: "Knowledge",
  pipeline: "Pipeline",
  dreams: "Dreams",
};

export function memoryTabLabel(tab: MemoryTab): string {
  return TAB_LABEL[tab];
}

/// The quarantine tab folded into Pipeline (proposal §7 / open Q6): the old
/// `memory-tab:quarantine` testid stays addressable as a redirect alias for
/// one release so existing automation keeps working, then it goes away.
export function resolveMemoryTabAlias(tab: string): MemoryTab | null {
  if (tab === "quarantine") return "pipeline";
  return MEMORY_TABS.includes(tab as MemoryTab) ? (tab as MemoryTab) : null;
}

// ── Pure helpers (exported via __memoryTest) ──────────────────────────────

export function realmOfRecord(record: Pick<MemoryPanelRecord, "scope">): string {
  return record.scope.realm;
}

/// Stable grouping key for a record scope. Each identity gets its own group;
/// mob/operator/realm scopes collapse to one group per (realm, kind).
export function scopeGroupKey(scope: MemoryRecordScope): string {
  switch (scope.scope) {
    case "identity":
      return `identity:${scope.realm}:${scope.identity}`;
    case "mob":
      return `mob:${scope.realm}:${scope.mob}`;
    case "operator":
      return `operator:${scope.realm}:${scope.operator}`;
    case "realm":
      return `realm:${scope.realm}`;
  }
}

export function scopeGroupLabel(scope: MemoryRecordScope): string {
  switch (scope.scope) {
    case "identity":
      return scope.identity;
    case "mob":
      return `Mob: ${scope.mob}`;
    case "operator":
      return `Operator: ${scope.operator}`;
    case "realm":
      return "Realm";
  }
}

/// Rank scope groups so identity groups sort first (alphabetically), then mob,
/// operator, and realm. Keeps the records tab visually stable.
function scopeGroupRank(scope: MemoryRecordScope): number {
  switch (scope.scope) {
    case "identity":
      return 0;
    case "mob":
      return 1;
    case "operator":
      return 2;
    case "realm":
      return 3;
  }
}

export interface MemoryScopeGroup {
  key: string;
  label: string;
  scope: MemoryRecordScope;
  records: MemoryPanelRecord[];
}

export function groupRecordsByScope(records: MemoryPanelRecord[]): MemoryScopeGroup[] {
  const byKey = new Map<string, MemoryScopeGroup>();
  for (const record of records) {
    const key = scopeGroupKey(record.scope);
    let group = byKey.get(key);
    if (!group) {
      group = {
        key,
        label: scopeGroupLabel(record.scope),
        scope: record.scope,
        records: [],
      };
      byKey.set(key, group);
    }
    group.records.push(record);
  }
  return Array.from(byKey.values()).sort((a, b) => {
    const rankDelta = scopeGroupRank(a.scope) - scopeGroupRank(b.scope);
    if (rankDelta !== 0) return rankDelta;
    return a.label.localeCompare(b.label);
  });
}

const TRUST_LABEL: Record<MemoryTrust, string> = {
  untrusted: "untrusted",
  agent_observed: "observed",
  agent_verified: "verified",
  application: "application",
  operator: "operator",
};

/// Lattice order (§6 of the architecture): untrusted < agent_observed <
/// agent_verified < application < operator.
const TRUST_RANK: Record<MemoryTrust, number> = {
  untrusted: 0,
  agent_observed: 1,
  agent_verified: 2,
  application: 3,
  operator: 4,
};

export function trustLabel(trust: MemoryTrust | undefined): string {
  return (trust && TRUST_LABEL[trust]) || String(trust || "unknown");
}

/// Visual tone bucket for a trust level, reusing the gpolicy state vocabulary.
export function trustTone(trust: MemoryTrust | undefined): "positive" | "neutral" | "muted" {
  switch (trust) {
    case "operator":
    case "application":
    case "agent_verified":
      return "positive";
    case "agent_observed":
      return "neutral";
    default:
      return "muted";
  }
}

export function statusLabel(status: MemoryRecordStatus | undefined): string {
  if (!status) return "unknown";
  switch (status.status) {
    case "active":
      return "active";
    case "superseded":
      return status.by ? `superseded → ${status.by}` : "superseded";
    case "quarantined":
      return status.reason ? `quarantined: ${status.reason}` : "quarantined";
    case "tombstoned":
      return "tombstoned";
  }
}

export function statusTone(
  status: MemoryRecordStatus | undefined,
): "positive" | "warning" | "muted" {
  if (!status) return "muted";
  switch (status.status) {
    case "active":
      return "positive";
    case "quarantined":
      return "warning";
    default:
      return "muted";
  }
}

const RELATIVE_UNITS: Array<[number, string]> = [
  [365 * 24 * 60 * 60 * 1000, "y"],
  [24 * 60 * 60 * 1000, "d"],
  [60 * 60 * 1000, "h"],
  [60 * 1000, "m"],
  [1000, "s"],
];

export function relativeAge(atMs: number | undefined, now = Date.now()): string {
  if (!atMs || atMs <= 0) return "—";
  const diff = now - atMs;
  if (diff < 0) return "now";
  if (diff < 1000) return "now";
  for (const [unitMs, suffix] of RELATIVE_UNITS) {
    if (diff >= unitMs) {
      return `${Math.floor(diff / unitMs)}${suffix} ago`;
    }
  }
  return "now";
}

/// Human label for a single evidence reference. Used as the click-through
/// button text and as the degraded fallback when the session is gone from
/// the timeline (the proposal's degrade rule).
export function evidenceLabel(evidence: MemoryEvidenceRef): string {
  const parts: string[] = [];
  if (evidence.session_id) parts.push(`session ${evidence.session_id}`);
  if (typeof evidence.generation === "number") parts.push(`gen ${evidence.generation}`);
  if (evidence.revision) parts.push(`rev ${evidence.revision}`);
  if (evidence.range && evidence.range.length === 2) {
    const [start, end] = evidence.range;
    parts.push(`msgs ${start}–${end}`);
  }
  return parts.join(" • ") || "evidence";
}

export function authorLine(author: MemoryAuthor | undefined): string {
  if (!author) return "unknown author";
  switch (author.author) {
    case "agent":
      return author.identity ? `agent ${author.identity}` : "agent";
    case "steward":
      return author.run_id ? `steward run ${author.run_id}` : "steward";
    case "distiller":
      return author.run_id ? `distiller run ${author.run_id}` : "distiller";
    case "operator":
      return "operator";
    case "application":
      return "application";
  }
}

/// Compact one-line summary of a dream run's op mix, e.g. "3 create · 1 tombstone".
export function dreamOpKindsSummary(opKinds: Record<string, number> | undefined): string {
  if (!opKinds) return "";
  const entries = Object.entries(opKinds)
    .filter(([, count]) => typeof count === "number" && count > 0)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  if (entries.length === 0) return "";
  return entries.map(([kind, count]) => `${count} ${kind}`).join(" · ");
}

export function dreamTimeRange(run: Pick<MemoryDreamRun, "first_op_at_ms" | "last_op_at_ms">, now = Date.now()): string {
  const first = run.first_op_at_ms;
  const last = run.last_op_at_ms;
  if (!first && !last) return "—";
  if (first && last && first !== last) {
    return `${relativeAge(first, now)} → ${relativeAge(last, now)}`;
  }
  return relativeAge(last || first, now);
}

export function injectionLine(injection: MemoryInjectionEntry, now = Date.now()): string {
  const surface = injection.surface === "build" ? "build" : "turn";
  return `${surface} • ${injection.identity} • ${relativeAge(injection.at_ms, now)}`;
}

/// Build panel/records params from the filter bar. Identity scope rides the
/// `identity` param; mob/operator ride `scope` + `scope_key` (the shapes the
/// server already accepts, `http_console.rs` handle_memory_panel_records).
export function buildRecordsQueryParams(
  filter: MemoryRecordsFilter,
  options: { cursor?: string; limit?: number } = {},
): Record<string, unknown> {
  const params: Record<string, unknown> = {};
  const key = filter.key?.trim();
  if (filter.scope === "identity" || (!filter.scope && key)) {
    if (key) params.identity = key;
    else params.scope = "identity";
  } else if (filter.scope === "mob" || filter.scope === "operator") {
    params.scope = filter.scope;
    if (key) params.scope_key = key;
  } else if (filter.scope === "realm") {
    params.scope = "realm";
  }
  if (filter.status) params.status = filter.status;
  if (filter.realm?.trim()) params.realm = filter.realm.trim();
  if (options.limit) params.limit = options.limit;
  if (options.cursor) params.cursor = options.cursor;
  return params;
}

export function hasActiveFilter(filter: MemoryRecordsFilter): boolean {
  return Boolean(filter.scope || filter.key?.trim() || filter.status || filter.realm?.trim());
}

export function filtersEquivalent(a: MemoryRecordsFilter, b: MemoryRecordsFilter): boolean {
  return (
    (a.scope || undefined) === (b.scope || undefined) &&
    (a.key?.trim() || "") === (b.key?.trim() || "") &&
    (a.status || undefined) === (b.status || undefined) &&
    (a.realm?.trim() || "") === (b.realm?.trim() || "")
  );
}

/// Message prefix of the error the headless layer throws when
/// meta/capabilities omits a method (`requireCapability`, headless.ts) —
/// raised client-side before any RPC is issued, so no rpcError code exists.
const CAPABILITY_MISS_PREFIX = "Meta capability missing for ";

/// Classify a memory-section RPC failure: -32030 is an ABAC denial (render
/// "no grant", never an empty store), -32601 means the panel is not wired on
/// this runtime, anything else is a real error. A per-method capability miss
/// also classifies as DENIED: under an enforced view the server intersects
/// meta/capabilities per principal (a scoped read grant drops panel/dreams
/// and panel/quarantine from the method list), so the miss IS denial by
/// intersection — it must degrade that section, never abort the whole panel.
/// Only the server's -32601 (store not configured) renders memory-unavailable.
export function memorySectionOutcome(error: unknown): "denied" | "unavailable" | "error" {
  const code = jsonRpcErrorCode(error);
  if (code === -32030) return "denied";
  if (code === -32601) return "unavailable";
  if (error instanceof Error && error.message.startsWith(CAPABILITY_MISS_PREFIX)) {
    return "denied";
  }
  return "error";
}

// ── Records list view + fetch orchestration ───────────────────────────────

export interface MemoryPagedState {
  records: MemoryPanelRecord[];
  nextCursor: string | null;
  /// The query behind this page (or its load-more continuation) was denied
  /// (-32030). Renders "no grant" — a denied scope must never be
  /// indistinguishable from an empty store.
  denied?: boolean;
}

export interface RecordsListView {
  mode: "grouped" | "flat";
  /// Flat display order (mode "flat"): the accumulated page, utility-sorted
  /// when the sort mode asks for it.
  records: MemoryPanelRecord[];
  /// Grouped display (mode "grouped") — built from the SAME accumulated
  /// source load-more appends to, so appended rows always render.
  groups: MemoryScopeGroup[];
  cursor: string | null;
  denied: boolean;
}

export function buildRecordsListView(args: {
  records: MemoryPanelRecord[];
  paged: MemoryPagedState | null;
  baseCursor: string | null;
  filter: MemoryRecordsFilter;
  sortMode: "recency" | "utility";
}): RecordsListView {
  const listed = args.paged ? args.paged.records : args.records;
  const mode = hasActiveFilter(args.filter) || args.sortMode === "utility" ? "flat" : "grouped";
  return {
    mode,
    records: args.sortMode === "utility" ? sortRecordsByUtility(listed) : listed,
    groups: mode === "grouped" ? groupRecordsByScope(listed) : [],
    cursor: args.paged ? args.paged.nextCursor : args.baseCursor,
    denied: args.paged?.denied === true,
  };
}

export interface MemoryRecordsPagerDeps {
  /// Resolves the page, or null when the query was DENIED (-32030). Other
  /// failures must throw — the pager keeps the prior page for those (the
  /// caller surfaces them via its own error banner).
  query: (params: Record<string, unknown>) => Promise<MemoryPanelRecordsResult | null>;
  setPaged: (paged: MemoryPagedState | null) => void;
  setLoading: (loading: boolean) => void;
}

/// Fetch orchestration for the Records tab (filtered queries + keyset
/// load-more), framework-free so the race behavior is unit-testable. A
/// monotonic request sequence makes responses last-write-wins by ISSUE
/// order: a slow broad query resolving after a narrower one issued later is
/// dropped, never applied.
export function createMemoryRecordsPager(deps: MemoryRecordsPagerDeps) {
  let seqCounter = 0;
  let applied: MemoryRecordsFilter = {};
  const pager = {
    appliedFilter(): MemoryRecordsFilter {
      return applied;
    },
    async applyFilter(next: MemoryRecordsFilter): Promise<void> {
      applied = next;
      // Whatever an in-flight fetch was for, it is stale now.
      const seq = ++seqCounter;
      if (!hasActiveFilter(next)) {
        deps.setPaged(null);
        deps.setLoading(false);
        return;
      }
      deps.setLoading(true);
      try {
        const result = await deps.query(buildRecordsQueryParams(next));
        if (seq !== seqCounter) return;
        if (result === null) {
          // Denied filtered query: mark it so the view says "no grant",
          // never "no memory records yet".
          deps.setPaged({ records: [], nextCursor: null, denied: true });
        } else {
          deps.setPaged({
            records: result.records || [],
            nextCursor: result.next_cursor ?? null,
          });
        }
      } catch {
        // Non-denial error: keep whatever page was showing; the transport
        // layer surfaces the error banner.
      } finally {
        if (seq === seqCounter) deps.setLoading(false);
      }
    },
    /// Blur-path re-apply: only when the value actually changed. A blur
    /// caused by clicking load-more must not re-issue the query and race
    /// the append.
    async applyFilterIfChanged(next: MemoryRecordsFilter): Promise<void> {
      if (filtersEquivalent(next, applied)) return;
      await pager.applyFilter(next);
    },
    async loadMore(current: {
      filter: MemoryRecordsFilter;
      paged: MemoryPagedState | null;
      baseRecords: MemoryPanelRecord[];
      baseCursor: string | null;
    }): Promise<void> {
      const cursor = current.paged ? current.paged.nextCursor : current.baseCursor;
      if (!cursor) return;
      const seq = ++seqCounter;
      deps.setLoading(true);
      try {
        const result = await deps.query(
          buildRecordsQueryParams(current.filter, { cursor }),
        );
        if (seq !== seqCounter) return;
        const base = current.paged ? current.paged.records : current.baseRecords;
        if (result === null) {
          // Denied continuation: keep the rows already shown, drop the
          // cursor, and flag the denial.
          deps.setPaged({ records: base, nextCursor: null, denied: true });
        } else {
          deps.setPaged({
            records: [...base, ...(result.records || [])],
            nextCursor: result.next_cursor ?? null,
          });
        }
      } catch {
        // Non-denial error: keep the current page.
      } finally {
        if (seq === seqCounter) deps.setLoading(false);
      }
    },
  };
  return pager;
}

// ── Recall utility (§5 RECALL) ────────────────────────────────────────────

export interface RecordUtility {
  injected: number;
  recalled: number;
  useful: number;
  /// judged_useful / injected; null when never injected.
  ratio: number | null;
  /// Estimated cumulative bytes from injection count and record size.
  bytesSpent: number;
  dead: boolean;
}

/// A record is DEAD weight when it keeps getting injected but has never been
/// judged useful (§8 claim: recall earns its bytes).
export const DEAD_INJECTION_THRESHOLD = 3;

export function recordUtility(
  record: Pick<MemoryPanelRecord, "usage" | "body_bytes">,
): RecordUtility {
  const injected = record.usage?.injected_count ?? 0;
  const recalled = record.usage?.explicit_recall_count ?? 0;
  const useful = record.usage?.judged_useful_count ?? 0;
  return {
    injected,
    recalled,
    useful,
    ratio: injected > 0 ? useful / injected : null,
    bytesSpent: injected * (record.body_bytes ?? 0),
    dead: injected >= DEAD_INJECTION_THRESHOLD && useful === 0,
  };
}

/// Utility ordering: dead weight first (most bytes wasted at the top), then
/// ascending usefulness ratio — the rows most likely to need demotion lead.
export function sortRecordsByUtility(records: MemoryPanelRecord[]): MemoryPanelRecord[] {
  return [...records].sort((a, b) => {
    const ua = recordUtility(a);
    const ub = recordUtility(b);
    if (ua.dead !== ub.dead) return ua.dead ? -1 : 1;
    const ra = ua.ratio ?? Number.POSITIVE_INFINITY;
    const rb = ub.ratio ?? Number.POSITIVE_INFINITY;
    if (ra !== rb) return ra - rb;
    return ub.bytesSpent - ua.bytesSpent;
  });
}

export function utilityLine(record: Pick<MemoryPanelRecord, "usage" | "body_bytes">): string {
  const u = recordUtility(record);
  const ratio = u.ratio === null ? "—" : u.ratio.toFixed(2);
  return `inj ${u.injected} · recall ${u.recalled} · useful ${u.useful} · ratio ${ratio} · ~${formatBytes(u.bytesSpent)} spent`;
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0B";
  if (bytes < 1024) return `${Math.round(bytes)}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)}MB`;
}

// ── Lattice audit (§5 LATTICE, invariants a + c) ──────────────────────────

/// Client page-walk ceiling (open Q4): 2,000 rows at 200/page is ten requests
/// against the fetch-on-demand plane. Walks past the cap stay honest via the
/// "checked N" partial header instead of masquerading as complete.
export const LATTICE_WALK_MAX_RECORDS = 2000;
export const LATTICE_WALK_PAGE_LIMIT = 200;

/// A violating record with enough context to open its Biography.
export interface LatticeViolationRef {
  id: string;
  realm: string;
}

export interface LatticeWalkResult {
  checked: number;
  /// True when the walk exhausted the store (every realm's keyset cursor
  /// ran out) rather than hitting the shared cap.
  complete: boolean;
  /// Invariant (a): LLM-authored records (agent/distiller/steward) above the
  /// agent_observed ceiling. Design intent: permanently empty.
  llmCeilingViolations: LatticeViolationRef[];
  /// Invariant (c): supersede cycles, plus dangling supersede targets when
  /// the walk is complete (dangling is only provable over the whole store).
  chainViolations: LatticeViolationRef[];
}

export function latticeInvariants(
  records: MemoryPanelRecord[],
  options: { complete: boolean },
): Pick<LatticeWalkResult, "llmCeilingViolations" | "chainViolations"> {
  const llmCeilingViolations: LatticeViolationRef[] = [];
  const byId = new Map(records.map((record) => [record.id, record]));
  for (const record of records) {
    const author = record.provenance?.author?.author;
    const rank = record.trust ? TRUST_RANK[record.trust] : undefined;
    if (
      (author === "agent" || author === "distiller" || author === "steward") &&
      typeof rank === "number" &&
      rank > TRUST_RANK.agent_observed
    ) {
      llmCeilingViolations.push({ id: record.id, realm: realmOfRecord(record) });
    }
  }
  const chainViolations = new Map<string, LatticeViolationRef>();
  for (const record of records) {
    // Follow supersede pointers; revisiting a node inside the current path
    // is a cycle. Pointers that leave the fetched set only count as dangling
    // when the walk covered the whole store.
    const path = new Set<string>([record.id]);
    let cursor = record.supersedes;
    while (cursor) {
      if (path.has(cursor)) {
        chainViolations.set(record.id, { id: record.id, realm: realmOfRecord(record) });
        break;
      }
      const parent = byId.get(cursor);
      if (!parent) {
        if (options.complete) {
          chainViolations.set(record.id, { id: record.id, realm: realmOfRecord(record) });
        }
        break;
      }
      path.add(cursor);
      cursor = parent.supersedes;
    }
  }
  return {
    llmCeilingViolations,
    chainViolations: Array.from(chainViolations.values()),
  };
}

/// Cheap content fingerprint over the loaded base page. The lattice walk
/// re-runs only when this changes, so content-identical debounced refreshes
/// (memory.* bursts re-read every 250ms) skip the up-to-10-RPC walk.
export function latticeFingerprint(
  records: MemoryPanelRecord[],
  realms: string[],
  baseCursor: string | null,
): string {
  const rows = records
    .map(
      (record) =>
        `${record.id}:${record.supersedes || ""}:${record.trust}:${record.status?.status || ""}:${record.updated_at_ms || 0}`,
    )
    .join("|");
  return `${realms.join(",")}#${baseCursor || ""}#${records.length}#${rows}`;
}

/// Walk the store page by page. Single-realm gateways follow the keyset
/// cursor directly; multi-realm gateways walk EACH realm independently via
/// the server-supported `realm` param (merged multi-realm pages never carry
/// a cursor and are truncated, so an unscoped walk can neither continue nor
/// honestly claim completeness). `complete` = every realm exhausted its
/// cursor within the shared cap.
export async function runLatticeWalk(
  fetchPage: (params: Record<string, unknown>) => Promise<MemoryPanelRecordsResult | null>,
  options: {
    realms: string[];
    maxRecords?: number;
    /// Probed before each page fetch so a superseded walk stops issuing
    /// RPCs instead of merely having its result discarded.
    isCancelled?: () => boolean;
  },
): Promise<LatticeWalkResult | null> {
  const max = options.maxRecords ?? LATTICE_WALK_MAX_RECORDS;
  const isCancelled = options.isCancelled || (() => false);
  // With zero/one known realm the unscoped listing is already single-realm.
  const realmParams: Array<string | undefined> =
    options.realms.length > 1 ? options.realms : [undefined];
  const all: MemoryPanelRecord[] = [];
  let exhaustedEverywhere = true;
  for (const realm of realmParams) {
    let cursor: string | undefined;
    for (;;) {
      if (isCancelled()) return null;
      if (all.length >= max) {
        exhaustedEverywhere = false;
        break;
      }
      const params: Record<string, unknown> = { limit: LATTICE_WALK_PAGE_LIMIT };
      if (realm) params.realm = realm;
      if (cursor) params.cursor = cursor;
      const page = await fetchPage(params);
      if (page === null) return null; // access denied → tile renders no grant
      all.push(...(page.records || []));
      if (!page.next_cursor) break; // this realm's cursor is exhausted
      cursor = page.next_cursor;
    }
    if (!exhaustedEverywhere) break;
  }
  const checked = all.slice(0, max);
  const complete = exhaustedEverywhere && all.length <= max;
  const invariants = latticeInvariants(checked, { complete });
  return {
    checked: checked.length,
    complete,
    ...invariants,
  };
}

// ── Verdict tiles (§3.1 strip / §5 widgets) ───────────────────────────────

export type VerdictStatus = "holding" | "degraded" | "violated" | "unverifiable" | "no-grant";

export interface VerdictTile {
  id: "echo-safety" | "taint-wall" | "lattice" | "recall" | "dreams" | "store-floor";
  label: string;
  status: VerdictStatus;
  lines: string[];
  /// A settled verdict whose page-walk is running again. It renders over the
  /// tile's corner, outside the line flow.
  rechecking?: boolean;
  /// Tiles are doors: clicking opens the tab holding the evidence.
  targetTab: MemoryTab;
  /// The rows that made the verdict red — each opens its Biography directly
  /// (capped; an "+N more" line accompanies when truncated).
  evidence?: LatticeViolationRef[];
}

/// How many violating record ids render on a VIOLATED tile before "+N more".
export const VERDICT_EVIDENCE_MAX = 5;

const VERDICT_STATUS_LABEL: Record<VerdictStatus, string> = {
  holding: "HOLDING",
  degraded: "DEGRADED",
  violated: "VIOLATED",
  unverifiable: "UNVERIFIABLE",
  "no-grant": "NO GRANT",
};

export function verdictStatusLabel(status: VerdictStatus): string {
  return VERDICT_STATUS_LABEL[status];
}

export interface VerdictInputs {
  records: MemoryPanelRecord[];
  recordsDenied: boolean;
  dreams: MemoryDreamRun[];
  dreamsDenied: boolean;
  lattice: LatticeWalkResult | null;
  latticeRunning?: boolean;
  /// Store overview (panel/overview), with scopes ALREADY filtered through
  /// visibleOverviewScopes — a denied scope's floor pressure must not leak
  /// into the tile. Null while the surface has not answered.
  overview?: {
    scopes: MemoryScopeOverview[];
    floors?: { records?: number; bytes?: number };
  } | null;
  overviewDenied?: boolean;
  now?: number;
}

export function computeVerdictTiles(inputs: VerdictInputs): VerdictTile[] {
  const now = inputs.now ?? Date.now();
  const tiles: VerdictTile[] = [];

  // Phase-1 boot state (§7): echo-safety, taint wall, and store floor name
  // the exact missing surface instead of guessing from partial data.
  tiles.push({
    id: "echo-safety",
    label: "ECHO-SAFETY",
    status: "unverifiable",
    lines: ["Automatic echo-safety assessment is not available here."],
    targetTab: "knowledge",
  });
  tiles.push({
    id: "taint-wall",
    label: "TAINT WALL",
    status: inputs.recordsDenied ? "no-grant" : "unverifiable",
    lines: inputs.recordsDenied
      ? ["records not readable by this principal"]
      : ["Automatic taint-wall assessment is not available here."],
    targetTab: "pipeline",
  });

  if (inputs.recordsDenied) {
    tiles.push({
      id: "lattice",
      label: "LATTICE",
      status: "no-grant",
      lines: ["records not readable by this principal"],
      targetTab: "records",
    });
  } else if (!inputs.lattice) {
    tiles.push({
      id: "lattice",
      label: "LATTICE",
      status: "unverifiable",
      lines: [inputs.latticeRunning ? "page-walk running…" : "page-walk not run"],
      targetTab: "records",
    });
  } else {
    // A settled verdict is retained while a re-check runs — the tile must
    // not flicker to UNVERIFIABLE on every debounced live refresh. The
    // re-check shows over the tile's corner rather than as another line, so
    // it never grows the tile row and moves the tiles below.
    const walk = inputs.lattice;
    const violations = [...walk.llmCeilingViolations, ...walk.chainViolations];
    tiles.push({
      id: "lattice",
      label: "LATTICE",
      status: violations.length > 0 ? "violated" : "holding",
      lines: [
        violations.length > 0
          ? `${violations.length} violation${violations.length === 1 ? "" : "s"}`
          : "0 violations",
        walk.complete
          ? `checked ${walk.checked}/${walk.checked}`
          : `checked first ${walk.checked} — partial (cap ${LATTICE_WALK_MAX_RECORDS})`,
        "invariant (b) needs ever_quarantined (surface 2)",
        ...(violations.length > VERDICT_EVIDENCE_MAX
          ? [`+${violations.length - VERDICT_EVIDENCE_MAX} more violations`]
          : []),
      ],
      rechecking: inputs.latticeRunning === true,
      targetTab: "records",
      evidence: violations.slice(0, VERDICT_EVIDENCE_MAX),
    });
  }

  if (inputs.recordsDenied) {
    tiles.push({
      id: "recall",
      label: "RECALL",
      status: "no-grant",
      lines: ["records not readable by this principal"],
      targetTab: "records",
    });
  } else {
    const dead = inputs.records.filter((record) => recordUtility(record).dead);
    const deadBytes = dead.reduce((sum, record) => sum + recordUtility(record).bytesSpent, 0);
    tiles.push({
      id: "recall",
      label: "RECALL",
      status: dead.length > 0 ? "degraded" : "holding",
      lines: [
        `${dead.length} dead weight of ${inputs.records.length} loaded`,
        `~${formatBytes(deadBytes)} spent (approx)`,
      ],
      targetTab: "records",
    });
  }

  if (inputs.dreamsDenied) {
    tiles.push({
      id: "dreams",
      label: "DREAMS",
      status: "no-grant",
      lines: ["dream audit not readable by this principal"],
      targetTab: "dreams",
    });
  } else if (inputs.dreams.length === 0) {
    tiles.push({
      id: "dreams",
      label: "DREAMS",
      status: "unverifiable",
      lines: ["no dream runs in the durable audit yet"],
      targetTab: "dreams",
    });
  } else {
    const lastOp = Math.max(
      ...inputs.dreams.map((run) => run.last_op_at_ms || run.first_op_at_ms || 0),
    );
    const quarantined = inputs.dreams.reduce((sum, run) => sum + (run.quarantined_ops || 0), 0);
    tiles.push({
      id: "dreams",
      label: "DREAMS",
      status: "holding",
      lines: [
        `last run ${relativeAge(lastOp, now)}`,
        quarantined > 0 ? `⚠ ${quarantined} quarantined ops` : "0 quarantined ops",
        "verdict sheet needs persisted DreamRun (surface 11)",
      ],
      targetTab: "dreams",
    });
  }

  if (inputs.overviewDenied) {
    tiles.push({
      id: "store-floor",
      label: "STORE FLOOR",
      status: "no-grant",
      lines: ["store overview not readable by this principal"],
      targetTab: "holdings",
    });
  } else if (!inputs.overview) {
    tiles.push({
      id: "store-floor",
      label: "STORE FLOOR",
      status: "unverifiable",
      lines: ["needs meta/memory/panel/overview (surface 1)"],
      targetTab: "holdings",
    });
  } else {
    const floor = storeFloorVerdict(inputs.overview.scopes);
    const floors = inputs.overview.floors;
    const floorLine = floors
      ? `floors ${floors.records ?? "?"} records / ${
          typeof floors.bytes === "number" ? formatBytes(floors.bytes) : "?"
        } per scope`
      : "floors unreported";
    tiles.push({
      id: "store-floor",
      label: "STORE FLOOR",
      status: floor.status === "ok" ? "holding" : "degraded",
      lines:
        floor.status === "ok"
          ? [
              `OK — no scope at floor pressure (${inputs.overview.scopes.length} scopes)`,
              floorLine,
            ]
          : [
              `PRESSURE — ${floor.pressured.length} scope${
                floor.pressured.length === 1 ? "" : "s"
              } at floor`,
              floor.pressured.map((scope) => overviewScopeLabel(scope)).join(" · "),
              floorLine,
            ],
      targetTab: "holdings",
    });
  }

  return tiles;
}

// ── Holdings scope overview (client-side, from loaded rows) ───────────────

export interface ScopeOverviewRow {
  key: string;
  label: string;
  scope: MemoryRecordScope;
  active: number;
  quarantined: number;
  superseded: number;
  tombstoned: number;
  bytes: number;
  trustMix: string;
}

export function scopeOverviewRows(records: MemoryPanelRecord[]): ScopeOverviewRow[] {
  return groupRecordsByScope(records).map((group) => {
    const counts = { active: 0, quarantined: 0, superseded: 0, tombstoned: 0 };
    let bytes = 0;
    const trustCounts = new Map<string, number>();
    for (const record of group.records) {
      const status = record.status?.status;
      if (status && status in counts) counts[status as keyof typeof counts] += 1;
      bytes += record.body_bytes ?? 0;
      const trust = trustLabel(record.trust);
      trustCounts.set(trust, (trustCounts.get(trust) || 0) + 1);
    }
    const trustMix = Array.from(trustCounts.entries())
      .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
      .map(([trust, count]) => `${count} ${trust}`)
      .join(" · ");
    return {
      key: group.key,
      label: group.label,
      scope: group.scope,
      ...counts,
      bytes,
      trustMix,
    };
  });
}

/// Filter-bar preset that reproduces a scope row's population.
export function filterForScope(scope: MemoryRecordScope): MemoryRecordsFilter {
  switch (scope.scope) {
    case "identity":
      return { scope: "identity", key: scope.identity };
    case "mob":
      return { scope: "mob", key: scope.mob };
    case "operator":
      return { scope: "operator", key: scope.operator };
    case "realm":
      return { scope: "realm" };
  }
}

// ── Phase-2: overview scopes + store floor (§3.1 / §5 STORE FLOOR) ────────

/// Group key for an overview scope row, matching scopeGroupKey so testids
/// and pivots line up with the loaded-records fallback rows.
export function overviewScopeKey(scope: MemoryScopeOverview): string {
  if (scope.scope_kind === "realm") return `realm:${scope.realm}`;
  return `${scope.scope_kind}:${scope.realm}:${scope.scope_key}`;
}

export function overviewScopeLabel(scope: MemoryScopeOverview): string {
  switch (scope.scope_kind) {
    case "identity":
      return scope.scope_key;
    case "mob":
      return `Mob: ${scope.scope_key}`;
    case "operator":
      return `Operator: ${scope.scope_key}`;
    case "realm":
      return "Realm";
    default:
      return `${scope.scope_kind}:${scope.scope_key}`;
  }
}

/// Records-filter preset reproducing an overview scope row's population
/// (parity with filterForScope over loaded rows).
export function filterForOverviewScope(scope: MemoryScopeOverview): MemoryRecordsFilter {
  if (scope.scope_kind === "realm") return { scope: "realm" };
  if (
    scope.scope_kind === "identity" ||
    scope.scope_kind === "mob" ||
    scope.scope_kind === "operator"
  ) {
    return { scope: scope.scope_kind, key: scope.scope_key };
  }
  return {};
}

/// Drop overview rows for scope kinds whose one-row probe was denied: a
/// denied scope renders the access-denied-tone row, never its counts (the
/// listing row-filters them; the aggregate must not leak what the rows hide).
export function visibleOverviewScopes(
  scopes: MemoryScopeOverview[],
  denied: { operatorScopeDenied?: boolean; mobScopeDenied?: boolean },
): MemoryScopeOverview[] {
  return scopes.filter((scope) => {
    if (scope.scope_kind === "operator" && denied.operatorScopeDenied) return false;
    if (scope.scope_kind === "mob" && denied.mobScopeDenied) return false;
    return true;
  });
}

/// Rank overview rows like the loaded-records grouping: identities first
/// (alphabetically), then mob, operator, realm.
export function sortOverviewScopes(scopes: MemoryScopeOverview[]): MemoryScopeOverview[] {
  const rank = (kind: string): number => {
    switch (kind) {
      case "identity":
        return 0;
      case "mob":
        return 1;
      case "operator":
        return 2;
      case "realm":
        return 3;
      default:
        return 4;
    }
  };
  return [...scopes].sort((a, b) => {
    const delta = rank(a.scope_kind) - rank(b.scope_kind);
    if (delta !== 0) return delta;
    return overviewScopeLabel(a).localeCompare(overviewScopeLabel(b));
  });
}

export interface StoreFloorVerdict {
  status: "ok" | "pressure";
  pressured: MemoryScopeOverview[];
}

/// STORE FLOOR verdict over the visible overview rows: OK when no scope
/// reports floor pressure, PRESSURE otherwise (§7.3: floors warn,
/// deterministic code never evicts).
export function storeFloorVerdict(scopes: MemoryScopeOverview[]): StoreFloorVerdict {
  const pressured = scopes.filter((scope) => scope.floor_pressure === true);
  return { status: pressured.length > 0 ? "pressure" : "ok", pressured };
}

// ── Phase-2: injection ledger DUP annotation (§3.3 / §5 ECHO-SAFETY) ──────

export interface AnnotatedInjection {
  entry: MemoryLedgerEntry;
  /// The same identity's immediately-previous ledger row injected the same
  /// record — the consecutive-duplicate tripwire for the historical
  /// ~18.5KB/turn echo defect.
  dup: boolean;
}

/// Rows arrive newest-first from panel/injections; dup is computed against
/// each identity's next-older row.
export function annotateInjectionDups(entries: MemoryLedgerEntry[]): AnnotatedInjection[] {
  const flags = new Array<boolean>(entries.length).fill(false);
  const previousByIdentity = new Map<string, string>();
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index];
    flags[index] = previousByIdentity.get(entry.identity) === entry.record_id;
    previousByIdentity.set(entry.identity, entry.record_id);
  }
  return entries.map((entry, index) => ({ entry, dup: flags[index] }));
}

// ── Phase-2: durable dream verdict sheets (§3.5) ──────────────────────────

export function dreamRunsNewestFirst(runs: MemoryDreamRunSheet[]): MemoryDreamRunSheet[] {
  return [...runs].sort(
    (a, b) =>
      (b.completed_at_ms ?? b.started_at_ms ?? 0) - (a.completed_at_ms ?? a.started_at_ms ?? 0),
  );
}

export function formatDurationMs(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60 * 1000) return `${(ms / 1000).toFixed(1)}s`;
  const minutes = Math.floor(ms / (60 * 1000));
  const seconds = Math.round((ms % (60 * 1000)) / 1000);
  return `${minutes}m ${seconds}s`;
}

export function dreamRunDuration(
  run: Pick<MemoryDreamRunSheet, "started_at_ms" | "completed_at_ms">,
): string {
  if (!run.started_at_ms || !run.completed_at_ms) return "—";
  return formatDurationMs(run.completed_at_ms - run.started_at_ms);
}

export interface NormalizedDreamRunDetail {
  phases: Array<[string, string]>;
  /// Non-zero verdict counters, in the steward's declaration order.
  verdicts: Array<[string, number]>;
  skips: string[];
  /// Set when the stored detail did not parse as the expected shape — the
  /// sheet degrades to the raw text instead of pretending it was empty.
  raw: string | null;
}

export function normalizeDreamRunDetail(
  detail: MemoryDreamRunDetail | string | undefined,
): NormalizedDreamRunDetail {
  if (detail === undefined || detail === null) {
    return { phases: [], verdicts: [], skips: [], raw: null };
  }
  if (typeof detail === "string") {
    return { phases: [], verdicts: [], skips: [], raw: detail };
  }
  const phases: Array<[string, string]> = [];
  for (const phase of detail.phases || []) {
    if (Array.isArray(phase) && phase.length >= 1) {
      phases.push([String(phase[0]), String(phase[1] ?? "")]);
    }
  }
  const verdicts: Array<[string, number]> = Object.entries(detail.verdicts || {}).filter(
    (candidate): candidate is [string, number] =>
      typeof candidate[1] === "number" && candidate[1] > 0,
  );
  const skips = (detail.skips || []).map((skip) => String(skip));
  return { phases, verdicts, skips, raw: null };
}

// ── Biography helpers ─────────────────────────────────────────────────────

/// Display order for the LINEAGE lane: newest first (current record at the
/// top, dimmed ancestors below), from the server's oldest→newest chain.
export function lineageLane(
  chain: MemoryPanelRecord[],
  currentId: string,
): Array<{ record: MemoryPanelRecord; current: boolean }> {
  return [...chain].reverse().map((record) => ({
    record,
    current: record.id === currentId,
  }));
}

/// Lossy client-side join: dream runs whose ≤12-record `memory_ids` sample
/// includes this record. Exact history needs the `history[]` field (surface 7).
export function dreamRunsTouching(dreams: MemoryDreamRun[], recordId: string): MemoryDreamRun[] {
  return dreams.filter((run) => (run.memory_ids || []).includes(recordId));
}

export interface EvidenceExcerptLine {
  id: string;
  speaker: string;
  text: string;
}

/// Project adapter-formatted conversation entries into excerpt lines. The
/// message range indexes evidence against session generations, which the
/// console timeline only approximates — the excerpt says so.
export function evidenceExcerptLines(
  entries: ConversationTimelineEntry[],
  range?: [number, number],
  maxLines = 30,
): EvidenceExcerptLine[] {
  let window = entries;
  if (range && range.length === 2) {
    const [start, end] = range;
    if (Number.isFinite(start) && Number.isFinite(end) && end >= start) {
      const from = Math.max(0, Math.min(start, entries.length));
      const to = Math.max(from, Math.min(end + 1, entries.length));
      const sliced = entries.slice(from, to);
      if (sliced.length > 0) window = sliced;
    }
  }
  const lines: EvidenceExcerptLine[] = [];
  for (const entry of window) {
    if (entry.kind !== "message") continue;
    const text = (entry.text || entry.copyText || "").trim();
    if (!text) continue;
    lines.push({ id: entry.id, speaker: entry.identity.label, text });
    if (lines.length >= maxLines) break;
  }
  return lines;
}

// ── Knowledge Lens composition (§3.3 phase-1 slice) ───────────────────────

export function identityOptions(records: MemoryPanelRecord[]): string[] {
  const identities = new Set<string>();
  for (const record of records) {
    if (record.scope.scope === "identity") identities.add(record.scope.identity);
  }
  return Array.from(identities).sort((a, b) => a.localeCompare(b));
}

export interface KnowledgeSegment {
  label: string;
  count: number;
  filter: MemoryRecordsFilter;
  /// Mob membership is not resolvable client-side in phase 1, so mob /
  /// operator / realm segments cover all records of that scope kind.
  approximate: boolean;
}

export function knowledgeComposition(
  records: MemoryPanelRecord[],
  identity: string,
): KnowledgeSegment[] {
  const count = (predicate: (record: MemoryPanelRecord) => boolean) =>
    records.filter(predicate).length;
  return [
    {
      label: `identity:${identity}`,
      count: count(
        (record) => record.scope.scope === "identity" && record.scope.identity === identity,
      ),
      filter: { scope: "identity", key: identity },
      approximate: false,
    },
    {
      label: "mob (all mobs)",
      count: count((record) => record.scope.scope === "mob"),
      filter: { scope: "mob" },
      approximate: true,
    },
    {
      label: "operator",
      count: count((record) => record.scope.scope === "operator"),
      filter: { scope: "operator" },
      approximate: true,
    },
    {
      label: "realm",
      count: count((record) => record.scope.scope === "realm"),
      filter: { scope: "realm" },
      approximate: true,
    },
  ];
}

// ── Live strip helpers (live-follow discipline + ring seam) ───────────────

export function dedupeFramesById(frames: ConsoleFrame[]): ConsoleFrame[] {
  const seen = new Set<string>();
  const result: ConsoleFrame[] = [];
  for (const frame of frames) {
    const key = frame.id || `${frame.event}:${frame.timestampMs || 0}`;
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(frame);
  }
  return result;
}

export function countFramesBehind(live: ConsoleFrame[], frozen: ConsoleFrame[]): number {
  const frozenIds = new Set(frozen.map((frame) => frame.id));
  return live.filter((frame) => !frozenIds.has(frame.id)).length;
}

/// "State here" pivot payload for a live memory frame: the durable record the
/// event talks about, if the payload names one.
export function memoryFramePivot(
  frame: Pick<ConsoleFrame, "event" | "data">,
): { recordId: string; realm?: string } | null {
  if (!frame.event.startsWith("memory.")) return null;
  const data = frame.data && typeof frame.data === "object"
    ? (frame.data as Record<string, unknown>)
    : {};
  const recordId = typeof data.record_id === "string" && data.record_id.trim()
    ? data.record_id.trim()
    : null;
  if (!recordId) return null;
  const realm = typeof data.realm === "string" && data.realm.trim() ? data.realm.trim() : undefined;
  return { recordId, realm };
}

/// A Biography offers the quarantine decision for a quarantined
/// identity-scope record whose detail carries the content hash to bind.
function canDecideQuarantinedRecord(record: MemoryFullRecord): boolean {
  return (
    record.status.status === "quarantined" &&
    record.scope.scope === "identity" &&
    typeof record.content_hash === "string" &&
    record.content_hash.length > 0
  );
}

/// The server's MAX_REVIEW_RATIONALE_BYTES: a review rationale is limited in
/// UTF-8 bytes of its trimmed text, not in characters.
const QUARANTINE_RATIONALE_MAX_BYTES = 400;

/// Why a rationale cannot be sent as typed, or null when it fits. Measured
/// on the trimmed text, which is what the request carries.
function quarantineRationaleProblem(rationale: string): string | null {
  const bytes = new TextEncoder().encode(rationale.trim()).length;
  return bytes > QUARANTINE_RATIONALE_MAX_BYTES
    ? `The rationale is ${bytes} bytes (UTF-8); the limit is ${QUARANTINE_RATIONALE_MAX_BYTES}.`
    : null;
}

/// One line describing what a quarantine decision did.
function quarantineDecisionSummary(result: MemoryQuarantineDecideResult): string {
  const successor = result.successor?.memory_id;
  switch (result.outcome) {
    case "released":
      return `Released as ${successor ?? "a new record"} (agent_observed, ever-quarantined); the original is kept, tombstoned.`;
    case "already_released":
      return `Already released as ${successor ?? "a new record"}; nothing changed.`;
    case "tombstoned": {
      const invalidated = result.decision?.review?.invalidated_promotions ?? [];
      return invalidated.length > 0
        ? `Tombstoned; it will never be recalled. Pending promotion ${invalidated.join(", ")} invalidated; it can no longer publish.`
        : "Tombstoned; it will never be recalled.";
    }
    case "already_tombstoned":
      return "Already tombstoned; nothing changed.";
    default:
      return `Decided: ${String(result.outcome)}`;
  }
}

/// Human text for a failed quarantine decision: the typed refusal reasons
/// of meta/memory/quarantine/decide, the access denial, or the raw error.
function quarantineDecisionErrorText(error: unknown): string {
  const rpcError = (error as { rpcError?: { code?: unknown; data?: unknown } } | null)?.rpcError;
  if (rpcError?.code === -32030) {
    return "No grant: deciding needs memory.quarantine.review plus view, memory read and memory write (release) or delete (tombstone) on this identity.";
  }
  const data = (rpcError?.data ?? null) as Record<string, unknown> | null;
  if (data?.kind === "memory_quarantine_review_refused") {
    switch (data.reason) {
      case "not_found":
        return "Refused: no such record in this identity's scope.";
      case "content_mismatch":
        return "Refused: the record differs from the one loaded here. Reload it and review again.";
      case "not_quarantined":
        return data.released_as
          ? `Refused: it was already released as ${String(data.released_as)}.`
          : `Refused: the record is ${String(data.status)}, not quarantined.`;
      case "gate_pending": {
        const expires =
          typeof data.expires_at_ms === "number"
            ? ` (or wait until it expires, ${new Date(data.expires_at_ms).toISOString()})`
            : "";
        return `Refused: gated promotion ${String(data.pending_id)} is waiting on it; decide it in the Gating inbox${expires}. Tombstoning the record instead invalidates that promotion.`;
      }
      case "successor_conflict":
        return `Refused: another record already holds the release id ${String(data.successor_id)}; nothing was changed.`;
      case "secret_detected":
        return `Refused: the content matches the ${String(data.class)} secret pattern; tombstone is the only exit.`;
      case "stale_update":
        return `Refused: the version this update replaces (${String(data.prior)}) is ${String(data.prior_status)}; tombstone it instead.`;
      default:
        break;
    }
  }
  return errorMessage(error);
}

export const __memoryTest = {
  scopeGroupKey,
  scopeGroupLabel,
  groupRecordsByScope,
  trustLabel,
  trustTone,
  statusLabel,
  statusTone,
  relativeAge,
  evidenceLabel,
  authorLine,
  dreamOpKindsSummary,
  dreamTimeRange,
  injectionLine,
  realmOfRecord,
  // Phase-1 additions
  memoryTabLabel,
  resolveMemoryTabAlias,
  buildRecordsQueryParams,
  hasActiveFilter,
  filtersEquivalent,
  memorySectionOutcome,
  buildRecordsListView,
  createMemoryRecordsPager,
  latticeFingerprint,
  recordUtility,
  sortRecordsByUtility,
  utilityLine,
  formatBytes,
  latticeInvariants,
  runLatticeWalk,
  computeVerdictTiles,
  verdictStatusLabel,
  scopeOverviewRows,
  filterForScope,
  lineageLane,
  dreamRunsTouching,
  evidenceExcerptLines,
  identityOptions,
  knowledgeComposition,
  dedupeFramesById,
  countFramesBehind,
  memoryFramePivot,
  // Phase-2 additions
  overviewScopeKey,
  overviewScopeLabel,
  filterForOverviewScope,
  visibleOverviewScopes,
  sortOverviewScopes,
  storeFloorVerdict,
  annotateInjectionDups,
  dreamRunsNewestFirst,
  formatDurationMs,
  dreamRunDuration,
  normalizeDreamRunDetail,
  canDecideQuarantinedRecord,
  quarantineRationaleProblem,
  quarantineDecisionSummary,
  quarantineDecisionErrorText,
};

// ── Presentational sub-components ─────────────────────────────────────────

function Chip({ label, tone }: { label: string; tone?: string }): React.JSX.Element {
  return (
    <span className="chip memory-chip" data-tone={tone || "neutral"}>
      {label}
    </span>
  );
}

function SectionNote({
  children,
  testid,
}: {
  children: React.ReactNode;
  testid?: string;
}): React.JSX.Element {
  return (
    <div className="memory-note" data-testid={testid}>
      {children}
    </div>
  );
}

function RecordRow({
  record,
  utilityMode,
  onSelect,
}: {
  record: MemoryPanelRecord;
  utilityMode?: boolean;
  onSelect: () => void;
}): React.JSX.Element {
  const utility = utilityMode ? recordUtility(record) : null;
  return (
    <button
      type="button"
      className="memory-row"
      data-testid={`memory-record:${record.id}`}
      onClick={onSelect}
    >
      <span className="memory-row__title">{record.title || record.id}</span>
      <span className="memory-row__meta">
        <Chip label={record.kind} />
        <Chip label={trustLabel(record.trust)} tone={trustTone(record.trust)} />
        <Chip label={statusLabel(record.status)} tone={statusTone(record.status)} />
        {utility?.dead ? <Chip label="DEAD" tone="warning" /> : null}
        <span className="memory-row__age">{relativeAge(record.updated_at_ms)}</span>
      </span>
      {utility ? (
        <span className="memory-row__meta memory-row__utility">{utilityLine(record)}</span>
      ) : null}
    </button>
  );
}

interface EvidenceState {
  key: string;
  /// "not-found": the session did not appear in the recent timeline window
  /// (it may merely be older than the window — never claim it is gone).
  /// "empty-range": the session was found but the evidence range holds no
  /// renderable message entries.
  status: "loading" | "loaded" | "not-found" | "empty-range";
  lines: EvidenceExcerptLine[];
}

function evidenceKey(evidence: MemoryEvidenceRef, index: number): string {
  return `${index}:${evidence.session_id || ""}:${evidence.generation ?? ""}`;
}

/// The decision block on a quarantined record's Biography. The body and
/// provenance above it are what the reviewer decides on; the request binds
/// the record's content hash so a changed record is refused, not decided.
function QuarantineDecisionBlock({
  realm,
  record,
  onDecide,
}: {
  realm: string;
  record: MemoryFullRecord;
  onDecide: (verdict: MemoryQuarantineVerdict, rationale: string | undefined) => Promise<void>;
}): React.JSX.Element {
  const [rationale, setRationale] = React.useState("");
  const [pending, setPending] = React.useState<MemoryQuarantineVerdict | null>(null);
  const [failure, setFailure] = React.useState<string | null>(null);
  const rationaleProblem = quarantineRationaleProblem(rationale);
  const blocked = pending !== null || rationaleProblem !== null;
  const notice = rationaleProblem ?? failure;
  async function decide(verdict: MemoryQuarantineVerdict): Promise<void> {
    if (rationaleProblem !== null) return;
    setPending(verdict);
    setFailure(null);
    try {
      await onDecide(verdict, rationale.trim() || undefined);
    } catch (err) {
      setFailure(quarantineDecisionErrorText(err));
    } finally {
      setPending(null);
    }
  }
  return (
    <div className="memory-detail__section" data-testid="memory-quarantine-decide">
      <span className="memory-detail__label">Review</span>
      <div className="memory-detail__line">
        Release makes this an active agent_observed record of{" "}
        {record.scope.scope === "identity" ? record.scope.identity : "this scope"} (realm {realm});
        it stays capped below verified trust because it was quarantined. Tombstone discards it.
      </div>
      <input
        className="memory-filterbar__input"
        data-testid="memory-quarantine-rationale"
        placeholder="Rationale (optional, recorded in the audit)"
        value={rationale}
        disabled={pending !== null}
        onChange={(event) => setRationale(event.target.value)}
      />
      <div className="memory-detail__line">
        <button
          type="button"
          className="memory-back"
          data-testid="memory-quarantine-release"
          disabled={blocked}
          onClick={() => void decide("release")}
        >
          {pending === "release" ? "Releasing…" : "Release"}
        </button>{" "}
        <button
          type="button"
          className="memory-back"
          data-testid="memory-quarantine-tombstone"
          disabled={blocked}
          onClick={() => void decide("tombstone")}
        >
          {pending === "tombstone" ? "Tombstoning…" : "Tombstone"}
        </button>
      </div>
      {notice ? (
        <div className="memory-detail__line" data-testid="memory-quarantine-error">
          {notice}
        </div>
      ) : null}
    </div>
  );
}

function BiographyView({
  detail,
  dreams,
  onBack,
  onSelectRecord,
  onLoadEvidence,
  onDecideQuarantine,
  decisionNotice,
}: {
  detail: MemoryRecordDetail;
  dreams: MemoryDreamRun[];
  onBack: () => void;
  onSelectRecord: (realm: string | undefined, memoryId: string) => void;
  onLoadEvidence?: MemoryPanelProps["onLoadEvidence"];
  onDecideQuarantine?: (
    record: MemoryFullRecord,
    verdict: MemoryQuarantineVerdict,
    rationale: string | undefined,
  ) => Promise<void>;
  decisionNotice?: string | null;
}): React.JSX.Element {
  const { record, chain, injections } = detail;
  const provenance = record.provenance;
  const evidence = provenance?.evidence || [];
  const verification = provenance?.verification;
  const usage = record.usage;
  const lane = lineageLane(chain, record.id);
  const touchingRuns = dreamRunsTouching(dreams, record.id);
  const [evidenceState, setEvidenceState] = React.useState<EvidenceState | null>(null);
  // Monotonic click sequence: a slow earlier evidence fetch must never
  // overwrite the excerpt of a later click (same discipline as the records
  // pager). Lives in a ref because openEvidence is re-created per render.
  const evidenceSeqRef = React.useRef(0);
  const recordIdentity =
    record.scope.scope === "identity"
      ? record.scope.identity
      : provenance?.author?.author === "agent"
        ? provenance.author.identity
        : undefined;

  async function openEvidence(ref: MemoryEvidenceRef, index: number): Promise<void> {
    if (!onLoadEvidence) return;
    const key = evidenceKey(ref, index);
    const seq = ++evidenceSeqRef.current;
    setEvidenceState({ key, status: "loading", lines: [] });
    const entries = await onLoadEvidence(recordIdentity, ref);
    if (seq !== evidenceSeqRef.current) return;
    const lines = entries ? evidenceExcerptLines(entries, ref.range) : [];
    setEvidenceState({
      key,
      status:
        entries === null ? "not-found" : lines.length > 0 ? "loaded" : "empty-range",
      lines,
    });
  }

  return (
    <div className="memory-detail" data-testid="memory-detail">
      <div className="memory-detail__head">
        <button type="button" className="memory-back" onClick={onBack} data-testid="memory-detail-back">
          ← Back
        </button>
        <h3>{record.title || record.id}</h3>
        <span className="memory-detail__chips">
          <Chip label={record.kind} />
          <Chip label={trustLabel(record.trust)} tone={trustTone(record.trust)} />
          <Chip label={statusLabel(record.status)} tone={statusTone(record.status)} />
        </span>
        <CopyButton
          text={JSON.stringify(record, null, 2)}
          label="Copy record JSON"
          className="memory-copy-json"
        />
      </div>

      {record.description ? (
        <p className="memory-detail__description">{record.description}</p>
      ) : null}

      <pre className="memory-detail__body" data-testid="memory-detail-body">{record.body}</pre>

      {decisionNotice ? (
        <div className="memory-note" data-testid="memory-quarantine-outcome">
          {decisionNotice}
        </div>
      ) : null}

      {onDecideQuarantine && canDecideQuarantinedRecord(record) ? (
        <QuarantineDecisionBlock
          key={record.id}
          realm={detail.realm}
          record={record}
          onDecide={(verdict, rationale) => onDecideQuarantine(record, verdict, rationale)}
        />
      ) : null}

      {record.tags && record.tags.length > 0 ? (
        <div className="memory-detail__tags">
          {record.tags.map((tag) => (
            <Chip key={tag} label={tag} tone="muted" />
          ))}
        </div>
      ) : null}

      <div className="memory-detail__section" data-testid="memory-detail-born">
        <span className="memory-detail__label">Born</span>
        <div className="memory-detail__line">{authorLine(provenance?.author)}</div>
        {evidence.length > 0 ? (
          <div className="memory-evidence">
            {evidence.map((ref, index) => (
              <button
                type="button"
                className="memory-evidence__ref"
                key={`ev-${index}`}
                data-testid={`memory-evidence:${index}`}
                onClick={() => void openEvidence(ref, index)}
                disabled={!onLoadEvidence}
                title={onLoadEvidence ? "Open transcript window" : undefined}
              >
                {evidenceLabel(ref)}
              </button>
            ))}
          </div>
        ) : null}
        {evidenceState ? (
          evidenceState.status === "loading" ? (
            <div className="memory-detail__line">Loading transcript…</div>
          ) : evidenceState.status === "not-found" ? (
            <div className="memory-detail__line" data-testid="memory-evidence-degraded">
              Session not found in the recent timeline window — evidence reference retained
              as label only.
            </div>
          ) : evidenceState.status === "empty-range" ? (
            <div className="memory-detail__line" data-testid="memory-evidence-empty">
              Session found, but no message entries in the evidence range — the window is
              approximate against the console timeline.
            </div>
          ) : (
            <div className="memory-excerpt" data-testid="memory-evidence-excerpt">
              <div className="memory-detail__line memory-excerpt__note">
                Approximate window against the console timeline (evidence indexes a session
                generation).
              </div>
              {evidenceState.lines.map((line) => (
                <div className="memory-excerpt__line" key={line.id}>
                  <span className="memory-excerpt__speaker">{line.speaker}</span>
                  <span className="memory-excerpt__text">{line.text}</span>
                </div>
              ))}
            </div>
          )
        ) : null}
        {verification?.checked ? (
          <div className="memory-detail__line memory-detail__verification">
            verified: {verification.checked}
          </div>
        ) : null}
      </div>

      {lane.length > 0 ? (
        <div className="memory-detail__section" data-testid="memory-detail-lineage">
          <span className="memory-detail__label">Lineage</span>
          <div className="memory-chain">
            {lane.map(({ record: entry, current }) => (
              <button
                type="button"
                className="memory-chain__row"
                key={entry.id}
                data-current={current ? "true" : undefined}
                data-dimmed={current ? undefined : "true"}
                data-testid={`memory-chain:${entry.id}`}
                onClick={() => {
                  if (!current) onSelectRecord(realmOfRecord(entry), entry.id);
                }}
              >
                <span className="memory-chain__marker">{current ? "●" : "○"}</span>
                <span className="memory-chain__title">{entry.title || entry.id}</span>
                <Chip label={trustLabel(entry.trust)} tone={trustTone(entry.trust)} />
                <Chip label={statusLabel(entry.status)} tone={statusTone(entry.status)} />
              </button>
            ))}
          </div>
        </div>
      ) : null}

      <div className="memory-detail__section" data-testid="memory-detail-life">
        <span className="memory-detail__label">Life</span>
        {usage ? (
          <div className="memory-detail__line">
            injected {usage.injected_count ?? 0} · recalled {usage.explicit_recall_count ?? 0} ·
            judged useful {usage.judged_useful_count ?? 0}
            {usage.last_injected_at_ms
              ? ` · last injected ${relativeAge(usage.last_injected_at_ms)}`
              : ""}
          </div>
        ) : (
          <div className="memory-detail__line">no usage recorded</div>
        )}
        {injections.length > 0 ? (
          injections.map((injection, index) => (
            <div className="memory-detail__line" key={`inj-${index}`}>
              {injectionLine(injection)}
            </div>
          ))
        ) : (
          <div className="memory-detail__line">no injections recorded for this record</div>
        )}
      </div>

      <div className="memory-detail__section" data-testid="memory-detail-dreams">
        <span className="memory-detail__label">Dreams</span>
        {touchingRuns.length > 0 ? (
          touchingRuns.map((run) => (
            <div className="memory-detail__line" key={run.run_id}>
              {run.run_id} · {dreamTimeRange(run)}
              {run.quarantined_ops ? ` · ⚠ ${run.quarantined_ops} quarantined` : ""}
            </div>
          ))
        ) : (
          <div className="memory-detail__line">
            no sampled dream runs reference this record (sample is ≤12 ids per run — exact
            history needs the record history[] surface)
          </div>
        )}
      </div>
    </div>
  );
}

function VerdictStrip({
  tiles,
  onOpen,
  onOpenRecord,
}: {
  tiles: VerdictTile[];
  onOpen: (tile: VerdictTile) => void;
  onOpenRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  return (
    <div className="memory-tiles" data-testid="memory-verdict-strip">
      {/* Tiles are div-role-buttons (SignalsRail idiom) so violation-evidence
          buttons can nest inside: a red tile is a door to the exact rows that
          made it red, not just to a tab. */}
      {tiles.map((tile) => (
        <div
          className="memory-tile"
          key={tile.id}
          role="button"
          tabIndex={0}
          data-status={tile.status}
          data-testid={`memory-verdict:${tile.id}`}
          onClick={() => onOpen(tile)}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") onOpen(tile);
          }}
        >
          <span className="memory-tile__label">{tile.label}</span>
          {tile.rechecking ? (
            <span className="memory-tile__recheck" data-testid={`memory-verdict-recheck:${tile.id}`}>
              re-checking…
            </span>
          ) : null}
          <span className="memory-tile__status" data-status={tile.status}>
            {verdictStatusLabel(tile.status)}
          </span>
          {tile.lines.map((line, index) => (
            <span className="memory-tile__line" key={`l-${index}`}>
              {line}
            </span>
          ))}
          {(tile.evidence || []).map((violation) => (
            <button
              type="button"
              className="memory-tile__evidence"
              key={violation.id}
              data-testid={`memory-verdict-evidence:${tile.id}:${violation.id}`}
              onClick={(event) => {
                event.stopPropagation();
                onOpenRecord(violation.realm, violation.id);
              }}
            >
              {violation.id}
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}

/// Exported for the jsdom component-interaction lane, which pins the
/// pause-on-scroll / N-behind / auto-unfreeze discipline.
export function MemoryLiveStrip({
  frames,
  onPivot,
}: {
  frames: ConsoleFrame[];
  onPivot: (realm: string | undefined, recordId: string) => void;
}): React.JSX.Element {
  // Live-follow discipline: pausing on scroll freezes the visible list;
  // "N behind · jump to live" resumes. Frames dedupe by id so
  // snapshot_complete replays never double-render.
  const deduped = React.useMemo(() => dedupeFramesById(frames), [frames]);
  const [frozen, setFrozen] = React.useState<ConsoleFrame[] | null>(null);
  const listRef = React.useRef<HTMLDivElement | null>(null);
  const shown = frozen ?? deduped;
  const behind = frozen ? countFramesBehind(deduped, frozen) : 0;

  function handleScroll(): void {
    const el = listRef.current;
    if (!el) return;
    if (el.scrollTop > 4) {
      setFrozen((current) => current ?? deduped);
    } else if (frozen && behind === 0) {
      setFrozen(null);
    }
  }

  function jumpToLive(): void {
    setFrozen(null);
    // Optional call: jsdom (the component-interaction lane) has no scrolling.
    listRef.current?.scrollTo?.({ top: 0 });
  }

  return (
    <div className="memory-group memory-live" data-testid="memory-live-strip">
      <div className="memory-group__label">
        Live memory events (in-memory ring — lossy)
        {behind > 0 ? (
          <button
            type="button"
            className="memory-live__jump"
            data-testid="memory-live-jump"
            onClick={jumpToLive}
          >
            {behind} behind · jump to live
          </button>
        ) : null}
      </div>
      <div className="memory-live__list" ref={listRef} onScroll={handleScroll}>
        {shown.length === 0 ? (
          <div className="memory-detail__line">No memory events in the ring.</div>
        ) : (
          shown.map((frame) => {
            const data = frame.data && typeof frame.data === "object"
              ? (frame.data as Record<string, unknown>)
              : {};
            const pivot = memoryFramePivot(frame);
            return (
              <div className="memory-live__row" key={frame.id} data-testid={`memory-live-row:${frame.id}`}>
                <span className="memory-row__age">{relativeAge(frame.timestampMs)}</span>
                <span className="memory-live__text">
                  {describeMemoryTimelineEvent(frame.event, data)}
                </span>
                {pivot ? (
                  <button
                    type="button"
                    className="memory-live__pivot"
                    data-testid={`memory-live-pivot:${frame.id}`}
                    onClick={() => onPivot(pivot.realm, pivot.recordId)}
                  >
                    state here
                  </button>
                ) : null}
              </div>
            );
          })
        )}
        {/* The ring keeps 1024 frames/identity, 4096 total — everything older
            is unknowable live. The seam keeps "no events" from being mistaken
            for "nothing happened". */}
        <div className="memory-live__seam" data-testid="memory-live-seam">
          — ring history starts here —
        </div>
      </div>
    </div>
  );
}

// ── Main panel ────────────────────────────────────────────────────────────

export function MemoryPanel({
  records,
  realms,
  quarantineRecords,
  pendingPromotions,
  dreams,
  detail,
  detailLoading = false,
  canReviewQuarantine = false,
  unavailable = false,
  error,
  nextCursor = null,
  recordsDenied = false,
  dreamsDenied = false,
  operatorScopeDenied = false,
  mobScopeDenied = false,
  overview = null,
  overviewDenied = false,
  proposals = [],
  proposalsDenied = false,
  injections = [],
  injectionsDenied = false,
  harvests = [],
  harvestsDenied = false,
  dreamRuns = [],
  dreamRunsDenied = false,
  auditVerdicts = [],
  auditVerdictsDenied = false,
  liveFrames = [],
  onRefresh,
  onSelectRecord,
  onClearDetail,
  onQueryRecords,
  onLoadEvidence,
  onOpenGating,
  onDecideQuarantine,
}: MemoryPanelProps): React.JSX.Element {
  const [tab, setTab] = React.useState<MemoryTab>("holdings");
  // The outcome of the last quarantine decision, shown on the Biography it
  // navigated to (the successor after a release, the origin otherwise).
  const [decisionNotice, setDecisionNotice] = React.useState<string | null>(null);
  const decideQuarantine = onDecideQuarantine
    ? async (
        record: MemoryFullRecord,
        verdict: MemoryQuarantineVerdict,
        rationale: string | undefined,
      ): Promise<void> => {
        if (!detail) return;
        const realm = detail.realm;
        const result = await onDecideQuarantine(realm, record, verdict, rationale);
        setDecisionNotice(quarantineDecisionSummary(result));
        onSelectRecord(realm, result.successor?.memory_id ?? record.id);
      }
    : undefined;
  const [filter, setFilter] = React.useState<MemoryRecordsFilter>({});
  const [sortMode, setSortMode] = React.useState<"recency" | "utility">("recency");
  const [paged, setPaged] = React.useState<MemoryPagedState | null>(null);
  const [pageLoading, setPageLoading] = React.useState(false);
  const queryRecordsRef = React.useRef(onQueryRecords);
  queryRecordsRef.current = onQueryRecords;
  const pagerRef = React.useRef<ReturnType<typeof createMemoryRecordsPager> | null>(null);
  if (!pagerRef.current) {
    pagerRef.current = createMemoryRecordsPager({
      query: (params) =>
        queryRecordsRef.current ? queryRecordsRef.current(params) : Promise.resolve(null),
      setPaged,
      setLoading: setPageLoading,
    });
  }
  const pager = pagerRef.current;
  const [lattice, setLattice] = React.useState<LatticeWalkResult | null>(null);
  const [latticeRunning, setLatticeRunning] = React.useState(false);
  const [knowledgeIdentity, setKnowledgeIdentity] = React.useState<string>("");

  // A loaded Biography (e.g. via the SignalsRail "state here" pivot) lands
  // on the Records tab, where the detail pane lives. Keyed on the open
  // record, not the detail object: a Refresh of the same record writes a new
  // object and must not pull the operator back from another tab.
  const openRecordKey = detail ? `${detail.realm}\u0000${detail.record.id}` : null;
  React.useEffect(() => {
    if (openRecordKey) setTab("records");
  }, [openRecordKey]);

  // Lattice invariants (a)+(c) via the client page-walk. Deduped by a
  // CONTENT fingerprint (not array identity): content-identical debounced
  // live refreshes skip the walk entirely. While a re-check runs, the prior
  // result is retained so a settled tile never flickers to UNVERIFIABLE.
  const latticeRanForRef = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (tab !== "holdings" || !onQueryRecords || recordsDenied) return;
    const fingerprint = latticeFingerprint(records, realms, nextCursor);
    if (latticeRanForRef.current === fingerprint) return;
    latticeRanForRef.current = fingerprint;
    setLatticeRunning(true);
    let cancelled = false;
    let covered = false;
    void runLatticeWalk((params) => onQueryRecords(params), {
      realms,
      isCancelled: () => cancelled,
    })
      .then((result) => {
        // null means denied or cancelled; keep the prior verdict on cancel,
        // clear it on a real walk that came back denied.
        if (cancelled) return;
        // Only a settled verdict covers this content; a denied walk walks
        // again with the next refresh, as a failed one does.
        covered = result !== null;
        setLattice(result);
      })
      .catch(() => {
        if (!cancelled) setLattice(null);
      })
      .finally(() => {
        if (!cancelled) setLatticeRunning(false);
      });
    return () => {
      cancelled = true;
      // A walk that did not settle a verdict never covered this fingerprint,
      // so un-mark it and the dep change that cancelled us starts a fresh walk
      // instead of skipping. Without this, a refresh landing mid-walk
      // (re-dock, the 250ms memory.* debounce) leaves the tile on "page-walk
      // running…" forever: the fingerprint reads as already-checked while
      // lattice is still null and latticeRunning was never cleared (its
      // finally is cancellation-guarded). A covered fingerprint stays marked,
      // so a refresh that brings the same records does not walk them again.
      if (!covered && latticeRanForRef.current === fingerprint) {
        latticeRanForRef.current = null;
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab, records, recordsDenied, realms, nextCursor]);

  // Holdings stays anchored on the base page (its label says "over the N
  // loaded records"); the Records list renders from the accumulated view.
  const overviewRows = React.useMemo(() => scopeOverviewRows(records), [records]);
  // Overview scopes with denied-probe kinds dropped (their denied rows render
  // instead) — feeds both the Holdings table and the STORE FLOOR tile.
  const overviewScopes = React.useMemo(
    () =>
      overview
        ? sortOverviewScopes(
            visibleOverviewScopes(overview.scopes || [], {
              operatorScopeDenied,
              mobScopeDenied,
            }),
          )
        : null,
    [overview, operatorScopeDenied, mobScopeDenied],
  );
  const tiles = React.useMemo(
    () =>
      computeVerdictTiles({
        records,
        recordsDenied,
        dreams,
        dreamsDenied,
        lattice,
        latticeRunning,
        overview:
          overviewScopes && overview
            ? { scopes: overviewScopes, floors: overview.floors }
            : null,
        overviewDenied,
      }),
    [
      records,
      recordsDenied,
      dreams,
      dreamsDenied,
      lattice,
      latticeRunning,
      overview,
      overviewScopes,
      overviewDenied,
    ],
  );
  const annotatedInjections = React.useMemo(
    () => annotateInjectionDups(injections),
    [injections],
  );
  const dreamSheets = React.useMemo(() => dreamRunsNewestFirst(dreamRuns), [dreamRuns]);
  const [expandedRuns, setExpandedRuns] = React.useState<Record<string, boolean>>({});
  const identities = React.useMemo(() => identityOptions(records), [records]);
  const selectedIdentity = knowledgeIdentity || identities[0] || "";
  const memoryFrames = React.useMemo(
    () => liveFrames.filter((frame) => frame.event.startsWith("memory.")),
    [liveFrames],
  );

  function applyFilter(next: MemoryRecordsFilter): void {
    setFilter(next);
    if (!onQueryRecords) return;
    void pager.applyFilter(next);
  }

  function loadMore(): void {
    if (!onQueryRecords) return;
    void pager.loadMore({ filter, paged, baseRecords: records, baseCursor: nextCursor });
  }

  function openRecordsFiltered(next: MemoryRecordsFilter): void {
    setTab("records");
    onClearDetail();
    applyFilter(next);
  }

  function openTile(tile: VerdictTile): void {
    if (tile.id === "recall") setSortMode("utility");
    setTab(tile.targetTab);
  }

  if (unavailable) {
    return (
      <div className="gating memory-panel" data-testid="memory-panel">
        <div className="gating__head">
          <h2>Memory</h2>
        </div>
        <div className="gating__empty" data-testid="memory-unavailable">
          The memory panel is not configured on this runtime.
        </div>
      </div>
    );
  }

  const listView = buildRecordsListView({
    records,
    paged,
    baseCursor: nextCursor,
    filter,
    sortMode,
  });
  const quarantineCount = quarantineRecords.length + pendingPromotions.length;

  return (
    <div className="gating memory-panel" data-testid="memory-panel">
      <div className="gating__head">
        <h2>Memory</h2>
        <p>
          {records.length} records
          {realms.length > 1 ? ` · ${realms.length} realms` : ""}
        </p>
      </div>
      <div className="gating__tabs">
        {MEMORY_TABS.map((candidate) => (
          <button
            key={candidate}
            className={`gating__tab ${tab === candidate ? "is-active" : ""}`}
            onClick={() => setTab(candidate)}
            data-testid={`memory-tab:${candidate}`}
          >
            {memoryTabLabel(candidate)}
            {candidate === "pipeline" && quarantineCount > 0 ? (
              <span className="n">{quarantineCount}</span>
            ) : null}
            {candidate === "dreams" && dreams.length > 0 ? (
              <span className="n">{dreams.length}</span>
            ) : null}
          </button>
        ))}
        {/* One-release redirect alias (open Q6): the quarantine tab folded
            into Pipeline; automation addressing memory-tab:quarantine keeps
            working until the next release removes this. */}
        <button
          className="gating__tab memory-tab-alias"
          onClick={() => setTab(resolveMemoryTabAlias("quarantine") || "pipeline")}
          data-testid="memory-tab:quarantine"
          aria-hidden="true"
          tabIndex={-1}
        >
          Quarantine
        </button>
        <button
          className="gating__tab"
          onClick={() => onRefresh(tab === "records" && detail
            ? { realm: detail.realm, memoryId: detail.record.id }
            : undefined)}
          data-testid="memory-refresh"
        >
          Refresh
        </button>
      </div>
      {/* The refresh error sits in a reserved row between the tabs and the
          body, so it never moves either under a click in progress. */}
      <PanelStatusRow error={error} testId="memory-error" />

      <div className="gating__list memory-panel__body">
        {tab === "holdings" ? (
          <div className="memory-groups" data-testid="memory-holdings">
            <VerdictStrip
              tiles={tiles}
              onOpen={openTile}
              onOpenRecord={(realm, memoryId) => onSelectRecord(realm, memoryId)}
            />
            {recordsDenied ? (
              <SectionNote testid="memory-holdings-denied">
                Records are not readable by this principal (access denied).
              </SectionNote>
            ) : (
              <div className="memory-group">
                <div className="memory-group__label">
                  {overviewScopes
                    ? `Scopes — store totals (panel/overview)${
                        overview?.floors
                          ? ` · floors ${overview.floors.records ?? "?"} records / ${
                              typeof overview.floors.bytes === "number"
                                ? formatBytes(overview.floors.bytes)
                                : "?"
                            } per scope`
                          : ""
                      }`
                    : `Scopes — counts over the ${records.length} loaded records (full totals need panel/overview)`}
                </div>
                {overviewScopes ? (
                  overviewScopes.length === 0 && !operatorScopeDenied && !mobScopeDenied ? (
                    <div className="gating__empty">No memory records yet.</div>
                  ) : (
                    overviewScopes.map((scope) => {
                      const key = overviewScopeKey(scope);
                      return (
                        <button
                          type="button"
                          className="memory-row memory-scope-row"
                          key={key}
                          data-testid={`memory-holdings-scope:${key}`}
                          onClick={() => openRecordsFiltered(filterForOverviewScope(scope))}
                        >
                          <span className="memory-row__title">{overviewScopeLabel(scope)}</span>
                          <span className="memory-row__meta">
                            <Chip label={`${scope.active ?? 0} active`} tone="positive" />
                            {(scope.quarantined ?? 0) > 0 ? (
                              <Chip label={`${scope.quarantined} quarantined`} tone="warning" />
                            ) : null}
                            {(scope.superseded ?? 0) > 0 ? (
                              <Chip label={`${scope.superseded} superseded`} tone="muted" />
                            ) : null}
                            {(scope.tombstoned ?? 0) > 0 ? (
                              <Chip label={`${scope.tombstoned} tombstoned`} tone="muted" />
                            ) : null}
                            {scope.floor_pressure ? (
                              <span data-testid={`memory-holdings-floor:${key}`}>
                                <Chip label="FLOOR PRESSURE" tone="warning" />
                              </span>
                            ) : null}
                            <span className="memory-row__age">
                              {formatBytes(scope.body_bytes ?? 0)}
                            </span>
                          </span>
                        </button>
                      );
                    })
                  )
                ) : overviewRows.length === 0 && !operatorScopeDenied && !mobScopeDenied ? (
                  <div className="gating__empty">No memory records yet.</div>
                ) : (
                  overviewRows.map((row) => (
                    <button
                      type="button"
                      className="memory-row memory-scope-row"
                      key={row.key}
                      data-testid={`memory-holdings-scope:${row.key}`}
                      onClick={() => openRecordsFiltered(filterForScope(row.scope))}
                    >
                      <span className="memory-row__title">{row.label}</span>
                      <span className="memory-row__meta">
                        <Chip label={`${row.active} active`} tone="positive" />
                        {row.quarantined > 0 ? (
                          <Chip label={`${row.quarantined} quarantined`} tone="warning" />
                        ) : null}
                        {row.superseded > 0 ? (
                          <Chip label={`${row.superseded} superseded`} tone="muted" />
                        ) : null}
                        {row.tombstoned > 0 ? (
                          <Chip label={`${row.tombstoned} tombstoned`} tone="muted" />
                        ) : null}
                        <span className="memory-row__age">{formatBytes(row.bytes)}</span>
                      </span>
                      {row.trustMix ? (
                        <span className="memory-row__meta memory-row__reason">{row.trustMix}</span>
                      ) : null}
                    </button>
                  ))
                )}
                {/* Denied scopes vanish from the row-filtered listing; the
                    one-row probes make them render as access-denied rows
                    instead of silently not existing (§3.1 / §5 ABAC). */}
                {mobScopeDenied ? (
                  <div
                    className="memory-row memory-row--static memory-scope-row"
                    data-testid="memory-holdings-scope-denied:mob"
                  >
                    <span className="memory-row__title">Mob scopes</span>
                    <span className="memory-row__meta">
                      <Chip label="no grant" tone="warning" />
                      <span className="memory-row__reason">requires mob.memory.read</span>
                    </span>
                  </div>
                ) : null}
                {operatorScopeDenied ? (
                  <div
                    className="memory-row memory-row--static memory-scope-row"
                    data-testid="memory-holdings-scope-denied:operator"
                  >
                    <span className="memory-row__title">Operator scope</span>
                    <span className="memory-row__meta">
                      <Chip label="no grant" tone="warning" />
                      <span className="memory-row__reason">requires operator.memory.read</span>
                    </span>
                  </div>
                ) : null}
              </div>
            )}
            <div className="memory-group">
              <div className="memory-group__label">In transit</div>
              <div className="memory-detail__line">
                {dreams.length > 0
                  ? `Last dream ${dreamTimeRange(dreams[0])} · ${dreams[0].ops ?? "—"} ops`
                  : dreamsDenied
                    ? "Dream audit: no grant"
                    : "No dream runs recorded yet"}
              </div>
              <div className="memory-detail__line">
                {canReviewQuarantine
                  ? `Quarantine queue ${quarantineRecords.length} · pending gate ${pendingPromotions.length}`
                  : "Quarantine queue: requires memory.quarantine.review"}
              </div>
              <div className="memory-detail__line">
                {proposalsDenied
                  ? "Proposals: no grant"
                  : `Proposals: ${proposals.length} pending${
                      proposals.filter((proposal) => proposal.tainted).length > 0
                        ? ` · ${proposals.filter((proposal) => proposal.tainted).length} held (taint)`
                        : ""
                    }`}
              </div>
              <div className="memory-detail__line">
                Detailed memory health is not available here.
              </div>
            </div>
            <div className="memory-group" data-testid="memory-harvests">
              <div className="memory-group__label">
                Harvest queue — retired identities awaiting the exit-interview dream
              </div>
              {harvestsDenied ? (
                <div className="memory-detail__line">Harvest queue: no grant.</div>
              ) : harvests.length === 0 ? (
                <div className="memory-detail__line">No pending harvests.</div>
              ) : (
                harvests.map((harvest, index) => (
                  <div
                    className="memory-row memory-row--static"
                    key={`${harvest.realm}:${harvest.identity}:${index}`}
                    data-testid={`memory-harvest:${harvest.identity}`}
                  >
                    <span className="memory-row__title">{harvest.identity}</span>
                    <span className="memory-row__meta">
                      {harvest.cause ? <Chip label={harvest.cause} tone="muted" /> : null}
                      {harvest.session_key ? (
                        <span className="memory-row__reason">session {harvest.session_key}</span>
                      ) : null}
                      <span className="memory-row__age">
                        retired {relativeAge(harvest.retired_at_ms)}
                      </span>
                    </span>
                  </div>
                ))
              )}
            </div>
          </div>
        ) : null}

        {tab === "records" ? (
          detail ? (
            <BiographyView
              detail={detail}
              dreams={dreams}
              onBack={() => {
                setDecisionNotice(null);
                onClearDetail();
              }}
              onSelectRecord={(realm, memoryId) => {
                setDecisionNotice(null);
                onSelectRecord(realm, memoryId);
              }}
              onLoadEvidence={onLoadEvidence}
              onDecideQuarantine={decideQuarantine}
              decisionNotice={decisionNotice}
            />
          ) : detailLoading ? (
            <div className="gating__empty">Loading record…</div>
          ) : (
            <div className="memory-groups">
              {onQueryRecords ? (
                <div className="memory-filterbar" data-testid="memory-filter">
                  <label>
                    scope
                    <select
                      value={filter.scope || ""}
                      data-testid="memory-filter:scope"
                      onChange={(event) =>
                        applyFilter({
                          ...filter,
                          scope: (event.target.value || undefined) as MemoryRecordsFilter["scope"],
                        })
                      }
                    >
                      <option value="">all</option>
                      <option value="identity">identity</option>
                      <option value="mob">mob</option>
                      <option value="operator">operator</option>
                      <option value="realm">realm</option>
                    </select>
                  </label>
                  <label>
                    identity / key
                    <input
                      value={filter.key || ""}
                      data-testid="memory-filter-input"
                      placeholder="identity or scope key"
                      onChange={(event) => setFilter({ ...filter, key: event.target.value })}
                      onKeyDown={(event) => {
                        if (event.key === "Enter") applyFilter(filter);
                      }}
                      onBlur={() => {
                        // Re-query only on real change: a blur caused by
                        // clicking load-more must not race the append.
                        void pager.applyFilterIfChanged(filter);
                      }}
                    />
                  </label>
                  <label>
                    status
                    <select
                      value={filter.status || ""}
                      data-testid="memory-filter:status"
                      onChange={(event) =>
                        applyFilter({
                          ...filter,
                          status: (event.target.value || undefined) as MemoryRecordsFilter["status"],
                        })
                      }
                    >
                      <option value="">all</option>
                      <option value="active">active</option>
                      <option value="quarantined">quarantined</option>
                      <option value="superseded">superseded</option>
                      <option value="tombstoned">tombstoned</option>
                    </select>
                  </label>
                  {realms.length > 1 ? (
                    <label>
                      realm
                      <select
                        value={filter.realm || ""}
                        data-testid="memory-filter:realm"
                        onChange={(event) =>
                          applyFilter({
                            ...filter,
                            realm: event.target.value || undefined,
                          })
                        }
                      >
                        <option value="">all (merged page)</option>
                        {realms.map((realm) => (
                          <option key={realm} value={realm}>
                            {realm}
                          </option>
                        ))}
                      </select>
                    </label>
                  ) : null}
                  <label>
                    sort
                    <select
                      value={sortMode}
                      data-testid="memory-sort"
                      onChange={(event) =>
                        setSortMode(event.target.value === "utility" ? "utility" : "recency")
                      }
                    >
                      <option value="recency">recency</option>
                      <option value="utility">utility</option>
                    </select>
                  </label>
                  {hasActiveFilter(filter) ? (
                    <button
                      type="button"
                      className="memory-back"
                      data-testid="memory-filter-clear"
                      onClick={() => applyFilter({})}
                    >
                      clear
                    </button>
                  ) : null}
                </div>
              ) : null}
              {sortMode === "utility" ? (
                <SectionNote testid="memory-utility-note">
                  Utility mode estimates total bytes from injection count and record size.
                  DEAD means injected at least {DEAD_INJECTION_THRESHOLD} times
                  and never judged useful.
                </SectionNote>
              ) : null}
              {realms.length > 1 && !filter.realm?.trim() ? (
                <SectionNote testid="memory-multi-realm-note">
                  Multi-realm view is a single merged page (keyset paging is
                  per-realm) — pick a realm above to page through its records.
                </SectionNote>
              ) : null}
              {pageLoading ? <div className="gating__empty">Loading records…</div> : null}
              {!pageLoading && listView.records.length === 0 ? (
                <div className="gating__empty" data-testid="memory-records-empty">
                  {recordsDenied || listView.denied
                    ? "Records: no grant."
                    : "No memory records yet."}
                </div>
              ) : null}
              {!pageLoading && listView.denied && listView.records.length > 0 ? (
                <SectionNote testid="memory-records-denied-note">
                  Further pages: no grant — the continuation of this query was
                  denied for this principal.
                </SectionNote>
              ) : null}
              {!pageLoading && listView.records.length > 0 ? (
                listView.mode === "flat" ? (
                  <div className="memory-group">
                    {listView.records.map((record) => (
                      <RecordRow
                        key={record.id}
                        record={record}
                        utilityMode={sortMode === "utility"}
                        onSelect={() => onSelectRecord(realmOfRecord(record), record.id)}
                      />
                    ))}
                  </div>
                ) : (
                  listView.groups.map((group) => (
                    <div className="memory-group" key={group.key} data-testid={`memory-group:${group.key}`}>
                      <div className="memory-group__label">{group.label}</div>
                      {group.records.map((record) => (
                        <RecordRow
                          key={record.id}
                          record={record}
                          onSelect={() => onSelectRecord(realmOfRecord(record), record.id)}
                        />
                      ))}
                    </div>
                  ))
                )
              ) : null}
              {listView.cursor && onQueryRecords ? (
                <button
                  type="button"
                  className="memory-back memory-load-more"
                  data-testid="memory-load-more"
                  disabled={pageLoading}
                  onClick={loadMore}
                >
                  load more
                </button>
              ) : null}
            </div>
          )
        ) : null}

        {tab === "knowledge" ? (
          <div className="memory-groups" data-testid="memory-knowledge">
            {identities.length === 0 ? (
              <div className="gating__empty">
                {recordsDenied
                  ? "Records: no grant."
                  : "No identity-scoped records loaded yet."}
              </div>
            ) : (
              <>
                <div className="memory-filterbar">
                  <label>
                    identity
                    <select
                      value={selectedIdentity}
                      data-testid="memory-knowledge-identity"
                      onChange={(event) => setKnowledgeIdentity(event.target.value)}
                    >
                      {identities.map((identity) => (
                        <option key={identity} value={identity}>
                          {identity}
                        </option>
                      ))}
                    </select>
                  </label>
                </div>
                <div className="memory-group">
                  <div className="memory-group__label">
                    Composition (scope union over loaded records)
                  </div>
                  {knowledgeComposition(records, selectedIdentity).map((segment) => (
                    <button
                      type="button"
                      className="memory-row"
                      key={segment.label}
                      data-testid={`memory-knowledge-segment:${segment.label}`}
                      onClick={() => openRecordsFiltered(segment.filter)}
                    >
                      <span className="memory-row__title">{segment.label}</span>
                      <span className="memory-row__meta">
                        <Chip label={`${segment.count} records`} />
                        {segment.approximate ? (
                          <span className="memory-row__reason">
                            all {segment.label.split(" ")[0]}-scope rows — membership resolution
                            needs panel/context (surface 10)
                          </span>
                        ) : null}
                      </span>
                    </button>
                  ))}
                </div>
              </>
            )}
            <SectionNote testid="memory-knowledge-as-injected">
              AS-INJECTED is unverifiable in phase 1 — the composed injection block requires
              meta/memory/panel/context (surface 10).
            </SectionNote>
            <div className="memory-group" data-testid="memory-knowledge-history">
              <div className="memory-group__label">
                Injection history (durable ledger, newest first)
              </div>
              {injectionsDenied ? (
                <div className="memory-detail__line">Injection history: no grant.</div>
              ) : annotatedInjections.length === 0 ? (
                <div className="memory-detail__line">No injection-ledger rows yet.</div>
              ) : (
                annotatedInjections.map(({ entry, dup }, index) => (
                  <div
                    className="memory-row memory-row--static"
                    key={`inj-${index}`}
                    data-testid={`memory-injection:${index}`}
                  >
                    <button
                      type="button"
                      className="memory-dream__record"
                      data-testid={`memory-injection-record:${index}`}
                      onClick={() => onSelectRecord(entry.realm, entry.record_id)}
                    >
                      {entry.record_id}
                    </button>
                    <span className="memory-row__meta">
                      <Chip label={entry.surface} tone="muted" />
                      <span className="memory-row__reason">
                        {entry.identity}
                        {entry.session_key ? ` · session ${entry.session_key}` : ""}
                      </span>
                      {dup ? (
                        <span data-testid={`memory-injection-dup:${index}`}>
                          <Chip label="DUP" tone="warning" />
                        </span>
                      ) : null}
                      <span className="memory-row__age">{relativeAge(entry.at_ms)}</span>
                    </span>
                  </div>
                ))
              )}
              <SectionNote testid="memory-knowledge-budget">
                Session memory budget data is not available here.
              </SectionNote>
            </div>
          </div>
        ) : null}

        {tab === "pipeline" ? (
          <div className="memory-quarantine" data-testid="memory-pipeline">
            <div className="memory-detail__line memory-pipeline__stages" data-testid="memory-pipeline-stages">
              PROPOSED ({proposalsDenied ? "no grant" : proposals.length}) ─▶ PENDING GATE (
              {pendingPromotions.length}) ─▶ COMMITTED · QUAR (
              {canReviewQuarantine ? quarantineRecords.length : "no grant"})
            </div>
            <div className="memory-note" data-testid="memory-quarantine-note">
              {onDecideQuarantine
                ? "Open a quarantined record to release or tombstone it. Gated promotions are decided in the Gating inbox; the memory steward's dream decides the rest."
                : "Read-only here. Verdicts come from the memory steward's dream, the gating flow, and reviewers who hold the quarantine review and memory write grants."}
            </div>
            <div className="memory-group" data-testid="memory-pipeline-proposals">
              <div className="memory-group__label">
                Proposed — awaiting a dream verdict (taint captured at propose time)
              </div>
              {proposalsDenied ? (
                <div className="memory-detail__line">Proposals: no grant.</div>
              ) : proposals.length === 0 ? (
                <div className="memory-detail__line">No pending proposals.</div>
              ) : (
                proposals.map((proposal) => (
                  <div
                    className="memory-row memory-row--static"
                    key={`${proposal.realm}:${proposal.proposal_id}`}
                    data-testid={`memory-proposal:${proposal.proposal_id}`}
                  >
                    <span className="memory-row__title">
                      {proposal.title || proposal.proposal_id}
                    </span>
                    <span className="memory-row__meta">
                      {proposal.kind ? <Chip label={proposal.kind} /> : null}
                      <Chip
                        label={`→ ${proposal.scope_kind}${
                          proposal.scope_key ? `:${proposal.scope_key}` : ""
                        }`}
                        tone="muted"
                      />
                      {proposal.tainted ? (
                        <span data-testid={`memory-proposal-taint:${proposal.proposal_id}`}>
                          <Chip label="tainted" tone="warning" />
                        </span>
                      ) : null}
                      {proposal.status ? (
                        <Chip
                          label={proposal.status}
                          tone={proposal.status === "held" ? "warning" : "muted"}
                        />
                      ) : null}
                      {proposal.author ? (
                        <span className="memory-row__reason">{proposal.author}</span>
                      ) : null}
                      <span className="memory-row__age">
                        {relativeAge(proposal.created_at_ms)}
                      </span>
                    </span>
                  </div>
                ))
              )}
            </div>
            {canReviewQuarantine ? (
              <>
                {quarantineRecords.length === 0 && pendingPromotions.length === 0 ? (
                  <div className="gating__empty">Quarantine queue is empty.</div>
                ) : null}
                {pendingPromotions.length > 0 ? (
                  <div className="memory-group">
                    <div className="memory-group__label">Pending gated promotions</div>
                    {pendingPromotions.map((pending) => (
                      <div
                        className="memory-row memory-row--static"
                        key={pending.pending_id}
                        data-testid={`memory-pending:${pending.pending_id}`}
                      >
                        <span className="memory-row__title">
                          {pending.record_id} → {pending.scope_kind}:{pending.scope_key}
                        </span>
                        <span className="memory-row__meta">
                          {pending.rationale ? (
                            <span className="memory-row__reason">{pending.rationale}</span>
                          ) : null}
                          {pending.status ? <Chip label={pending.status} tone="muted" /> : null}
                          <span className="memory-row__age">{relativeAge(pending.created_at_ms)}</span>
                          {onOpenGating ? (
                            <button
                              type="button"
                              className="memory-back"
                              data-testid={`memory-pipeline-decide:${pending.pending_id}`}
                              onClick={onOpenGating}
                            >
                              → decide in Gating inbox
                            </button>
                          ) : null}
                        </span>
                      </div>
                    ))}
                  </div>
                ) : null}
                {quarantineRecords.length > 0 ? (
                  <div className="memory-group">
                    <div className="memory-group__label">Quarantined records</div>
                    {quarantineRecords.map((record) => {
                      const reason =
                        record.status.status === "quarantined" ? record.status.reason : undefined;
                      return (
                        <button
                          type="button"
                          className="memory-row"
                          key={record.id}
                          data-testid={`memory-quarantine-record:${record.id}`}
                          onClick={() => onSelectRecord(realmOfRecord(record), record.id)}
                        >
                          <span className="memory-row__title">{record.title || record.id}</span>
                          <span className="memory-row__meta">
                            {reason ? <span className="memory-row__reason">{reason}</span> : null}
                            <Chip label={trustLabel(record.trust)} tone={trustTone(record.trust)} />
                            <span className="memory-row__age">{relativeAge(record.created_at_ms)}</span>
                          </span>
                        </button>
                      );
                    })}
                  </div>
                ) : null}
              </>
            ) : (
              <SectionNote testid="memory-pipeline-no-grant">
                Quarantine queue: no grant — rows require memory.quarantine.review.
              </SectionNote>
            )}
            <div className="memory-group" data-testid="memory-review-queue">
              <div className="memory-group__label">
                Review queue — memories you might want to correct
              </div>
              {auditVerdictsDenied ? (
                <div className="memory-detail__line">Review queue: no grant.</div>
              ) : auditVerdicts.length === 0 ? (
                <div className="memory-detail__line">Review queue is empty.</div>
              ) : (
                auditVerdicts.map((verdict) => (
                  <div
                    className="memory-row memory-row--static"
                    key={`${verdict.realm}:${verdict.run_id}:${verdict.record_id}`}
                    data-testid={`memory-review:${verdict.run_id}:${verdict.record_id}`}
                  >
                    <button
                      type="button"
                      className="memory-dream__record"
                      data-testid={`memory-review-record:${verdict.run_id}:${verdict.record_id}`}
                      onClick={() => onSelectRecord(verdict.realm, verdict.record_id)}
                    >
                      {verdict.record_id}
                    </button>
                    <span className="memory-row__meta">
                      {verdict.verdict ? <Chip label={verdict.verdict} tone="warning" /> : null}
                      {verdict.rationale ? (
                        <span className="memory-row__reason">{verdict.rationale}</span>
                      ) : null}
                      <span className="memory-row__reason">{verdict.run_id}</span>
                      <span className="memory-row__age">{relativeAge(verdict.created_at_ms)}</span>
                    </span>
                  </div>
                ))
              )}
              <div className="memory-note">
                Read-only — the correction affordance ships with the write-path design.
              </div>
            </div>
            <MemoryLiveStrip
              frames={memoryFrames}
              onPivot={(realm, recordId) => void onSelectRecord(realm, recordId)}
            />
          </div>
        ) : null}

        {tab === "dreams" ? (
          <div className="memory-dreams">
            <div className="memory-group" data-testid="memory-dream-runs">
              <div className="memory-group__label">
                Durable verdict sheets (dream_runs — survive restarts)
              </div>
              {dreamRunsDenied ? (
                <div className="memory-detail__line">Verdict sheets: no grant.</div>
              ) : dreamSheets.length === 0 ? (
                <div className="memory-detail__line">
                  No persisted dream runs yet — runs before the dream_runs table land only in
                  the audit reconstruction below.
                </div>
              ) : (
                dreamSheets.map((run) => {
                  const expanded = expandedRuns[run.run_id] === true;
                  const detail = normalizeDreamRunDetail(run.detail);
                  return (
                    <div
                      className="gpolicy memory-dream-run"
                      key={run.run_id}
                      data-testid={`memory-dream-run:${run.run_id}`}
                    >
                      <button
                        type="button"
                        className="memory-row memory-dream-run__head"
                        data-testid={`memory-dream-run-toggle:${run.run_id}`}
                        onClick={() =>
                          setExpandedRuns((current) => ({
                            ...current,
                            [run.run_id]: !expanded,
                          }))
                        }
                      >
                        <span className="memory-row__title">
                          {expanded ? "▾" : "▸"} {run.run_id}
                        </span>
                        <span className="memory-row__meta">
                          {run.partition ? <Chip label={run.partition} tone="muted" /> : null}
                          <span className="memory-row__reason">
                            {dreamRunDuration(run)} ·{" "}
                            {typeof run.ops_committed === "number"
                              ? `${run.ops_committed} ops`
                              : "— ops"}
                          </span>
                          <span className="memory-row__age">
                            {relativeAge(run.completed_at_ms || run.started_at_ms)}
                          </span>
                        </span>
                      </button>
                      {expanded ? (
                        <div
                          className="memory-dream-run__detail"
                          data-testid={`memory-dream-run-detail:${run.run_id}`}
                        >
                          {detail.raw !== null ? (
                            <div className="memory-detail__line">
                              unparsed detail: {detail.raw}
                            </div>
                          ) : (
                            <>
                              {detail.phases.length > 0 ? (
                                detail.phases.map(([name, note], index) => (
                                  <div className="memory-detail__line" key={`ph-${index}`}>
                                    {name}
                                    {note ? ` — ${note}` : ""}
                                  </div>
                                ))
                              ) : (
                                <div className="memory-detail__line">no phases recorded</div>
                              )}
                              <div className="memory-detail__line">
                                verdicts:{" "}
                                {detail.verdicts.length > 0
                                  ? detail.verdicts
                                      .map(([name, count]) => `${count} ${name}`)
                                      .join(" · ")
                                  : "all counters zero"}
                              </div>
                              {detail.skips.map((skip, index) => (
                                <div
                                  className="memory-detail__line memory-dream__rationale"
                                  key={`sk-${index}`}
                                >
                                  skip: {skip}
                                </div>
                              ))}
                            </>
                          )}
                        </div>
                      ) : null}
                    </div>
                  );
                })
              )}
            </div>
            <div className="memory-group__label">Reconstructed from audit rows</div>
            {dreams.length === 0 ? (
              <div className="gating__empty">
                {dreamsDenied ? "Dream audit: no grant." : "No dream runs recorded yet."}
              </div>
            ) : (
              dreams.map((run) => {
                const summary = dreamOpKindsSummary(run.op_kinds);
                return (
                  <div className="gpolicy memory-dream" key={run.run_id} data-testid={`memory-dream:${run.run_id}`}>
                    <div className="gpolicy__head">
                      <span className="gpolicy__action">{run.run_id}</span>
                      {run.quarantined_ops && run.quarantined_ops > 0 ? (
                        <Chip label={`${run.quarantined_ops} quarantined`} tone="warning" />
                      ) : null}
                    </div>
                    <div className="gpolicy__meta">{dreamTimeRange(run)}</div>
                    <div className="gpolicy__meta">
                      {typeof run.ops === "number" ? `${run.ops} ops` : "—"}
                      {summary ? ` · ${summary}` : ""}
                    </div>
                    {(run.memory_ids || []).length > 0 ? (
                      <div className="memory-dream__touched">
                        touched:
                        {(run.memory_ids || []).map((memoryId) => (
                          <button
                            type="button"
                            className="memory-dream__record"
                            key={memoryId}
                            data-testid={`memory-dream-record:${run.run_id}:${memoryId}`}
                            onClick={() => onSelectRecord(run.realm, memoryId)}
                          >
                            {memoryId}
                          </button>
                        ))}
                      </div>
                    ) : null}
                    {(run.rationales || []).map((rationale, index) => (
                      <div className="memory-dream__rationale" key={`r-${index}`}>
                        {rationale}
                      </div>
                    ))}
                  </div>
                );
              })
            )}
          </div>
        ) : null}
      </div>
    </div>
  );
}
