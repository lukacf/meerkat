// The Memory panel's pictures as data: where each record sits on the 30-day
// landscape, records per day and injections per hour, the day groups of the
// Records timeline, the kind clusters, the pipeline's stages, the dream runs
// of the last week and the injection dots of the Knowledge tab. Pure module
// (no React): the components draw what these return, and the node tests
// check the placement.

import type {
  MemoryDreamRun,
  MemoryDreamRunSheet,
  MemoryLedgerEntry,
  MemoryPanelRecord,
  MemoryRecordKind,
  MemoryScopeOverview,
  MemoryTrust,
} from "../../types";
import { normalizeDreamRunDetail, scopeGroupKey } from "./model";

export const HOUR_MS = 60 * 60 * 1000;
export const DAY_MS = 24 * HOUR_MS;
/** The landscape's window: the last 30 days. */
export const LANDSCAPE_DAYS = 30;
/** The records the console reads for the landscape: the base page plus the
 * trust rules walk, which stops at 2000. */
export const LANDSCAPE_MAX_RECORDS = 2000;
/** Ticks one scope row draws at most; the rest of a dense row is sampled
 * (quarantined ticks always stay). */
export const LANDSCAPE_MAX_TICKS_PER_ROW = 480;

/** The kinds in the order people meet them (Clusters, the Kind filter). */
export const MEMORY_KINDS: readonly MemoryRecordKind[] = [
  "fact",
  "preference",
  "procedure",
  "gotcha",
  "relationship",
  "open_loop",
  "reference",
];

/** Trust on a five-step ladder: untrusted 1 up to operator 5. */
export function trustLevel(trust: MemoryTrust | undefined): number {
  switch (trust) {
    case "untrusted":
      return 1;
    case "agent_observed":
      return 2;
    case "agent_verified":
      return 3;
    case "application":
      return 4;
    case "operator":
      return 5;
    default:
      return 0;
  }
}

/** Every record the console has read, once each, in first-seen order. */
export function mergeReadRecords(
  ...lists: Array<ReadonlyArray<MemoryPanelRecord> | null | undefined>
): MemoryPanelRecord[] {
  const seen = new Set<string>();
  const merged: MemoryPanelRecord[] = [];
  for (const list of lists) {
    for (const record of list ?? []) {
      if (!record?.id || seen.has(record.id)) continue;
      seen.add(record.id);
      merged.push(record);
    }
  }
  return merged;
}

/** When a record was written: created, else last updated. */
export function recordWrittenAt(record: Pick<MemoryPanelRecord, "created_at_ms" | "updated_at_ms">): number | null {
  const at = record.created_at_ms || record.updated_at_ms;
  return typeof at === "number" && Number.isFinite(at) && at > 0 ? at : null;
}

/** When a record last changed: updated, else created. */
export function recordChangedAt(record: Pick<MemoryPanelRecord, "created_at_ms" | "updated_at_ms">): number | null {
  const at = record.updated_at_ms || record.created_at_ms;
  return typeof at === "number" && Number.isFinite(at) && at > 0 ? at : null;
}

/** How a record's tick reads: ink while active, a warning triangle while
 * quarantined, faint once replaced or discarded. */
export type TickShape = "active" | "quarantined" | "faint";

export function tickShape(record: Pick<MemoryPanelRecord, "status">): TickShape {
  switch (record.status?.status) {
    case "quarantined":
      return "quarantined";
    case "superseded":
    case "tombstoned":
      return "faint";
    default:
      return "active";
  }
}

export interface LandscapeTick {
  id: string;
  realm: string;
  title: string;
  /** Position on the row's axis, 0 (30 days ago) to 1 (now). */
  x: number;
  at: number;
  shape: TickShape;
}

export interface LandscapeLane {
  key: string;
  ticks: LandscapeTick[];
  /** Records written before the window opened. */
  earlier: number;
  /** Ticks left out to keep a dense row drawable. */
  sampled: number;
}

export interface LandscapeWindow {
  start: number;
  end: number;
}

export function landscapeWindow(now: number, days = LANDSCAPE_DAYS): LandscapeWindow {
  return { start: now - days * DAY_MS, end: now };
}

/** Keep every quarantined tick and an even sample of the rest. */
function sampleTicks(ticks: LandscapeTick[], max: number): { ticks: LandscapeTick[]; sampled: number } {
  if (ticks.length <= max) return { ticks, sampled: 0 };
  const keep = ticks.filter((tick) => tick.shape === "quarantined");
  const rest = ticks.filter((tick) => tick.shape !== "quarantined");
  const room = Math.max(0, max - keep.length);
  const step = rest.length / Math.max(1, room);
  const picked: LandscapeTick[] = [];
  for (let index = 0; index < room && Math.floor(index * step) < rest.length; index += 1) {
    picked.push(rest[Math.floor(index * step)]);
  }
  const ticksOut = [...keep, ...picked].sort((a, b) => a.x - b.x || a.id.localeCompare(b.id));
  return { ticks: ticksOut, sampled: ticks.length - ticksOut.length };
}

/**
 * The landscape's rows: for every scope, one tick per record at the time it
 * was written, placed on the window's axis. Records older than the window
 * count as `earlier`; undated records draw nothing. Keys match the scope
 * rows (`scopeGroupKey`, `overviewScopeKey`).
 */
export function landscapeLanes(
  records: ReadonlyArray<MemoryPanelRecord>,
  window: LandscapeWindow,
  maxTicksPerRow = LANDSCAPE_MAX_TICKS_PER_ROW,
): Map<string, LandscapeLane> {
  const span = Math.max(1, window.end - window.start);
  const lanes = new Map<string, LandscapeLane>();
  for (const record of records) {
    if (!record?.scope) continue;
    const key = scopeGroupKey(record.scope);
    let lane = lanes.get(key);
    if (!lane) {
      lane = { key, ticks: [], earlier: 0, sampled: 0 };
      lanes.set(key, lane);
    }
    const at = recordWrittenAt(record);
    if (at === null) continue;
    if (at < window.start) {
      lane.earlier += 1;
      continue;
    }
    lane.ticks.push({
      id: record.id,
      realm: record.scope.realm,
      title: record.title || "Untitled record",
      x: Math.min(1, Math.max(0, (at - window.start) / span)),
      at,
      shape: tickShape(record),
    });
  }
  for (const lane of lanes.values()) {
    lane.ticks.sort((a, b) => a.x - b.x || a.id.localeCompare(b.id));
    const sampled = sampleTicks(lane.ticks, maxTicksPerRow);
    lane.ticks = sampled.ticks;
    lane.sampled = sampled.sampled;
  }
  return lanes;
}

/** Counts per bin of `size` ms, oldest first, the last bin ending at `now`. */
export function binCounts(times: ReadonlyArray<number>, now: number, bins: number, size: number): number[] {
  const counts = new Array<number>(bins).fill(0);
  const start = now - bins * size;
  for (const at of times) {
    if (!Number.isFinite(at) || at < start || at > now) continue;
    const index = Math.min(bins - 1, Math.floor((at - start) / size));
    counts[index] += 1;
  }
  return counts;
}

/** How many times fall inside the last `span` ms. */
export function countSince(times: ReadonlyArray<number>, now: number, span: number): number {
  return times.filter((at) => Number.isFinite(at) && at >= now - span && at <= now).length;
}

/** Records written per day over the last `days` days, oldest first. */
export function recordsPerDay(records: ReadonlyArray<MemoryPanelRecord>, now: number, days = LANDSCAPE_DAYS): number[] {
  const times = records.map(recordWrittenAt).filter((at): at is number => at !== null);
  return binCounts(times, now, days, DAY_MS);
}

/** Memory given to agents per hour over the last day, oldest first. */
export function injectionsPerHour(injections: ReadonlyArray<Pick<MemoryLedgerEntry, "at_ms">>, now: number, hours = 24): number[] {
  return binCounts(injections.map((entry) => entry.at_ms), now, hours, HOUR_MS);
}

function localDayKey(at: number): string {
  const date = new Date(at);
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function startOfLocalDay(at: number): number {
  const date = new Date(at);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

/** A day heading: "Today", "Yesterday", then "Mon 27 Oct" (with the year
 * when it differs from now's). */
export function dayHeading(at: number, now: number): string {
  const today = startOfLocalDay(now);
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  const day = startOfLocalDay(at);
  if (day === today) return "Today";
  if (day === yesterday.getTime()) return "Yesterday";
  const date = new Date(at);
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  const weekday = date.toLocaleDateString("en-GB", { weekday: "short" });
  const dayMonth = date.toLocaleDateString("en-GB", { day: "numeric", month: "short" });
  return sameYear ? `${weekday} ${dayMonth}` : `${weekday} ${dayMonth} ${date.getFullYear()}`;
}

export interface RecordDayGroup {
  key: string;
  label: string;
  records: MemoryPanelRecord[];
}

/** Records by the local day they last changed, in the order given (the
 * listing's newest-first order). Undated records close the list. */
export function recordsByDay(records: ReadonlyArray<MemoryPanelRecord>, now: number): RecordDayGroup[] {
  const groups: RecordDayGroup[] = [];
  const byKey = new Map<string, RecordDayGroup>();
  let undated: RecordDayGroup | null = null;
  for (const record of records) {
    const at = recordChangedAt(record);
    if (at === null) {
      undated ??= { key: "undated", label: "Date unknown", records: [] };
      undated.records.push(record);
      continue;
    }
    const key = localDayKey(at);
    let group = byKey.get(key);
    if (!group) {
      group = { key, label: dayHeading(at, now), records: [] };
      byKey.set(key, group);
      groups.push(group);
    }
    group.records.push(record);
  }
  if (undated) groups.push(undated);
  return groups;
}

/** Supersede links between two records that are both on screen: newer
 * replaces older. */
export function supersedeLinks(records: ReadonlyArray<MemoryPanelRecord>): Array<{ newer: string; older: string }> {
  const shown = new Set(records.map((record) => record.id));
  const links: Array<{ newer: string; older: string }> = [];
  for (const record of records) {
    if (record.supersedes && shown.has(record.supersedes) && record.supersedes !== record.id) {
      links.push({ newer: record.id, older: record.supersedes });
    }
  }
  return links;
}

/** How often a record was used: given to agents, recalled, judged useful. */
export function recordUses(record: Pick<MemoryPanelRecord, "usage">): number {
  const usage = record.usage;
  return (usage?.injected_count ?? 0) + (usage?.explicit_recall_count ?? 0) + (usage?.judged_useful_count ?? 0);
}

export interface KindCluster {
  kind: MemoryRecordKind;
  count: number;
  /** Records per trust level, untrusted (index 0) to operator (index 4). */
  trust: [number, number, number, number, number];
  quarantined: number;
  /** The three most used records, most recent first on ties. */
  top: Array<{ id: string; realm: string; title: string; uses: number }>;
}

/** One cluster per kind, in MEMORY_KINDS order, over the given records. */
export function kindClusters(records: ReadonlyArray<MemoryPanelRecord>): KindCluster[] {
  return MEMORY_KINDS.map((kind) => {
    const members = records.filter((record) => record.kind === kind);
    const trust: [number, number, number, number, number] = [0, 0, 0, 0, 0];
    for (const record of members) {
      const level = trustLevel(record.trust);
      if (level >= 1) trust[level - 1] += 1;
    }
    const top = [...members]
      .sort((a, b) => recordUses(b) - recordUses(a) || (recordChangedAt(b) ?? 0) - (recordChangedAt(a) ?? 0))
      .slice(0, 3)
      .map((record) => ({
        id: record.id,
        realm: record.scope.realm,
        title: record.title || "Untitled record",
        uses: recordUses(record),
      }));
    return {
      kind,
      count: members.length,
      trust,
      quarantined: members.filter((record) => record.status?.status === "quarantined").length,
      top,
    };
  });
}

/** Store totals over the visible overview scopes. */
export interface StoreTotals {
  active: number;
  quarantined: number;
  superseded: number;
  tombstoned: number;
  total: number;
  bytes: number;
  nearLimit: number;
  /** Active records in team (mob) scopes. */
  team: number;
}

export function storeTotals(scopes: ReadonlyArray<MemoryScopeOverview>): StoreTotals {
  const totals: StoreTotals = { active: 0, quarantined: 0, superseded: 0, tombstoned: 0, total: 0, bytes: 0, nearLimit: 0, team: 0 };
  for (const scope of scopes) {
    totals.active += scope.active ?? 0;
    totals.quarantined += scope.quarantined ?? 0;
    totals.superseded += scope.superseded ?? 0;
    totals.tombstoned += scope.tombstoned ?? 0;
    totals.bytes += scope.body_bytes ?? 0;
    if (scope.floor_pressure === true) totals.nearLimit += 1;
    if (scope.scope_kind === "mob") totals.team += scope.active ?? 0;
  }
  totals.total = totals.active + totals.quarantined + totals.superseded + totals.tombstoned;
  return totals;
}

/** The same totals counted over loaded records, for runtimes without
 * store totals. */
export function loadedTotals(records: ReadonlyArray<MemoryPanelRecord>): StoreTotals {
  const totals: StoreTotals = { active: 0, quarantined: 0, superseded: 0, tombstoned: 0, total: 0, bytes: 0, nearLimit: 0, team: 0 };
  for (const record of records) {
    const status = record.status?.status;
    if (status === "active") totals.active += 1;
    else if (status === "quarantined") totals.quarantined += 1;
    else if (status === "superseded") totals.superseded += 1;
    else if (status === "tombstoned") totals.tombstoned += 1;
    totals.bytes += record.body_bytes ?? 0;
    if (record.scope?.scope === "mob" && status === "active") totals.team += 1;
  }
  totals.total = records.length;
  return totals;
}

/** What the steward's dream runs decided, summed over the given sheets. */
export interface DreamDecisions {
  accepted: number;
  held: number;
  rejected: number;
  released: number;
  discarded: number;
  gated: number;
}

export function dreamDecisions(sheets: ReadonlyArray<MemoryDreamRunSheet>): DreamDecisions {
  const totals: DreamDecisions = { accepted: 0, held: 0, rejected: 0, released: 0, discarded: 0, gated: 0 };
  for (const sheet of sheets) {
    const detail = normalizeDreamRunDetail(sheet.detail);
    for (const [name, count] of detail.verdicts) {
      switch (name) {
        case "proposals_accepted":
          totals.accepted += count;
          break;
        case "proposals_held":
          totals.held += count;
          break;
        case "proposals_rejected":
          totals.rejected += count;
          break;
        case "quarantine_released":
          totals.released += count;
          break;
        case "quarantine_tombstoned":
          totals.discarded += count;
          break;
        case "proposals_gated":
        case "quarantine_gated":
          totals.gated += count;
          break;
        default:
          break;
      }
    }
  }
  return totals;
}

export interface DreamBar {
  runId: string;
  realm: string;
  partition?: string;
  start: number;
  end: number;
  /** 0-1 positions on the week's axis. */
  x0: number;
  x1: number;
  ops: number | null;
  quarantined: number;
  /** A durable verdict sheet exists for this run (it opens its sheet). */
  sheet: boolean;
}

export interface DreamRow {
  realm: string;
  bars: DreamBar[];
}

/**
 * The dream runs of the last `days` days, one row per realm: a bar per run
 * from its first change (or start) to its last change (or completion), with
 * the changes it committed and the ones it quarantined. Sheets and audit
 * runs of the same run id merge into one bar.
 */
export function dreamTimeline(
  sheets: ReadonlyArray<MemoryDreamRunSheet>,
  runs: ReadonlyArray<MemoryDreamRun>,
  now: number,
  days = 7,
): { start: number; end: number; rows: DreamRow[]; older: number } {
  const start = now - days * DAY_MS;
  const span = Math.max(1, now - start);
  const byRun = new Map<string, { realm: string; partition?: string; start?: number; end?: number; ops: number | null; quarantined: number; sheet: boolean }>();
  for (const sheet of sheets) {
    byRun.set(sheet.run_id, {
      realm: sheet.realm,
      partition: sheet.partition,
      start: sheet.started_at_ms,
      end: sheet.completed_at_ms,
      ops: typeof sheet.ops_committed === "number" ? sheet.ops_committed : null,
      quarantined: 0,
      sheet: true,
    });
  }
  for (const run of runs) {
    const merged = byRun.get(run.run_id);
    const first = run.first_op_at_ms;
    const last = run.last_op_at_ms;
    if (merged) {
      merged.start = Math.min(merged.start ?? Infinity, first ?? Infinity);
      merged.end = Math.max(merged.end ?? 0, last ?? 0) || merged.end;
      if (merged.ops === null && typeof run.ops === "number") merged.ops = run.ops;
      merged.quarantined = run.quarantined_ops ?? 0;
    } else {
      byRun.set(run.run_id, {
        realm: run.realm,
        start: first,
        end: last,
        ops: typeof run.ops === "number" ? run.ops : null,
        quarantined: run.quarantined_ops ?? 0,
        sheet: false,
      });
    }
  }
  const rows = new Map<string, DreamRow>();
  let older = 0;
  for (const [runId, run] of byRun) {
    const runStart = Number.isFinite(run.start ?? NaN) ? (run.start as number) : run.end;
    const runEnd = Number.isFinite(run.end ?? NaN) && (run.end as number) > 0 ? (run.end as number) : runStart;
    if (!runStart || !runEnd) continue;
    if (runEnd < start) {
      older += 1;
      continue;
    }
    let row = rows.get(run.realm);
    if (!row) {
      row = { realm: run.realm, bars: [] };
      rows.set(run.realm, row);
    }
    row.bars.push({
      runId,
      realm: run.realm,
      partition: run.partition,
      start: runStart,
      end: Math.max(runStart, runEnd),
      x0: Math.max(0, (runStart - start) / span),
      x1: Math.min(1, (Math.max(runStart, runEnd) - start) / span),
      ops: run.ops,
      quarantined: run.quarantined,
      sheet: run.sheet,
    });
  }
  const ordered = Array.from(rows.values())
    .map((row) => ({ ...row, bars: row.bars.sort((a, b) => a.start - b.start) }))
    .sort((a, b) => a.realm.localeCompare(b.realm));
  return { start, end: now, rows: ordered, older };
}

export interface InjectionDot {
  key: string;
  recordId: string;
  realm: string;
  identity: string;
  at: number;
  x: number;
  /** Given as the agent started (hollow) or during a turn (filled). */
  surface: "build" | "turn";
}

/** The memory given to one agent over the last `days` days, as dots on a
 * week's axis, oldest first. */
export function injectionDots(
  injections: ReadonlyArray<MemoryLedgerEntry>,
  identity: string,
  now: number,
  days = 7,
): InjectionDot[] {
  const start = now - days * DAY_MS;
  const span = Math.max(1, now - start);
  return injections
    .map((entry, index) => ({ entry, index }))
    .filter(({ entry }) => entry.identity === identity && entry.at_ms >= start && entry.at_ms <= now)
    .map(({ entry, index }) => ({
      key: `${index}`,
      recordId: entry.record_id,
      realm: entry.realm,
      identity: entry.identity,
      at: entry.at_ms,
      x: (entry.at_ms - start) / span,
      surface: entry.surface === "build" ? ("build" as const) : ("turn" as const),
    }))
    .sort((a, b) => a.at - b.at);
}

/** Highlight ranges of every occurrence of `query` in `text`, case-insensitive. */
export function matchRanges(text: string, query: string): Array<[number, number]> {
  const needle = query.trim().toLowerCase();
  if (!needle || !text) return [];
  const haystack = text.toLowerCase();
  const ranges: Array<[number, number]> = [];
  let from = 0;
  while (from <= haystack.length) {
    const at = haystack.indexOf(needle, from);
    if (at < 0) break;
    ranges.push([at, at + needle.length]);
    from = at + needle.length;
  }
  return ranges;
}

/** Whether a record matches a text search over its title, description and tags. */
export function recordMatchesText(
  record: Pick<MemoryPanelRecord, "title" | "description" | "tags">,
  query: string,
): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return [record.title, record.description, ...(record.tags ?? [])].some(
    (part) => typeof part === "string" && part.toLowerCase().includes(needle),
  );
}

/** The Records tab's client-side refinements over loaded records. */
export interface RecordsRefinement {
  text: string;
  kinds: ReadonlyArray<MemoryRecordKind>;
  /** Trust levels 1-5 to keep. */
  trust: ReadonlyArray<number>;
}

export const NO_REFINEMENT: RecordsRefinement = { text: "", kinds: [], trust: [] };

export function hasRefinement(refinement: RecordsRefinement): boolean {
  return Boolean(refinement.text.trim()) || refinement.kinds.length > 0 || refinement.trust.length > 0;
}

export function refineRecords(
  records: ReadonlyArray<MemoryPanelRecord>,
  refinement: RecordsRefinement,
): MemoryPanelRecord[] {
  if (!hasRefinement(refinement)) return [...records];
  return records.filter(
    (record) =>
      recordMatchesText(record, refinement.text) &&
      (refinement.kinds.length === 0 || refinement.kinds.includes(record.kind)) &&
      (refinement.trust.length === 0 || refinement.trust.includes(trustLevel(record.trust))),
  );
}
