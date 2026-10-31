// The activity ledger: what each agent did over the last day, minute by
// minute, its runs as spans, and the messages between each ordered pair of
// agents. Fed with every console frame (before any panel filters them) and
// with history backfills; panels read it through useActivityLedger() for
// sparklines, live state and traffic. Counting reads the shared frame
// classifier, so a frame and its saved history copy count once, and a
// message is one message however many of its ends the console sees: the
// sender's call, its receipt (the envelope id) and the recipient's notice
// (the same id as its delivery id).

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
const MAX_MESSAGES = 20_000;
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

/** One message between agents, as the ledger knows it so far. An end is a
 * console identity (`resolved`) or a peer's reference (a comms name or an
 * opaque peer id) that may resolve later. */
interface MessageRecord {
  atMs: number;
  from: string;
  fromResolved: boolean;
  to: string;
  toResolved: boolean;
}

/** One end of a message a frame shows, keyed so its other ends find it. */
interface MessageFact {
  key: string;
  record: MessageRecord;
}

/** Messages each agent sent, per minute, derived once per version. */
interface DerivedSent {
  version: number;
  sent: Map<string, Map<number, number>>;
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

/** The envelope id a send tool's receipt names: the id the recipient's notice carries as its delivery id. */
function sentEnvelopeId(data: Data): string {
  for (const candidate of [data.result, data.content]) {
    const texts = Array.isArray(candidate)
      ? candidate.map((block) => textValue(recordOf(block).text))
      : [typeof candidate === "string" ? candidate : candidate && typeof candidate === "object" ? JSON.stringify(candidate) : ""];
    for (const text of texts) {
      if (!text || !text.includes("envelope_id")) continue;
      try {
        const parsed = recordOf(JSON.parse(text));
        const id = textValue(recordOf(parsed.receipt).envelope_id) || textValue(parsed.envelope_id);
        if (id) return id;
      } catch {
        // Not a JSON receipt.
      }
    }
  }
  return "";
}

/** The comms blocks of a notice frame (or of a saved message carrying one). */
function commsBlocksOf(data: Data): Data[] {
  const blocks: Data[] = [];
  for (const candidate of [data, recordOf(data.message)]) {
    if (textValue(candidate.type) === "comms") blocks.push(candidate);
    for (const block of Array.isArray(candidate.blocks) ? candidate.blocks : []) {
      const record = recordOf(block);
      if (textValue(record.type) === "comms") blocks.push(record);
    }
  }
  return blocks;
}

class Ledger implements ActivityLedger {
  version = 0;
  private identityRecords = new Map<string, IdentityRecord>();
  /** Messages by key, and the keys of their other ends. */
  private messages = new Map<string, MessageRecord>();
  private messageAliases = new Map<string, string>();
  private derived: DerivedSent | null = null;
  /** Peer ids to the names (or identities) they stand for. */
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
    this.messages.clear();
    this.messageAliases.clear();
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
    // Messages merge by their own keys, whichever copy of a frame shows them.
    changed = this.applyMessages(frame, data, identity, atMs) || changed;
    if (this.seen.has(classified.eventKey)) return changed;
    this.remember(classified.eventKey);
    if (identity) changed = this.count(identity, classified, frame, atMs) || changed;
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

  /** Learn peer ids' names: from the `peers` tool's results, and from every
   * comms notice, which names its peer by id and comms name. */
  private capturePeers(frame: ConsoleFrame, data: Data): boolean {
    const learned: Array<[string, string]> = [];
    if (frame.event === "tool_execution_completed" || frame.event === "tool_result_received") {
      if ((textValue(data.name) || textValue(data.tool_name)) === "peers") learned.push(...peersFromResult(data));
    } else if (frame.event === "system_notice") {
      for (const block of commsBlocksOf(data)) {
        const peer = recordOf(block.peer);
        const id = textValue(peer.id);
        const name = textValue(peer.display_name);
        if (id && name) learned.push([id, name]);
      }
    }
    let changed = false;
    for (const [id, name] of learned) {
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

  /** The ends of messages this frame shows: a send call, a send's receipt
   * (which ties the call to its envelope id), or comms notices. */
  private applyMessages(frame: ConsoleFrame, data: Data, identity: string, atMs: number): boolean {
    if (!identity) return false;
    const name = textValue(data.name) || textValue(data.tool_name);
    const callId = textValue(data.id) || textValue(data.tool_call_id);
    if (frame.event === "tool_call_requested" || frame.event === "tool_call") {
      if (!PEER_TOOLS.has(name)) return false;
      const args = recordOf(data.args);
      const to = textValue(args.display_name) || textValue(args.to) || textValue(args.identity)
        || textValue(args.target_identity) || textValue(args.recipient) || textValue(args.peer_id);
      if (!to) return false;
      return this.recordMessage({
        key: `call:${identity}:${callId || frame.id}`,
        record: { atMs, from: identity, fromResolved: true, to, toResolved: false },
      });
    }
    if ((frame.event === "tool_result_received" || frame.event === "tool_execution_completed") && PEER_TOOLS.has(name) && callId) {
      const envelope = sentEnvelopeId(data);
      return envelope ? this.linkMessages(`call:${identity}:${callId}`, `envelope:${envelope}`) : false;
    }
    if (frame.event !== "system_notice") return false;
    let changed = false;
    commsBlocksOf(data).forEach((block, index) => {
      const kind = textValue(block.kind);
      if (kind === "lifecycle" || kind === "response_progress") return;
      if (/^mob\.kickoff_/.test(textValue(block.intent))) return;
      const peer = recordOf(block.peer);
      const other = textValue(peer.display_name) || textValue(peer.id);
      if (!other) return;
      const outgoing = textValue(block.direction) === "outgoing";
      // An incoming notice's delivery id is the sender's envelope id.
      const delivery = textValue(block.delivery_id);
      const key = delivery && !outgoing
        ? `envelope:${delivery}`
        : `notice:${identity}:${outgoing ? "out" : "in"}:${delivery || textValue(block.request_id) || frame.id}:${index}`;
      changed = this.recordMessage({
        key,
        record: outgoing
          ? { atMs, from: identity, fromResolved: true, to: other, toResolved: false }
          : { atMs, from: other, fromResolved: false, to: identity, toResolved: true },
      }) || changed;
    });
    return changed;
  }

  private messageKey(key: string): string {
    let current = key;
    for (let hops = 0; hops < 8; hops++) {
      const next = this.messageAliases.get(current);
      if (!next || next === current) break;
      current = next;
    }
    return current;
  }

  private recordMessage(fact: MessageFact): boolean {
    const key = this.messageKey(fact.key);
    const known = this.messages.get(key);
    if (known) return this.mergeMessage(known, fact.record);
    this.messages.set(key, { ...fact.record });
    if (this.messages.size > MAX_MESSAGES) this.messages.delete(this.messages.keys().next().value!);
    return true;
  }

  /** Two keys name one message (a call and its receipt's envelope): one record, its ends merged. */
  private linkMessages(a: string, b: string): boolean {
    const first = this.messageKey(a);
    const second = this.messageKey(b);
    if (first === second) return false;
    const kept = this.messages.get(first);
    const merged = this.messages.get(second);
    this.messageAliases.set(second, first);
    if (this.messageAliases.size > MAX_MESSAGES * 2) this.messageAliases.delete(this.messageAliases.keys().next().value!);
    if (!merged) return false;
    this.messages.delete(second);
    if (!kept) {
      this.messages.set(first, merged);
      return true;
    }
    this.mergeMessage(kept, merged);
    return true;
  }

  /** Keep the known ends: a console identity over a peer reference. When a
   * peer id meets the identity it stands for, remember it. */
  private mergeMessage(kept: MessageRecord, other: MessageRecord): boolean {
    let changed = false;
    if (!kept.toResolved && other.toResolved) {
      if (kept.to !== other.to) this.peerNames.set(kept.to, other.to);
      kept.to = other.to;
      kept.toResolved = true;
      changed = true;
    }
    if (!kept.fromResolved && other.fromResolved) {
      if (kept.from !== other.from) this.peerNames.set(kept.from, other.from);
      kept.from = other.from;
      kept.fromResolved = true;
      changed = true;
    }
    if (other.atMs < kept.atMs) {
      kept.atMs = other.atMs;
      changed = true;
    }
    return changed;
  }

  /** A message end as an agent's identity: a console identity through the resolver; a peer reference through its name first. */
  private messageEnd(reference: string, resolved: boolean): string {
    return resolved ? this.canonical(reference) || reference : this.peerIdentity(reference);
  }

  /** Messages each agent sent, per minute, rebuilt when the ledger changed. */
  private derivedSent(): Map<string, Map<number, number>> {
    if (this.derived && this.derived.version === this.version) return this.derived.sent;
    const sent = new Map<string, Map<number, number>>();
    for (const message of this.messages.values()) {
      const from = this.messageEnd(message.from, message.fromResolved);
      const to = this.messageEnd(message.to, message.toResolved);
      if (!from || !to || from === to) continue;
      const minute = minuteOf(message.atMs);
      let byMinute = sent.get(from);
      if (!byMinute) {
        byMinute = new Map();
        sent.set(from, byMinute);
      }
      byMinute.set(minute, (byMinute.get(minute) ?? 0) + 1);
    }
    this.derived = { version: this.version, sent };
    return sent;
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
    for (const [key, message] of this.messages) {
      if (message.atMs < nowMs - LEDGER_WINDOW_MS) this.messages.delete(key);
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
    const add = (minute: number, n: number) => {
      const atMs = minute * LEDGER_BUCKET_MS;
      if (atMs < startMs || atMs >= endMs) return;
      out[Math.min(bins - 1, Math.floor((atMs - startMs) / binMs))] += n;
    };
    if (metric === "messagesSent") {
      for (const byMinute of this.sentFor(identity)) for (const [minute, n] of byMinute) add(minute, n);
      return out;
    }
    for (const record of this.recordsFor(identity)) {
      for (const [minute, counts] of record.buckets) add(minute, counts[metric]);
    }
    return out;
  }

  /** The per-minute counts of messages the agent sent (every agent's for null). */
  private sentFor(identity: string | null): Array<Map<number, number>> {
    const sent = this.derivedSent();
    if (identity === null) return [...sent.values()];
    const canonical = this.canonical(identity) || identity;
    const byMinute = sent.get(canonical);
    return byMinute ? [byMinute] : [];
  }

  totals(identity: string | null, window: LedgerWindow = {}): LedgerCounts {
    const sinceMs = window.sinceMs ?? Number.NEGATIVE_INFINITY;
    const untilMs = window.untilMs ?? Number.POSITIVE_INFINITY;
    const out = emptyCounts();
    const inWindow = (minute: number) => {
      const atMs = minute * LEDGER_BUCKET_MS;
      return atMs >= minuteOf(sinceMs) * LEDGER_BUCKET_MS && atMs <= untilMs;
    };
    for (const record of this.recordsFor(identity)) {
      for (const [minute, counts] of record.buckets) {
        if (!inWindow(minute)) continue;
        for (const metric of METRICS) if (metric !== "messagesSent") out[metric] += counts[metric];
      }
    }
    for (const byMinute of this.sentFor(identity)) {
      for (const [minute, n] of byMinute) if (inWindow(minute)) out.messagesSent += n;
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
    const sinceMs = window.sinceMs ?? Number.NEGATIVE_INFINITY;
    const untilMs = window.untilMs ?? Number.POSITIVE_INFINITY;
    const out: LedgerTraffic[] = [];
    // Ends resolve when read: a name the roster learned since counts with its agent.
    const pairs = new Map<string, LedgerTraffic>();
    for (const message of this.messages.values()) {
      if (message.atMs < sinceMs || message.atMs > untilMs) continue;
      const from = this.messageEnd(message.from, message.fromResolved);
      const to = this.messageEnd(message.to, message.toResolved);
      if (!from || !to || from === to) continue;
      const key = `${from}\u0000${to}`;
      const known = pairs.get(key);
      if (known) {
        known.count += 1;
        if (message.atMs > known.lastMs) known.lastMs = message.atMs;
      } else {
        pairs.set(key, { from, to, count: 1, lastMs: message.atMs });
      }
    }
    out.push(...pairs.values());
    return out.sort((a, b) => b.count - a.count || b.lastMs - a.lastMs).slice(0, MAX_PAIRS);
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
