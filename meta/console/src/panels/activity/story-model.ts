// Today's story and day lanes (console v2 spec B4), built from the shared
// activity entries and the activity ledger: the day as rows people scan
// (a turn as one row, a run of one agent's quiet events folded into one),
// grouped by hour, and the lanes, spans, connectors and marks the
// Swimlanes chart draws. Pure module (no React).

import type { FrameFamily, FrameSeverity } from "../../lib/frame-classifier";
import type { LedgerRunSpan, LedgerToolSpan } from "../../lib/activity-ledger";
import type { Tone } from "../../../../packages/console-components/src/panel-kit/tone";
import { entryMatches, isProblem, severityTone, strongerSeverity, type ActivityEntry } from "./activity-model";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

// ── Windows ───────────────────────────────────────────────────────────

export type TodayView = "day" | "3h" | "1h";

export interface TimeWindow {
  start: number;
  end: number;
}

/** Local midnight of the day `nowMs` falls in. */
export function startOfLocalDay(nowMs: number): number {
  const day = new Date(nowMs);
  day.setHours(0, 0, 0, 0);
  return day.getTime();
}

/** The time a Today view covers: the day from local midnight, or the last 3 hours or hour. */
export function todayWindow(view: TodayView, nowMs: number): TimeWindow {
  if (view === "1h") return { start: nowMs - HOUR, end: nowMs };
  if (view === "3h") return { start: nowMs - 3 * HOUR, end: nowMs };
  return { start: startOfLocalDay(nowMs), end: nowMs };
}

/** The bins a view's trends count in: about 12 to 24 of a nice size (5 or
 * 15 minutes, an hour), so a day that began an hour ago still reads as a
 * trend. */
export function todayBins(view: TodayView, window: TimeWindow): { bins: number; binMs: number } {
  const span = Math.max(MINUTE, window.end - window.start);
  const steps = [5 * MINUTE, 10 * MINUTE, 15 * MINUTE, 30 * MINUTE, HOUR, 2 * HOUR];
  const wanted = view === "day" ? span / 24 : span / 12;
  const binMs = steps.find((step) => step >= wanted) ?? 2 * HOUR;
  return { bins: Math.max(1, Math.ceil(span / binMs)), binMs };
}

export function inWindow(atMs: number, window: TimeWindow): boolean {
  return atMs >= window.start && atMs <= window.end;
}

// ── Filters ───────────────────────────────────────────────────────────

export type StoryFilter = "all" | "problems" | "approvals" | "messages" | "memory" | "lifecycle";

export const STORY_FILTERS: ReadonlyArray<{ id: StoryFilter; label: string }> = [
  { id: "all", label: "All" },
  { id: "problems", label: "Problems" },
  { id: "approvals", label: "Approvals" },
  { id: "messages", label: "Messages" },
  { id: "memory", label: "Memory" },
  { id: "lifecycle", label: "Lifecycle" },
];

const FILTER_FAMILY: Partial<Record<StoryFilter, FrameFamily>> = {
  approvals: "approval",
  messages: "message",
  memory: "memory",
  lifecycle: "lifecycle",
};

export function entryPassesFilter(entry: ActivityEntry, filter: StoryFilter): boolean {
  if (filter === "all") return true;
  if (filter === "problems") return isProblem(entry);
  return entry.family === FILTER_FAMILY[filter];
}

/** How many entries each filter keeps. */
export function filterCounts(entries: readonly ActivityEntry[]): Record<StoryFilter, number> {
  const counts: Record<StoryFilter, number> = { all: 0, problems: 0, approvals: 0, messages: 0, memory: 0, lifecycle: 0 };
  for (const entry of entries) {
    counts.all += 1;
    for (const filter of ["problems", "approvals", "messages", "memory", "lifecycle"] as const) {
      if (entryPassesFilter(entry, filter)) counts[filter] += 1;
    }
  }
  return counts;
}

// ── Story rows ────────────────────────────────────────────────────────

/** One event on its own. */
export interface StoryEvent {
  kind: "event";
  key: string;
  entry: ActivityEntry;
  actor: string | null;
  atMs: number;
  severity: FrameSeverity;
}

/** One turn of an agent: its request, what it did and how it ended. */
export interface StoryTurn {
  kind: "turn";
  key: string;
  actor: string;
  interactionId: string;
  /** Oldest first. */
  entries: ActivityEntry[];
  request?: ActivityEntry;
  outcome?: ActivityEntry;
  startMs: number;
  /** The turn's latest event. */
  atMs: number;
  severity: FrameSeverity;
}

/** A run of one agent's quiet rows within an hour, folded into one. */
export interface StoryFold {
  kind: "fold";
  key: string;
  actor: string;
  /** Newest first. */
  rows: Array<StoryEvent | StoryTurn>;
  startMs: number;
  atMs: number;
  severity: FrameSeverity;
}

export type StoryRow = StoryEvent | StoryTurn | StoryFold;

export interface StoryHour {
  key: string;
  startMs: number;
  /** Newest first. */
  rows: StoryRow[];
  /** Events in the hour. */
  count: number;
}

/** Families that belong to the turn they happen in. */
const TURN_FAMILIES: ReadonlySet<FrameFamily> = new Set(["turn", "tool", "message", "workgraph"]);
/** Fold runs of at least this many quiet rows of one agent. */
const FOLD_MIN = 3;

/** The entries of a row, oldest first. */
export function rowEntries(row: StoryRow): ActivityEntry[] {
  if (row.kind === "event") return [row.entry];
  if (row.kind === "turn") return row.entries;
  return row.rows.slice().reverse().flatMap(rowEntries);
}

function eventRow(entry: ActivityEntry): StoryEvent {
  return { kind: "event", key: `event:${entry.key}`, entry, actor: entry.actor, atMs: entry.atMs, severity: entry.severity };
}

function turnRow(actor: string, interactionId: string, entries: ActivityEntry[]): StoryTurn | StoryEvent {
  const ordered = entries.slice().sort((a, b) => a.atMs - b.atMs);
  if (ordered.length === 1) return eventRow(ordered[0]);
  const request = ordered.find((entry) => entry.frame.event === "user_input" || entry.frame.event === "interaction_started");
  const outcome = [...ordered].reverse().find((entry) => entry.span?.kind === "run" && entry.span.edge === "end");
  let severity: FrameSeverity = "info";
  for (const entry of ordered) severity = strongerSeverity(severity, entry.severity);
  return {
    kind: "turn",
    key: `turn:${actor}:${interactionId}`,
    actor,
    interactionId,
    entries: ordered,
    ...(request ? { request } : {}),
    ...(outcome ? { outcome } : {}),
    startMs: ordered[0].atMs,
    atMs: ordered[ordered.length - 1].atMs,
    severity,
  };
}

function rowActor(row: StoryEvent | StoryTurn): string | null {
  return row.actor;
}

/** Whether a row may fold into its agent's run of quiet rows. */
function foldable(row: StoryEvent | StoryTurn): boolean {
  if (row.severity !== "info") return false;
  if (row.kind === "event" && (row.entry.family === "approval" || row.entry.family === "lifecycle")) return false;
  return rowActor(row) !== null;
}

function foldRuns(rows: Array<StoryEvent | StoryTurn>): StoryRow[] {
  const out: StoryRow[] = [];
  let index = 0;
  while (index < rows.length) {
    const row = rows[index];
    const actor = rowActor(row);
    if (!foldable(row) || actor === null) {
      out.push(row);
      index += 1;
      continue;
    }
    let end = index + 1;
    while (end < rows.length && foldable(rows[end]) && rowActor(rows[end]) === actor) end += 1;
    const run = rows.slice(index, end);
    if (run.length >= FOLD_MIN) {
      out.push({
        kind: "fold",
        key: `fold:${actor}:${run[run.length - 1].key}`,
        actor,
        rows: run,
        startMs: Math.min(...run.map((item) => (item.kind === "turn" ? item.startMs : item.atMs))),
        atMs: Math.max(...run.map((item) => item.atMs)),
        severity: "info",
      });
    } else {
      out.push(...run);
    }
    index = end;
  }
  return out;
}

/** The start of the local hour a time falls in. */
export function startOfLocalHour(atMs: number): number {
  const hour = new Date(atMs);
  hour.setMinutes(0, 0, 0);
  return hour.getTime();
}

export interface StoryOptions {
  filter?: StoryFilter;
  query?: string;
  nameOf?: (agent: string) => string;
}

/**
 * The story: the entries that pass the filter and search, a turn's events
 * as one row (its request, its tool calls and messages, its end), the rest
 * one row each, grouped by local hour (newest first), with runs of one
 * agent's quiet rows folded. Problems, approvals and lifecycle events never
 * fold away.
 */
export function buildStory(entries: readonly ActivityEntry[], options: StoryOptions = {}): StoryHour[] {
  const filter = options.filter ?? "all";
  const nameOf = options.nameOf ?? ((agent: string) => agent);
  const kept = entries.filter((entry) => entryPassesFilter(entry, filter)
    && (!options.query || entryMatches(entry, options.query, nameOf)));
  const turns = new Map<string, { actor: string; interactionId: string; entries: ActivityEntry[] }>();
  const rows: Array<StoryEvent | StoryTurn> = [];
  for (const entry of kept) {
    if (entry.actor && entry.interactionId && TURN_FAMILIES.has(entry.family)) {
      const key = `${entry.actor}\u0000${entry.interactionId}`;
      const turn = turns.get(key);
      if (turn) turn.entries.push(entry);
      else turns.set(key, { actor: entry.actor, interactionId: entry.interactionId, entries: [entry] });
      continue;
    }
    rows.push(eventRow(entry));
  }
  for (const turn of turns.values()) rows.push(turnRow(turn.actor, turn.interactionId, turn.entries));
  rows.sort((a, b) => b.atMs - a.atMs || (a.key < b.key ? -1 : 1));
  const hours = new Map<number, Array<StoryEvent | StoryTurn>>();
  for (const row of rows) {
    const hour = startOfLocalHour(row.atMs);
    const list = hours.get(hour);
    if (list) list.push(row);
    else hours.set(hour, [row]);
  }
  return [...hours.entries()]
    .sort(([a], [b]) => b - a)
    .map(([startMs, hourRows]) => ({
      key: `hour:${startMs}`,
      startMs,
      rows: foldRuns(hourRows),
      count: hourRows.reduce((sum, row) => sum + (row.kind === "turn" ? row.entries.length : 1), 0),
    }));
}

/** Every row of a story, hours flattened, newest first. */
export function storyRows(hours: readonly StoryHour[]): StoryRow[] {
  return hours.flatMap((hour) => hour.rows);
}

// ── Words for rows ────────────────────────────────────────────────────

function countWords(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

/**
 * What a turn did besides its request and its end: "messaged Billing 13
 * times", "used 2 tools", "1 tool timed out". Agents by `nameOf`.
 */
export function turnFacts(turn: StoryTurn, nameOf: (agent: string) => string): string[] {
  const facts: string[] = [];
  const messages = turn.entries.filter((entry) => entry.family === "message" && entry.actor === turn.actor);
  if (messages.length) {
    const targets = [...new Set(messages.map((entry) => entry.target).filter((target): target is string => Boolean(target)))];
    if (targets.length === 1) {
      facts.push(messages.length === 1 ? `messaged ${nameOf(targets[0])}` : `messaged ${nameOf(targets[0])} ${messages.length} times`);
    } else if (targets.length > 1) {
      facts.push(`sent ${countWords(messages.length, "message")} to ${countWords(targets.length, "agent")}`);
    } else {
      facts.push(`sent ${countWords(messages.length, "message")}`);
    }
  }
  const tools = turn.entries.filter((entry) => (entry.family === "tool" || entry.family === "workgraph") && entry.severity === "info");
  if (tools.length) facts.push(`used ${countWords(tools.length, "tool")}`);
  const problems = turn.entries.filter((entry) => entry !== turn.outcome && isProblem(entry));
  for (const problem of problems.slice(0, 2)) facts.push(problem.title.toLowerCase());
  if (problems.length > 2) facts.push(`${problems.length - 2} more problems`);
  return facts;
}

/** How a turn ended, in words: "replied", "failed", "was stopped", "still working". */
export function turnOutcomeWords(turn: StoryTurn): { text: string; tone: Tone } | null {
  const outcome = turn.outcome;
  if (!outcome) return turn.request ? { text: "no reply yet", tone: "neutral" } : null;
  const result = outcome.span?.outcome;
  if (result === "failed") return { text: outcome.title === "Turn dropped" ? "dropped" : "failed", tone: outcome.tone };
  if (result === "cancelled") return { text: outcome.title.replace(/^Turn /, "").toLowerCase(), tone: "neutral" };
  return { text: "replied", tone: "ok" };
}

/** A fold's summary after the agent's name: "13 messages to Billing · 2 turns". */
export function foldFacts(fold: StoryFold, nameOf: (agent: string) => string): string[] {
  const entries = fold.rows.flatMap(rowEntries);
  const facts: string[] = [];
  const turns = fold.rows.filter((row) => row.kind === "turn"
    || (row.entry.span?.kind === "run" && row.entry.span.edge === "end")).length;
  if (turns) facts.push(countWords(turns, "turn"));
  const messages = entries.filter((entry) => entry.family === "message" && entry.actor === fold.actor);
  if (messages.length) {
    const targets = [...new Set(messages.map((entry) => entry.target).filter((target): target is string => Boolean(target)))];
    facts.push(targets.length === 1
      ? `${countWords(messages.length, "message")} to ${nameOf(targets[0])}`
      : `${countWords(messages.length, "message")}${targets.length ? ` to ${countWords(targets.length, "agent")}` : ""}`);
  }
  const tools = entries.filter((entry) => entry.family === "tool" || entry.family === "workgraph").length;
  if (tools) facts.push(countWords(tools, "tool call"));
  const memory = entries.filter((entry) => entry.family === "memory").length;
  if (memory) facts.push(countWords(memory, "memory event"));
  if (!facts.length) facts.push(countWords(entries.length, "event"));
  return facts;
}

// ── Lanes ─────────────────────────────────────────────────────────────

export const QUIETER_LANE = "__quieter";
export const PROBLEMS_LANE = "__problems";

export interface LaneSpec {
  id: string;
  /** The agent's key (null for the folded lane). */
  agent: string | null;
  label: string;
  live?: boolean;
  /** The folded lane of quieter agents: the agents it holds. */
  agents?: string[];
}

export type SpanState = "done" | "live" | "failed" | "cancelled";

export interface SpanSpec {
  id: string;
  lane: string;
  agent: string;
  start: number;
  /** Open while the turn runs. */
  end?: number;
  state: SpanState;
  /** Tool call times inside the span. */
  ticks: number[];
  runId?: string;
  interactionId?: string;
  label: string;
}

export interface LinkSpec {
  id: string;
  from: string;
  to: string;
  at: number;
  label: string;
  entryKey: string;
}

export type MarkShape = "diamond" | "triangle" | "hollow-diamond" | "square" | "dot";

export interface MarkSpec {
  id: string;
  lane: string;
  at: number;
  shape: MarkShape;
  tone: Tone;
  label: string;
  entryKey: string;
}

export interface LaneModel {
  lanes: LaneSpec[];
  spans: SpanSpec[];
  links: LinkSpec[];
  marks: MarkSpec[];
  /** The PROBLEMS lane's marks. */
  problems: MarkSpec[];
  /** Agents in the folded lane. */
  quieter: string[];
}

export interface LaneModelInput {
  entries: readonly ActivityEntry[];
  runs: readonly LedgerRunSpan[];
  tools: readonly LedgerToolSpan[];
  window: TimeWindow;
  nameOf: (agent: string) => string;
  /** Agents working now (open turns), for the lane's live mark. */
  isLive?: (agent: string) => boolean;
  /** At most this many lanes before the folded one (12). */
  maxLanes?: number;
  /** Agents whose lanes show even when quiet (a selection). */
  pinned?: readonly string[];
}

function spanState(run: LedgerRunSpan): SpanState {
  if (run.endMs === undefined) return "live";
  if (run.outcome === "failed") return "failed";
  if (run.outcome === "cancelled") return "cancelled";
  return "done";
}

const SPAN_WORDS: Readonly<Record<SpanState, string>> = {
  done: "finished",
  live: "working now",
  failed: "failed",
  cancelled: "stopped",
};

function clock(atMs: number): string {
  const date = new Date(atMs);
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/**
 * The Swimlanes chart's model for a window: one lane per agent, the busiest
 * first (`maxLanes`, then one folded lane of ticks for the quieter ones),
 * turns as spans with their tool calls as ticks, messages as connectors
 * between lanes, approvals and lifecycle events as marks, and the problems
 * lined up on top.
 */
export function buildLaneModel(input: LaneModelInput): LaneModel {
  const { window, nameOf } = input;
  const maxLanes = Math.max(1, input.maxLanes ?? 12);
  const entries = input.entries.filter((entry) => inWindow(entry.atMs, window));
  const runs = input.runs.filter((run) => {
    const start = run.startMs ?? run.endMs ?? 0;
    const end = run.endMs ?? Number.POSITIVE_INFINITY;
    return end >= window.start && start <= window.end;
  });
  const activity = new Map<string, number>();
  const bump = (agent: string | null | undefined, by = 1) => {
    if (agent) activity.set(agent, (activity.get(agent) ?? 0) + by);
  };
  for (const entry of entries) bump(entry.actor);
  for (const run of runs) bump(run.identity, 2);
  for (const agent of input.pinned ?? []) if (!activity.has(agent)) activity.set(agent, 0);
  const ranked = [...activity.entries()]
    .sort(([a, x], [b, y]) => y - x || nameOf(a).localeCompare(nameOf(b)))
    .map(([agent]) => agent);
  const pinned = new Set(input.pinned ?? []);
  const shown = ranked.length > maxLanes + 1
    ? [...ranked.slice(0, maxLanes), ...ranked.slice(maxLanes).filter((agent) => pinned.has(agent))]
    : ranked;
  const shownSet = new Set(shown);
  const quieter = ranked.filter((agent) => !shownSet.has(agent));
  const laneOf = (agent: string | null | undefined): string | null => {
    if (!agent) return null;
    if (shownSet.has(agent)) return agent;
    return quieter.includes(agent) ? QUIETER_LANE : null;
  };
  const lanes: LaneSpec[] = shown.map((agent) => ({
    id: agent,
    agent,
    label: nameOf(agent),
    ...(input.isLive?.(agent) ? { live: true } : {}),
  }));
  if (quieter.length) {
    lanes.push({ id: QUIETER_LANE, agent: null, label: `+${quieter.length} quieter ${quieter.length === 1 ? "agent" : "agents"}`, agents: quieter });
  }

  const toolsByAgent = new Map<string, number[]>();
  for (const tool of input.tools) {
    const at = tool.startMs ?? tool.endMs;
    if (at === undefined || !inWindow(at, window)) continue;
    const list = toolsByAgent.get(tool.identity);
    if (list) list.push(at);
    else toolsByAgent.set(tool.identity, [at]);
  }
  const spans: SpanSpec[] = [];
  for (const run of runs) {
    const lane = laneOf(run.identity);
    if (!lane) continue;
    const state = spanState(run);
    const start = Math.max(window.start, run.startMs ?? run.endMs ?? window.start);
    const end = run.endMs === undefined ? undefined : Math.min(window.end, run.endMs);
    const until = end ?? window.end;
    const ticks = (toolsByAgent.get(run.identity) ?? []).filter((at) => at >= start && at <= until);
    const name = nameOf(run.identity);
    const duration = end !== undefined && run.startMs !== undefined ? ` · ${Math.max(1, Math.round((end - run.startMs) / 1000))} s` : "";
    spans.push({
      id: run.runId || run.key,
      lane,
      agent: run.identity,
      start,
      ...(end !== undefined ? { end } : {}),
      state,
      ticks,
      ...(run.runId ? { runId: run.runId } : {}),
      ...(run.interactionId ? { interactionId: run.interactionId } : {}),
      label: `${name}'s turn, ${clock(start)}${end !== undefined ? `-${clock(end)}` : ""}${duration}, ${SPAN_WORDS[state]}${ticks.length ? `, ${countWords(ticks.length, "tool call")}` : ""}`,
    });
  }

  const links: LinkSpec[] = [];
  const marks: MarkSpec[] = [];
  const problems: MarkSpec[] = [];
  for (const entry of entries) {
    if (entry.family === "message" && entry.actor && entry.target) {
      const from = laneOf(entry.actor);
      const to = laneOf(entry.target);
      if (from && to && from !== to) {
        links.push({
          id: `link:${entry.key}`,
          from,
          to,
          at: entry.atMs,
          label: `${nameOf(entry.actor)} to ${nameOf(entry.target)}, ${clock(entry.atMs)}${entry.snippet ? `: ${entry.snippet}` : ""}`,
          entryKey: entry.key,
        });
      }
    }
    const lane = laneOf(entry.actor);
    if (lane && entry.family === "approval") {
      marks.push({ id: `mark:${entry.key}`, lane, at: entry.atMs, shape: "hollow-diamond", tone: entry.tone === "crit" ? "crit" : entry.tone, label: `${entry.title}, ${clock(entry.atMs)}`, entryKey: entry.key });
    } else if (lane && entry.family === "lifecycle") {
      marks.push({ id: `mark:${entry.key}`, lane, at: entry.atMs, shape: "square", tone: entry.tone, label: `${nameOf(entry.actor ?? "")}: ${entry.title.toLowerCase()}, ${clock(entry.atMs)}`, entryKey: entry.key });
    }
    if (isProblem(entry)) {
      problems.push({
        id: `problem:${entry.key}`,
        lane: PROBLEMS_LANE,
        at: entry.atMs,
        shape: entry.severity === "crit" ? "diamond" : "triangle",
        tone: severityTone(entry.severity),
        label: `${entry.title}${entry.actor ? `, ${nameOf(entry.actor)}` : ""}, ${clock(entry.atMs)}`,
        entryKey: entry.key,
      });
    }
  }
  return { lanes, spans, links, marks, problems, quieter };
}
