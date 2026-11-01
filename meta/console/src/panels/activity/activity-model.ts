// The activity model the Today, Logs and Signals panels share (console v2
// spec B4-B6): every frame the console holds, from the live buffer and the
// agents' history logs, read once through the shared frame classifier and
// kept as one entry per logical event. A frame and its saved history copy
// are one entry, and so are the two ends of one message between agents
// (the sender's call and the recipient's notice), so no panel lists an event
// twice. Pure module (no React): node test bundles load it too.

import type { ConsoleFrame } from "../../types";
import {
  classifyFrame,
  frameSentence,
  type ClassifiedFrame,
  type FrameFamily,
  type FrameSegment,
  type FrameSeverity,
  type FrameSpan,
} from "../../lib/frame-classifier";
import { consoleFrameRowKey } from "../../../../packages/console-core/src/timeline-recovery";
import {
  EMPTY_AGENT_DIRECTORY,
  agentDisplayName,
  type AgentDirectory,
} from "../../../../packages/console-components/src/panel-kit/agents";
import type { Tone } from "../../../../packages/console-components/src/panel-kit/tone";

type Data = Record<string, unknown>;

export interface ActivityEntry {
  /** The event's key: the classifier's event key, or for a message the key
   * its two ends share. Unique in a list. */
  key: string;
  /** The React key of the row showing it: the shown copy's id and version. */
  rowKey: string;
  /** The copy the row shows: a live frame before a saved history copy, and
   * for a message the sender's call. */
  frame: ConsoleFrame;
  /** Every copy of the event the console holds, oldest first. */
  frames: ConsoleFrame[];
  /** When it happened: the earliest copy's time. */
  atMs: number;
  classified: ClassifiedFrame;
  family: FrameFamily;
  familyWord: string;
  severity: FrameSeverity;
  tone: Tone;
  title: string;
  /** The sentence, agents by key. */
  segments: FrameSegment[];
  snippet?: string;
  /** The agent the event belongs to (its lane); null for the console's and
   * the system's own events. */
  actor: string | null;
  /** The other agent: a message's recipient. */
  target?: string;
  span?: FrameSpan;
  durationMs?: number;
  interactionId?: string;
  runId?: string;
}

const PEER_TOOLS: ReadonlySet<string> = new Set(["send_message", "send_request", "send_response", "reply_to_peer"]);
/** How far apart the two ends of one message may be stamped. */
const MESSAGE_TWIN_MS = 120_000;

function recordOf(value: unknown): Data {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Data : {};
}

function textValue(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function frameAgent(frame: ConsoleFrame): string | null {
  const identity = frame.identity?.trim();
  return identity && identity !== "_system" ? identity : null;
}

/** The run a frame belongs to, from the frame or its payload. */
export function frameRunId(frame: ConsoleFrame): string | undefined {
  const data = recordOf(frame.data);
  return frame.runId?.trim() || textValue(data.run_id) || textValue(recordOf(data.identity).run_id) || undefined;
}

/** The interaction (turn) a frame belongs to, from the frame or its payload. */
export function frameInteractionId(frame: ConsoleFrame): string | undefined {
  const data = recordOf(frame.data);
  return frame.interactionId?.trim() || textValue(data.interaction_id) || textValue(recordOf(data.identity).interaction_id) || undefined;
}

/** The comms blocks of a notice frame (or of a saved message carrying one). */
function commsBlocks(data: Data): Data[] {
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

/** JSON texts a tool result carries (its result string and text blocks). */
function resultTexts(data: Data): string[] {
  const texts: string[] = [];
  for (const candidate of [data.result, data.content]) {
    if (typeof candidate === "string") texts.push(candidate);
    else if (Array.isArray(candidate)) {
      for (const block of candidate) {
        const text = textValue(recordOf(block).text);
        if (text) texts.push(text);
      }
    }
  }
  return texts;
}

function parseJson(text: string): Data | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("{")) return null;
  try {
    return recordOf(JSON.parse(trimmed));
  } catch {
    return null;
  }
}

/** The envelope id a send's receipt names, per sender and call: the id the
 * recipient's notice carries as its delivery id. */
function learnEnvelopes(frames: readonly ConsoleFrame[]): Map<string, string> {
  const envelopes = new Map<string, string>();
  for (const frame of frames) {
    if (frame.event !== "tool_result_received" && frame.event !== "tool_execution_completed") continue;
    const data = recordOf(frame.data);
    if (!PEER_TOOLS.has(textValue(data.name) || textValue(data.tool_name))) continue;
    const callId = textValue(data.id) || textValue(data.tool_call_id);
    const agent = frameAgent(frame);
    if (!callId || !agent) continue;
    for (const text of resultTexts(data)) {
      if (!text.includes("envelope_id")) continue;
      const parsed = parseJson(text);
      const envelope = textValue(recordOf(parsed?.receipt).envelope_id) || textValue(parsed?.envelope_id);
      if (envelope) {
        envelopes.set(`${agent}\u0000${callId}`, envelope);
        break;
      }
    }
  }
  return envelopes;
}

/** A message's words, compared across its two ends: no envelope line, no
 * case, no spacing. */
function messageWords(value: string | undefined): string {
  return (value ?? "")
    .replace(/^\s*(?:Peer\s+(?:message|request|response)\s+from\s+[^:\n]+:|\[COMMS\s+\w+\s+from\s+[^\]]+\])\s*/i, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .trim()
    .slice(0, 100);
}

interface MessageEnd {
  /** "send": the sender's call or outgoing notice; "receive": the recipient's notice. */
  end: "send" | "receive";
  sender: string | null;
  recipient: string;
  /** The envelope both ends name, when known. */
  envelope: string;
  words: string;
}

function messageEndOf(
  frame: ConsoleFrame,
  classified: ClassifiedFrame,
  envelopes: ReadonlyMap<string, string>,
): MessageEnd | null {
  if (classified.family !== "message") return null;
  const data = recordOf(frame.data);
  if (frame.event === "system_notice") {
    const block = commsBlocks(data).find((candidate) => {
      const kind = textValue(candidate.kind);
      return kind !== "lifecycle" && kind !== "response_progress";
    });
    if (!block) return null;
    const outgoing = textValue(block.direction) === "outgoing";
    return {
      end: outgoing ? "send" : "receive",
      sender: classified.actor,
      recipient: classified.target ?? "",
      envelope: outgoing ? "" : textValue(block.delivery_id),
      words: messageWords(classified.snippet),
    };
  }
  const agent = frameAgent(frame);
  const callId = textValue(data.id) || textValue(data.tool_call_id);
  return {
    end: "send",
    sender: agent,
    // The classifier names the recipient through the directory (comms peer
    // ids included); the other end's notice names it otherwise.
    recipient: classified.target ?? "",
    envelope: agent && callId ? envelopes.get(`${agent}\u0000${callId}`) ?? "" : "",
    words: messageWords(classified.snippet),
  };
}

/** The sentence with a message's recipient named where the classifier could
 * only say "a peer". */
function withRecipient(segments: FrameSegment[], recipient: string | undefined): FrameSegment[] {
  if (!recipient) return segments;
  const index = segments.lastIndexOf("a peer");
  if (index < 0) return segments;
  const next = segments.slice();
  next[index] = { agent: recipient };
  return next;
}

/** A copy's rank as the one a row shows: live and sent copies before saved
 * history copies, a message's sending end before its receiving end. */
function copyRank(frame: ConsoleFrame, end: MessageEnd | null): number {
  let rank = 0;
  if (frame.sourceKind === "session_history") rank += 2;
  if (end?.end === "receive") rank += 4;
  return rank;
}

const SEVERITY_ORDER: Readonly<Record<FrameSeverity, number>> = { info: 0, warn: 1, crit: 2 };

interface Building extends ActivityEntry {
  message: MessageEnd | null;
  rank: number;
}

function entryOf(
  frame: ConsoleFrame,
  classified: ClassifiedFrame,
  atMs: number,
  message: MessageEnd | null,
): Building {
  const target = message ? message.recipient || classified.target : classified.target;
  return {
    key: message?.envelope ? `message:${message.envelope}` : classified.eventKey,
    rowKey: consoleFrameRowKey(frame),
    frame,
    frames: [frame],
    atMs,
    classified,
    family: classified.family,
    familyWord: classified.familyWord,
    severity: classified.severity,
    tone: classified.tone,
    title: classified.title,
    segments: withRecipient(classified.segments, target),
    ...(classified.snippet ? { snippet: classified.snippet } : {}),
    actor: message ? message.sender ?? classified.actor : classified.actor,
    ...(target ? { target } : {}),
    ...(classified.span ? { span: classified.span } : {}),
    ...(classified.durationMs !== undefined ? { durationMs: classified.durationMs } : {}),
    ...(frameInteractionId(frame) ? { interactionId: frameInteractionId(frame) } : {}),
    ...(frameRunId(frame) ? { runId: frameRunId(frame) } : {}),
    message,
    rank: copyRank(frame, message),
  };
}

/** Fold another copy of the same event into an entry. */
function merge(into: Building, other: Building): void {
  if (!into.frames.some((frame) => consoleFrameRowKey(frame) === other.rowKey)) {
    into.frames.push(...other.frames);
    into.frames.sort((a, b) => (a.timestampMs ?? 0) - (b.timestampMs ?? 0));
  }
  const target = into.target || other.target;
  const keepOther = other.rank < into.rank;
  if (keepOther) {
    const keepActor = into.actor ?? other.actor;
    into.frame = other.frame;
    into.rowKey = other.rowKey;
    into.classified = other.classified;
    into.family = other.family;
    into.familyWord = other.familyWord;
    into.title = other.title;
    into.segments = other.segments;
    into.snippet = other.snippet ?? into.snippet;
    into.span = other.span ?? into.span;
    into.interactionId = other.interactionId ?? into.interactionId;
    into.runId = other.runId ?? into.runId;
    into.rank = other.rank;
    into.message = other.message ?? into.message;
    into.actor = other.actor ?? keepActor;
  } else {
    into.snippet = into.snippet ?? other.snippet;
    into.interactionId = into.interactionId ?? other.interactionId;
    into.runId = into.runId ?? other.runId;
    into.actor = into.actor ?? other.actor;
  }
  if (target) into.target = target;
  into.segments = withRecipient(into.segments, into.target);
  if (other.durationMs !== undefined && into.durationMs === undefined) into.durationMs = other.durationMs;
  if (SEVERITY_ORDER[other.severity] > SEVERITY_ORDER[into.severity]) {
    into.severity = other.severity;
    into.tone = other.tone;
  }
  into.atMs = Math.min(into.atMs, other.atMs);
}

export interface BuildActivityOptions {
  directory?: AgentDirectory;
  /** Keep plumbing frames (snapshots, deltas): Logs' "Show internal events". */
  includeInternal?: boolean;
}

/**
 * One entry per logical event, newest first. Frames without a time are
 * left out (nothing can place them). The two ends of a message merge by the
 * envelope id the send's receipt and the recipient's notice share, or, when
 * the receipt is not loaded, by sender and words within two minutes. Names
 * (a send's recipient by its comms peer id) resolve through `directory`.
 */
export function buildActivityEntries(frames: readonly ConsoleFrame[], options: BuildActivityOptions = {}): ActivityEntry[] {
  const envelopes = learnEnvelopes(frames);
  const byKey = new Map<string, Building>();
  const unlinkedMessages = new Map<string, Building[]>();
  for (const frame of frames) {
    const atMs = frame.timestampMs;
    if (typeof atMs !== "number" || !Number.isFinite(atMs)) continue;
    const classified = classifyFrame(frame, options.directory);
    if (classified.internal && !options.includeInternal) continue;
    const message = messageEndOf(frame, classified, envelopes);
    const candidate = entryOf(frame, classified, atMs, message);
    const known = byKey.get(candidate.key);
    if (known) {
      merge(known, candidate);
      continue;
    }
    if (message && message.sender && message.words) {
      // The other end of this message, when no envelope ties them: the same
      // sender and words, stamped close together, from the other side.
      const wordsKey = `${message.sender}\u0000${message.words}`;
      const twins = unlinkedMessages.get(wordsKey) ?? [];
      const twin = twins.find((other) => other.message && other.message.end !== message.end
        && Math.abs(other.atMs - atMs) <= MESSAGE_TWIN_MS
        && (!other.message.envelope || !message.envelope || other.message.envelope === message.envelope));
      if (twin) {
        merge(twin, candidate);
        if (message.envelope && !byKey.has(`message:${message.envelope}`)) byKey.set(`message:${message.envelope}`, twin);
        continue;
      }
      twins.push(candidate);
      unlinkedMessages.set(wordsKey, twins);
    }
    byKey.set(candidate.key, candidate);
  }
  const seen = new Set<Building>();
  const entries: ActivityEntry[] = [];
  for (const entry of byKey.values()) {
    if (seen.has(entry)) continue;
    seen.add(entry);
    const { message: _message, rank: _rank, ...plain } = entry;
    entries.push(plain);
  }
  return entries.sort((a, b) => b.atMs - a.atMs || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
}

/**
 * Every frame of the given sources, once: a frame two sources share (the
 * live buffer and an agent's log hold the same object) or one delivered
 * twice keeps its newest version.
 */
export function mergeFrameSources(...sources: ReadonlyArray<readonly ConsoleFrame[] | null | undefined>): ConsoleFrame[] {
  const byKey = new Map<string, ConsoleFrame>();
  for (const source of sources) {
    for (const frame of source ?? []) {
      const key = frame.id ? `${frame.id}\u0000${frame.event}` : consoleFrameRowKey(frame);
      const held = byKey.get(key);
      if (!held || (frame.frameVersion ?? 0) > (held.frameVersion ?? 0)
        || ((frame.frameVersion ?? 0) === (held.frameVersion ?? 0) && (frame.updatedAtMs ?? 0) > (held.updatedAtMs ?? 0))) {
        byKey.set(key, frame);
      }
    }
  }
  return [...byKey.values()];
}

/** The sentence in words, agents named by `nameOf` ("System" for none). */
export function entrySentence(entry: Pick<ActivityEntry, "segments">, nameOf: (agent: string) => string): string {
  return frameSentence(entry, nameOf);
}

/** A display-name function over a directory. */
export function agentNamer(directory: AgentDirectory = EMPTY_AGENT_DIRECTORY): (agent: string) => string {
  return (agent) => agentDisplayName(agent, directory) || agent;
}

/** Whether an entry matches a search: its sentence, excerpt, title and family words. */
export function entryMatches(entry: ActivityEntry, query: string, nameOf: (agent: string) => string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  const haystack = [
    entrySentence(entry, nameOf),
    entry.snippet ?? "",
    entry.title,
    entry.familyWord,
    entry.frame.event,
  ].join("\n").toLowerCase();
  return haystack.includes(needle);
}

/** Whether a frame is worth a problem mark: a warning or a failure. */
export function isProblem(entry: Pick<ActivityEntry, "severity">): boolean {
  return entry.severity !== "info";
}

/** The stronger of two severities. */
export function strongerSeverity(a: FrameSeverity, b: FrameSeverity): FrameSeverity {
  return SEVERITY_ORDER[a] >= SEVERITY_ORDER[b] ? a : b;
}

/** A severity as the tone of its mark. */
export function severityTone(severity: FrameSeverity): Tone {
  return severity === "crit" ? "crit" : severity === "warn" ? "warn" : "neutral";
}
