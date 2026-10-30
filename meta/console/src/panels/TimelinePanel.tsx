import React from "react";
import { describeFailure } from "../lib/failure-summary";
import { turnSender, turnSenderWords } from "../lib/turn-origin";
import { stripPeerTransportScaffold } from "../lib/adapters";
import type { ConsoleFrame } from "../types";
import {
  EmptyState,
  PanelBody,
  PanelHeader,
  PanelShell,
  StatusDot,
  agentDisplayName,
  formatAbsoluteTime,
  formatClockTime,
  formatShortDate,
  humanizeEnum,
  joinMeta,
  normalizeStatusWord,
  plural,
  useAgentDirectory,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";

interface TimelinePanelProps {
  frames: ConsoleFrame[];
}

const INTERNAL_TIMELINE_EVENTS = new Set([
  "keep-alive",
  "snapshot_complete",
  "snapshot_started",
  "subscribed",
]);

/** Most events the day view lists. */
const MAX_ENTRIES = 80;
/** Longest reply or request excerpt shown after a sentence. */
const SNIPPET_MAX = 140;
const PEER_TOOLS = new Set(["send_request", "send_message", "send_response"]);

/** Part of a sentence: plain words, or an agent shown by display name
 * (null for events without one, which read "System"). */
type Segment = string | { agent: string | null };

/** One event of the day in words: a sentence, an optional excerpt, a tone
 * for its dot and a one-word kind. Interaction IDs never appear. */
export interface TodayLine {
  segments: Segment[];
  snippet?: string;
  tone: Tone;
  kind: string;
}

function recordOf(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

/** Readable text from a payload: strings, arrays of parts, and assistant
 * messages (their text blocks only, never reasoning or tool blocks). */
function textOf(value: unknown): string {
  if (typeof value === "string") return value.trim();
  if (Array.isArray(value)) return value.map(textOf).filter(Boolean).join(" ").trim();
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    if (Array.isArray(record.blocks)) {
      return record.blocks
        .map((block) => {
          const part = recordOf(block);
          const type = part.block_type ?? part.type;
          return type === "text" ? textOf(recordOf(part.data).text ?? part.text) : "";
        })
        .filter(Boolean)
        .join(" ")
        .trim();
    }
    return textOf(record.text ?? record.result ?? record.summary ?? record.body ?? record.content);
  }
  return "";
}

function snippetOf(value: string): string | undefined {
  const text = stripPeerTransportScaffold(value).replace(/\s+/g, " ").trim();
  if (!text || /^You have been spawned as\b/i.test(text)) return undefined;
  return text.length > SNIPPET_MAX ? `${text.slice(0, SNIPPET_MAX - 3).trimEnd()}...` : text;
}

function lastPathSegment(value: string): string {
  return value.split("/").pop() || value;
}

function peerTarget(args: Record<string, unknown>): string {
  for (const key of ["display_name", "to"]) {
    const value = args[key];
    if (typeof value === "string" && value.trim()) return lastPathSegment(value.trim());
  }
  return "";
}

function memoryLine(event: string, data: Record<string, unknown>, who: Segment): TodayLine {
  const kind = "Memory";
  const reason = snippetOf(textOf(data.reason));
  switch (event) {
    case "memory.dream.completed": return { segments: [who, " finished a memory dream"], tone: "ok", kind };
    case "memory.dream.started": return { segments: [who, " started a memory dream"], tone: "neutral", kind };
    case "memory.dream.skipped": return { segments: [who, " skipped a memory dream"], snippet: reason, tone: "neutral", kind };
    case "memory.record.promoted": return { segments: [who, " promoted a memory record"], tone: "ok", kind };
    case "memory.write.quarantined":
      return { segments: ["A memory write from ", who, " was quarantined"], snippet: reason, tone: "warn", kind };
    case "memory.quarantine.verdict": {
      const verdict = textOf(data.verdict);
      return { segments: [who, " reviewed a quarantined record"], snippet: verdict ? humanizeEnum(verdict) : undefined, tone: "neutral", kind };
    }
    case "memory.quarantine.release_blocked":
      return { segments: ["A quarantine release by ", who, " was blocked"], tone: "warn", kind };
    case "memory.conflict.signal": return { segments: [who, " flagged a memory conflict"], snippet: reason, tone: "warn", kind };
    case "memory.budget.denied": return { segments: [who, " reached the memory budget"], tone: "warn", kind };
    case "memory.taint.transition":
      return data.kind === "tainted"
        ? { segments: [who, "'s session memory was marked tainted"], tone: "warn", kind }
        : { segments: [who, `'s session memory: ${humanizeEnum(textOf(data.kind) || "changed").toLowerCase()}`], tone: "neutral", kind };
    default:
      return { segments: [who, `: memory ${humanizeEnum(event.slice("memory.".length)).toLowerCase()}`], tone: "neutral", kind };
  }
}

/** The sentence for one frame. Agents are named through the directory when
 * rendered; frames without an agent read "System". */
export function describeTodayFrame(frame: ConsoleFrame): TodayLine {
  const data = recordOf(frame.data);
  const identity = frame.identity?.trim();
  const who: Segment = { agent: identity && identity !== "_system" ? identity : null };
  switch (frame.event) {
    case "interaction_complete":
      return {
        segments: [who, " finished a turn"],
        snippet: snippetOf(textOf(data.result ?? data.text ?? data.message ?? data.content)),
        tone: "ok",
        kind: "Turn",
      };
    case "interaction_started":
      return {
        segments: [who, " started a turn"],
        snippet: snippetOf(textOf(data.content ?? data.text ?? data.prompt ?? data.input)),
        tone: "info",
        kind: "Turn",
      };
    case "user_input": {
      // The typed origin names the sender: you, another operator, a
      // scheduled run, a connector.
      const words = turnSenderWords(turnSender(data));
      return {
        segments: words.after ? [words.before, who, words.after] : [words.before, who],
        snippet: snippetOf(textOf(data.content ?? data.input ?? data.prompt ?? data.text)),
        tone: "info",
        kind: "Turn",
      };
    }
    case "interaction_failed":
    case "run_failed":
      return { segments: [who, "'s turn failed"], snippet: snippetOf(describeFailure(data, "")), tone: "crit", kind: "Problem" };
    case "gating_decision": {
      const decision = normalizeStatusWord(String(data.decision || ""));
      const action = typeof data.action_id === "string" ? humanizeEnum(data.action_id) : "";
      const base = { snippet: action || undefined, kind: "Approval" };
      if (decision === "requested" || decision === "pending") return { ...base, segments: [who, " asked for approval"], tone: "warn" };
      if (["approve", "approved", "auto_approved", "allowed"].includes(decision)) {
        return { ...base, segments: ["Approval granted to ", who], tone: "ok" };
      }
      if (["reject", "rejected", "denied"].includes(decision)) return { ...base, segments: ["Approval refused for ", who], tone: "crit" };
      if (decision === "escalate" || decision === "escalated") return { ...base, segments: ["Approval for ", who, " was escalated"], tone: "warn" };
      return {
        ...base,
        segments: ["Approval decision for ", who, decision ? `: ${humanizeEnum(decision).toLowerCase()}` : ""],
        tone: "neutral",
      };
    }
    case "member_ready": return { segments: [who, " is ready"], tone: "ok", kind: "Lifecycle" };
    case "member_retired":
      return { segments: [who, " was retired"], snippet: snippetOf(textOf(data.reason)), tone: "neutral", kind: "Lifecycle" };
    case "state_changed": {
      const state = textOf(data.state ?? data.new_state);
      return { segments: [who, state ? ` is now ${humanizeEnum(state).toLowerCase()}` : " changed state"], tone: "neutral", kind: "Lifecycle" };
    }
    case "route_changed":
      return { segments: ["Routing changed"], snippet: snippetOf(textOf(data.reason)), tone: "info", kind: "Routing" };
    case "topology_updated":
      return { segments: ["The team layout changed"], tone: "info", kind: "Routing" };
    case "tool_call_requested": {
      const name = typeof data.name === "string" ? data.name : "";
      const args = recordOf(data.args);
      if (PEER_TOOLS.has(name)) {
        const target = peerTarget(args);
        const verb = name === "send_request" ? " asked " : name === "send_response" ? " replied to " : " messaged ";
        return {
          segments: [who, verb, target ? { agent: target } : "a peer"],
          snippet: snippetOf(textOf(args.body ?? args.params ?? args.result ?? args.intent)),
          tone: "neutral",
          kind: "Message",
        };
      }
      return { segments: [who, ` used ${humanizeEnum(name).toLowerCase() || "a tool"}`], tone: "neutral", kind: "Tool" };
    }
    default:
      if (frame.event.startsWith("memory.")) return memoryLine(frame.event, data, who);
      return { segments: [who, `: ${humanizeEnum(frame.event).toLowerCase()}`], tone: "neutral", kind: "Activity" };
  }
}

function startOfToday(): number {
  const day = new Date();
  day.setHours(0, 0, 0, 0);
  return day.getTime();
}

export function TimelinePanel({ frames }: TimelinePanelProps): React.JSX.Element {
  const directory = useAgentDirectory();
  const entries = React.useMemo(() => {
    const todayMs = startOfToday();
    return frames
      .filter((frame) => !INTERNAL_TIMELINE_EVENTS.has(frame.event))
      .filter((frame) => (frame.timestampMs || 0) >= todayMs)
      .slice(0, MAX_ENTRIES)
      .map((frame, index) => ({
        key: frame.id || `${frame.event}:${frame.timestampMs}:${index}`,
        ms: frame.timestampMs as number,
        line: describeTodayFrame(frame),
      }));
  }, [frames]);

  const now = Date.now();
  const renderSegment = (segment: Segment, index: number) =>
    typeof segment === "string" ? (
      <React.Fragment key={index}>{segment}</React.Fragment>
    ) : segment.agent === null ? (
      <span key={index} className="today__who">System</span>
    ) : (
      <span key={index} className="today__who" title={segment.agent}>
        {agentDisplayName(segment.agent, directory) || segment.agent}
      </span>
    );

  return (
    <PanelShell className="today" testId="timeline-panel">
      <PanelHeader title="Today" subtitle={joinMeta(plural(entries.length, "event"), formatShortDate(now, now))} />
      <PanelBody>
        {entries.length === 0 ? (
          <EmptyState
            title="Nothing has happened yet today"
            body="Turns, approvals, messages and changes to the team appear here as they happen."
          />
        ) : (
          <ol className="today__list">
            {entries.map(({ key, ms, line }, index) => (
              // A frame can arrive twice (live and history copies): the
              // position keeps the keys apart.
              <li className="today__row" key={`${key}:${index}`}>
                <time className="today__time" dateTime={new Date(ms).toISOString()} title={formatAbsoluteTime(ms)}>
                  {formatClockTime(ms)}
                </time>
                <span className="today__rail">
                  <StatusDot tone={line.tone} />
                </span>
                <span className="today__body">
                  <span className="today__text">
                    {line.segments.map(renderSegment)}
                    {line.snippet ? (
                      <>
                        {": "}
                        <span className="today__snippet">{line.snippet}</span>
                      </>
                    ) : null}
                  </span>
                  <span className="today__kind">{line.kind}</span>
                </span>
              </li>
            ))}
          </ol>
        )}
      </PanelBody>
    </PanelShell>
  );
}
