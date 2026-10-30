import React from "react";
import type { ConversationTimelineEntry } from "@console-core";
import { CopyButton } from "../../../packages/console-components/src/copy-button";
import {
  AgentAvatar,
  AgentName,
  Button,
  Chip,
  EmptyState,
  KeyValueList,
  KitGlyph,
  ListRow,
  LoadingState,
  Meter,
  Notice,
  PanelBody,
  PanelHeader,
  PanelSection,
  PanelShell,
  PanelTabs,
  RelativeTime,
  RowList,
  SearchField,
  SectionHeader,
  SelectField,
  StatTile,
  StatusDot,
  StatusRow,
  TechnicalDetails,
  TechnicalId,
  TileGrid,
  Toolbar,
  agentDisplayName,
  formatAbsoluteTime,
  formatDuration,
  formatRelativeTime,
  humanizeEnum,
  joinMeta,
  plural,
  stripAgentKeyPrefix,
  useAgentDirectory,
  type PanelTab,
  type SelectOption,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";
import { accessActionLabel, isKnownAccessAction } from "../lib/access-actions";
import { describeMemoryTimelineEvent } from "../lib/adapters";
import { describeMemoryEventForPeople } from "../lib/memory-event-words";
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
  MemoryScopeOverview,
  MemoryTrust,
} from "../types";

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
  /** The team picker, when the console serves more than one team; shown
   * among the header actions. */
  runtimeControl?: React.ReactNode;
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
  /// False while the panel renders before its first records page has
  /// landed, when `records` and `realms` are an empty placeholder rather than
  /// the store's listing. The lattice page-walk waits for the listing: a walk
  /// over the placeholder would settle a verdict before the records arrive,
  /// and their arrival would then walk the same store again.
  loaded?: boolean;
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
  /// Deep-link into the gating inbox: memory promotion verdicts ride the
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

/// The scope key a records search means: a known key typed as is, else the
/// one key whose display name equals the text (case-insensitive). Anything
/// else, a partial or shared name included, is sent as typed, so a raw key
/// is never swapped for a guess.
export function resolveRecordsSearchKey(
  text: string,
  candidates: ReadonlyArray<{ key: string; name: string }>,
): string {
  const typed = text.trim();
  if (!typed) return typed;
  if (candidates.some((candidate) => candidate.key === typed)) return typed;
  const needle = typed.toLowerCase();
  const matches = Array.from(new Set(
    candidates.filter((candidate) => candidate.name.trim().toLowerCase() === needle).map((candidate) => candidate.key),
  ));
  return matches.length === 1 ? matches[0] : typed;
}

/// Build panel/records params from the filter bar. Identity scope rides the
/// `identity` param; mob/operator ride `scope` + `scope_key` (the shapes the
/// server already accepts, `http_console.rs` handle_memory_panel_records).
export function buildRecordsQueryParams(
  filter: MemoryRecordsFilter,
  options: { cursor?: string; limit?: number; resolveKey?: (text: string) => string } = {},
): Record<string, unknown> {
  const params: Record<string, unknown> = {};
  // The server matches scope keys exactly; a typed display name resolves to
  // its key first (the page shows names, the keys sit in tooltips).
  const typed = filter.key?.trim();
  const key = typed && options.resolveKey ? options.resolveKey(typed).trim() : typed;
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
/// meta/capabilities omits a method (`requireCapability`, headless.ts),
/// raised client-side before any RPC is issued, so no rpcError code exists.
const CAPABILITY_MISS_PREFIX = "Meta capability missing for ";

/// Classify a memory-section RPC failure: -32030 is an ABAC denial (render
/// "no grant", never an empty store), -32601 means the panel is not wired on
/// this runtime, anything else is a real error. A per-method capability miss
/// also classifies as DENIED: under an enforced view the server intersects
/// meta/capabilities per principal (a scoped read grant drops panel/dreams
/// and panel/quarantine from the method list), so the miss IS denial by
/// intersection: it must degrade that section, never abort the whole panel.
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
  /// (-32030). Renders "no grant": a denied scope must never be
  /// indistinguishable from an empty store.
  denied?: boolean;
}

export interface RecordsListView {
  mode: "grouped" | "flat";
  /// Flat display order (mode "flat"): the accumulated page, utility-sorted
  /// when the sort mode asks for it.
  records: MemoryPanelRecord[];
  /// Grouped display (mode "grouped"), built from the SAME accumulated
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
  /// failures must throw: the pager keeps the prior page for those (the
  /// caller surfaces them via its own error banner).
  query: (params: Record<string, unknown>) => Promise<MemoryPanelRecordsResult | null>;
  setPaged: (paged: MemoryPagedState | null) => void;
  setLoading: (loading: boolean) => void;
  /// Resolves a typed display name to the scope key the server matches.
  resolveKey?: (text: string) => string;
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
        const result = await deps.query(buildRecordsQueryParams(next, { resolveKey: deps.resolveKey }));
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
          buildRecordsQueryParams(current.filter, { cursor, resolveKey: deps.resolveKey }),
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
/// ascending usefulness ratio: the rows most likely to need demotion lead.
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

// ── Verdict tiles (the Holdings checks) ───────────────────────────────────

export type VerdictStatus = "holding" | "degraded" | "violated" | "unverifiable" | "no-grant";

export interface VerdictTile {
  id: "echo-safety" | "taint-wall" | "lattice" | "recall" | "dreams" | "store-floor";
  label: string;
  status: VerdictStatus;
  /// What the check found, in one plain sentence: the tile's visible line.
  summary: string;
  /// What the check covers and how far it got: the tile's help tooltip.
  lines: string[];
  /// A settled verdict whose page-walk is running again. It renders over the
  /// tile's corner, outside the text flow.
  rechecking?: boolean;
  /// Tiles are doors: clicking opens the tab holding the evidence.
  targetTab: MemoryTab;
  /// The rows that made the verdict red; each opens its Biography directly.
  /// Capped at VERDICT_EVIDENCE_MAX; `evidenceMore` counts the rest.
  evidence?: LatticeViolationRef[];
  evidenceMore?: number;
}

/// How many violating records render on a VIOLATED tile before "+N more".
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

/// The tone of a verdict: holding ok, degraded warn, violated crit. A check
/// that cannot run (unverifiable, no grant) is locked, never green.
export function verdictTone(status: VerdictStatus): Tone {
  switch (status) {
    case "holding":
      return "ok";
    case "degraded":
      return "warn";
    case "violated":
      return "crit";
    default:
      return "locked";
  }
}

export interface VerdictInputs {
  records: MemoryPanelRecord[];
  recordsDenied: boolean;
  dreams: MemoryDreamRun[];
  dreamsDenied: boolean;
  lattice: LatticeWalkResult | null;
  latticeRunning?: boolean;
  /// Store overview (panel/overview), with scopes ALREADY filtered through
  /// visibleOverviewScopes: a denied scope's floor pressure must not leak
  /// into the tile. Null while the surface has not answered.
  overview?: {
    scopes: MemoryScopeOverview[];
    floors?: { records?: number; bytes?: number };
  } | null;
  overviewDenied?: boolean;
  /// Names a scope in the tile copy ("Support Triage", "Ops team").
  /// Defaults to overviewScopeName without an agent directory.
  scopeName?: (scope: MemoryScopeOverview) => string;
  now?: number;
}

const RECORDS_NOT_READABLE = "Your access does not include reading memory records.";

/// The checks' internal names, kept for the tooltip only.
const VERDICT_INTERNAL_NAMES: Record<VerdictTile["id"], string> = {
  "echo-safety": "Echo safety",
  "taint-wall": "Taint wall",
  lattice: "Lattice",
  recall: "Recall",
  dreams: "Dreams",
  "store-floor": "Store floor",
};

export function computeVerdictTiles(inputs: VerdictInputs): VerdictTile[] {
  const now = inputs.now ?? Date.now();
  const nameScope = inputs.scopeName ?? ((scope: MemoryScopeOverview) => overviewScopeName(scope));
  const tiles: VerdictTile[] = [];

  // Echo safety and the taint wall cannot be assessed from the panel's
  // reads. They say so, and point at the related evidence, instead of
  // guessing from partial data.
  tiles.push({
    id: "echo-safety",
    label: "Repeated injections",
    status: "unverifiable",
    summary: "Not checked automatically",
    lines: [
      "The same memory should not be injected into an agent again and again.",
      "This console cannot check it automatically. On the Knowledge tab, the list of memory given to agents marks repeats.",
    ],
    targetTab: "knowledge",
  });
  tiles.push({
    id: "taint-wall",
    label: "Untrusted sessions",
    status: inputs.recordsDenied ? "no-grant" : "unverifiable",
    summary: inputs.recordsDenied ? "No access to records" : "Not checked automatically",
    lines: [
      "Memory written in an untrusted session stays quarantined.",
      inputs.recordsDenied
        ? RECORDS_NOT_READABLE
        : "This console cannot check it automatically. The Pipeline tab marks proposals from untrusted sessions.",
    ],
    targetTab: "pipeline",
  });

  const latticeScope =
    "Records written by agents stay at observed trust or below, and replacement chains have no loops or gaps.";
  if (inputs.recordsDenied) {
    tiles.push({
      id: "lattice",
      label: "Trust rules",
      status: "no-grant",
      summary: "No access to records",
      lines: [latticeScope, RECORDS_NOT_READABLE],
      targetTab: "records",
    });
  } else if (!inputs.lattice) {
    tiles.push({
      id: "lattice",
      label: "Trust rules",
      status: "unverifiable",
      summary: inputs.latticeRunning ? "Checking the records" : "Not checked yet",
      lines: [
        latticeScope,
        inputs.latticeRunning
          ? "Checking the records now."
          : "The check runs once the records have loaded.",
      ],
      targetTab: "records",
    });
  } else {
    // A settled verdict is retained while a re-check runs: the tile must not
    // flicker to UNVERIFIABLE on every debounced live refresh. The re-check
    // shows over the tile's corner, so the tile's text never changes and the
    // tiles around it never move.
    const walk = inputs.lattice;
    const violations = [...walk.llmCeilingViolations, ...walk.chainViolations];
    const count = violations.length;
    const coverage = walk.complete ? "" : ` in the first ${walk.checked}`;
    tiles.push({
      id: "lattice",
      label: "Trust rules",
      status: count > 0 ? "violated" : "holding",
      summary:
        count > 0
          ? `${plural(count, "record")} ${count === 1 ? "breaks" : "break"} the trust rules${coverage}`
          : `No record breaks the trust rules${coverage}`,
      lines: [
        latticeScope,
        plural(count, "violation"),
        walk.complete
          ? `Checked ${walk.checked} of ${plural(walk.checked, "record")}.`
          : `Checked the first ${plural(walk.checked, "record")}; the check stops at ${LATTICE_WALK_MAX_RECORDS}.`,
        ...(count > VERDICT_EVIDENCE_MAX
          ? [`+${count - VERDICT_EVIDENCE_MAX} more violations`]
          : []),
      ],
      rechecking: inputs.latticeRunning === true,
      targetTab: "records",
      evidence: violations.slice(0, VERDICT_EVIDENCE_MAX),
      evidenceMore: Math.max(0, count - VERDICT_EVIDENCE_MAX),
    });
  }

  const recallScope = `A record injected at least ${DEAD_INJECTION_THRESHOLD} times without ever being judged useful counts as never useful; it only costs space.`;
  if (inputs.recordsDenied) {
    tiles.push({
      id: "recall",
      label: "Usefulness",
      status: "no-grant",
      summary: "No access to records",
      lines: [recallScope, RECORDS_NOT_READABLE],
      targetTab: "records",
    });
  } else {
    const dead = inputs.records.filter((record) => recordUtility(record).dead);
    const deadBytes = dead.reduce((sum, record) => sum + recordUtility(record).bytesSpent, 0);
    const loaded = inputs.records.length;
    tiles.push({
      id: "recall",
      label: "Usefulness",
      status: dead.length > 0 ? "degraded" : "holding",
      summary:
        dead.length > 0
          ? `${dead.length} of ${plural(loaded, "record")} never ${dead.length === 1 ? "helps" : "help"}`
          : "No record is injected without helping",
      lines: [
        recallScope,
        `${dead.length} of ${plural(loaded, "loaded record")} ${dead.length === 1 ? "is" : "are"} never useful.`,
        ...(dead.length > 0 ? [`About ${formatSize(deadBytes)} spent injecting them.`] : []),
      ],
      targetTab: "records",
    });
  }

  const dreamsScope = "The memory steward's dream runs, which review, merge and tidy memory.";
  if (inputs.dreamsDenied) {
    tiles.push({
      id: "dreams",
      label: "Dream runs",
      status: "no-grant",
      summary: "No access to the dream history",
      lines: [dreamsScope, "Your access does not include the dream history."],
      targetTab: "dreams",
    });
  } else if (inputs.dreams.length === 0) {
    tiles.push({
      id: "dreams",
      label: "Dream runs",
      status: "unverifiable",
      summary: "No dream has run yet",
      lines: [dreamsScope, "The audit log holds no dream run yet."],
      targetTab: "dreams",
    });
  } else {
    const lastOp = Math.max(
      ...inputs.dreams.map((run) => run.last_op_at_ms || run.first_op_at_ms || 0),
    );
    const quarantined = inputs.dreams.reduce((sum, run) => sum + (run.quarantined_ops || 0), 0);
    const when = formatRelativeTime(lastOp, now);
    const lastRun = when ? `Last run ${when}` : "Last run time unknown";
    tiles.push({
      id: "dreams",
      label: "Dream runs",
      status: "holding",
      summary:
        quarantined > 0 ? `${lastRun}, ${plural(quarantined, "change")} quarantined` : lastRun,
      lines: [
        dreamsScope,
        `${lastRun}.`,
        quarantined > 0
          ? `${plural(quarantined, "change")} quarantined.`
          : "No change quarantined.",
      ],
      targetTab: "dreams",
    });
  }

  const floorScope =
    "Warns when a scope holds close to the per-scope limit of records or space.";
  if (inputs.overviewDenied) {
    tiles.push({
      id: "store-floor",
      label: "Storage limits",
      status: "no-grant",
      summary: "No access to store totals",
      lines: [floorScope, "Your access does not include the store totals."],
      targetTab: "holdings",
    });
  } else if (!inputs.overview) {
    tiles.push({
      id: "store-floor",
      label: "Storage limits",
      status: "unverifiable",
      summary: "Store totals are not available",
      lines: [floorScope, "This runtime does not report store totals."],
      targetTab: "holdings",
    });
  } else {
    const floor = storeFloorVerdict(inputs.overview.scopes);
    const floors = inputs.overview.floors;
    const limitLine = floors
      ? `Limit per scope: ${floors.records ?? "?"} records or ${
          typeof floors.bytes === "number" ? formatSize(floors.bytes) : "?"
        }.`
      : "The per-scope limit is not reported.";
    const names = floor.pressured.map((scope) => nameScope(scope));
    tiles.push({
      id: "store-floor",
      label: "Storage limits",
      status: floor.status === "ok" ? "holding" : "degraded",
      summary:
        floor.status === "ok"
          ? "No scope is near its limit"
          : names.length === 1
            ? `${names[0]} is near its limit`
            : `${names.length} scopes are near their limit`,
      lines:
        floor.status === "ok"
          ? [
              floorScope,
              `No scope is near its limit (${plural(inputs.overview.scopes.length, "scope")} checked).`,
              limitLine,
            ]
          : [floorScope, `Near the limit: ${names.join(", ")}.`, limitLine],
      targetTab: "holdings",
    });
  }

  // The headline is in plain words; the check's internal name stays
  // available as the tooltip's last line for people who know it.
  return tiles.map((tile) => ({ ...tile, lines: [...tile.lines, `Internal name: ${VERDICT_INTERNAL_NAMES[tile.id]}`] }));
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
  /// record: the consecutive-duplicate tripwire for the historical
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
  /// Set when the stored detail did not parse as the expected shape: the
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
/// console timeline only approximates; the excerpt says so.
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

/// "Open record" pivot payload for a live memory frame (the Signals rail's
/// "Open in Memory" uses the same): the durable record the event talks about,
/// if the payload names one.
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

/// A message about a quarantine decision: a plain sentence, plus the record
/// or promotion IDs it concerns, which render as copyable technical IDs
/// after the sentence instead of inside it. `title` is the sentence's
/// tooltip: the grant names behind an access refusal.
export interface QuarantineNotice {
  text: string;
  ids: Array<{ label: string; value: string }>;
  title?: string;
}

/// A record status said in a sentence ("discarded", "replaced"), for the
/// statuses a refusal reports.
function statusInSentence(status: unknown): string {
  const known = STATUS_WORD[String(status) as MemoryRecordStatus["status"]];
  return known ? known.toLowerCase() : humanizeEnum(String(status)).toLowerCase();
}

/// What a quarantine decision did. The panel then opens the record it left
/// active (the release) or the decided record itself.
function quarantineDecisionSummary(result: MemoryQuarantineDecideResult): QuarantineNotice {
  const successor = result.successor?.memory_id;
  const successorIds = successor ? [{ label: "New record", value: successor }] : [];
  switch (result.outcome) {
    case "released":
      return {
        text: "Released. A new active record holds this memory at observed trust, marked as once quarantined; the quarantined original is kept, marked as discarded.",
        ids: successorIds,
      };
    case "already_released":
      return { text: "Already released earlier; nothing changed.", ids: successorIds };
    case "tombstoned": {
      const invalidated = result.decision?.review?.invalidated_promotions ?? [];
      return {
        text:
          invalidated.length === 0
            ? "Discarded; it will never be recalled."
            : `Discarded; it will never be recalled. ${
                invalidated.length === 1
                  ? "The promotion waiting on it was cancelled"
                  : `The ${invalidated.length} promotions waiting on it were cancelled`
              } and can no longer publish.`,
        ids: invalidated.map((id) => ({ label: "Cancelled promotion", value: id })),
      };
    }
    case "already_tombstoned":
      return { text: "Already discarded; nothing changed.", ids: [] };
    default:
      return { text: `Decided: ${humanizeEnum(String(result.outcome)).toLowerCase()}.`, ids: [] };
  }
}

/// The grants a quarantine decision needs, for the refusal's tooltip.
const QUARANTINE_DECISION_GRANTS =
  "Grants: memory.quarantine.review, agent.view and agent.memory.read, plus agent.memory.write to release or agent.memory.delete to discard (tombstone)";

/// A failed quarantine decision in words: the typed refusal reasons of
/// meta/memory/quarantine/decide, the access denial, or the raw error.
function quarantineDecisionErrorText(error: unknown): QuarantineNotice {
  const rpcError = (error as { rpcError?: { code?: unknown; data?: unknown } } | null)?.rpcError;
  if (rpcError?.code === -32030) {
    return {
      text: "No grant. To decide, you need permission to review quarantined memory and to view this agent and read its memory. Releasing also needs permission to write its memory, and discarding to delete its memory.",
      ids: [],
      title: QUARANTINE_DECISION_GRANTS,
    };
  }
  const data = (rpcError?.data ?? null) as Record<string, unknown> | null;
  if (data?.kind === "memory_quarantine_review_refused") {
    switch (data.reason) {
      case "not_found":
        return { text: "Refused: this record is no longer in the agent's memory.", ids: [] };
      case "content_mismatch":
        return {
          text: "Refused: the record differs from the one loaded here. Reload it and review again.",
          ids: [],
        };
      case "not_quarantined":
        return data.released_as
          ? {
              text: "Refused: it was already released.",
              ids: [{ label: "Released as", value: String(data.released_as) }],
            }
          : { text: `Refused: the record is ${statusInSentence(data.status)}, not quarantined.`, ids: [] };
      case "gate_pending": {
        const expires =
          typeof data.expires_at_ms === "number" && data.expires_at_ms > 0
            ? `, or wait until it expires on ${formatAbsoluteTime(data.expires_at_ms)}`
            : "";
        return {
          text: `Refused: a promotion of this record is waiting for approval. Decide it in Approvals${expires}. Discarding the record instead cancels that promotion.`,
          ids: data.pending_id ? [{ label: "Waiting promotion", value: String(data.pending_id) }] : [],
        };
      }
      case "successor_conflict":
        return {
          text: "Refused: another record already holds the ID this release would create; nothing was changed.",
          ids: data.successor_id ? [{ label: "Conflicting record", value: String(data.successor_id) }] : [],
        };
      case "secret_detected":
        return {
          text: `Refused: the content matches the ${String(data.class)} secret pattern; it can only be discarded.`,
          ids: [],
        };
      case "stale_update":
        return {
          text: `Refused: the version this record replaces is ${statusInSentence(data.prior_status)}; discard it instead.`,
          ids: data.prior ? [{ label: "Replaced version", value: String(data.prior) }] : [],
        };
      default:
        break;
    }
  }
  return { text: errorMessage(error), ids: [] };
}

// ── Display helpers ───────────────────────────────────────────────────────
// What people read. The data helpers above keep their exact outputs (tests
// and pivots depend on them); these turn the same data into words.

/// A key resolver: an agent key to the name people know it by.
export type NameOf = (key: string) => string;

/// A size for people: "860 B", "2.2 KB", "32.0 MB".
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const KIND_LABEL: Record<string, string> = {
  preference: "Preference",
  fact: "Fact",
  gotcha: "Gotcha",
  procedure: "Procedure",
  relationship: "Relationship",
  open_loop: "Open loop",
  reference: "Reference",
};

/// A record kind in words: "Fact", "Open loop".
export function kindLabel(kind: string | undefined): string {
  if (!kind) return "Record";
  return KIND_LABEL[kind] ?? humanizeEnum(kind);
}

const TRUST_DISPLAY: Record<MemoryTrust, string> = {
  untrusted: "Untrusted",
  agent_observed: "Observed",
  agent_verified: "Verified",
  application: "Application",
  operator: "Operator",
};

/// A trust level in words: "Observed", "Verified".
export function trustDisplay(trust: MemoryTrust | undefined): string {
  if (!trust) return "Unknown trust";
  return TRUST_DISPLAY[trust] ?? humanizeEnum(trust);
}

/// Trust chips are green from verified up and neutral below; status chips
/// carry the warnings.
export function trustChipTone(trust: MemoryTrust | undefined): Tone {
  return trustTone(trust) === "positive" ? "ok" : "neutral";
}

const STATUS_WORD: Record<MemoryRecordStatus["status"], string> = {
  active: "Active",
  superseded: "Replaced",
  quarantined: "Quarantined",
  tombstoned: "Discarded",
};

/// A record status in one word: "Active", "Replaced". The store's own
/// words (superseded, tombstoned) stay in the tooltip, statusTitle.
export function statusWord(status: MemoryRecordStatus | undefined): string {
  if (!status) return "Unknown";
  return STATUS_WORD[status.status] ?? humanizeEnum(status.status);
}

const STATUS_TITLE: Partial<Record<MemoryRecordStatus["status"], string>> = {
  superseded: "Superseded: a newer version replaced this record.",
  tombstoned: "Tombstoned: kept only as a deletion marker; it is never recalled.",
};

/// The tooltip of a status word: the store's term for it and what it means.
export function statusTitle(status: MemoryRecordStatus | undefined): string | undefined {
  return status ? STATUS_TITLE[status.status] : undefined;
}

export function statusChipTone(status: MemoryRecordStatus | undefined): Tone {
  switch (status?.status) {
    case "active":
      return "ok";
    case "quarantined":
      return "warn";
    default:
      return "neutral";
  }
}

/// Who wrote a record: "Coordinator (agent)", "The memory steward".
export function authorDisplay(
  author: MemoryAuthor | undefined,
  nameOf: NameOf = stripAgentKeyPrefix,
): string {
  if (!author) return "Unknown";
  switch (author.author) {
    case "agent":
      return author.identity ? `${nameOf(author.identity) || author.identity} (agent)` : "An agent";
    case "steward":
      return "The memory steward";
    case "distiller":
      return "The memory distiller";
    case "operator":
      return "An operator";
    case "application":
      return "The application";
    default:
      return "Unknown";
  }
}

/// What kind of memory a scope holds: "Agent memory", "Team memory".
export function scopeKindLabel(kind: string): string {
  switch (kind) {
    case "identity":
      return "Agent memory";
    case "mob":
      return "Team memory";
    case "operator":
      return "Operator memory";
    case "realm":
      return "Shared memory";
    default:
      return humanizeEnum(kind) || "Memory";
  }
}

/// The key a scope is filed under (empty for the realm-wide scope).
export function scopeKeyOf(scope: MemoryRecordScope): string {
  switch (scope.scope) {
    case "identity":
      return scope.identity;
    case "mob":
      return scope.mob;
    case "operator":
      return scope.operator;
    case "realm":
      return "";
  }
}

/// A team named from its key: "ops-team" reads "Ops team", "research" reads
/// "Research team". The key itself stays the tooltip.
export function teamName(key: string): string {
  const words = humanizeEnum(key);
  if (!words) return "Team";
  return /\bteam$/i.test(words) ? words : `${words} team`;
}

/// The owner of a scope by name: the agent's display name, the team's name
/// from its key, the operator key as given, "Everyone".
export function scopeOwnerName(kind: string, key: string, nameOf: NameOf = stripAgentKeyPrefix): string {
  switch (kind) {
    case "identity":
      return nameOf(key) || key;
    case "mob":
      return teamName(key);
    case "realm":
      return "Everyone";
    default:
      return key || humanizeEnum(kind);
  }
}

/// A records group heading: the agent's name, or the kind of memory and
/// whose it is ("Team memory · Ops team").
export function scopeHeading(scope: MemoryRecordScope, nameOf: NameOf = stripAgentKeyPrefix): string {
  switch (scope.scope) {
    case "identity":
      return nameOf(scope.identity) || scope.identity;
    case "mob":
      return joinMeta(scopeKindLabel(scope.scope), teamName(scope.mob));
    case "realm":
      return "Shared memory";
    default:
      return joinMeta(scopeKindLabel(scope.scope), scopeKeyOf(scope));
  }
}

/// A store-totals scope named in a sentence: "Support Triage",
/// "Research team", "Shared memory".
export function overviewScopeName(scope: MemoryScopeOverview, nameOf: NameOf = stripAgentKeyPrefix): string {
  switch (scope.scope_kind) {
    case "identity":
      return nameOf(scope.scope_key) || scope.scope_key;
    case "mob":
      return teamName(scope.scope_key);
    case "operator":
      return `Operator ${scope.scope_key}`;
    case "realm":
      return "Shared memory";
    default:
      return [scope.scope_kind, scope.scope_key].filter(Boolean).join(" ");
  }
}

/// A scope's record counts in words: "3 active, 1 replaced".
export function scopeCountsText(counts: {
  active?: number;
  quarantined?: number;
  superseded?: number;
  tombstoned?: number;
}): string {
  const parts = [`${counts.active ?? 0} active`];
  if ((counts.quarantined ?? 0) > 0) parts.push(`${counts.quarantined} quarantined`);
  if ((counts.superseded ?? 0) > 0) parts.push(`${counts.superseded} replaced`);
  if ((counts.tombstoned ?? 0) > 0) parts.push(`${counts.tombstoned} discarded`);
  return parts.join(", ");
}

/// Where a proposal or promotion goes: an agent by name, "Ops team",
/// "admin@example.test (operator)", "everyone".
export function targetName(kind: string, key: string, nameOf: NameOf = stripAgentKeyPrefix): string {
  switch (kind) {
    case "identity":
      return nameOf(key) || key;
    case "mob":
      return teamName(key);
    case "operator":
      return `${key} (operator)`;
    case "realm":
      return "everyone";
    default:
      return [kind, key].filter(Boolean).join(" ");
  }
}

/// An evidence reference as a sentence: "Conversation with Coordinator,
/// messages 12-18". The session, generation and revision stay technical.
export function evidenceDisplay(evidence: MemoryEvidenceRef, who?: string): string {
  const base = who ? `Conversation with ${who}` : "Conversation";
  if (evidence.range && evidence.range.length === 2) {
    const [start, end] = evidence.range;
    return start === end ? `${base}, message ${start}` : `${base}, messages ${start}-${end}`;
  }
  return base;
}

/// The technical half of an evidence reference: "generation 4".
export function evidenceDetails(evidence: MemoryEvidenceRef): string {
  return joinMeta(
    typeof evidence.generation === "number" ? `generation ${evidence.generation}` : null,
    evidence.revision ? `revision ${evidence.revision}` : null,
  );
}

/// When a record was injected: a build-surface injection happens as the
/// agent starts, a turn-surface one during a turn.
export function injectionMoment(surface: string | undefined): string {
  return surface === "build" ? "at startup" : "during a turn";
}

/// A record's usefulness in words, for the least-useful-first list.
export function utilityDisplay(record: Pick<MemoryPanelRecord, "usage" | "body_bytes">): string {
  const utility = recordUtility(record);
  if (utility.injected === 0 && utility.recalled === 0) return "Never injected or recalled";
  return joinMeta(
    `Injected ${plural(utility.injected, "time")}`,
    `recalled ${plural(utility.recalled, "time")}`,
    `judged useful ${plural(utility.useful, "time")}`,
    utility.bytesSpent > 0 ? `about ${formatSize(utility.bytesSpent)} spent` : null,
  );
}

const REVIEW_VERDICT_LABEL: Record<string, string> = {
  dead_weight: "Never useful",
};

/// A usage-audit verdict in words: dead_weight reads "Never useful".
export function reviewVerdictLabel(verdict: string): string {
  return REVIEW_VERDICT_LABEL[verdict] ?? humanizeEnum(verdict);
}

/// The store's own words in a humanized label, said plainly: "tombstoned"
/// reads "discarded", "superseded" reads "replaced".
function plainStoreWords(words: string): string {
  return words
    .replace(/\btombstoned\b/g, "discarded")
    .replace(/\btombstone\b/g, "discard")
    .replace(/\bsuperseded\b/g, "replaced")
    .replace(/\bsupersede\b/g, "replace");
}

/// A dream run's non-zero verdict counters as chips: "5 kept", "1
/// quarantined". Counters about quarantine, blocks or dead weight warn.
export function dreamVerdictChips(
  verdicts: Array<[string, number]>,
): Array<{ key: string; label: string; tone: Tone }> {
  return verdicts.map(([name, count]) => ({
    key: name,
    label: `${count} ${plainStoreWords(humanizeEnum(name).toLowerCase())}`,
    tone: /quarantin|blocked|dead|reject/.test(name) ? "warn" : "neutral",
  }));
}

const OP_KIND_DONE: Record<string, string> = {
  create: "created",
  promote: "promoted",
  merge: "merged",
  quarantine: "quarantined",
  supersede: "replaced",
  tombstone: "discarded",
  update: "updated",
  release: "released",
};

/// A dream run's changes by kind, most first: "3 promoted, 2 merged".
export function dreamOpKindsDisplay(opKinds: Record<string, number> | undefined): string {
  if (!opKinds) return "";
  return Object.entries(opKinds)
    .filter(([, count]) => typeof count === "number" && count > 0)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([kind, count]) => `${count} ${OP_KIND_DONE[kind] ?? plainStoreWords(humanizeEnum(kind).toLowerCase())}`)
    .join(", ");
}

/// Whose memory a dream ran over, from its partition key
/// ("identity:coordinator" reads "Coordinator", "mob:ops-team" reads
/// "Ops team").
export function partitionName(partition: string | undefined, nameOf: NameOf = stripAgentKeyPrefix): string {
  if (!partition) return "";
  const separator = partition.indexOf(":");
  const kind = separator > 0 ? partition.slice(0, separator) : partition;
  const key = separator > 0 ? partition.slice(separator + 1) : "";
  if (kind === "realm") return "everyone";
  if (!key) return partition;
  switch (kind) {
    case "identity":
    case "agent":
    case "member":
      return nameOf(partition) || key;
    case "mob":
      return teamName(key);
    case "operator":
      return `${key} (operator)`;
    default:
      return partition;
  }
}

/// A knowledge-lens segment in words, for the selected agent.
export function knowledgeSegmentTitle(segment: KnowledgeSegment, agentName: string): string {
  switch (segment.filter.scope) {
    case "identity":
      return `${agentName}'s own memory`;
    case "mob":
      return "Team memory";
    case "operator":
      return "Operator memory";
    case "realm":
      return "Shared memory";
    default:
      return segment.label;
  }
}

/// A permission the reader lacks, as a sentence: "You need permission to
/// review quarantined memory". The grant's own name is the tooltip
/// (grantTitle), never the sentence.
export function needsGrantText(grant: string): string {
  if (!isKnownAccessAction(grant)) return "You need a permission you do not have";
  const words = accessActionLabel(grant);
  return `You need permission to ${words.charAt(0).toLowerCase()}${words.slice(1)}`;
}

/// The tooltip under a needsGrantText sentence: the grant's name.
export function grantTitle(grant: string): string {
  return `Grant: ${grant}`;
}

/// A realm named in a line: "ops space". Realms are the separate stores of
/// state a gateway can serve; the word "realm" stays the tooltip.
export function realmSpaceName(realm: string): string {
  return `${realm} space`;
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
  resolveRecordsSearchKey,
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
  // Display helpers
  verdictTone,
  formatSize,
  kindLabel,
  trustDisplay,
  trustChipTone,
  statusWord,
  statusTitle,
  statusChipTone,
  authorDisplay,
  scopeKindLabel,
  scopeKeyOf,
  teamName,
  scopeOwnerName,
  scopeHeading,
  overviewScopeName,
  scopeCountsText,
  targetName,
  evidenceDisplay,
  evidenceDetails,
  injectionMoment,
  utilityDisplay,
  reviewVerdictLabel,
  dreamVerdictChips,
  dreamOpKindsDisplay,
  partitionName,
  knowledgeSegmentTitle,
  needsGrantText,
  grantTitle,
  realmSpaceName,
};

// ── Presentational pieces ─────────────────────────────────────────────────

/// Display names for agent keys, from the directory the console provides.
function useNameOf(): NameOf {
  const directory = useAgentDirectory();
  return React.useCallback((key: string) => agentDisplayName(key, directory), [directory]);
}

/// Parts of a meta line joined with the middle dot, skipping empty ones.
function MetaLine({ parts }: { parts: React.ReactNode[] }): React.JSX.Element {
  const shown = parts.filter(
    (part) => part !== null && part !== undefined && part !== false && part !== "",
  );
  return (
    <>
      {shown.map((part, index) => (
        <React.Fragment key={index}>
          {index > 0 ? " · " : null}
          {part}
        </React.Fragment>
      ))}
    </>
  );
}

/// Where a proposal or promotion goes, with the agent's name when it is
/// an agent's memory and the team's name (its key in the tooltip) when it
/// is a team's.
function ScopeTarget({ kind, scopeKey }: { kind: string; scopeKey: string }): React.JSX.Element {
  if (kind === "identity") return <AgentName id={scopeKey} />;
  if (kind === "mob") return <span title={scopeKey}>{targetName(kind, scopeKey)}</span>;
  return <>{targetName(kind, scopeKey)}</>;
}

/// The facts line under a record title: its kind in words, then the trust
/// and status chips. Rows leave out "Active", the default. A replaced
/// record's Biography links the newer version.
function RecordFacts({
  record,
  showActive = false,
  hideStatus = false,
  onOpenNewer,
}: {
  record: Pick<MemoryPanelRecord, "kind" | "trust" | "status">;
  showActive?: boolean;
  hideStatus?: boolean;
  onOpenNewer?: () => void;
}): React.JSX.Element {
  const status = record.status?.status;
  return (
    <span className="memory-facts">
      <span className="memory-facts__kind">{kindLabel(record.kind)}</span>
      <Chip tone={trustChipTone(record.trust)}>{trustDisplay(record.trust)}</Chip>
      {status && !hideStatus && (showActive || status !== "active") ? (
        <Chip tone={statusChipTone(record.status)} title={statusTitle(record.status)}>
          {statusWord(record.status)}
        </Chip>
      ) : null}
      {status === "superseded" && !hideStatus && onOpenNewer ? (
        <button type="button" className="memory-link" onClick={onOpenNewer}>
          Open the newer version
        </button>
      ) : null}
    </span>
  );
}

/// A record named by its title (looked up in the loaded records) that opens
/// its Biography. A record that is not loaded shows its ID, middle-truncated.
function RecordLink({
  id,
  realm,
  titles,
  testId,
  className,
  onOpen,
}: {
  id: string;
  realm: string | undefined;
  titles: ReadonlyMap<string, string>;
  testId?: string;
  className?: string;
  onOpen: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const title = titles.get(id);
  return (
    <button
      type="button"
      className={className ? `memory-link ${className}` : "memory-link"}
      data-testid={testId}
      title={title ? id : undefined}
      onClick={() => onOpen(realm, id)}
    >
      {title ?? <TechnicalId value={id} copy={false} />}
    </button>
  );
}

/// A quarantine message: the sentence, then the IDs it concerns.
function QuarantineNoticeText({ notice }: { notice: QuarantineNotice }): React.JSX.Element {
  return (
    <>
      {notice.title ? <span title={notice.title}>{notice.text}</span> : notice.text}
      {notice.ids.length > 0 ? (
        <span className="memory-notice-ids">
          {notice.ids.map((id) => (
            <span className="memory-notice-ids__item" key={`${id.label}:${id.value}`}>
              <span className="memory-notice-ids__label">{id.label}</span>
              <TechnicalId value={id.value} label={id.label} />
            </span>
          ))}
        </span>
      ) : null}
    </>
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
  const reason = record.status?.status === "quarantined" ? record.status.reason : undefined;
  return (
    <ListRow
      className="memory-record-row"
      testId={`memory-record:${record.id}`}
      onActivate={onSelect}
      title={record.title || "Untitled record"}
      titleAttr={record.title || undefined}
      subtitle={<RecordFacts record={record} />}
      meta={
        <>
          {utility?.dead ? <Chip tone="warn">Never useful</Chip> : null}
          <RelativeTime value={record.updated_at_ms} />
        </>
      }
    >
      {reason || utility ? (
        <>
          {reason ? (
            <span className="memory-row-note" title={reason}>
              {reason}
            </span>
          ) : null}
          {utility ? <span className="memory-row-note">{utilityDisplay(record)}</span> : null}
        </>
      ) : null}
    </ListRow>
  );
}

interface EvidenceState {
  key: string;
  /// "not-found": the session did not appear in the recent timeline window
  /// (it may merely be older than the window; never claim it is gone).
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
  record,
  onDecide,
}: {
  record: MemoryFullRecord;
  onDecide: (verdict: MemoryQuarantineVerdict, rationale: string | undefined) => Promise<void>;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const rationaleId = React.useId();
  const [rationale, setRationale] = React.useState("");
  const [pending, setPending] = React.useState<MemoryQuarantineVerdict | null>(null);
  const [failure, setFailure] = React.useState<QuarantineNotice | null>(null);
  const rationaleProblem = quarantineRationaleProblem(rationale);
  const blocked = pending !== null || rationaleProblem !== null;
  const notice: QuarantineNotice | null = rationaleProblem
    ? { text: rationaleProblem, ids: [] }
    : failure;
  const owner =
    record.scope.scope === "identity"
      ? `${nameOf(record.scope.identity) || record.scope.identity}'s memory`
      : "this memory";
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
    <section className="memory-detail__section memory-review" data-testid="memory-quarantine-decide">
      <SectionHeader as="h4" label="Review" />
      <div className="memory-detail__content memory-review__body">
        <Notice tone="warn">
          Release makes this an active record in {owner} at observed trust. Because it was
          quarantined, it can never reach verified trust. Discard marks it deleted for good, so
          it is never recalled.
        </Notice>
        <label className="memory-review__label" htmlFor={rationaleId}>
          Rationale, optional and kept in the audit log
        </label>
        <input
          id={rationaleId}
          className="cc-input memory-quarantine__rationale"
          aria-label="Rationale"
          data-testid="memory-quarantine-rationale"
          placeholder="Why you release or discard it"
          value={rationale}
          disabled={pending !== null}
          onChange={(event) => setRationale(event.target.value)}
        />
        <div className="cc-btn-row">
          <Button
            data-testid="memory-quarantine-release"
            disabled={blocked}
            onClick={() => void decide("release")}
          >
            {pending === "release" ? "Releasing…" : "Release"}
          </Button>
          <Button
            variant="danger"
            data-testid="memory-quarantine-tombstone"
            title="Tombstone: keeps only a deletion marker; the record is never recalled."
            disabled={blocked}
            onClick={() => void decide("tombstone")}
          >
            {pending === "tombstone" ? "Discarding…" : "Discard"}
          </Button>
        </div>
        {notice ? (
          <Notice tone="crit" testId="memory-quarantine-error">
            <QuarantineNoticeText notice={notice} />
          </Notice>
        ) : null}
      </div>
    </section>
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
  decisionNotice?: QuarantineNotice | null;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const { record, chain, injections } = detail;
  const provenance = record.provenance;
  const author = provenance?.author;
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
      : author?.author === "agent"
        ? author.identity
        : undefined;
  const conversationWith = recordIdentity ? nameOf(recordIdentity) || recordIdentity : undefined;
  const supersededBy = record.status?.status === "superseded" ? record.status.by : undefined;
  const authorRun =
    author && (author.author === "steward" || author.author === "distiller") ? author.run_id : undefined;
  const scopeKey = scopeKeyOf(record.scope);

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
      status: entries === null ? "not-found" : lines.length > 0 ? "loaded" : "empty-range",
      lines,
    });
  }

  return (
    <article className="memory-detail" data-testid="memory-detail">
      <div className="memory-detail__bar">
        <Button
          variant="ghost"
          size="sm"
          icon={<span className="memory-back-mark" />}
          data-testid="memory-detail-back"
          onClick={onBack}
        >
          All records
        </Button>
        <CopyButton
          text={JSON.stringify(record, null, 2)}
          label="Copy record JSON"
          className="memory-copy-json"
        />
      </div>

      <header className="memory-detail__head">
        <h3 className="memory-detail__title">{record.title || "Untitled record"}</h3>
        <div className="memory-detail__facts">
          <RecordFacts
            record={record}
            showActive
            onOpenNewer={
              supersededBy ? () => onSelectRecord(realmOfRecord(record), supersededBy) : undefined
            }
          />
          <span className="memory-detail__owner">
            {record.scope.scope === "identity" ? (
              <>
                <AgentName id={record.scope.identity} />
                {"'s memory"}
              </>
            ) : (
              scopeHeading(record.scope, nameOf)
            )}
            {record.updated_at_ms ? (
              <>
                {", updated "}
                <RelativeTime value={record.updated_at_ms} />
              </>
            ) : null}
          </span>
        </div>
      </header>

      {record.description ? (
        <p className="memory-detail__description">{record.description}</p>
      ) : null}

      <div className="memory-detail__body" data-testid="memory-detail-body">
        {record.body}
      </div>

      {decisionNotice ? (
        <Notice tone="ok" testId="memory-quarantine-outcome" className="memory-detail__notice">
          <QuarantineNoticeText notice={decisionNotice} />
        </Notice>
      ) : null}

      {onDecideQuarantine && canDecideQuarantinedRecord(record) ? (
        <QuarantineDecisionBlock
          key={record.id}
          record={record}
          onDecide={(verdict, rationale) => onDecideQuarantine(record, verdict, rationale)}
        />
      ) : null}

      {record.tags && record.tags.length > 0 ? (
        <div className="cc-chips memory-detail__tags">
          {record.tags.map((tag) => (
            <Chip key={tag}>{tag}</Chip>
          ))}
        </div>
      ) : null}

      <section className="memory-detail__section" data-testid="memory-detail-born">
        <SectionHeader as="h4" label="Where it came from" />
        <div className="memory-detail__content">
          <KeyValueList
            items={[
              {
                key: "author",
                label: "Written by",
                value:
                  author?.author === "agent" && author.identity ? (
                    <>
                      <AgentName id={author.identity} />
                      {" (agent)"}
                    </>
                  ) : (
                    authorDisplay(author, nameOf)
                  ),
              },
              evidence.length > 0
                ? {
                    key: "evidence",
                    label: "Evidence",
                    value: (
                      <span className="memory-evidence">
                        {evidence.map((ref, index) => (
                          // The session ID, generation and revision are in
                          // the technical details below, not on the link.
                          <button
                            type="button"
                            className="memory-evidence__ref"
                            key={`ev-${index}`}
                            data-testid={`memory-evidence:${index}`}
                            onClick={() => void openEvidence(ref, index)}
                            disabled={!onLoadEvidence}
                            title={onLoadEvidence ? "Open the conversation" : undefined}
                          >
                            <span className="memory-evidence__label">
                              {evidenceDisplay(ref, conversationWith)}
                            </span>
                          </button>
                        ))}
                      </span>
                    ),
                  }
                : null,
              verification?.checked
                ? { key: "verified", label: "Verified", value: verification.checked }
                : null,
            ]}
          />
          {evidenceState ? (
            evidenceState.status === "loading" ? (
              <p className="memory-caption" role="status">
                Loading the conversation…
              </p>
            ) : evidenceState.status === "not-found" ? (
              <Notice testId="memory-evidence-degraded">
                This conversation was not found in the recent timeline window. It may be older
                than the window; only its reference is kept here.
              </Notice>
            ) : evidenceState.status === "empty-range" ? (
              <Notice testId="memory-evidence-empty">
                The conversation was found, but it has no messages in this range. Ranges are
                matched to the console timeline approximately.
              </Notice>
            ) : (
              <div className="memory-excerpt" data-testid="memory-evidence-excerpt">
                <p className="memory-excerpt__note">
                  Matched to the console timeline approximately.
                </p>
                {evidenceState.lines.map((line) => (
                  <div className="memory-excerpt__line" key={line.id}>
                    <span className="memory-excerpt__speaker">{line.speaker}</span>
                    <span className="memory-excerpt__text">{line.text}</span>
                  </div>
                ))}
              </div>
            )
          ) : null}
        </div>
      </section>

      {lane.length > 0 ? (
        <section className="memory-detail__section" data-testid="memory-detail-lineage">
          <SectionHeader as="h4" label="History" count={lane.length} />
          <RowList className="memory-chain">
            {lane.map(({ record: entry, current }) => (
              <ListRow
                key={entry.id}
                className="memory-chain__row"
                dense
                selected={current}
                testId={`memory-chain:${entry.id}`}
                dataAttributes={{
                  "data-current": current ? "true" : undefined,
                  "data-dimmed": current ? undefined : "true",
                }}
                onActivate={() => {
                  if (!current) onSelectRecord(realmOfRecord(entry), entry.id);
                }}
                lead={<span className="memory-chain__marker" aria-hidden="true" />}
                title={entry.title || "Untitled record"}
                titleAttr={entry.title || undefined}
                meta={
                  <>
                    <Chip tone={trustChipTone(entry.trust)}>{trustDisplay(entry.trust)}</Chip>
                    <Chip tone={statusChipTone(entry.status)} title={statusTitle(entry.status)}>
                      {statusWord(entry.status)}
                    </Chip>
                    {current ? (
                      <span className="memory-chain__current">Current</span>
                    ) : (
                      <RelativeTime value={entry.updated_at_ms} />
                    )}
                  </>
                }
              />
            ))}
          </RowList>
        </section>
      ) : null}

      <section className="memory-detail__section" data-testid="memory-detail-life">
        <SectionHeader as="h4" label="Usage" />
        <p className="memory-usage">
          {usage ? (
            <>
              injected {usage.injected_count ?? 0} · recalled {usage.explicit_recall_count ?? 0} ·
              judged useful {usage.judged_useful_count ?? 0}
              {usage.last_injected_at_ms ? (
                <>
                  {" · last injected "}
                  <RelativeTime value={usage.last_injected_at_ms} />
                </>
              ) : null}
            </>
          ) : (
            "No usage recorded yet."
          )}
        </p>
        {injections.length > 0 ? (
          <RowList className="memory-detail__rows">
            {injections.map((injection, index) => (
              <ListRow
                key={`inj-${index}`}
                dense
                title={
                  <>
                    {"Given to "}
                    <AgentName id={injection.identity} />
                    {` ${injectionMoment(injection.surface)}`}
                  </>
                }
                titleAttr={injection.session_key ? `Session ${injection.session_key}` : undefined}
                meta={<RelativeTime value={injection.at_ms} />}
              />
            ))}
          </RowList>
        ) : (
          <p className="memory-caption">No injections recorded for this record.</p>
        )}
      </section>

      <section className="memory-detail__section" data-testid="memory-detail-dreams">
        <SectionHeader as="h4" label="Dreams" />
        {touchingRuns.length > 0 ? (
          <RowList className="memory-detail__rows">
            {touchingRuns.map((run) => (
              <ListRow
                key={run.run_id}
                dense
                title="Reviewed by a dream"
                titleAttr={`Dream run ${run.run_id}`}
                meta={
                  <>
                    {run.quarantined_ops ? (
                      <Chip tone="warn">{`${plural(run.quarantined_ops, "change")} quarantined`}</Chip>
                    ) : null}
                    <RelativeTime value={run.last_op_at_ms || run.first_op_at_ms} />
                  </>
                }
              />
            ))}
          </RowList>
        ) : (
          <p className="memory-caption">
            No sampled dream run mentions this record. Each dream run lists a sample of up to 12
            of the records it touched.
          </p>
        )}
      </section>

      <TechnicalDetails
        className="memory-detail__tech"
        items={[
          { label: "Memory ID", value: record.id },
          { label: "Scope", value: record.scope.scope, plain: true },
          scopeKey ? { label: "Scope key", value: scopeKey } : null,
          { label: "Realm", value: detail.realm, plain: true },
          { label: "Kind", value: record.kind, plain: true },
          { label: "Trust", value: record.trust, plain: true },
          { label: "Content hash", value: record.content_hash },
          { label: "Replaces", value: record.supersedes },
          { label: "Replaced by", value: supersededBy },
          record.derived_from && record.derived_from.length > 0
            ? {
                label: "Derived from",
                value: (
                  <span className="memory-tech-list">
                    {record.derived_from.map((id) => (
                      <TechnicalId key={id} value={id} label="Record ID" />
                    ))}
                  </span>
                ),
              }
            : null,
          { label: "Author run", value: authorRun },
          // Each evidence reference's session, generation and revision: the
          // evidence link above shows only the conversation and its messages.
          ...evidence.map((ref, index) => {
            const details = evidenceDetails(ref);
            return ref.session_id || details
              ? {
                  label: evidence.length > 1 ? `Evidence ${index + 1}` : "Evidence",
                  value: (
                    <span className="memory-tech-list">
                      {ref.session_id ? <TechnicalId value={ref.session_id} label="Session ID" /> : null}
                      {details ? <span className="memory-tech-note">{details}</span> : null}
                    </span>
                  ),
                }
              : null;
          }),
          touchingRuns.length > 0
            ? {
                label: "Dream runs",
                value: (
                  <span className="memory-tech-list">
                    {touchingRuns.map((run) => (
                      <TechnicalId key={run.run_id} value={run.run_id} label="Dream run ID" />
                    ))}
                  </span>
                ),
              }
            : null,
        ]}
      />
    </article>
  );
}

// ── Holdings ──────────────────────────────────────────────────────────────

/// The two checks this console can never run render last, muted, on one
/// line.
const MUTED_TILES: ReadonlySet<VerdictTile["id"]> = new Set(["echo-safety", "taint-wall"]);

function VerdictTileCard({
  tile,
  muted,
  recordTitles,
  onOpen,
  onOpenRecord,
}: {
  tile: VerdictTile;
  muted: boolean;
  recordTitles: ReadonlyMap<string, string>;
  onOpen: (tile: VerdictTile) => void;
  onOpenRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const tone = verdictTone(tile.status);
  const evidence = tile.evidence ?? [];
  return (
    <StatTile
      className={muted ? "memory-tile memory-tile--quiet" : "memory-tile"}
      testId={`memory-verdict:${tile.id}`}
      dataAttributes={{ "data-status": tile.status }}
      label={tile.label}
      tone={tone}
      muted={muted}
      help={tile.lines.join("\n")}
      status={
        <>
          <Chip tone={tone} dot>
            {verdictStatusLabel(tile.status)}
          </Chip>
          {tile.rechecking ? (
            // Over the tile's corner, outside the text flow: a re-check never
            // grows the tile or moves the tiles around it.
            <span className="memory-tile__recheck" data-testid={`memory-verdict-recheck:${tile.id}`}>
              <StatusDot tone="info" live />
              re-checking
            </span>
          ) : null}
        </>
      }
      summary={tile.summary}
      onActivate={() => onOpen(tile)}
    >
      {evidence.length > 0 ? (
        <>
          {evidence.map((violation, index) => (
            <RecordLink
              key={`${violation.realm}:${violation.id}:${index}`}
              className="memory-tile__evidence"
              id={violation.id}
              realm={violation.realm}
              titles={recordTitles}
              testId={`memory-verdict-evidence:${tile.id}:${violation.id}`}
              onOpen={onOpenRecord}
            />
          ))}
          {tile.evidenceMore ? (
            <span className="memory-tile__more">{`+${tile.evidenceMore} more`}</span>
          ) : null}
        </>
      ) : null}
    </StatTile>
  );
}

function VerdictStrip({
  tiles,
  recordTitles,
  onOpen,
  onOpenRecord,
}: {
  tiles: VerdictTile[];
  recordTitles: ReadonlyMap<string, string>;
  onOpen: (tile: VerdictTile) => void;
  onOpenRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const main = tiles.filter((tile) => !MUTED_TILES.has(tile.id));
  const quiet = tiles.filter((tile) => MUTED_TILES.has(tile.id));
  return (
    <div className="memory-checks" data-testid="memory-verdict-strip">
      <TileGrid className="memory-tiles">
        {main.map((tile) => (
          <VerdictTileCard
            key={tile.id}
            tile={tile}
            muted={false}
            recordTitles={recordTitles}
            onOpen={onOpen}
            onOpenRecord={onOpenRecord}
          />
        ))}
      </TileGrid>
      {quiet.length > 0 ? (
        <div className="memory-tiles-quiet">
          {quiet.map((tile) => (
            <VerdictTileCard
              key={tile.id}
              tile={tile}
              muted
              recordTitles={recordTitles}
              onOpen={onOpen}
              onOpenRecord={onOpenRecord}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

/// One Holdings scope row, from the store totals or, when those are not
/// available, from the loaded records.
interface ScopeRowModel {
  key: string;
  kind: string;
  scopeKey: string;
  realm: string;
  active: number;
  quarantined: number;
  superseded: number;
  tombstoned: number;
  bytes: number;
  floorPressure: boolean;
  filter: MemoryRecordsFilter;
}

function scopeRowFromOverview(scope: MemoryScopeOverview): ScopeRowModel {
  return {
    key: overviewScopeKey(scope),
    kind: scope.scope_kind,
    scopeKey: scope.scope_key,
    realm: scope.realm,
    active: scope.active ?? 0,
    quarantined: scope.quarantined ?? 0,
    superseded: scope.superseded ?? 0,
    tombstoned: scope.tombstoned ?? 0,
    bytes: scope.body_bytes ?? 0,
    floorPressure: scope.floor_pressure === true,
    filter: filterForOverviewScope(scope),
  };
}

function scopeRowFromLoaded(row: ScopeOverviewRow): ScopeRowModel {
  return {
    key: row.key,
    kind: row.scope.scope,
    scopeKey: scopeKeyOf(row.scope),
    realm: row.scope.realm,
    active: row.active,
    quarantined: row.quarantined,
    superseded: row.superseded,
    tombstoned: row.tombstoned,
    bytes: row.bytes,
    floorPressure: false,
    filter: filterForScope(row.scope),
  };
}

function ScopeRow({
  row,
  multiRealm,
  limitTitle,
  onOpen,
}: {
  row: ScopeRowModel;
  multiRealm: boolean;
  limitTitle: string;
  onOpen: () => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const agent = row.kind === "identity";
  const name = scopeOwnerName(row.kind, row.scopeKey, nameOf);
  const counts = scopeCountsText(row);
  return (
    <ListRow
      className="memory-scope-row"
      testId={`memory-holdings-scope:${row.key}`}
      onActivate={onOpen}
      lead={agent ? <AgentAvatar name={name} /> : <span className="memory-scope-mark" aria-hidden="true" />}
      title={agent ? <AgentName id={row.scopeKey} /> : name}
      // A team reads by its name; the key it is filed under is the tooltip.
      titleAttr={row.kind === "mob" && row.scopeKey ? row.scopeKey : undefined}
      subtitle={
        <MetaLine
          parts={[
            scopeKindLabel(row.kind),
            multiRealm ? <span title={`Realm ${row.realm}`}>{realmSpaceName(row.realm)}</span> : null,
            counts,
          ]}
        />
      }
      meta={
        <>
          {row.floorPressure ? (
            <Chip tone="warn" testId={`memory-holdings-floor:${row.key}`} title={limitTitle}>
              Near limit
            </Chip>
          ) : null}
          <Meter
            className="memory-scope-meter"
            showLabel={false}
            label={counts}
            segments={[
              { value: row.active, tone: "ok", label: `${row.active} active` },
              { value: row.quarantined, tone: "warn", label: `${row.quarantined} quarantined` },
              { value: row.superseded, tone: "neutral", label: `${row.superseded} replaced` },
              { value: row.tombstoned, tone: "locked", label: `${row.tombstoned} discarded` },
            ]}
          />
          <span className="memory-size">{formatSize(row.bytes)}</span>
        </>
      }
    />
  );
}

/// Denied scopes vanish from the row-filtered listing; the one-row probes
/// make them render as no-grant rows instead of silently not existing. The
/// missing grant reads as a sentence, its name the tooltip.
function DeniedScopeRow({ kind }: { kind: "mob" | "operator" }): React.JSX.Element {
  const grant = `${kind}.memory.read`;
  return (
    <ListRow
      className="memory-scope-row"
      testId={`memory-holdings-scope-denied:${kind}`}
      lead={<KitGlyph name="shield" className="memory-scope-lock" />}
      title={kind === "mob" ? "Team memory" : "Operator memory"}
      subtitle={<span title={grantTitle(grant)}>{needsGrantText(grant)}</span>}
      meta={<Chip tone="locked">No grant</Chip>}
    />
  );
}

function HarvestRow({ harvest }: { harvest: MemoryHarvestEntry }): React.JSX.Element {
  const nameOf = useNameOf();
  const name = nameOf(harvest.identity) || harvest.identity;
  const cause =
    harvest.cause && harvest.cause.trim().toLowerCase() !== "retired" ? humanizeEnum(harvest.cause) : "";
  return (
    <ListRow
      testId={`memory-harvest:${harvest.identity}`}
      lead={<AgentAvatar name={name} />}
      title={<AgentName id={harvest.identity} />}
      subtitle={
        harvest.retired_at_ms ? (
          <RelativeTime
            value={harvest.retired_at_ms}
            prefix="Retired"
            title={joinMeta(
              formatAbsoluteTime(harvest.retired_at_ms),
              harvest.session_key ? `session ${harvest.session_key}` : null,
            )}
          />
        ) : (
          "Retired"
        )
      }
      meta={cause ? <Chip>{cause}</Chip> : undefined}
    />
  );
}

// ── Dreams ────────────────────────────────────────────────────────────────

function DreamRunRow({
  run,
  expanded,
  onToggle,
}: {
  run: MemoryDreamRunSheet;
  expanded: boolean;
  onToggle: () => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const detail = normalizeDreamRunDetail(run.detail);
  const who = partitionName(run.partition, nameOf);
  const at = run.completed_at_ms || run.started_at_ms;
  const duration =
    run.started_at_ms && run.completed_at_ms
      ? formatDuration(run.completed_at_ms - run.started_at_ms)
      : "";
  const chips = dreamVerdictChips(detail.verdicts);
  return (
    <div className="memory-dream-run" data-testid={`memory-dream-run:${run.run_id}`}>
      <ListRow
        className="memory-dream-run__head"
        testId={`memory-dream-run-toggle:${run.run_id}`}
        onActivate={onToggle}
        ariaExpanded={expanded}
        lead={<span className="memory-disclosure" data-open={expanded ? "true" : undefined} aria-hidden="true" />}
        title={who ? `Dream for ${who}` : "Dream"}
        subtitle={
          <MetaLine
            parts={[
              at ? <RelativeTime value={at} /> : null,
              duration,
              typeof run.ops_committed === "number" ? plural(run.ops_committed, "change") : null,
            ]}
          />
        }
        meta={
          chips.length > 0 ? (
            <>
              {chips.map((chip) => (
                <Chip key={chip.key} tone={chip.tone} title={chip.key}>
                  {chip.label}
                </Chip>
              ))}
            </>
          ) : undefined
        }
      />
      {expanded ? (
        <div className="memory-dream-run__detail" data-testid={`memory-dream-run-detail:${run.run_id}`}>
          {detail.raw !== null ? (
            <Notice>The details of this run are stored in a form this console cannot read.</Notice>
          ) : (
            <>
              {detail.phases.length > 0 ? (
                <ol className="memory-phases" aria-label="Phases">
                  {detail.phases.map(([name, note], index) => (
                    <li className="memory-phases__step" key={`ph-${index}`}>
                      <KitGlyph name="check" className="memory-phases__mark" />
                      <span className="memory-phases__name">{humanizeEnum(name)}</span>
                      {note ? <span className="memory-phases__note">{note}</span> : null}
                    </li>
                  ))}
                </ol>
              ) : (
                <p className="memory-caption">No phases were recorded.</p>
              )}
              {detail.verdicts.length === 0 ? (
                <p className="memory-caption">No changes were counted.</p>
              ) : null}
              {detail.skips.map((skip, index) => (
                <Notice tone="warn" key={`sk-${index}`}>
                  {skip}
                </Notice>
              ))}
            </>
          )}
          <TechnicalDetails
            items={[
              { label: "Run ID", value: run.run_id },
              { label: "Partition", value: run.partition },
              { label: "Realm", value: run.realm, plain: true },
              { label: "Started", value: formatAbsoluteTime(run.started_at_ms), plain: true },
              { label: "Completed", value: formatAbsoluteTime(run.completed_at_ms), plain: true },
              detail.raw !== null ? { label: "Stored details", value: detail.raw, plain: true } : null,
            ]}
          />
        </div>
      ) : null}
    </div>
  );
}

function AuditRunRow({
  run,
  partition,
  recordTitles,
  onOpenRecord,
}: {
  run: MemoryDreamRun;
  partition: string | undefined;
  recordTitles: ReadonlyMap<string, string>;
  onOpenRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const who = partitionName(partition, nameOf);
  const at = run.last_op_at_ms || run.first_op_at_ms;
  const span =
    run.first_op_at_ms && run.last_op_at_ms && run.last_op_at_ms > run.first_op_at_ms
      ? formatDuration(run.last_op_at_ms - run.first_op_at_ms)
      : "";
  const touched = run.memory_ids || [];
  const rationales = run.rationales || [];
  return (
    <ListRow
      className="memory-audit-run"
      testId={`memory-dream:${run.run_id}`}
      title={who ? `Dream for ${who}` : "Dream run"}
      titleAttr={`Dream run ${run.run_id}`}
      subtitle={
        <MetaLine
          parts={[
            at ? <RelativeTime value={at} /> : null,
            span,
            typeof run.ops === "number" ? plural(run.ops, "change") : null,
            dreamOpKindsDisplay(run.op_kinds),
          ]}
        />
      }
      meta={
        run.quarantined_ops && run.quarantined_ops > 0 ? (
          <Chip tone="warn">{`${run.quarantined_ops} quarantined`}</Chip>
        ) : undefined
      }
    >
      {touched.length > 0 || rationales.length > 0 ? (
        <>
          {touched.length > 0 ? (
            <span className="memory-touched">
              <span className="memory-touched__label">Touched</span>
              {touched.map((memoryId) => (
                <RecordLink
                  key={memoryId}
                  className="memory-touched__record"
                  id={memoryId}
                  realm={run.realm}
                  titles={recordTitles}
                  testId={`memory-dream-record:${run.run_id}:${memoryId}`}
                  onOpen={onOpenRecord}
                />
              ))}
            </span>
          ) : null}
          {rationales.map((rationale, index) => (
            <span className="memory-row-note memory-audit-run__note" key={`r-${index}`}>
              {rationale}
            </span>
          ))}
        </>
      ) : null}
    </ListRow>
  );
}

// ── Live events ───────────────────────────────────────────────────────────

/// Exported for the jsdom component-interaction lane, which pins the
/// pause-on-scroll / N-behind / auto-unfreeze discipline.
export function MemoryLiveStrip({
  frames,
  recordTitles,
  onPivot,
}: {
  frames: ConsoleFrame[];
  /** Record titles by memory id, so events name records people know. */
  recordTitles?: ReadonlyMap<string, string>;
  onPivot: (realm: string | undefined, recordId: string) => void;
}): React.JSX.Element {
  const directory = useAgentDirectory();
  // Live-follow discipline: scrolling away from the top freezes the visible
  // list; "N behind · Jump to live" resumes. Frames dedupe by id so
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
    <section className="cc-section memory-live" data-testid="memory-live-strip">
      <SectionHeader
        label="Live events"
        action={
          behind > 0 ? (
            <Button size="sm" variant="ghost" data-testid="memory-live-jump" onClick={jumpToLive}>
              {`${behind} behind · Jump to live`}
            </Button>
          ) : null
        }
      />
      <p className="memory-caption">Recent memory events, newest first. Older events are not kept here.</p>
      <div className="cc-rows memory-live__list" ref={listRef} onScroll={handleScroll}>
        {shown.length === 0 ? (
          <EmptyState inline title="No memory events yet." />
        ) : (
          shown.map((frame) => {
            const data =
              frame.data && typeof frame.data === "object"
                ? (frame.data as Record<string, unknown>)
                : {};
            const pivot = memoryFramePivot(frame);
            // Names and record titles lead; the exact technical line is the
            // tooltip.
            const text = describeMemoryEventForPeople(frame.event, data, {
              agentName: (key) => agentDisplayName(key, directory),
              recordTitle: (memoryId) => recordTitles?.get(memoryId),
            });
            return (
              <ListRow
                key={frame.id}
                dense
                className="memory-live__row"
                testId={`memory-live-row:${frame.id}`}
                title={text}
                titleAttr={describeMemoryTimelineEvent(frame.event, data)}
                meta={
                  <>
                    {pivot ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        data-testid={`memory-live-pivot:${frame.id}`}
                        onClick={() => onPivot(pivot.realm, pivot.recordId)}
                      >
                        Open record
                      </Button>
                    ) : null}
                    <RelativeTime value={frame.timestampMs} />
                  </>
                }
              />
            );
          })
        )}
        {/* The ring keeps 1024 frames per identity, 4096 in total; anything
            older is unknowable live. The seam keeps "no events" from being
            mistaken for "nothing happened". */}
        <div className="memory-live__seam" data-testid="memory-live-seam">
          Live history starts here
        </div>
      </div>
    </section>
  );
}

// ── Main panel ────────────────────────────────────────────────────────────

const SCOPE_FILTER_OPTIONS: SelectOption[] = [
  { value: "", label: "All" },
  { value: "identity", label: "Agents" },
  { value: "mob", label: "Teams" },
  { value: "operator", label: "Operators" },
  { value: "realm", label: "Shared" },
];

const STATUS_FILTER_OPTIONS: SelectOption[] = [
  { value: "", label: "All" },
  { value: "active", label: STATUS_WORD.active },
  { value: "quarantined", label: STATUS_WORD.quarantined },
  { value: "superseded", label: STATUS_WORD.superseded },
  { value: "tombstoned", label: STATUS_WORD.tombstoned },
];

const SORT_OPTIONS: SelectOption[] = [
  { value: "recency", label: "Recent" },
  { value: "utility", label: "Least useful" },
];

export function MemoryPanel({
  runtimeControl,
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
  loaded = true,
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
  const nameOf = useNameOf();
  const [tab, setTab] = React.useState<MemoryTab>("holdings");
  const tabIds = React.useId();
  // The outcome of the last quarantine decision, shown on the Biography it
  // navigated to (the released record after a release, the origin otherwise).
  const [decisionNotice, setDecisionNotice] = React.useState<QuarantineNotice | null>(null);
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
  // Names people can type in the records search: each agent scope key the
  // panel has seen, by its display name, and each team key by the team name
  // the headings show.
  const searchDirectory = useAgentDirectory();
  const searchCandidatesRef = React.useRef<Array<{ key: string; name: string }>>([]);
  searchCandidatesRef.current = React.useMemo(() => {
    const keys = new Set<string>();
    const teams = new Set<string>();
    for (const record of records) {
      if (record.scope?.scope === "identity" && record.scope.identity) keys.add(record.scope.identity);
      if (record.scope?.scope === "mob" && record.scope.mob) teams.add(record.scope.mob);
    }
    for (const scope of overview?.scopes ?? []) {
      if (scope.scope_kind === "identity" && scope.scope_key) keys.add(scope.scope_key);
      if (scope.scope_kind === "mob" && scope.scope_key) teams.add(scope.scope_key);
    }
    return [
      ...Array.from(keys, (key) => ({ key, name: agentDisplayName(key, searchDirectory) })),
      ...Array.from(teams, (key) => ({ key, name: teamName(key) })),
    ];
  }, [records, overview, searchDirectory]);
  const pagerRef = React.useRef<ReturnType<typeof createMemoryRecordsPager> | null>(null);
  if (!pagerRef.current) {
    pagerRef.current = createMemoryRecordsPager({
      query: (params) =>
        queryRecordsRef.current ? queryRecordsRef.current(params) : Promise.resolve(null),
      setPaged,
      setLoading: setPageLoading,
      resolveKey: (text) => resolveRecordsSearchKey(text, searchCandidatesRef.current),
    });
  }
  const pager = pagerRef.current;
  const [lattice, setLattice] = React.useState<LatticeWalkResult | null>(null);
  const [latticeRunning, setLatticeRunning] = React.useState(false);
  const [knowledgeIdentity, setKnowledgeIdentity] = React.useState<string>("");

  // A loaded Biography (e.g. via the Signals rail's "Open in Memory" pivot) lands
  // on the Records tab, where the detail pane lives. Keyed on the open
  // record, not the detail object: a Refresh of the same record writes a new
  // object and must not pull the operator back from another tab.
  const openRecordKey = detail ? `${detail.realm}\u0000${detail.record.id}` : null;
  React.useEffect(() => {
    if (openRecordKey) setTab("records");
  }, [openRecordKey]);

  // Lattice invariants (a) and (c) via the client page-walk. Deduped by a
  // CONTENT fingerprint (not array identity): content-identical debounced
  // live refreshes skip the walk entirely. While a re-check runs, the prior
  // result is retained so a settled tile never flickers to UNVERIFIABLE.
  // Nothing walks before the first records page lands: the panel can render
  // first with an empty placeholder, and a verdict settled over it would
  // make the first page's arrival walk the same store a second time, in the
  // middle of whatever the operator does next.
  const latticeRanForRef = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (tab !== "holdings" || !onQueryRecords || recordsDenied || !loaded) return;
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
      // (re-dock, the 250ms memory.* debounce) leaves the tile checking
      // forever: the fingerprint reads as already-checked while lattice is
      // still null and latticeRunning was never cleared (its finally is
      // cancellation-guarded). A covered fingerprint stays marked, so a
      // refresh that brings the same records does not walk them again.
      if (!covered && latticeRanForRef.current === fingerprint) {
        latticeRanForRef.current = null;
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab, records, recordsDenied, realms, nextCursor, loaded]);

  // When the data last landed: each good refresh delivers new record arrays,
  // a failed one keeps the old ones (and shows its error instead).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const updatedAt = React.useMemo(() => (loaded ? Date.now() : null), [records, loaded]);

  // Overview scopes with denied-probe kinds dropped (their denied rows render
  // instead); feeds both the Holdings scopes and the store floor tile.
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
        scopeName: (scope) => overviewScopeName(scope, nameOf),
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
      nameOf,
    ],
  );
  // Record titles for every place that only holds an ID (violations,
  // injections, review verdicts, dream samples, promotions).
  const recordTitles = React.useMemo(() => {
    const titles = new Map<string, string>();
    const add = (list: ReadonlyArray<Pick<MemoryPanelRecord, "id" | "title">> | undefined) => {
      for (const entry of list ?? []) {
        if (entry.title && !titles.has(entry.id)) titles.set(entry.id, entry.title);
      }
    };
    add(records);
    add(quarantineRecords);
    add(paged?.records);
    if (detail) add([detail.record, ...detail.chain]);
    return titles;
  }, [records, quarantineRecords, paged, detail]);
  const annotatedInjections = React.useMemo(
    () => annotateInjectionDups(injections),
    [injections],
  );
  const dreamSheets = React.useMemo(() => dreamRunsNewestFirst(dreamRuns), [dreamRuns]);
  const sheetPartitions = React.useMemo(
    () => new Map(dreamRuns.map((run) => [run.run_id, run.partition])),
    [dreamRuns],
  );
  const [expandedRuns, setExpandedRuns] = React.useState<Record<string, boolean>>({});
  const identities = React.useMemo(() => identityOptions(records), [records]);
  const selectedIdentity = identities.includes(knowledgeIdentity)
    ? knowledgeIdentity
    : identities[0] || "";
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
      <PanelShell className="memory-panel" testId="memory-panel">
        <PanelHeader title="Memory" actions={runtimeControl || undefined} />
        <EmptyState
          kind="not-configured"
          testId="memory-unavailable"
          title="Memory is not set up for this team."
          body="This runtime has no memory store configured, so there is nothing to show here."
        />
      </PanelShell>
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
  const multiRealm = realms.length > 1;
  const recordCount = nextCursor ? `${records.length}+ records` : plural(records.length, "record");
  const subtitle = !loaded
    ? "Loading records"
    : recordsDenied
      ? "No access to records"
      : joinMeta(recordCount, multiRealm ? plural(realms.length, "space") : null);
  const tabs: Array<PanelTab<MemoryTab>> = MEMORY_TABS.map((candidate) => ({
    id: candidate,
    label: memoryTabLabel(candidate),
    count:
      candidate === "pipeline" && quarantineCount > 0
        ? quarantineCount
        : candidate === "dreams" && dreams.length > 0
          ? dreams.length
          : null,
    testId: `memory-tab:${candidate}`,
  }));

  function renderHoldings(): React.JSX.Element {
    const rows: ScopeRowModel[] = overviewScopes
      ? overviewScopes.map(scopeRowFromOverview)
      : scopeOverviewRows(records).map(scopeRowFromLoaded);
    const totalBytes = rows.reduce((sum, row) => sum + row.bytes, 0);
    const floors = overview?.floors;
    const limitTitle = floors
      ? `Close to the per-scope limit of ${floors.records ?? "?"} records or ${
          typeof floors.bytes === "number" ? formatSize(floors.bytes) : "?"
        }.`
      : "Close to the per-scope limit.";
    const tainted = proposals.filter((proposal) => proposal.tainted).length;
    const lastDream = dreams[0];
    const lastDreamAt = lastDream ? lastDream.last_op_at_ms || lastDream.first_op_at_ms : undefined;
    const scopesReady = loaded && !recordsDenied;
    return (
      <div className="memory-tab" data-testid="memory-holdings">
        <PanelSection label="Checks">
          <VerdictStrip
            tiles={tiles}
            recordTitles={recordTitles}
            onOpen={openTile}
            onOpenRecord={(realm, memoryId) => onSelectRecord(realm, memoryId)}
          />
        </PanelSection>

        <PanelSection
          label="Scopes"
          count={scopesReady && rows.length > 0 ? rows.length : null}
          action={
            scopesReady && rows.length > 0 ? (
              <span className="memory-section-meta">{`${formatSize(totalBytes)} in total`}</span>
            ) : undefined
          }
        >
          {recordsDenied ? (
            <Notice tone="locked" testId="memory-holdings-denied">
              Records: no grant. Your access does not include reading memory records.
            </Notice>
          ) : !loaded ? (
            <LoadingState label="Loading memory" rows={3} />
          ) : (
            <>
              {!overviewScopes ? (
                <p className="memory-caption">
                  {`Counts cover the ${plural(records.length, "loaded record")}, not the whole store.`}
                </p>
              ) : null}
              {rows.length === 0 && !operatorScopeDenied && !mobScopeDenied ? (
                <EmptyState inline title="No memory records yet." />
              ) : (
                <RowList>
                  {rows.map((row) => (
                    <ScopeRow
                      key={row.key}
                      row={row}
                      multiRealm={multiRealm}
                      limitTitle={limitTitle}
                      onOpen={() => openRecordsFiltered(row.filter)}
                    />
                  ))}
                  {mobScopeDenied ? <DeniedScopeRow kind="mob" /> : null}
                  {operatorScopeDenied ? <DeniedScopeRow kind="operator" /> : null}
                </RowList>
              )}
            </>
          )}
        </PanelSection>

        <PanelSection label="In progress" padded>
          <KeyValueList
            className="memory-progress"
            items={[
              {
                key: "dream",
                label: "Last dream",
                value: lastDream ? (
                  <MetaLine
                    parts={[
                      lastDreamAt ? <RelativeTime value={lastDreamAt} /> : "Time unknown",
                      typeof lastDream.ops === "number" ? plural(lastDream.ops, "change") : null,
                    ]}
                  />
                ) : dreamsDenied ? (
                  "No grant"
                ) : (
                  "No dream has run yet"
                ),
              },
              {
                key: "quarantine",
                label: "Quarantine",
                value: canReviewQuarantine ? (
                  <span className="memory-progress__value">
                    {`${plural(quarantineRecords.length, "record")}, ${pendingPromotions.length} waiting for approval`}
                    <Button size="sm" onClick={() => setTab("pipeline")}>
                      Open pipeline
                    </Button>
                  </span>
                ) : (
                  <span title={grantTitle("memory.quarantine.review")}>
                    {needsGrantText("memory.quarantine.review")}
                  </span>
                ),
              },
              {
                key: "proposals",
                label: "Proposals",
                value: proposalsDenied
                  ? "No grant"
                  : `${proposals.length} waiting${
                      tainted > 0
                        ? `, ${tainted} held because ${tainted === 1 ? "its source was" : "their sources were"} untrusted`
                        : ""
                    }`,
              },
            ]}
          />
        </PanelSection>

        <PanelSection
          label="Retired agents awaiting their summary"
          count={!harvestsDenied && harvests.length > 0 ? harvests.length : null}
          testId="memory-harvests"
        >
          {harvestsDenied ? (
            <EmptyState inline kind="no-access" title="Retired agents: no grant." />
          ) : harvests.length === 0 ? (
            <EmptyState inline title="No retired agent is waiting." />
          ) : (
            <RowList>
              {harvests.map((harvest, index) => (
                <HarvestRow key={`${harvest.realm}:${harvest.identity}:${index}`} harvest={harvest} />
              ))}
            </RowList>
          )}
        </PanelSection>
      </div>
    );
  }

  function renderRecordList(): React.JSX.Element {
    const filtered = hasActiveFilter(filter);
    return (
      <div className="memory-tab">
        {onQueryRecords ? (
          <Toolbar testId="memory-filter" className="memory-toolbar">
            <SearchField
              label="Search by agent"
              placeholder="Agent name or key"
              value={filter.key || ""}
              testId="memory-filter-input"
              onChange={(value) => setFilter({ ...filter, key: value })}
              onKeyDown={(event) => {
                if (event.key === "Enter") applyFilter(filter);
              }}
              onBlur={() => {
                // Re-query only on a real change: a blur caused by clicking
                // load more must not race the append.
                void pager.applyFilterIfChanged(filter);
              }}
            />
            <SelectField
              label="Scope"
              value={filter.scope || ""}
              options={SCOPE_FILTER_OPTIONS}
              testId="memory-filter:scope"
              onChange={(value) =>
                applyFilter({ ...filter, scope: (value || undefined) as MemoryRecordsFilter["scope"] })
              }
            />
            <SelectField
              label="Status"
              value={filter.status || ""}
              options={STATUS_FILTER_OPTIONS}
              testId="memory-filter:status"
              onChange={(value) =>
                applyFilter({ ...filter, status: (value || undefined) as MemoryRecordsFilter["status"] })
              }
            />
            {multiRealm ? (
              // Realms read as spaces: the stores of state this gateway serves.
              <SelectField
                label="Space"
                value={filter.realm || ""}
                options={[
                  { value: "", label: "All spaces" },
                  ...realms.map((realm) => ({ value: realm, label: realm })),
                ]}
                testId="memory-filter:realm"
                onChange={(value) => applyFilter({ ...filter, realm: value || undefined })}
              />
            ) : null}
            <SelectField
              label="Sort"
              value={sortMode}
              options={SORT_OPTIONS}
              testId="memory-sort"
              onChange={(value) => setSortMode(value === "utility" ? "utility" : "recency")}
            />
            {filtered ? (
              <Button variant="ghost" data-testid="memory-filter-clear" onClick={() => applyFilter({})}>
                Clear filters
              </Button>
            ) : null}
          </Toolbar>
        ) : null}
        {sortMode === "utility" ? (
          <Notice testId="memory-utility-note">
            {`Least useful first: records injected at least ${DEAD_INJECTION_THRESHOLD} times and never judged useful lead, marked Never useful. Space spent is estimated from the injections and the record's size.`}
          </Notice>
        ) : null}
        {multiRealm && !filter.realm?.trim() ? (
          <Notice testId="memory-multi-realm-note">
            This page combines records from several spaces. Pick a space to page through its
            records.
          </Notice>
        ) : null}
        {!loaded || pageLoading ? <LoadingState label="Loading records" rows={4} /> : null}
        {loaded && !pageLoading && listView.records.length === 0 ? (
          recordsDenied || listView.denied ? (
            <EmptyState
              kind="no-access"
              testId="memory-records-empty"
              title="Records: no grant."
              body="Your access does not include reading these memory records."
            />
          ) : filtered ? (
            <EmptyState
              kind="filtered"
              testId="memory-records-empty"
              title="No records match these filters."
              action={<Button onClick={() => applyFilter({})}>Clear filters</Button>}
            />
          ) : (
            <EmptyState
              testId="memory-records-empty"
              title="No memory records yet."
              body="Records appear here once agents, dreams or operators write memory."
            />
          )
        ) : null}
        {loaded && !pageLoading && listView.denied && listView.records.length > 0 ? (
          <Notice tone="locked" testId="memory-records-denied-note">
            Further pages: no grant. Loading more of this list was denied for your access.
          </Notice>
        ) : null}
        {loaded && !pageLoading && listView.records.length > 0 ? (
          listView.mode === "flat" ? (
            <section className="cc-section memory-record-group">
              <SectionHeader
                label={sortMode === "utility" && !filtered ? "Least useful first" : "Matching records"}
                count={listView.records.length}
              />
              <RowList>
                {listView.records.map((record) => (
                  <RecordRow
                    key={record.id}
                    record={record}
                    utilityMode={sortMode === "utility"}
                    onSelect={() => onSelectRecord(realmOfRecord(record), record.id)}
                  />
                ))}
              </RowList>
            </section>
          ) : (
            listView.groups.map((group) => (
              <section
                className="cc-section memory-record-group"
                key={group.key}
                data-testid={`memory-group:${group.key}`}
              >
                <SectionHeader
                  label={
                    <>
                      {group.scope.scope === "identity" ? (
                        <AgentName id={group.scope.identity} />
                      ) : group.scope.scope === "mob" ? (
                        <span title={group.scope.mob}>{scopeHeading(group.scope, nameOf)}</span>
                      ) : (
                        scopeHeading(group.scope, nameOf)
                      )}
                      {multiRealm ? (
                        <span title={`Realm ${group.scope.realm}`}>
                          {` · ${realmSpaceName(group.scope.realm)}`}
                        </span>
                      ) : null}
                    </>
                  }
                  count={group.records.length}
                />
                <RowList>
                  {group.records.map((record) => (
                    <RecordRow
                      key={record.id}
                      record={record}
                      onSelect={() => onSelectRecord(realmOfRecord(record), record.id)}
                    />
                  ))}
                </RowList>
              </section>
            ))
          )
        ) : null}
        {listView.cursor && onQueryRecords ? (
          <div className="memory-load-more">
            <Button data-testid="memory-load-more" disabled={pageLoading} onClick={loadMore}>
              Load more records
            </Button>
          </div>
        ) : null}
      </div>
    );
  }

  function renderKnowledge(): React.JSX.Element {
    const agentName = selectedIdentity ? nameOf(selectedIdentity) || selectedIdentity : "";
    const segments = selectedIdentity ? knowledgeComposition(records, selectedIdentity) : [];
    const total = segments.reduce((sum, segment) => sum + segment.count, 0);
    return (
      <div className="memory-tab" data-testid="memory-knowledge">
        {identities.length === 0 ? (
          <EmptyState
            inline
            kind={recordsDenied ? "no-access" : "empty"}
            title={recordsDenied ? "Records: no grant." : "No agent memory is loaded yet."}
          />
        ) : (
          <>
            <Toolbar className="memory-toolbar">
              <SelectField
                label="Agent"
                value={selectedIdentity}
                options={identities.map((identity) => ({
                  value: identity,
                  label: nameOf(identity) || identity,
                }))}
                testId="memory-knowledge-identity"
                onChange={(value) => setKnowledgeIdentity(value)}
              />
            </Toolbar>
            <PanelSection label={`What ${agentName} can draw on`} count={total}>
              <RowList>
                {segments.map((segment) => (
                  <ListRow
                    key={segment.label}
                    className="memory-segment-row"
                    testId={`memory-knowledge-segment:${segment.label}`}
                    onActivate={() => openRecordsFiltered(segment.filter)}
                    title={knowledgeSegmentTitle(segment, agentName)}
                    subtitle={segment.approximate ? "Every record of this kind" : "Only this agent's records"}
                    meta={
                      <>
                        <Meter
                          className="memory-share-meter"
                          showLabel={false}
                          total={total}
                          label={`${plural(segment.count, "record")} of ${total}`}
                          segments={[{ value: segment.count, tone: "neutral" }]}
                        />
                        <span className="memory-count">{plural(segment.count, "record")}</span>
                      </>
                    }
                  />
                ))}
              </RowList>
              <p className="memory-caption">
                Team, operator and shared counts include every record of that kind, because this
                console cannot tell which teams an agent belongs to.
              </p>
            </PanelSection>
          </>
        )}
        <PanelSection
          label={<span title="Injection history">Memory given to agents</span>}
          count={!injectionsDenied && annotatedInjections.length > 0 ? annotatedInjections.length : null}
          testId="memory-knowledge-history"
        >
          {injectionsDenied ? (
            <EmptyState inline kind="no-access" title="Memory given to agents: no grant." />
          ) : annotatedInjections.length === 0 ? (
            <EmptyState inline title="No memory has been given to an agent yet." />
          ) : (
            <RowList>
              {annotatedInjections.map(({ entry, dup }, index) => (
                <ListRow
                  key={`inj-${index}`}
                  className="memory-injection-row"
                  testId={`memory-injection:${index}`}
                  title={
                    <RecordLink
                      id={entry.record_id}
                      realm={entry.realm}
                      titles={recordTitles}
                      testId={`memory-injection-record:${index}`}
                      onOpen={onSelectRecord}
                    />
                  }
                  subtitle={
                    <>
                      {"Given to "}
                      <AgentName id={entry.identity} />
                      {` ${injectionMoment(entry.surface)}`}
                    </>
                  }
                  meta={
                    <>
                      {dup ? (
                        <Chip
                          tone="warn"
                          testId={`memory-injection-dup:${index}`}
                          title="The agent was given the same record in its previous injection too."
                        >
                          Repeated
                        </Chip>
                      ) : null}
                      <RelativeTime
                        value={entry.at_ms}
                        title={joinMeta(
                          formatAbsoluteTime(entry.at_ms),
                          entry.session_key ? `session ${entry.session_key}` : null,
                        )}
                      />
                    </>
                  }
                />
              ))}
            </RowList>
          )}
        </PanelSection>
        <p className="memory-caption memory-footnote">
          <span data-testid="memory-knowledge-as-injected">
            The exact memory text given to an agent is not shown here.
          </span>{" "}
          <span data-testid="memory-knowledge-budget">
            Per-session memory budgets are not shown here either.
          </span>
        </p>
      </div>
    );
  }

  function renderPipeline(): React.JSX.Element {
    return (
      <div className="memory-tab" data-testid="memory-pipeline">
        <ol className="memory-stepper" data-testid="memory-pipeline-stages" aria-label="Pipeline stages">
          <li className="memory-stepper__step">
            <span className="memory-stepper__label">Proposed</span>{" "}
            <span className="memory-stepper__count">{proposalsDenied ? "no grant" : proposals.length}</span>
          </li>
          <li className="memory-stepper__step">
            <span className="memory-stepper__label">Waiting for approval</span>{" "}
            <span className="memory-stepper__count">
              {canReviewQuarantine ? pendingPromotions.length : "no grant"}
            </span>
          </li>
          <li className="memory-stepper__step">
            <span className="memory-stepper__label">Quarantined</span>{" "}
            <span className="memory-stepper__count">
              {canReviewQuarantine ? quarantineRecords.length : "no grant"}
            </span>
          </li>
        </ol>
        <Notice testId="memory-quarantine-note">
          {onDecideQuarantine
            ? "Open a quarantined record to release or discard it. Promotions waiting for approval are decided in Approvals; the memory steward's dream decides the rest."
            : "Read only. Decisions come from the memory steward's dream, from Approvals, and from reviewers who can review quarantine and write memory."}
        </Notice>

        <PanelSection
          label="Proposed"
          count={!proposalsDenied && proposals.length > 0 ? proposals.length : null}
          testId="memory-pipeline-proposals"
        >
          {proposalsDenied ? (
            <EmptyState inline kind="no-access" title="Proposals: no grant." />
          ) : proposals.length === 0 ? (
            <EmptyState inline title="No proposal is waiting for a dream." />
          ) : (
            <RowList>
              {proposals.map((proposal) => (
                <ListRow
                  key={`${proposal.realm}:${proposal.proposal_id}`}
                  className="memory-proposal-row"
                  testId={`memory-proposal:${proposal.proposal_id}`}
                  title={proposal.title || "Untitled proposal"}
                  titleAttr={joinMeta(proposal.title, `proposal ${proposal.proposal_id}`)}
                  subtitle={
                    <>
                      {`${kindLabel(proposal.kind)} for `}
                      <ScopeTarget kind={proposal.scope_kind} scopeKey={proposal.scope_key} />
                      {proposal.author ? (
                        <>
                          {" · proposed by "}
                          <AgentName id={proposal.author} />
                        </>
                      ) : null}
                    </>
                  }
                  meta={
                    <>
                      {proposal.tainted ? (
                        <Chip
                          tone="warn"
                          testId={`memory-proposal-taint:${proposal.proposal_id}`}
                          title="Tainted: proposed in a session that read untrusted input."
                        >
                          Untrusted source
                        </Chip>
                      ) : null}
                      {proposal.status && proposal.status !== "pending" ? (
                        <Chip tone={proposal.status === "held" ? "warn" : "neutral"}>
                          {humanizeEnum(proposal.status)}
                        </Chip>
                      ) : null}
                      <RelativeTime value={proposal.created_at_ms} />
                    </>
                  }
                />
              ))}
            </RowList>
          )}
        </PanelSection>

        {canReviewQuarantine ? (
          <>
            <PanelSection
              label="Waiting for approval"
              count={pendingPromotions.length > 0 ? pendingPromotions.length : null}
            >
              {pendingPromotions.length === 0 ? (
                <EmptyState inline title="No promotion is waiting for approval." />
              ) : (
                <RowList>
                  {pendingPromotions.map((pending) => (
                    <ListRow
                      key={pending.pending_id}
                      className="memory-pending-row"
                      testId={`memory-pending:${pending.pending_id}`}
                      onActivate={() => onSelectRecord(pending.realm, pending.record_id)}
                      titleAttr={`Promotion ${pending.pending_id}`}
                      title={
                        recordTitles.get(pending.record_id) ?? (
                          <TechnicalId value={pending.record_id} copy={false} />
                        )
                      }
                      subtitle={
                        <>
                          {"To "}
                          <ScopeTarget kind={pending.scope_kind} scopeKey={pending.scope_key} />
                        </>
                      }
                      meta={
                        <>
                          {pending.status && pending.status !== "pending" ? (
                            <Chip>{humanizeEnum(pending.status)}</Chip>
                          ) : null}
                          <RelativeTime value={pending.created_at_ms} />
                        </>
                      }
                      actions={
                        onOpenGating ? (
                          <Button
                            size="sm"
                            data-testid={`memory-pipeline-decide:${pending.pending_id}`}
                            onClick={onOpenGating}
                          >
                            Decide in Approvals
                          </Button>
                        ) : undefined
                      }
                    >
                      {pending.rationale ? (
                        <span className="memory-row-note" title={pending.rationale}>
                          {pending.rationale}
                        </span>
                      ) : null}
                    </ListRow>
                  ))}
                </RowList>
              )}
            </PanelSection>
            <PanelSection
              label="Quarantined"
              count={quarantineRecords.length > 0 ? quarantineRecords.length : null}
            >
              {quarantineRecords.length === 0 ? (
                <EmptyState inline title="Nothing is quarantined." />
              ) : (
                <RowList>
                  {quarantineRecords.map((record) => {
                    const reason =
                      record.status.status === "quarantined" ? record.status.reason : undefined;
                    return (
                      <ListRow
                        key={record.id}
                        className="memory-quarantine-row"
                        testId={`memory-quarantine-record:${record.id}`}
                        onActivate={() => onSelectRecord(realmOfRecord(record), record.id)}
                        title={record.title || "Untitled record"}
                        titleAttr={record.title || undefined}
                        subtitle={<RecordFacts record={record} hideStatus />}
                        meta={<RelativeTime value={record.created_at_ms} />}
                      >
                        {reason ? (
                          <span className="memory-row-note" title={reason}>
                            {reason}
                          </span>
                        ) : null}
                      </ListRow>
                    );
                  })}
                </RowList>
              )}
            </PanelSection>
          </>
        ) : (
          <Notice tone="locked" testId="memory-pipeline-no-grant">
            <span title={grantTitle("memory.quarantine.review")}>
              {`Quarantine: no grant. ${needsGrantText("memory.quarantine.review")} to see the queue.`}
            </span>
          </Notice>
        )}

        <PanelSection
          label="Worth a look"
          count={!auditVerdictsDenied && auditVerdicts.length > 0 ? auditVerdicts.length : null}
          testId="memory-review-queue"
        >
          {auditVerdictsDenied ? (
            <EmptyState inline kind="no-access" title="Review queue: no grant." />
          ) : auditVerdicts.length === 0 ? (
            <EmptyState inline title="Nothing is flagged for a closer look." />
          ) : (
            <>
              <p className="memory-caption">Records a dream flagged for a closer look.</p>
              <RowList>
                {auditVerdicts.map((verdict) => (
                  <ListRow
                    key={`${verdict.realm}:${verdict.run_id}:${verdict.record_id}`}
                    className="memory-review-row"
                    testId={`memory-review:${verdict.run_id}:${verdict.record_id}`}
                    title={
                      <RecordLink
                        id={verdict.record_id}
                        realm={verdict.realm}
                        titles={recordTitles}
                        testId={`memory-review-record:${verdict.run_id}:${verdict.record_id}`}
                        onOpen={onSelectRecord}
                      />
                    }
                    subtitle={verdict.rationale}
                    meta={
                      <>
                        {verdict.verdict ? (
                          <Chip tone="warn">{reviewVerdictLabel(verdict.verdict)}</Chip>
                        ) : null}
                        <RelativeTime
                          value={verdict.created_at_ms}
                          prefix="Flagged"
                          title={joinMeta(
                            formatAbsoluteTime(verdict.created_at_ms),
                            `dream run ${verdict.run_id}`,
                          )}
                        />
                      </>
                    }
                  />
                ))}
              </RowList>
            </>
          )}
        </PanelSection>

        <MemoryLiveStrip
          frames={memoryFrames}
          recordTitles={recordTitles}
          onPivot={(realm, recordId) => void onSelectRecord(realm, recordId)}
        />
      </div>
    );
  }

  function renderDreams(): React.JSX.Element {
    return (
      <div className="memory-tab memory-dreams">
        <PanelSection
          label="Dream runs"
          count={!dreamRunsDenied && dreamSheets.length > 0 ? dreamSheets.length : null}
          testId="memory-dream-runs"
        >
          {dreamRunsDenied ? (
            <EmptyState inline kind="no-access" title="Dream runs: no grant." />
          ) : dreamSheets.length === 0 ? (
            <EmptyState
              inline
              title="No dream run is recorded yet."
              body="Older runs may still appear in the audit log below."
            />
          ) : (
            <RowList>
              {dreamSheets.map((run) => (
                <DreamRunRow
                  key={run.run_id}
                  run={run}
                  expanded={expandedRuns[run.run_id] === true}
                  onToggle={() =>
                    setExpandedRuns((current) => ({ ...current, [run.run_id]: !current[run.run_id] }))
                  }
                />
              ))}
            </RowList>
          )}
        </PanelSection>
        <PanelSection
          label="Earlier runs, from the audit log"
          count={dreams.length > 0 ? dreams.length : null}
        >
          {dreams.length === 0 ? (
            <EmptyState
              inline
              kind={dreamsDenied ? "no-access" : "empty"}
              title={dreamsDenied ? "Dream audit: no grant." : "No dream runs recorded yet."}
            />
          ) : (
            <RowList>
              {dreams.map((run) => (
                <AuditRunRow
                  key={run.run_id}
                  run={run}
                  partition={sheetPartitions.get(run.run_id)}
                  recordTitles={recordTitles}
                  onOpenRecord={onSelectRecord}
                />
              ))}
            </RowList>
          )}
        </PanelSection>
      </div>
    );
  }

  function renderRecords(): React.JSX.Element {
    if (detail) {
      return (
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
      );
    }
    if (detailLoading) return <LoadingState label="Loading record" rows={5} />;
    return renderRecordList();
  }

  return (
    <PanelShell className="memory-panel" testId="memory-panel">
      <PanelHeader
        title="Memory"
        subtitle={subtitle}
        divider={false}
        actions={
          <>
            {runtimeControl}
            <Button
              icon={<KitGlyph name="refresh" />}
              data-testid="memory-refresh"
              onClick={() =>
                onRefresh(
                  tab === "records" && detail
                    ? { realm: detail.realm, memoryId: detail.record.id }
                    : undefined,
                )
              }
            >
              Refresh
            </Button>
          </>
        }
      />
      <PanelTabs<MemoryTab> tabs={tabs} active={tab} onSelect={setTab} ariaLabel="Memory views" idBase={tabIds} />
      {/* One-release redirect alias (open Q6): the quarantine tab folded into
          Pipeline; automation addressing memory-tab:quarantine keeps working
          until the next release removes this. Outside the tab list, invisible
          and out of the tab order. */}
      <button
        type="button"
        className="memory-tab-alias"
        onClick={() => setTab(resolveMemoryTabAlias("quarantine") || "pipeline")}
        data-testid="memory-tab:quarantine"
        aria-hidden="true"
        tabIndex={-1}
      >
        Quarantine
      </button>
      {/* The reserved status row sits between the tabs and the body, so a
          refresh error or its recovery never moves either under a click. */}
      <StatusRow error={error} errorTestId="memory-error" updatedAt={updatedAt ?? undefined}>
        {!loaded ? "Loading memory" : undefined}
      </StatusRow>
      <PanelBody className="memory-panel__body" tabPanel={{ idBase: tabIds, tab }}>
        {tab === "holdings" ? renderHoldings() : null}
        {tab === "records" ? renderRecords() : null}
        {tab === "knowledge" ? renderKnowledge() : null}
        {tab === "pipeline" ? renderPipeline() : null}
        {tab === "dreams" ? renderDreams() : null}
      </PanelBody>
    </PanelShell>
  );
}
