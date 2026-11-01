// The Memory panel's data model: tabs, scope grouping, the records filter
// and pager, the recall utility, the trust rules walk, the six checks, the
// store overview, the injection ledger, dream sheets, the biography and the
// live strip. Pure module (no React), exercised by MemoryPanel.test.ts.

import type { ConversationTimelineEntry } from "@console-core";
import { jsonRpcErrorCode } from "../../lib/errors";
import {
  formatRelativeTime,
  joinMeta,
  plural,
} from "../../../../packages/console-components/src/panel-kit/format";
import type { Tone } from "../../../../packages/console-components/src/panel-kit/tone";
import type {
  ConsoleFrame,
  MemoryAuditVerdictEntry,
  MemoryDreamRun,
  MemoryDreamRunDetail,
  MemoryDreamRunSheet,
  MemoryFullRecord,
  MemoryInjectionEntry,
  MemoryLedgerEntry,
  MemoryPanelRecord,
  MemoryPanelRecordsResult,
  MemoryRecordScope,
  MemoryScopeOverview,
} from "../../types";
import { TRUST_RANK, formatSize, overviewScopeName, trustLabel } from "./words";

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
  /// Realm name: the listing reads that realm's records only.
  realm?: string;
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

// ── Pure data helpers ─────────────────────────────────────────────────────



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
/// "no access", never an empty store), -32601 means the panel is not wired on
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
  if (isCapabilityMiss(error)) return "denied";
  return "error";
}

/// Whether the headless layer refused the call because meta/capabilities
/// does not list its method.
export function isCapabilityMiss(error: unknown): boolean {
  return error instanceof Error && error.message.startsWith(CAPABILITY_MISS_PREFIX);
}

/// Classify a failure of the base records listing, the read that decides
/// whether the panel has a store at all. A per-section capability miss is
/// denial by intersection (memorySectionOutcome), but a runtime with no
/// memory store advertises none of the panel methods: when the experience
/// does not report a store, a missing records method means memory is not
/// set up here, never "no access". The server's -32601 reads the same.
export function memoryListingOutcome(
  error: unknown,
  storeAvailable: boolean | undefined,
): "denied" | "unavailable" | "error" {
  const outcome = memorySectionOutcome(error);
  if (outcome === "denied" && storeAvailable !== true && isCapabilityMiss(error)) return "unavailable";
  return outcome;
}

/// Whether the experience says this runtime has no memory store, so the
/// panel shows "not set up" without reading anything.
export function memoryNotSetUp(memory: { available?: boolean } | null | undefined): boolean {
  return memory?.available === false;
}

// ── Records list view + fetch orchestration ───────────────────────────────

export interface MemoryPagedState {
  records: MemoryPanelRecord[];
  nextCursor: string | null;
  /// The query behind this page (or its load-more continuation) was denied
  /// (-32030). Renders "no access": a denied scope must never be
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
        const first = await deps.query(buildRecordsQueryParams(next, { resolveKey: deps.resolveKey }));
        if (seq !== seqCounter) return;
        const result = await skipEmptyPages(
          deps.query,
          first,
          (cursor) => buildRecordsQueryParams(next, { cursor, resolveKey: deps.resolveKey }),
          () => seq === seqCounter,
        );
        if (result === "stale" || seq !== seqCounter) return;
        if (result === null) {
          // Denied filtered query: mark it so the view says "no access",
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
        const first = await deps.query(
          buildRecordsQueryParams(current.filter, { cursor, resolveKey: deps.resolveKey }),
        );
        if (seq !== seqCounter) return;
        const result = await skipEmptyPages(
          deps.query,
          first,
          (next) => buildRecordsQueryParams(current.filter, { cursor: next, resolveKey: deps.resolveKey }),
          () => seq === seqCounter,
        );
        if (result === "stale" || seq !== seqCounter) return;
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
  const ratio = u.ratio === null ? "-" : u.ratio.toFixed(2);
  return `inj ${u.injected} · recall ${u.recalled} · useful ${u.useful} · ratio ${ratio} · ~${formatBytes(u.bytesSpent)} spent`;
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0B";
  if (bytes < 1024) return `${Math.round(bytes)}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)}MB`;
}

/// A record's usefulness in words, for the least-useful-first list.
export function utilityDisplay(record: Pick<MemoryPanelRecord, "usage" | "body_bytes">): string {
  const utility = recordUtility(record);
  if (utility.injected === 0 && utility.recalled === 0) return "Never given to agents or recalled";
  return joinMeta(
    `Given to agents ${plural(utility.injected, "time")}`,
    `recalled ${plural(utility.recalled, "time")}`,
    `judged useful ${plural(utility.useful, "time")}`,
    utility.bytesSpent > 0 ? `about ${formatSize(utility.bytesSpent)} spent` : null,
  );
}


// ── Lattice audit (§5 LATTICE, invariants a + c) ──────────────────────────

/// Client page-walk ceiling (open Q4): 2,000 rows at 200/page is ten requests
/// against the fetch-on-demand plane. Walks past the cap stay honest via the
/// "checked N" partial header instead of masquerading as complete.
export const LATTICE_WALK_MAX_RECORDS = 2000;
export const LATTICE_WALK_PAGE_LIMIT = 200;
/// Pages the walk may read beyond the ones the cap needs, for short pages.
export const LATTICE_WALK_EXTRA_PAGES = 10;
/// Empty pages that still carry a cursor that one "Load more" (or one
/// filter) follows before it stops and shows what it has: a restricted
/// caller can read empty pages while the store holds more rows past them.
export const EMPTY_PAGE_HOPS = 4;

/// Follow the cursor past empty pages, up to EMPTY_PAGE_HOPS more reads.
/// Stops early (returns null) when the caller's sequence moved on.
async function skipEmptyPages(
  query: (params: Record<string, unknown>) => Promise<MemoryPanelRecordsResult | null>,
  first: MemoryPanelRecordsResult | null,
  paramsFor: (cursor: string) => Record<string, unknown>,
  current: () => boolean,
): Promise<MemoryPanelRecordsResult | null | "stale"> {
  let result = first;
  let hops = 0;
  while (result && (result.records?.length ?? 0) === 0 && result.next_cursor && hops < EMPTY_PAGE_HOPS) {
    hops += 1;
    const next = await query(paramsFor(result.next_cursor));
    if (!current()) return "stale";
    result = next;
  }
  return result;
}

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
  /// The records the walk read (up to the cap): the landscape's ticks and
  /// the Records search reach past the first page through them.
  records?: MemoryPanelRecord[];
}

/// The one way an LLM-authored record rises above observed trust: the
/// memory steward re-tiers it to verified, which the store allows only for a
/// record carrying a verification claim and never for one that was once
/// quarantined (the transitive taint ceiling). Such a record keeps its
/// original author, so the check must not count it.
export function isStewardVerified(
  record: Pick<MemoryPanelRecord, "trust" | "provenance" | "ever_quarantined">,
): boolean {
  return (
    record.trust === "agent_verified" &&
    record.ever_quarantined !== true &&
    Boolean(record.provenance?.verification)
  );
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
      rank > TRUST_RANK.agent_observed &&
      !isStewardVerified(record)
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
/// cursor directly; multi-realm gateways walk each realm in turn through the
/// server-supported `realm` param. A merged multi-realm page carries a cursor
/// as well (one position in the order every realm shares, which resumes them
/// all), so the per-realm walk is a choice of order, not a need. `complete` =
/// every realm exhausted its cursor within the shared cap.
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
  // Pages a restricted caller reads can come back short or empty while
  // still carrying a cursor (the server pages before it filters rows), so
  // the walk also stops after a bounded number of pages, as incomplete.
  const maxPages = Math.ceil(max / LATTICE_WALK_PAGE_LIMIT) + LATTICE_WALK_EXTRA_PAGES;
  let pages = 0;
  for (const realm of realmParams) {
    let cursor: string | undefined;
    for (;;) {
      if (isCancelled()) return null;
      if (all.length >= max || pages >= maxPages) {
        exhaustedEverywhere = false;
        break;
      }
      pages += 1;
      const params: Record<string, unknown> = { limit: LATTICE_WALK_PAGE_LIMIT };
      if (realm) params.realm = realm;
      if (cursor) params.cursor = cursor;
      const page = await fetchPage(params);
      if (page === null) return null; // access denied → tile renders no access
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
    records: checked,
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
  "no-grant": "NO ACCESS",
};

export function verdictStatusLabel(status: VerdictStatus): string {
  return VERDICT_STATUS_LABEL[status];
}

/// The tone of a verdict: holding ok, degraded warn, violated crit. A check
/// that cannot run (unverifiable, no access) is locked, never green.
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
  /// Access withheld some realm's dream runs (withheld_by_access).
  dreamsWithheld?: boolean;
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

/// What a reader whose access withholds dream runs is told.
export const DREAMS_WITHHELD = "Dream history needs access to all memory in this space.";

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
    label: "Repeated memory",
    status: "unverifiable",
    summary: "Not checked automatically",
    lines: [
      "An agent should not be given the same memory again and again.",
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
    "Records written by agents stay at observed trust or below, unless the memory steward verified them, and replacement chains have no loops or gaps.";
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

  const recallScope = `A record given to agents at least ${DEAD_INJECTION_THRESHOLD} times without ever being judged useful counts as never useful; it only costs space.`;
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
          : "No record is given to agents without helping",
      lines: [
        recallScope,
        `${dead.length} of ${plural(loaded, "loaded record")} ${dead.length === 1 ? "is" : "are"} never useful.`,
        ...(dead.length > 0 ? [`About ${formatSize(deadBytes)} spent giving them to agents.`] : []),
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
  } else if (inputs.dreams.length === 0 && inputs.dreamsWithheld) {
    // Access withheld the runs: never say no dream has run.
    tiles.push({
      id: "dreams",
      label: "Dream runs",
      status: "no-grant",
      summary: "Dream history is withheld by access",
      lines: [dreamsScope, DREAMS_WITHHELD],
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
        ...(inputs.dreamsWithheld ? ["Some spaces' dream runs are withheld by access."] : []),
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

/// The same rule for any list keyed by scope kind (proposals, promotions
/// waiting for approval): these reads are not row filtered on the server,
/// so the console at least never shows rows of a scope kind its probes
/// proved unreadable. The server must filter them too (it is not the only
/// guard).
export function visibleForScopeKinds<T extends { scope_kind: string }>(
  items: T[],
  denied: { operatorScopeDenied?: boolean; mobScopeDenied?: boolean },
): T[] {
  if (!denied.operatorScopeDenied && !denied.mobScopeDenied) return items;
  return items.filter((item) => {
    if (item.scope_kind === "operator" && denied.operatorScopeDenied) return false;
    if (item.scope_kind === "mob" && denied.mobScopeDenied) return false;
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

/// One open audit verdict per record: every dream run re-flags a record it
/// still doubts, so the queue keeps the newest verdict for each record and
/// counts how many runs flagged it. Newest first.
export interface DedupedAuditVerdict {
  verdict: MemoryAuditVerdictEntry;
  /// Runs that flagged this record, the kept one included.
  flags: number;
}

export function dedupeAuditVerdicts(verdicts: MemoryAuditVerdictEntry[]): DedupedAuditVerdict[] {
  const byRecord = new Map<string, DedupedAuditVerdict>();
  for (const verdict of verdicts) {
    const key = `${verdict.realm}\u0000${verdict.record_id}`;
    const kept = byRecord.get(key);
    if (!kept) {
      byRecord.set(key, { verdict, flags: 1 });
      continue;
    }
    kept.flags += 1;
    if ((verdict.created_at_ms ?? 0) > (kept.verdict.created_at_ms ?? 0)) kept.verdict = verdict;
  }
  return Array.from(byRecord.values()).sort(
    (a, b) => (b.verdict.created_at_ms ?? 0) - (a.verdict.created_at_ms ?? 0),
  );
}

// ── Phase-2: durable dream verdict sheets (§3.5) ──────────────────────────

export function dreamRunsNewestFirst(runs: MemoryDreamRunSheet[]): MemoryDreamRunSheet[] {
  return [...runs].sort(
    (a, b) =>
      (b.completed_at_ms ?? b.started_at_ms ?? 0) - (a.completed_at_ms ?? a.started_at_ms ?? 0),
  );
}

export function formatDurationMs(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "-";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60 * 1000) return `${(ms / 1000).toFixed(1)}s`;
  const minutes = Math.floor(ms / (60 * 1000));
  const seconds = Math.round((ms % (60 * 1000)) / 1000);
  return `${minutes}m ${seconds}s`;
}

export function dreamRunDuration(
  run: Pick<MemoryDreamRunSheet, "started_at_ms" | "completed_at_ms">,
): string {
  if (!run.started_at_ms || !run.completed_at_ms) return "-";
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

/// Display order for the version history: oldest first, as the server's
/// chain arrives and as the versions graph reads left to right, with the
/// open record marked. Versions that replace the open record follow it;
/// they never sit above it as if they came first.
export function lineageLane(
  chain: MemoryPanelRecord[],
  currentId: string,
): Array<{ record: MemoryPanelRecord; current: boolean }> {
  return chain.map((record) => ({
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
