// The Logs panel's model (console v2 spec B5): the exact record of events,
// one row per logical event (the shared activity entries), its level, the
// clock time to the millisecond, the expanded detail's summary and data,
// interaction groups and the severity strip's bins. Pure module (no React):
// the panel's node test bundle loads it.

import type { ConsoleFrame } from "../../types";
import {
  EMPTY_AGENT_DIRECTORY,
  agentDisplayName,
  type AgentDirectory,
} from "../../../../packages/console-components/src/panel-kit/agents";
import type { ActivityEntry } from "./activity-model";

export type LogLevel = "info" | "warn" | "error";

/** Stream control frames: never a log row. */
const INTERNAL_LOG_EVENTS = new Set([
  "keep-alive",
  "snapshot_complete",
  "snapshot_started",
  "subscribed",
]);

export function isLogFrameVisible(frame: ConsoleFrame): boolean {
  return !INTERNAL_LOG_EVENTS.has(frame.event);
}

/** An entry's level: the classifier's severity in the Logs words. */
export function logLevel(entry: Pick<ActivityEntry, "severity">): LogLevel {
  return entry.severity === "crit" ? "error" : entry.severity === "warn" ? "warn" : "info";
}

export const LEVEL_WORDS: Readonly<Record<LogLevel, string>> = {
  info: "Info",
  warn: "Warning",
  error: "Error",
};

/** A time on the 24-hour clock to the millisecond: "17:09:20.803". */
export function formatLogTime(tsMs?: number): string {
  if (!tsMs) return "-";
  const d = new Date(tsMs);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  const ms = String(d.getMilliseconds()).padStart(3, "0");
  return `${hh}:${mm}:${ss}.${ms}`;
}

/** The name the identity column shows: the directory's display name, the
 * key without its kind prefix, or "System" for console events. */
export function logIdentityLabel(identity: string | undefined, directory: AgentDirectory = EMPTY_AGENT_DIRECTORY): string {
  const key = identity?.trim();
  if (!key || key === "_system") return "System";
  return agentDisplayName(key, directory) || key;
}

const HIDDEN_HISTORY_BLOCK_TYPES = new Set([
  "reasoning",
  "server_tool_content",
  "tool_call",
  "tool_result",
  "tool_results",
  "tool_use",
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function historyBlockType(record: Record<string, unknown>): string | undefined {
  const raw = record.block_type ?? record.type;
  return typeof raw === "string" ? raw : undefined;
}

function sanitizeLogValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value
      .map(sanitizeLogValue)
      .filter((item) => item !== undefined);
  }
  if (!isRecord(value)) return value;

  const blockType = historyBlockType(value);
  if (blockType && HIDDEN_HISTORY_BLOCK_TYPES.has(blockType)) {
    return undefined;
  }

  const clean: Record<string, unknown> = {};
  for (const [key, child] of Object.entries(value)) {
    const sanitized = sanitizeLogValue(child);
    if (sanitized !== undefined) clean[key] = sanitized;
  }
  return clean;
}

/** A frame's data without reasoning and tool blocks of saved messages. */
export function sanitizeLogFrameData(data: unknown): unknown {
  return sanitizeLogValue(data) ?? null;
}

function textFromContentBlock(value: unknown): string | null {
  if (typeof value === "string") {
    const text = value.trim();
    return text ? text : null;
  }
  if (!isRecord(value)) return null;
  for (const key of ["text", "body", "content", "result", "summary"]) {
    const child = value[key];
    if (typeof child === "string" && child.trim()) {
      return child.trim();
    }
  }
  const data = value.data;
  if (isRecord(data)) {
    return textFromContentBlock(data);
  }
  return null;
}

function textFromContent(value: unknown): string | null {
  if (Array.isArray(value)) {
    const text = value
      .map(textFromContentBlock)
      .filter((part): part is string => Boolean(part))
      .join(" ")
      .replace(/\s+/g, " ")
      .trim();
    return text ? text : null;
  }
  return textFromContentBlock(value);
}

function preferredLogSummary(frame: ConsoleFrame, data: Record<string, unknown>): string | null {
  if (frame.event === "user_input") {
    const text = textFromContent(data.content ?? data.input ?? data.prompt);
    return text ? `input=${text.slice(0, 120)}` : null;
  }
  for (const key of ["result", "text", "summary", "body", "message_text"]) {
    const value = data[key];
    if (typeof value === "string" && value.trim()) {
      return `${key}=${value.trim().slice(0, 120)}`;
    }
  }
  const contentText = textFromContent(data.content);
  return contentText ? `content=${contentText.slice(0, 120)}` : null;
}

/** The frame's key fields in one line ("result=Visible answer."): the
 * expanded detail's summary. */
export function summarizeLogFrame(frame: ConsoleFrame): string {
  const sanitized = sanitizeLogFrameData(frame.data);
  const d = isRecord(sanitized) ? sanitized : {};
  const preferred = preferredLogSummary(frame, d);
  if (preferred) return preferred;
  const bits: string[] = [];
  for (const [k, v] of Object.entries(d).filter(([key]) => key !== "message").slice(0, 4)) {
    if (v === null || v === undefined) continue;
    let str: string;
    if (typeof v === "object") { try { str = JSON.stringify(v).slice(0, 40); } catch { str = "[obj]"; } }
    else str = String(v).slice(0, 60);
    bits.push(`${k}=${str}`);
  }
  return bits.join(" ");
}

/** A frame's data for the expanded row: JSON with a 2-space indent, capped
 * at about 10 KB so a giant tool result never blows up the panel. */
export function formatFrameData(frame: ConsoleFrame): string {
  const data = sanitizeLogFrameData(frame.data ?? null);
  if (data === null || data === undefined) return "(no data)";
  try {
    const out = JSON.stringify(data, null, 2);
    if (out.length > 10_000) return out.slice(0, 10_000) + "\n... (truncated)";
    return out;
  } catch {
    return String(data);
  }
}

/** Whether a frame carries a schema extraction result. */
export function hasStructuredOutput(frame: ConsoleFrame): boolean {
  const d = frame.data;
  if (!d || typeof d !== "object") return false;
  return (d as Record<string, unknown>).structured_output != null;
}

/** Counts per level. */
export function levelCounts(entries: readonly Pick<ActivityEntry, "severity">[]): Record<LogLevel, number> {
  const counts: Record<LogLevel, number> = { info: 0, warn: 0, error: 0 };
  for (const entry of entries) counts[logLevel(entry)] += 1;
  return counts;
}

export interface LogGroup {
  /** `interaction:<id>`, or `single:<entry key>` for an event of no interaction. */
  key: string;
  interactionId?: string;
  actor: string | null;
  /** Newest first, as the rows read. */
  entries: ActivityEntry[];
  startMs: number;
  endMs: number;
  level: LogLevel;
}

/** Group by interaction (Logs' "Group by interaction"): consecutive and
 * scattered events of one interaction in one group, groups newest first. */
export function groupByInteraction(entries: readonly ActivityEntry[]): LogGroup[] {
  const groups = new Map<string, LogGroup>();
  const order: LogGroup[] = [];
  const rank: Readonly<Record<LogLevel, number>> = { info: 0, warn: 1, error: 2 };
  for (const entry of entries) {
    const key = entry.interactionId ? `interaction:${entry.interactionId}` : `single:${entry.key}`;
    let group = groups.get(key);
    if (!group) {
      group = {
        key,
        ...(entry.interactionId ? { interactionId: entry.interactionId } : {}),
        actor: entry.actor,
        entries: [],
        startMs: entry.atMs,
        endMs: entry.atMs,
        level: "info",
      };
      groups.set(key, group);
      order.push(group);
    }
    group.entries.push(entry);
    group.startMs = Math.min(group.startMs, entry.atMs);
    group.endMs = Math.max(group.endMs, entry.atMs);
    group.actor = group.actor ?? entry.actor;
    const level = logLevel(entry);
    if (rank[level] > rank[group.level]) group.level = level;
  }
  return order.sort((a, b) => b.endMs - a.endMs);
}

/** A duration in words for the Logs column: "4.2 s", "850 ms", "2 min". */
export function formatLogDuration(ms: number | undefined): string {
  if (ms === undefined || !Number.isFinite(ms) || ms < 0) return "";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)} s`;
  const minutes = Math.round(ms / 60_000);
  return `${minutes} min`;
}
