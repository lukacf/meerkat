// The activity ledger: what each agent did over the last day, minute by
// minute, its runs as spans, and the messages between each ordered pair of
// agents. Fed with every console frame (before any panel filters them) and
// with history backfills; panels read it through useActivityLedger() for
// sparklines, live state and traffic. Counting reads the shared frame
// classifier, so a frame and its saved history copy count once.

import React from "react";
import { stripAgentKeyPrefix } from "../../../packages/console-core/src/agent-title";
import type { ConsoleAgent, ConsoleFrame } from "../types";
import { classifyFrame, type ClassifiedFrame } from "./frame-classifier";

/** The window the ledger keeps. */
export const LEDGER_WINDOW_MS = 24 * 60 * 60 * 1000;
/** One bucket: a minute. */
export const LEDGER_BUCKET_MS = 60_000;
const MAX_BUCKETS = LEDGER_WINDOW_MS / LEDGER_BUCKET_MS;
const MAX_IDENTITIES = 2_000;
const MAX_PAIRS = 2_000;
const MAX_RUNS_PER_IDENTITY = 500;
const MAX_TOOLS_PER_IDENTITY = 1_000;
const MAX_SEEN_EVENTS = 50_000;
/** One message seen from both ends (the sender's call, the recipient's notice) within this window counts once. */
const MESSAGE_MATCH_MS = 60_000;
const DEFAULT_NOTIFY_MS = 250;

/** What the ledger counts per agent per minute. */
export type LedgerMetric = "turnsStarted" | "turnsFinished" | "failures" | "toolCalls" | "messagesSent" | "events";

export type LedgerCounts = Record<LedgerMetric, number>;

const METRICS: readonly LedgerMetric[] = ["turnsStarted", "turnsFinished", "failures", "toolCalls", "messagesSent", "events"];

function emptyCounts(): LedgerCounts {
  return { turnsStarted: 0, turnsFinished: 0, failures: 0, toolCalls: 0, messagesSent: 0, events: 0 };
}

/** A run of one agent: from run_started to interaction_complete or
 * interaction_failed. Either end may be unseen (before the window, or still
 * running). */
export interface LedgerRunSpan {
  key: string;
  identity: string;
  runId?: string;
  interactionId?: string;
  startMs?: number;
  endMs?: number;
  outcome?: "ok" | "failed" | "cancelled";
  /** Tool calls the agent made while the run was open. */
  toolCalls: number;
}

/** One tool execution, with its duration when the end reports it. */
export interface LedgerToolSpan {
  key: string;
  identity: string;
  startMs?: number;
  endMs?: number;
  durationMs?: number;
  outcome?: "ok" | "failed" | "cancelled" | "timed_out";
}

/** Messages from one agent to another in a window. */
export interface LedgerTraffic {
  from: string;
  to: string;
  count: number;
  lastMs: number;
}

/** What an agent is doing now, as the ledger saw it. */
export interface LedgerLiveState {
  /** The newest run that started and has not ended. */
  openRun: LedgerRunSpan | null;
  /** A tool execution that started and has not ended. */
  openTool: LedgerToolSpan | null;
  /** A model call that started and has not ended. */
  callingModel: boolean;
  /** The newest frame of the agent the ledger counted. */
  lastActivityMs?: number;
}

/** A window of minute bins ending at `endMs`: `bins` bins of `binMs` each. */
export interface LedgerSeriesWindow {
  bins: number;
  binMs?: number;
  endMs?: number;
}

/** A time window: [sinceMs, untilMs]. */
export interface LedgerWindow {
  sinceMs?: number;
  untilMs?: number;
}

/** Resolves a reference (an identity, member id, label, comms name) to an agent's identity. */
export type LedgerResolver = (reference: string) => string | undefined;

export interface ActivityLedgerReader {
  /** Changes with every change the ledger counts. */
  readonly version: number;
  /** Agents the ledger saw act in its window, newest first. */
  identities(): string[];
  /** Minute counts in bins, oldest bin first. A null identity sums every agent. */
  series(identity: string | null, metric: LedgerMetric, window: LedgerSeriesWindow): number[];
  /** Totals in a window. A null identity sums every agent. */
  totals(identity: string | null, window?: LedgerWindow): LedgerCounts;
  liveState(identity: string): LedgerLiveState;
  /** Whether the agent has a run that started and has not ended. */
  hasOpenRun(identity: string): boolean;
  /** Runs that overlap the window, oldest first. */
  runSpans(identity: string | null, window?: LedgerWindow): LedgerRunSpan[];
  /** Tool executions that overlap the window, oldest first. */
  toolSpans(identity: string | null, window?: LedgerWindow): LedgerToolSpan[];
  /** Messages per ordered pair in the window, busiest first. */
  traffic(window?: LedgerWindow): LedgerTraffic[];
}

export interface ActivityLedger extends ActivityLedgerReader {
  /** Count one frame. Returns whether the ledger changed. */
  ingest(frame: ConsoleFrame): boolean;
  /** Count a page of frames (a history backfill), in any order. */
  ingestMany(frames: readonly ConsoleFrame[]): boolean;
  /** Resolve peer references from now on (and in traffic reads) through `resolver`. */
  setResolver(resolver: LedgerResolver | null): void;
  /** Forget everything. */
  reset(): void;
  /** Listen for changes, at most once per notify interval. */
  subscribe(listener: () => void): () => void;
  getVersion(): number;
}

export interface ActivityLedgerOptions {
  now?: () => number;
  /** How long a burst of changes waits before listeners hear of it (0: at once). */
  notifyMs?: number;
}

interface IdentityRecord {
  buckets: Map<number, LedgerCounts>;
  lastMs: number;
  runs: Map<string, LedgerRunSpan>;
  runByInteraction: Map<string, string>;
  tools: Map<string, LedgerToolSpan>;
  openModelCalls: Map<string, number>;
}

interface MessageGroup {
  atMs: number;
  sources: Set<string>;
}

interface PairRecord {
  from: string;
  to: string;
  buckets: Map<number, number>;
  lastMs: number;
  recent: MessageGroup[];
}

type Data = Record<string, unknown>;

function recordOf(value: unknown): Data {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Data : {};
}

function textValue(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function minuteOf(ms: number): number {
  return Math.floor(ms / LEDGER_BUCKET_MS);
}

function lastPathSegment(value: string): string {
  return value.split("/").filter(Boolean).pop() || value;
}

const PEER_TOOLS: ReadonlySet<string> = new Set(["send_message", "send_request", "send_response", "reply_to_peer"]);
const LIFECYCLE_ENDS: ReadonlySet<string> = new Set(["identity_retired", "identity_respawned", "identity_reset", "identity_deleted", "member_retired"]);

function compact(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "");
}

/**
 * A resolver for the console's agents: a reference matches an agent's
 * identity, member, agent or session id, durable identity label or display
 * label, exactly or ignoring case and punctuation, whole or by its last path
 * segment ("team/lead/support-member").
 */
export function createAgentResolver(
  agents: readonly Pick<ConsoleAgent, "identity" | "member_id" | "agent_id" | "session_id" | "label" | "labels">[],
): LedgerResolver {
  const exact = new Map<string, string>();
  const loose = new Map<string, string>();
  for (const agent of agents) {
    const identity = (agent.identity || agent.member_id || agent.agent_id || "").trim();
    if (!identity) continue;
    for (const key of [identity, agent.member_id, agent.agent_id, agent.session_id, agent.labels?.agent_identity, agent.label]) {
      const trimmed = typeof key === "string" ? key.trim() : "";
      if (!trimmed) continue;
      if (!exact.has(trimmed)) exact.set(trimmed, identity);
      for (const form of [trimmed.toLowerCase(), compact(trimmed), compact(stripAgentKeyPrefix(trimmed))]) {
        if (form && !loose.has(form)) loose.set(form, identity);
      }
    }
  }
  return (reference) => {
    const trimmed = reference.trim();
    if (!trimmed) return undefined;
    for (const candidate of [trimmed, lastPathSegment(trimmed)]) {
      const found = exact.get(candidate) ?? loose.get(candidate.toLowerCase()) ?? loose.get(compact(candidate))
        ?? loose.get(compact(stripAgentKeyPrefix(candidate)));
      if (found) return found;
    }
    return undefined;
  };
}

/** The peers a `peers` tool result lists: peer id to name. */
function peersFromResult(data: Data): Array<[string, string]> {
  const candidates: unknown[] = [data.result, data.content];
  const out: Array<[string, string]> = [];
  for (const candidate of candidates) {
    const texts = Array.isArray(candidate)
      ? candidate.map((block) => textValue(recordOf(block).text))
      : [typeof candidate === "string" ? candidate : candidate && typeof candidate === "object" ? JSON.stringify(candidate) : ""];
    for (const text of texts) {
      if (!text) continue;
      try {
        const parsed = recordOf(JSON.parse(text));
        for (const peer of Array.isArray(parsed.peers) ? parsed.peers : []) {
          const record = recordOf(peer);
          const id = textValue(record.peer_id);
          const name = textValue(record.name) || textValue(record.display_name);
          if (id && name) out.push([id, name]);
        }
      } catch {
        // Not a JSON peers list.
      }
    }
  }
  return out;
}

/** A message one frame records, from one of its ends. */
interface MessageFact {
  from: string;
  to: string;
  /** Which end saw it: the sender's call, the sender's or the recipient's notice. */
  source: "call" | "sent-notice" | "received-notice";
}

class Ledger implements ActivityLedger {
  version = 0;
  private identityRecords = new Map<string, IdentityRecord>();
  private pairs = new Map<string, PairRecord>();
  private peerNames = new Map<string, string>();
  private seen = new Set<string>();
  private resolver: LedgerResolver | null = null;
  private listeners = new Set<() => void>();
  private notifyTimer: ReturnType<typeof setTimeout> | null = null;
  /** The version listeners last heard of: what getVersion answers, so a
   * reader re-renders once per notification, not once per frame. */
  private publishedVersion = 0;
  private ingestsSincePrune = 0;
  private readonly now: () => number;
  private readonly notifyMs: number;

  constructor(options: ActivityLedgerOptions = {}) {
    this.now = options.now ?? Date.now;
    this.notifyMs = Math.max(0, options.notifyMs ?? DEFAULT_NOTIFY_MS);
  }

  getVersion = (): number => this.publishedVersion;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  setResolver(resolver: LedgerResolver | null): void {
    this.resolver = resolver;
    this.changed();
  }

  reset(): void {
    this.identityRecords.clear();
    this.pairs.clear();
    this.peerNames.clear();
    this.seen.clear();
    this.changed();
  }

  ingestMany(frames: readonly ConsoleFrame[]): boolean {
    let changed = false;
    for (const frame of frames) changed = this.ingestOne(frame) || changed;
    if (changed) this.changed();
    return changed;
  }

  ingest(frame: ConsoleFrame): boolean {
    const changed = this.ingestOne(frame);
    if (changed) this.changed();
    return changed;
  }

  private ingestOne(frame: ConsoleFrame): boolean {
    const atMs = frame.timestampMs;
    if (typeof atMs !== "number" || !Number.isFinite(atMs)) return false;
    const nowMs = this.now();
    if (atMs < nowMs - LEDGER_WINDOW_MS) return false;
    if (++this.ingestsSincePrune >= 500) {
      this.ingestsSincePrune = 0;
      this.prune(nowMs);
    }
    const data = recordOf(frame.data);
    let changed = this.capturePeers(frame, data);
    const classified = classifyFrame(frame);
    const identity = this.canonical(frame.identity);
    changed = this.applySpan(classified, identity, atMs, frame.event) || changed;
    if (this.seen.has(classified.eventKey)) return changed;
    this.remember(classified.eventKey);
    if (identity) changed = this.count(identity, classified, frame, atMs) || changed;
    for (const message of this.messagesOf(frame, data, classified, identity)) {
      changed = this.recordMessage(message, atMs) || changed;
    }
    return changed;
  }

  private remember(key: string): void {
    this.seen.add(key);
    if (this.seen.size > MAX_SEEN_EVENTS) this.seen.delete(this.seen.values().next().value!);
  }

  private canonical(reference: string | undefined | null): string {
    const trimmed = (reference || "").trim();
    if (!trimmed || trimmed === "_system") return "";
    return this.resolver?.(trimmed) ?? trimmed;
  }

  /** A peer reference as an agent's identity: the peers registry's name for
   * an opaque peer id, then the resolver, else the reference's last segment. */
  private peerIdentity(reference: string): string {
    const trimmed = reference.trim();
    if (!trimmed) return "";
    const named = this.peerNames.get(trimmed) ?? trimmed;
    return this.resolver?.(named) ?? this.resolver?.(lastPathSegment(named)) ?? lastPathSegment(named);
  }

  private record(identity: string, atMs: number): IdentityRecord {
    let record = this.identityRecords.get(identity);
    if (!record) {
      if (this.identityRecords.size >= MAX_IDENTITIES) this.evictOldestIdentity();
      record = { buckets: new Map(), lastMs: atMs, runs: new Map(), runByInteraction: new Map(), tools: new Map(), openModelCalls: new Map() };
      this.identityRecords.set(identity, record);
    }
    if (atMs > record.lastMs) record.lastMs = atMs;
    return record;
  }

  private evictOldestIdentity(): void {
    let oldest: [string, number] | null = null;
    for (const [identity, record] of this.identityRecords) {
      if (!oldest || record.lastMs < oldest[1]) oldest = [identity, record.lastMs];
    }
    if (oldest) this.identityRecords.delete(oldest[0]);
  }

  private capturePeers(frame: ConsoleFrame, data: Data): boolean {
    if (frame.event !== "tool_execution_completed" && frame.event !== "tool_result_received") return false;
    if ((textValue(data.name) || textValue(data.tool_name)) !== "peers") return false;
    let changed = false;
    for (const [id, name] of peersFromResult(data)) {
      if (this.peerNames.get(id) === name) continue;
      this.peerNames.set(id, name);
      changed = true;
    }
    return changed;
  }

  private count(identity: string, classified: ClassifiedFrame, frame: ConsoleFrame, atMs: number): boolean {
    const record = this.record(identity, atMs);
    const minute = minuteOf(atMs);
    let counts = record.buckets.get(minute);
    if (!counts) {
      counts = emptyCounts();
      record.buckets.set(minute, counts);
      if (record.buckets.size > MAX_BUCKETS) {
        record.buckets.delete(Math.min(...record.buckets.keys()));
      }
    }
    let changed = false;
    const bump = (metric: LedgerMetric) => { counts![metric] += 1; changed = true; };
    if (!classified.internal) bump("events");
    if (classified.span?.kind === "run" && classified.span.edge === "start") bump("turnsStarted");
    if (classified.span?.kind === "run" && classified.span.edge === "end" && classified.span.outcome === "ok") bump("turnsFinished");
    if (classified.severity === "crit") bump("failures");
    if (frame.event === "tool_call_requested" || frame.event === "tool_call") {
      bump("toolCalls");
      const open = this.newestOpenRun(record);
      if (open) open.toolCalls += 1;
    }
    return changed;
  }

  private newestOpenRun(record: IdentityRecord): LedgerRunSpan | null {
    let newest: LedgerRunSpan | null = null;
    for (const run of record.runs.values()) {
      if (run.endMs !== undefined || run.startMs === undefined) continue;
      if (!newest || run.startMs > newest.startMs!) newest = run;
    }
    return newest;
  }

  private applySpan(classified: ClassifiedFrame, identity: string, atMs: number, event: string): boolean {
    if (!identity) return false;
    if (LIFECYCLE_ENDS.has(event)) {
      // A retire, respawn, reset or delete ends every run still open.
      const record = this.identityRecords.get(identity);
      let changed = false;
      for (const run of record?.runs.values() ?? []) {
        if (run.endMs !== undefined) continue;
        run.endMs = atMs;
        run.outcome = "cancelled";
        changed = true;
      }
      record?.openModelCalls.clear();
      return changed;
    }
    const span = classified.span;
    if (!span) return false;
    const record = this.record(identity, atMs);
    if (span.kind === "run") {
      let key = span.runId ? `run:${identity}:${span.runId}` : span.key;
      if (!span.runId && span.interactionId) key = record.runByInteraction.get(span.interactionId) ?? key;
      let run = record.runs.get(key);
      if (!run) {
        run = { key, identity, toolCalls: 0 };
        record.runs.set(key, run);
        if (record.runs.size > MAX_RUNS_PER_IDENTITY) this.trimSpans(record.runs, MAX_RUNS_PER_IDENTITY);
      }
      if (span.runId) run.runId = span.runId;
      if (span.interactionId) {
        run.interactionId = span.interactionId;
        record.runByInteraction.set(span.interactionId, key);
      }
      if (span.edge === "start") {
        if (run.startMs === atMs) return false;
        run.startMs = run.startMs === undefined ? atMs : Math.min(run.startMs, atMs);
      } else {
        const outcome: LedgerRunSpan["outcome"] = span.outcome === "timed_out" ? "failed" : span.outcome ?? "ok";
        if (run.endMs === atMs && run.outcome === outcome) return false;
        run.endMs = atMs;
        run.outcome = outcome;
        record.openModelCalls.delete(`turn:${identity}:${span.runId ?? ""}`);
      }
      return true;
    }
    if (span.kind === "tool") {
      let tool = record.tools.get(span.key);
      if (!tool) {
        tool = { key: span.key, identity };
        record.tools.set(span.key, tool);
        if (record.tools.size > MAX_TOOLS_PER_IDENTITY) this.trimSpans(record.tools, MAX_TOOLS_PER_IDENTITY);
      }
      const before = `${tool.startMs}:${tool.endMs}:${tool.outcome}:${tool.durationMs}`;
      if (span.edge === "start") {
        tool.startMs = tool.startMs === undefined ? atMs : Math.min(tool.startMs, atMs);
      } else {
        tool.endMs = atMs;
        tool.outcome = span.outcome;
        if (classified.durationMs !== undefined) {
          tool.durationMs = classified.durationMs;
          tool.startMs ??= atMs - classified.durationMs;
        }
      }
      return before !== `${tool.startMs}:${tool.endMs}:${tool.outcome}:${tool.durationMs}`;
    }
    // A model call: open until its end.
    if (span.edge === "start") {
      if (record.openModelCalls.get(span.key) === atMs) return false;
      record.openModelCalls.set(span.key, atMs);
      return true;
    }
    return record.openModelCalls.delete(span.key);
  }

  /** Drop the oldest spans past `max`: ended ones first, then the oldest open ones. */
  private trimSpans<T extends { startMs?: number; endMs?: number }>(spans: Map<string, T>, max: number): void {
    const ordered = [...spans.entries()].sort(([, a], [, b]) => (a.endMs ?? a.startMs ?? 0) - (b.endMs ?? b.startMs ?? 0));
    for (const [key, span] of ordered) {
      if (spans.size <= max) return;
      if (span.endMs === undefined && span.startMs !== undefined) continue;
      spans.delete(key);
    }
    for (const [key] of ordered) {
      if (spans.size <= max) return;
      spans.delete(key);
    }
  }

  private messagesOf(frame: ConsoleFrame, data: Data, classified: ClassifiedFrame, identity: string): MessageFact[] {
    if (!identity) return [];
    if (frame.event === "tool_call_requested" || frame.event === "tool_call") {
      const name = textValue(data.name) || textValue(data.tool_name);
      if (!PEER_TOOLS.has(name)) return [];
      const args = recordOf(data.args);
      const reference = textValue(args.display_name) || textValue(args.to) || textValue(args.identity)
        || textValue(args.target_identity) || textValue(args.recipient) || textValue(args.peer_id);
      const to = reference ? this.peerIdentity(reference) : "";
      return to && to !== identity ? [{ from: identity, to, source: "call" }] : [];
    }
    if (frame.event !== "system_notice" || classified.family !== "message" || classified.internal) return [];
    const blocks: Data[] = [];
    for (const candidate of [data, recordOf(data.message)]) {
      if (textValue(candidate.type) === "comms") blocks.push(candidate);
      for (const block of Array.isArray(candidate.blocks) ? candidate.blocks : []) {
        const record = recordOf(block);
        if (textValue(record.type) === "comms") blocks.push(record);
      }
    }
    const facts: MessageFact[] = [];
    for (const block of blocks) {
      const kind = textValue(block.kind);
      if (kind === "lifecycle" || kind === "response_progress") continue;
      const peer = recordOf(block.peer);
      const reference = textValue(peer.display_name) || textValue(peer.id);
      const other = reference ? this.peerIdentity(reference) : "";
      if (!other || other === identity) continue;
      const outgoing = textValue(block.direction) === "outgoing";
      facts.push(outgoing
        ? { from: identity, to: other, source: "sent-notice" }
        : { from: other, to: identity, source: "received-notice" });
    }
    return facts;
  }

  private recordMessage(message: MessageFact, atMs: number): boolean {
    const pairKey = `${message.from}\u0000${message.to}`;
    let pair = this.pairs.get(pairKey);
    if (!pair) {
      if (this.pairs.size >= MAX_PAIRS) this.evictOldestPair();
      pair = { from: message.from, to: message.to, buckets: new Map(), lastMs: atMs, recent: [] };
      this.pairs.set(pairKey, pair);
    }
    // The other end of a message already counted joins it instead of counting again.
    const match = pair.recent
      .filter((group) => !group.sources.has(message.source) && Math.abs(group.atMs - atMs) <= MESSAGE_MATCH_MS)
      .sort((a, b) => a.atMs - b.atMs)[0];
    if (match) {
      match.sources.add(message.source);
      return false;
    }
    pair.recent.push({ atMs, sources: new Set([message.source]) });
    pair.recent = pair.recent.filter((group) => Math.abs(group.atMs - atMs) <= MESSAGE_MATCH_MS * 4).slice(-64);
    const minute = minuteOf(atMs);
    pair.buckets.set(minute, (pair.buckets.get(minute) ?? 0) + 1);
    if (pair.buckets.size > MAX_BUCKETS) pair.buckets.delete(Math.min(...pair.buckets.keys()));
    if (atMs > pair.lastMs) pair.lastMs = atMs;
    const sender = this.record(message.from, atMs);
    let counts = sender.buckets.get(minute);
    if (!counts) {
      counts = emptyCounts();
      sender.buckets.set(minute, counts);
    }
    counts.messagesSent += 1;
    return true;
  }

  private evictOldestPair(): void {
    let oldest: [string, number] | null = null;
    for (const [key, pair] of this.pairs) {
      if (!oldest || pair.lastMs < oldest[1]) oldest = [key, pair.lastMs];
    }
    if (oldest) this.pairs.delete(oldest[0]);
  }

  private prune(nowMs: number): void {
    const oldestMinute = minuteOf(nowMs - LEDGER_WINDOW_MS);
    for (const [identity, record] of this.identityRecords) {
      for (const minute of record.buckets.keys()) if (minute < oldestMinute) record.buckets.delete(minute);
      for (const [key, run] of record.runs) {
        if ((run.endMs ?? run.startMs ?? 0) < nowMs - LEDGER_WINDOW_MS) record.runs.delete(key);
      }
      for (const [key, tool] of record.tools) {
        if ((tool.endMs ?? tool.startMs ?? 0) < nowMs - LEDGER_WINDOW_MS) record.tools.delete(key);
      }
      if (record.buckets.size === 0 && record.runs.size === 0 && record.lastMs < nowMs - LEDGER_WINDOW_MS) {
        this.identityRecords.delete(identity);
      }
    }
    for (const [key, pair] of this.pairs) {
      for (const minute of pair.buckets.keys()) if (minute < oldestMinute) pair.buckets.delete(minute);
      if (pair.buckets.size === 0) this.pairs.delete(key);
    }
  }

  private changed(): void {
    this.version += 1;
    if (this.listeners.size === 0 || this.notifyMs === 0) {
      this.publish();
      return;
    }
    if (this.notifyTimer !== null) return;
    this.notifyTimer = setTimeout(() => {
      this.notifyTimer = null;
      this.publish();
    }, this.notifyMs);
  }

  private publish(): void {
    if (this.publishedVersion === this.version) return;
    this.publishedVersion = this.version;
    for (const listener of [...this.listeners]) listener();
  }

  // ── Reads ──────────────────────────────────────────────────────────

  identities(): string[] {
    return [...this.identityRecords.entries()]
      .sort(([, a], [, b]) => b.lastMs - a.lastMs)
      .map(([identity]) => this.canonical(identity) || identity)
      .filter((identity, index, all) => all.indexOf(identity) === index);
  }

  /** The records an identity's reads cover: its own, and any kept under another name that now resolves to it. */
  private recordsFor(identity: string | null): IdentityRecord[] {
    if (identity === null) return [...this.identityRecords.values()];
    const canonical = this.canonical(identity) || identity;
    const out: IdentityRecord[] = [];
    for (const [key, record] of this.identityRecords) {
      if (key === identity || key === canonical || this.canonical(key) === canonical) out.push(record);
    }
    return out;
  }

  series(identity: string | null, metric: LedgerMetric, window: LedgerSeriesWindow): number[] {
    const bins = Math.max(0, Math.floor(window.bins));
    const binMs = Math.max(LEDGER_BUCKET_MS, window.binMs ?? LEDGER_BUCKET_MS);
    const endMs = window.endMs ?? this.now();
    const out = new Array<number>(bins).fill(0);
    if (bins === 0) return out;
    const startMs = endMs - bins * binMs;
    for (const record of this.recordsFor(identity)) {
      for (const [minute, counts] of record.buckets) {
        const atMs = minute * LEDGER_BUCKET_MS;
        if (atMs < startMs || atMs >= endMs) continue;
        const index = Math.min(bins - 1, Math.floor((atMs - startMs) / binMs));
        out[index] += counts[metric];
      }
    }
    return out;
  }

  totals(identity: string | null, window: LedgerWindow = {}): LedgerCounts {
    const sinceMs = window.sinceMs ?? Number.NEGATIVE_INFINITY;
    const untilMs = window.untilMs ?? Number.POSITIVE_INFINITY;
    const out = emptyCounts();
    for (const record of this.recordsFor(identity)) {
      for (const [minute, counts] of record.buckets) {
        const atMs = minute * LEDGER_BUCKET_MS;
        if (atMs < minuteOf(sinceMs) * LEDGER_BUCKET_MS || atMs > untilMs) continue;
        for (const metric of METRICS) out[metric] += counts[metric];
      }
    }
    return out;
  }

  liveState(identity: string): LedgerLiveState {
    let openRun: LedgerRunSpan | null = null;
    let openTool: LedgerToolSpan | null = null;
    let callingModel = false;
    let lastActivityMs: number | undefined;
    for (const record of this.recordsFor(identity)) {
      const run = this.newestOpenRun(record);
      if (run && (!openRun || run.startMs! > openRun.startMs!)) openRun = run;
      for (const tool of record.tools.values()) {
        if (tool.endMs !== undefined || tool.startMs === undefined) continue;
        if (!openTool || tool.startMs > openTool.startMs!) openTool = tool;
      }
      if (record.openModelCalls.size > 0) callingModel = true;
      if (record.lastMs && (lastActivityMs === undefined || record.lastMs > lastActivityMs)) lastActivityMs = record.lastMs;
    }
    return { openRun: openRun ? { ...openRun } : null, openTool: openTool ? { ...openTool } : null, callingModel, ...(lastActivityMs !== undefined ? { lastActivityMs } : {}) };
  }

  hasOpenRun(identity: string): boolean {
    return this.recordsFor(identity).some((record) => this.newestOpenRun(record) !== null);
  }

  runSpans(identity: string | null, window: LedgerWindow = {}): LedgerRunSpan[] {
    return this.spansIn(identity, window, (record) => record.runs.values());
  }

  toolSpans(identity: string | null, window: LedgerWindow = {}): LedgerToolSpan[] {
    return this.spansIn(identity, window, (record) => record.tools.values());
  }

  private spansIn<T extends { identity: string; startMs?: number; endMs?: number }>(
    identity: string | null,
    window: LedgerWindow,
    spansOf: (record: IdentityRecord) => Iterable<T>,
  ): T[] {
    const sinceMs = window.sinceMs ?? Number.NEGATIVE_INFINITY;
    const untilMs = window.untilMs ?? Number.POSITIVE_INFINITY;
    const out: T[] = [];
    for (const record of this.recordsFor(identity)) {
      for (const span of spansOf(record)) {
        const start = span.startMs ?? span.endMs ?? 0;
        const end = span.endMs ?? Number.POSITIVE_INFINITY;
        if (end < sinceMs || start > untilMs) continue;
        out.push({ ...span, identity: this.canonical(span.identity) || span.identity });
      }
    }
    return out.sort((a, b) => (a.startMs ?? a.endMs ?? 0) - (b.startMs ?? b.endMs ?? 0));
  }

  traffic(window: LedgerWindow = {}): LedgerTraffic[] {
    const sinceMinute = minuteOf(window.sinceMs ?? Number.MIN_SAFE_INTEGER);
    const untilMs = window.untilMs ?? Number.POSITIVE_INFINITY;
    const merged = new Map<string, LedgerTraffic>();
    for (const pair of this.pairs.values()) {
      let count = 0;
      let lastMs = 0;
      for (const [minute, n] of pair.buckets) {
        if (minute < sinceMinute || minute * LEDGER_BUCKET_MS > untilMs) continue;
        count += n;
        lastMs = Math.max(lastMs, minute * LEDGER_BUCKET_MS);
      }
      if (count === 0) continue;
      // A name that resolves now merges with the pair it belongs to.
      const from = this.canonical(pair.from) || pair.from;
      const to = this.canonical(pair.to) || pair.to;
      if (from === to) continue;
      const key = `${from}\u0000${to}`;
      const known = merged.get(key);
      const pairLast = Math.min(pair.lastMs, untilMs);
      if (known) {
        known.count += count;
        known.lastMs = Math.max(known.lastMs, pairLast);
      } else {
        merged.set(key, { from, to, count, lastMs: Math.max(lastMs, pairLast) });
      }
    }
    return [...merged.values()].sort((a, b) => b.count - a.count || b.lastMs - a.lastMs);
  }
}

export function createActivityLedger(options: ActivityLedgerOptions = {}): ActivityLedger {
  return new Ledger(options);
}

// ── React ─────────────────────────────────────────────────────────────

const EMPTY_LEDGER: ActivityLedger = createActivityLedger({ notifyMs: 0 });
const ActivityLedgerContext = React.createContext<ActivityLedger>(EMPTY_LEDGER);

/** Hands the console's ledger to the panels below. */
export function ActivityLedgerProvider({ ledger, children }: { ledger: ActivityLedger; children?: React.ReactNode }): React.JSX.Element {
  return React.createElement(ActivityLedgerContext.Provider, { value: ledger }, children);
}

/** What a panel reads: the ledger's selectors, and the clock its series end at. */
export interface ActivityLedgerView extends ActivityLedgerReader {
  nowMs: number;
}

export interface UseActivityLedgerOptions {
  /** Re-read on this interval too, so series slide with time (ms). */
  tickMs?: number;
}

/**
 * The console's activity ledger. Re-renders the caller when the ledger
 * changes (at most every notify interval, 250 ms) and, with `tickMs`, on
 * that clock. Without a provider it reads an empty ledger.
 */
export function useActivityLedger(options: UseActivityLedgerOptions = {}): ActivityLedgerView {
  const ledger = React.useContext(ActivityLedgerContext);
  const version = React.useSyncExternalStore(ledger.subscribe, ledger.getVersion, ledger.getVersion);
  const [nowMs, setNowMs] = React.useState(() => Date.now());
  const tickMs = options.tickMs;
  React.useEffect(() => {
    if (!tickMs || tickMs <= 0) return undefined;
    const timer = setInterval(() => setNowMs(Date.now()), tickMs);
    return () => clearInterval(timer);
  }, [tickMs]);
  return React.useMemo<ActivityLedgerView>(() => ({
    version,
    nowMs: Math.max(nowMs, Date.now()),
    identities: () => ledger.identities(),
    series: (identity, metric, window) => ledger.series(identity, metric, window),
    totals: (identity, window) => ledger.totals(identity, window),
    liveState: (identity) => ledger.liveState(identity),
    hasOpenRun: (identity) => ledger.hasOpenRun(identity),
    runSpans: (identity, window) => ledger.runSpans(identity, window),
    toolSpans: (identity, window) => ledger.toolSpans(identity, window),
    traffic: (window) => ledger.traffic(window),
  }), [ledger, version, nowMs]);
}
